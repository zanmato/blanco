use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement, Render,
    Styled, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme, Sizable as _, StyledExt,
    button::Button,
    h_flex,
    scroll::ScrollableElement,
    table::{Column, DataTable, TableDelegate, TableState},
    v_flex,
};

use crate::app_database::EnvironmentType;
use crate::result_ext::ResultExt as _;
use blanco_core::{ColumnInfo, IndexInfo};
use blanco_ui::{SqlView, SqlViewMessage};

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
    ddl_log: Entity<SqlView>,
    ddl_visible: bool,
    ddl_loaded: bool,
    ddl_loading: bool,
    ddl_error: Option<String>,
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

        let highlight_theme = cx.theme().highlight_theme.clone();
        let ddl_log = cx.new(|_| SqlView::new(1, highlight_theme, "sql"));

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
            ddl_log,
            ddl_visible: false,
            ddl_loaded: false,
            ddl_loading: false,
            ddl_error: None,
        }
    }

    /// Whether this driver can produce a `CREATE TABLE` statement. MSSQL has no
    /// implementation, so the button is hidden rather than surfacing an error.
    fn supports_table_ddl(&self) -> bool {
        !matches!(self._db_type, database::DatabaseType::MsSql)
    }

    fn toggle_ddl(&mut self, cx: &mut Context<Self>) {
        if self.ddl_visible {
            self.ddl_visible = false;
            cx.notify();
            return;
        }

        self.ddl_visible = true;
        if self.ddl_loaded {
            cx.notify();
            return;
        }

        self.ddl_loading = true;
        self.ddl_error = None;
        cx.notify();

        let db_service = database::DatabaseService::global(cx).clone();
        let connection_id = self._connection_id;
        let database_name = self._database_name.clone();
        let schema_name = self._schema_name.clone();
        let table_name = self._table_name.clone();

        cx.spawn(async move |this, cx| {
            use database::DatabaseServiceTrait as _;
            let result = match db_service
                .get_or_create_connection_by_id(connection_id, Some(&database_name))
                .await
            {
                Ok(connection) => {
                    connection
                        .table_ddl(schema_name.as_deref(), &table_name)
                        .await
                }
                Err(e) => Err(e),
            };

            this.update(cx, |this, cx| match result {
                Ok(ddl) => this.set_ddl(ddl, cx),
                Err(e) => {
                    this.ddl_loading = false;
                    this.ddl_error = Some(format!("{e:#}"));
                    cx.notify();
                }
            })
            .log_err();
        })
        .detach();
    }

    fn set_ddl(&mut self, ddl: String, cx: &mut Context<Self>) {
        self.ddl_loading = false;
        self.ddl_loaded = true;
        self.ddl_error = None;
        self.ddl_log.update(cx, |log, cx| {
            log.clear(cx);
            log.append_text(&SqlViewMessage::SqlStatement(ddl), cx);
        });
        cx.notify();
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
                            .when(self.supports_table_ddl(), |this| {
                                let label = if self.ddl_visible {
                                    "Hide Create Statement"
                                } else {
                                    "Show Create Statement"
                                };
                                this.child(
                                    h_flex().child(
                                        Button::new("toggle-table-ddl")
                                            .outline()
                                            .small()
                                            .label(label)
                                            .on_click(cx.listener(|this, _, _window, cx| {
                                                this.toggle_ddl(cx);
                                            })),
                                    ),
                                )
                            })
                            .child(
                                v_flex()
                                    .flex_grow(1.)
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
                                    .flex_grow(1.)
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
                            .when(self.ddl_visible, |this| {
                                let editor_bg = theme
                                    .highlight_theme
                                    .style
                                    .editor_background
                                    .unwrap_or(theme.background);
                                this.child(
                                    v_flex()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_lg()
                                                .font_semibold()
                                                .text_color(theme.foreground)
                                                .child("Create Statement"),
                                        )
                                        .child(
                                            div()
                                                .h(px(280.))
                                                .bg(editor_bg)
                                                .border_1()
                                                .border_color(theme.border)
                                                .rounded(theme.radius)
                                                .overflow_hidden()
                                                .child(if let Some(error) = &self.ddl_error {
                                                    div()
                                                        .p_4()
                                                        .text_color(theme.danger_foreground)
                                                        .child(format!("Error: {error}"))
                                                        .into_any_element()
                                                } else if self.ddl_loading {
                                                    div()
                                                        .p_4()
                                                        .text_color(theme.muted_foreground)
                                                        .child("Loading…")
                                                        .into_any_element()
                                                } else {
                                                    self.ddl_log.clone().into_any_element()
                                                }),
                                        ),
                                )
                            })
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
