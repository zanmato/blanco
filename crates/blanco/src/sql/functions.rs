//! Built-in function catalogs per SQL dialect, used for completion and
//! signature help.
//!
//! `postgres.rs`, `mysql.rs` and `clickhouse.rs` are generated from live
//! servers by `scripts/extract-sql-functions.py`; `sqlite.rs` and `mssql.rs`
//! are curated by hand because those engines expose no signature catalog.
//! Every list is sorted by lowercased name (then arity) so overloads of a name
//! are adjacent and lookups can binary-search.

use blanco_core::DatabaseType;
use lsp_types::{Documentation, MarkupContent, MarkupKind, ParameterInformation, ParameterLabel};
use std::sync::OnceLock;

mod clickhouse;
mod mssql;
mod mysql;
mod postgres;
mod sqlite;

/// One signature of a built-in function. A function with several overloads
/// contributes one entry per overload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FunctionSignature {
    /// The name in the dialect's preferred spelling.
    pub name: &'static str,
    /// One entry per parameter, as shown to the user (`"string text"`,
    /// `"delimiter"`, `"[, max_substrings]"`).
    pub parameters: &'static [&'static str],
    /// Empty when the dialect does not report it.
    pub return_type: &'static str,
    /// Markdown. May be empty.
    pub documentation: &'static str,
}

impl FunctionSignature {
    /// `name(param, param) -> return_type`
    pub fn label(&self) -> String {
        let mut label = format!("{}({})", self.name, self.parameters.join(", "));
        if !self.return_type.is_empty() {
            label.push_str(" -> ");
            label.push_str(self.return_type);
        }
        label
    }

    pub fn is_variadic(&self) -> bool {
        self.parameters
            .last()
            .is_some_and(|last| last.contains("...") || last.starts_with("VARIADIC "))
    }

    pub fn lsp_documentation(&self) -> Option<Documentation> {
        (!self.documentation.is_empty()).then(|| {
            Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: self.documentation.to_string(),
            })
        })
    }

    /// The LSP signature, with parameter label offsets computed from
    /// [`Self::label`] so the popover can highlight the active one.
    pub fn to_lsp_signature(&self) -> lsp_types::SignatureInformation {
        let label = self.label();
        let mut parameters = Vec::with_capacity(self.parameters.len());
        // Offsets are UTF-16 code units per the LSP spec; the catalogs are
        // ASCII so byte and UTF-16 offsets coincide, but count properly anyway.
        let mut utf16_offset = self.name.encode_utf16().count() + 1;
        for (index, parameter) in self.parameters.iter().enumerate() {
            let length = parameter.encode_utf16().count();
            parameters.push(ParameterInformation {
                label: ParameterLabel::LabelOffsets([
                    utf16_offset as u32,
                    (utf16_offset + length) as u32,
                ]),
                documentation: None,
            });
            utf16_offset += length;
            if index + 1 < self.parameters.len() {
                utf16_offset += 2;
            }
        }
        lsp_types::SignatureInformation {
            label,
            documentation: self.lsp_documentation(),
            parameters: Some(parameters),
            active_parameter: None,
        }
    }
}

/// All built-in signatures for a dialect, sorted by lowercased name.
pub fn builtin_functions(dialect: DatabaseType) -> &'static [FunctionSignature] {
    match dialect {
        DatabaseType::PostgreSQL => postgres::FUNCTIONS,
        DatabaseType::MySQL => mysql::FUNCTIONS,
        DatabaseType::SQLite => sqlite::FUNCTIONS,
        DatabaseType::MsSql => mssql::FUNCTIONS,
        DatabaseType::ClickHouse => clickhouse::FUNCTIONS,
        DatabaseType::Redis => &[],
    }
}

fn compare_names(a: &str, b: &str) -> std::cmp::Ordering {
    a.chars()
        .map(|c| c.to_ascii_lowercase())
        .cmp(b.chars().map(|c| c.to_ascii_lowercase()))
}

/// Every overload of `name` in the dialect, case-insensitively. Empty when the
/// function is unknown.
pub fn lookup(dialect: DatabaseType, name: &str) -> &'static [FunctionSignature] {
    let functions = builtin_functions(dialect);
    let start = functions.partition_point(|f| compare_names(f.name, name).is_lt());
    let end = start + functions[start..].partition_point(|f| compare_names(f.name, name).is_eq());
    &functions[start..end]
}

/// One entry per distinct function name: the overload run as a subslice.
#[derive(Debug, Clone, Copy)]
pub struct FunctionGroup {
    pub name: &'static str,
    pub overloads: &'static [FunctionSignature],
}

fn group_by_name(functions: &'static [FunctionSignature]) -> Vec<FunctionGroup> {
    let mut groups: Vec<FunctionGroup> = Vec::new();
    let mut start = 0;
    while start < functions.len() {
        let name = functions[start].name;
        let end =
            start + functions[start..].partition_point(|f| compare_names(f.name, name).is_eq());
        groups.push(FunctionGroup {
            name,
            overloads: &functions[start..end],
        });
        start = end;
    }
    groups
}

/// The distinct function names of a dialect with their overloads, in catalog
/// order. Built once per dialect.
pub fn function_groups(dialect: DatabaseType) -> &'static [FunctionGroup] {
    static GROUPS: [OnceLock<Vec<FunctionGroup>>; 6] = [
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
    ];
    let slot = match dialect {
        DatabaseType::PostgreSQL => 0,
        DatabaseType::MySQL => 1,
        DatabaseType::SQLite => 2,
        DatabaseType::MsSql => 3,
        DatabaseType::ClickHouse => 4,
        DatabaseType::Redis => 5,
    };
    GROUPS[slot].get_or_init(|| group_by_name(builtin_functions(dialect)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQL_DIALECTS: [DatabaseType; 5] = [
        DatabaseType::PostgreSQL,
        DatabaseType::MySQL,
        DatabaseType::SQLite,
        DatabaseType::MsSql,
        DatabaseType::ClickHouse,
    ];

    #[test]
    fn catalogs_are_sorted_and_non_empty() {
        for dialect in SQL_DIALECTS {
            let functions = builtin_functions(dialect);
            assert!(!functions.is_empty(), "{dialect:?} catalog is empty");
            for pair in functions.windows(2) {
                assert!(
                    compare_names(pair[0].name, pair[1].name).is_le(),
                    "{dialect:?}: {} sorts after {}",
                    pair[0].name,
                    pair[1].name
                );
            }
            for function in functions {
                assert!(!function.name.is_empty());
            }
        }
        assert!(builtin_functions(DatabaseType::Redis).is_empty());
    }

    #[test]
    fn lookup_is_case_insensitive_and_dialect_specific() {
        let split_part = lookup(DatabaseType::PostgreSQL, "SPLIT_PART");
        assert_eq!(split_part.len(), 1);
        assert_eq!(split_part[0].parameters, &["text", "text", "integer"]);
        assert_eq!(split_part[0].return_type, "text");
        assert!(lookup(DatabaseType::MySQL, "split_part").is_empty());
        assert!(!lookup(DatabaseType::MySQL, "substring_index").is_empty());
        assert!(!lookup(DatabaseType::PostgreSQL, "jsonb_to_recordset").is_empty());
        assert!(!lookup(DatabaseType::ClickHouse, "splitbychar").is_empty());
        assert!(!lookup(DatabaseType::SQLite, "json_extract").is_empty());
        assert!(!lookup(DatabaseType::MsSql, "datediff").is_empty());
        assert!(lookup(DatabaseType::PostgreSQL, "no_such_function").is_empty());
    }

    #[test]
    fn overloads_are_grouped() {
        let round = lookup(DatabaseType::PostgreSQL, "round");
        assert!(round.len() >= 2, "round should have overloads");
        assert!(round.iter().all(|f| f.name == "round"));

        let groups = function_groups(DatabaseType::PostgreSQL);
        let round_group = groups
            .iter()
            .find(|g| g.name == "round")
            .expect("round group");
        assert_eq!(round_group.overloads.len(), round.len());
        let names: std::collections::HashSet<&str> = groups.iter().map(|g| g.name).collect();
        assert_eq!(names.len(), groups.len(), "group names must be unique");
    }

    #[test]
    fn lsp_signature_offsets_slice_back_to_parameters() {
        let signature = FunctionSignature {
            name: "split_part",
            parameters: &["string text", "delimiter text", "n integer"],
            return_type: "text",
            documentation: "docs",
        };
        assert_eq!(
            signature.label(),
            "split_part(string text, delimiter text, n integer) -> text"
        );
        let lsp = signature.to_lsp_signature();
        let parameters = lsp.parameters.expect("parameters");
        for (parameter, expected) in parameters.iter().zip(signature.parameters) {
            let ParameterLabel::LabelOffsets([start, end]) = parameter.label else {
                panic!("expected offsets");
            };
            assert_eq!(&lsp.label[start as usize..end as usize], *expected);
        }
        assert!(
            FunctionSignature {
                name: "concat",
                parameters: &["str1", "str2", "..."],
                return_type: "",
                documentation: "",
            }
            .is_variadic()
        );
    }
}
