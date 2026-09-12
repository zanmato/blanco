pub mod completion;
pub mod formatting;
pub mod functions;
pub mod signature_help;

pub use completion::SqlCompletionProvider;
pub use formatting::SqruffService;
pub use signature_help::SqlSignatureHelpProvider;
pub use sql_parser::{
    extract_command_at_cursor, extract_statement_info, extract_statement_info_with_styles,
};

use std::ops::Range;

use gpui_component::highlighter::{LanguageConfig, LanguageRegistry};
use ropey::Rope;

/// Byte range of the SQL statement the cursor sits in.
///
/// `cursor` is a UTF-8 byte offset into `text`. This is the "active
/// statement": what Run executes, what the linter is pointed at, and what the
/// editor frames with its statement outline. Returns `None` when the parser
/// finds no statement in the buffer at all.
pub fn active_statement_range(text: &Rope, cursor: usize) -> Option<Range<usize>> {
    extract_statement_info(text, cursor).map(|info| info.byte_range)
}

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
