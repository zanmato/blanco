//! Signature help for SQL tabs: the built-in catalog of the connection's
//! dialect plus the user-defined functions of the current schema.

use crate::sql::completion::SqlCompletionProvider;
use crate::sql::functions::{self, FunctionSignature};
use anyhow::Result;
use blanco_core::{DatabaseType, FunctionSignatureInfo};
use gpui::{App, AppContext as _, Task, Window};
use gpui_component::input::{Rope, SignatureHelpProvider};
use lsp_types::{
    Documentation, MarkupContent, MarkupKind, ParameterInformation, ParameterLabel, SignatureHelp,
    SignatureInformation,
};
use sql_parser::statement_parser::{self, EnclosingCall};
use std::sync::Arc;

pub struct SqlSignatureHelpProvider {
    completion: SqlCompletionProvider,
}

impl SqlSignatureHelpProvider {
    pub fn new(completion: SqlCompletionProvider) -> Self {
        Self { completion }
    }
}

/// Build the LSP signature for a user-defined function, mirroring
/// [`FunctionSignature::to_lsp_signature`].
fn udf_signature(function: &FunctionSignatureInfo) -> SignatureInformation {
    let mut label = format!("{}({})", function.name, function.parameters.join(", "));
    if let Some(return_type) = function.return_type.as_deref().filter(|t| !t.is_empty()) {
        label.push_str(" -> ");
        label.push_str(return_type);
    }
    let mut parameters = Vec::with_capacity(function.parameters.len());
    let mut offset = function.name.encode_utf16().count() + 1;
    for (index, parameter) in function.parameters.iter().enumerate() {
        let length = parameter.encode_utf16().count();
        parameters.push(ParameterInformation {
            label: ParameterLabel::LabelOffsets([offset as u32, (offset + length) as u32]),
            documentation: None,
        });
        offset += length;
        if index + 1 < function.parameters.len() {
            offset += 2;
        }
    }
    let mut documentation = String::new();
    if let Some(schema) = &function.schema {
        documentation.push_str(&format!("*User-defined function in `{schema}`*"));
    }
    if let Some(comment) = function.comment.as_deref().filter(|c| !c.is_empty()) {
        if !documentation.is_empty() {
            documentation.push_str("\n\n");
        }
        documentation.push_str(comment);
    }
    SignatureInformation {
        label,
        documentation: (!documentation.is_empty()).then_some(Documentation::MarkupContent(
            MarkupContent {
                kind: MarkupKind::Markdown,
                value: documentation,
            },
        )),
        parameters: Some(parameters),
        active_parameter: None,
    }
}

/// Whether a signature with `parameter_count` parameters (variadic or not) can
/// accept an argument at `argument_index`.
fn accepts_argument(parameter_count: usize, variadic: bool, argument_index: usize) -> bool {
    variadic || argument_index < parameter_count
}

/// The signature help for `call`, or `None` when no built-in or user-defined
/// function matches its name.
pub fn build_signature_help(
    call: &EnclosingCall,
    driver: DatabaseType,
    user_functions: &[FunctionSignatureInfo],
) -> Option<SignatureHelp> {
    // (signature, parameter count, variadic). User-defined functions first so
    // a UDF shadowing a built-in name shows up as the primary overload.
    let mut candidates: Vec<(SignatureInformation, usize, bool)> = user_functions
        .iter()
        .filter(|function| function.name.eq_ignore_ascii_case(&call.name))
        .map(|function| {
            let variadic = function
                .parameters
                .last()
                .is_some_and(|last| last.starts_with("VARIADIC "));
            (udf_signature(function), function.parameters.len(), variadic)
        })
        .collect();
    candidates.extend(functions::lookup(driver, &call.name).iter().map(
        |function: &FunctionSignature| {
            (
                function.to_lsp_signature(),
                function.parameters.len(),
                function.is_variadic(),
            )
        },
    ));
    if candidates.is_empty() {
        return None;
    }

    // Prefer the first overload that has room for the argument being typed;
    // otherwise fall back to the widest one so the popover stays useful.
    let active_signature = candidates
        .iter()
        .position(|(_, count, variadic)| accepts_argument(*count, *variadic, call.argument_index))
        .or_else(|| {
            candidates
                .iter()
                .enumerate()
                .max_by_key(|(_, (_, count, _))| *count)
                .map(|(index, _)| index)
        })
        .unwrap_or(0);

    let signatures = candidates
        .into_iter()
        .map(|(mut signature, count, _)| {
            signature.active_parameter =
                (count > 0).then(|| call.argument_index.min(count.saturating_sub(1)) as u32);
            signature
        })
        .collect();

    Some(SignatureHelp {
        signatures,
        active_signature: Some(active_signature as u32),
        active_parameter: None,
    })
}

impl SignatureHelpProvider for SqlSignatureHelpProvider {
    fn signature_help(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<SignatureHelp>>> {
        let rope = text.clone();
        let driver = self.completion.driver;
        let schema = self.completion.current_schema.clone();

        // The UDF cache is read without IO; a cold cache is warmed in the
        // background so the next keystroke includes user functions.
        let user_functions: Arc<Vec<FunctionSignatureInfo>> =
            match self.completion.try_get_cached_functions(&schema) {
                Some(cached) => cached,
                None => {
                    let provider = self.completion.clone();
                    cx.spawn(async move |_| {
                        if let Err(error) = provider.get_cached_functions(&schema).await {
                            tracing::warn!("Failed to fetch function signatures: {error}");
                        }
                    })
                    .detach();
                    Arc::new(Vec::new())
                }
            };

        cx.background_spawn(async move {
            let offset = offset.min(rope.len());
            let Some(context) = statement_parser::extract_completion_context(&rope, offset) else {
                return Ok(None);
            };
            let Some(call) = context.enclosing_call else {
                return Ok(None);
            };
            Ok(build_signature_help(&call, driver, &user_functions))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, argument_index: usize) -> EnclosingCall {
        EnclosingCall {
            name: name.to_string(),
            argument_index,
            open_paren: 0,
        }
    }

    fn active_label(help: &SignatureHelp) -> (&str, Option<u32>) {
        let signature = &help.signatures[help.active_signature.unwrap_or(0) as usize];
        (signature.label.as_str(), signature.active_parameter)
    }

    #[test]
    fn builtin_split_part_tracks_active_parameter() {
        let help = build_signature_help(&call("SPLIT_PART", 2), DatabaseType::PostgreSQL, &[])
            .expect("split_part is a postgres built-in");
        assert_eq!(
            active_label(&help),
            ("split_part(text, text, integer) -> text", Some(2))
        );
        // Beyond the last parameter the last one stays highlighted.
        let help = build_signature_help(&call("split_part", 7), DatabaseType::PostgreSQL, &[])
            .expect("split_part");
        assert_eq!(active_label(&help).1, Some(2));
    }

    #[test]
    fn overload_selection_prefers_signature_with_room() {
        // round(numeric) and round(numeric, integer): typing the second argument
        // must select the two-parameter overload.
        let help =
            build_signature_help(&call("round", 1), DatabaseType::PostgreSQL, &[]).expect("round");
        assert!(help.signatures.len() >= 2);
        let (label, active) = active_label(&help);
        assert!(label.starts_with("round(numeric, integer)"), "got {label}");
        assert_eq!(active, Some(1));
    }

    #[test]
    fn unknown_or_other_dialect_yields_none() {
        assert!(build_signature_help(&call("split_part", 0), DatabaseType::MySQL, &[]).is_none());
        assert!(
            build_signature_help(&call("no_such_fn", 0), DatabaseType::PostgreSQL, &[]).is_none()
        );
        assert!(
            build_signature_help(&call("SUBSTRING_INDEX", 1), DatabaseType::MySQL, &[]).is_some()
        );
    }

    #[test]
    fn user_defined_functions_come_first_and_carry_offsets() {
        let udf = FunctionSignatureInfo {
            schema: Some("public".into()),
            name: "add1".into(),
            parameters: vec!["x integer".into(), "y integer DEFAULT 0".into()],
            return_type: Some("integer".into()),
            comment: Some("adds".into()),
        };
        let help =
            build_signature_help(&call("ADD1", 1), DatabaseType::PostgreSQL, &[udf]).expect("udf");
        assert_eq!(help.signatures.len(), 1);
        let signature = &help.signatures[0];
        assert_eq!(
            signature.label,
            "add1(x integer, y integer DEFAULT 0) -> integer"
        );
        let parameters = signature.parameters.as_ref().expect("parameters");
        let ParameterLabel::LabelOffsets([start, end]) = parameters[1].label else {
            panic!("offsets");
        };
        assert_eq!(
            &signature.label[start as usize..end as usize],
            "y integer DEFAULT 0"
        );
        assert_eq!(signature.active_parameter, Some(1));
    }
}
