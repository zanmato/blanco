use blanco_core::ObjectKind;
use blanco_ui::{SqlView, SqlViewMessage};
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement,
    Render, SharedString, Styled, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme, v_flex};

use blanco_core::ConnectionContext;

pub struct ObjectDdlTab {
    pub kind: ObjectKind,
    pub context: ConnectionContext,
    pub object_name: String,
    sql_view: Entity<SqlView>,
    focus_handle: FocusHandle,
    loading: bool,
    error: Option<String>,
}

impl ObjectDdlTab {
    /// The tab strip label, in the current language.
    pub fn title(&self) -> String {
        t!(
            "structure.object_ddl.title",
            kind = kind_name(self.kind).to_lowercase(),
            name = self.object_name
        )
        .into_owned()
    }

    pub fn new(
        kind: ObjectKind,
        context: ConnectionContext,
        object_name: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let language = crate::sql::highlight_language(context.db_type);
        let sql_view = cx.new(|cx| SqlView::new(1, language, cx));
        Self {
            kind,
            context,
            object_name,
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

fn kind_name(kind: ObjectKind) -> std::borrow::Cow<'static, str> {
    match kind {
        ObjectKind::Procedure => t!("structure.object_ddl.kind.procedure"),
        ObjectKind::Function => t!("structure.object_ddl.kind.function"),
        ObjectKind::Trigger => t!("structure.object_ddl.kind.trigger"),
        ObjectKind::Enum => t!("structure.object_ddl.kind.enum"),
        ObjectKind::CompositeType => t!("structure.object_ddl.kind.composite_type"),
        ObjectKind::Domain => t!("structure.object_ddl.kind.domain"),
        ObjectKind::Sequence => t!("structure.object_ddl.kind.sequence"),
    }
}

fn ddl_header(kind: ObjectKind) -> std::borrow::Cow<'static, str> {
    match kind {
        ObjectKind::Procedure => t!("structure.object_ddl.header.procedure"),
        ObjectKind::Function => t!("structure.object_ddl.header.function"),
        ObjectKind::Trigger => t!("structure.object_ddl.header.trigger"),
        ObjectKind::Enum => t!("structure.object_ddl.header.enum"),
        ObjectKind::CompositeType => t!("structure.object_ddl.header.composite_type"),
        ObjectKind::Domain => t!("structure.object_ddl.header.domain"),
        ObjectKind::Sequence => t!("structure.object_ddl.header.sequence"),
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
        let header: SharedString = ddl_header(self.kind).into();
        let status: Option<SharedString> = if self.loading {
            Some(t!("structure.loading").into())
        } else {
            self.error
                .as_ref()
                .map(|error| t!("structure.error", error = error).into())
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
