//! Markdown formatting helpers shared by the schema-discovery tool handlers.

use std::fmt::Write as _;

use blanco_core::{DatabaseSchemaResult, TableSchemaInfo};

pub(super) fn format_explore_markdown(result: &DatabaseSchemaResult) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Tables in {}", result.display_name);
    let pagination = &result.pagination;
    let _ = writeln!(
        out,
        "_Pagination: limit={}, offset={}, has_more={}_",
        pagination
            .limit
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".to_string()),
        pagination
            .offset
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".to_string()),
        pagination.has_more,
    );

    if result.tables.is_empty() {
        out.push_str("\nNo tables matched.\n");
        return out;
    }

    for table in &result.tables {
        out.push('\n');
        let _ = writeln!(out, "## {}.{}", table.schema, table.name);
        for col in &table.columns {
            if let Some(fk) = &col.foreign_key {
                let _ = writeln!(
                    out,
                    "- {} -> {}.{}",
                    col.name, fk.foreign_table_name, fk.foreign_column_name
                );
            }
        }
        for inbound in &table.referenced_by {
            let _ = writeln!(
                out,
                "- {} <- {}.{}",
                inbound.to_column, inbound.from_table, inbound.from_column
            );
        }
    }
    out
}

pub(super) fn format_schema_markdown(result: &DatabaseSchemaResult) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Tables in {}", result.display_name);
    let _ = writeln!(out, "_Connection type: {}_", result.connection_type);
    let pagination = &result.pagination;
    let _ = writeln!(
        out,
        "_Pagination: limit={}, offset={}, has_more={}_",
        pagination
            .limit
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".to_string()),
        pagination
            .offset
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".to_string()),
        pagination.has_more,
    );

    if result.tables.is_empty() {
        out.push_str("\nNo tables matched.\n");
        return out;
    }

    for table in &result.tables {
        out.push('\n');
        format_table_markdown(&mut out, table);
    }
    out
}

fn format_table_markdown(out: &mut String, table: &TableSchemaInfo) {
    let _ = writeln!(
        out,
        "## {}.{} ({})",
        table.schema, table.name, table.object_type
    );
    out.push_str("| column | type | nullable | pk | default |\n");
    out.push_str("|--------|------|----------|----|---------|\n");
    for col in &table.columns {
        let default = col
            .default_value
            .as_deref()
            .map(|d| d.replace('|', "\\|").replace('\n', " "))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} |",
            col.name,
            col.data_type,
            if col.is_nullable { "yes" } else { "no" },
            if col.is_primary_key { "yes" } else { "no" },
            default,
        );
    }

    let fks: Vec<_> = table
        .columns
        .iter()
        .filter_map(|c| c.foreign_key.as_ref().map(|fk| (c.name.as_str(), fk)))
        .collect();
    if !fks.is_empty() {
        out.push_str("\nForeign keys:\n");
        for (column, fk) in fks {
            match &fk.constraint_name {
                Some(name) => {
                    let _ = writeln!(
                        out,
                        "- {} -> {}.{} ({})",
                        column, fk.foreign_table_name, fk.foreign_column_name, name
                    );
                }
                None => {
                    let _ = writeln!(
                        out,
                        "- {} -> {}.{}",
                        column, fk.foreign_table_name, fk.foreign_column_name
                    );
                }
            }
        }
    }

    if !table.referenced_by.is_empty() {
        out.push_str("\nReferenced by:\n");
        for inbound in &table.referenced_by {
            match &inbound.constraint_name {
                Some(name) => {
                    let _ = writeln!(
                        out,
                        "- {}.{} -> {} ({})",
                        inbound.from_table, inbound.from_column, inbound.to_column, name
                    );
                }
                None => {
                    let _ = writeln!(
                        out,
                        "- {}.{} -> {}",
                        inbound.from_table, inbound.from_column, inbound.to_column
                    );
                }
            }
        }
    }
}
