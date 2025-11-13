#![allow(dead_code)]

use crate::transformers::{DataTransformer, SelectedTableData, TransformError};
use std::collections::HashMap;

pub struct JsonTransformer;

impl DataTransformer for JsonTransformer {
    fn format_name(&self) -> &'static str {
        "JSON"
    }

    fn file_extension(&self) -> &'static str {
        "json"
    }

    fn description(&self) -> &'static str {
        "JSON array of objects format"
    }

    fn transform_selected_data(&self, data: &SelectedTableData) -> Result<String, TransformError> {
        if !data.has_selection() {
            return Err(TransformError::EmptySelection);
        }

        let mut json_objects = Vec::new();

        // If we have selected rows, export complete rows
        if !data.selected_rows.is_empty() {
            let mut row_indices: Vec<usize> = data.selected_rows.iter().map(|r| r.row).collect();
            row_indices.sort();

            for row_idx in row_indices {
                if let Some(row) = data.selected_rows.iter().find(|r| r.row == row_idx) {
                    let mut obj = HashMap::new();
                    for cell in &row.cells {
                        if let Some(col_name) = &cell.column_name {
                            let json_value = convert_to_json_value(
                                &cell.value,
                                cell.column_type.as_deref().unwrap_or("text"),
                            );
                            obj.insert(col_name.clone(), json_value);
                        }
                    }
                    json_objects.push(obj);
                }
            }
        }

        // Serialize to JSON
        serde_json::to_string_pretty(&json_objects)
            .map_err(|e| TransformError::FormatError(e.to_string()))
    }

    fn transform_single_cell(
        &self,
        value: &str,
        column_type: &str,
    ) -> Result<String, TransformError> {
        let json_value = convert_to_json_value(value, column_type);
        serde_json::to_string(&json_value).map_err(|e| TransformError::FormatError(e.to_string()))
    }
}

/// Convert a database value to appropriate JSON Value based on column type
fn convert_to_json_value(value: &str, column_type: &str) -> serde_json::Value {
    let normalized_type = column_type.to_lowercase();

    // Check for NULL values
    if is_null_value(value) {
        return serde_json::Value::Null;
    }

    // Try to parse based on column type hints
    if let Some(json_value) = try_parse_typed_value(value, &normalized_type) {
        return json_value;
    }

    // Fallback: try to infer the type from the value itself
    infer_json_type(value)
}

/// Check if a value should be treated as NULL
fn is_null_value(value: &str) -> bool {
    value.is_empty() || value.eq_ignore_ascii_case("null") || value == "--"
}

/// Try to parse a value based on column type hints
fn try_parse_typed_value(value: &str, column_type: &str) -> Option<serde_json::Value> {
    // Boolean type
    if column_type.contains("bool") {
        return Some(parse_boolean(value));
    }

    // Numeric types
    if is_numeric_type(column_type) {
        return parse_numeric(value);
    }

    // JSON type
    if column_type.contains("json") {
        return parse_json(value);
    }

    None
}

/// Check if column type indicates numeric data
fn is_numeric_type(column_type: &str) -> bool {
    column_type.contains("int")
        || column_type.contains("float")
        || column_type.contains("double")
        || column_type.contains("numeric")
        || column_type.contains("decimal")
        || column_type.contains("real")
        || column_type.contains("serial")
        || column_type.contains("bigserial")
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
        && let Some(num) = serde_json::Number::from_f64(float_val) {
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
    fn test_is_null_value() {
        assert!(is_null_value(""));
        assert!(is_null_value("NULL"));
        assert!(is_null_value("null"));
        assert!(is_null_value("--"));
        assert!(!is_null_value("0"));
        assert!(!is_null_value("false"));
        assert!(!is_null_value("some text"));
    }

    #[test]
    fn test_is_numeric_type() {
        assert!(is_numeric_type("integer"));
        assert!(is_numeric_type("int"));
        assert!(is_numeric_type("bigint"));
        assert!(is_numeric_type("float"));
        assert!(is_numeric_type("double"));
        assert!(is_numeric_type("numeric"));
        assert!(is_numeric_type("decimal"));
        assert!(is_numeric_type("real"));
        assert!(is_numeric_type("serial"));
        assert!(is_numeric_type("bigserial"));
        assert!(!is_numeric_type("text"));
        assert!(!is_numeric_type("varchar"));
        assert!(!is_numeric_type("boolean"));
    }

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
        assert_eq!(convert_to_json_value("", "text"), serde_json::Value::Null);
        assert_eq!(
            convert_to_json_value("NULL", "varchar"),
            serde_json::Value::Null
        );

        // Typed numeric values
        assert_eq!(convert_to_json_value("123", "integer"), json!(123));
        assert_eq!(
            convert_to_json_value("45.67", "decimal(10,2)"),
            json!(45.67)
        );

        // Typed boolean values
        assert_eq!(convert_to_json_value("true", "boolean"), json!(true));
        assert_eq!(convert_to_json_value("0", "bool"), json!(false));

        // JSON columns
        assert_eq!(
            convert_to_json_value(r#"{"a": 1}"#, "json"),
            json!({"a": 1})
        );
        assert_eq!(
            convert_to_json_value(r#"[1,2,3]"#, "jsonb"),
            json!([1, 2, 3])
        );

        // Text columns with type inference
        assert_eq!(convert_to_json_value("123", "text"), json!(123)); // Inferred as number
        assert_eq!(convert_to_json_value("hello", "varchar"), json!("hello"));
    }

    #[test]
    fn test_json_transformer() {
        let transformer = JsonTransformer;
        assert_eq!(transformer.format_name(), "JSON");
        assert_eq!(transformer.file_extension(), "json");

        // Test single cell transformation
        assert_eq!(
            transformer.transform_single_cell("123", "integer").unwrap(),
            "123"
        );
        assert_eq!(
            transformer.transform_single_cell("", "text").unwrap(),
            "null"
        );
        assert_eq!(
            transformer.transform_single_cell("hello", "text").unwrap(),
            "\"hello\""
        );

        // Test complex types
        assert_eq!(
            transformer
                .transform_single_cell(r#"{"key": "value"}"#, "json")
                .unwrap(),
            r#"{"key":"value"}"#
        );
        assert_eq!(
            transformer
                .transform_single_cell("true", "boolean")
                .unwrap(),
            "true"
        );
    }
}
