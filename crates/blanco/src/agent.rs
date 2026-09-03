mod agent_chat;
#[cfg(test)]
mod agent_chat_test;
mod chat_provider_resolver;
// mod chat_message;
mod chat_message_view;
mod chat_session;
mod chat_types;
mod openai_compatible;
mod streaming;
pub(crate) mod tool_handlers;

// Re-export main types for the editor panel
pub use agent_chat::ChatPanel;
pub use chat_provider_resolver::ChatProviderResolver;
pub use chat_session::{ChatSessionContext, TabLanguage};
