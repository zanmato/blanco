//! Copying one column of a result: its values one per line, or as an IN list
//! for the WHERE clause of the next query.

use std::collections::HashSet;

use blanco_core::Dialect;
use gpui::{App, ClipboardItem, Context, Window};

use super::ResultsPanel;
use crate::app::CopyColumnValues;

/// The values one per line. NULL is written as `NULL`.
pub fn values_list<'a>(
    values: impl IntoIterator<Item = Option<&'a str>>,
    distinct: bool,
) -> String {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .map(|value| value.unwrap_or("NULL"))
        .filter(|value| !distinct || seen.insert(*value))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `('a', 'b')`, with strings escaped for `dialect` and the numbers of a
/// numeric column bare. NULL is left out, since `x IN (NULL)` never matches.
pub fn in_list<'a>(
    values: impl IntoIterator<Item = Option<&'a str>>,
    numeric: bool,
    dialect: Dialect,
    distinct: bool,
) -> String {
    let mut seen = HashSet::new();
    let items: Vec<String> = values
        .into_iter()
        .flatten()
        .filter(|value| !distinct || seen.insert(*value))
        .map(|value| {
            if numeric && is_plain_number(value) {
                value.trim().to_string()
            } else {
                dialect.string_literal(value)
            }
        })
        .collect();
    format!("({})", items.join(", "))
}

fn is_plain_number(value: &str) -> bool {
    let value = value.trim();
    value
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E'))
        && value.parse::<f64>().is_ok()
}

impl ResultsPanel {
    /// The rows a column copy takes: the selected ones when the action asks
    /// for them and more than one is selected, otherwise every row.
    fn column_copy_rows(&self, selected_only: bool, cx: &App) -> Vec<usize> {
        let table_state = self.table_state.read(cx);
        let selected_rows = table_state.selection().rows();
        if selected_only && selected_rows.len() > 1 {
            selected_rows
        } else {
            (0..table_state.delegate().rows.len()).collect()
        }
    }

    pub(super) fn on_copy_column_values(
        &mut self,
        action: &CopyColumnValues,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = self.column_copy_rows(action.selected_only, cx);
        if self.defer_action(
            rows.clone(),
            Some(action.column),
            action,
            window,
            cx,
            Self::on_copy_column_values,
        ) {
            return;
        }
        let delegate = self.table_state.read(cx).delegate();
        let values = rows.iter().map(|&row| {
            delegate
                .rows
                .get(row)
                .and_then(|cells| cells.get(action.column))
                .and_then(|value| value.as_deref())
        });
        let text = if action.as_in_list {
            let numeric = delegate
                .column_types
                .get(action.column)
                .is_some_and(|column_type| column_type.is_numeric());
            let dialect = delegate.db_type.unwrap_or(self.db_type).dialect();
            in_list(values, numeric, dialect, action.distinct)
        } else {
            values_list(values, action.distinct)
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use database::DatabaseType;

    #[test]
    fn test_values_list() {
        let values = [Some("a"), None, Some("b"), Some("a")];
        assert_eq!(values_list(values, false), "a\nNULL\nb\na");
        assert_eq!(values_list(values, true), "a\nNULL\nb");
    }

    #[test]
    fn test_in_list_escapes_and_leaves_out_null() {
        let postgres = DatabaseType::PostgreSQL.dialect();
        assert_eq!(
            in_list([Some("O'Brien"), None, Some("x")], false, postgres, false),
            "('O''Brien', 'x')"
        );
        assert_eq!(
            in_list(
                [Some("1"), Some("2"), Some("1"), None],
                true,
                postgres,
                true
            ),
            "(1, 2)"
        );
        // MySQL escapes backslashes too.
        assert_eq!(
            in_list([Some(r"a\b")], false, DatabaseType::MySQL.dialect(), false),
            r"('a\\b')"
        );
        // A non-number in a numeric column is still quoted.
        assert_eq!(in_list([Some("NaN")], true, postgres, false), "('NaN')");
    }
}
