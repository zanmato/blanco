use anyhow::Result;
use blanco_core::HoverProvider;
use gpui::Task;
use ropey::Rope;

use crate::connection::SqliteConnection;

impl HoverProvider for SqliteConnection {
    fn hover(
        &self,
        _rope: &Rope,
        _offset: usize,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<gpui_component::input::InputState>,
    ) -> Task<Result<Option<lsp_types::Hover>>> {
        // TODO: Implement hover functionality for SQLite
        // For now, return no hover information
        Task::ready(Ok(None))
    }
}