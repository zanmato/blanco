use anyhow::{Result, anyhow};
use async_trait::async_trait;
use blanco_core::{Connection, KeyListing, KeyValueResult, QueryResult, RedisType, RedisValue};
use fred::prelude::*;
use fred::types::{ClusterHash, CustomCommand};

use crate::command::{key_to_string, reply_to_query_result, tokenize, value_to_string};

/// Number of logical databases to assume when `CONFIG GET databases` is
/// unavailable (the Redis default).
const DEFAULT_DATABASE_COUNT: u8 = 16;

/// Upper bound on keys returned by a single `get_tables` SCAN sweep so a large
/// keyspace can't stall the sidebar.
const SCAN_KEY_CAP: usize = 1000;

/// The `client_name` query parameter of a connection URL is Blanco's, not
/// the driver's: it is taken off and sent with `CLIENT SETNAME`.
const CLIENT_NAME_PARAM: &str = "client_name";

pub struct RedisConnection {
    client: Client,
    display_name: String,
    client_name: Option<String>,
}

impl RedisConnection {
    /// Build (but do not yet connect) a Redis client from a `redis://` /
    /// `rediss://` URL. Call [`Connection::connect`] to establish the link.
    pub fn from_connection_string(connection_string: &str) -> Result<Self> {
        let (connection_string, client_name) = take_client_name(connection_string);
        let connection_string = connection_string.as_str();
        let config = Config::from_url(connection_string)
            .map_err(|e| anyhow!("invalid Redis connection URL: {e}"))?;
        let client = Builder::from_config(config)
            .build()
            .map_err(|e| anyhow!("failed to build Redis client: {e}"))?;

        // Derive a display name from the URL without leaking the password.
        let display_name = match url::Url::parse(connection_string) {
            Ok(url) => {
                let host = url.host_str().unwrap_or("localhost");
                let port = url.port().unwrap_or(6379);
                let db = url.path().trim_start_matches('/');
                if db.is_empty() {
                    format!("{host}:{port}")
                } else {
                    format!("{host}:{port}/{db}")
                }
            }
            Err(_) => connection_string.to_string(),
        };

        Ok(Self {
            client,
            display_name,
            client_name,
        })
    }

    /// Run a raw command (already tokenized into name + args) and return the
    /// decoded reply.
    async fn run_command(&self, parts: &[String]) -> Result<Value> {
        let (name, args) = parts
            .split_first()
            .ok_or_else(|| anyhow!("empty command"))?;
        let custom = CustomCommand::new(name.to_uppercase(), ClusterHash::FirstKey, false);
        let args: Vec<Value> = args.iter().map(|a| Value::from(a.as_str())).collect();
        let reply: Value = self
            .client
            .custom(custom, args)
            .await
            .map_err(|e| anyhow!("{e}"))?;
        Ok(reply)
    }

    /// Run a single command line: tokenize then execute.
    async fn run_line(&self, line: &str) -> Result<Value> {
        let parts = tokenize(line)?;
        if parts.is_empty() {
            return Err(anyhow!("empty command"));
        }
        self.run_command(&parts).await
    }

    /// Iterate the keyspace with SCAN, returning up to `limit` keys matching
    /// `pattern` and whether more matched.
    async fn scan_keys(&self, pattern: &str, limit: usize) -> Result<KeyListing> {
        let mut cursor = "0".to_string();
        let mut keys = Vec::new();
        loop {
            let parts = vec![
                "SCAN".to_string(),
                cursor.clone(),
                "MATCH".to_string(),
                pattern.to_string(),
                "COUNT".to_string(),
                "500".to_string(),
            ];
            let reply = self.run_command(&parts).await?;
            let Value::Array(items) = reply else {
                break;
            };
            cursor = items
                .first()
                .and_then(value_to_string)
                .unwrap_or_else(|| "0".to_string());
            if let Some(Value::Array(batch)) = items.get(1) {
                for key in batch {
                    if let Some(key) = value_to_string(key) {
                        keys.push(key);
                    }
                }
            }
            if cursor == "0" || keys.len() > limit {
                break;
            }
        }
        // A sweep that stopped early, or found more than asked for, leaves
        // keys behind.
        let truncated = cursor != "0" || keys.len() > limit;
        keys.truncate(limit);
        Ok(KeyListing { keys, truncated })
    }

    /// Name this client connection for `CLIENT LIST`.
    async fn set_client_name(&self) {
        if let Some(name) = &self.client_name
            && let Err(error) = self
                .run_command(&["CLIENT".into(), "SETNAME".into(), name.clone()])
                .await
        {
            tracing::warn!("Failed to set the Redis client name: {error:#}");
        }
    }

    /// Read the decoded value of `key`, dispatching on its Redis type.
    async fn decode_value(
        &self,
        key_type: RedisType,
        key: &str,
    ) -> Result<(RedisValue, Option<String>)> {
        let value = match key_type {
            RedisType::String => {
                let reply = self
                    .run_command(&["GET".to_string(), key.to_string()])
                    .await?;
                let raw: &[u8] = match &reply {
                    Value::Bytes(bytes) => bytes,
                    Value::String(text) => text.as_bytes(),
                    _ => &[],
                };
                if let Some((compression, text)) = decompress(raw) {
                    return Ok((RedisValue::Str(text), Some(compression.to_string())));
                }
                RedisValue::Str(value_to_string(&reply).unwrap_or_default())
            }
            RedisType::List => {
                let reply = self
                    .run_command(&[
                        "LRANGE".to_string(),
                        key.to_string(),
                        "0".to_string(),
                        "-1".to_string(),
                    ])
                    .await?;
                RedisValue::List(array_to_strings(reply))
            }
            RedisType::Set => {
                let reply = self
                    .run_command(&["SMEMBERS".to_string(), key.to_string()])
                    .await?;
                RedisValue::Set(array_to_strings(reply))
            }
            RedisType::Hash => {
                let reply = self
                    .run_command(&["HGETALL".to_string(), key.to_string()])
                    .await?;
                RedisValue::Hash(reply_to_field_pairs(reply))
            }
            RedisType::ZSet => {
                let reply = self
                    .run_command(&[
                        "ZRANGE".to_string(),
                        key.to_string(),
                        "0".to_string(),
                        "-1".to_string(),
                        "WITHSCORES".to_string(),
                    ])
                    .await?;
                let members = reply_to_field_pairs(reply)
                    .into_iter()
                    .map(|(member, score)| (member, score.parse::<f64>().unwrap_or(0.0)))
                    .collect();
                RedisValue::ZSet(members)
            }
            RedisType::Stream => {
                let reply = self
                    .run_command(&[
                        "XRANGE".to_string(),
                        key.to_string(),
                        "-".to_string(),
                        "+".to_string(),
                    ])
                    .await?;
                RedisValue::Stream(parse_stream_entries(reply))
            }
            RedisType::None => RedisValue::None,
        };
        Ok((value, None))
    }
}

/// Split Blanco's `client_name` parameter off a connection URL.
fn take_client_name(connection_string: &str) -> (String, Option<String>) {
    let Ok(mut url) = url::Url::parse(connection_string) else {
        return (connection_string.to_string(), None);
    };
    let mut client_name = None;
    let others: Vec<(String, String)> = url
        .query_pairs()
        .filter_map(|(key, value)| {
            if key == CLIENT_NAME_PARAM {
                client_name = Some(value.to_string()).filter(|name| !name.is_empty());
                None
            } else {
                Some((key.to_string(), value.to_string()))
            }
        })
        .collect();
    if others.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(others);
    }
    (url.to_string(), client_name)
}

/// The value inside a gzip, zstd or lz4 frame, recognised by its magic bytes,
/// with the compression's name. `None` for anything else, or a frame whose
/// content is not text.
fn decompress(bytes: &[u8]) -> Option<(&'static str, String)> {
    use std::io::Read as _;

    // Decompressed values are capped so a hostile value cannot exhaust memory.
    const LIMIT: u64 = 64 * 1024 * 1024;
    let mut out = Vec::new();
    let compression = if bytes.starts_with(&[0x1f, 0x8b]) {
        flate2::read::GzDecoder::new(bytes)
            .take(LIMIT)
            .read_to_end(&mut out)
            .ok()?;
        "gzip"
    } else if bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        zstd::stream::read::Decoder::new(bytes)
            .ok()?
            .take(LIMIT)
            .read_to_end(&mut out)
            .ok()?;
        "zstd"
    } else if bytes.starts_with(&[0x04, 0x22, 0x4d, 0x18]) {
        lz4_flex::frame::FrameDecoder::new(bytes)
            .take(LIMIT)
            .read_to_end(&mut out)
            .ok()?;
        "lz4"
    } else {
        return None;
    };
    Some((compression, String::from_utf8(out).ok()?))
}

/// Flatten a Redis array reply into display strings, dropping nils.
fn array_to_strings(value: Value) -> Vec<String> {
    match value {
        Value::Array(items) => items.iter().filter_map(value_to_string).collect(),
        other => value_to_string(&other).into_iter().collect(),
    }
}

/// Decode a field/value reply that may arrive as a RESP3 map or a flat RESP2
/// array `[f1, v1, f2, v2, ...]` into ordered pairs.
fn reply_to_field_pairs(value: Value) -> Vec<(String, String)> {
    match value {
        Value::Map(map) => map
            .iter()
            .map(|(k, v)| (key_to_string(k), value_to_string(v).unwrap_or_default()))
            .collect(),
        Value::Array(items) => items
            .chunks(2)
            .filter_map(|chunk| match chunk {
                [field, val] => Some((
                    value_to_string(field).unwrap_or_default(),
                    value_to_string(val).unwrap_or_default(),
                )),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Parse an `XRANGE` reply: an array of `[id, [f1, v1, f2, v2, ...]]` entries.
fn parse_stream_entries(value: Value) -> Vec<(String, Vec<(String, String)>)> {
    let Value::Array(entries) = value else {
        return Vec::new();
    };
    entries
        .into_iter()
        .filter_map(|entry| {
            let Value::Array(parts) = entry else {
                return None;
            };
            let mut parts = parts.into_iter();
            let id = value_to_string(&parts.next()?).unwrap_or_default();
            let fields = parts.next().map(reply_to_field_pairs).unwrap_or_default();
            Some((id, fields))
        })
        .collect()
}

#[async_trait]
impl Connection for RedisConnection {
    fn database_type(&self) -> blanco_core::DatabaseType {
        blanco_core::DatabaseType::Redis
    }

    fn get_display_name(&self) -> String {
        self.display_name.clone()
    }

    async fn connect(&mut self, _connection_string: &str) -> Result<()> {
        self.client.init().await.map_err(|e| anyhow!("{e}"))?;
        self.set_client_name().await;
        // A reconnect opens a new server connection, which has no name yet.
        if let Some(name) = self.client_name.clone() {
            let client = self.client.clone();
            self.client.on_reconnect(move |_| {
                let client = client.clone();
                let name = name.clone();
                async move {
                    let custom = CustomCommand::new("CLIENT", ClusterHash::FirstKey, false);
                    let _: Value = client
                        .custom(
                            custom,
                            vec![Value::from("SETNAME"), Value::from(name.as_str())],
                        )
                        .await
                        .unwrap_or(Value::Null);
                    Ok(())
                }
            });
        }
        Ok(())
    }

    async fn ping(&self) -> Result<()> {
        self.run_command(&["PING".to_string()]).await.map(|_| ())
    }

    async fn execute_query(
        &self,
        query: &str,
        _database_name: Option<&str>,
        _parameters: Option<&[String]>,
    ) -> Result<QueryResult> {
        let reply = self.run_line(query).await?;
        Ok(reply_to_query_result(query.trim(), reply))
    }

    async fn execute_script(
        &self,
        query: &str,
        _database_name: Option<&str>,
    ) -> Result<Vec<QueryResult>> {
        let mut results = Vec::new();
        for line in query.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let reply = self.run_line(line).await?;
            results.push(reply_to_query_result(line, reply));
        }
        if results.is_empty() {
            return Err(anyhow!("no command to run"));
        }
        Ok(results)
    }

    async fn execute_write(
        &self,
        query: &str,
        _database_name: Option<&str>,
        _parameters: &[Option<String>],
    ) -> Result<u64> {
        let reply = self.run_line(query).await?;
        // Many write commands (DEL, LPUSH, HSET, SADD…) reply with an affected
        // count; treat a non-integer reply (e.g. OK) as a single change.
        match reply {
            Value::Integer(n) => Ok(n.max(0) as u64),
            _ => Ok(1),
        }
    }

    async fn get_databases(&self) -> Result<Vec<String>> {
        // Try CONFIG GET databases; fall back to the Redis default of 16.
        let count = match self
            .run_command(&[
                "CONFIG".to_string(),
                "GET".to_string(),
                "databases".to_string(),
            ])
            .await
        {
            Ok(Value::Array(items)) => items
                .get(1)
                .and_then(value_to_string)
                .and_then(|s| s.parse::<u8>().ok())
                .unwrap_or(DEFAULT_DATABASE_COUNT),
            Ok(Value::Map(map)) => map
                .iter()
                .find(|(k, _)| key_to_string(k) == "databases")
                .and_then(|(_, v)| value_to_string(v))
                .and_then(|s| s.parse::<u8>().ok())
                .unwrap_or(DEFAULT_DATABASE_COUNT),
            _ => DEFAULT_DATABASE_COUNT,
        };
        Ok((0..count).map(|i| i.to_string()).collect())
    }

    async fn get_schemas(&self) -> Result<Vec<String>> {
        // Redis has no schema layer, but the schema-less sidebar loader iterates
        // get_schemas() and calls get_tables() once per entry. Return a single
        // placeholder so the current database's keys are listed; the name is not
        // rendered (schema-less trees flatten keys into category folders).
        Ok(vec!["default".to_string()])
    }

    async fn get_tables(&self, _schema: Option<&str>) -> Result<Vec<String>> {
        Ok(self.scan_keys("*", SCAN_KEY_CAP).await?.keys)
    }

    async fn list_keys(&self, pattern: &str, limit: usize) -> Result<KeyListing> {
        let pattern = if pattern.trim().is_empty() {
            "*"
        } else {
            pattern
        };
        self.scan_keys(pattern, limit).await
    }

    async fn set_key_ttl(&self, key: &str, ttl: Option<i64>) -> Result<()> {
        let parts = match ttl {
            Some(seconds) => vec!["EXPIRE".to_string(), key.to_string(), seconds.to_string()],
            None => vec!["PERSIST".to_string(), key.to_string()],
        };
        self.run_command(&parts).await.map(|_| ())
    }

    fn supports_schemas(&self) -> bool {
        false
    }

    async fn inspect_key(&self, _database_name: Option<&str>, key: &str) -> Result<KeyValueResult> {
        let type_reply = self
            .run_command(&["TYPE".to_string(), key.to_string()])
            .await?;
        let key_type =
            RedisType::from_type_reply(&value_to_string(&type_reply).unwrap_or_default());

        // TTL in seconds: -1 = no expiry, -2 = key missing; both map to None.
        let ttl_reply = self
            .run_command(&["TTL".to_string(), key.to_string()])
            .await?;
        let ttl = match ttl_reply {
            Value::Integer(seconds) if seconds >= 0 => Some(seconds),
            _ => None,
        };

        let (value, compression) = self.decode_value(key_type, key).await?;

        Ok(KeyValueResult {
            key: key.to_string(),
            key_type,
            ttl,
            value,
            compression,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_name_is_taken_off_the_url() {
        let (url, name) = take_client_name("redis://localhost:6379/0?client_name=blanco&x=1");
        assert_eq!(name.as_deref(), Some("blanco"));
        assert_eq!(url, "redis://localhost:6379/0?x=1");
        let (url, name) = take_client_name("redis://localhost:6379/0");
        assert_eq!((url.as_str(), name), ("redis://localhost:6379/0", None));
    }

    #[test]
    fn compressed_values_are_recognised() {
        use std::io::Write as _;

        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(b"hello").unwrap();
        let gzip = gzip.finish().unwrap();
        assert_eq!(decompress(&gzip), Some(("gzip", "hello".to_string())));

        let zstd = zstd::encode_all(&b"hello"[..], 0).unwrap();
        assert_eq!(decompress(&zstd), Some(("zstd", "hello".to_string())));

        let mut lz4 = lz4_flex::frame::FrameEncoder::new(Vec::new());
        lz4.write_all(b"hello").unwrap();
        let lz4 = lz4.finish().unwrap();
        assert_eq!(decompress(&lz4), Some(("lz4", "hello".to_string())));

        assert_eq!(decompress(b"plain text"), None);
    }

    /// A Redis to test against: `REDIS_TEST_URL`, by default a local server on
    /// port 6390 using database 15. Missing servers skip the test unless
    /// `BLANCO_RUN_DB_TESTS=1`.
    async fn test_connection(client_name: Option<&str>) -> Option<RedisConnection> {
        let url = std::env::var("REDIS_TEST_URL")
            .unwrap_or_else(|_| "redis://127.0.0.1:6390/15".to_string());
        let url = match client_name {
            Some(name) => format!("{url}?client_name={name}"),
            None => url,
        };
        let mut connection = RedisConnection::from_connection_string(&url).ok()?;
        let connected = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Connection::connect(&mut connection, &url),
        )
        .await;
        match connected {
            Ok(Ok(())) => Some(connection),
            _ if std::env::var("BLANCO_RUN_DB_TESTS").as_deref() == Ok("1") => {
                panic!("Redis unreachable at {url}")
            }
            _ => None,
        }
    }

    #[tokio::test]
    async fn keys_listed_by_pattern_and_page() {
        let Some(redis) = test_connection(None).await else {
            return;
        };
        // Tests share the database, so these keys have their own prefix.
        let keys = ["list:a:1", "list:a:2", "list:a:3", "list:a:4", "list:b:1"];
        for key in keys {
            redis.run_line(&format!("SET {key} x")).await.unwrap();
        }

        let page = redis.list_keys("list:a:*", 3).await.unwrap();
        assert_eq!(page.keys.len(), 3);
        assert!(page.keys.iter().all(|key| key.starts_with("list:a:")));
        assert!(page.truncated);

        let all = redis.list_keys("list:b:*", 10).await.unwrap();
        assert_eq!(
            (all.keys, all.truncated),
            (vec!["list:b:1".to_string()], false)
        );
        redis
            .run_line(&format!("DEL {}", keys.join(" ")))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn ttl_is_set_and_removed() {
        let Some(redis) = test_connection(None).await else {
            return;
        };
        redis.run_line("SET ttl:key x").await.unwrap();
        redis.set_key_ttl("ttl:key", Some(100)).await.unwrap();
        let ttl = redis.inspect_key(None, "ttl:key").await.unwrap().ttl;
        assert!(ttl.is_some_and(|ttl| (90..=100).contains(&ttl)), "{ttl:?}");
        redis.set_key_ttl("ttl:key", None).await.unwrap();
        assert_eq!(redis.inspect_key(None, "ttl:key").await.unwrap().ttl, None);
        redis.run_line("DEL ttl:key").await.unwrap();
    }

    #[tokio::test]
    async fn client_name_is_set_on_connect() {
        let Some(redis) = test_connection(Some("blanco-test")).await else {
            return;
        };
        let name = redis.run_line("CLIENT GETNAME").await.unwrap();
        assert_eq!(value_to_string(&name).as_deref(), Some("blanco-test"));
    }

    #[tokio::test]
    async fn compressed_strings_are_shown_decompressed() {
        use std::io::Write as _;

        let Some(redis) = test_connection(None).await else {
            return;
        };
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(br#"{"user": 1}"#).unwrap();
        let _: () = redis
            .client
            .set("gz:key", gzip.finish().unwrap(), None, None, false)
            .await
            .unwrap();

        let inspected = redis.inspect_key(None, "gz:key").await.unwrap();
        assert_eq!(inspected.compression.as_deref(), Some("gzip"));
        assert_eq!(inspected.value, RedisValue::Str(r#"{"user": 1}"#.into()));
        redis.run_line("DEL gz:key").await.unwrap();
    }
}
