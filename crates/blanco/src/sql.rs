pub mod completion;
pub mod expand_star;
pub mod formatting;
pub mod functions;
pub mod signature_help;

pub use completion::SqlCompletionProvider;
pub use formatting::SqruffService;
pub use signature_help::SqlSignatureHelpProvider;
pub use sql_parser::{
    extract_command_at_cursor, extract_statement_info, extract_statement_info_with_styles,
};

use std::collections::HashSet;
use std::ops::Range;
use std::sync::{LazyLock, Mutex, PoisonError};

use blanco_core::{DatabaseType, SqlGrammar};
use gpui_component::highlighter::{LanguageConfig, LanguageRegistry};
use ropey::Rope;

/// Byte range of the SQL statement the cursor sits in.
///
/// `cursor` is a UTF-8 byte offset into `text`. This is the "active
/// statement": what Run executes, what the linter is pointed at, and what the
/// editor frames with its statement outline. Returns `None` when the parser
/// finds no statement in the buffer at all.
pub fn active_statement_range(
    grammar: SqlGrammar,
    text: &Rope,
    cursor: usize,
) -> Option<Range<usize>> {
    extract_statement_info(grammar, text, cursor).map(|info| info.byte_range)
}

/// Register the generic SQL grammar for the code editor's syntax highlighting.
///
/// gpui-component is built without its bundled `tree-sitter-sql` feature, so we
/// register the grammars of `tree-sitter-sql-polyglot`, the same ones our
/// statement parser uses. Only the ANSI grammar is registered up front, for
/// SQL that belongs to no backend. A backend's grammar is registered by
/// [`highlight_language`] when the first editor for it opens.
pub fn register_languages() {
    ensure_registered(SqlGrammar::Ansi);
}

/// The highlighter language for buffers of `db_type`, registered on first use.
pub fn highlight_language(db_type: DatabaseType) -> &'static str {
    let language = db_type.editor_language();
    if let Some(grammar) = SqlGrammar::from_language_name(language) {
        ensure_registered(grammar);
    }
    language
}

/// The highlighter language of `grammar`, registered on first use.
pub fn grammar_language(grammar: SqlGrammar) -> &'static str {
    ensure_registered(grammar);
    grammar.language_name()
}

fn ensure_registered(grammar: SqlGrammar) {
    // Every registration bumps the registry's generation, which invalidates
    // highlight caches, so a grammar is registered once rather than per editor.
    static REGISTERED: LazyLock<Mutex<HashSet<SqlGrammar>>> =
        LazyLock::new(|| Mutex::new(HashSet::new()));
    let mut registered = REGISTERED.lock().unwrap_or_else(PoisonError::into_inner);
    if !registered.insert(grammar) {
        return;
    }
    let name = grammar.language_name();
    LanguageRegistry::singleton().register(
        name,
        &LanguageConfig::new(
            name,
            sql_parser::language(grammar),
            vec![],
            sql_parser::highlights_query(grammar),
            "",
            "",
        ),
    );
}
