//! The structure tab: a table's columns, indexes, foreign keys, table
//! information and create statement, one section at a time. Selecting a row
//! opens it in the side panel, where a change is staged. Staged changes wait
//! in the bar at the bottom until Apply runs them together.

use app_database::{AppDatabase, VirtualForeignKey};
use blanco_core::ddl::{
    ColumnOperation, ColumnSpec, DdlError, ForeignKeyOperation, IndexOperation, NewTable,
    TableChange, create_table_sql,
};
use blanco_core::{
    ColumnInfo, ConnectionContext, ForeignKeyConstraint, IndexInfo, ObjectKind, TableInfo,
};
use blanco_ui::{IconName, SqlView, SqlViewMessage};
use gpui::{
    AnyElement, App, AppContext, Context, Entity, FocusHandle, Focusable, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement as _, Styled, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    notification::NotificationType,
    scroll::ScrollableElement,
    table::{DataTable, TableEvent, TableState},
    v_flex,
};

use self::forms::{ColumnForm, ForeignKeyForm, IndexForm};
use self::lists::{
    ColumnsTableDelegate, ForeignKeysTableDelegate, IndexesTableDelegate, order_indexes,
};
use self::staging::{
    ChangeKind, LoadedStructure, PendingChange, RowMark, RowState, StagedStructure,
    StructureChange, column_spec,
};
use crate::result_ext::ResultExt as _;
use crate::status_bar::ActivityReporter;

mod forms;
mod lists;
mod staging;

/// The part of the structure the tab shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    Columns,
    Indexes,
    ForeignKeys,
    Table,
    CreateStatement,
}

/// What the side panel shows. Rows are indices into the staged structure.
enum Panel {
    NewColumn(Entity<ColumnForm>),
    /// A listed column, with its form unless it cannot be changed (dropped,
    /// or the connection is read only).
    Column {
        row: usize,
        form: Option<Entity<ColumnForm>>,
    },
    NewIndex(Entity<IndexForm>),
    Index(usize),
    NewForeignKey(Entity<ForeignKeyForm>),
    ForeignKey(usize),
    /// A virtual key, new (`id` 0) or saved.
    VirtualKey {
        id: i64,
        form: Entity<ForeignKeyForm>,
    },
}

pub struct TableStructureTab {
    pub title: String,
    pub context: ConnectionContext,
    pub table_name: String,
    /// The structure as the server last reported it.
    columns: Vec<ColumnInfo>,
    indexes: Vec<IndexInfo>,
    foreign_keys: Vec<ForeignKeyConstraint>,
    table_info: TableInfo,
    server_version_num: Option<u32>,
    /// The database's own type names, suggested in the column type field.
    user_types: Vec<String>,
    /// Foreign keys declared in Blanco for this table.
    virtual_keys: Vec<VirtualForeignKey>,
    /// Changes waiting for Apply, in the order they run.
    pending: Vec<PendingChange>,
    /// The structure with the pending changes replayed onto it, which the
    /// lists show and the row actions address.
    staged: StagedStructure,
    applying: bool,
    section: Section,
    panel: Option<Panel>,
    /// Whether the pending changes bar shows their statements.
    sql_visible: bool,
    apply_preview: Entity<SqlView>,
    columns_table_state: Entity<TableState<ColumnsTableDelegate>>,
    indexes_table_state: Entity<TableState<IndexesTableDelegate>>,
    foreign_keys_table_state: Entity<TableState<ForeignKeysTableDelegate>>,
    focus_handle: FocusHandle,
    loading: bool,
    error: Option<String>,
    /// The connection is flagged read-only, so DDL is hidden rather than
    /// offered and refused by the connection wrapper.
    read_only: bool,
    ddl_log: Entity<SqlView>,
    ddl_text: Option<String>,
    ddl_loading: bool,
    ddl_error: Option<String>,
    /// Filters the columns grid by name.
    column_search: Entity<InputState>,
    table_comment: Entity<TextareaState>,
    /// The name field of a table being created, `None` for a table the
    /// server has.
    creating: Option<Entity<InputState>>,
    /// The changes a new table starts with, so closing it untouched asks
    /// nothing.
    initial_changes: Vec<StructureChange>,
    _subscriptions: Vec<Subscription>,
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
        let language = crate::sql::highlight_language(context.db_type);
        let ddl_log = cx.new(|cx| SqlView::new(1, highlight_theme.clone(), language, cx));
        let apply_preview = cx.new(|cx| SqlView::new(usize::MAX, highlight_theme, language, cx));

        let indexes = order_indexes(indexes, &columns);
        let tab = cx.entity().downgrade();
        let dialect = context.dialect();
        let columns_table_state = cx.new(|cx| {
            TableState::new(ColumnsTableDelegate::new(dialect, tab.clone()), window, cx)
                .row_selectable(false)
                .cell_selectable(true)
                .col_selectable(false)
        });
        let indexes_table_state = cx.new(|cx| {
            TableState::new(IndexesTableDelegate::new(dialect, tab.clone()), window, cx)
                .row_selectable(false)
                .cell_selectable(true)
                .col_selectable(false)
        });
        let foreign_keys_table_state = cx.new(|cx| {
            TableState::new(
                ForeignKeysTableDelegate::new(context.schema_name.clone(), tab),
                window,
                cx,
            )
            .row_selectable(false)
            .cell_selectable(true)
            .col_selectable(false)
        });

        let column_search = cx.new(|cx| InputState::new(window, cx).placeholder("Filter columns"));
        let table_comment = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 8)
                .placeholder("No comment")
        });
        let subscriptions = vec![
            cx.subscribe(&column_search, |this, search, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let filter = search.read(cx).value().to_string();
                    this.columns_table_state.update(cx, |table, cx| {
                        table.delegate_mut().set_filter(filter);
                        table.clear_selection(cx);
                        cx.notify();
                    });
                }
            }),
            cx.subscribe_in(
                &columns_table_state,
                window,
                |this, table, event: &TableEvent, window, cx| {
                    // A selection made before switching sections arrives
                    // after the switch, and must not open its row there.
                    if this.section == Section::Columns
                        && let Some(row) = selected_row(event)
                        && let Some(column_ix) = table.read(cx).delegate().column_at(row)
                    {
                        this.show_column(column_ix, false, window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &indexes_table_state,
                window,
                |this, _, event: &TableEvent, _, cx| {
                    if this.section == Section::Indexes
                        && let Some(row) = selected_row(event)
                    {
                        this.panel = Some(Panel::Index(row));
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &foreign_keys_table_state,
                window,
                |this, table, event: &TableEvent, window, cx| {
                    if this.section != Section::ForeignKeys {
                        return;
                    }
                    let Some(row) = selected_row(event) else {
                        return;
                    };
                    match table
                        .read(cx)
                        .delegate()
                        .virtual_key_at(row)
                        .map(|key| key.id)
                    {
                        Some(id) => this.edit_virtual_key(id, window, cx),
                        None => {
                            this.panel = Some(Panel::ForeignKey(row));
                            cx.notify();
                        }
                    }
                },
            ),
        ];

        let mut tab = Self {
            title,
            context,
            table_name,
            columns,
            indexes,
            foreign_keys: Vec::new(),
            table_info: TableInfo::default(),
            server_version_num: None,
            user_types: Vec::new(),
            virtual_keys: Vec::new(),
            pending: Vec::new(),
            staged: StagedStructure::default(),
            applying: false,
            section: Section::Columns,
            panel: None,
            sql_visible: false,
            apply_preview,
            columns_table_state,
            indexes_table_state,
            foreign_keys_table_state,
            focus_handle: cx.focus_handle(),
            loading: false,
            error: None,
            read_only: false,
            ddl_log,
            ddl_text: None,
            ddl_loading: false,
            ddl_error: None,
            column_search,
            table_comment,
            creating: None,
            initial_changes: Vec::new(),
            _subscriptions: subscriptions,
        };
        tab.restage(window, cx);
        tab
    }

    /// A tab for a table not created yet. It starts with an `id` key column
    /// the database numbers itself where it can, and its changes become one
    /// `CREATE TABLE` rather than a list of alterations.
    pub fn new_table(
        context: ConnectionContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let dialect = context.dialect();
        let mut tab = Self::new(context, String::new(), Vec::new(), Vec::new(), window, cx);
        tab.title = "New table".to_string();
        tab.sql_visible = true;
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Table name"));
        tab._subscriptions
            .push(cx.subscribe(&name, |this, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.fill_apply_preview(cx);
                    cx.notify();
                }
            }));
        tab.creating = Some(name);
        let (key_type, attributes) = dialect.new_table_key();
        tab.stage(
            StructureChange::Column(ColumnOperation::Add(ColumnSpec {
                name: "id".to_string(),
                data_type: key_type.to_string(),
                attributes: attributes.map(str::to_string),
                primary_key: true,
                ..ColumnSpec::default()
            })),
            window,
            cx,
        );
        tab.initial_changes = tab
            .pending
            .iter()
            .map(|pending| pending.change.clone())
            .collect();
        tab
    }

    pub(crate) fn is_creating(&self) -> bool {
        self.creating.is_some()
    }

    fn new_table_name(&self, cx: &App) -> String {
        self.creating
            .as_ref()
            .map(|name| name.read(cx).value().trim().to_string())
            .unwrap_or_default()
    }

    /// The table the pending changes describe, for a table being created.
    /// Editing a column still to be added replaces its addition, so every
    /// column, index and key appears once.
    fn new_table_definition(&self) -> NewTable {
        let mut definition = NewTable::default();
        for pending in &self.pending {
            match &pending.change {
                StructureChange::Column(ColumnOperation::Add(spec)) => {
                    definition.columns.push(spec.clone())
                }
                StructureChange::Index(IndexOperation::Create(spec)) => {
                    definition.indexes.push(spec.clone())
                }
                StructureChange::ForeignKey(operation @ ForeignKeyOperation::Create { .. }) => {
                    definition.foreign_keys.push(operation.clone())
                }
                _ => {}
            }
        }
        definition
    }

    /// The statements that create the new table, under `name`.
    fn create_statements(&self, name: &str) -> Result<Vec<String>, DdlError> {
        create_table_sql(
            self.context.db_type,
            self.context.schema_name.as_deref(),
            name,
            &self.new_table_definition(),
        )
    }

    /// Whether the backend has foreign keys to list. ClickHouse has none.
    fn shows_foreign_keys(&self) -> bool {
        self.context.dialect().supports_foreign_keys()
    }

    /// Whether this driver can produce a `CREATE TABLE` statement. MSSQL has
    /// no implementation, so the section is left out.
    fn supports_table_ddl(&self) -> bool {
        self.context.dialect().supports_create_table_ddl() && !self.is_creating()
    }

    /// Whether structure changes are offered: the backend must have them,
    /// the connection must be writable and no Apply may be running.
    fn can_edit_structure(&self) -> bool {
        self.context.dialect().supports_column_ddl() && !self.read_only && !self.applying
    }

    fn loaded(&self) -> LoadedStructure<'_> {
        LoadedStructure {
            columns: &self.columns,
            indexes: &self.indexes,
            foreign_keys: &self.foreign_keys,
            table_comment: self.table_info.comment.as_deref(),
        }
    }

    /// Fetch the structure, table information and the read-only flag from
    /// the server and rebuild the lists. The create statement is fetched
    /// again the next time its section shows.
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        cx.notify();

        let db_service = database::DatabaseService::global(cx).clone();
        let connection_id = self.context.connection_id;
        let database_name = self.context.database_name.clone();
        let schema_name = self.context.schema_name.clone();
        let table_name = self.table_name.clone();
        let shows_foreign_keys = self.shows_foreign_keys();
        let creating = self.is_creating();
        let dialect = self.context.dialect();
        let app_database = AppDatabase::global(cx).clone();
        let virtual_keys_database = database_name.clone();
        let virtual_keys_schema = schema_name.clone();
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
                let schema = schema_name.as_deref();
                // A table being created has nothing on the server yet; the
                // tab still needs the connection's flags and types.
                let (columns, indexes) = if creating {
                    (Vec::new(), Vec::new())
                } else {
                    (
                        connection
                            .get_columns_for_table(&table_name, schema)
                            .await?,
                        connection
                            .get_indexes_for_table(&table_name, schema)
                            .await?,
                    )
                };
                let foreign_keys = if shows_foreign_keys && !creating {
                    connection
                        .get_foreign_keys_for_table(&table_name, schema)
                        .await?
                } else {
                    Vec::new()
                };
                // The extras only add to the tab, so a failure leaves them
                // out rather than failing the structure.
                let table_info = if creating {
                    TableInfo::default()
                } else {
                    connection
                        .table_info(schema, &table_name)
                        .await
                        .inspect_err(|error| {
                            tracing::warn!("Reading table information of {table_name}: {error:#}")
                        })
                        .unwrap_or_default()
                };
                let server_version_num = if dialect.has_nulls_not_distinct() {
                    connection.server_version_num().await.log_err().flatten()
                } else {
                    None
                };
                let user_types = connection
                    .list_types(schema)
                    .await
                    .log_err()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|(_, kind)| *kind != ObjectKind::Sequence)
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>();
                Ok(Loaded {
                    columns,
                    indexes,
                    foreign_keys,
                    table_info,
                    server_version_num,
                    user_types,
                    read_only,
                })
            })
            .await;
            let virtual_keys = crate::virtual_keys::load(
                &app_database,
                connection_id,
                &virtual_keys_database,
                virtual_keys_schema.as_deref(),
            )
            .await;

            this.update_in(cx, |this, window, cx| {
                this.loading = false;
                this.virtual_keys = virtual_keys
                    .into_iter()
                    .filter(|key| key.table_name == this.table_name)
                    .collect();
                match result {
                    Ok(loaded) => {
                        this.read_only = loaded.read_only;
                        this.indexes = order_indexes(loaded.indexes, &loaded.columns);
                        this.columns = loaded.columns;
                        this.foreign_keys = loaded.foreign_keys;
                        this.table_info = loaded.table_info;
                        this.server_version_num = loaded.server_version_num;
                        this.user_types = loaded.user_types;
                        if !this.is_creating() {
                            this.panel = None;
                        }
                        this.restage(window, cx);
                        this.reset_table_comment(window, cx);
                        this.ddl_text = None;
                        if this.section == Section::CreateStatement {
                            this.load_ddl(cx);
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

    fn reset_table_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let comment = self.staged_table_comment().to_string();
        self.table_comment.update(cx, |input, cx| {
            input.set_value(comment, window, cx);
        });
    }

    fn staged_table_comment(&self) -> &str {
        self.staged.table_comment.as_deref().unwrap_or_default()
    }

    fn load_ddl(&mut self, cx: &mut Context<Self>) {
        if self.ddl_loading {
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
            let result = gpui_tokio::Tokio::spawn_result(cx, async move {
                use database::DatabaseServiceTrait as _;
                let connection = db_service
                    .get_or_create_connection_by_id(connection_id, Some(&database_name))
                    .await?;
                connection
                    .table_ddl(schema_name.as_deref(), &table_name)
                    .await
            })
            .await;

            this.update(cx, |this, cx| {
                this.ddl_loading = false;
                match result {
                    Ok(ddl) => {
                        this.ddl_log.update(cx, |log, cx| {
                            log.clear(cx);
                            log.append_text(&SqlViewMessage::SqlStatement(ddl.clone()), cx);
                        });
                        this.ddl_text = Some(ddl);
                    }
                    Err(error) => this.ddl_error = Some(format!("{error:#}")),
                }
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }

    pub(crate) fn has_pending_changes(&self, cx: &App) -> bool {
        if self.is_creating() {
            return !self.new_table_name(cx).is_empty()
                || self
                    .pending
                    .iter()
                    .map(|pending| &pending.change)
                    .ne(self.initial_changes.iter());
        }
        !self.pending.is_empty()
    }

    pub(crate) fn set_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if self.section == section {
            return;
        }
        self.section = section;
        self.panel = None;
        if section == Section::CreateStatement && self.ddl_text.is_none() {
            self.load_ddl(cx);
        }
        cx.notify();
    }

    fn notify_error(&self, message: String, window: &mut Window, cx: &mut Context<Self>) {
        window.push_notification((NotificationType::Error, message), cx);
    }

    /// Stage `change` after the pending ones. A change the backend cannot
    /// make, or that does not fit the staged structure, is refused here so
    /// Apply never meets it. Returns whether it was staged.
    fn stage(
        &mut self,
        change: StructureChange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(staged) = self.pending_change(change, window, cx) else {
            return false;
        };
        let mut pending = self.pending.clone();
        pending.push(staged);
        self.try_pending(pending, window, cx)
    }

    /// Replace the pending change at `change_ix`, as editing a column that is
    /// still to be added does.
    fn replace_change(
        &mut self,
        change_ix: usize,
        change: StructureChange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if change_ix >= self.pending.len() {
            return false;
        }
        let Some(staged) = self.pending_change(change, window, cx) else {
            return false;
        };
        let mut pending = self.pending.clone();
        pending[change_ix] = staged;
        self.try_pending(pending, window, cx)
    }

    fn pending_change(
        &self,
        change: StructureChange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<PendingChange> {
        // A new table is created in one statement, checked as a whole when
        // the change is taken.
        if self.is_creating() {
            return Some(PendingChange {
                change,
                statements: Vec::new(),
            });
        }
        match change.statements(
            self.context.db_type,
            self.context.schema_name.as_deref(),
            &self.table_name,
        ) {
            Ok(statements) => Some(PendingChange { change, statements }),
            Err(error) => {
                self.notify_error(format!("{}: {error}", change.label()), window, cx);
                None
            }
        }
    }

    /// Take `pending` as the new list of changes if every one of them still
    /// fits the structure.
    fn try_pending(
        &mut self,
        pending: Vec<PendingChange>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let replayed = staging::replay(self.loaded(), pending.iter().map(|p| &p.change));
        if let Some((ix, reason)) = replayed.rejected.first() {
            let label = pending
                .get(*ix)
                .map(|pending| pending.change.label())
                .unwrap_or_default();
            self.notify_error(format!("{label}: {reason}"), window, cx);
            return false;
        }
        let previous = std::mem::replace(&mut self.pending, pending);
        if self.is_creating() {
            // The name may still be empty, which says nothing about the rest.
            let name = Some(self.new_table_name(cx))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "new_table".to_string());
            if let Err(error) = self.create_statements(&name) {
                self.pending = previous;
                self.notify_error(format!("New table: {error}"), window, cx);
                return false;
            }
        }
        self.restage(window, cx);
        true
    }

    /// Stage a column change as the side panel would.
    #[cfg(feature = "screenshots")]
    pub(crate) fn stage_column(
        &mut self,
        operation: ColumnOperation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stage(StructureChange::Column(operation), window, cx);
    }

    /// Stage an index change as the side panel would.
    #[cfg(feature = "screenshots")]
    pub(crate) fn stage_index(
        &mut self,
        operation: IndexOperation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stage(StructureChange::Index(operation), window, cx);
    }

    /// Open the last column in the side panel, as selecting it would.
    #[cfg(feature = "screenshots")]
    pub(crate) fn show_last_column(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.staged.columns.len().checked_sub(1) {
            self.select_column_row(row, cx);
            self.show_column(row, false, window, cx);
        }
    }

    /// Drop pending changes. Later changes that depended on them (an index
    /// on a column one added) go with them.
    pub fn undo_changes(
        &mut self,
        mut change_ixs: Vec<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        change_ixs.sort_unstable();
        change_ixs.dedup();
        for change_ix in change_ixs.into_iter().rev() {
            if change_ix < self.pending.len() {
                self.pending.remove(change_ix);
            }
        }
        self.restage(window, cx);
    }

    pub fn undo_change(&mut self, change_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.undo_changes(vec![change_ix], window, cx);
    }

    pub fn discard_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pending.clear();
        self.restage(window, cx);
    }

    /// Replay the pending changes onto the loaded structure and show the
    /// result. Changes that no longer fit, after an undo or a reload, are
    /// dropped and named in a notification.
    fn restage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut replayed = staging::replay(self.loaded(), self.pending.iter().map(|p| &p.change));
        if !replayed.rejected.is_empty() {
            let mut dropped: Vec<String> = replayed
                .rejected
                .iter()
                .rev()
                .map(|(ix, reason)| {
                    let pending = self.pending.remove(*ix);
                    format!("{} ({reason})", pending.change.describe())
                })
                .collect();
            dropped.reverse();
            window.push_notification(
                (
                    NotificationType::Warning,
                    format!("Undid changes that no longer apply: {}", dropped.join(", ")),
                ),
                cx,
            );
            // The rejected changes changed nothing, so this replay matches
            // the first one with the change indices shifted.
            replayed = staging::replay(self.loaded(), self.pending.iter().map(|p| &p.change));
        }
        let comment_was_staged = !self.staged.table_mark.changes.is_empty();
        self.staged = replayed.structure;
        if comment_was_staged && self.staged.table_mark.changes.is_empty() {
            self.reset_table_comment(window, cx);
        }
        self.fill_apply_preview(cx);
        self.show_staged(window, cx);
        self.refresh_panel(window, cx);
    }

    /// Hand the staged structure to the three lists, each column sized to
    /// its contents.
    fn show_staged(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editable = self.can_edit_structure();
        let staged = &self.staged;
        self.columns_table_state.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.set_rows(
                staged.columns.clone(),
                staged.column_marks.clone(),
                editable,
            );
            delegate.fit_columns(window, cx);
            table.refresh(cx);
        });
        self.indexes_table_state.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.indexes = staged.indexes.clone();
            delegate.marks = staged.index_marks.clone();
            delegate.editable = editable;
            delegate.fit_columns(window, cx);
            table.refresh(cx);
        });
        let virtual_keys = self.virtual_keys.clone();
        self.foreign_keys_table_state.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.foreign_keys = staged.foreign_keys.clone();
            delegate.marks = staged.foreign_key_marks.clone();
            delegate.virtual_keys = virtual_keys;
            delegate.editable = editable;
            delegate.fit_columns(window, cx);
            table.refresh(cx);
        });
        cx.notify();
    }

    /// Show the panel's row as it is staged now. A listed column's form is
    /// rebuilt from the staged column. A row that is gone closes the panel.
    fn refresh_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.panel {
            Some(Panel::Column { row, .. }) => self.show_column(row, true, window, cx),
            Some(Panel::Index(row)) if row >= self.staged.indexes.len() => self.panel = None,
            Some(Panel::ForeignKey(row)) if row >= self.staged.foreign_keys.len() => {
                self.panel = None
            }
            _ => {}
        }
    }

    /// Open the staged column `row` in the panel. Selecting the row the
    /// panel already shows keeps its unsaved edits unless `rebuild`.
    fn show_column(
        &mut self,
        row: usize,
        rebuild: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !rebuild && matches!(self.panel, Some(Panel::Column { row: shown, .. }) if shown == row)
        {
            return;
        }
        let Some(column) = self.staged.columns.get(row) else {
            self.panel = None;
            cx.notify();
            return;
        };
        let live = self
            .staged
            .column_marks
            .get(row)
            .is_none_or(RowMark::is_live);
        let form = (live && self.can_edit_structure()).then(|| {
            let dialect = self.context.dialect();
            let spec = column_spec(column, &self.table_info.column_definitions);
            let user_types = self.user_types.clone();
            let creating = self.is_creating();
            cx.new(|cx| ColumnForm::new(dialect, Some(spec), user_types, creating, window, cx))
        });
        self.panel = Some(Panel::Column { row, form });
        cx.notify();
    }

    /// Open the column `row` (of the staged columns) for editing, as the
    /// grid's menu does.
    pub fn edit_column(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.section = Section::Columns;
        self.show_column(row, false, window, cx);
        if let Some(Panel::Column {
            form: Some(form), ..
        }) = &self.panel
        {
            let input = form.read(cx).first_input();
            input.update(cx, |input, cx| input.focus(window, cx));
        }
    }

    pub fn new_column(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dialect = self.context.dialect();
        let user_types = self.user_types.clone();
        let creating = self.is_creating();
        let form = cx.new(|cx| ColumnForm::new(dialect, None, user_types, creating, window, cx));
        let input = form.read(cx).first_input();
        input.update(cx, |input, cx| input.focus(window, cx));
        self.columns_table_state
            .update(cx, |table, cx| table.clear_selection(cx));
        self.panel = Some(Panel::NewColumn(form));
        cx.notify();
    }

    pub fn new_index(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dialect = self.context.dialect();
        let columns = self.staged.live_column_names();
        let table_name = self.table_name.clone();
        let server_version_num = self.server_version_num;
        let form = cx.new(|cx| {
            IndexForm::new(
                dialect,
                server_version_num,
                &table_name,
                columns,
                window,
                cx,
            )
        });
        let input = form.read(cx).first_input();
        input.update(cx, |input, cx| input.focus(window, cx));
        self.indexes_table_state
            .update(cx, |table, cx| table.clear_selection(cx));
        self.panel = Some(Panel::NewIndex(form));
        cx.notify();
    }

    pub fn new_foreign_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let columns = self.staged.live_column_names();
        let context = self.context.clone();
        let table_name = self.table_name.clone();
        let form = cx.new(|cx| ForeignKeyForm::new(context, table_name, columns, window, cx));
        self.foreign_keys_table_state
            .update(cx, |table, cx| table.clear_selection(cx));
        self.panel = Some(Panel::NewForeignKey(form));
        cx.notify();
    }

    /// Open a new virtual key (`id` 0) or a saved one in the panel.
    pub fn edit_virtual_key(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(&self.panel, Some(Panel::VirtualKey { id: shown, .. }) if *shown == id && id != 0)
        {
            return;
        }
        let existing = self.virtual_keys.iter().find(|key| key.id == id).cloned();
        if id != 0 && existing.is_none() {
            return;
        }
        // The server's columns: a staged column does not exist yet.
        let columns = self
            .columns
            .iter()
            .map(|column| column.name.clone())
            .collect();
        let context = self.context.clone();
        let table_name = self.table_name.clone();
        let form = cx.new(|cx| {
            ForeignKeyForm::new_virtual(context, table_name, columns, existing.as_ref(), window, cx)
        });
        if id == 0 {
            self.foreign_keys_table_state
                .update(cx, |table, cx| table.clear_selection(cx));
        }
        self.section = Section::ForeignKeys;
        self.panel = Some(Panel::VirtualKey { id, form });
        cx.notify();
    }

    fn save_virtual_key(
        &mut self,
        form: &Entity<ForeignKeyForm>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = match form.read(cx).virtual_key(cx) {
            Ok(key) => key,
            Err(message) => {
                self.notify_error(format!("Virtual key: {message}"), window, cx);
                return;
            }
        };
        let app_database = AppDatabase::global(cx).clone();
        cx.spawn_in(window, async move |this, cx| {
            let saved = app_database.save_virtual_foreign_key(&key).await;
            this.update_in(cx, |this, window, cx| match saved {
                Ok(id) => {
                    let mut key = key;
                    key.id = id;
                    match this.virtual_keys.iter_mut().find(|existing| existing.id == id) {
                        Some(existing) => *existing = key,
                        None => this.virtual_keys.push(key),
                    }
                    this.virtual_keys.sort_by(|a, b| a.name.cmp(&b.name));
                    this.show_staged(window, cx);
                    this.panel = None;
                    this.edit_virtual_key(id, window, cx);
                }
                Err(error) => {
                    let message = if matches!(&error, sqlx::Error::Database(db) if db.is_unique_violation()) {
                        format!("The table already has a virtual key named \"{}\"", key.name)
                    } else {
                        format!("Saving the virtual key failed: {error}")
                    };
                    this.notify_error(message, window, cx);
                }
            })
            .log_err();
        })
        .detach();
    }

    pub fn delete_virtual_key(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let app_database = AppDatabase::global(cx).clone();
        cx.spawn_in(window, async move |this, cx| {
            let deleted = app_database.delete_virtual_foreign_key(id).await;
            this.update_in(cx, |this, window, cx| match deleted {
                Ok(()) => {
                    this.virtual_keys.retain(|key| key.id != id);
                    if matches!(&this.panel, Some(Panel::VirtualKey { id: shown, .. }) if *shown == id) {
                        this.panel = None;
                    }
                    this.show_staged(window, cx);
                }
                Err(error) => this.notify_error(
                    format!("Deleting the virtual key failed: {error}"),
                    window,
                    cx,
                ),
            })
            .log_err();
        })
        .detach();
    }

    fn close_panel(&mut self, cx: &mut Context<Self>) {
        self.panel = None;
        self.columns_table_state
            .update(cx, |table, cx| table.clear_selection(cx));
        self.indexes_table_state
            .update(cx, |table, cx| table.clear_selection(cx));
        self.foreign_keys_table_state
            .update(cx, |table, cx| table.clear_selection(cx));
        cx.notify();
    }

    /// Stage what the panel's form describes.
    pub(crate) fn save_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.panel {
            Some(Panel::NewColumn(form)) => {
                let Some(operation) = form.read(cx).operation(cx) else {
                    return;
                };
                if self.stage(StructureChange::Column(operation), window, cx) {
                    let row = self.staged.columns.len().saturating_sub(1);
                    self.show_column(row, true, window, cx);
                    self.select_column_row(row, cx);
                }
            }
            Some(Panel::Column {
                row,
                form: Some(form),
            }) => {
                let row = *row;
                let form = form.read(cx);
                let mark = self
                    .staged
                    .column_marks
                    .get(row)
                    .cloned()
                    .unwrap_or_default();
                if mark.state == RowState::Added {
                    // A column still to be added is changed by changing the
                    // addition, so Apply adds it as it is now described.
                    let add_ix = mark.changes.iter().map(|(ix, _)| *ix).find(|ix| {
                        matches!(
                            self.pending.get(*ix).map(|pending| &pending.change),
                            Some(StructureChange::Column(ColumnOperation::Add(_)))
                        )
                    });
                    let mut spec = form.spec(cx);
                    if let Some(add_ix) = add_ix {
                        // The form shows no MySQL attributes, so the
                        // addition keeps its own (the new key's AUTO_INCREMENT).
                        if let Some(StructureChange::Column(ColumnOperation::Add(added))) =
                            self.pending.get(add_ix).map(|pending| &pending.change)
                        {
                            spec.attributes = added.attributes.clone();
                        }
                        self.replace_change(
                            add_ix,
                            StructureChange::Column(ColumnOperation::Add(spec)),
                            window,
                            cx,
                        );
                    }
                } else if let Some(operation) = form.operation(cx) {
                    self.stage(StructureChange::Column(operation), window, cx);
                }
            }
            Some(Panel::NewIndex(form)) => {
                let operation = form.read(cx).operation(cx);
                if self.stage(StructureChange::Index(operation), window, cx) {
                    let row = self.staged.indexes.len().saturating_sub(1);
                    self.panel = Some(Panel::Index(row));
                    cx.notify();
                }
            }
            Some(Panel::NewForeignKey(form)) => {
                let operation = form.read(cx).operation(cx);
                if self.stage(StructureChange::ForeignKey(operation), window, cx) {
                    let row = self.staged.foreign_keys.len().saturating_sub(1);
                    self.panel = Some(Panel::ForeignKey(row));
                    cx.notify();
                }
            }
            Some(Panel::VirtualKey { form, .. }) => {
                let form = form.clone();
                self.save_virtual_key(&form, window, cx);
            }
            Some(Panel::Column { form: None, .. })
            | Some(Panel::Index(_))
            | Some(Panel::ForeignKey(_))
            | None => {}
        }
    }

    fn select_column_row(&mut self, column_ix: usize, cx: &mut Context<Self>) {
        self.columns_table_state.update(cx, |table, cx| {
            if let Some(row) = table.delegate().row_of(column_ix) {
                table.set_selected_row(row, cx);
                table.scroll_to_row(row, cx);
            }
        });
    }

    pub fn drop_column(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(column) = self.staged.columns.get(row) {
            let name = column.name.clone();
            self.stage(
                StructureChange::Column(ColumnOperation::Drop { name }),
                window,
                cx,
            );
        }
    }

    pub fn drop_index(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.staged.indexes.get(row) {
            let name = index.name.clone();
            self.stage(
                StructureChange::Index(IndexOperation::Drop { name }),
                window,
                cx,
            );
        }
    }

    pub fn drop_foreign_key(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(foreign_key) = self.staged.foreign_keys.get(row) {
            let name = foreign_key.name.clone();
            self.stage(
                StructureChange::ForeignKey(ForeignKeyOperation::Drop { name }),
                window,
                cx,
            );
        }
    }

    fn stage_table_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let comment = self.table_comment.read(cx).value().trim().to_string();
        if comment == self.staged_table_comment().trim() {
            return;
        }
        // Changing a staged comment again replaces it, and back to the
        // server's comment undoes it.
        let staged_ix = self.staged.table_mark.changes.last().map(|(ix, _)| *ix);
        let loaded = self
            .table_info
            .comment
            .as_deref()
            .unwrap_or_default()
            .trim();
        if let Some(ix) = staged_ix
            && comment == loaded
        {
            self.undo_change(ix, window, cx);
            return;
        }
        let comment = (!comment.is_empty()).then_some(comment);
        let change = StructureChange::Table(TableChange::Comment(comment));
        match staged_ix {
            Some(ix) => self.replace_change(ix, change, window, cx),
            None => self.stage(change, window, cx),
        };
    }

    /// Fill the preview with every pending statement, each change's under a
    /// comment naming it.
    fn fill_apply_preview(&self, cx: &mut Context<Self>) {
        if self.is_creating() {
            let name = self.new_table_name(cx);
            let statements = if name.is_empty() {
                Err("Name the table to see its statement".to_string())
            } else {
                self.create_statements(&name)
                    .map_err(|error| error.to_string())
            };
            self.apply_preview.update(cx, |log, cx| {
                log.clear(cx);
                match statements {
                    Ok(statements) => {
                        for statement in statements {
                            log.append_text(&SqlViewMessage::SqlStatement(statement), cx);
                        }
                    }
                    Err(message) => log.append_text(&SqlViewMessage::Comment(message), cx),
                }
            });
            return;
        }
        let pending = self.pending.clone();
        self.apply_preview.update(cx, |log, cx| {
            log.clear(cx);
            for change in pending {
                log.append_text(&SqlViewMessage::Comment(change.change.describe()), cx);
                for statement in change.statements {
                    log.append_text(&SqlViewMessage::SqlStatement(statement), cx);
                }
            }
        });
    }

    /// Apply right away, or after a confirmation on a connection whose
    /// environment asks for one.
    fn request_apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_empty() || self.applying {
            return;
        }
        if self.is_creating() && self.new_table_name(cx).is_empty() {
            self.notify_error("Name the table first".to_string(), window, cx);
            return;
        }
        if !self.context.confirms_writes() {
            self.apply_changes(window, cx);
            return;
        }
        let environment = self
            .context
            .environment_type
            .map(|environment| environment.to_string())
            .unwrap_or_default();
        let count = if self.is_creating() {
            format!("the table {}", self.new_table_name(cx))
        } else {
            changes_label(self.pending.len())
        };
        let action = if self.is_creating() {
            "Create"
        } else {
            "Apply"
        };
        let preview = self.apply_preview.clone();
        let tab = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let tab = tab.clone();
            dialog
                .title(format!("{action} {count} on {environment}?"))
                .w(px(560.))
                .child(div().text_sm().child("The statements run in this order."))
                .child(
                    v_flex()
                        .h(px(280.))
                        .min_h_0()
                        .overflow_hidden()
                        .child(preview.clone()),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().child(
                                Button::new("apply-structure-cancel")
                                    .label("Cancel")
                                    .outline(),
                            ),
                        )
                        .child(
                            DialogAction::new().child(
                                Button::new("apply-structure-confirm")
                                    .danger()
                                    .icon(IconName::Check)
                                    .label(format!("{action} on {environment}"))
                                    .on_click(move |_, window, cx| {
                                        tab.update(cx, |tab, cx| tab.apply_changes(window, cx))
                                            .log_err();
                                    }),
                            ),
                        ),
                )
        });
    }

    /// Run every pending statement as one batch, in one transaction where
    /// the backend can roll DDL back, then reload the tab and the sidebar's
    /// view of the connection. A failure reloads too: where DDL commits on
    /// its own the server may hold part of the batch, and the reload drops
    /// the changes that no longer apply.
    pub fn apply_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_empty() || self.applying {
            return;
        }
        let new_name = self.creating.as_ref().map(|_| self.new_table_name(cx));
        let statements: Vec<String> = match &new_name {
            Some(name) => match self.create_statements(name) {
                Ok(statements) => statements,
                Err(error) => {
                    self.notify_error(format!("New table: {error}"), window, cx);
                    return;
                }
            },
            None => self
                .pending
                .iter()
                .flat_map(|pending| pending.statements.clone())
                .collect(),
        };
        let operations: Vec<blanco_core::WriteOperation> =
            statements.iter().map(|sql| sql.as_str().into()).collect();
        let change_count = self.pending.len();
        let db_service = database::DatabaseService::global(cx).clone();
        let connection_id = self.context.connection_id;
        let database_name = self.context.database_name.clone();
        let table_name = new_name.clone().unwrap_or_else(|| self.table_name.clone());
        let activity = ActivityReporter::global(cx).begin(format!(
            "{}: {}",
            self.context.connection_name,
            if new_name.is_some() {
                "creating a table"
            } else {
                "applying structure changes"
            }
        ));
        self.applying = true;
        self.panel = None;
        self.show_staged(window, cx);

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
            this.update_in(cx, |this, window, cx| {
                this.applying = false;
                match result {
                    Ok(()) => {
                        this.pending.clear();
                        let message = if this.creating.take().is_some() {
                            // The tab now shows the table the server has.
                            this.table_name = table_name.clone();
                            this.title = table_name.clone();
                            this.initial_changes.clear();
                            this.sql_visible = false;
                            format!("Created {table_name}")
                        } else {
                            format!("Applied {} to {table_name}", changes_label(change_count))
                        };
                        window.push_notification((NotificationType::Success, message), cx);
                    }
                    Err(error) => {
                        let what = if this.is_creating() {
                            format!("Creating {table_name}")
                        } else {
                            format!("Applying changes to {table_name}")
                        };
                        tracing::error!("{what} failed: {error:#}");
                        window.push_notification(
                            (NotificationType::Error, format!("{what} failed: {error:#}")),
                            cx,
                        );
                        // A failed create leaves nothing to reload, and the
                        // definition stays to be fixed.
                        if this.is_creating() {
                            this.restage(window, cx);
                            return;
                        }
                    }
                }
                this.reload(window, cx);
                window.dispatch_action(
                    Box::new(crate::app::RefreshConnectionTree { connection_id }),
                    cx,
                );
            })
            .log_err();
        })
        .detach();
    }

    fn open_ddl_in_query_tab(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ddl) = self.ddl_text.clone() else {
            return;
        };
        window.dispatch_action(
            Box::new(crate::app::ShowQueryInNewTab {
                connection_id: self.context.connection_id,
                database_name: self.context.database_name.clone(),
                schema_name: self.context.schema_name.clone(),
                title: format!("{} DDL", self.table_name),
                sql: ddl,
            }),
            cx,
        );
    }
}

/// What a reload fetched.
struct Loaded {
    columns: Vec<ColumnInfo>,
    indexes: Vec<IndexInfo>,
    foreign_keys: Vec<ForeignKeyConstraint>,
    table_info: TableInfo,
    server_version_num: Option<u32>,
    user_types: Vec<String>,
    read_only: bool,
}

/// The row a click or a keyboard move selected in one of the grids.
fn selected_row(event: &TableEvent) -> Option<usize> {
    match event {
        TableEvent::SelectRow(row) | TableEvent::SelectCell(row, _) => Some(*row),
        _ => None,
    }
}

fn changes_label(count: usize) -> String {
    match count {
        1 => "1 change".to_string(),
        count => format!("{count} changes"),
    }
}

/// `12345678` as `12,345,678`.
fn group_digits(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (ix, digit) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

fn format_bytes(bytes: u64) -> String {
    blanco_core::binary::format_size(usize::try_from(bytes).unwrap_or(usize::MAX))
}

impl Focusable for TableStructureTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TableStructureTab {
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let mut path = vec![self.context.database_name.clone()];
        path.extend(
            self.context
                .schema_name
                .clone()
                .filter(|schema| !schema.is_empty()),
        );
        h_flex()
            .px_4()
            .py_2()
            .gap_3()
            .border_b_1()
            .border_color(theme.border)
            .map(|this| match &self.creating {
                Some(name) => this.child(Input::new(name).small().w(px(260.))),
                None => this.child(
                    div()
                        .text_base()
                        .font_semibold()
                        .text_color(theme.foreground)
                        .child(self.table_name.clone()),
                ),
            })
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(path.join(" / ")),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(self.context.db_type.as_str()),
            )
            .when(self.read_only, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Read-only connection"),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("reload-structure")
                    .ghost()
                    .small()
                    .icon(IconName::RotateCcw)
                    .tooltip("Reload the structure from the server")
                    .loading(self.loading)
                    .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
            )
    }

    fn sections(&self) -> Vec<(Section, &'static str, Option<usize>)> {
        let live = |marks: &[RowMark]| marks.iter().filter(|mark| mark.is_live()).count();
        let mut sections = vec![
            (
                Section::Columns,
                "Columns",
                Some(live(&self.staged.column_marks)),
            ),
            (
                Section::Indexes,
                "Indexes",
                Some(live(&self.staged.index_marks)),
            ),
        ];
        // Listed on every backend: a virtual key needs no server support. A
        // new table has none until it exists, so it lists declared keys only.
        if !self.is_creating() || self.shows_foreign_keys() {
            sections.push((
                Section::ForeignKeys,
                "Foreign keys",
                Some(live(&self.staged.foreign_key_marks) + self.virtual_keys.len()),
            ));
        }
        if !self.is_creating() {
            sections.push((Section::Table, "Table", None));
        }
        if self.supports_table_ddl() {
            sections.push((Section::CreateStatement, "Create statement", None));
        }
        sections
    }

    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .w(px(176.))
            .flex_shrink_0()
            .h_full()
            .p_2()
            .gap_0p5()
            .border_r_1()
            .border_color(theme.border)
            .children(self.sections().into_iter().map(|(section, label, count)| {
                let active = self.section == section;
                h_flex()
                    .id(SharedString::from(format!("structure-section-{label}")))
                    .justify_between()
                    .px_2()
                    .py_1p5()
                    .rounded(theme.radius)
                    .text_sm()
                    .cursor_pointer()
                    .text_color(theme.foreground)
                    .when(active, |this| this.bg(theme.list_active))
                    .when(!active, |this| this.hover(|this| this.bg(theme.list_hover)))
                    .child(label)
                    .when_some(count, |this, count| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(count.to_string()),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.set_section(section, cx)))
            }))
    }

    fn render_grid(&self, grid: AnyElement, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .min_h_0()
            .mx_3()
            .mb_3()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius)
            .overflow_hidden()
            .child(grid)
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let can_edit = self.can_edit_structure();
        let dialect = self.context.dialect();
        h_flex()
            .px_3()
            .py_2()
            .gap_2()
            .map(|this| match self.section {
                Section::Columns => this
                    .child(
                        Input::new(&self.column_search)
                            .small()
                            .w(px(240.))
                            .cleanable(true)
                            .prefix(
                                Icon::new(IconName::Search)
                                    .small()
                                    .text_color(theme.muted_foreground),
                            ),
                    )
                    .child(div().flex_1())
                    .when(can_edit, |this| {
                        this.child(
                            Button::new("add-column")
                                .outline()
                                .small()
                                .icon(IconName::Plus)
                                .label("Column")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.new_column(window, cx)),
                                ),
                        )
                    }),
                Section::Indexes => this.child(div().flex_1()).when(can_edit, |this| {
                    this.child(
                        Button::new("add-index")
                            .outline()
                            .small()
                            .icon(IconName::Plus)
                            .label("Index")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.new_index(window, cx)),
                            ),
                    )
                }),
                Section::ForeignKeys => this
                    .child(div().flex_1())
                    .when(!self.is_creating(), |this| {
                        this.child(
                        Button::new("add-virtual-key")
                            .outline()
                            .small()
                            .icon(IconName::Plus)
                            .label("Virtual Key")
                            // Virtual keys are stored with their connection,
                            // which a file opened directly does not have.
                            .disabled(self.context.is_temporary())
                            .tooltip(if self.context.is_temporary() {
                                "Save the connection to give its tables virtual keys"
                            } else {
                                "A foreign key kept in Blanco, for the grid and the schema graph"
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.edit_virtual_key(0, window, cx)
                            })),
                    )
                    })
                    // A new table declares its keys inline, which every
                    // backend with foreign keys takes.
                    .when(
                        can_edit
                            && if self.is_creating() {
                                dialect.supports_foreign_keys()
                            } else {
                                dialect.supports_foreign_key_ddl()
                            },
                        |this| {
                            this.child(
                                Button::new("add-foreign-key")
                                    .outline()
                                    .small()
                                    .icon(IconName::Plus)
                                    .label("Foreign Key")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.new_foreign_key(window, cx)
                                    })),
                            )
                        },
                    ),
                Section::Table => this,
                // The statement view has a copy button of its own.
                Section::CreateStatement => this.child(div().flex_1()).child(
                    Button::new("open-table-ddl")
                        .outline()
                        .small()
                        .label("Open in Query Tab")
                        .disabled(self.ddl_text.is_none())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_ddl_in_query_tab(window, cx)
                        })),
                ),
            })
    }

    fn render_section(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.section {
            Section::Columns => self
                .render_grid(
                    DataTable::new(&self.columns_table_state)
                        .bordered(false)
                        .into_any_element(),
                    cx,
                )
                .into_any_element(),
            Section::Indexes => self
                .render_grid(
                    DataTable::new(&self.indexes_table_state)
                        .bordered(false)
                        .into_any_element(),
                    cx,
                )
                .into_any_element(),
            Section::ForeignKeys => self
                .render_grid(
                    DataTable::new(&self.foreign_keys_table_state)
                        .bordered(false)
                        .into_any_element(),
                    cx,
                )
                .into_any_element(),
            Section::Table => self.render_table_section(cx).into_any_element(),
            Section::CreateStatement => self.render_create_statement(cx).into_any_element(),
        }
    }

    fn render_create_statement(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let editor_bg = theme
            .highlight_theme
            .style
            .editor_background
            .unwrap_or(theme.background);
        div()
            .flex_1()
            .min_h_0()
            .mx_3()
            .mb_3()
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
            })
    }

    fn render_table_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let dialect = self.context.dialect();
        let info = &self.table_info;
        let can_comment = self.can_edit_structure() && dialect.supports_comments();
        let card = || {
            v_flex()
                .gap_3()
                .p_4()
                .min_w(px(300.))
                .flex_1()
                .border_1()
                .border_color(theme.border)
                .rounded(theme.radius)
        };
        let title = |text: &'static str| {
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child(text)
        };
        let stat = |label: String, value: String, note: Option<String>| {
            h_flex()
                .justify_between()
                .items_start()
                .gap_4()
                .py_1p5()
                .border_b_1()
                .border_color(theme.border)
                .text_sm()
                .child(div().text_color(theme.muted_foreground).child(label))
                .child(
                    v_flex()
                        .items_end()
                        .child(div().text_color(theme.foreground).child(value))
                        .when_some(note, |this, note| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(note),
                            )
                        }),
                )
        };

        let mut stats = Vec::new();
        if let Some(rows) = info.estimated_rows {
            stats.push(stat(
                "Rows".into(),
                format!("about {}", group_digits(rows)),
                None,
            ));
        }
        if let Some(bytes) = info.data_bytes {
            stats.push(stat("Data size".into(), format_bytes(bytes), None));
        }
        if let Some(bytes) = info.index_bytes {
            stats.push(stat("Index size".into(), format_bytes(bytes), None));
        }
        if let (Some(data), Some(index)) = (info.data_bytes, info.index_bytes) {
            stats.push(stat("Total size".into(), format_bytes(data + index), None));
        }
        if let Some(created) = &info.created {
            stats.push(stat("Created".into(), created.clone(), None));
        }
        if let Some(modified) = &info.last_modified {
            stats.push(stat(
                "Last modified".into(),
                modified.at.clone(),
                Some(modified.note.clone()),
            ));
        }
        for (label, value) in &info.options {
            stats.push(stat(label.clone(), value.clone(), None));
        }

        let comment_changed =
            self.table_comment.read(cx).value().trim() != self.staged_table_comment().trim();
        v_flex().flex_1().min_h_0().overflow_y_scrollbar().child(
            h_flex()
                .flex_wrap()
                .items_start()
                .gap_3()
                .px_3()
                .pb_3()
                .when(dialect.supports_comments(), |this| {
                    this.child(
                        card()
                            .child(title("Comment"))
                            .child(Textarea::new(&self.table_comment).disabled(!can_comment))
                            .when(can_comment, |this| {
                                this.child(
                                    h_flex().justify_end().child(
                                        Button::new("stage-table-comment")
                                            .small()
                                            .primary()
                                            .label("Save Comment")
                                            .disabled(!comment_changed)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.stage_table_comment(window, cx)
                                            })),
                                    ),
                                )
                            }),
                    )
                })
                .child(card().child(title("Information")).map(|this| {
                    if stats.is_empty() {
                        this.child(div().text_sm().text_color(theme.muted_foreground).child(
                            format!(
                                "{} records no size or change times for tables.",
                                self.context.db_type
                            ),
                        ))
                    } else {
                        this.child(v_flex().children(stats)).child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Rows and sizes are the server's estimates."),
                        )
                    }
                })),
        )
    }

    fn render_panel(&self, panel: &Panel, cx: &mut Context<Self>) -> impl IntoElement {
        let can_edit = self.can_edit_structure();
        let (title, mark, body, buttons) = match panel {
            Panel::NewColumn(form) => (
                "New column".to_string(),
                None,
                form.clone().into_any_element(),
                vec![self.cancel_button(cx), self.save_button("Add Column", cx)],
            ),
            Panel::Column { row, form } => {
                let column = self.staged.columns.get(*row);
                let mark = self
                    .staged
                    .column_marks
                    .get(*row)
                    .cloned()
                    .unwrap_or_default();
                let body = match (form, column) {
                    (Some(form), _) => form.clone().into_any_element(),
                    (None, Some(column)) => details(
                        vec![
                            ("Type", column.display_type()),
                            (
                                "Nullable",
                                if column.is_nullable { "Yes" } else { "No" }.to_string(),
                            ),
                            ("Default", column.default_value.clone().unwrap_or_default()),
                            ("Comment", column.comment.clone().unwrap_or_default()),
                        ],
                        cx,
                    ),
                    (None, None) => div().into_any_element(),
                };
                let mut buttons = Vec::new();
                if can_edit {
                    match mark.state {
                        RowState::Dropped | RowState::Added => {
                            buttons.push(self.undo_button(&mark, cx));
                        }
                        RowState::Unchanged | RowState::Modified => {
                            let row = *row;
                            buttons.push(
                                Button::new("panel-drop-column")
                                    .ghost()
                                    .small()
                                    .label("Drop")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.drop_column(row, window, cx)
                                    }))
                                    .into_any_element(),
                            );
                            if mark.state == RowState::Modified {
                                buttons.push(self.undo_button(&mark, cx));
                            }
                        }
                    }
                    if form.is_some() {
                        buttons.push(div().flex_1().into_any_element());
                        buttons.push(self.save_button("Save", cx));
                    }
                }
                (
                    format!(
                        "Column {}",
                        column
                            .map(|column| column.name.as_str())
                            .unwrap_or_default()
                    ),
                    Some(mark),
                    body,
                    buttons,
                )
            }
            Panel::NewIndex(form) => (
                "New index".to_string(),
                None,
                form.clone().into_any_element(),
                vec![self.cancel_button(cx), self.save_button("Add Index", cx)],
            ),
            Panel::Index(row) => {
                let index = self.staged.indexes.get(*row);
                let mark = self
                    .staged
                    .index_marks
                    .get(*row)
                    .cloned()
                    .unwrap_or_default();
                let body = match index {
                    Some(index) => details(
                        vec![
                            ("Columns", index.column_names.join(", ")),
                            (
                                "Unique",
                                match (index.is_unique, index.nulls_not_distinct) {
                                    (true, true) => "Yes, NULLs not distinct",
                                    (true, false) => "Yes",
                                    (false, _) => "No",
                                }
                                .to_string(),
                            ),
                            ("Method", index.algorithm.clone()),
                            ("Condition", index.condition.clone().unwrap_or_default()),
                            ("Comment", index.comment.clone().unwrap_or_default()),
                        ],
                        cx,
                    ),
                    None => div().into_any_element(),
                };
                let mut buttons = Vec::new();
                if can_edit {
                    if mark.state == RowState::Unchanged {
                        let row = *row;
                        buttons.push(
                            Button::new("panel-drop-index")
                                .ghost()
                                .small()
                                .label("Drop Index")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.drop_index(row, window, cx)
                                }))
                                .into_any_element(),
                        );
                    } else {
                        buttons.push(self.undo_button(&mark, cx));
                    }
                }
                (
                    format!(
                        "Index {}",
                        index.map(|index| index.name.as_str()).unwrap_or_default()
                    ),
                    Some(mark),
                    body,
                    buttons,
                )
            }
            Panel::NewForeignKey(form) => (
                "New foreign key".to_string(),
                None,
                form.clone().into_any_element(),
                vec![
                    self.cancel_button(cx),
                    self.save_button("Add Foreign Key", cx),
                ],
            ),
            Panel::ForeignKey(row) => {
                let foreign_key = self.staged.foreign_keys.get(*row);
                let mark = self
                    .staged
                    .foreign_key_marks
                    .get(*row)
                    .cloned()
                    .unwrap_or_default();
                let body = match foreign_key {
                    Some(foreign_key) => details(
                        vec![
                            ("Columns", foreign_key.columns.join(", ")),
                            (
                                "References",
                                format!(
                                    "{}{} ({})",
                                    foreign_key
                                        .referenced_schema
                                        .as_deref()
                                        .map(|schema| format!("{schema}."))
                                        .unwrap_or_default(),
                                    foreign_key.referenced_table,
                                    foreign_key.referenced_columns.join(", ")
                                ),
                            ),
                            ("On update", foreign_key.on_update.clone()),
                            ("On delete", foreign_key.on_delete.clone()),
                        ],
                        cx,
                    ),
                    None => div().into_any_element(),
                };
                let mut buttons = Vec::new();
                if can_edit && self.context.dialect().supports_foreign_key_ddl() {
                    if mark.state == RowState::Unchanged {
                        let row = *row;
                        buttons.push(
                            Button::new("panel-drop-foreign-key")
                                .ghost()
                                .small()
                                .label("Drop Foreign Key")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.drop_foreign_key(row, window, cx)
                                }))
                                .into_any_element(),
                        );
                    } else {
                        buttons.push(self.undo_button(&mark, cx));
                    }
                }
                let name = match foreign_key {
                    Some(key) if key.name.is_empty() => "(generated)".to_string(),
                    Some(key) => key.name.clone(),
                    None => String::new(),
                };
                (format!("Foreign key {name}"), Some(mark), body, buttons)
            }
            Panel::VirtualKey { id, form } => {
                let mut buttons = Vec::new();
                if *id != 0 {
                    let id = *id;
                    buttons.push(
                        Button::new("panel-delete-virtual-key")
                            .ghost()
                            .small()
                            .label("Delete")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.delete_virtual_key(id, window, cx)
                            }))
                            .into_any_element(),
                    );
                    buttons.push(div().flex_1().into_any_element());
                } else {
                    buttons.push(self.cancel_button(cx));
                }
                buttons
                    .push(self.save_button(if *id == 0 { "Add Virtual Key" } else { "Save" }, cx));
                let title = match self.virtual_keys.iter().find(|key| key.id == *id) {
                    Some(key) => format!("Virtual key {}", key.name),
                    None => "New virtual key".to_string(),
                };
                (title, None, form.clone().into_any_element(), buttons)
            }
        };

        let theme = cx.theme();
        let badge = mark.and_then(|mark| {
            let (label, color) = match mark.state {
                RowState::Unchanged => return None,
                RowState::Added => ("new", theme.green),
                RowState::Modified => ("changed", theme.yellow),
                RowState::Dropped => ("dropped", theme.red),
            };
            Some(div().text_xs().text_color(color).child(label))
        });
        v_flex()
            .w(px(340.))
            .flex_shrink_0()
            .h_full()
            .border_l_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .px_4()
                    .py_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_semibold()
                            .text_color(theme.foreground)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(title),
                    )
                    .children(badge)
                    .child(
                        Button::new("close-structure-panel")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip("Close")
                            .on_click(cx.listener(|this, _, _, cx| this.close_panel(cx))),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .overflow_y_scrollbar()
                    .child(body),
            )
            .when(!buttons.is_empty(), |this| {
                this.child(
                    h_flex()
                        .px_4()
                        .py_3()
                        .gap_2()
                        .justify_end()
                        .border_t_1()
                        .border_color(theme.border)
                        .children(buttons),
                )
            })
    }

    fn cancel_button(&self, cx: &mut Context<Self>) -> AnyElement {
        Button::new("panel-cancel")
            .outline()
            .small()
            .label("Cancel")
            .on_click(cx.listener(|this, _, _, cx| this.close_panel(cx)))
            .into_any_element()
    }

    fn save_button(&self, label: &'static str, cx: &mut Context<Self>) -> AnyElement {
        Button::new("panel-save")
            .primary()
            .small()
            .label(label)
            .on_click(cx.listener(|this, _, window, cx| this.save_panel(window, cx)))
            .into_any_element()
    }

    /// Undo every pending change on a row: an addition is removed, a drop
    /// or a change is taken back.
    fn undo_button(&self, mark: &RowMark, cx: &mut Context<Self>) -> AnyElement {
        let change_ixs: Vec<usize> = mark.changes.iter().map(|(ix, _)| *ix).collect();
        let added = mark.state == RowState::Added;
        let label = match mark.state {
            RowState::Added => "Remove",
            RowState::Dropped => "Undo Drop",
            RowState::Modified | RowState::Unchanged => "Undo Changes",
        };
        Button::new("panel-undo")
            .outline()
            .small()
            .label(label)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.undo_changes(change_ixs.clone(), window, cx);
                if added {
                    this.close_panel(cx);
                }
            }))
            .into_any_element()
    }

    fn render_pending_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let count = |kind: ChangeKind| {
            self.pending
                .iter()
                .filter(|pending| pending.change.kind() == kind)
                .count()
        };
        let summary = [
            (count(ChangeKind::Add), "+", theme.green),
            (count(ChangeKind::Modify), "~", theme.yellow),
            (count(ChangeKind::Drop), "−", theme.red),
        ];
        let partial_note = (!self.context.dialect().rolls_back_ddl()).then(|| {
            format!(
                "{} cannot roll back DDL, so a failing statement leaves the ones before it applied.",
                self.context.db_type
            )
        });
        let confirms_writes = self.context.confirms_writes();
        let creating = self.is_creating();
        let verb = if creating { "Create Table" } else { "Apply" };
        let apply_label = match self.context.environment_type {
            Some(environment) if confirms_writes => format!("{verb} on {environment}"),
            _ => verb.to_string(),
        };
        // A new table counts what it will have rather than pending changes.
        let headline = if creating {
            let definition = self.new_table_definition();
            let count = |count: usize, one: &str, many: &str| match count {
                1 => format!("1 {one}"),
                count => format!("{count} {many}"),
            };
            let mut parts = vec![count(definition.columns.len(), "column", "columns")];
            if !definition.indexes.is_empty() {
                parts.push(count(definition.indexes.len(), "index", "indexes"));
            }
            if !definition.foreign_keys.is_empty() {
                parts.push(count(
                    definition.foreign_keys.len(),
                    "foreign key",
                    "foreign keys",
                ));
            }
            format!("New table, {}", parts.join(", "))
        } else {
            format!("{} pending", changes_label(self.pending.len()))
        };
        let summary = if creating {
            Vec::new()
        } else {
            summary.to_vec()
        };
        v_flex()
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .px_4()
                    .py_2()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(theme.foreground)
                            .child(headline),
                    )
                    .children(summary.into_iter().filter(|(count, _, _)| *count > 0).map(
                        |(count, sign, color)| {
                            div()
                                .text_sm()
                                .text_color(color)
                                .child(format!("{sign}{count}"))
                        },
                    ))
                    .child(div().flex_1())
                    .child(
                        Button::new("toggle-structure-sql")
                            .ghost()
                            .small()
                            .label(if self.sql_visible {
                                "Hide SQL"
                            } else {
                                "Show SQL"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sql_visible = !this.sql_visible;
                                cx.notify();
                            })),
                    )
                    .when(!creating, |this| {
                        this.child(
                            Button::new("discard-structure-changes")
                                .outline()
                                .small()
                                .label("Discard")
                                .disabled(self.applying)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.discard_changes(window, cx)
                                })),
                        )
                    })
                    .child(
                        Button::new("apply-structure-changes")
                            .map(|button| {
                                if confirms_writes {
                                    button.danger()
                                } else {
                                    button.primary()
                                }
                            })
                            .small()
                            .icon(IconName::Check)
                            .label(apply_label)
                            .loading(self.applying)
                            .disabled(
                                self.applying
                                    || (creating && self.new_table_name(cx).is_empty())
                                    || self.pending.is_empty(),
                            )
                            .on_click(
                                cx.listener(|this, _, window, cx| this.request_apply(window, cx)),
                            ),
                    ),
            )
            .when(self.sql_visible, |this| {
                this.child(
                    v_flex()
                        .px_4()
                        .pb_3()
                        .gap_2()
                        .when_some(partial_note.filter(|_| !creating), |this, note| {
                            this.child(div().text_xs().text_color(theme.warning).child(note))
                        })
                        .child(
                            v_flex()
                                .h(px(160.))
                                .min_h_0()
                                .border_1()
                                .border_color(theme.border)
                                .rounded(theme.radius)
                                .overflow_hidden()
                                .child(self.apply_preview.clone()),
                        ),
                )
            })
    }
}

/// Label and value pairs, as the panel shows a row it cannot change.
fn details(rows: Vec<(&'static str, String)>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    v_flex()
        .gap_3()
        .children(rows.into_iter().map(|(label, value)| {
            v_flex()
                .gap_0p5()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(label),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.foreground)
                        .child(if value.is_empty() {
                            "None".to_string()
                        } else {
                            value
                        }),
                )
        }))
        .into_any_element()
}

impl Render for TableStructureTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = self.render_header(cx).into_any_element();
        let body = if let Some(error) = &self.error {
            div()
                .p_4()
                .text_color(cx.theme().danger_foreground)
                .child(format!("Error: {error}"))
                .into_any_element()
        } else if self.loading && self.columns.is_empty() {
            div()
                .p_4()
                .text_color(cx.theme().muted_foreground)
                .child("Loading…")
                .into_any_element()
        } else {
            let rail = self.render_rail(cx).into_any_element();
            let toolbar = self.render_toolbar(cx).into_any_element();
            let section = self.render_section(cx);
            let panel = self
                .panel
                .as_ref()
                .map(|panel| self.render_panel(panel, cx).into_any_element());
            h_flex()
                .flex_1()
                .min_h_0()
                .items_stretch()
                .child(rail)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(toolbar)
                        .child(section),
                )
                .children(panel)
                .into_any_element()
        };
        let pending_bar = (self.is_creating() || !self.pending.is_empty())
            .then(|| self.render_pending_bar(cx).into_any_element());

        // No background: the editor card owns this surface. A square fill
        // here would cover the card's rounded corners.
        v_flex()
            .size_full()
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .child(header)
            .child(body)
            .children(pending_bar)
    }
}

#[cfg(test)]
#[path = "table_structure_test.rs"]
mod tab_tests;

#[cfg(test)]
mod tests {
    use super::group_digits;

    #[test]
    fn digits_are_grouped_by_thousands() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1000), "1,000");
        assert_eq!(group_digits(12_345_678), "12,345,678");
    }
}
