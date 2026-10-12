//! Reading a table without its large values.
//!
//! A plain `SELECT *` on one table is rewritten so that text and binary
//! columns holding more than a threshold come back as a short preview, with
//! the value's size in a helper column beside it. [`DeferredPlan::apply`]
//! takes the helper columns out of the result again and reports which cells
//! were cut short; [`Dialect::deferred_values_query`] fetches the full values
//! of chosen rows by primary key when one is opened or copied.

use sqlparser::ast::{GroupByExpr, SelectItem, SetExpr, Statement, TableFactor};
use sqlparser::parser::Parser;

use crate::{ColumnInfo, ColumnType, DatabaseType, Dialect, QueryResult};

/// How much of a deferred value is fetched for the grid: characters of text,
/// bytes of binary.
pub const PREVIEW_LENGTH: usize = 256;

/// Helper columns are named with this prefix and the position of the column
/// they measure.
const SIZE_COLUMN_PREFIX: &str = "__blanco_size_";

/// A cell that holds a preview rather than its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredCell {
    pub row: usize,
    /// Index into the result's columns.
    pub column: usize,
    /// Size of the full value in bytes.
    pub size: u64,
}

/// A read with large values left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredPlan {
    /// The query to run in place of the one the plan was made for.
    pub query: String,
    /// How many columns the table has, which the helper columns follow.
    column_count: usize,
    /// The columns read through a preview expression, with the type their
    /// declaration gives them.
    previewed: Vec<(usize, ColumnType)>,
}

impl DeferredPlan {
    /// Take the helper columns out of `result`, which came from
    /// [`Self::query`], and return the cells that hold a preview.
    pub fn apply(&self, result: &mut QueryResult, threshold: u64) -> Vec<DeferredCell> {
        let helpers: Vec<(usize, usize)> = result
            .columns
            .iter()
            .enumerate()
            .skip(self.column_count)
            .filter_map(|(index, name)| {
                let column = name.strip_prefix(SIZE_COLUMN_PREFIX)?.parse().ok()?;
                Some((index, column))
            })
            .collect();
        if helpers.is_empty() {
            return Vec::new();
        }
        let mut deferred = Vec::new();
        for (row_index, row) in result.rows.iter_mut().enumerate() {
            for &(helper, column) in &helpers {
                let size = row
                    .get(helper)
                    .and_then(|size| size.as_deref())
                    .and_then(|size| size.trim().parse::<u64>().ok());
                if let Some(size) = size
                    && size > threshold
                {
                    deferred.push(DeferredCell {
                        row: row_index,
                        column,
                        size,
                    });
                }
            }
            row.truncate(self.column_count);
        }
        result.columns.truncate(self.column_count);
        result.column_types.truncate(self.column_count);
        // SQLite has no type for an expression, and a first row holding NULL
        // gives the driver nothing to go by either.
        for &(column, column_type) in &self.previewed {
            if let Some(result_type) = result.column_types.get_mut(column)
                && *result_type == ColumnType::Unknown
            {
                *result_type = column_type;
            }
        }
        deferred
    }
}

/// The select of a single statement that reads every column of one table:
/// no join, grouping, `DISTINCT` or anything but `*` in its list.
fn plain_star_select(statements: &mut [Statement]) -> Option<&mut sqlparser::ast::Select> {
    let [Statement::Query(select_query)] = statements else {
        return None;
    };
    let SetExpr::Select(select) = select_query.body.as_mut() else {
        return None;
    };
    let star_only = matches!(select.projection.as_slice(), [SelectItem::Wildcard(options)]
        if options.to_string().is_empty());
    let one_table = matches!(select.from.as_slice(), [from]
        if from.joins.is_empty() && matches!(from.relation, TableFactor::Table { .. }));
    let ungrouped = match &select.group_by {
        GroupByExpr::Expressions(expressions, modifiers) => {
            expressions.is_empty() && modifiers.is_empty()
        }
        GroupByExpr::All(_) => false,
    };
    (star_only && one_table && ungrouped && select.distinct.is_none()).then_some(&mut **select)
}

/// What kind of large value a column can hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LargeKind {
    Text,
    Binary,
}

impl Dialect {
    /// The read to run in place of `query` so values over `threshold` bytes
    /// arrive as previews. `None` when the query is anything but a plain
    /// `SELECT *` on one table, the table has no primary key to fetch a
    /// value by later, or none of its columns can hold a large value.
    pub fn deferred_read(
        &self,
        query: &str,
        table_columns: &[ColumnInfo],
        threshold: u64,
    ) -> Option<DeferredPlan> {
        if threshold == 0 || !self.rows_editable() {
            return None;
        }
        if !table_columns.iter().any(|column| column.is_primary_key) {
            return None;
        }
        let large: Vec<Option<LargeKind>> = table_columns
            .iter()
            .map(|column| {
                (!column.is_primary_key)
                    .then(|| self.large_kind(column, threshold))
                    .flatten()
            })
            .collect();
        if large.iter().all(Option::is_none) {
            return None;
        }

        let dialect = self.sqlparser_dialect();
        let mut statements = Parser::parse_sql(&*dialect, query).ok()?;
        let select = plain_star_select(&mut statements)?;

        let mut items = Vec::with_capacity(table_columns.len() * 2);
        let mut sizes = Vec::new();
        for (index, column) in table_columns.iter().enumerate() {
            let quoted = self.quote_identifier(&column.name);
            match large[index] {
                Some(kind) => {
                    let size = self.byte_length_sql(&quoted);
                    items.push(format!(
                        "CASE WHEN {size} > {threshold} THEN {} ELSE {quoted} END AS {quoted}",
                        self.preview_sql(&quoted, kind)
                    ));
                    sizes.push(format!(
                        "{size} AS {}",
                        self.quote_identifier(&format!("{SIZE_COLUMN_PREFIX}{index}"))
                    ));
                }
                None => items.push(quoted),
            }
        }
        items.extend(sizes);

        // The select list is parsed rather than assembled as syntax nodes, so
        // it is checked by the same parser that will print the statement.
        let scaffold = format!("SELECT {} FROM blanco_deferred", items.join(", "));
        let mut scaffold = Parser::parse_sql(&*dialect, &scaffold).ok()?;
        let [Statement::Query(scaffold_query)] = scaffold.as_mut_slice() else {
            return None;
        };
        let SetExpr::Select(scaffold_select) = scaffold_query.body.as_mut() else {
            return None;
        };
        select.projection = std::mem::take(&mut scaffold_select.projection);

        Some(DeferredPlan {
            query: statements.first()?.to_string(),
            column_count: table_columns.len(),
            previewed: large
                .iter()
                .enumerate()
                .filter_map(|(index, kind)| {
                    let column_type = match kind.as_ref()? {
                        LargeKind::Text => ColumnType::Text,
                        LargeKind::Binary => ColumnType::Binary,
                    };
                    Some((index, column_type))
                })
                .collect(),
        })
    }

    /// Whether `query` is a plain `SELECT *` on one table, the only kind of
    /// read whose large values can be deferred.
    pub fn reads_one_table_in_full(&self, query: &str) -> bool {
        Parser::parse_sql(&*self.sqlparser_dialect(), query)
            .is_ok_and(|mut statements| plain_star_select(&mut statements).is_some())
    }

    /// The full values of `columns` for the rows with the given primary
    /// keys: the key columns first, then `columns`, so each returned row can
    /// be matched to the grid row it belongs to.
    pub fn deferred_values_query(
        &self,
        table: &str,
        key_columns: &[(String, ColumnType)],
        columns: &[String],
        keys: &[Vec<String>],
    ) -> String {
        let select_list = key_columns
            .iter()
            .map(|(name, _)| name)
            .chain(columns)
            .map(|name| self.quote_identifier(name))
            .collect::<Vec<_>>()
            .join(", ");
        let rows = keys
            .iter()
            .map(|key| {
                let row = key_columns
                    .iter()
                    .zip(key)
                    .map(|((name, column_type), value)| {
                        format!(
                            "{} = {}",
                            self.quote_identifier(name),
                            self.value_literal(value, *column_type)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" AND ");
                format!("({row})")
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        format!(
            "SELECT {select_list} FROM {} WHERE {rows}",
            self.quote_path(table)
        )
    }

    /// Whether `column` can hold more than `threshold` bytes of text or
    /// binary, going by its declared type.
    fn large_kind(&self, column: &ColumnInfo, threshold: u64) -> Option<LargeKind> {
        let data_type = column.data_type.to_ascii_lowercase();
        let base = data_type
            .split('(')
            .next()
            .unwrap_or(data_type.as_str())
            .trim();
        // A declared length that cannot reach the threshold rules a column
        // out. Unbounded types report none, a negative one or an absurd one.
        let unbounded = data_type.contains("(max)")
            || column
                .character_maximum_length
                .is_none_or(|length| length <= 0 || u64::from(length.unsigned_abs()) > threshold);
        let fits = |kind: LargeKind| unbounded.then_some(kind);
        match self.database_type() {
            DatabaseType::PostgreSQL => match base {
                "text" => Some(LargeKind::Text),
                "bytea" => Some(LargeKind::Binary),
                "character varying" | "varchar" => fits(LargeKind::Text),
                _ => None,
            },
            DatabaseType::MySQL => match base {
                "text" | "mediumtext" | "longtext" => Some(LargeKind::Text),
                "blob" | "mediumblob" | "longblob" => Some(LargeKind::Binary),
                "varchar" => fits(LargeKind::Text),
                "varbinary" => fits(LargeKind::Binary),
                _ => None,
            },
            DatabaseType::SQLite => {
                if base.contains("blob") {
                    Some(LargeKind::Binary)
                } else if base.contains("text") || base.contains("clob") {
                    Some(LargeKind::Text)
                } else if base.contains("char") {
                    fits(LargeKind::Text)
                } else {
                    None
                }
            }
            DatabaseType::MsSql => match base {
                "text" | "ntext" => Some(LargeKind::Text),
                "image" => Some(LargeKind::Binary),
                "varchar" | "nvarchar" => fits(LargeKind::Text),
                "varbinary" => fits(LargeKind::Binary),
                _ => None,
            },
            DatabaseType::ClickHouse | DatabaseType::Redis => None,
        }
    }

    /// The size of `column` in bytes.
    fn byte_length_sql(&self, column: &str) -> String {
        match self.database_type() {
            DatabaseType::PostgreSQL => format!("OCTET_LENGTH({column})"),
            DatabaseType::MsSql => format!("DATALENGTH({column})"),
            // LENGTH counts characters of text in SQLite, bytes of a blob.
            DatabaseType::SQLite => format!("LENGTH(CAST({column} AS BLOB))"),
            _ => format!("LENGTH({column})"),
        }
    }

    /// The start of `column`, keeping its type.
    fn preview_sql(&self, column: &str, kind: LargeKind) -> String {
        match (self.database_type(), kind) {
            (DatabaseType::MsSql, _) => format!("SUBSTRING({column}, 1, {PREVIEW_LENGTH})"),
            (DatabaseType::PostgreSQL | DatabaseType::MySQL, LargeKind::Text) => {
                format!("LEFT({column}, {PREVIEW_LENGTH})")
            }
            _ => format!("SUBSTR({column}, 1, {PREVIEW_LENGTH})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str, data_type: &str, primary_key: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.to_string(),
            data_type: data_type.to_string(),
            is_nullable: !primary_key,
            is_primary_key: primary_key,
            default_value: None,
            character_maximum_length: None,
            foreign_key: None,
            comment: None,
            numeric_precision: None,
            numeric_scale: None,
        }
    }

    fn files() -> Vec<ColumnInfo> {
        vec![
            column("id", "integer", true),
            column("name", "text", false),
            column("body", "bytea", false),
            column("size", "integer", false),
        ]
    }

    #[test]
    fn star_select_is_rewritten_with_previews_and_sizes() {
        let dialect = DatabaseType::PostgreSQL.dialect();
        let plan = dialect
            .deferred_read(
                "SELECT * FROM public.files WHERE size > 10 ORDER BY id LIMIT 100;",
                &files(),
                1024,
            )
            .expect("a plain star select is deferred");
        assert_eq!(
            plan.query,
            concat!(
                r#"SELECT "id", "#,
                r#"CASE WHEN OCTET_LENGTH("name") > 1024 THEN LEFT("name", 256) ELSE "name" END AS "name", "#,
                r#"CASE WHEN OCTET_LENGTH("body") > 1024 THEN SUBSTR("body", 1, 256) ELSE "body" END AS "body", "#,
                r#""size", "#,
                r#"OCTET_LENGTH("name") AS "__blanco_size_1", "#,
                r#"OCTET_LENGTH("body") AS "__blanco_size_2" "#,
                r#"FROM public.files WHERE size > 10 ORDER BY id LIMIT 100"#
            )
        );
    }

    #[test]
    fn other_queries_are_left_alone() {
        let dialect = DatabaseType::PostgreSQL.dialect();
        let columns = files();
        for query in [
            "SELECT id, body FROM files",
            "SELECT * FROM files JOIN owners ON owners.id = files.id",
            "SELECT DISTINCT * FROM files",
            "SELECT * FROM files GROUP BY id",
            "SELECT * FROM (SELECT * FROM files) AS inner_files",
            "UPDATE files SET size = 1",
            "SELECT * FROM files; SELECT * FROM files",
            "not sql",
        ] {
            assert_eq!(
                dialect.deferred_read(query, &columns, 1024),
                None,
                "{query}"
            );
        }
        // Off, no key to fetch by, or nothing that can be large.
        assert_eq!(
            dialect.deferred_read("SELECT * FROM files", &columns, 0),
            None
        );
        let keyless: Vec<ColumnInfo> = columns
            .iter()
            .cloned()
            .map(|mut column| {
                column.is_primary_key = false;
                column
            })
            .collect();
        assert_eq!(
            dialect.deferred_read("SELECT * FROM files", &keyless, 1024),
            None
        );
        let small = vec![column("id", "integer", true), column("n", "integer", false)];
        assert_eq!(
            dialect.deferred_read("SELECT * FROM files", &small, 1024),
            None
        );
        assert_eq!(
            DatabaseType::ClickHouse
                .dialect()
                .deferred_read("SELECT * FROM files", &columns, 1024),
            None
        );
    }

    #[test]
    fn declared_lengths_decide_which_columns_can_be_large() {
        let sized = |data_type: &str, length: Option<i32>| ColumnInfo {
            character_maximum_length: length,
            ..column("value", data_type, false)
        };
        let kind =
            |db_type: DatabaseType, column: &ColumnInfo| db_type.dialect().large_kind(column, 1024);
        let text = Some(LargeKind::Text);
        let binary = Some(LargeKind::Binary);
        assert_eq!(
            kind(
                DatabaseType::PostgreSQL,
                &sized("character varying", Some(255))
            ),
            None
        );
        assert_eq!(
            kind(DatabaseType::PostgreSQL, &sized("character varying", None)),
            text
        );
        assert_eq!(kind(DatabaseType::PostgreSQL, &sized("jsonb", None)), None);
        assert_eq!(
            kind(DatabaseType::MySQL, &sized("varchar", Some(4000))),
            text
        );
        assert_eq!(
            kind(DatabaseType::MySQL, &sized("varchar", Some(100))),
            None
        );
        assert_eq!(
            kind(DatabaseType::MySQL, &sized("longblob", Some(i32::MAX))),
            binary
        );
        assert_eq!(
            kind(DatabaseType::MySQL, &sized("tinytext", Some(255))),
            None
        );
        assert_eq!(kind(DatabaseType::SQLite, &sized("BLOB", None)), binary);
        assert_eq!(
            kind(DatabaseType::SQLite, &sized("VARCHAR(20)", Some(20))),
            None
        );
        assert_eq!(
            kind(DatabaseType::MsSql, &sized("nvarchar", Some(-1))),
            text
        );
        assert_eq!(
            kind(DatabaseType::MsSql, &sized("nvarchar", Some(50))),
            None
        );
        assert_eq!(
            kind(DatabaseType::MsSql, &sized("varbinary(max)", None)),
            binary
        );
    }

    #[test]
    fn apply_strips_helpers_and_reports_cut_cells() {
        let dialect = DatabaseType::PostgreSQL.dialect();
        let plan = dialect
            .deferred_read("SELECT * FROM files", &files(), 1024)
            .expect("plan");
        let text = |value: &str| Some(value.to_string());
        let mut result = QueryResult {
            columns: [
                "id",
                "name",
                "body",
                "size",
                "__blanco_size_1",
                "__blanco_size_2",
            ]
            .map(str::to_string)
            .to_vec(),
            column_types: vec![
                ColumnType::Integer,
                ColumnType::Text,
                ColumnType::Binary,
                ColumnType::Integer,
                ColumnType::Integer,
                ColumnType::Integer,
            ],
            rows: vec![
                vec![
                    text("1"),
                    text("a"),
                    text("0x00"),
                    text("1"),
                    text("1"),
                    text("1"),
                ],
                vec![
                    text("2"),
                    text("b"),
                    text("0x89"),
                    text("9"),
                    text("1"),
                    text("5000"),
                ],
                vec![text("3"), None, None, None, None, None],
                vec![
                    text("4"),
                    text("long"),
                    text("0x"),
                    None,
                    text("1025"),
                    text("0"),
                ],
            ],
            ..QueryResult::default()
        };
        let deferred = plan.apply(&mut result, 1024);
        assert_eq!(
            deferred,
            vec![
                DeferredCell {
                    row: 1,
                    column: 2,
                    size: 5000
                },
                DeferredCell {
                    row: 3,
                    column: 1,
                    size: 1025
                },
            ]
        );
        assert_eq!(result.columns, ["id", "name", "body", "size"]);
        assert_eq!(result.column_types.len(), 4);
        assert!(result.rows.iter().all(|row| row.len() == 4));
        assert_eq!(result.rows[1][2], text("0x89"));
    }

    #[test]
    fn full_values_are_fetched_by_key() {
        let key = [
            ("tenant".to_string(), ColumnType::Integer),
            ("id".to_string(), ColumnType::Binary),
        ];
        let query = DatabaseType::MySQL.dialect().deferred_values_query(
            "app.files",
            &key,
            &["body".to_string()],
            &[
                vec!["1".to_string(), "0x0a".to_string()],
                vec!["2".to_string(), "0x0b".to_string()],
            ],
        );
        assert_eq!(
            query,
            "SELECT `tenant`, `id`, `body` FROM `app`.`files` WHERE (`tenant` = '1' AND `id` = X'0a') OR (`tenant` = '2' AND `id` = X'0b')"
        );
    }
}
