use crate::transformers::{DataTransformer, SelectedTableData, TransformError};
use blanco_core::ColumnType;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct JsonTransformer {
    first_row: AtomicBool,
}

impl JsonTransformer {
    pub fn new() -> Self {
        Self {
            first_row: AtomicBool::new(true),
        }
    }
}

impl DataTransformer for JsonTransformer {
    fn format_name(&self) -> &'static str {
        "JSON"
    }

    fn transform_selected_data(&self, data: &SelectedTableData) -> Result<String, TransformError> {
        if !data.has_selection() {
            return Err(TransformError::EmptySelection);
        }

        let mut json_objects = Vec::with_capacity(data.selected_rows.len());

        // If we have selected rows, export complete rows
        if !data.selected_rows.is_empty() {
            for row in &data.selected_rows {
                let mut obj = HashMap::new();
                for cell in &row.cells {
                    if let Some(col_name) = &cell.column_name {
                        let column_type = cell.column_type.unwrap_or(ColumnType::Text);
                        let json_value = convert_to_json_value(cell.value.as_deref(), &column_type);
                        obj.insert(col_name.clone(), json_value);
                    }
                }
                json_objects.push(obj);
            }
        }

        // Serialize to JSON
        serde_json::to_string_pretty(&json_objects)
            .map_err(|e| TransformError::FormatError(e.to_string()))
    }

    fn initialize_stream(
        &self,
        _columns: &[String],
        _column_types: &[ColumnType],
    ) -> Result<String, TransformError> {
        // Start JSON array
        Ok("[\n".to_string())
    }

    fn transform_stream_row(
        &self,
        row_data: &[Option<String>],
        columns: &[String],
        column_types: &[ColumnType],
    ) -> Result<String, TransformError> {
        let mut obj = std::collections::HashMap::new();

        for (i, value) in row_data.iter().enumerate() {
            if let Some(col_name) = columns.get(i) {
                let column_type = column_types.get(i).copied().unwrap_or(ColumnType::Text);
                let json_value = convert_to_json_value(value.as_deref(), &column_type);
                obj.insert(col_name.clone(), json_value);
            }
        }

        // Format as pretty-printed JSON with proper indentation
        let json_str = serde_json::to_string_pretty(&obj)
            .map_err(|e| TransformError::FormatError(e.to_string()))?;

        // Estimate capacity with indentation
        let estimated_capacity = json_str.len() + (json_str.lines().count() * 2) + 10;
        let mut output = String::with_capacity(estimated_capacity);

        // Add comma separator if this is not the first row
        if self
            .first_row
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            output.push_str(",\n");
        }

        // Add indentation to each line
        for (i, line) in json_str.lines().enumerate() {
            if i > 0 {
                output.push('\n');
            }
            output.push_str("  ");
            output.push_str(line);
        }

        Ok(output)
    }

    fn finalize_stream(&self) -> Result<String, TransformError> {
        // End JSON array
        Ok("\n]".to_string())
    }

    fn transform_header_row(&self, _columns: &[String]) -> Result<String, TransformError> {
        // JSON doesn't need headers as a separate row
        Ok(String::new())
    }
}

/// Convert a database value to appropriate JSON Value based on column type
fn convert_to_json_value(value: Option<&str>, column_type: &ColumnType) -> serde_json::Value {
    let value = match value {
        Some(v) => v,
        None => return serde_json::Value::Null,
    };

    // Try to parse based on column type enum
    match column_type {
        ColumnType::Boolean => parse_boolean(value),
        ColumnType::Integer | ColumnType::Numeric => {
            parse_numeric(value).unwrap_or_else(|| infer_json_type(value))
        }
        ColumnType::Json => parse_json(value).unwrap_or_else(|| infer_json_type(value)),
        _ => infer_json_type(value),
    }
}

/// Parse string as boolean value
fn parse_boolean(value: &str) -> serde_json::Value {
    let lower_value = value.to_lowercase();
    match lower_value.as_str() {
        "true" | "1" | "t" | "yes" | "y" | "on" => serde_json::Value::Bool(true),
        "false" | "0" | "f" | "no" | "n" | "off" => serde_json::Value::Bool(false),
        _ => serde_json::Value::String(value.to_string()),
    }
}

/// Parse string as numeric value
fn parse_numeric(value: &str) -> Option<serde_json::Value> {
    // Try integer first
    if let Ok(int_val) = value.parse::<i64>() {
        return Some(serde_json::Value::Number(serde_json::Number::from(int_val)));
    }

    // Try float
    if let Ok(float_val) = value.parse::<f64>()
        && let Some(num) = serde_json::Number::from_f64(float_val)
    {
        return Some(serde_json::Value::Number(num));
    }

    None
}

/// Parse string as JSON (for JSON/JSONB columns)
fn parse_json(value: &str) -> Option<serde_json::Value> {
    serde_json::from_str(value).ok()
}

/// Infer JSON type from value content when column type hint is not available
fn infer_json_type(value: &str) -> serde_json::Value {
    // Try JSON parsing first (in case it's a JSON string)
    if let Ok(parsed) = serde_json::from_str(value) {
        return parsed;
    }

    // Try numeric inference
    if let Some(numeric) = parse_numeric(value) {
        return numeric;
    }

    // Try boolean inference
    let lower_value = value.to_lowercase();
    match lower_value.as_str() {
        "true" | "false" => return parse_boolean(value),
        _ => {}
    }

    // Default to string
    serde_json::Value::String(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_boolean() {
        assert_eq!(parse_boolean("true"), json!(true));
        assert_eq!(parse_boolean("false"), json!(false));
        assert_eq!(parse_boolean("1"), json!(true));
        assert_eq!(parse_boolean("0"), json!(false));
        assert_eq!(parse_boolean("yes"), json!(true));
        assert_eq!(parse_boolean("no"), json!(false));
        assert_eq!(parse_boolean("on"), json!(true));
        assert_eq!(parse_boolean("off"), json!(false));
        assert_eq!(parse_boolean("invalid"), json!("invalid"));
    }

    #[test]
    fn test_parse_numeric() {
        assert_eq!(parse_numeric("123"), Some(json!(123)));
        assert_eq!(parse_numeric("-42"), Some(json!(-42)));
        assert_eq!(parse_numeric("45.67"), Some(json!(45.67)));
        assert_eq!(parse_numeric("-2.71"), Some(json!(-2.71_f64)));
        assert_eq!(parse_numeric("not_a_number"), None);
        assert_eq!(parse_numeric("inf"), None); // Special float values not supported by serde_json::Number
    }

    #[test]
    fn test_parse_json() {
        assert_eq!(
            parse_json(r#"{"key": "value"}"#),
            Some(json!({"key": "value"}))
        );
        assert_eq!(parse_json(r#"[1, 2, 3]"#), Some(json!([1, 2, 3])));
        assert_eq!(parse_json(r#"true"#), Some(json!(true)));
        assert_eq!(parse_json("invalid json"), None);
    }

    #[test]
    fn test_infer_json_type() {
        // JSON strings
        assert_eq!(
            infer_json_type(r#"{"key": "value"}"#),
            json!({"key": "value"})
        );
        assert_eq!(infer_json_type(r#"[1, 2, 3]"#), json!([1, 2, 3]));

        // Numbers
        assert_eq!(infer_json_type("123"), json!(123));
        assert_eq!(infer_json_type("-45.67"), json!(-45.67));

        // Booleans
        assert_eq!(infer_json_type("true"), json!(true));
        assert_eq!(infer_json_type("false"), json!(false));

        // Strings (fallback)
        assert_eq!(infer_json_type("hello world"), json!("hello world"));
        assert_eq!(infer_json_type("123abc"), json!("123abc")); // Looks numeric but isn't
    }

    #[test]
    fn test_convert_to_json_value_with_column_types() {
        // NULL values
        assert_eq!(
            convert_to_json_value(None, &ColumnType::Text),
            serde_json::Value::Null
        );

        // Typed numeric values
        assert_eq!(
            convert_to_json_value(Some("123"), &ColumnType::Integer),
            json!(123)
        );
        assert_eq!(
            convert_to_json_value(Some("45.67"), &ColumnType::Numeric),
            json!(45.67)
        );

        // Typed boolean values
        assert_eq!(
            convert_to_json_value(Some("true"), &ColumnType::Boolean),
            json!(true)
        );
        assert_eq!(
            convert_to_json_value(Some("0"), &ColumnType::Boolean),
            json!(false)
        );

        // JSON columns
        assert_eq!(
            convert_to_json_value(Some(r#"{"a": 1}"#), &ColumnType::Json),
            json!({"a": 1})
        );
        assert_eq!(
            convert_to_json_value(Some(r#"[1,2,3]"#), &ColumnType::Json),
            json!([1, 2, 3])
        );

        // Text columns with type inference
        assert_eq!(
            convert_to_json_value(Some("123"), &ColumnType::Text),
            json!(123)
        ); // Inferred as number
        assert_eq!(
            convert_to_json_value(Some("hello"), &ColumnType::Text),
            json!("hello")
        );

        // A literal text "NULL" should be treated as the string "NULL", not as null
        assert_eq!(
            convert_to_json_value(Some("NULL"), &ColumnType::Text),
            json!("NULL")
        );
    }
}
