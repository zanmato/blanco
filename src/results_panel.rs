use std::ops::Range;

use gpui::{
    div, App, AppContext, Context, Entity, FocusHandle, Focusable,
    IntoElement, ParentElement, Render, Styled, Window,
};
use gpui_component::{
    table::{Column, ColumnSort, Table, TableDelegate},
    v_flex, ActiveTheme,
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
        div().child(col.name.clone())
    }

    fn render_td(
        &self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<Table<Self>>,
    ) -> impl IntoElement {
        let text = self.rows
            .get(row_ix)
            .and_then(|row| row.get(col_ix))
            .cloned()
            .unwrap_or_else(|| "--".to_string());

        div().child(text)
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
}

impl ResultsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let delegate = ResultsTableDelegate::new();
        let table = cx.new(|cx| Table::new(delegate, window, cx));

        Self {
            focus_handle: cx.focus_handle(),
            table,
        }
    }

    pub fn set_query_result(&mut self, result: QueryResult, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            table.delegate_mut().set_query_result(result);
            table.refresh(cx);
        });
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
        v_flex()
            .size_full()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(self.table.clone())
            .child(
                div()
                    .px_4()
                    .py_2()
                    .bg(cx.theme().muted)
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("Results: {} rows", self.table.read(cx).delegate().rows_count(cx)))
            )
    }
}
