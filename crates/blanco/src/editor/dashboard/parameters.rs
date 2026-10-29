//! What a dashboard puts into its panels' queries: its time range and its
//! variables, written the way Grafana's SQL data sources write them.
//!
//! `$__timeFilter(column)` becomes `(column >= from AND column <= to)`,
//! `$__timeFrom()` and `$__timeTo()` the range's ends, each an instant the
//! backend reads in UTC (`Dialect::timestamp_literal`). A variable is
//! `$name` or `${name}`, written as a string literal of the backend, or
//! `${name:raw}` written as it is.

use blanco_core::Dialect;
use blanco_core::statement_stream::{SqlRegion, SqlRegionKind, sql_regions};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sql_parser::statement_parser::QueryParameter;

/// How far back a dashboard looks, ending now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeRange {
    #[serde(rename = "1h")]
    LastHour,
    #[serde(rename = "6h")]
    Last6Hours,
    #[serde(rename = "24h")]
    Last24Hours,
    #[default]
    #[serde(rename = "7d")]
    Last7Days,
    #[serde(rename = "30d")]
    Last30Days,
    #[serde(rename = "90d")]
    Last90Days,
    #[serde(rename = "1y")]
    LastYear,
    #[serde(rename = "all")]
    AllTime,
}

impl TimeRange {
    pub const ALL: [TimeRange; 8] = [
        TimeRange::LastHour,
        TimeRange::Last6Hours,
        TimeRange::Last24Hours,
        TimeRange::Last7Days,
        TimeRange::Last30Days,
        TimeRange::Last90Days,
        TimeRange::LastYear,
        TimeRange::AllTime,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TimeRange::LastHour => "Last hour",
            TimeRange::Last6Hours => "Last 6 hours",
            TimeRange::Last24Hours => "Last 24 hours",
            TimeRange::Last7Days => "Last 7 days",
            TimeRange::Last30Days => "Last 30 days",
            TimeRange::Last90Days => "Last 90 days",
            TimeRange::LastYear => "Last year",
            TimeRange::AllTime => "All time",
        }
    }

    /// The stored key (`"7d"`), as the dashboards table keeps it.
    pub fn key(self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_else(|| "7d".to_string())
    }

    /// The range a stored key names, the default for one this build does
    /// not know.
    pub fn from_key(key: &str) -> Self {
        serde_json::from_value(serde_json::Value::String(key.to_string())).unwrap_or_default()
    }

    fn duration(self) -> Option<Duration> {
        match self {
            TimeRange::LastHour => Some(Duration::hours(1)),
            TimeRange::Last6Hours => Some(Duration::hours(6)),
            TimeRange::Last24Hours => Some(Duration::hours(24)),
            TimeRange::Last7Days => Some(Duration::days(7)),
            TimeRange::Last30Days => Some(Duration::days(30)),
            TimeRange::Last90Days => Some(Duration::days(90)),
            TimeRange::LastYear => Some(Duration::days(365)),
            TimeRange::AllTime => None,
        }
    }
}

/// A value the panels' queries can name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardVariable {
    pub name: String,
    pub source: VariableSource,
    /// The value picked, `None` for the first one there is.
    #[serde(default)]
    pub current: Option<String>,
}

/// Where a variable's choices come from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VariableSource {
    Values {
        values: Vec<String>,
    },
    /// The first column of the rows a query returns, run on a connection
    /// like a panel's query.
    Query {
        connection_id: i64,
        database_name: String,
        query: String,
    },
}

/// A variable's name is written after `$`, so it is an identifier.
pub fn is_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with("__")
}

/// Read a dashboard's stored variables. Ones this build cannot read are
/// left out.
pub fn variables_from_json(text: &str) -> Vec<DashboardVariable> {
    serde_json::from_str(text).unwrap_or_else(|error| {
        tracing::warn!("A dashboard's variables did not read: {error}");
        Vec::new()
    })
}

pub fn variables_to_json(variables: &[DashboardVariable]) -> String {
    serde_json::to_string(variables).unwrap_or_else(|_| "[]".to_string())
}

/// The time range and variable values of one run, which every panel of a
/// refresh gets the same of.
#[derive(Clone, Debug, PartialEq)]
pub struct DashboardParameters {
    pub time_range: TimeRange,
    /// Each variable's name and the value it stands for.
    pub values: Vec<(String, String)>,
    pub now: DateTime<Utc>,
}

impl DashboardParameters {
    pub fn new(time_range: TimeRange, values: Vec<(String, String)>) -> Self {
        Self {
            time_range,
            values,
            now: Utc::now(),
        }
    }

    fn from_time(&self) -> DateTime<Utc> {
        match self.time_range.duration() {
            Some(duration) => self.now - duration,
            None => DateTime::<Utc>::UNIX_EPOCH,
        }
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(variable, _)| variable == name)
            .map(|(_, value)| value.as_str())
    }

    /// `sql` with the macros and variables it names written out. Anything
    /// else that starts with `$`, such as PostgreSQL's `$1` or a variable
    /// the dashboard does not have, is left as it is. Within a string
    /// literal only variables are written, as text of that literal, so
    /// `'%$name%'` works. Comments, quoted identifiers and dollar-quoted
    /// bodies are left alone.
    pub fn apply(&self, sql: &str, dialect: Dialect) -> String {
        let regions = sql_regions(dialect, sql);
        let mut written = String::with_capacity(sql.len());
        let mut position = 0;
        while let Some(offset) = sql[position..].find('$') {
            let at = position + offset;
            written.push_str(&sql[position..at]);
            let expansion = match region_at(&regions, at) {
                Some(region) if region.kind == SqlRegionKind::Code => {
                    self.expand(sql, at, &regions, dialect)
                }
                Some(region) if region.kind == SqlRegionKind::String => {
                    let quote = sql[region.range.start..].chars().next().unwrap_or('\'');
                    self.expand_in_string(&sql[at..], quote, dialect)
                }
                _ => None,
            };
            match expansion {
                Some((replacement, consumed)) => {
                    written.push_str(&replacement);
                    position = at + consumed;
                }
                None => {
                    written.push('$');
                    position = at + 1;
                }
            }
        }
        written.push_str(&sql[position..]);
        written
    }

    /// What the `$` expression at `at` in code becomes, and how many bytes
    /// of `sql` it was.
    fn expand(
        &self,
        sql: &str,
        at: usize,
        regions: &[SqlRegion],
        dialect: Dialect,
    ) -> Option<(String, usize)> {
        const TIME_FILTER: &str = "$__timeFilter(";
        let text = &sql[at..];
        if text.starts_with(TIME_FILTER) {
            let argument_start = at + TIME_FILTER.len();
            let close = closing_parenthesis(sql, argument_start, regions)?;
            let column = sql[argument_start..close].trim();
            if column.is_empty() {
                return None;
            }
            // In parentheses, so `NOT $__timeFilter(t)` negates both ends.
            let filter = match self.time_range.duration() {
                Some(_) => format!(
                    "({column} >= {} AND {column} <= {})",
                    dialect.timestamp_literal(self.from_time()),
                    dialect.timestamp_literal(self.now)
                ),
                None => "1 = 1".to_string(),
            };
            return Some((filter, close + 1 - at));
        }
        for (name, time) in [
            ("$__timeFrom()", self.from_time()),
            ("$__timeTo()", self.now),
        ] {
            if text.starts_with(name) {
                return Some((dialect.timestamp_literal(time), name.len()));
            }
        }
        let (name, raw, consumed) = variable_reference(text)?;
        let value = self.value(name)?;
        let written = if raw {
            value.to_string()
        } else {
            dialect.string_literal(value)
        };
        Some((written, consumed))
    }

    /// A variable in a string literal closed by `quote` becomes its value
    /// escaped for that literal.
    fn expand_in_string(
        &self,
        text: &str,
        quote: char,
        dialect: Dialect,
    ) -> Option<(String, usize)> {
        let (name, raw, consumed) = variable_reference(text)?;
        let value = self.value(name)?;
        let written = if raw {
            value.to_string()
        } else if quote == '"' {
            // A MySQL string in double quotes, where a single quote needs no
            // escape and a double one does.
            value.replace('\\', "\\\\").replace('"', "\"\"")
        } else {
            dialect.escape_string_literal(value)
        };
        Some((written, consumed))
    }

    /// Whether a bind parameter the editor found is one of these, which the
    /// run writes out rather than asks for.
    pub fn covers(&self, parameter: &QueryParameter) -> bool {
        let text = parameter.raw_text.trim_start_matches('$');
        parameter.raw_text.starts_with('$')
            && (text.starts_with("__") || self.value(text).is_some())
    }
}

/// The region holding byte `position`.
fn region_at(regions: &[SqlRegion], position: usize) -> Option<&SqlRegion> {
    let ix = regions.partition_point(|region| region.range.end <= position);
    regions.get(ix)
}

/// The `)` in code that closes the parenthesis opened just before `start`,
/// past any nested ones and whatever strings hold.
fn closing_parenthesis(sql: &str, start: usize, regions: &[SqlRegion]) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, c) in sql[start..].char_indices() {
        let position = start + offset;
        if region_at(regions, position).is_none_or(|region| region.kind != SqlRegionKind::Code) {
            continue;
        }
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => return Some(position),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// The variable `$name`, `${name}` or `${name:raw}` at the start of `text`:
/// its name, whether it is raw, and how many bytes it takes.
fn variable_reference(text: &str) -> Option<(&str, bool, usize)> {
    if let Some(after) = text.strip_prefix("${") {
        let close = after.find('}')?;
        let (name, raw) = match after[..close].split_once(':') {
            Some((name, "raw")) => (name, true),
            Some(_) => return None,
            None => (&after[..close], false),
        };
        return Some((name, raw, 2 + close + 1));
    }
    let rest = text.strip_prefix('$')?;
    let name_length = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    let name = &rest[..name_length];
    is_variable_name(name).then_some((name, false, 1 + name_length))
}

#[cfg(test)]
mod tests {
    use super::*;
    use blanco_core::DatabaseType;

    fn parameters(range: TimeRange) -> DashboardParameters {
        DashboardParameters {
            time_range: range,
            values: vec![
                ("region".to_string(), "North's".to_string()),
                ("limit".to_string(), "10".to_string()),
            ],
            now: DateTime::parse_from_rfc3339("2026-10-26T12:00:00Z")
                .expect("time")
                .with_timezone(&Utc),
        }
    }

    #[test]
    fn macros_and_variables_are_written_out() {
        let dialect = DatabaseType::PostgreSQL.dialect();
        let sql = "SELECT * FROM sales WHERE $__timeFilter(sold_at) AND region = $region \
                   AND id > $1 AND note = '$unknown' AND x = ${region} LIMIT ${limit:raw}";
        assert_eq!(
            parameters(TimeRange::Last24Hours).apply(sql, dialect),
            "SELECT * FROM sales WHERE (sold_at >= '2026-10-25T12:00:00Z' AND sold_at <= \
             '2026-10-26T12:00:00Z') AND region = 'North''s' AND id > $1 AND note = '$unknown' \
             AND x = 'North''s' LIMIT 10"
        );
        assert_eq!(
            parameters(TimeRange::AllTime).apply(
                "WHERE $__timeFilter(t) AND $__timeTo() > $__timeFrom()",
                dialect
            ),
            "WHERE 1 = 1 AND '2026-10-26T12:00:00Z' > '1970-01-01T00:00:00Z'"
        );
    }

    #[test]
    fn variables_in_strings_take_the_literal_s_escaping() {
        let parameters = parameters(TimeRange::Last7Days);
        assert_eq!(
            parameters.apply(
                "WHERE region = '$region' OR name LIKE '%${region}%'",
                DatabaseType::PostgreSQL.dialect()
            ),
            "WHERE region = 'North''s' OR name LIKE '%North''s%'"
        );
        assert_eq!(
            parameters.apply(
                r#"WHERE region = "$region" OR note = 'a\'$region'"#,
                DatabaseType::MySQL.dialect()
            ),
            r#"WHERE region = "North's" OR note = 'a\'North''s'"#
        );
    }

    #[test]
    fn comments_identifiers_and_dollar_quotes_are_left_alone() {
        let parameters = parameters(TimeRange::Last7Days);
        let sql = "SELECT \"$region\" -- $region\n/* $region */ FROM t; \
                   DO $body$ BEGIN PERFORM $region; END $body$";
        assert_eq!(
            parameters.apply(sql, DatabaseType::PostgreSQL.dialect()),
            sql,
            "nothing here is code"
        );
    }

    #[test]
    fn the_time_filter_takes_an_expression_and_negates_whole() {
        let parameters = parameters(TimeRange::Last24Hours);
        assert_eq!(
            parameters.apply(
                "WHERE NOT $__timeFilter(coalesce(t, ')')) AND x = 1",
                DatabaseType::PostgreSQL.dialect()
            ),
            "WHERE NOT (coalesce(t, ')') >= '2026-10-25T12:00:00Z' AND coalesce(t, ')') <= \
             '2026-10-26T12:00:00Z') AND x = 1"
        );
        assert_eq!(
            parameters.apply("WHERE $__timeFilter(t)", DatabaseType::MySQL.dialect()),
            "WHERE (t >= FROM_UNIXTIME(1792929600) AND t <= FROM_UNIXTIME(1793016000))"
        );
        assert_eq!(
            parameters.apply("WHERE $__timeFilter(t", DatabaseType::SQLite.dialect()),
            "WHERE $__timeFilter(t",
            "an unclosed macro stays as written"
        );
    }

    #[test]
    fn a_longer_name_is_not_a_shorter_variable() {
        let dialect = DatabaseType::SQLite.dialect();
        assert_eq!(
            parameters(TimeRange::Last7Days).apply("$region_id $region", dialect),
            "$region_id 'North''s'"
        );
    }

    #[test]
    fn ranges_round_trip_their_keys() {
        for range in TimeRange::ALL {
            assert_eq!(TimeRange::from_key(&range.key()), range);
        }
        assert_eq!(TimeRange::from_key("forever"), TimeRange::default());
        assert!(is_variable_name("region_2"));
        assert!(!is_variable_name("2region"));
        assert!(!is_variable_name("__timeFilter"));
    }
}
