//! Tab data structures used by the editor panel.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut, Range};
use std::sync::Arc;

use blanco_core::ObjectKind;
use blanco_ui::SqlView;
use gpui::{App, Entity};
use gpui_component::input::{EditorState, RangeDecorationCollection};

use crate::results_panel::ResultsPanel;
use crate::settings::SettingsView;
use crate::sql::{SqlCompletionProvider, SqruffService};
use blanco_core::ConnectionContext;

use super::object_ddl::ObjectDdlTab;
use super::parameter_form::ParameterValue;
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
    ProcessList(Entity<super::process_list::ProcessListTab>),
    Dashboard(Entity<super::dashboard::DashboardTab>),
}

/// What every tab bound to a connection has in common: the identity it works
/// against, its code editor, results grid and terminal, and the app-database row
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
    /// Whether the terminal pane under the editor is shown.
    pub terminal_enabled: bool,
    /// The pane's shell, spawned the first time the pane is opened and kept
    /// while it is hidden.
    pub terminal: Option<super::terminal_pane::TerminalPane>,
    /// The user asked not to be warned again about writes to PROD from this
    /// tab. Lives with the tab and is never persisted, so a restart or a new
    /// tab warns again.
    pub skip_prod_write_confirmation: bool,
    /// The results area under the editor is hidden (Toggle Results).
    pub results_hidden: bool,
    /// The `.sql` file the tab was opened from or saved to.
    pub file: Option<TabFile>,
}

/// A tab's file on disk and whether the editor differs from it.
pub struct TabFile {
    pub path: std::path::PathBuf,
    /// The text last read from or written to the file; `None` when the file
    /// could not be read back, which counts as unsaved.
    pub saved_text: Option<String>,
    pub dirty: bool,
}

impl TabFile {
    pub fn new(path: std::path::PathBuf, saved_text: Option<String>, current: &str) -> Self {
        let dirty = saved_text.as_deref() != Some(current);
        Self {
            path,
            saved_text,
            dirty,
        }
    }

    /// The file name, used as the tab title.
    pub fn name(&self) -> String {
        file_name(&self.path)
    }

    pub fn path_string(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

pub fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

impl ConnectionBackedTab {
    /// The terminal view to show under the editor, when the pane is open.
    pub fn terminal_pane_view(&self) -> Option<Entity<blanco_terminal::view::TerminalView>> {
        if !self.terminal_enabled {
            return None;
        }
        self.terminal.as_ref().map(|pane| pane.view.clone())
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
    pub last_parameter_values: HashMap<String, ParameterValue>,
    /// Frame the editor draws around the statement the cursor sits in. Absent
    /// for backends whose commands are single lines rather than `;`-delimited
    /// statements (Redis), which have nothing to outline.
    pub statement_outline: Option<RangeDecorationCollection>,
    /// The statement the outline currently frames.
    pub outlined_statement: OutlinedStatement,
}

/// The statement outline's last parse: the editor state it was made from, and
/// the byte range it produced.
///
/// The editor notifies its observers on every repaint, not only on edits and
/// cursor moves, so the outline checks this before parsing the buffer again.
#[derive(Default)]
pub struct OutlinedStatement {
    /// Document version and selection the range was parsed from, or `None`
    /// before the first parse.
    pub parsed_from: Option<(u64, Range<usize>)>,
    /// Byte range of the statement under the cursor, when there is one.
    pub range: Option<Range<usize>>,
}

/// A JavaScript script tab. Mirrors [`QueryTab`] minus the SQL-only machinery
/// (formatter, linter, SQL completion, parameter modal): a script's "query" is
/// an arbitrary program, so none of those apply. It keeps the results panel
/// (fed only by explicit `db.display(...)` calls), the log view that the
/// script's `console` output streams into, and the terminal pane.
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
            TabType::ProcessList(tab) => tab.read(cx).title.clone(),
            TabType::Dashboard(tab) => tab.read(cx).title.clone(),
        }
    }

    /// What closing the tab loses, by name: a file with unsaved changes, or
    /// a table with structure changes not yet applied.
    pub fn unsaved_name(&self, cx: &App) -> Option<String> {
        match self {
            TabType::TableStructure(tab) => {
                let tab = tab.read(cx);
                tab.has_pending_changes(cx).then(|| tab.title.clone())
            }
            _ => self
                .connection_tab()
                .and_then(|tab| tab.file.as_ref())
                .filter(|file| file.dirty)
                .map(TabFile::name),
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
            TabType::ProcessList(tab) => Some(tab.read(cx).context.clone()),
            TabType::Settings(_) | TabType::Snippet(_) | TabType::Dashboard(_) => None,
        }
    }

    /// The table, view or routine the tab shows, for the sidebar to select:
    /// a structure or DDL tab's object, or the table a query tab's current
    /// result was read from.
    pub fn reveal_target(&self, cx: &App) -> Option<crate::connections::RevealTarget> {
        let (context, name) = match self {
            TabType::TableStructure(tab) => {
                let tab = tab.read(cx);
                (tab.context.clone(), tab.table_name.clone())
            }
            TabType::ObjectDdl(tab) => {
                let tab = tab.read(cx);
                (tab.context.clone(), tab.object_name.clone())
            }
            TabType::Query(tab) => {
                let name = tab.base.results_panel.read(cx).source_table(cx)?;
                (
                    tab.base.context.clone(),
                    unquote_identifier(&name).to_string(),
                )
            }
            _ => return None,
        };
        Some(crate::connections::RevealTarget {
            connection_id: context.connection_id,
            database_name: context.database_name,
            schema_name: context.schema_name,
            name,
        })
    }

    /// The shared editor/results/terminal state of a query or script tab.
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

    pub fn query(&self) -> Option<&QueryTab> {
        match self {
            TabType::Query(tab) => Some(tab),
            _ => None,
        }
    }

    #[cfg(any(test, feature = "screenshots"))]
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
    pub kind: ObjectKind,
    pub context: ConnectionContext,
    pub object_name: String,
}

/// Parameters for creating a table structure tab
#[derive(Clone)]
pub struct TableStructureParams {
    pub context: ConnectionContext,
    pub table_name: String,
}

/// `name` without the quotes a parser keeps on a quoted identifier.
fn unquote_identifier(name: &str) -> &str {
    for (open, close) in [('"', '"'), ('`', '`'), ('[', ']')] {
        if let Some(inner) = name
            .strip_prefix(open)
            .and_then(|rest| rest.strip_suffix(close))
        {
            return inner;
        }
    }
    name
}
