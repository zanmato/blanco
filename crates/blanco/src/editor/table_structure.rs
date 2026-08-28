use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement, Render,
    Styled, WeakEntity, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme, Sizable as _, StyledExt, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex,
    input::{Input, InputState},
    menu::{PopupMenu, PopupMenuItem},
    notification::NotificationType,
    scroll::ScrollableElement,
    table::{Column, DataTable, TableDelegate, TableState},
    v_flex,
};

use crate::result_ext::ResultExt as _;
use crate::status_bar::ActivityReporter;
use blanco_core::ConnectionContext;
use blanco_core::ddl::{ColumnOperation, ColumnSpec, IndexOperation};
use blanco_core::{ColumnInfo, IndexInfo};
use blanco_ui::{SqlView, SqlViewMessage};

pub struct TableStructureTab {
    pub title: String,
    pub context: ConnectionContext,
    pub table_name: String,
    columns: Vec<ColumnInfo>,
    indexes: Vec<IndexInfo>,
    columns_table_state: Entity<TableState<ColumnsTableDelegate>>,
    indexes_table_state: Entity<TableState<IndexesTableDelegate>>,
    focus_handle: FocusHandle,
    loading: bool,
    error: Option<String>,
    /// The connection is flagged read-only, so DDL is hidden rather than
    /// offered and refused by the connection wrapper.
    read_only: bool,
    ddl_log: Entity<SqlView>,
    ddl_visible: bool,
    ddl_loaded: bool,
    ddl_loading: bool,
    ddl_error: Option<String>,
}

impl TableStructureTab {
    pub fn new(
        context: ConnectionContext,
        table_name: String,
        columns: Vec<ColumnInfo>,
        indexes: Vec<IndexInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title = table_name.clone();

        let highlight_theme = cx.theme().highlight_theme.clone();
        let ddl_log = cx.new(|_| SqlView::new(1, highlight_theme, "sql"));

        let tab = cx.entity().downgrade();
        let columns_table_state =
            Self::columns_table(columns.clone(), false, tab.clone(), window, cx);
        let indexes_table_state = Self::indexes_table(indexes.clone(), false, tab, window, cx);

        Self {
            title,
            context,
            table_name,
            columns,
            indexes,
            columns_table_state,
            indexes_table_state,
            focus_handle: cx.focus_handle(),
            loading: false,
            error: None,
            read_only: false,
            ddl_log,
            ddl_visible: false,
            ddl_loaded: false,
            ddl_loading: false,
            ddl_error: None,
        }
    }

    fn columns_table(
        columns: Vec<ColumnInfo>,
        editable: bool,
        tab: WeakEntity<Self>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TableState<ColumnsTableDelegate>> {
        let delegate = ColumnsTableDelegate::new(columns, editable, tab);
        cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .row_selectable(false)
                .cell_selectable(true)
                .col_selectable(false)
        })
    }

    fn indexes_table(
        indexes: Vec<IndexInfo>,
        editable: bool,
        tab: WeakEntity<Self>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TableState<IndexesTableDelegate>> {
        let delegate = IndexesTableDelegate::new(indexes, editable, tab);
        cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .row_selectable(false)
                .cell_selectable(true)
                .col_selectable(false)
        })
    }

    /// Whether this driver can produce a `CREATE TABLE` statement. MSSQL has no
    /// implementation, so the button is hidden rather than surfacing an error.
    fn supports_table_ddl(&self) -> bool {
        self.context.dialect().supports_create_table_ddl()
    }

    /// Whether column/index DDL is offered: the backend must have it and the
    /// connection must be writable.
    fn can_edit_structure(&self) -> bool {
        self.context.dialect().supports_column_ddl() && !self.read_only
    }

    /// Fetch columns, indexes and the read-only flag from the server and
    /// rebuild both grids. Also invalidates the CREATE statement panel so it
    /// refetches after a DDL change.
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        cx.notify();

        let db_service = database::DatabaseService::global(cx).clone();
        let connection_id = self.context.connection_id;
        let database_name = self.context.database_name.clone();
        let schema_name = self.context.schema_name.clone();
        let table_name = self.table_name.clone();
        let activity = ActivityReporter::global(cx).begin(format!(
            "{}: loading structure",
            self.context.connection_name
        ));

        cx.spawn_in(window, async move |this, cx| {
            let _activity = activity;
            let result = gpui_tokio::Tokio::spawn_result(cx, async move {
                use database::DatabaseServiceTrait as _;
                let read_only = db_service
                    .get_connection_config(connection_id)
                    .await
                    .is_some_and(|config| config.read_only);
                let connection = db_service
                    .get_or_create_connection_by_id(connection_id, Some(&database_name))
                    .await?;
                let columns = connection
                    .get_columns_for_table(&table_name, schema_name.as_deref())
                    .await?;
                let indexes = connection
                    .get_indexes_for_table(&table_name, schema_name.as_deref())
                    .await?;
                Ok((columns, indexes, read_only))
            })
            .await;

            this.update_in(cx, |this, window, cx| {
                this.loading = false;
                match result {
                    Ok((columns, indexes, read_only)) => {
                        this.read_only = read_only;
                        this.set_columns(columns, window, cx);
                        this.set_indexes(indexes, window, cx);
                        this.ddl_loaded = false;
                        if this.ddl_visible {
                            this.ddl_visible = false;
                            this.toggle_ddl(cx);
                        }
                    }
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
                cx.notify();
            })
            .log_err();
        })
        .detach();
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
        let connection_id = self.context.connection_id;
        let database_name = self.context.database_name.clone();
        let schema_name = self.context.schema_name.clone();
        let table_name = self.table_name.clone();

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
        let tab = cx.entity().downgrade();
        self.columns = columns.clone();
        self.columns_table_state =
            Self::columns_table(columns, self.can_edit_structure(), tab, window, cx);
        cx.notify();
    }

    pub fn set_indexes(
        &mut self,
        indexes: Vec<IndexInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = cx.entity().downgrade();
        self.indexes = indexes.clone();
        self.indexes_table_state =
            Self::indexes_table(indexes, self.can_edit_structure(), tab, window, cx);
        cx.notify();
    }

    fn prod_note(&self) -> &'static str {
        if self.context.is_prod() {
            " on a PROD connection"
        } else {
            ""
        }
    }

    fn confirm_label(&self, label: &str) -> String {
        if self.context.is_prod() {
            format!("{label} on PROD")
        } else {
            label.to_string()
        }
    }

    /// Open the add (`existing == None`) or edit column form.
    pub fn open_column_editor(
        &mut self,
        existing: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing_column = existing.and_then(|row| self.columns.get(row)).cloned();
        let form = cx.new(|cx| ColumnForm::new(existing_column.as_ref(), window, cx));
        let title = match &existing_column {
            Some(column) => format!("Edit column \"{}\"", column.name),
            None => format!("Add column to \"{}\"", self.table_name),
        };
        let description = format!(
            "The change is applied immediately{}. Type and default are used verbatim.",
            self.prod_note()
        );
        let confirm_label = self.confirm_label(if existing_column.is_some() {
            "Apply"
        } else {
            "Add"
        });
        let is_prod = self.context.is_prod();
        let this_handle = cx.entity().downgrade();

        window.open_dialog(cx, move |dialog, _, _| {
            let form = form.clone();
            let existing_column = existing_column.clone();
            let this_handle = this_handle.clone();
            dialog
                .title(title.clone())
                .w(px(460.))
                .child(div().text_sm().child(description.clone()))
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("column-cancel").label("Cancel").outline()),
                        )
                        .child(
                            DialogAction::new().child(
                                Button::new("column-confirm")
                                    .when(is_prod, |this| this.danger())
                                    .when(!is_prod, |this| this.primary())
                                    .label(confirm_label.clone())
                                    .on_click(move |_, window, cx| {
                                        let spec = form.read(cx).spec(cx);
                                        let operation = match &existing_column {
                                            Some(column) => ColumnOperation::Alter {
                                                name: column.name.clone(),
                                                spec,
                                            },
                                            None => ColumnOperation::Add(spec),
                                        };
                                        this_handle
                                            .update(cx, |this, cx| {
                                                this.run_column_operation(operation, window, cx);
                                            })
                                            .log_err();
                                    }),
                            ),
                        ),
                )
        });
    }

    pub fn confirm_rename_column(
        &mut self,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(column) = self.columns.get(row).cloned() else {
            return;
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("New name")
                .default_value(&column.name)
        });
        let title = format!("Rename column \"{}\"?", column.name);
        let description = format!("Enter the new name{}.", self.prod_note());
        let confirm_label = self.confirm_label("Rename");
        let this_handle = cx.entity().downgrade();

        window.open_dialog(cx, move |dialog, _, _| {
            let input = input.clone();
            let column = column.clone();
            let this_handle = this_handle.clone();
            dialog
                .title(title.clone())
                .child(description.clone())
                .child(Input::new(&input))
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().child(
                                Button::new("rename-column-cancel")
                                    .label("Cancel")
                                    .outline(),
                            ),
                        )
                        .child(
                            DialogAction::new().child(
                                Button::new("rename-column-confirm")
                                    .danger()
                                    .label(confirm_label.clone())
                                    .on_click(move |_, window, cx| {
                                        let operation = ColumnOperation::Rename {
                                            from: column.name.clone(),
                                            to: input.read(cx).value().to_string(),
                                        };
                                        this_handle
                                            .update(cx, |this, cx| {
                                                this.run_column_operation(operation, window, cx);
                                            })
                                            .log_err();
                                    }),
                            ),
                        ),
                )
        });
    }

    pub fn confirm_drop_column(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(column) = self.columns.get(row).cloned() else {
            return;
        };
        let description = format!(
            "\"{}\" and all of its data will be permanently deleted{}.",
            column.name,
            self.prod_note()
        );
        self.confirm_danger(
            format!("Drop column \"{}\"?", column.name),
            description,
            self.confirm_label("Drop"),
            DdlRequest::Column(ColumnOperation::Drop { name: column.name }),
            window,
            cx,
        );
    }

    pub fn confirm_drop_index(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.indexes.get(row).cloned() else {
            return;
        };
        let description = format!(
            "Index \"{}\" will be permanently deleted{}.",
            index.name,
            self.prod_note()
        );
        self.confirm_danger(
            format!("Drop index \"{}\"?", index.name),
            description,
            self.confirm_label("Drop"),
            DdlRequest::Index(IndexOperation::Drop { name: index.name }),
            window,
            cx,
        );
    }

    fn confirm_danger(
        &mut self,
        title: String,
        description: String,
        confirm_label: String,
        request: DdlRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this_handle = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let request = request.clone();
            let this_handle = this_handle.clone();
            dialog
                .title(title.clone())
                .child(description.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("ddl-cancel").label("Cancel").outline()),
                        )
                        .child(
                            DialogAction::new().child(
                                Button::new("ddl-confirm")
                                    .danger()
                                    .label(confirm_label.clone())
                                    .on_click(move |_, window, cx| {
                                        let request = request.clone();
                                        this_handle
                                            .update(cx, |this, cx| match request {
                                                DdlRequest::Column(operation) => {
                                                    this.run_column_operation(operation, window, cx)
                                                }
                                                DdlRequest::Index(operation) => {
                                                    this.run_index_operation(operation, window, cx)
                                                }
                                            })
                                            .log_err();
                                    }),
                            ),
                        ),
                )
        });
    }

    pub fn open_index_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let column_names: Vec<String> = self.columns.iter().map(|c| c.name.clone()).collect();
        let form = cx.new(|cx| IndexForm::new(&self.table_name, column_names, window, cx));
        let title = format!("Add index to \"{}\"", self.table_name);
        let description = format!(
            "The index is created immediately{}. Columns are indexed in table order.",
            self.prod_note()
        );
        let confirm_label = self.confirm_label("Create");
        let is_prod = self.context.is_prod();
        let this_handle = cx.entity().downgrade();

        window.open_dialog(cx, move |dialog, _, _| {
            let form = form.clone();
            let this_handle = this_handle.clone();
            dialog
                .title(title.clone())
                .w(px(460.))
                .child(div().text_sm().child(description.clone()))
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("index-cancel").label("Cancel").outline()),
                        )
                        .child(
                            DialogAction::new().child(
                                Button::new("index-confirm")
                                    .when(is_prod, |this| this.danger())
                                    .when(!is_prod, |this| this.primary())
                                    .label(confirm_label.clone())
                                    .on_click(move |_, window, cx| {
                                        let operation = form.read(cx).operation(cx);
                                        this_handle
                                            .update(cx, |this, cx| {
                                                this.run_index_operation(operation, window, cx);
                                            })
                                            .log_err();
                                    }),
                            ),
                        ),
                )
        });
    }

    fn run_column_operation(
        &mut self,
        operation: ColumnOperation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let statements = blanco_core::ddl::column_operation_sql(
            self.context.db_type,
            self.context.schema_name.as_deref(),
            &self.table_name,
            &operation,
        );
        self.run_ddl(operation.label(), statements, window, cx);
    }

    fn run_index_operation(
        &mut self,
        operation: IndexOperation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let statements = blanco_core::ddl::index_operation_sql(
            self.context.db_type,
            self.context.schema_name.as_deref(),
            &self.table_name,
            &operation,
        );
        self.run_ddl(operation.label(), statements, window, cx);
    }

    /// Run the statements as one transaction, then reload the tab and the
    /// sidebar's view of the connection.
    fn run_ddl(
        &mut self,
        label: &'static str,
        statements: Result<Vec<String>, blanco_core::ddl::DdlError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let statements = match statements {
            Ok(statements) => statements,
            Err(error) => {
                window
                    .push_notification((NotificationType::Error, format!("{label}: {error}")), cx);
                return;
            }
        };
        let operations: Vec<blanco_core::WriteOperation> =
            statements.iter().map(|sql| sql.as_str().into()).collect();
        let db_service = database::DatabaseService::global(cx).clone();
        let connection_id = self.context.connection_id;
        let database_name = self.context.database_name.clone();
        let table_name = self.table_name.clone();
        let activity = ActivityReporter::global(cx)
            .begin(format!("{}: {label}", self.context.connection_name));

        cx.spawn_in(window, async move |this, cx| {
            let _activity = activity;
            let result = gpui_tokio::Tokio::spawn_result(cx, async move {
                let connection = db_service
                    .get_or_create_connection(connection_id, Some(&database_name))
                    .await?;
                connection
                    .execute_operations_transactional(&operations, Some(&database_name))
                    .await
                    .map_err(|failure| failure.error)?;
                Ok(())
            })
            .await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    window.push_notification(
                        (
                            NotificationType::Success,
                            format!("{label} on {table_name} succeeded"),
                        ),
                        cx,
                    );
                    this.reload(window, cx);
                    window.dispatch_action(
                        Box::new(crate::app::RefreshConnectionTree { connection_id }),
                        cx,
                    );
                }
                Err(error) => {
                    tracing::error!("{label} on {table_name} failed: {error:#}");
                    window.push_notification(
                        (
                            NotificationType::Error,
                            format!("{label} on {table_name} failed: {error:#}"),
                        ),
                        cx,
                    );
                }
            })
            .log_err();
        })
        .detach();
    }
}

#[derive(Clone)]
enum DdlRequest {
    Column(ColumnOperation),
    Index(IndexOperation),
}

/// The add/edit column dialog body.
struct ColumnForm {
    name: Entity<InputState>,
    data_type: Entity<InputState>,
    default: Entity<InputState>,
    nullable: bool,
}

impl ColumnForm {
    fn new(existing: Option<&ColumnInfo>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| {
            let state = InputState::new(window, cx).placeholder("Column name");
            match existing {
                Some(column) => state.default_value(&column.name),
                None => state,
            }
        });
        let data_type = cx.new(|cx| {
            let state =
                InputState::new(window, cx).placeholder("Type, e.g. integer or varchar(120)");
            match existing {
                Some(column) => state.default_value(&column.data_type),
                None => state,
            }
        });
        let default = cx.new(|cx| {
            let state = InputState::new(window, cx).placeholder("Default expression (optional)");
            match existing.and_then(|column| column.default_value.as_deref()) {
                Some(value) => state.default_value(value),
                None => state,
            }
        });
        Self {
            name,
            data_type,
            default,
            nullable: existing.map(|column| column.is_nullable).unwrap_or(true),
        }
    }

    fn spec(&self, cx: &App) -> ColumnSpec {
        let default = self.default.read(cx).value().trim().to_string();
        ColumnSpec {
            name: self.name.read(cx).value().to_string(),
            data_type: self.data_type.read(cx).value().to_string(),
            nullable: self.nullable,
            default: (!default.is_empty()).then_some(default),
        }
    }
}

impl Render for ColumnForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .py_2()
            .child(labeled("Name", Input::new(&self.name)))
            .child(labeled("Type", Input::new(&self.data_type)))
            .child(labeled("Default", Input::new(&self.default)))
            .child(
                Checkbox::new("column-nullable")
                    .label("Nullable")
                    .checked(self.nullable)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.nullable = *checked;
                        cx.notify();
                    })),
            )
    }
}

/// The add index dialog body.
struct IndexForm {
    name: Entity<InputState>,
    unique: bool,
    columns: Vec<(String, bool)>,
}

impl IndexForm {
    fn new(
        table_name: &str,
        columns: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Index name")
                .default_value(&format!("{table_name}_idx"))
        });
        Self {
            name,
            unique: false,
            columns: columns.into_iter().map(|c| (c, false)).collect(),
        }
    }

    fn operation(&self, cx: &App) -> IndexOperation {
        IndexOperation::Create {
            name: self.name.read(cx).value().to_string(),
            columns: self
                .columns
                .iter()
                .filter(|(_, selected)| *selected)
                .map(|(name, _)| name.clone())
                .collect(),
            unique: self.unique,
        }
    }
}

impl Render for IndexForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .py_2()
            .child(labeled("Name", Input::new(&self.name)))
            .child(
                Checkbox::new("index-unique")
                    .label("Unique")
                    .checked(self.unique)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.unique = *checked;
                        cx.notify();
                    })),
            )
            .child(div().text_sm().child("Columns"))
            .child(
                v_flex()
                    .gap_1()
                    .max_h(px(220.))
                    .overflow_y_scrollbar()
                    .children(
                        self.columns
                            .iter()
                            .enumerate()
                            .map(|(ix, (name, selected))| {
                                Checkbox::new(("index-column", ix))
                                    .label(name.clone())
                                    .checked(*selected)
                                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                        if let Some(column) = this.columns.get_mut(ix) {
                                            column.1 = *checked;
                                        }
                                        cx.notify();
                                    }))
                            }),
                    ),
            )
    }
}

fn labeled(label: &'static str, input: Input) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_sm().child(label))
        .child(input)
}

impl Focusable for TableStructureTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TableStructureTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let can_edit = self.can_edit_structure();

        v_flex()
            .flex_1()
            .h_full()
            .w_full()
            .overflow_hidden()
            // No background: the editor card owns this surface. A square fill
            // here would cover the card's rounded corners.
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
                                h_flex()
                                    .gap_2()
                                    .when(self.supports_table_ddl(), |this| {
                                        let label = if self.ddl_visible {
                                            "Hide Create Statement"
                                        } else {
                                            "Show Create Statement"
                                        };
                                        this.child(
                                            Button::new("toggle-table-ddl")
                                                .outline()
                                                .small()
                                                .label(label)
                                                .on_click(cx.listener(|this, _, _window, cx| {
                                                    this.toggle_ddl(cx);
                                                })),
                                        )
                                    })
                                    .when(can_edit, |this| {
                                        this.child(
                                            Button::new("add-column")
                                                .outline()
                                                .small()
                                                .label("Add Column")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.open_column_editor(None, window, cx);
                                                })),
                                        )
                                        .child(
                                            Button::new("add-index")
                                                .outline()
                                                .small()
                                                .label("Add Index")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.open_index_editor(window, cx);
                                                })),
                                        )
                                    })
                                    .when(self.read_only, |this| {
                                        this.child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child("Read-only connection"),
                                        )
                                    }),
                            )
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
    editable: bool,
    tab: WeakEntity<TableStructureTab>,
}

impl ColumnsTableDelegate {
    pub fn new(
        columns: Vec<ColumnInfo>,
        editable: bool,
        tab: WeakEntity<TableStructureTab>,
    ) -> Self {
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
            editable,
            tab,
        }
    }
}

impl TableDelegate for ColumnsTableDelegate {
    fn context_menu(
        &mut self,
        (row_ix, _): (usize, usize),
        menu: PopupMenu,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        if !self.editable || row_ix >= self.columns.len() {
            return menu;
        }
        let tab = self.tab.clone();
        let item = |label: &'static str,
                    action: fn(
            &mut TableStructureTab,
            usize,
            &mut Window,
            &mut Context<TableStructureTab>,
        )| {
            let tab = tab.clone();
            PopupMenuItem::new(label).on_click(move |_, window, cx| {
                tab.update(cx, |tab, cx| action(tab, row_ix, window, cx))
                    .log_err();
            })
        };
        menu.item(item("Edit Column", |tab, row, window, cx| {
            tab.open_column_editor(Some(row), window, cx)
        }))
        .item(item(
            "Rename Column",
            TableStructureTab::confirm_rename_column,
        ))
        .separator()
        .item(item("Drop Column", TableStructureTab::confirm_drop_column))
    }

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
    editable: bool,
    tab: WeakEntity<TableStructureTab>,
}

impl IndexesTableDelegate {
    pub fn new(
        indexes: Vec<IndexInfo>,
        editable: bool,
        tab: WeakEntity<TableStructureTab>,
    ) -> Self {
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
            editable,
            tab,
        }
    }
}

impl TableDelegate for IndexesTableDelegate {
    fn context_menu(
        &mut self,
        (row_ix, _): (usize, usize),
        menu: PopupMenu,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        if !self.editable || row_ix >= self.indexes.len() {
            return menu;
        }
        let tab = self.tab.clone();
        menu.item(
            PopupMenuItem::new("Drop Index").on_click(move |_, window, cx| {
                tab.update(cx, |tab, cx| tab.confirm_drop_index(row_ix, window, cx))
                    .log_err();
            }),
        )
    }

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
