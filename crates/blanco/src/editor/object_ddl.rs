use blanco_core::connection_trait::RoutineKind;
use gpui::{
    App, Context, FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme, v_flex};

use crate::app_database::EnvironmentType;

pub struct ObjectDdlTab {
    pub title: String,
    pub kind: RoutineKind,
    pub _connection_id: i64,
    pub _db_type: database::DatabaseType,
    pub _connection_name: Option<String>,
    pub _database_name: String,
    pub _schema_name: Option<String>,
    pub _object_name: String,
    pub _environment_type: Option<EnvironmentType>,
    ddl: SharedString,
    focus_handle: FocusHandle,
    loading: bool,
    error: Option<String>,
}

impl ObjectDdlTab {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kind: RoutineKind,
        connection_id: i64,
        db_type: database::DatabaseType,
        connection_name: Option<String>,
        database_name: String,
        schema_name: Option<String>,
        object_name: String,
        environment_type: Option<EnvironmentType>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title = format!("{}: {}", kind.display_name().to_lowercase(), object_name);
        Self {
            title,
            kind,
            _connection_id: connection_id,
            _db_type: db_type,
            _connection_name: connection_name,
            _database_name: database_name,
            _schema_name: schema_name,
            _object_name: object_name,
            _environment_type: environment_type,
            ddl: SharedString::from(""),
            focus_handle: cx.focus_handle(),
            loading: true,
            error: None,
        }
    }

    pub fn set_ddl(&mut self, ddl: String, cx: &mut Context<Self>) {
        self.loading = false;
        self.error = None;
        self.ddl = SharedString::from(ddl);
        cx.notify();
    }

    pub fn set_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.loading = false;
        self.error = Some(error);
        cx.notify();
    }
}

impl Focusable for ObjectDdlTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ObjectDdlTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let header = format!("{} DDL", self.kind.display_name());
        let status: Option<SharedString> = if self.loading {
            Some("Loading…".into())
        } else {
            self.error.as_ref().map(|e| format!("Error: {e}").into())
        };

        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_color(theme.muted_foreground)
                    .child(header),
            )
            .when_some(status, |this, msg| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_color(theme.muted_foreground)
                        .child(msg),
                )
            })
            .child(
                div()
                    .id("object-ddl-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_3()
                    .py_2()
                    .font_family("monospace")
                    .text_color(theme.foreground)
                    .whitespace_normal()
                    .child(self.ddl.clone()),
            )
    }
}
