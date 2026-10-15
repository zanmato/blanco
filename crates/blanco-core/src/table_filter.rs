//! Server-side filters for a table's rows: predicates built per backend from
//! column, operator and value, and injected into the `WHERE` clause of the
//! query that produced a result.

use sqlparser::ast::{BinaryOperator, Expr, SetExpr, Statement};
use sqlparser::parser::Parser;
use sqlparser::tokenizer::Token;

use crate::{DatabaseType, Dialect};

/// How a filter condition compares a column with its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FilterOperator {
    Equals,
    NotEquals,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
    /// Case-insensitive substring match on the value's text.
    Contains,
    NotContains,
    StartsWith,
    EndsWith,
    /// A `LIKE` pattern as typed, with `%` and `_` wildcards.
    Like,
    NotLike,
    /// A comma-separated list of values.
    In,
    NotIn,
    IsNull,
    IsNotNull,
}

impl FilterOperator {
    pub const ALL: [FilterOperator; 16] = [
        Self::Equals,
        Self::NotEquals,
        Self::Less,
        Self::LessOrEqual,
        Self::Greater,
        Self::GreaterOrEqual,
        Self::Contains,
        Self::NotContains,
        Self::StartsWith,
        Self::EndsWith,
        Self::Like,
        Self::NotLike,
        Self::In,
        Self::NotIn,
        Self::IsNull,
        Self::IsNotNull,
    ];

    /// The label shown in the operator menu.
    pub fn label(self) -> &'static str {
        match self {
            Self::Equals => "=",
            Self::NotEquals => "<>",
            Self::Less => "<",
            Self::LessOrEqual => "<=",
            Self::Greater => ">",
            Self::GreaterOrEqual => ">=",
            Self::Contains => "contains",
            Self::NotContains => "does not contain",
            Self::StartsWith => "starts with",
            Self::EndsWith => "ends with",
            Self::Like => "LIKE",
            Self::NotLike => "NOT LIKE",
            Self::In => "IN",
            Self::NotIn => "NOT IN",
            Self::IsNull => "IS NULL",
            Self::IsNotNull => "IS NOT NULL",
        }
    }

    /// Stable name for persisting the default operator.
    pub fn key(self) -> &'static str {
        match self {
            Self::Equals => "equals",
            Self::NotEquals => "not_equals",
            Self::Less => "less",
            Self::LessOrEqual => "less_or_equal",
            Self::Greater => "greater",
            Self::GreaterOrEqual => "greater_or_equal",
            Self::Contains => "contains",
            Self::NotContains => "not_contains",
            Self::StartsWith => "starts_with",
            Self::EndsWith => "ends_with",
            Self::Like => "like",
            Self::NotLike => "not_like",
            Self::In => "in",
            Self::NotIn => "not_in",
            Self::IsNull => "is_null",
            Self::IsNotNull => "is_not_null",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|operator| operator.key() == key)
    }

    /// Whether the operator compares with a value; `IS NULL` does not.
    pub fn takes_value(self) -> bool {
        !matches!(self, Self::IsNull | Self::IsNotNull)
    }
}

/// How the conditions of a filter combine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FilterJoin {
    #[default]
    And,
    Or,
}

impl FilterJoin {
    fn keyword(self) -> &'static str {
        match self {
            Self::And => "AND",
            Self::Or => "OR",
        }
    }
}

/// One `column operator value` row of a filter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterCondition {
    pub column: String,
    pub operator: FilterOperator,
    pub value: String,
    /// The column holds numbers, so a numeric value is compared as a number
    /// rather than as text (ClickHouse refuses to compare a number with a
    /// string).
    pub numeric: bool,
    /// The column holds bytes, so a value written as hex is compared as the
    /// bytes it spells rather than as its digits.
    pub binary: bool,
}

impl Dialect {
    /// The predicate for one condition, with the column quoted and the value
    /// written as a literal of this backend.
    pub fn filter_predicate(&self, condition: &FilterCondition) -> String {
        let column = self.quote_identifier(&condition.column);
        let value = condition.value.as_str();
        if condition.binary
            && let Some(predicate) = self.binary_predicate(&column, condition.operator, value)
        {
            return predicate;
        }
        let literal = |value: &str| self.filter_literal(value.trim(), condition.numeric);
        match condition.operator {
            FilterOperator::Equals => format!("{column} = {}", literal(value)),
            FilterOperator::NotEquals => format!("{column} <> {}", literal(value)),
            FilterOperator::Less => format!("{column} < {}", literal(value)),
            FilterOperator::LessOrEqual => format!("{column} <= {}", literal(value)),
            FilterOperator::Greater => format!("{column} > {}", literal(value)),
            FilterOperator::GreaterOrEqual => format!("{column} >= {}", literal(value)),
            FilterOperator::Contains => self.text_match(&column, value, "%", "%", false),
            FilterOperator::NotContains => self.text_match(&column, value, "%", "%", true),
            FilterOperator::StartsWith => self.text_match(&column, value, "", "%", false),
            FilterOperator::EndsWith => self.text_match(&column, value, "%", "", false),
            FilterOperator::Like => format!("{column} LIKE {}", self.string_literal(value)),
            FilterOperator::NotLike => format!("{column} NOT LIKE {}", self.string_literal(value)),
            FilterOperator::In | FilterOperator::NotIn => {
                let values = value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(literal)
                    .collect::<Vec<_>>();
                let not = if condition.operator == FilterOperator::NotIn {
                    "NOT "
                } else {
                    ""
                };
                if values.is_empty() {
                    // `IN ()` is a syntax error; an empty list matches nothing.
                    return if not.is_empty() { "1 = 0" } else { "1 = 1" }.to_string();
                }
                format!("{column} {not}IN ({})", values.join(", "))
            }
            FilterOperator::IsNull => format!("{column} IS NULL"),
            FilterOperator::IsNotNull => format!("{column} IS NOT NULL"),
        }
    }

    /// The predicate comparing a binary column with hex, with or without
    /// its `0x`. `None` when the value is not hex or the operator matches
    /// text, which leaves the comparison to the text predicates.
    fn binary_predicate(
        &self,
        column: &str,
        operator: FilterOperator,
        value: &str,
    ) -> Option<String> {
        let literal = |value: &str| {
            let bytes = crate::binary::decode_lenient(value)?;
            let hex = crate::binary::encode(&bytes);
            Some((
                self.value_literal(&hex, crate::ColumnType::Binary),
                bytes.len(),
            ))
        };
        let comparison = |symbol: &str| {
            let (literal, _) = literal(value)?;
            Some(format!("{column} {symbol} {literal}"))
        };
        // DuckDB's string functions take no BLOB, so its bytes are matched
        // as hex text, two digits per byte.
        if self.database_type() == DatabaseType::DuckDb
            && matches!(
                operator,
                FilterOperator::Contains
                    | FilterOperator::NotContains
                    | FilterOperator::StartsWith
                    | FilterOperator::EndsWith
            )
        {
            let bytes = crate::binary::decode_lenient(value)?;
            let hex: String = bytes.iter().map(|byte| format!("{byte:02X}")).collect();
            let digits = format!("HEX({column})");
            return Some(match operator {
                // Anchored to whole bytes, so a match cannot straddle two.
                FilterOperator::Contains => format!("REGEXP_MATCHES({digits}, '^(..)*{hex}')"),
                FilterOperator::NotContains => {
                    format!("NOT REGEXP_MATCHES({digits}, '^(..)*{hex}')")
                }
                FilterOperator::StartsWith => format!("STARTS_WITH({digits}, '{hex}')"),
                _ => format!("ENDS_WITH({digits}, '{hex}')"),
            });
        }
        let length = match self.database_type() {
            DatabaseType::MsSql => format!("DATALENGTH({column})"),
            _ => format!("LENGTH({column})"),
        };
        let substring = match self.database_type() {
            DatabaseType::MsSql => "SUBSTRING",
            _ => "SUBSTR",
        };
        match operator {
            FilterOperator::Equals => comparison("="),
            FilterOperator::NotEquals => comparison("<>"),
            FilterOperator::Less => comparison("<"),
            FilterOperator::LessOrEqual => comparison("<="),
            FilterOperator::Greater => comparison(">"),
            FilterOperator::GreaterOrEqual => comparison(">="),
            FilterOperator::Contains | FilterOperator::NotContains => {
                let (literal, _) = literal(value)?;
                let position = match self.database_type() {
                    DatabaseType::PostgreSQL => format!("POSITION({literal} IN {column})"),
                    DatabaseType::MySQL => format!("LOCATE({literal}, {column})"),
                    DatabaseType::MsSql => format!("CHARINDEX({literal}, {column})"),
                    _ => format!("INSTR({column}, {literal})"),
                };
                let symbol = if operator == FilterOperator::Contains {
                    ">"
                } else {
                    "="
                };
                Some(format!("{position} {symbol} 0"))
            }
            FilterOperator::StartsWith => {
                let (literal, count) = literal(value)?;
                Some(format!("{substring}({column}, 1, {count}) = {literal}"))
            }
            FilterOperator::EndsWith => {
                let (literal, count) = literal(value)?;
                // A value longer than the column cannot be its ending, and
                // the start it would give means something else per backend.
                Some(format!(
                    "{length} >= {count} AND {substring}({column}, {length} - {count} + 1, {count}) = {literal}"
                ))
            }
            FilterOperator::In | FilterOperator::NotIn => {
                let literals = value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| literal(value).map(|(literal, _)| literal))
                    .collect::<Option<Vec<_>>>()?;
                if literals.is_empty() {
                    return None;
                }
                let not = if operator == FilterOperator::NotIn {
                    "NOT "
                } else {
                    ""
                };
                Some(format!("{column} {not}IN ({})", literals.join(", ")))
            }
            FilterOperator::Like
            | FilterOperator::NotLike
            | FilterOperator::IsNull
            | FilterOperator::IsNotNull => None,
        }
    }

    /// The conditions joined with `join`.
    pub fn filter_conditions(&self, conditions: &[FilterCondition], join: FilterJoin) -> String {
        conditions
            .iter()
            .map(|condition| self.filter_predicate(condition))
            .collect::<Vec<_>>()
            .join(&format!(" {} ", join.keyword()))
    }

    /// A predicate matching rows where any of `columns` contains `needle`,
    /// ignoring case.
    pub fn any_column_contains(&self, columns: &[String], needle: &str) -> String {
        if columns.is_empty() {
            return "1 = 0".to_string();
        }
        columns
            .iter()
            .map(|column| self.text_match(&self.quote_identifier(column), needle, "%", "%", false))
            .collect::<Vec<_>>()
            .join(" OR ")
    }

    /// A number when the column is numeric and the value is a plain number,
    /// otherwise a string literal. Only digits, one sign, one point and an
    /// exponent pass, so nothing but a number reaches the SQL unquoted.
    fn filter_literal(&self, value: &str, numeric: bool) -> String {
        if numeric && is_plain_number(value) {
            value.to_string()
        } else {
            self.string_literal(value)
        }
    }

    /// Case-insensitive `LIKE` of the column's text against `needle` with its
    /// wildcards escaped and `prefix`/`suffix` wildcards around it.
    fn text_match(
        &self,
        column: &str,
        needle: &str,
        prefix: &str,
        suffix: &str,
        negated: bool,
    ) -> String {
        let escaped = needle
            .to_lowercase()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = self.string_literal(&format!("{prefix}{escaped}{suffix}"));
        let text = match self.database_type() {
            DatabaseType::PostgreSQL | DatabaseType::SQLite | DatabaseType::DuckDb => {
                format!("CAST({column} AS TEXT)")
            }
            DatabaseType::MySQL => format!("CAST({column} AS CHAR)"),
            DatabaseType::MsSql => format!("CAST({column} AS NVARCHAR(MAX))"),
            DatabaseType::ClickHouse | DatabaseType::Redis => format!("toString({column})"),
        };
        // PostgreSQL, MySQL and ClickHouse escape with a backslash by
        // default; SQLite, DuckDB and SQL Server have no default escape
        // character.
        let escape = match self.database_type() {
            DatabaseType::SQLite | DatabaseType::DuckDb | DatabaseType::MsSql => " ESCAPE '\\'",
            _ => "",
        };
        let not = if negated { "NOT " } else { "" };
        format!("LOWER({text}) {not}LIKE {pattern}{escape}")
    }

    /// `query` with `predicate` added to its `WHERE` clause (combined with an
    /// existing condition by `AND`). The query must be a single plain
    /// `SELECT`; its column list, ordering and limit are kept. The predicate
    /// must be one complete expression, so a typed `WHERE` cannot smuggle in
    /// a second statement.
    pub fn with_filter(&self, query: &str, predicate: &str) -> Result<String, String> {
        let dialect = self.sqlparser_dialect();
        let mut statements = Parser::parse_sql(&*dialect, query)
            .map_err(|error| format!("The query cannot be filtered: {error}"))?;
        let [statement] = statements.as_mut_slice() else {
            return Err("Only a single SELECT can be filtered".to_string());
        };
        let Statement::Query(select_query) = statement else {
            return Err("Only a SELECT can be filtered".to_string());
        };
        let SetExpr::Select(select) = select_query.body.as_mut() else {
            return Err("Only a plain SELECT can be filtered".to_string());
        };

        let mut parser = Parser::new(&*dialect)
            .try_with_sql(predicate)
            .map_err(|error| format!("Invalid filter: {error}"))?;
        let filter = parser
            .parse_expr()
            .map_err(|error| format!("Invalid filter: {error}"))?;
        let rest = parser.peek_token();
        if rest.token != Token::EOF {
            return Err(format!(
                "Invalid filter: unexpected {} after the condition",
                rest.token
            ));
        }

        select.selection = Some(match select.selection.take() {
            Some(existing) => Expr::BinaryOp {
                left: Box::new(Expr::Nested(Box::new(existing))),
                op: BinaryOperator::And,
                right: Box::new(Expr::Nested(Box::new(filter))),
            },
            None => filter,
        });
        Ok(statement.to_string())
    }
}

fn is_plain_number(value: &str) -> bool {
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

    fn condition(column: &str, operator: FilterOperator, value: &str) -> FilterCondition {
        FilterCondition {
            column: column.to_string(),
            operator,
            value: value.to_string(),
            numeric: false,
            binary: false,
        }
    }

    #[test]
    fn binary_columns_compare_hex_as_bytes() {
        let binary = |operator, value: &str| FilterCondition {
            column: "body".to_string(),
            operator,
            value: value.to_string(),
            numeric: false,
            binary: true,
        };
        let pg = DatabaseType::PostgreSQL.dialect();
        assert_eq!(
            pg.filter_predicate(&binary(FilterOperator::Equals, "0x89504E47")),
            r#""body" = '\x89504e47'::bytea"#
        );
        // The digits alone are hex too, since the column only holds bytes.
        assert_eq!(
            pg.filter_predicate(&binary(FilterOperator::StartsWith, "8950")),
            r#"SUBSTR("body", 1, 2) = '\x8950'::bytea"#
        );
        assert_eq!(
            pg.filter_predicate(&binary(FilterOperator::Contains, "\\x4e47")),
            r#"POSITION('\x4e47'::bytea IN "body") > 0"#
        );
        assert_eq!(
            DatabaseType::MySQL
                .dialect()
                .filter_predicate(&binary(FilterOperator::In, "0x01, 0x02")),
            "`body` IN (X'01', X'02')"
        );
        assert_eq!(
            DatabaseType::MsSql
                .dialect()
                .filter_predicate(&binary(FilterOperator::EndsWith, "0xff")),
            "DATALENGTH([body]) >= 1 AND SUBSTRING([body], DATALENGTH([body]) - 1 + 1, 1) = 0xff"
        );
        assert_eq!(
            DatabaseType::SQLite
                .dialect()
                .filter_predicate(&binary(FilterOperator::NotContains, "0xff")),
            r#"INSTR("body", X'ff') = 0"#
        );
        // Not hex: compared as the text it is.
        assert_eq!(
            pg.filter_predicate(&binary(FilterOperator::Equals, "hello")),
            r#""body" = 'hello'"#
        );
    }

    #[test]
    fn predicates_quote_columns_and_values() {
        let pg = DatabaseType::PostgreSQL.dialect();
        assert_eq!(
            pg.filter_predicate(&condition("name", FilterOperator::Equals, "O'Brien")),
            "\"name\" = 'O''Brien'"
        );
        assert_eq!(
            pg.filter_predicate(&FilterCondition {
                numeric: true,
                binary: false,
                ..condition("id", FilterOperator::Greater, " 42 ")
            }),
            "\"id\" > 42"
        );
        // A numeric column still quotes what is not a number.
        assert_eq!(
            pg.filter_predicate(&FilterCondition {
                numeric: true,
                binary: false,
                ..condition("id", FilterOperator::Equals, "1; DROP TABLE x")
            }),
            "\"id\" = '1; DROP TABLE x'"
        );
        assert_eq!(
            pg.filter_predicate(&FilterCondition {
                numeric: true,
                binary: false,
                ..condition("id", FilterOperator::In, "1, 2,,3")
            }),
            "\"id\" IN (1, 2, 3)"
        );
        assert_eq!(
            pg.filter_predicate(&condition("id", FilterOperator::NotIn, " ")),
            "1 = 1"
        );
        assert_eq!(
            pg.filter_predicate(&condition("deleted_at", FilterOperator::IsNull, "x")),
            "\"deleted_at\" IS NULL"
        );
        assert_eq!(
            DatabaseType::MsSql.dialect().filter_predicate(&condition(
                "na]me",
                FilterOperator::NotEquals,
                "a"
            )),
            "[na]]me] <> 'a'"
        );
    }

    #[test]
    fn text_matches_ignore_case_and_escape_wildcards() {
        let contains = condition("name", FilterOperator::Contains, "50%_A");
        assert_eq!(
            DatabaseType::PostgreSQL
                .dialect()
                .filter_predicate(&contains),
            "LOWER(CAST(\"name\" AS TEXT)) LIKE '%50\\%\\_a%'"
        );
        // MySQL doubles the backslashes inside the literal.
        assert_eq!(
            DatabaseType::MySQL.dialect().filter_predicate(&contains),
            "LOWER(CAST(`name` AS CHAR)) LIKE '%50\\\\%\\\\_a%'"
        );
        assert_eq!(
            DatabaseType::SQLite.dialect().filter_predicate(&condition(
                "name",
                FilterOperator::StartsWith,
                "a"
            )),
            "LOWER(CAST(\"name\" AS TEXT)) LIKE 'a%' ESCAPE '\\'"
        );
        assert_eq!(
            DatabaseType::MsSql.dialect().filter_predicate(&condition(
                "name",
                FilterOperator::NotContains,
                "a"
            )),
            "LOWER(CAST([name] AS NVARCHAR(MAX))) NOT LIKE '%a%' ESCAPE '\\'"
        );
        assert_eq!(
            DatabaseType::ClickHouse
                .dialect()
                .filter_predicate(&condition("name", FilterOperator::EndsWith, "a")),
            "LOWER(toString(`name`)) LIKE '%a'"
        );
    }

    #[test]
    fn conditions_join_and_any_column_searches() {
        let pg = DatabaseType::PostgreSQL.dialect();
        let conditions = [
            condition("a", FilterOperator::Equals, "1"),
            condition("b", FilterOperator::IsNotNull, ""),
        ];
        assert_eq!(
            pg.filter_conditions(&conditions, FilterJoin::Or),
            "\"a\" = '1' OR \"b\" IS NOT NULL"
        );
        assert_eq!(
            pg.any_column_contains(&["a".to_string(), "b".to_string()], "x"),
            "LOWER(CAST(\"a\" AS TEXT)) LIKE '%x%' OR LOWER(CAST(\"b\" AS TEXT)) LIKE '%x%'"
        );
        assert_eq!(pg.any_column_contains(&[], "x"), "1 = 0");
    }

    #[test]
    fn with_filter_keeps_the_query_and_adds_the_condition() {
        let pg = DatabaseType::PostgreSQL.dialect();
        assert_eq!(
            pg.with_filter(
                "SELECT * FROM public.users\nLIMIT 100;",
                "\"a\" = 1 OR b > 2"
            )
            .unwrap(),
            "SELECT * FROM public.users WHERE \"a\" = 1 OR b > 2 LIMIT 100"
        );
        assert_eq!(
            pg.with_filter(
                "SELECT id, name FROM users WHERE active ORDER BY id DESC LIMIT 5",
                "name = 'x' OR id = 1"
            )
            .unwrap(),
            "SELECT id, name FROM users WHERE (active) AND (name = 'x' OR id = 1) ORDER BY id DESC LIMIT 5"
        );
        assert_eq!(
            DatabaseType::MsSql
                .dialect()
                .with_filter("SELECT TOP 100 * FROM [dbo].[t]", "[a] = 'x'")
                .unwrap(),
            "SELECT TOP 100 * FROM [dbo].[t] WHERE [a] = 'x'"
        );
        assert_eq!(
            DatabaseType::ClickHouse
                .dialect()
                .with_filter("SELECT * FROM `db`.`t` LIMIT 10", "`a` = 1")
                .unwrap(),
            "SELECT * FROM `db`.`t` WHERE `a` = 1 LIMIT 10"
        );
    }

    #[test]
    fn with_filter_refuses_what_it_cannot_filter() {
        let pg = DatabaseType::PostgreSQL.dialect();
        assert!(pg.with_filter("SELECT 1; SELECT 2", "a = 1").is_err());
        assert!(pg.with_filter("UPDATE t SET a = 1", "a = 1").is_err());
        assert!(
            pg.with_filter("SELECT * FROM a UNION SELECT * FROM b", "a = 1")
                .is_err()
        );
        let error = pg
            .with_filter("SELECT * FROM t", "a = 1; DROP TABLE t")
            .unwrap_err();
        assert!(error.starts_with("Invalid filter"), "{error}");
        assert!(pg.with_filter("SELECT * FROM t", "a = ").is_err());
    }

    #[test]
    fn plain_numbers() {
        for value in ["1", "-1", "+2.5", ".5", "3.", "1e10", "1.5E-3"] {
            assert!(is_plain_number(value), "{value}");
        }
        for value in ["", "-", ".", "1e", "0x10", "1 2", "1;", "NaN", "1e+"] {
            assert!(!is_plain_number(value), "{value}");
        }
    }

    #[test]
    fn operator_keys_round_trip() {
        for operator in FilterOperator::ALL {
            assert_eq!(FilterOperator::from_key(operator.key()), Some(operator));
        }
    }
}
