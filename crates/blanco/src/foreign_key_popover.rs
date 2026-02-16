use blanco_core::{DatabaseService as DatabaseServiceTrait, QueryResult};
use database::DatabaseService;
use gpui::prelude::*;
use gpui::{
    App, AsyncApp, Context, FocusHandle, Focusable, FontWeight, Render, SharedString, WeakEntity,
    Window, div, px,
};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ActiveTheme, v_flex};

/// Popover component for displaying foreign key lookup results
pub struct ForeignKeyPopover {
    table_name: String,
    column_name: String,
    result: Option<QueryResult>,
    is_loading: bool,
    error: Option<String>,
    focus_handle: FocusHandle,
}

impl ForeignKeyPopover {
    pub fn new(
        table_name: &str,
        column_name: &str,
        reference_value: &str,
        connection_id: i64,
        database_name: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        // Clone DatabaseService before entering async block
        let db_service = DatabaseService::global(cx).clone();

        // Immediately trigger loading of the result
        let table_name_clone = table_name.to_owned();
        let column_name_clone = column_name.to_owned();
        let reference_value_clone = reference_value.to_owned();
        let database_name_clone = database_name.clone();

        cx.spawn(
            async move |weak_this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let result = db_service
                    .foreign_key_lookup(
                        connection_id,
                        database_name_clone.as_ref().map(|s| s.as_ref()),
                        &table_name_clone,
                        &column_name_clone,
                        &reference_value_clone,
                    )
                    .await;

                weak_this
                    .update(cx, |this, cx| {
                        match result {
                            Ok(r) => {
                                this.result = Some(r);
                                this.is_loading = false;
                            }
                            Err(e) => {
                                this.error = Some(e.to_string());
                                this.is_loading = false;
                            }
                        }
                        cx.notify();
                    })
                    .ok();
            },
        )
        .detach();

        Self {
            table_name: table_name.to_owned(),
            column_name: column_name.to_owned(),
            result: None,
            is_loading: true,
            error: None,
            focus_handle,
        }
    }
}

impl Focusable for ForeignKeyPopover {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ForeignKeyPopover {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .flex_grow()
            .gap_2()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_xs()
                    .child(format!("{}: {}", self.table_name, self.column_name)),
            )
            .when(self.is_loading, |this| {
                this.child(
                    v_flex().items_center().justify_center().p_4().child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child("Loading..."),
                    ),
                )
            })
            .when_some(self.error.clone(), |this, err| {
                this.child(
                    v_flex()
                        .p_3()
                        .bg(cx.theme().danger.opacity(0.1))
                        .rounded(cx.theme().radius)
                        .child(div().text_color(cx.theme().danger_foreground).child(err)),
                )
            })
            .when_some(self.result.as_ref(), |this, result| {
                this.child(
                    div()
                        .id("fk-popover-results")
                        .size_full()
                        .overflow_scrollbar()
                        .when(result.rows.is_empty(), |this| {
                            this.child(
                                div()
                                    .p_3()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("No results found"),
                            )
                        })
                        .when(!result.rows.is_empty(), |this| {
                            this.child(self.render_results(result, cx))
                        }),
                )
            })
    }
}

impl ForeignKeyPopover {
    fn render_results(&self, result: &QueryResult, cx: &mut Context<Self>) -> impl IntoElement {
        let col_count = result.columns.len() as u16;

        // Create a flat list of all cells (header + data rows)
        let mut all_cells = Vec::new();

        // Add header cells
        for col in &result.columns {
            all_cells.push(
                div()
                    .px_2()
                    .py_1()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_xs()
                    .bg(cx.theme().table_head)
                    .min_w(px(50.))
                    .max_w(px(100.))
                    .border_b_1()
                    .border_color(cx.theme().table_row_border)
                    .text_color(cx.theme().table_head_foreground)
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(col.clone()),
            );
        }

        // Add data row cells
        for row in &result.rows {
            for value in row {
                let is_null = value == "NULL";
                all_cells.push(
                    div()
                        .px_2()
                        .py_1()
                        .min_w(px(50.))
                        .max_w(px(100.))
                        .text_xs()
                        .border_b_1()
                        .border_color(cx.theme().table_row_border)
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .when(is_null, |this| this.italic().opacity(0.6))
                        .child(value.clone()),
                );
            }
        }

        div()
            .grid()
            .grid_cols_min_content(col_count)
            .flex_grow()
            .bg(cx.theme().table)
            .gap_0()
            .border_1()
            .border_color(cx.theme().table_row_border)
            .rounded(cx.theme().radius)
            .children(all_cells)
    }
}
