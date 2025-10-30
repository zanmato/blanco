use anyhow::Result;
use gpui::{Task, Window};
use ropey::Rope;
use std::sync::Arc;

// Re-export LSP types publicly
pub use lsp_types::{CompletionContext, CompletionResponse, Hover};

/// Position in a text document (0-indexed)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

impl Position {
    pub fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

/// Represents a range in a text document
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl Range {
    pub fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }
}

/// Trait for providing auto-completion functionality
/// This trait should be implemented by database connections to provide
/// intelligent SQL completion based on database schema and query context.
pub trait CompletionProvider: Send + Sync {
    /// Provide completion suggestions for the given position in the text
    ///
    /// # Arguments
    /// * `rope` - The text content of the document
    /// * `offset` - The character offset from the beginning of the document
    /// * `trigger` - The completion context (trigger character, etc.)
    /// * `window` - GPUI window context
    /// * `cx` - Application context for the input state
    ///
    /// # Returns
    /// A task that resolves to a completion response with suggestions
    fn completions(
        &self,
        rope: &Rope,
        offset: usize,
        trigger: CompletionContext,
        window: &mut Window,
        cx: &mut gpui::Context<gpui_component::input::InputState>,
    ) -> Task<Result<CompletionResponse>>;
}

/// Trait for providing hover information
/// This trait should be implemented by database connections to provide
/// contextual information when hovering over SQL identifiers.
pub trait HoverProvider: Send + Sync {
    /// Provide hover information for the given position in the text
    ///
    /// # Arguments
    /// * `rope` - The text content of the document
    /// * `offset` - The character offset from the beginning of the document
    /// * `window` - GPUI window context
    /// * `cx` - Application context for the input state
    ///
    /// # Returns
    /// A task that resolves to an optional hover response with information
    fn hover(
        &self,
        rope: &Rope,
        offset: usize,
        window: &mut Window,
        cx: &mut gpui::Context<gpui_component::input::InputState>,
    ) -> Task<Result<Option<Hover>>>;
}

/// Helper trait for combining completion and hover providers
pub trait LanguageProvider: CompletionProvider + HoverProvider {
    /// Get the name of the language/provider
    fn name(&self) -> &'static str;

    /// Check if this provider can handle the given file/content
    fn can_handle(&self, content: &str) -> bool {
        // Default implementation checks for common SQL keywords
        let content_lower = content.to_lowercase();
        content_lower.contains("select") ||
        content_lower.contains("from") ||
        content_lower.contains("insert") ||
        content_lower.contains("update") ||
        content_lower.contains("delete") ||
        content_lower.contains("create") ||
        content_lower.contains("drop")
    }
}

/// Blanket implementation for all types that implement both CompletionProvider and HoverProvider
impl<T> LanguageProvider for T
where
    T: CompletionProvider + HoverProvider + Send + Sync,
{
    fn name(&self) -> &'static str {
        "sql"
    }
}

/// A registry for managing multiple language providers
pub struct LanguageProviderRegistry {
    providers: Vec<Arc<dyn LanguageProvider>>,
}

impl LanguageProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Register a new language provider
    pub fn register_provider(&mut self, provider: Arc<dyn LanguageProvider>) {
        self.providers.push(provider);
    }

    /// Find a provider that can handle the given content
    pub fn find_provider(&self, content: &str) -> Option<Arc<dyn LanguageProvider>> {
        self.providers
            .iter()
            .find(|provider| provider.can_handle(content))
            .cloned()
    }

    /// Get all registered providers
    pub fn get_providers(&self) -> &[Arc<dyn LanguageProvider>] {
        &self.providers
    }
}

impl Default for LanguageProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

