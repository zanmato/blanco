use std::ops::Range;

use gpui::{
    div, App, AppContext, Context, Entity, FocusHandle, Focusable,
    IntoElement, ParentElement, Render, Styled, Window,
};
use gpui_component::{
    table::{Column, ColumnSort, Table, TableDelegate},
    v_flex, ActiveTheme,
};

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
    rows: Vec<ResultRow>,
}

impl ResultsTableDelegate {
    pub fn new() -> Self {
        Self {
            columns: vec![
                Column::new("id", "ID")
                    .width(60.)
                    .resizable(true)
                    .sortable(),
                Column::new("name", "Name")
                    .width(150.)
                    .resizable(true)
                    .sortable(),
                Column::new("email", "Email")
                    .width(200.)
                    .resizable(true)
                    .sortable(),
                Column::new("age", "Age")
                    .width(80.)
                    .resizable(true)
                    .sortable(),
                Column::new("city", "City")
                    .width(150.)
                    .resizable(true)
                    .sortable(),
            ],
            rows: vec![
                ResultRow {
                    id: 1,
                    name: "Alice Johnson".into(),
                    email: "alice@example.com".into(),
                    age: 28,
                    city: "New York".into(),
                },
                ResultRow {
                    id: 2,
                    name: "Bob Smith".into(),
                    email: "bob@example.com".into(),
                    age: 34,
                    city: "Los Angeles".into(),
                },
                ResultRow {
                    id: 3,
                    name: "Charlie Brown".into(),
                    email: "charlie@example.com".into(),
                    age: 25,
                    city: "Chicago".into(),
                },
                ResultRow {
                    id: 4,
                    name: "Diana Prince".into(),
                    email: "diana@example.com".into(),
                    age: 31,
                    city: "San Francisco".into(),
                },
                ResultRow {
                    id: 5,
                    name: "Eve Wilson".into(),
                    email: "eve@example.com".into(),
                    age: 29,
                    city: "Seattle".into(),
                },
            ],
        }
    }

    pub fn set_rows(&mut self, rows: Vec<ResultRow>) {
        self.rows = rows;
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
        let row = &self.rows[row_ix];
        let col = &self.columns[col_ix];

        let text = match col.key.as_ref() {
            "id" => row.id.to_string(),
            "name" => row.name.clone(),
            "email" => row.email.clone(),
            "age" => row.age.to_string(),
            "city" => row.city.clone(),
            _ => "--".to_string(),
        };

        div().child(text)
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        _: &mut Context<Table<Self>>,
    ) {
        let col = &self.columns[col_ix];
        match col.key.as_ref() {
            "id" => self.rows.sort_by(|a, b| match sort {
                ColumnSort::Descending => b.id.cmp(&a.id),
                _ => a.id.cmp(&b.id),
            }),
            "name" => self.rows.sort_by(|a, b| match sort {
                ColumnSort::Descending => b.name.cmp(&a.name),
                _ => a.name.cmp(&b.name),
            }),
            "age" => self.rows.sort_by(|a, b| match sort {
                ColumnSort::Descending => b.age.cmp(&a.age),
                _ => a.age.cmp(&b.age),
            }),
            _ => {}
        }
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
