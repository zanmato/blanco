//! Tab data structures used by the editor panel.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use blanco_core::RoutineKind;
use blanco_ui::SqlView;
use gpui::{App, Entity};
use gpui_component::input::EditorState;

use crate::agent::{ChatPanel, ChatSessionContext, TabLanguage};
use crate::results_panel::ResultsPanel;
use crate::settings::SettingsView;
use crate::sql::{SqlCompletionProvider, SqruffService};
use blanco_core::ConnectionContext;

use super::object_ddl::ObjectDdlTab;
use super::snippet_editor::SnippetEditor;
use super::table_structure::TableStructureTab;

pub enum TabType {
    Query(Box<QueryTab>),
    Script(Box<ScriptTab>),
    Settings(SettingsTab),
    Snippet(Entity<SnippetEditor>),
    TableStructure(Entity<TableStructureTab>),
    ObjectDdl(Entity<ObjectDdlTab>),
    SchemaGraph(Entity<super::schema_graph::SchemaGraphTab>),
}

/// What every tab bound to a connection has in common: the identity it works
/// against, its code editor, results grid and chat, and the app-database row
/// it persists to. [`QueryTab`] and [`ScriptTab`] embed one and deref to it,
/// so the panel treats the two kinds alike wherever the buffer's language does
/// not matter.
pub struct ConnectionBackedTab {
    pub title: String,
    pub context: ConnectionContext,
    pub editor: Entity<EditorState>,
    pub db_id: Option<i64>,
    /// Unix seconds of the last run, shown in the tab overflow menu.
    pub last_run_at: Option<i64>,
    pub results_panel: Entity<ResultsPanel>,
    pub chat_enabled: bool,
    pub chat_panel: Option<Entity<ChatPanel>>,
}

impl ConnectionBackedTab {
    /// The chat session context for this tab's buffer.
    pub fn chat_session_context(&self, language: TabLanguage) -> ChatSessionContext {
        ChatSessionContext::new()
            .with_input_state(self.editor.downgrade())
            .with_connection(
                self.context.connection_id,
                self.context.database_name.clone(),
                self.context.db_type,
            )
            .with_tab_language(language)
    }
}

pub struct QueryTab {
    pub base: ConnectionBackedTab,
    pub sql_view: Entity<SqlView>,
    /// Log shown in the "Apply edits" popover. Held here rather than created
    /// in the popover's content closure, which re-runs every render and would
    /// reset the view's scroll position.
    pub commit_preview: Entity<SqlView>,
    pub sql_view_visible: bool,
    pub sqruff_service: Option<Arc<SqruffService>>,
    pub completion_provider: Option<SqlCompletionProvider>,
    /// In-memory cache of the last values entered for query parameters in
    /// this tab, keyed by parameter label (e.g. "$1" or ":user_id"). Used
    /// to prefill the parameter modal on subsequent runs within the same
    /// session.
    pub last_parameter_values: HashMap<String, String>,
}

/// A JavaScript script tab. Mirrors [`QueryTab`] minus the SQL-only machinery
/// (formatter, linter, SQL completion, parameter modal): a script's "query" is
/// an arbitrary program, so none of those apply. It keeps the results panel
/// (fed only by explicit `db.display(...)` calls), the log view that the
/// script's `console` output streams into, and the chat panel, whose session is
/// told the tab holds JavaScript.
pub struct ScriptTab {
    pub base: ConnectionBackedTab,
    pub log_view: Entity<SqlView>,
    pub log_visible: bool,
}

impl Deref for QueryTab {
    type Target = ConnectionBackedTab;
    fn deref(&self) -> &ConnectionBackedTab {
        &self.base
    }
}

impl DerefMut for QueryTab {
    fn deref_mut(&mut self) -> &mut ConnectionBackedTab {
        &mut self.base
    }
}

impl Deref for ScriptTab {
    type Target = ConnectionBackedTab;
    fn deref(&self) -> &ConnectionBackedTab {
        &self.base
    }
}

impl DerefMut for ScriptTab {
    fn deref_mut(&mut self) -> &mut ConnectionBackedTab {
        &mut self.base
    }
}

impl TabType {
    /// The tab strip label.
    pub fn title(&self, cx: &App) -> String {
        match self {
            TabType::Query(tab) => tab.title.clone(),
            TabType::Script(tab) => tab.title.clone(),
            TabType::Settings(tab) => tab.title.clone(),
            TabType::Snippet(editor) => editor.read(cx).get_title(),
            TabType::TableStructure(tab) => tab.read(cx).title.clone(),
            TabType::ObjectDdl(tab) => tab.read(cx).title.clone(),
            TabType::SchemaGraph(tab) => tab.read(cx).title.clone(),
        }
    }

    /// The connection this tab works against, `None` for connection-less tabs
    /// (settings, snippets).
    pub fn context(&self, cx: &App) -> Option<ConnectionContext> {
        match self {
            TabType::Query(tab) => Some(tab.context.clone()),
            TabType::Script(tab) => Some(tab.context.clone()),
            TabType::TableStructure(tab) => Some(tab.read(cx).context.clone()),
            TabType::ObjectDdl(tab) => Some(tab.read(cx).context.clone()),
            TabType::SchemaGraph(tab) => Some(tab.read(cx).context.clone()),
            TabType::Settings(_) | TabType::Snippet(_) => None,
        }
    }

    /// The shared editor/results/chat state of a query or script tab.
    pub fn connection_tab(&self) -> Option<&ConnectionBackedTab> {
        match self {
            TabType::Query(tab) => Some(&tab.base),
            TabType::Script(tab) => Some(&tab.base),
            _ => None,
        }
    }

    pub fn connection_tab_mut(&mut self) -> Option<&mut ConnectionBackedTab> {
        match self {
            TabType::Query(tab) => Some(&mut tab.base),
            TabType::Script(tab) => Some(&mut tab.base),
            _ => None,
        }
    }

    /// What the buffer of a query or script tab holds.
    pub fn tab_language(&self) -> Option<TabLanguage> {
        match self {
            TabType::Query(_) => Some(TabLanguage::Query),
            TabType::Script(_) => Some(TabLanguage::Script),
            _ => None,
        }
    }

    pub fn query(&self) -> Option<&QueryTab> {
        match self {
            TabType::Query(tab) => Some(tab),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn script(&self) -> Option<&ScriptTab> {
        match self {
            TabType::Script(tab) => Some(tab),
            _ => None,
        }
    }

    /// The app-database row a query or script tab persists to.
    pub fn db_id(&self) -> Option<i64> {
        self.connection_tab().and_then(|tab| tab.db_id)
    }
}

pub struct SettingsTab {
    pub title: String,
    pub settings_view: Entity<SettingsView>,
}

/// Parameters for creating a new tab with connection
#[derive(Clone)]
pub struct TabCreationParams {
    pub title: String,
    pub content: Option<String>,
    pub db_id: Option<i64>,
    pub last_run_at: Option<i64>,
    pub context: ConnectionContext,
}

/// Parameters for creating an object DDL tab (procedures/functions/triggers).
#[derive(Clone)]
pub struct ObjectDdlParams {
    pub kind: RoutineKind,
    pub context: ConnectionContext,
    pub object_name: String,
}

/// Parameters for creating a table structure tab
#[derive(Clone)]
pub struct TableStructureParams {
    pub context: ConnectionContext,
    pub table_name: String,
}
