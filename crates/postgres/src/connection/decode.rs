//! Row value decoding helpers for [`PostgresConnection`].
//!
//! Each `handle_*_type` method takes a row and column index and returns the
//! string representation of that cell, with `None` meaning SQL NULL. The
//! top-level entry point is `convert_row_value_to_string`, which routes by
//! `ColumnType` to the per-type handlers.

use blanco_core::ColumnType;
use sqlx::{Row, ValueRef};

use super::PostgresConnection;

pub(super) fn format_pg_interval(interval: sqlx::postgres::types::PgInterval) -> String {
    let mut parts: Vec<String> = Vec::new();
    let years = interval.months / 12;
    let months = interval.months % 12;
    if years != 0 {
        parts.push(format!(
            "{} {}",
            years,
            if years == 1 { "year" } else { "years" }
        ));
    }
    if months != 0 {
        parts.push(format!(
            "{} {}",
            months,
            if months == 1 { "mon" } else { "mons" }
        ));
    }
    if interval.days != 0 {
        parts.push(format!(
            "{} {}",
            interval.days,
            if interval.days == 1 { "day" } else { "days" }
        ));
    }

    if interval.microseconds != 0 || parts.is_empty() {
        let negative = interval.microseconds < 0;
        let total = interval.microseconds.unsigned_abs();
        let micros_per_second: u64 = 1_000_000;
        let micros_per_minute: u64 = 60 * micros_per_second;
        let micros_per_hour: u64 = 60 * micros_per_minute;
        let hours = total / micros_per_hour;
        let minutes = (total % micros_per_hour) / micros_per_minute;
        let seconds = (total % micros_per_minute) / micros_per_second;
        let micros = total % micros_per_second;
        let time = if micros == 0 {
            format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
        } else {
            format!("{:02}:{:02}:{:02}.{:06}", hours, minutes, seconds, micros)
                .trim_end_matches('0')
                .to_string()
        };
        parts.push(if negative { format!("-{}", time) } else { time });
    }

    parts.join(" ")
}

impl PostgresConnection {
    /// Extract the base type from an array type (e.g., "TEXT[]" -> Some("TEXT"))
    fn extract_base_array_type(array_type: &str) -> Option<&str> {
        array_type.strip_suffix("[]")
    }

    /// Check if a value is NULL without attempting type conversion.
    ///
    /// Returns `true` only when the wire value is definitively NULL. If we
    /// cannot even acquire the raw value, treat the cell as non-NULL so the
    /// decoder still gets a chance and we don't silently turn decode errors
    /// into NULLs.
    fn is_null_value(row: &sqlx::postgres::PgRow, column_index: usize) -> bool {
        match row.try_get_raw(column_index) {
            Ok(raw_value) => raw_value.is_null(),
            Err(_) => false,
        }
    }

    /// Read the column's raw bytes as text. Used as a last-chance decoder
    /// for non-NULL values that none of the typed decoders handled. Returns
    /// `Some(string)` if the bytes are valid UTF-8, or a visible
    /// `[!type: ...]` marker carrying a hex preview otherwise. Never
    /// returns `None` for a non-NULL value, so a real SQL NULL and a decode
    /// failure are always distinguishable in the UI.
    fn as_text_or_marker(
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        column_type: &str,
    ) -> Option<String> {
        let raw_value = match row.try_get_raw(column_index) {
            Ok(v) => v,
            Err(e) => {
                return Some(format!("[!{column_type}: cannot read raw value: {e}]"));
            }
        };
        if raw_value.is_null() {
            return None;
        }
        if let Ok(text) = raw_value.as_str() {
            return Some(text.to_string());
        }
        match raw_value.as_bytes() {
            Ok(bytes) => match std::str::from_utf8(bytes) {
                Ok(s) => Some(s.to_string()),
                Err(_) => {
                    let hex_preview: String = bytes
                        .iter()
                        .take(20)
                        .map(|b| format!("{:02x}", b))
                        .collect();
                    Some(format!(
                        "[!{column_type}: {} bytes, hex {}{}]",
                        bytes.len(),
                        hex_preview,
                        if bytes.len() > 20 { "..." } else { "" }
                    ))
                }
            },
            Err(e) => Some(format!("[!{column_type}: cannot read bytes: {e}]")),
        }
    }

    /// Map PostgreSQL type name to ColumnType enum.
    ///
    /// The set of types here is the result of comparing what
    /// `pg_catalog.pg_type` exposes against what we used to handle. Many
    /// PostgreSQL types (network, geometric, ranges, ts*, regX OID aliases,
    /// xml, bit strings, jsonpath, pg_lsn, etc.) come back over the wire as
    /// readable text. Mapping them to `Text` (rather than `Unknown`) routes
    /// them through the string decoder, which is fast and avoids a
    /// "unknown column type" warning per row.
    pub(super) fn map_postgres_type(type_name: &str) -> ColumnType {
        let lower = type_name.to_lowercase();
        match lower.as_str() {
            // Integers (signed + serial)
            "smallint" | "int2" | "smallserial" | "int" | "int4" | "integer" | "serial"
            | "bigint" | "int8" | "bigserial" => ColumnType::Integer,
            // PostgreSQL object identifier integers
            "oid" | "xid" | "xid8" | "cid" | "tid" => ColumnType::Integer,
            // Numerics
            "real" | "float4" | "double precision" | "float8" | "numeric" | "decimal" | "money" => {
                ColumnType::Numeric
            }
            "boolean" | "bool" => ColumnType::Boolean,
            // Character / text family (note: "char" is the 1-byte type, char is bpchar)
            "text" | "varchar" | "character varying" | "char" | "\"char\"" | "bpchar"
            | "character" | "name" => ColumnType::Text,
            // Date / time
            "timestamp" | "timestamp without time zone" | "timestamptz"
            | "timestamp with time zone" | "date" | "time" | "time without time zone" | "timetz"
            | "time with time zone" | "interval" => ColumnType::DateTime,
            "uuid" => ColumnType::Uuid,
            "json" | "jsonb" | "jsonpath" => ColumnType::Json,
            "bytea" => ColumnType::Binary,
            "array" => ColumnType::Array,
            _ if lower.ends_with("[]") => ColumnType::Array,
            // Network address types: come back as text
            "inet" | "cidr" | "macaddr" | "macaddr8" => ColumnType::Text,
            // Bit strings: text representation is "1010..."
            "bit" | "varbit" | "bit varying" => ColumnType::Text,
            // Geometric types: rendered as text like "(1,2),(3,4)"
            "point" | "line" | "lseg" | "box" | "path" | "polygon" | "circle" => ColumnType::Text,
            // Full-text search
            "tsvector" | "tsquery" | "gtsvector" => ColumnType::Text,
            // Ranges and multiranges (built-in)
            "int4range" | "int8range" | "numrange" | "daterange" | "tsrange" | "tstzrange"
            | "int4multirange" | "int8multirange" | "nummultirange" | "datemultirange"
            | "tsmultirange" | "tstzmultirange" => ColumnType::Text,
            // System catalog identifier aliases (regclass, regtype, etc.)
            "regclass" | "regcollation" | "regconfig" | "regdictionary" | "regnamespace"
            | "regoper" | "regoperator" | "regproc" | "regprocedure" | "regrole" | "regtype" => {
                ColumnType::Text
            }
            // Misc text-renderable
            "xml" | "pg_lsn" | "aclitem" | "pg_snapshot" | "txid_snapshot" => ColumnType::Text,
            _ => ColumnType::Unknown,
        }
    }

    /// Handle array types with proper base type detection
    fn handle_array_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        column_type: &str,
    ) -> Option<String> {
        let base_type = Self::extract_base_array_type(column_type);

        match base_type {
            Some("text") | Some("varchar") | Some("char") | Some("BPCHAR") | Some("TEXT")
            | Some("VARCHAR") | Some("CHAR") | Some("NAME") => {
                if let Ok(array_val) = row.try_get::<Option<Vec<String>>, _>(column_index) {
                    return array_val.map(|v| {
                        format!(
                            "{{{}}}",
                            v.iter()
                                .map(|x| format!("\"{}\"", x))
                                .collect::<Vec<_>>()
                                .join(",")
                        )
                    });
                }
            }
            Some("int4") | Some("integer") | Some("int") | Some("INT4") => {
                if let Ok(array_val) = row.try_get::<Option<Vec<i32>>, _>(column_index) {
                    return array_val.map(|v| {
                        format!(
                            "{{{}}}",
                            v.iter()
                                .map(|x| x.to_string())
                                .collect::<Vec<_>>()
                                .join(",")
                        )
                    });
                }
            }
            Some("int8") | Some("bigint") | Some("INT8") => {
                if let Ok(array_val) = row.try_get::<Option<Vec<i64>>, _>(column_index) {
                    return array_val.map(|v| {
                        format!(
                            "{{{}}}",
                            v.iter()
                                .map(|x| x.to_string())
                                .collect::<Vec<_>>()
                                .join(",")
                        )
                    });
                }
            }
            Some("int2") | Some("smallint") | Some("INT2") => {
                if let Ok(array_val) = row.try_get::<Option<Vec<i16>>, _>(column_index) {
                    return array_val.map(|v| {
                        format!(
                            "{{{}}}",
                            v.iter()
                                .map(|x| x.to_string())
                                .collect::<Vec<_>>()
                                .join(",")
                        )
                    });
                }
            }
            Some("float4") | Some("real") | Some("FLOAT4") => {
                if let Ok(array_val) = row.try_get::<Option<Vec<f32>>, _>(column_index) {
                    return array_val.map(|v| {
                        format!(
                            "{{{}}}",
                            v.iter()
                                .map(|x| x.to_string())
                                .collect::<Vec<_>>()
                                .join(",")
                        )
                    });
                }
            }
            Some("float8") | Some("double precision") | Some("FLOAT8") => {
                if let Ok(array_val) = row.try_get::<Option<Vec<f64>>, _>(column_index) {
                    return array_val.map(|v| {
                        format!(
                            "{{{}}}",
                            v.iter()
                                .map(|x| x.to_string())
                                .collect::<Vec<_>>()
                                .join(",")
                        )
                    });
                }
            }
            Some("bool") | Some("boolean") | Some("BOOL") => {
                if let Ok(array_val) = row.try_get::<Option<Vec<bool>>, _>(column_index) {
                    return array_val.map(|v| {
                        format!(
                            "{{{}}}",
                            v.iter()
                                .map(|x| x.to_string())
                                .collect::<Vec<_>>()
                                .join(",")
                        )
                    });
                }
            }
            Some("uuid") | Some("UUID") => {
                if let Ok(array_val) = row.try_get::<Option<Vec<uuid::Uuid>>, _>(column_index) {
                    return array_val.map(|v| {
                        format!(
                            "{{{}}}",
                            v.iter()
                                .map(|x| x.to_string())
                                .collect::<Vec<_>>()
                                .join(",")
                        )
                    });
                }
            }
            _ => {
                if let Ok(array_val) = row.try_get::<Option<String>, _>(column_index) {
                    return array_val;
                }
            }
        }

        tracing::warn!(
            "Array column type '{}' couldn't be converted to any supported array type",
            column_type
        );
        None
    }

    fn handle_uuid_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        _column_type: &str,
    ) -> Option<String> {
        if let Ok(val) = row.try_get::<Option<uuid::Uuid>, _>(column_index) {
            val.map(|v| v.to_string())
        } else {
            None
        }
    }

    fn handle_string_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        column_type: &str,
    ) -> Option<String> {
        if let Ok(val) = row.try_get::<Option<String>, _>(column_index) {
            return val;
        }
        // Many types route to ColumnType::Text but sqlx doesn't have a typed
        // String decoder for them (xml, inet, point, ranges, ...). Fall back
        // to reading raw bytes so they still display.
        Self::as_text_or_marker(row, column_index, column_type)
    }

    fn handle_bool_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        _column_type: &str,
    ) -> Option<String> {
        if let Ok(val) = row.try_get::<Option<bool>, _>(column_index) {
            val.map(|v| v.to_string())
        } else {
            None
        }
    }

    fn handle_integer_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        raw_type: &str,
    ) -> Option<String> {
        match raw_type.to_lowercase().as_str() {
            "smallint" | "int2" | "smallserial" => {
                if let Ok(val) = row.try_get::<Option<i16>, _>(column_index) {
                    return val.map(|v| v.to_string());
                }
            }
            "integer" | "int" | "int4" | "serial" => {
                if let Ok(val) = row.try_get::<Option<i32>, _>(column_index) {
                    return val.map(|v| v.to_string());
                }
            }
            "bigint" | "int8" | "bigserial" => {
                if let Ok(val) = row.try_get::<Option<i64>, _>(column_index) {
                    return val.map(|v| v.to_string());
                }
            }
            _ => {
                if let Ok(val) = row.try_get::<Option<i16>, _>(column_index) {
                    return val.map(|v| v.to_string());
                }
                if let Ok(val) = row.try_get::<Option<i32>, _>(column_index) {
                    return val.map(|v| v.to_string());
                }
                if let Ok(val) = row.try_get::<Option<i64>, _>(column_index) {
                    return val.map(|v| v.to_string());
                }
            }
        }
        None
    }

    fn handle_numeric_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        raw_type: &str,
    ) -> Option<String> {
        match raw_type.to_lowercase().as_str() {
            "real" | "float4" => {
                if let Ok(val) = row.try_get::<Option<f32>, _>(column_index) {
                    return val.map(|v| v.to_string());
                }
            }
            "double precision" | "float8" => {
                if let Ok(val) = row.try_get::<Option<f64>, _>(column_index) {
                    return val.map(|v| v.to_string());
                }
            }
            "numeric" | "decimal" | "money" => {
                if let Ok(val) = row.try_get::<Option<rust_decimal::Decimal>, _>(column_index) {
                    return val.map(|v| v.to_string());
                }
            }
            _ => {
                if let Ok(val) = row.try_get::<Option<rust_decimal::Decimal>, _>(column_index) {
                    return val.map(|v| v.to_string());
                }
                if let Ok(val) = row.try_get::<Option<f64>, _>(column_index) {
                    return val.map(|v| v.to_string());
                }
            }
        }
        None
    }

    fn handle_timestamp_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        raw_type: &str,
    ) -> Option<String> {
        match raw_type.to_lowercase().as_str() {
            "timestamptz" => {
                if let Ok(val) =
                    row.try_get::<Option<chrono::DateTime<chrono::Local>>, _>(column_index)
                {
                    return val.map(|v| v.to_rfc3339());
                }
            }
            "timestamp" => {
                if let Ok(val) = row.try_get::<Option<chrono::NaiveDateTime>, _>(column_index) {
                    return val.map(|v| v.format("%Y-%m-%d %H:%M:%S").to_string());
                }
            }
            "date" => {
                if let Ok(val) = row.try_get::<Option<chrono::NaiveDate>, _>(column_index) {
                    return val.map(|v| v.format("%Y-%m-%d").to_string());
                }
            }
            "time" => {
                if let Ok(val) = row.try_get::<Option<chrono::NaiveTime>, _>(column_index) {
                    return val.map(|v| v.format("%H:%M:%S").to_string());
                }
            }
            "timetz" => {
                if let Ok(val) = row.try_get::<Option<chrono::NaiveTime>, _>(column_index) {
                    return val.map(|v| v.format("%H:%M:%S").to_string());
                }
            }
            "interval" => {
                // sqlx's PgInterval decoder only handles the binary protocol, but
                // `raw_sql` queries come back in text mode. Read the raw value and
                // fall back to the binary decoder when needed.
                if let Ok(raw_value) = row.try_get_raw(column_index) {
                    if raw_value.is_null() {
                        return None;
                    }
                    if let Ok(text) = raw_value.as_str() {
                        return Some(text.to_string());
                    }
                }
                if let Ok(val) =
                    row.try_get::<Option<sqlx::postgres::types::PgInterval>, _>(column_index)
                {
                    return val.map(format_pg_interval);
                }
            }
            _ => {}
        }
        None
    }

    fn handle_json_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        _column_type: &str,
    ) -> Option<String> {
        if let Ok(val) = row.try_get::<Option<serde_json::Value>, _>(column_index) {
            return val.map(|v| {
                if v.is_string() {
                    v.as_str().unwrap_or("").to_string()
                } else {
                    v.to_string()
                }
            });
        }
        self.try_string_conversion(row, column_index, "json")
    }

    fn try_string_conversion(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        column_type: &str,
    ) -> Option<String> {
        if let Ok(val) = row.try_get::<Option<String>, _>(column_index) {
            return val;
        }

        match column_type {
            "regclass" => {
                if let Ok(oid_val) = row.try_get::<Option<i32>, _>(column_index) {
                    return oid_val.map(|oid| format!("OID:{}", oid));
                }
                if let Ok(oid_val) = row.try_get::<i32, _>(column_index) {
                    return Some(format!("OID:{}", oid_val));
                }
            }
            _ => {
                if let Ok(val) = row.try_get::<String, _>(column_index) {
                    return Some(val);
                }
            }
        }

        tracing::warn!("Failed to convert column type '{}' to string", column_type);
        None
    }

    fn handle_unknown_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        column_type: &str,
    ) -> Option<String> {
        Self::as_text_or_marker(row, column_index, column_type)
    }

    /// Convert a PostgreSQL row value to string representation. `None` means SQL NULL.
    pub(super) fn convert_row_value_to_string(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        column_types: &[ColumnType],
        raw_column_types: &[String],
    ) -> Option<String> {
        let column_type = column_types
            .get(column_index)
            .copied()
            .unwrap_or(ColumnType::Unknown);

        let raw_type = raw_column_types
            .get(column_index)
            .map(|s| s.as_str())
            .unwrap_or("");

        if Self::is_null_value(row, column_index) {
            return None;
        }

        let decoded = match column_type {
            ColumnType::Array => self.handle_array_type(row, column_index, raw_type),
            ColumnType::Integer | ColumnType::UnsignedInteger => {
                self.handle_integer_type(row, column_index, raw_type)
            }
            ColumnType::Numeric => self.handle_numeric_type(row, column_index, raw_type),
            ColumnType::Boolean => self.handle_bool_type(row, column_index, raw_type),
            ColumnType::Text => self.handle_string_type(row, column_index, raw_type),
            ColumnType::DateTime => self.handle_timestamp_type(row, column_index, raw_type),
            ColumnType::Uuid => self.handle_uuid_type(row, column_index, raw_type),
            ColumnType::Json => self.handle_json_type(row, column_index, raw_type),
            ColumnType::Binary => self.handle_unknown_type(row, column_index, raw_type),
            ColumnType::Unknown => self.handle_unknown_type(row, column_index, raw_type),
        };

        // The cell is known to be non-NULL (we checked above). If a typed
        // decoder returned None it means the typed decode failed — fall back
        // to raw text so we never conflate decode failure with SQL NULL.
        decoded.or_else(|| Self::as_text_or_marker(row, column_index, raw_type))
    }
}
