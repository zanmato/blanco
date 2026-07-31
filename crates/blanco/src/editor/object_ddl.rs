use blanco_core::RoutineKind;
use blanco_ui::{SqlView, SqlViewMessage};
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement,
    Render, SharedString, Styled, Window, div, prelude::FluentBuilder as _,
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
    sql_view: Entity<SqlView>,
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
        let highlight_theme = cx.theme().highlight_theme.clone();
        let sql_view = cx.new(|_| SqlView::new(1, highlight_theme, "sql"));
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
            sql_view,
            focus_handle: cx.focus_handle(),
            loading: true,
            error: None,
        }
    }

    pub fn set_ddl(&mut self, ddl: String, cx: &mut Context<Self>) {
        self.loading = false;
        self.error = None;
        self.sql_view.update(cx, |log, cx| {
            log.clear(cx);
            log.append_text(&SqlViewMessage::SqlStatement(ddl), cx);
        });
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
            // No background: the editor card owns this surface. A square fill
            // here would cover the card's rounded corners.
            .size_full()
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
            .child(div().flex_1().min_h_0().child(self.sql_view.clone()))
    }
}
