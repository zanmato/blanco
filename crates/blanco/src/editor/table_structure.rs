use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement, Render,
    Styled, Window, div, px,
};
use gpui_component::{
    ActiveTheme, StyledExt,
    scroll::ScrollableElement,
    table::{Column, DataTable, TableDelegate, TableState},
    v_flex,
};

use crate::app_database::EnvironmentType;
use blanco_core::connection_trait::{ColumnInfo, IndexInfo};

pub struct TableStructureTab {
    pub title: String,
    pub _connection_id: i64,
    pub _db_type: database::DatabaseType,
    pub _connection_name: Option<String>,
    pub _database_name: String,
    pub _schema_name: Option<String>,
    pub _table_name: String,
    pub _environment_type: Option<EnvironmentType>,
    columns_table_state: Entity<TableState<ColumnsTableDelegate>>,
    indexes_table_state: Entity<TableState<IndexesTableDelegate>>,
    focus_handle: FocusHandle,
    loading: bool,
    error: Option<String>,
}

impl TableStructureTab {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        connection_id: i64,
        db_type: database::DatabaseType,
        connection_name: Option<String>,
        database_name: String,
        schema_name: Option<String>,
        table_name: String,
        environment_type: Option<EnvironmentType>,
        columns: Vec<ColumnInfo>,
        indexes: Vec<IndexInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title = table_name.clone();

        let columns_delegate = ColumnsTableDelegate::new(columns);
        let columns_table_state = cx.new(|cx| {
            TableState::new(columns_delegate, window, cx)
                .row_selectable(false)
                .cell_selectable(true)
                .col_selectable(false)
        });

        let indexes_delegate = IndexesTableDelegate::new(indexes);
        let indexes_table_state = cx.new(|cx| {
            TableState::new(indexes_delegate, window, cx)
                .row_selectable(false)
                .cell_selectable(true)
                .col_selectable(false)
        });

        Self {
            title,
            _connection_id: connection_id,
            _db_type: db_type,
            _connection_name: connection_name,
            _database_name: database_name,
            _schema_name: schema_name,
            _table_name: table_name,
            _environment_type: environment_type,
            columns_table_state,
            indexes_table_state,
            focus_handle: cx.focus_handle(),
            loading: false,
            error: None,
        }
    }

    pub fn set_columns(
        &mut self,
        columns: Vec<ColumnInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delegate = ColumnsTableDelegate::new(columns);
        self.columns_table_state = cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .row_selectable(false)
                .cell_selectable(true)
                .col_selectable(false)
        });
        cx.notify();
    }

    pub fn set_indexes(
        &mut self,
        indexes: Vec<IndexInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delegate = IndexesTableDelegate::new(indexes);
        self.indexes_table_state = cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .row_selectable(false)
                .cell_selectable(true)
                .col_selectable(false)
        });
        cx.notify();
    }
}

impl Focusable for TableStructureTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TableStructureTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .flex_1()
            .h_full()
            .w_full()
            .overflow_hidden()
            .bg(theme.background)
            .child(
                v_flex()
                    .flex_1()
                    .overflow_y_scrollbar()
                    .p_4()
                    .gap_4()
                    .child(if let Some(error) = &self.error {
                        div()
                            .text_color(theme.danger_foreground)
                            .child(format!("Error: {}", error))
                            .into_any_element()
                    } else if self.loading {
                        div()
                            .text_color(theme.muted_foreground)
                            .child("Loading...")
                            .into_any_element()
                    } else {
                        v_flex()
                            .flex_1()
                            .gap_4()
                            .child(
                                v_flex()
                                    .flex_grow()
                                    .flex_basis(px(300.))
                                    .min_h(px(150.))
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_semibold()
                                            .text_color(theme.foreground)
                                            .child("Columns"),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .border_1()
                                            .border_color(theme.border)
                                            .overflow_hidden()
                                            .child(
                                                DataTable::new(&self.columns_table_state)
                                                    .bordered(false),
                                            ),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .flex_grow()
                                    .flex_basis(px(150.))
                                    .min_h(px(80.))
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_semibold()
                                            .text_color(theme.foreground)
                                            .child("Indexes"),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .border_1()
                                            .border_color(theme.border)
                                            .overflow_hidden()
                                            .child(
                                                DataTable::new(&self.indexes_table_state)
                                                    .bordered(false),
                                            ),
                                    ),
                            )
                            .into_any_element()
                    }),
            )
    }
}

#[derive(Clone)]
pub struct ColumnsTableDelegate {
    columns: Vec<ColumnInfo>,
    column_defs: Vec<Column>,
}

impl ColumnsTableDelegate {
    pub fn new(columns: Vec<ColumnInfo>) -> Self {
        let column_defs = vec![
            Column {
                name: "#".into(),
                width: px(40.),
                ..Default::default()
            },
            Column {
                name: "Name".into(),
                width: px(150.),
                ..Default::default()
            },
            Column {
                name: "Type".into(),
                width: px(120.),
                ..Default::default()
            },
            Column {
                name: "Nullable".into(),
                width: px(90.),
                ..Default::default()
            },
            Column {
                name: "Default".into(),
                width: px(100.),
                ..Default::default()
            },
            Column {
                name: "Foreign Key".into(),
                width: px(150.),
                ..Default::default()
            },
        ];

        Self {
            columns,
            column_defs,
        }
    }
}

impl TableDelegate for ColumnsTableDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.column_defs.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.column_defs[col_ix].clone()
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let col = &self.column_defs[col_ix];
        div()
            .h_full()
            .flex()
            .items_center()
            .px_2()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(px(12.))
            .child(col.name.clone())
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let col = &self.columns[row_ix];
        let value = match col_ix {
            0 => (row_ix + 1).to_string(),
            1 => col.name.clone(),
            2 => col.data_type.clone(),
            3 => if col.is_nullable { "Yes" } else { "No" }.to_string(),
            4 => col.default_value.clone().unwrap_or_default(),
            5 => col
                .foreign_key
                .as_ref()
                .map(|fk| format!("{}({})", fk.foreign_table_name, fk.foreign_column_name))
                .unwrap_or_default(),
            _ => String::new(),
        };

        div()
            .h_full()
            .flex()
            .items_center()
            .px_2()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(px(12.))
            .text_color(cx.theme().foreground)
            .child(value)
    }
}

#[derive(Clone)]
pub struct IndexesTableDelegate {
    indexes: Vec<IndexInfo>,
    column_defs: Vec<Column>,
}

impl IndexesTableDelegate {
    pub fn new(indexes: Vec<IndexInfo>) -> Self {
        let column_defs = vec![
            Column {
                name: "Name".into(),
                width: px(200.),
                ..Default::default()
            },
            Column {
                name: "Algorithm".into(),
                width: px(90.),
                ..Default::default()
            },
            Column {
                name: "Unique".into(),
                width: px(80.),
                ..Default::default()
            },
            Column {
                name: "Columns".into(),
                width: px(150.),
                ..Default::default()
            },
            Column {
                name: "Condition".into(),
                width: px(150.),
                ..Default::default()
            },
            Column {
                name: "Comment".into(),
                width: px(150.),
                ..Default::default()
            },
        ];

        Self {
            indexes,
            column_defs,
        }
    }
}

impl TableDelegate for IndexesTableDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.column_defs.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.indexes.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.column_defs[col_ix].clone()
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let col = &self.column_defs[col_ix];
        div()
            .h_full()
            .flex()
            .items_center()
            .px_2()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(px(12.))
            .child(col.name.clone())
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let idx = &self.indexes[row_ix];
        let value = match col_ix {
            0 => idx.name.clone(),
            1 => idx.algorithm.clone(),
            2 => if idx.is_unique { "Yes" } else { "No" }.to_string(),
            3 => idx.column_names.join(", "),
            4 => idx.condition.clone().unwrap_or_default(),
            5 => idx.comment.clone().unwrap_or_default(),
            _ => String::new(),
        };

        div()
            .h_full()
            .flex()
            .items_center()
            .px_2()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(px(12.))
            .text_color(cx.theme().foreground)
            .child(value)
    }
}
