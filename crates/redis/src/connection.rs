use anyhow::{Result, anyhow};
use async_trait::async_trait;
use blanco_core::{Connection, KeyValueResult, QueryResult, RedisType, RedisValue};
use fred::prelude::*;
use fred::types::{ClusterHash, CustomCommand};

use crate::command::{key_to_string, reply_to_query_result, tokenize, value_to_string};

/// Number of logical databases to assume when `CONFIG GET databases` is
/// unavailable (the Redis default).
const DEFAULT_DATABASE_COUNT: u8 = 16;

/// Upper bound on keys returned by a single `get_tables` SCAN sweep so a large
/// keyspace can't stall the sidebar.
const SCAN_KEY_CAP: usize = 1000;

pub struct RedisConnection {
    client: Client,
    display_name: String,
}

impl RedisConnection {
    /// Build (but do not yet connect) a Redis client from a `redis://` /
    /// `rediss://` URL. Call [`Connection::connect`] to establish the link.
    pub fn from_connection_string(connection_string: &str) -> Result<Self> {
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

    /// Iterate the keyspace with SCAN, returning up to `SCAN_KEY_CAP` keys
    /// matching `pattern`.
    async fn scan_keys(&self, pattern: &str) -> Result<Vec<String>> {
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
            if cursor == "0" || keys.len() >= SCAN_KEY_CAP {
                break;
            }
        }
        keys.truncate(SCAN_KEY_CAP);
        Ok(keys)
    }

    /// Read the decoded value of `key`, dispatching on its Redis type.
    async fn decode_value(&self, key_type: RedisType, key: &str) -> Result<RedisValue> {
        let value = match key_type {
            RedisType::String => {
                let reply = self
                    .run_command(&["GET".to_string(), key.to_string()])
                    .await?;
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
        Ok(value)
    }
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
        self.scan_keys("*").await
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

        let value = self.decode_value(key_type, key).await?;

        Ok(KeyValueResult {
            key: key.to_string(),
            key_type,
            ttl,
            value,
        })
    }
}
