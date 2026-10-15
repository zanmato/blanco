//! Prints DuckDB's built-in functions as JSON lines for
//! `scripts/extract-sql-functions.py duckdb`, which turns them into
//! `crates/blanco/src/sql/functions/duckdb.rs`. The first line carries the
//! bundled DuckDB version.
#![allow(clippy::print_stdout)]

use duckdb::types::Value;

fn texts(value: Value) -> Vec<String> {
    match value {
        Value::List(values) => values
            .into_iter()
            .filter_map(|value| match value {
                Value::Text(text) => Some(text),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn text(value: Value) -> String {
    match value {
        Value::Text(text) => text,
        Value::List(values) => values
            .into_iter()
            .map(text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let connection = duckdb::Connection::open_in_memory()?;
    let version: String = connection.query_row("SELECT version()", [], |row| row.get(0))?;
    println!("{}", serde_json::json!({ "version": version }));

    let mut statement = connection.prepare(
        "SELECT function_name, function_type, parameters, parameter_types, varargs,
                return_type, description, examples, categories
         FROM duckdb_functions()
         WHERE internal AND schema_name = 'main'
           AND function_type IN ('scalar', 'aggregate', 'macro', 'table', 'table_macro')
         ORDER BY function_name",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        println!(
            "{}",
            serde_json::json!({
                "name": text(row.get::<_, Value>(0)?),
                "type": text(row.get::<_, Value>(1)?),
                "parameters": texts(row.get::<_, Value>(2)?),
                "parameter_types": texts(row.get::<_, Value>(3)?),
                "varargs": text(row.get::<_, Value>(4)?),
                "return_type": text(row.get::<_, Value>(5)?),
                "description": text(row.get::<_, Value>(6)?),
                "examples": texts(row.get::<_, Value>(7)?),
                "categories": texts(row.get::<_, Value>(8)?),
            })
        );
    }
    Ok(())
}
