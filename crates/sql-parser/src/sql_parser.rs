//! Tree-sitter based SQL statement parsing shared by the editor (statement at
//! cursor, bind parameters, completion context) and the agent tools. Pure: no
//! UI dependencies, so it builds and tests without GPUI.

pub mod explain;
pub mod statement_parser;

pub use statement_parser::{
    CompletionContext, ParameterStyle, QueryParameter, StatementInfo, contains_node_kind,
    extract_command_at_cursor, extract_completion_context, extract_statement_info,
    extract_statement_info_with_styles, ident_eq, strip_identifier_quotes,
};
