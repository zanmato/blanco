use std::ops::Range;

use gpui::{
    div, px, prelude::FluentBuilder, App, AppContext, Context, Entity, FocusHandle, Focusable,
    IntoElement, ParentElement, Render, Styled, Window,
};
use gpui_component::{
    h_flex,
    Icon,
    table::{Column, ColumnSort, Table, TableDelegate},
    v_flex, ActiveTheme, IconName,
};

use crate::database::QueryResult;

#[derive(Clone, Debug)]
pub struct ResultRow {
    pub id: usize,
    pub name: String,
    pub email: String,
    pub age: i32,
    pub city: String,
}

pub struct ResultsTableDelegate {
    columns: Vec<Column>,
    rows: Vec<Vec<String>>,
}

impl ResultsTableDelegate {
    pub fn new() -> Self {
        Self {
            columns: vec![],
            rows: vec![],
        }
    }

    pub fn set_query_result(&mut self, result: QueryResult) {
        // Build columns from result
        self.columns = result
            .columns
            .iter()
            .enumerate()
            .map(|(i, name)| {
                Column::new(&format!("col_{}", i), name)
                    .width(150.)
                    .resizable(true)
                    .sortable()
            })
            .collect();

        self.rows = result.rows;
    }
}

impl TableDelegate for ResultsTableDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> &Column {
        &self.columns[col_ix]
    }

    fn render_th(
        &self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<Table<Self>>,
    ) -> impl IntoElement {
        let col = &self.columns[col_ix];
        div()
            .font_family("Fira Code")
            .child(col.name.clone())
    }

    fn render_td(
        &self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<Table<Self>>,
    ) -> impl IntoElement {
        let text = self.rows
            .get(row_ix)
            .and_then(|row| row.get(col_ix))
            .cloned()
            .unwrap_or_else(|| "--".to_string());

        // Check if the value is NULL
        let is_null = text.eq_ignore_ascii_case("null") || text == "--";
        let display_text = if is_null { "NULL".to_string() } else { text };

        div()
            .font_family("Fira Code")
            .when(is_null, |this| {
                this.text_color(cx.theme().muted_foreground)
                    .italic()
            })
            .child(display_text)
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        _: &mut Context<Table<Self>>,
    ) {
        // Sort rows by the specified column
        self.rows.sort_by(|a, b| {
            let a_val = a.get(col_ix).map(|s| s.as_str()).unwrap_or("");
            let b_val = b.get(col_ix).map(|s| s.as_str()).unwrap_or("");

            match sort {
                ColumnSort::Descending => b_val.cmp(a_val),
                _ => a_val.cmp(b_val),
            }
        });
    }

    fn visible_rows_changed(
        &mut self,
        _visible_range: Range<usize>,
        _: &mut Window,
        _: &mut Context<Table<Self>>,
    ) {
    }

    fn visible_columns_changed(
        &mut self,
        _visible_range: Range<usize>,
        _: &mut Window,
        _: &mut Context<Table<Self>>,
    ) {
    }
}

pub struct ResultsPanel {
    focus_handle: FocusHandle,
    table: Entity<Table<ResultsTableDelegate>>,
    current_result: Option<QueryResult>,
}

impl ResultsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let delegate = ResultsTableDelegate::new();
        let table = cx.new(|cx| Table::new(delegate, window, cx));

        Self {
            focus_handle: cx.focus_handle(),
            table,
            current_result: None,
        }
    }

    pub fn set_query_result(&mut self, result: QueryResult, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            table.delegate_mut().set_query_result(result.clone());
            table.refresh(cx);
        });
        self.current_result = Some(result);
        cx.notify();
    }

    pub fn clear_results(&mut self, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            table.delegate_mut().set_query_result(QueryResult::default());
            table.refresh(cx);
        });
        self.current_result = None;
        cx.notify();
    }
}

impl Focusable for ResultsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ResultsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let row_count = self.table.read(cx).delegate().rows_count(cx);

        v_flex()
            .size_full()
            .border_t_1()
            .border_color(cx.theme().border)
            // The table component has built-in scrolling (both vertical and horizontal)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.table.clone())
            )
            .child(
                h_flex()
                    .px_4()
                    .py_2()
                    .gap_3()
                    .bg(cx.theme().muted)
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_sm()
                    .items_center()
                    .when_some(self.current_result.as_ref(), |this, result| {
                        this
                            // Status indicator
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_1()
                                    .when(!result.is_error, |this| {
                                        this.text_color(cx.theme().green)
                                            .child(Icon::new(IconName::CircleCheck).size(px(14.)))
                                            .child("Success")
                                    })
                                    .when(result.is_error, |this| {
                                        this.text_color(cx.theme().red)
                                            .child(Icon::new(IconName::CircleX).size(px(14.)))
                                            .child("Error")
                                    })
                            )
                            // Separator
                            .child(
                                div()
                                    .h(px(16.))
                                    .w(px(1.))
                                    .bg(cx.theme().border)
                            )
                            // Query text (truncated)
                            .when_some(result.query_text.as_ref(), |this, query| {
                                let truncated = if query.len() > 60 {
                                    format!("{}...", &query[..60].trim())
                                } else {
                                    query.clone()
                                };
                                this.child(
                                    div()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(truncated.replace("\n", " "))
                                )
                            })
                            // Spacer
                            .child(div().flex_1())
                            // Execution time
                            .when_some(result.execution_time_ms, |this, time_ms| {
                                this.child(
                                    div()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!("{}ms", time_ms))
                                )
                            })
                            // Row count
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_1()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(Icon::new(IconName::ChevronsUpDown).size(px(14.)))
                                    .child(format!("{} rows", row_count))
                            )
                    })
                    .when(self.current_result.is_none(), |this| {
                        this.text_color(cx.theme().muted_foreground)
                            .child("No query executed")
                    })
            )
    }
}
