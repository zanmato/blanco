//! Sqruff service for SQL linting and formatting.
//!
//! This module provides integration with sqruff-lib for real-time SQL linting
//! and formatting functionality. Linting is scoped to the current query/statement
//! that the cursor is in, not the entire file.

use gpui_component::highlighter::{Diagnostic, DiagnosticSeverity};
use gpui_component::input::Position;
use sqruff_lib::core::config::{FluffConfig, Value};
use sqruff_lib::core::linter::core::Linter;
use std::sync::Mutex;

/// Sqruff service for SQL linting and formatting.
///
/// This service wraps sqruff-lib to provide:
/// - Linting of SQL statements with configurable dialects
/// - Formatting of SQL statements
/// - Conversion of sqruff violations to gpui-component diagnostics
pub struct SqruffService {
    linter: Mutex<Linter>,
}

impl SqruffService {
    /// Create a new SqruffService with the specified dialect.
    ///
    /// # Arguments
    /// * `dialect` - SQL dialect to use for linting/formatting
    ///
    /// # Returns
    /// Result with the service or an error message
    pub fn new(dialect: &str) -> Result<Self, String> {
        let mut configs = ahash::AHashMap::new();

        let mut core_config = ahash::AHashMap::new();
        core_config.insert("dialect".to_string(), Value::String(dialect.into()));
        // Exclude LT12 (files must end with trailing newline) as it's not relevant for editor queries
        core_config.insert("exclude_rules".to_string(), Value::String("LT12".into()));
        configs.insert("core".to_string(), Value::Map(core_config));

        let mut indentation_config = ahash::AHashMap::new();
        indentation_config.insert("tab_space_size".to_string(), Value::Int(2));
        indentation_config.insert("indented_joins".to_string(), Value::Bool(false));
        indentation_config.insert("indent_unit".to_string(), Value::String("space".into()));
        configs.insert("indentation".to_string(), Value::Map(indentation_config));

        let config = FluffConfig::new(configs, None, None);
        let linter = Linter::new(config, None, None, false);

        Ok(Self {
            linter: Mutex::new(linter),
        })
    }

    /// Lint SQL text and return diagnostics.
    ///
    /// # Arguments
    /// * `sql` - The SQL text to lint
    /// * `statement_offset` - Optional byte offset to add to diagnostics
    ///                      (useful when linting a sub-section of a larger file)
    ///
    /// # Returns
    /// Vector of diagnostics or an error message
    pub fn lint(
        &self,
        sql: &str,
        _statement_offset: Option<usize>,
    ) -> Result<Vec<Diagnostic>, String> {
        if sql.trim().is_empty() {
            return Ok(Vec::new());
        }

        let mut linter = self
            .linter
            .lock()
            .map_err(|e| format!("Linter lock poisoned: {}", e))?;
        let linted_file = linter.lint_string_wrapped(sql, false);
        drop(linter);

        let violations = linted_file.violations();

        // Sqruff provides positions relative to the SQL text we passed in (the statement)
        // We return diagnostics relative to the statement text
        // The caller is responsible for adjusting positions to the full file if needed
        let diagnostics: Vec<Diagnostic> = violations
            .iter()
            .filter_map(|violation| {
                // Convert sqruff violation to gpui_component Diagnostic
                let severity = Self::severity_for_rule(violation.rule_code());

                // sqruff uses 1-indexed line numbers and column positions
                let line_no = violation.line_no.saturating_sub(1) as u32;
                let col_no = violation.line_pos.saturating_sub(1) as u32;

                // Calculate end position based on source_slice length
                let length = if violation.source_slice.end > violation.source_slice.start {
                    (violation.source_slice.end - violation.source_slice.start) as u32
                } else {
                    10
                };

                let start = Position::new(line_no, col_no);
                let end = Position::new(line_no, col_no + length);

                let message = format!("{}: {}", violation.rule_code(), violation.description);

                Some(Diagnostic::new(start..end, message).with_severity(severity))
            })
            .collect();

        Ok(diagnostics)
    }

    /// Format SQL text.
    ///
    /// # Arguments
    /// * `sql` - The SQL text to format
    ///
    /// # Returns
    /// Formatted SQL string or an error message
    pub fn format(&self, sql: &str) -> Result<String, String> {
        if sql.trim().is_empty() {
            return Ok(sql.to_string());
        }

        let mut linter = self
            .linter
            .lock()
            .map_err(|e| format!("Linter lock poisoned: {}", e))?;
        let linted_file = linter.lint_string_wrapped(sql, true);
        drop(linter);

        // Apply fixes to get formatted SQL
        let formatted = linted_file.fix_string();

        if formatted.is_empty() {
            tracing::warn!("Sqruff formatter produced empty output, using original");
            Ok(sql.to_string())
        } else {
            Ok(formatted)
        }
    }

    /// Map sqruff rule code to diagnostic severity.
    fn severity_for_rule(rule_code: &str) -> DiagnosticSeverity {
        // Sqruff rule codes: CP01, LT02, AL01, etc.
        // CP = Capitalization, LT = Layout, AL = Aliasing, etc.
        // Most are warnings, but some layout issues could be hints
        match &rule_code[..2.min(rule_code.len())] {
            "LT" => DiagnosticSeverity::Hint,    // Layout rules are hints
            "CV" => DiagnosticSeverity::Warning, // Convention violations are warnings
            "ST" => DiagnosticSeverity::Error,   // Structure issues can be errors
            "RF" => DiagnosticSeverity::Warning, // Reference issues are warnings
            "AM" => DiagnosticSeverity::Warning, // Ambiguous code is warning
            "JJ" => DiagnosticSeverity::Warning, // Jinja issues are warnings
            _ => DiagnosticSeverity::Warning,    // Default to warning
        }
    }
}
