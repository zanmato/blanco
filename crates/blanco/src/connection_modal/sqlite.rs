use gpui::{AnyElement, App, Context, Entity, IntoElement, ParentElement, Styled, Window};
use gpui_component::{input::InputState, v_flex};

use app_database::{ConnectionData, EnvironmentType};

use super::NewConnectionModal;
use super::shared::{file_picker_input, make_input};

pub(super) struct SqliteForm {
    pub file_path_input: Entity<InputState>,
}

impl SqliteForm {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<NewConnectionModal>,
        connection_data: Option<&ConnectionData>,
    ) -> Self {
        let file_path_input = make_input(
            window,
            cx,
            "/path/to/database.db",
            false,
            connection_data.and_then(|c| c.database_path.as_deref()),
        );
        Self { file_path_input }
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
        let mut connection = ConnectionData::new_sqlite(name, file_path);
        connection.environment_type = environment_type;
        Some(connection)
    }
}

pub(super) fn render(
    modal: &NewConnectionModal,
    cx: &mut Context<NewConnectionModal>,
) -> AnyElement {
    v_flex()
        .gap_3()
        .child(file_picker_input(
            "sqlite-file-picker",
            "Database path",
            &modal.sqlite_form.file_path_input,
            cx,
        ))
        .into_any_element()
}
