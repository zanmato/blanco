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

    /// Check if a value is NULL without attempting type conversion
    fn is_null_value(row: &sqlx::postgres::PgRow, column_index: usize) -> bool {
        if let Ok(raw_value) = row.try_get_raw(column_index) {
            raw_value.is_null()
        } else {
            false
        }
    }

    /// Map PostgreSQL type name to ColumnType enum
    pub(super) fn map_postgres_type(type_name: &str) -> ColumnType {
        match type_name.to_lowercase().as_str() {
            "smallint" | "int2" | "int" | "int4" | "integer" | "bigint" | "int8" | "serial"
            | "bigserial" => ColumnType::Integer,
            "real" | "float4" | "double precision" | "float8" | "numeric" | "decimal" | "money" => {
                ColumnType::Numeric
            }
            "boolean" | "bool" => ColumnType::Boolean,
            "text" | "varchar" | "character varying" | "char" | "bpchar" | "name" => {
                ColumnType::Text
            }
            "timestamp" | "timestamptz" | "date" | "time" | "timetz" | "interval" => {
                ColumnType::DateTime
            }
            "uuid" => ColumnType::Uuid,
            "json" | "jsonb" => ColumnType::Json,
            "array" => ColumnType::Array,
            _ if type_name.to_lowercase().ends_with("[]") => ColumnType::Array,
            "bytea" => ColumnType::Binary,
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
        _column_type: &str,
    ) -> Option<String> {
        row.try_get::<Option<String>, _>(column_index)
            .unwrap_or_default()
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
        if let Ok(raw_value) = row.try_get_raw(column_index) {
            if raw_value.is_null() {
                return None;
            }

            match raw_value.as_str() {
                Ok(text_val) => Some(text_val.to_string()),
                Err(_) => match raw_value.as_bytes() {
                    Ok(bytes) => match String::from_utf8(bytes.to_vec()) {
                        Ok(string_val) => Some(string_val),
                        Err(_) => {
                            let hex_repr = bytes
                                .iter()
                                .map(|b| format!("{:02x}", b))
                                .collect::<String>();
                            tracing::warn!(
                                "Unable to convert column type '{}' to valid UTF-8, showing hex: {}...",
                                column_type,
                                &hex_repr[..hex_repr.len().min(40)]
                            );
                            Some(format!(
                                "[binary data: {} bytes, starts with: {}]",
                                bytes.len(),
                                &hex_repr[..hex_repr.len().min(20)]
                            ))
                        }
                    },
                    Err(_) => {
                        tracing::warn!(
                            "Unable to access raw bytes for column type '{}' at index {}, falling back to NULL",
                            column_type,
                            column_index
                        );
                        None
                    }
                },
            }
        } else {
            tracing::warn!(
                "Unmatched PostgreSQL column type '{}' at index {}, falling back to NULL",
                column_type,
                column_index
            );
            None
        }
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

        match column_type {
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
            ColumnType::Unknown => {
                tracing::warn!("Unknown column type falling back to raw value access");
                self.handle_unknown_type(row, column_index, raw_type)
            }
        }
    }
}
