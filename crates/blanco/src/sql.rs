pub mod completion;
pub mod formatting;
pub mod functions;
pub mod selection_range_provider;
pub mod signature_help;

pub use completion::SqlCompletionProvider;
pub use formatting::SqruffService;
pub use selection_range_provider::SqlSelectionRangeProvider;
pub use signature_help::SqlSignatureHelpProvider;
pub use sql_parser::{
    extract_command_at_cursor, extract_statement_info, extract_statement_info_with_styles,
};

use gpui_component::highlighter::{LanguageConfig, LanguageRegistry};

/// Register the SQL grammar for the code editor's syntax highlighting.
///
/// gpui-component is built without its bundled `tree-sitter-sql` feature, so we
/// register our own vendored `tree-sitter-sequel` grammar here. This keeps the
/// editor highlighting and our statement parser on one shared SQL grammar.
pub fn register_languages() {
    LanguageRegistry::singleton().register(
        "sql",
        &LanguageConfig::new(
            "sql",
            tree_sitter_sequel::LANGUAGE.into(),
            vec![],
            tree_sitter_sequel::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
    );
}
