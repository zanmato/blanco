//! Tab data structures used by the editor panel.

use std::collections::HashMap;
use std::sync::Arc;

use blanco_core::RoutineKind;
use blanco_ui::SqlView;
use gpui::Entity;
use gpui_component::input::InputState;

use crate::agent::ChatPanel;
use crate::app_database::EnvironmentType;
use crate::results_panel::ResultsPanel;
use crate::settings::SettingsView;
use crate::sql::{SqlCompletionProvider, SqruffService};

use super::object_ddl::ObjectDdlTab;
use super::snippet_editor::SnippetEditor;
use super::table_structure::TableStructureTab;

pub enum TabType {
    Query(Box<QueryTab>),
    Settings(SettingsTab),
    Snippet(Entity<SnippetEditor>),
    TableStructure(Entity<TableStructureTab>),
    ObjectDdl(Entity<ObjectDdlTab>),
    SchemaGraph(Entity<super::schema_graph::SchemaGraphTab>),
}

pub struct QueryTab {
    pub title: String,
    pub connection_id: i64,
    pub _db_type: database::DatabaseType,
    pub connection_name: Option<String>,
    pub database_name: String,
    pub schema_name: Option<String>,
    pub environment_type: Option<EnvironmentType>,
    pub editor: Entity<InputState>,
    pub db_id: Option<i64>,
    pub results_panel: Entity<ResultsPanel>,
    pub sql_view: Entity<SqlView>,
    pub chat_enabled: bool,
    pub chat_panel: Option<Entity<ChatPanel>>,
    pub sql_view_visible: bool,
    pub sqruff_service: Option<Arc<SqruffService>>,
    pub completion_provider: Option<SqlCompletionProvider>,
    /// In-memory cache of the last values entered for query parameters in
    /// this tab, keyed by parameter label (e.g. "$1" or ":user_id"). Used
    /// to prefill the parameter modal on subsequent runs within the same
    /// session.
    pub last_parameter_values: HashMap<String, String>,
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
    pub connection_id: i64,
    pub db_type: database::DatabaseType,
    pub connection_name: Option<String>,
    pub database_name: String,
    pub schema_name: Option<String>,
    pub environment_type: Option<EnvironmentType>,
}

/// Parameters for creating an object DDL tab (procedures/functions/triggers).
#[derive(Clone)]
pub struct ObjectDdlParams {
    pub kind: RoutineKind,
    pub connection_id: i64,
    pub connection_name: String,
    pub db_type: database::DatabaseType,
    pub database_name: String,
    pub schema_name: Option<String>,
    pub object_name: String,
    pub environment_type: Option<EnvironmentType>,
}

/// Parameters for creating a table structure tab
#[derive(Clone)]
pub struct TableStructureParams {
    pub connection_id: i64,
    pub connection_name: String,
    pub db_type: database::DatabaseType,
    pub database_name: String,
    pub schema_name: Option<String>,
    pub table_name: String,
    pub environment_type: Option<EnvironmentType>,
}
