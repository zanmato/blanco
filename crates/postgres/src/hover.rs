// Placeholder hover for PostgreSQL
// TODO: Implement PostgreSQL-specific hover logic

use anyhow::Result;
use blanco_core::{HoverProvider, Hover, Position};
use gpui::{Task, Window};
use gpui_component::input::InputState;
use ropey::Rope;

use crate::connection::PostgresConnection;

impl HoverProvider for PostgresConnection {
    fn hover(
        &self,
        _rope: &Rope,
        _offset: usize,
        _window: &mut Window,
        _cx: &mut gpui::Context<InputState>,
    ) -> Task<Result<Option<Hover>>> {
        // TODO: Implement PostgreSQL hover logic
        Task::ready(Ok(None))
    }
}