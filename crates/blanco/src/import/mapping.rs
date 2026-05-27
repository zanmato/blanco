use blanco_core::ColumnInfo;
use database::DatabaseType;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Transform {
    None,
    Slugify,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ColumnMapping {
    Skip,
    CsvColumn(usize),
    Fixed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConflictStrategy {
    Fail,
    DoNothing {
        keys: Vec<String>,
    },
    Update {
        keys: Vec<String>,
        update_columns: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledMapping {
    pub table_column: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub source: MappingSource,
    pub transform: Transform,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MappingSource {
    CsvColumn(usize),
    Fixed(String),
}

pub fn compile_mappings(
    columns: &[ColumnInfo],
    mappings: &[ColumnMapping],
    transforms: &[Transform],
) -> Vec<CompiledMapping> {
    columns
        .iter()
        .zip(mappings.iter())
        .zip(transforms.iter())
        .filter_map(|((col, mapping), transform)| match mapping {
            ColumnMapping::Skip => None,
            ColumnMapping::CsvColumn(idx) => Some(CompiledMapping {
                table_column: col.name.clone(),
                data_type: col.data_type.clone(),
                is_nullable: col.is_nullable,
                source: MappingSource::CsvColumn(*idx),
                transform: *transform,
            }),
            ColumnMapping::Fixed(value) => Some(CompiledMapping {
                table_column: col.name.clone(),
                data_type: col.data_type.clone(),
                is_nullable: col.is_nullable,
                source: MappingSource::Fixed(value.clone()),
                transform: *transform,
            }),
        })
        .collect()
}

pub fn cell_to_param(raw: Option<&str>, is_nullable: bool, transform: Transform) -> Option<String> {
    match raw {
        None => None,
        Some("") => {
            if is_nullable {
                None
            } else {
                Some(String::new())
            }
        }
        Some(value) => Some(apply_transform(value, transform)),
    }
}

fn apply_transform(value: &str, transform: Transform) -> String {
    match transform {
        Transform::None => value.to_string(),
        Transform::Slugify => slugify(value),
    }
}

pub fn slugify(value: &str) -> String {
    let ascii = deunicode::deunicode_with_tofu(value, "");
    let lower = ascii.to_lowercase();
    let mut result = String::with_capacity(lower.len());
    for c in lower.chars() {
        if c.is_ascii_alphanumeric() {
            result.push(c);
        } else if (c == ' ' || c == '-' || c == '_') && !result.ends_with('-') {
            result.push('-');
        }
    }
    result.trim_end_matches('-').to_string()
}

pub fn quote_ident(ident: &str, db_type: DatabaseType) -> String {
    match db_type {
        DatabaseType::MySQL => format!("`{}`", ident.replace('`', "``")),
        _ => format!("\"{}\"", ident.replace('"', "\"\"")),
    }
}

pub fn quote_qualified(schema: Option<&str>, table: &str, db_type: DatabaseType) -> String {
    match schema {
        Some(s) if !s.is_empty() => format!(
            "{}.{}",
            quote_ident(s, db_type),
            quote_ident(table, db_type)
        ),
        _ => quote_ident(table, db_type),
    }
}

pub fn build_insert_sql(
    fq_table: &str,
    columns: &[String],
    data_types: &[String],
    row_count: usize,
    db_type: DatabaseType,
    conflict: &ConflictStrategy,
) -> String {
    let quoted_cols: Vec<String> = columns.iter().map(|c| quote_ident(c, db_type)).collect();
    let placeholders_per_row = columns.len();

    let placeholder_for = |global_idx: usize, col_idx: usize| -> String {
        let ph = match db_type {
            DatabaseType::PostgreSQL => format!("${}", global_idx + 1),
            _ => "?".to_string(),
        };
        if db_type == DatabaseType::PostgreSQL
            && let Some(dt) = data_types.get(col_idx)
        {
            let lower = dt.to_lowercase();
            if !is_text_type(&lower) {
                return format!("{}::{}", ph, lower);
            }
        }
        ph
    };

    let needs_type_cast = data_types
        .iter()
        .any(|dt| !is_text_type(&dt.to_lowercase()));

    let placeholder_for_simple = |global_idx: usize| -> String {
        match db_type {
            DatabaseType::PostgreSQL => format!("${}", global_idx + 1),
            _ => "?".to_string(),
        }
    };

    let mut values_groups: Vec<String> = Vec::with_capacity(row_count);
    for row in 0..row_count {
        let start = row * placeholders_per_row;
        let group: Vec<String> = if needs_type_cast {
            (0..placeholders_per_row)
                .map(|i| placeholder_for(start + i, i))
                .collect()
        } else {
            (0..placeholders_per_row)
                .map(|i| placeholder_for_simple(start + i))
                .collect()
        };
        values_groups.push(format!("({})", group.join(",")));
    }

    let mut sql = match (db_type, conflict) {
        (DatabaseType::MySQL, ConflictStrategy::DoNothing { .. }) => format!(
            "INSERT IGNORE INTO {} ({}) VALUES {}",
            fq_table,
            quoted_cols.join(","),
            values_groups.join(",")
        ),
        _ => format!(
            "INSERT INTO {} ({}) VALUES {}",
            fq_table,
            quoted_cols.join(","),
            values_groups.join(",")
        ),
    };

    match (db_type, conflict) {
        (DatabaseType::MySQL, ConflictStrategy::DoNothing { .. }) => {}
        (DatabaseType::MySQL, ConflictStrategy::Update { update_columns, .. }) => {
            let assignments: Vec<String> = update_columns
                .iter()
                .map(|c| {
                    let q = quote_ident(c, db_type);
                    format!("{} = VALUES({})", q, q)
                })
                .collect();
            if !assignments.is_empty() {
                sql.push_str(" ON DUPLICATE KEY UPDATE ");
                sql.push_str(&assignments.join(","));
            }
        }
        (_, ConflictStrategy::DoNothing { keys }) => {
            let key_list = keys
                .iter()
                .map(|k| quote_ident(k, db_type))
                .collect::<Vec<_>>()
                .join(",");
            if key_list.is_empty() {
                sql.push_str(" ON CONFLICT DO NOTHING");
            } else {
                sql.push_str(&format!(" ON CONFLICT ({}) DO NOTHING", key_list));
            }
        }
        (
            _,
            ConflictStrategy::Update {
                keys,
                update_columns,
            },
        ) => {
            let key_list = keys
                .iter()
                .map(|k| quote_ident(k, db_type))
                .collect::<Vec<_>>()
                .join(",");
            let assignments: Vec<String> = update_columns
                .iter()
                .map(|c| {
                    let q = quote_ident(c, db_type);
                    format!("{} = EXCLUDED.{}", q, q)
                })
                .collect();
            if !assignments.is_empty() {
                sql.push_str(&format!(
                    " ON CONFLICT ({}) DO UPDATE SET {}",
                    key_list,
                    assignments.join(",")
                ));
            }
        }
        (_, ConflictStrategy::Fail) => {}
    }

    sql
}

/// Produce a human-readable preview with literal values substituted.
pub fn preview_statements(
    fq_table: &str,
    compiled: &[CompiledMapping],
    sample_rows: &[Vec<String>],
    db_type: DatabaseType,
    conflict: &ConflictStrategy,
    max: usize,
) -> Vec<String> {
    let columns: Vec<String> = compiled.iter().map(|m| m.table_column.clone()).collect();
    if columns.is_empty() {
        return vec!["-- no columns mapped --".to_string()];
    }
    let quoted_cols: Vec<String> = columns.iter().map(|c| quote_ident(c, db_type)).collect();

    let rows: Vec<String> = sample_rows
        .iter()
        .take(max)
        .map(|row| {
            let values: Vec<String> = compiled
                .iter()
                .map(|m| match &m.source {
                    MappingSource::Fixed(v) => {
                        literal(Some(v.as_str()), m.is_nullable, m.transform)
                    }
                    MappingSource::CsvColumn(idx) => literal(
                        row.get(*idx).map(|s| s.as_str()),
                        m.is_nullable,
                        m.transform,
                    ),
                })
                .collect();
            format!("({})", values.join(","))
        })
        .collect();

    if rows.is_empty() {
        return vec!["-- no sample data --".to_string()];
    }

    let insert_keyword = match (db_type, conflict) {
        (DatabaseType::MySQL, ConflictStrategy::DoNothing { .. }) => "INSERT IGNORE INTO",
        _ => "INSERT INTO",
    };
    let mut stmt = format!(
        "{} {} ({})\nVALUES {}",
        insert_keyword,
        fq_table,
        quoted_cols.join(", "),
        rows.join(",\n       "),
    );
    append_conflict_clause(&mut stmt, &columns, db_type, conflict);
    stmt.push(';');
    vec![stmt]
}

fn append_conflict_clause(
    sql: &mut String,
    _columns: &[String],
    db_type: DatabaseType,
    conflict: &ConflictStrategy,
) {
    match (db_type, conflict) {
        (DatabaseType::MySQL, ConflictStrategy::DoNothing { .. }) => {}
        (DatabaseType::MySQL, ConflictStrategy::Update { update_columns, .. }) => {
            if !update_columns.is_empty() {
                let assignments: Vec<String> = update_columns
                    .iter()
                    .map(|c| {
                        let q = quote_ident(c, db_type);
                        format!("{} = VALUES({})", q, q)
                    })
                    .collect();
                sql.push_str(" ON DUPLICATE KEY UPDATE ");
                sql.push_str(&assignments.join(","));
            }
        }
        (_, ConflictStrategy::DoNothing { keys }) => {
            let key_list = keys
                .iter()
                .map(|k| quote_ident(k, db_type))
                .collect::<Vec<_>>()
                .join(",");
            if key_list.is_empty() {
                sql.push_str(" ON CONFLICT DO NOTHING");
            } else {
                sql.push_str(&format!(" ON CONFLICT ({}) DO NOTHING", key_list));
            }
        }
        (
            _,
            ConflictStrategy::Update {
                keys,
                update_columns,
            },
        ) => {
            if update_columns.is_empty() {
                return;
            }
            let key_list = keys
                .iter()
                .map(|k| quote_ident(k, db_type))
                .collect::<Vec<_>>()
                .join(",");
            let assignments: Vec<String> = update_columns
                .iter()
                .map(|c| {
                    let q = quote_ident(c, db_type);
                    format!("{} = EXCLUDED.{}", q, q)
                })
                .collect();
            sql.push_str(&format!(
                " ON CONFLICT ({}) DO UPDATE SET {}",
                key_list,
                assignments.join(",")
            ));
        }
        (_, ConflictStrategy::Fail) => {}
    }
}

fn literal(raw: Option<&str>, is_nullable: bool, transform: Transform) -> String {
    match cell_to_param(raw, is_nullable, transform) {
        None => "NULL".to_string(),
        Some(v) => format!("'{}'", v.replace('\'', "''")),
    }
}

fn is_text_type(lower: &str) -> bool {
    matches!(
        lower,
        "text"
            | "character varying"
            | "varchar"
            | "char"
            | "character"
            | "bpchar"
            | "name"
            | "cstring"
    ) || lower.starts_with("varchar")
        || lower.starts_with("character varying")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns() -> Vec<ColumnInfo> {
        vec![
            ColumnInfo {
                name: "id".into(),
                data_type: "integer".into(),
                is_nullable: false,
                is_primary_key: true,
                default_value: Some("nextval".into()),
                character_maximum_length: None,
                foreign_key: None,
            },
            ColumnInfo {
                name: "name".into(),
                data_type: "text".into(),
                is_nullable: false,
                is_primary_key: false,
                default_value: None,
                character_maximum_length: None,
                foreign_key: None,
            },
            ColumnInfo {
                name: "email".into(),
                data_type: "text".into(),
                is_nullable: true,
                is_primary_key: false,
                default_value: None,
                character_maximum_length: None,
                foreign_key: None,
            },
            ColumnInfo {
                name: "tenant_id".into(),
                data_type: "text".into(),
                is_nullable: false,
                is_primary_key: false,
                default_value: None,
                character_maximum_length: None,
                foreign_key: None,
            },
        ]
    }

    #[test]
    fn compile_skip_csv_fixed_combo() {
        let cols = columns();
        let mappings = vec![
            ColumnMapping::Skip,
            ColumnMapping::CsvColumn(0),
            ColumnMapping::CsvColumn(1),
            ColumnMapping::Fixed("acme".into()),
        ];
        let compiled = compile_mappings(&cols, &mappings, &[Transform::None; 4]);
        assert_eq!(compiled.len(), 3);
        assert_eq!(compiled[0].table_column, "name");
        assert!(matches!(compiled[2].source, MappingSource::Fixed(_)));
    }

    #[test]
    fn empty_cell_respects_nullability() {
        assert_eq!(cell_to_param(Some(""), true, Transform::None), None);
        assert_eq!(
            cell_to_param(Some(""), false, Transform::None),
            Some(String::new())
        );
        assert_eq!(
            cell_to_param(Some("x"), true, Transform::None),
            Some("x".into())
        );
        assert_eq!(cell_to_param(None, false, Transform::None), None);
    }

    #[test]
    fn postgres_insert_uses_dollar_placeholders() {
        let sql = build_insert_sql(
            "\"public\".\"users\"",
            &["name".into(), "email".into()],
            &["text".into(), "text".into()],
            2,
            DatabaseType::PostgreSQL,
            &ConflictStrategy::Fail,
        );
        assert!(sql.contains("($1,$2)"));
        assert!(sql.contains("($3,$4)"));
    }

    #[test]
    fn postgres_insert_casts_non_text_types() {
        let sql = build_insert_sql(
            "\"public\".\"items\"",
            &["id".into(), "name".into()],
            &["uuid".into(), "text".into()],
            1,
            DatabaseType::PostgreSQL,
            &ConflictStrategy::Fail,
        );
        assert!(sql.contains("$1::uuid"));
        assert!(sql.contains("$2")); // no cast for text
        assert!(!sql.contains("$2::"));
    }

    #[test]
    fn mysql_insert_uses_question_placeholders_and_on_duplicate() {
        let sql = build_insert_sql(
            "`users`",
            &["name".into(), "email".into()],
            &["text".into(), "text".into()],
            1,
            DatabaseType::MySQL,
            &ConflictStrategy::Update {
                keys: vec!["id".into()],
                update_columns: vec!["email".into()],
            },
        );
        assert!(sql.contains("(?,?)"));
        assert!(sql.contains("ON DUPLICATE KEY UPDATE `email` = VALUES(`email`)"));
    }

    #[test]
    fn postgres_on_conflict_do_update() {
        let sql = build_insert_sql(
            "\"users\"",
            &["id".into(), "name".into()],
            &["uuid".into(), "text".into()],
            1,
            DatabaseType::PostgreSQL,
            &ConflictStrategy::Update {
                keys: vec!["id".into()],
                update_columns: vec!["name".into()],
            },
        );
        assert!(sql.contains("ON CONFLICT (\"id\") DO UPDATE SET \"name\" = EXCLUDED.\"name\""));
        assert!(sql.contains("$1::uuid"));
    }
}
