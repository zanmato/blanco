//! Pairs the brackets of SQL text and gives each its nesting depth, for
//! coloring brackets by depth in the editor.
//!
//! The text is lexed with [`sql_regions`] rather than read from a syntax tree,
//! which is mostly error nodes while a statement is being typed: brackets in
//! strings, comments, quoted identifiers (SQL Server's and SQLite's `[name]`
//! among them) and dollar-quoted bodies are not brackets.

use std::ops::Range;

use crate::dialect::Dialect;
use crate::statement_stream::{SqlRegionKind, sql_regions};

/// A bracket in the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bracket {
    /// Byte offset of the bracket, one byte long.
    pub offset: usize,
    /// How many pairs enclose it, or `None` when it has no partner.
    pub depth: Option<u32>,
}

fn closing(open: u8) -> Option<u8> {
    match open {
        b'(' => Some(b')'),
        b'[' => Some(b']'),
        b'{' => Some(b'}'),
        _ => None,
    }
}

/// The brackets and statements of a script.
#[derive(Debug, Default)]
pub struct ScriptBrackets {
    /// Every bracket, in order of its offset.
    pub brackets: Vec<Bracket>,
    /// The statements the script's `;`s divide it into, in order, each from
    /// its first character that is not whitespace to its `;` or last such
    /// character.
    pub statements: Vec<Range<usize>>,
}

impl ScriptBrackets {
    /// The statement holding `cursor`, its end included, else the one
    /// nearest to it.
    pub fn statement_at(&self, cursor: usize) -> Option<Range<usize>> {
        self.statements
            .iter()
            .min_by_key(|statement| {
                if cursor < statement.start {
                    statement.start - cursor
                } else {
                    cursor.saturating_sub(statement.end)
                }
            })
            .cloned()
    }

    /// The brackets within `range`.
    pub fn brackets_in(&self, range: &Range<usize>) -> &[Bracket] {
        let first = self
            .brackets
            .partition_point(|bracket| bracket.offset < range.start);
        let end = self
            .brackets
            .partition_point(|bracket| bracket.offset < range.end);
        self.brackets.get(first..end).unwrap_or_default()
    }
}

/// The brackets of `sql` in order of their offsets.
///
/// A closing bracket that does not close the innermost open one has no
/// partner and leaves the open ones be. A `;` ends a statement, so brackets
/// still open there have no partner either, and one statement's missing `)`
/// does not shift the colors of every statement after it.
pub fn brackets(dialect: Dialect, sql: &str) -> Vec<Bracket> {
    script_brackets(dialect, sql).brackets
}

/// The brackets of `sql`, as [`brackets`] pairs them, and its statements.
/// Text after an unclosed quote or comment belongs to it, so a statement
/// being typed with a string still open runs to the end of the script.
pub fn script_brackets(dialect: Dialect, sql: &str) -> ScriptBrackets {
    let bytes = sql.as_bytes();
    let mut brackets: Vec<Bracket> = Vec::new();
    let mut statements: Vec<Range<usize>> = Vec::new();
    // Indexes into `brackets` of the open brackets, innermost last.
    let mut open: Vec<usize> = Vec::new();
    let mut statement_start = 0;
    let mut push_statement = |range: Range<usize>| {
        let text = bytes.get(range.clone()).unwrap_or_default();
        let leading = text.iter().take_while(|b| b.is_ascii_whitespace()).count();
        let trailing = text
            .iter()
            .rev()
            .take_while(|b| b.is_ascii_whitespace())
            .count();
        if leading < text.len() {
            statements.push(range.start + leading..range.end - trailing);
        }
    };

    for region in sql_regions(dialect, sql) {
        if region.kind != SqlRegionKind::Code {
            continue;
        }
        let Some(code) = bytes.get(region.range.clone()) else {
            continue;
        };
        for (ix, &byte) in code.iter().enumerate() {
            let offset = region.range.start + ix;
            match byte {
                b'(' | b'[' | b'{' => {
                    open.push(brackets.len());
                    brackets.push(Bracket {
                        offset,
                        depth: None,
                    });
                }
                b')' | b']' | b'}' => {
                    let closes_innermost = open
                        .last()
                        .and_then(|&ix| brackets.get(ix))
                        .and_then(|bracket| bytes.get(bracket.offset))
                        .and_then(|&opening| closing(opening))
                        == Some(byte);
                    let mut depth = None;
                    if closes_innermost && let Some(ix) = open.pop() {
                        let pair_depth = open.len() as u32;
                        if let Some(opening) = brackets.get_mut(ix) {
                            opening.depth = Some(pair_depth);
                        }
                        depth = Some(pair_depth);
                    }
                    brackets.push(Bracket { offset, depth });
                }
                b';' => {
                    open.clear();
                    push_statement(statement_start..offset + 1);
                    statement_start = offset + 1;
                }
                _ => {}
            }
        }
    }
    push_statement(statement_start..sql.len());
    ScriptBrackets {
        brackets,
        statements,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database_type::DatabaseType;

    fn depths(db_type: DatabaseType, sql: &str) -> Vec<(char, Option<u32>)> {
        brackets(db_type.dialect(), sql)
            .into_iter()
            .map(|bracket| (char::from(sql.as_bytes()[bracket.offset]), bracket.depth))
            .collect()
    }

    #[test]
    fn nested_pairs_get_their_depth() {
        assert_eq!(
            depths(
                DatabaseType::PostgreSQL,
                "SELECT REPLACE(REPLACE(description, '(', ''), ')', '')"
            ),
            vec![
                ('(', Some(0)),
                ('(', Some(1)),
                (')', Some(1)),
                (')', Some(0)),
            ]
        );
    }

    #[test]
    fn quoted_text_and_comments_hold_no_brackets() {
        let sql = "SELECT \"a(\", $$ ( $$, x -- (\n/* ) */ FROM t";
        assert_eq!(depths(DatabaseType::PostgreSQL, sql), vec![]);
        // `[name]` quotes an identifier on SQL Server, and is a subscript on
        // PostgreSQL.
        assert_eq!(depths(DatabaseType::MsSql, "SELECT [a(]"), vec![]);
        assert_eq!(
            depths(DatabaseType::PostgreSQL, "SELECT a[1]"),
            vec![('[', Some(0)), (']', Some(0))]
        );
    }

    #[test]
    fn brackets_without_a_partner_are_marked() {
        assert_eq!(
            depths(DatabaseType::PostgreSQL, "SELECT (1]) )"),
            vec![('(', Some(0)), (']', None), (')', Some(0)), (')', None)]
        );
        // A statement left open does not carry into the next one.
        assert_eq!(
            depths(DatabaseType::PostgreSQL, "SELECT (1; SELECT (2)"),
            vec![('(', None), ('(', Some(0)), (')', Some(0))]
        );
    }

    #[test]
    fn every_row_of_a_large_insert_is_paired() {
        let rows: Vec<String> = (0..100_000)
            .map(|ix| format!("({ix}, 'name ({ix})', ARRAY[{ix}, {ix}])"))
            .collect();
        let sql = format!(
            "INSERT INTO t (id, name, tags) VALUES {};",
            rows.join(",\n")
        );
        let found = brackets(DatabaseType::PostgreSQL.dialect(), &sql);
        assert_eq!(found.len(), 2 + 4 * 100_000);
        assert!(found.iter().all(|bracket| bracket.depth.is_some()));
    }

    #[test]
    fn statements_end_at_semicolons_outside_quotes() {
        let sql = "SELECT 1;\n\nSELECT ';' -- ;\n, (2);  \n";
        let script = script_brackets(DatabaseType::PostgreSQL.dialect(), sql);
        let texts: Vec<_> = script.statements.iter().map(|r| &sql[r.clone()]).collect();
        assert_eq!(texts, vec!["SELECT 1;", "SELECT ';' -- ;\n, (2);"]);
        // The end of a statement, its `;` passed, is still in it, and a
        // cursor between two is in the nearer one.
        assert_eq!(script.statement_at(9), Some(0..9));
        assert_eq!(script.statement_at(10), Some(0..9));
        assert_eq!(script.statement_at(11), Some(11..33));
        assert_eq!(script.brackets_in(&(0..9)), &[]);
        assert_eq!(script.brackets_in(&(11..33)).len(), 2);
    }

    #[test]
    fn an_open_string_runs_its_statement_to_the_end() {
        let sql = "SELECT 1; SELECT REPLACE('a', ';";
        let script = script_brackets(DatabaseType::PostgreSQL.dialect(), sql);
        assert_eq!(script.statements, vec![0..9, 10..sql.len()]);
        assert_eq!(
            script.brackets_in(&(10..sql.len())),
            &[Bracket {
                offset: 24,
                depth: None
            }]
        );
    }

    #[test]
    fn a_function_body_is_one_statement() {
        let sql = "CREATE FUNCTION f() RETURNS int AS $$\nBEGIN\n  RETURN (1);\nEND;\n$$ LANGUAGE plpgsql;\nSELECT f();";
        let script = script_brackets(DatabaseType::PostgreSQL.dialect(), sql);
        let texts: Vec<_> = script.statements.iter().map(|r| &sql[r.clone()]).collect();
        assert_eq!(texts.len(), 2);
        assert!(texts[0].ends_with("LANGUAGE plpgsql;"));
        // The body is quoted text, so only `f()` of each statement pairs.
        assert_eq!(script.brackets.len(), 4);
    }
}
