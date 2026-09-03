//! The terminal pane a query or script tab can dock under its content. The
//! shell starts in the agent workspace folder with the MCP server's details and
//! the tab's identity in its environment, so a coding agent launched there
//! (claude, opencode, codex, ...) can target this very tab.

use std::collections::HashMap;

use blanco_core::ConnectionContext;
use blanco_terminal::element::TerminalStyle;
use blanco_terminal::palette::TerminalPalette;
use blanco_terminal::view::TerminalView;
use blanco_terminal::{Terminal, TerminalSpawn};
use gpui::{App, Entity, Subscription, Window, px};
use gpui_component::{ActiveTheme as _, Theme};

use crate::app_settings::AppSettings;
use crate::mcp::{McpService, McpStatus, discovery};

/// A tab's terminal: the view plus the subscription that tears the pane down
/// when the shell exits.
pub struct TerminalPane {
    pub view: Entity<TerminalView>,
    pub _subscription: Subscription,
}

/// Start the user's shell in the agent workspace for the tab identified by
/// `tab_id` and `context`. Agents read `BLANCO_TAB_ID` to address the tab
/// through MCP even after the user switches tabs.
pub fn spawn_tab_terminal(
    tab_id: u64,
    context: &ConnectionContext,
    window: &mut Window,
    cx: &mut App,
) -> anyhow::Result<Entity<Terminal>> {
    let workspace = discovery::workspace_dir();
    if let Err(error) = std::fs::create_dir_all(&workspace) {
        tracing::warn!("Failed to create the agent workspace: {error}");
    }
    let mut env = HashMap::new();
    env.insert(
        "BLANCO_MCP_WORKSPACE".to_string(),
        workspace.to_string_lossy().to_string(),
    );
    env.insert("BLANCO_TAB_ID".to_string(), tab_id.to_string());
    env.insert(
        "BLANCO_CONNECTION_ID".to_string(),
        context.connection_id.to_string(),
    );
    env.insert(
        "BLANCO_CONNECTION_NAME".to_string(),
        context.connection_name.clone(),
    );
    env.insert("BLANCO_DATABASE".to_string(), context.database_name.clone());
    if let McpStatus::Running { port, token } = McpService::global(cx).status() {
        env.insert("BLANCO_MCP_URL".to_string(), discovery::server_url(*port));
        env.insert("BLANCO_MCP_TOKEN".to_string(), token.clone());
    }

    let spawn = TerminalSpawn {
        program: None,
        args: Vec::new(),
        working_directory: Some(workspace),
        env,
        ..TerminalSpawn::default()
    };
    let window_id = window.window_handle().window_id().as_u64();
    Terminal::spawn(spawn, window_id, cx)
}

/// The terminal colors for the current theme: the theme's accent colors for
/// the ANSI slots and its text colors for the greys, so the pane looks like
/// the rest of the app in both light and dark themes.
pub fn palette_from_theme(theme: &Theme) -> TerminalPalette {
    let dark = theme.mode.is_dark();
    let (black, white, bright_white) = if dark {
        (theme.muted, theme.secondary_foreground, theme.foreground)
    } else {
        (theme.foreground, theme.muted, theme.background)
    };
    TerminalPalette {
        ansi: [
            black,
            theme.red,
            theme.green,
            theme.yellow,
            theme.blue,
            theme.magenta,
            theme.cyan,
            white,
            theme.muted_foreground,
            theme.red_light,
            theme.green_light,
            theme.yellow_light,
            theme.blue_light,
            theme.magenta_light,
            theme.cyan_light,
            bright_white,
        ],
        foreground: theme.foreground,
        background: theme.background,
        cursor: theme.caret,
        selection: theme.selection,
    }
}

/// The pane's style, read every frame so theme and font size changes apply
/// live.
pub fn style_from_theme(cx: &App) -> TerminalStyle {
    let theme = cx.theme();
    let font_size = AppSettings::global(cx)
        .settings
        .appearance
        .terminal_font_size;
    TerminalStyle {
        font_family: theme.mono_font_family.clone(),
        font_size: px(font_size),
        line_height_scale: 1.4,
        palette: palette_from_theme(theme),
    }
}
