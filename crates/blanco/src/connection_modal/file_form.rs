use blanco_core::DatabaseType;
use gpui::{AnyElement, App, Context, Entity, IntoElement, ParentElement, Styled, Window};
use gpui_component::{input::InputState, v_flex};

use app_database::{ConnectionData, EnvironmentType};

use super::NewConnectionModal;
use super::shared::{file_picker_input, make_input};

/// The form of a backend that opens a database file: SQLite and DuckDB.
pub(super) struct FileForm {
    db_type: DatabaseType,
    pub file_path_input: Entity<InputState>,
}

impl FileForm {
    pub fn new(
        db_type: DatabaseType,
        window: &mut Window,
        cx: &mut Context<NewConnectionModal>,
        connection_data: Option<&ConnectionData>,
    ) -> Self {
        let placeholder = match db_type {
            DatabaseType::DuckDb => "/path/to/database.duckdb, or :memory:",
            _ => "/path/to/database.db",
        };
        let file_path_input = make_input(
            window,
            cx,
            placeholder,
            false,
            connection_data
                .filter(|connection| connection.db_type == db_type)
                .and_then(|connection| connection.database_path.as_deref()),
        );
        Self {
            db_type,
            file_path_input,
        }
    }

    pub fn validate(&self, cx: &App) -> Option<String> {
        if self.file_path_input.read(cx).value().is_empty() {
            Some("Please enter a file path".to_string())
        } else {
            None
        }
    }

    pub fn get_connection_data(
        &self,
        name: String,
        environment_type: EnvironmentType,
        cx: &App,
    ) -> Option<ConnectionData> {
        let file_path = self.file_path_input.read(cx).value().to_string();
        if file_path.is_empty() {
            return None;
        }
        let mut connection = ConnectionData::new_file(self.db_type, name, file_path);
        connection.environment_type = environment_type;
        Some(connection)
    }

    pub fn render(&self, cx: &mut Context<NewConnectionModal>) -> AnyElement {
        let id = match self.db_type {
            DatabaseType::DuckDb => "duckdb-file-picker",
            _ => "sqlite-file-picker",
        };
        v_flex()
            .gap_3()
            .child(file_picker_input(
                id,
                "Database path",
                &self.file_path_input,
                cx,
            ))
            .into_any_element()
    }
}
