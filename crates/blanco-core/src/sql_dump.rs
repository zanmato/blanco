//! Pieces of a SQL dump: value literals by column type, statements that turn
//! foreign key checks off around a load, tables ordered so referenced ones
//! come first, and rows grouped into batched INSERT statements.

use std::collections::{BTreeMap, BTreeSet};

use crate::{ColumnType, DatabaseType, Dialect};

impl Dialect {
    /// A cell's text as a literal for an INSERT: `NULL`, a bare number or
    /// boolean when the column holds one, otherwise a string literal.
    pub fn value_literal(&self, value: Option<&str>, column_type: ColumnType) -> String {
        let Some(value) = value else {
            return "NULL".to_string();
        };
        match column_type {
            ColumnType::Integer | ColumnType::UnsignedInteger | ColumnType::Numeric
                if is_plain_number(value) =>
            {
                value.to_string()
            }
            ColumnType::Boolean => match parse_bool(value) {
                Some(truth) => match self.database_type() {
                    DatabaseType::PostgreSQL | DatabaseType::ClickHouse => {
                        if truth { "TRUE" } else { "FALSE" }.to_string()
                    }
                    _ => if truth { "1" } else { "0" }.to_string(),
                },
                None => self.string_literal(value),
            },
            _ => self.string_literal(value),
        }
    }

    /// Statements that turn foreign key checks off before a load and back on
    /// after it, where the backend has a session-wide switch. PostgreSQL's
    /// `session_replication_role` needs a superuser; SQL Server's checks are
    /// per table and ClickHouse has no foreign keys, so those get `None`.
    pub fn foreign_key_check_toggle(&self) -> Option<(&'static str, &'static str)> {
        match self.database_type() {
            DatabaseType::MySQL => {
                Some(("SET FOREIGN_KEY_CHECKS = 0;", "SET FOREIGN_KEY_CHECKS = 1;"))
            }
            DatabaseType::SQLite => {
                Some(("PRAGMA foreign_keys = OFF;", "PRAGMA foreign_keys = ON;"))
            }
            DatabaseType::PostgreSQL => Some((
                "SET session_replication_role = replica;",
                "SET session_replication_role = DEFAULT;",
            )),
            DatabaseType::MsSql | DatabaseType::ClickHouse | DatabaseType::Redis => None,
        }
    }

    /// What goes between an INSERT's column list and `VALUES` so a dump can
    /// write the values of identity columns: PostgreSQL refuses them in a
    /// `GENERATED ALWAYS AS IDENTITY` column otherwise.
    pub fn insert_override_clause(&self) -> Option<&'static str> {
        (self.database_type() == DatabaseType::PostgreSQL).then_some("OVERRIDING SYSTEM VALUE")
    }

    /// A statement moving the sequence behind `column` (serial or identity)
    /// past the loaded rows. `setval` ignores a column without a sequence.
    pub fn sequence_reset(&self, quoted_table: &str, column: &str) -> Option<String> {
        (self.database_type() == DatabaseType::PostgreSQL).then(|| {
            let quoted_column = self.quote_identifier(column);
            format!(
                "SELECT setval(pg_get_serial_sequence({}, {}), MAX({quoted_column})) FROM {quoted_table} HAVING MAX({quoted_column}) IS NOT NULL;",
                self.string_literal(quoted_table),
                self.string_literal(column),
            )
        })
    }

    /// `DROP TABLE IF EXISTS` for a qualified, quoted table name.
    pub fn drop_table_if_exists(&self, quoted_table: &str) -> String {
        format!("DROP TABLE IF EXISTS {quoted_table};")
    }
}

/// Order tables so that each comes after the tables it references. `tables`
/// maps a table to the tables it references; references to tables outside
/// the map are ignored. Returns the order and whether a cycle kept it from
/// being complete (the tables of a cycle then follow in name order).
pub fn dependency_order(tables: &BTreeMap<String, BTreeSet<String>>) -> (Vec<String>, bool) {
    let mut remaining: BTreeMap<&str, BTreeSet<&str>> = tables
        .iter()
        .map(|(table, references)| {
            let references = references
                .iter()
                .filter(|referenced| *referenced != table && tables.contains_key(*referenced))
                .map(String::as_str)
                .collect();
            (table.as_str(), references)
        })
        .collect();
    let mut order = Vec::with_capacity(tables.len());
    loop {
        let ready: Vec<&str> = remaining
            .iter()
            .filter(|(_, references)| references.is_empty())
            .map(|(table, _)| *table)
            .collect();
        if ready.is_empty() {
            break;
        }
        for table in ready {
            remaining.remove(table);
            for references in remaining.values_mut() {
                references.remove(table);
            }
            order.push(table.to_string());
        }
    }
    let cyclic = !remaining.is_empty();
    order.extend(remaining.keys().map(|table| table.to_string()));
    (order, cyclic)
}

/// Groups rows into `INSERT INTO table (columns) VALUES (...), (...);`
/// statements of at most `batch_size` rows.
pub struct InsertBatcher {
    prefix: String,
    batch_size: usize,
    rows: Vec<String>,
}

impl InsertBatcher {
    pub fn new(quoted_table: &str, quoted_columns: &[String], batch_size: usize) -> Self {
        Self {
            prefix: format!(
                "INSERT INTO {quoted_table} ({}) VALUES",
                quoted_columns.join(", ")
            ),
            batch_size: batch_size.max(1),
            rows: Vec::new(),
        }
    }

    /// Like [`Self::new`] with `clause` (e.g. `OVERRIDING SYSTEM VALUE`)
    /// before `VALUES`.
    pub fn with_clause(
        quoted_table: &str,
        quoted_columns: &[String],
        clause: Option<&str>,
        batch_size: usize,
    ) -> Self {
        let clause = clause
            .map(|clause| format!(" {clause}"))
            .unwrap_or_default();
        Self {
            prefix: format!(
                "INSERT INTO {quoted_table} ({}){clause} VALUES",
                quoted_columns.join(", ")
            ),
            batch_size: batch_size.max(1),
            rows: Vec::new(),
        }
    }

    /// Add a row of literals; returns a statement when the batch is full.
    pub fn push(&mut self, literals: &[String]) -> Option<String> {
        self.rows.push(format!("({})", literals.join(", ")));
        (self.rows.len() >= self.batch_size)
            .then(|| self.flush())
            .flatten()
    }

    /// The statement for the rows not yet written, if any.
    pub fn flush(&mut self) -> Option<String> {
        if self.rows.is_empty() {
            return None;
        }
        let statement = format!("{}\n  {};", self.prefix, self.rows.join(",\n  "));
        self.rows.clear();
        Some(statement)
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "t" | "1" | "yes" | "y" | "on" => Some(true),
        "false" | "f" | "0" | "no" | "n" | "off" => Some(false),
        _ => None,
    }
}

/// Digits with an optional sign, point and exponent: nothing else reaches the
/// SQL unquoted.
pub(crate) fn is_plain_number(value: &str) -> bool {
    let digits = value.strip_prefix(['-', '+']).unwrap_or(value);
    let (mantissa, exponent) = match digits.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (digits, None),
    };
    let mut parts = mantissa.splitn(2, '.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next().unwrap_or_default();
    let all_digits = |part: &str| part.chars().all(|c| c.is_ascii_digit());
    let exponent_ok = exponent.is_none_or(|exponent| {
        let exponent = exponent.strip_prefix(['-', '+']).unwrap_or(exponent);
        !exponent.is_empty() && all_digits(exponent)
    });
    !(whole.is_empty() && fraction.is_empty())
        && all_digits(whole)
        && all_digits(fraction)
        && exponent_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_follow_the_column_type() {
        let pg = DatabaseType::PostgreSQL.dialect();
        assert_eq!(pg.value_literal(None, ColumnType::Text), "NULL");
        assert_eq!(pg.value_literal(Some("42"), ColumnType::Integer), "42");
        assert_eq!(
            pg.value_literal(Some("-1.5e3"), ColumnType::Numeric),
            "-1.5e3"
        );
        // Not a plain number: quoted even in a numeric column.
        assert_eq!(pg.value_literal(Some("NaN"), ColumnType::Numeric), "'NaN'");
        assert_eq!(pg.value_literal(Some(""), ColumnType::Integer), "''");
        assert_eq!(pg.value_literal(Some("t"), ColumnType::Boolean), "TRUE");
        assert_eq!(
            pg.value_literal(Some("O'Brien"), ColumnType::Text),
            "'O''Brien'"
        );
        assert_eq!(pg.value_literal(Some("42"), ColumnType::Text), "'42'");
        let mysql = DatabaseType::MySQL.dialect();
        assert_eq!(mysql.value_literal(Some("true"), ColumnType::Boolean), "1");
        assert_eq!(
            mysql.value_literal(Some("a\\b"), ColumnType::Text),
            "'a\\\\b'"
        );
    }

    #[test]
    fn referenced_tables_come_first() {
        let tables: BTreeMap<String, BTreeSet<String>> = [
            ("order_items", vec!["orders", "products"]),
            ("orders", vec!["customers"]),
            ("customers", vec![]),
            ("products", vec!["outside_the_export"]),
            ("tree", vec!["tree"]),
        ]
        .into_iter()
        .map(|(table, refs)| {
            (
                table.to_string(),
                refs.into_iter().map(str::to_string).collect(),
            )
        })
        .collect();
        let (order, cyclic) = dependency_order(&tables);
        assert!(!cyclic);
        assert_eq!(
            order,
            ["customers", "products", "tree", "orders", "order_items"]
        );
    }

    #[test]
    fn cycles_are_reported_and_still_listed() {
        let tables: BTreeMap<String, BTreeSet<String>> = [
            ("a", vec!["b"]),
            ("b", vec!["a"]),
            ("c", vec![]),
            ("d", vec!["a"]),
        ]
        .into_iter()
        .map(|(table, refs)| {
            (
                table.to_string(),
                refs.into_iter().map(str::to_string).collect(),
            )
        })
        .collect();
        let (order, cyclic) = dependency_order(&tables);
        assert!(cyclic);
        assert_eq!(order, ["c", "a", "b", "d"]);
    }

    #[test]
    fn inserts_are_batched() {
        let mut batcher = InsertBatcher::new("\"t\"", &["\"a\"".into(), "\"b\"".into()], 2);
        assert_eq!(batcher.push(&["1".into(), "'x'".into()]), None);
        assert_eq!(
            batcher.push(&["2".into(), "NULL".into()]).as_deref(),
            Some("INSERT INTO \"t\" (\"a\", \"b\") VALUES\n  (1, 'x'),\n  (2, NULL);")
        );
        assert_eq!(batcher.push(&["3".into(), "'y'".into()]), None);
        assert_eq!(
            batcher.flush().as_deref(),
            Some("INSERT INTO \"t\" (\"a\", \"b\") VALUES\n  (3, 'y');")
        );
        assert_eq!(batcher.flush(), None);
    }

    #[test]
    fn postgres_identity_values_and_sequences() {
        let pg = DatabaseType::PostgreSQL.dialect();
        let mut batcher = InsertBatcher::with_clause(
            "\"t\"",
            &["\"id\"".into()],
            pg.insert_override_clause(),
            10,
        );
        batcher.push(&["1".into()]);
        assert_eq!(
            batcher.flush().as_deref(),
            Some("INSERT INTO \"t\" (\"id\") OVERRIDING SYSTEM VALUE VALUES\n  (1);")
        );
        assert_eq!(
            pg.sequence_reset("\"s\".\"t\"", "id").as_deref(),
            Some(
                "SELECT setval(pg_get_serial_sequence('\"s\".\"t\"', 'id'), MAX(\"id\")) FROM \"s\".\"t\" HAVING MAX(\"id\") IS NOT NULL;"
            )
        );
        assert_eq!(DatabaseType::MySQL.dialect().insert_override_clause(), None);
    }

    #[test]
    fn foreign_key_toggles() {
        assert!(
            DatabaseType::MySQL
                .dialect()
                .foreign_key_check_toggle()
                .is_some()
        );
        assert!(
            DatabaseType::MsSql
                .dialect()
                .foreign_key_check_toggle()
                .is_none()
        );
    }
}
