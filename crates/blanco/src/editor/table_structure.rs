use std::cmp::Ordering;

use gpui::{
    AnyElement, App, AppContext, Context, Div, Entity, FocusHandle, Focusable, FontWeight,
    IntoElement, ParentElement, Render, Styled, Subscription, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme, Disableable as _, Icon, IconName, IndexPath, Sizable as _, StyledExt,
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{PopupMenu, PopupMenuItem},
    notification::NotificationType,
    popover::{Popover, PopoverState},
    scroll::ScrollableElement,
    select::{SearchableVec, Select, SelectEvent, SelectState},
    table::{Column, ColumnSort, DataTable, TableDelegate, TableState},
    v_flex,
};

use crate::result_ext::ResultExt as _;
use crate::status_bar::ActivityReporter;
use blanco_core::ConnectionContext;
use blanco_core::ddl::{
    ColumnOperation, ColumnSpec, ForeignKeyOperation, IndexOperation, ReferentialAction,
};
use blanco_core::{ColumnInfo, ForeignKeyConstraint, IndexInfo};
use blanco_ui::{SqlView, SqlViewMessage};

use self::staging::{PendingChange, RowMark, RowState, StagedStructure, StructureChange};

mod staging;

pub struct TableStructureTab {
    pub title: String,
    pub context: ConnectionContext,
    pub table_name: String,
    /// The structure as the server last reported it.
    columns: Vec<ColumnInfo>,
    indexes: Vec<IndexInfo>,
    foreign_keys: Vec<ForeignKeyConstraint>,
    /// Changes waiting for Apply, in the order they run.
    pending: Vec<PendingChange>,
    /// The structure with the pending changes replayed onto it, which the
    /// lists show and the row actions address.
    staged: StagedStructure,
    applying: bool,
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
    ddl_visible: bool,
    ddl_loaded: bool,
    ddl_loading: bool,
    ddl_error: Option<String>,
    /// Filters the columns grid by name.
    column_search: Entity<InputState>,
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
        let columns_table_state = Self::columns_table(tab.clone(), window, cx);
        let indexes_table_state = Self::indexes_table(tab.clone(), window, cx);
        let foreign_keys_table_state = Self::foreign_keys_table(&context, tab, window, cx);

        let column_search = cx.new(|cx| InputState::new(window, cx).placeholder("Search columns"));
        let search_subscription =
            cx.subscribe(&column_search, |this, search, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let filter = search.read(cx).value().to_string();
                    this.columns_table_state.update(cx, |table, cx| {
                        table.delegate_mut().set_filter(filter);
                        table.clear_selection(cx);
                        cx.notify();
                    });
                }
            });

        let mut tab = Self {
            title,
            context,
            table_name,
            columns,
            indexes,
            foreign_keys: Vec::new(),
            pending: Vec::new(),
            staged: StagedStructure::default(),
            applying: false,
            apply_preview,
            columns_table_state,
            indexes_table_state,
            foreign_keys_table_state,
            focus_handle: cx.focus_handle(),
            loading: false,
            error: None,
            read_only: false,
            ddl_log,
            ddl_visible: false,
            ddl_loaded: false,
            ddl_loading: false,
            ddl_error: None,
            column_search,
            _subscriptions: vec![search_subscription],
        };
        tab.restage(window, cx);
        tab
    }

    fn columns_table(
        tab: WeakEntity<Self>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TableState<ColumnsTableDelegate>> {
        let delegate = ColumnsTableDelegate::new(tab);
        cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .row_selectable(false)
                .cell_selectable(true)
                .col_selectable(false)
        })
    }

    fn indexes_table(
        tab: WeakEntity<Self>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TableState<IndexesTableDelegate>> {
        let delegate = IndexesTableDelegate::new(tab);
        cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .row_selectable(false)
                .cell_selectable(true)
                .col_selectable(false)
        })
    }

    fn foreign_keys_table(
        context: &ConnectionContext,
        tab: WeakEntity<Self>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TableState<ForeignKeysTableDelegate>> {
        let delegate = ForeignKeysTableDelegate::new(context.schema_name.clone(), tab);
        cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .row_selectable(false)
                .cell_selectable(true)
                .col_selectable(false)
        })
    }

    /// Whether the backend has foreign keys to list. ClickHouse has none.
    fn shows_foreign_keys(&self) -> bool {
        self.context.dialect().supports_foreign_keys()
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

    /// Fetch columns, indexes, foreign keys and the read-only flag from the
    /// server and rebuild the grids. Also invalidates the CREATE statement panel so it
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
        let shows_foreign_keys = self.shows_foreign_keys();
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
                let foreign_keys = if shows_foreign_keys {
                    connection
                        .get_foreign_keys_for_table(&table_name, schema_name.as_deref())
                        .await?
                } else {
                    Vec::new()
                };
                Ok((columns, indexes, foreign_keys, read_only))
            })
            .await;

            this.update_in(cx, |this, window, cx| {
                this.loading = false;
                match result {
                    Ok((columns, indexes, foreign_keys, read_only)) => {
                        this.read_only = read_only;
                        this.indexes = order_indexes(indexes, &columns);
                        this.columns = columns;
                        this.foreign_keys = foreign_keys;
                        this.restage(window, cx);
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

    pub(crate) fn has_pending_changes(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Stage `change` after the pending ones. A change the backend cannot
    /// make, or that does not fit the staged structure, is refused here so
    /// Apply never meets it.
    fn stage(&mut self, change: StructureChange, window: &mut Window, cx: &mut Context<Self>) {
        let statements = match change.statements(
            self.context.db_type,
            self.context.schema_name.as_deref(),
            &self.table_name,
        ) {
            Ok(statements) => statements,
            Err(error) => {
                window.push_notification(
                    (
                        NotificationType::Error,
                        format!("{}: {error}", change.label()),
                    ),
                    cx,
                );
                return;
            }
        };
        let replayed = staging::replay(
            &self.columns,
            &self.indexes,
            &self.foreign_keys,
            self.pending
                .iter()
                .map(|pending| &pending.change)
                .chain([&change]),
        );
        let new_ix = self.pending.len();
        if let Some((_, reason)) = replayed.rejected.iter().find(|(ix, _)| *ix == new_ix) {
            window.push_notification(
                (
                    NotificationType::Error,
                    format!("{}: {reason}", change.label()),
                ),
                cx,
            );
            return;
        }
        self.pending.push(PendingChange { change, statements });
        self.restage(window, cx);
    }

    /// Stage a column change as its dialog would.
    #[cfg(feature = "screenshots")]
    pub(crate) fn stage_column(
        &mut self,
        operation: ColumnOperation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stage(StructureChange::Column(operation), window, cx);
    }

    /// Stage an index change as its dialog would.
    #[cfg(feature = "screenshots")]
    pub(crate) fn stage_index(
        &mut self,
        operation: IndexOperation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stage(StructureChange::Index(operation), window, cx);
    }

    /// Drop one pending change. Later changes that depended on it (an index
    /// on a column it added) go with it.
    pub fn undo_change(&mut self, change_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if change_ix < self.pending.len() {
            self.pending.remove(change_ix);
            self.restage(window, cx);
        }
    }

    pub fn discard_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pending.clear();
        self.restage(window, cx);
    }

    /// Replay the pending changes onto the loaded structure and show the
    /// result. Changes that no longer fit, after an undo or a reload, are
    /// dropped and named in a notification.
    fn restage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let replay = |this: &Self| {
            staging::replay(
                &this.columns,
                &this.indexes,
                &this.foreign_keys,
                this.pending.iter().map(|pending| &pending.change),
            )
        };
        let mut replayed = replay(self);
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
            replayed = replay(self);
        }
        self.staged = replayed.structure;
        self.show_staged(cx);
    }

    /// Hand the staged structure to the three lists.
    fn show_staged(&mut self, cx: &mut Context<Self>) {
        let editable = self.can_edit_structure() && !self.applying;
        let staged = &self.staged;
        self.columns_table_state.update(cx, |table, cx| {
            table.delegate_mut().set_rows(
                staged.columns.clone(),
                staged.column_marks.clone(),
                editable,
            );
            table.refresh(cx);
        });
        self.indexes_table_state.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.indexes = staged.indexes.clone();
            delegate.marks = staged.index_marks.clone();
            delegate.editable = editable;
            table.refresh(cx);
        });
        self.foreign_keys_table_state.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.foreign_keys = staged.foreign_keys.clone();
            delegate.marks = staged.foreign_key_marks.clone();
            delegate.editable = editable;
            table.refresh(cx);
        });
        cx.notify();
    }

    fn prod_note(&self) -> String {
        match self.context.environment_type {
            Some(environment) if environment.confirms_writes() => {
                format!(" on a {environment} connection")
            }
            _ => String::new(),
        }
    }

    fn confirm_label(&self, label: &str) -> String {
        match self.context.environment_type {
            Some(environment) if environment.confirms_writes() => {
                format!("{label} on {environment}")
            }
            _ => label.to_string(),
        }
    }

    /// Open the add (`existing == None`) or edit column form. `existing` is
    /// a row of the staged columns.
    pub fn open_column_editor(
        &mut self,
        existing: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing_column = existing
            .and_then(|row| self.staged.columns.get(row))
            .cloned();
        let form = cx.new(|cx| ColumnForm::new(existing_column.as_ref(), window, cx));
        let title = match &existing_column {
            Some(column) => format!("Edit column \"{}\"", column.name),
            None => format!("Add column to \"{}\"", self.table_name),
        };
        let description = "Type and default are used verbatim. The change runs when you apply it.";
        let confirm_label = if existing_column.is_some() {
            "Save"
        } else {
            "Add"
        };
        let this_handle = cx.entity().downgrade();

        window.open_dialog(cx, move |dialog, _, _| {
            let form = form.clone();
            let existing_column = existing_column.clone();
            let this_handle = this_handle.clone();
            dialog
                .title(title.clone())
                .w(px(460.))
                .child(div().text_sm().child(description))
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
                                    .primary()
                                    .label(confirm_label)
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
                                                this.stage(
                                                    StructureChange::Column(operation),
                                                    window,
                                                    cx,
                                                );
                                            })
                                            .log_err();
                                    }),
                            ),
                        ),
                )
        });
    }

    pub fn open_rename_column(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(column) = self.staged.columns.get(row).cloned() else {
            return;
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("New name")
                .default_value(&column.name)
        });
        let title = format!("Rename column \"{}\"", column.name);
        let this_handle = cx.entity().downgrade();

        window.open_dialog(cx, move |dialog, _, _| {
            let input = input.clone();
            let column = column.clone();
            let this_handle = this_handle.clone();
            dialog
                .title(title.clone())
                .child("Enter the new name. The change runs when you apply it.")
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
                                    .primary()
                                    .label("Rename")
                                    .on_click(move |_, window, cx| {
                                        let operation = ColumnOperation::Rename {
                                            from: column.name.clone(),
                                            to: input.read(cx).value().to_string(),
                                        };
                                        this_handle
                                            .update(cx, |this, cx| {
                                                this.stage(
                                                    StructureChange::Column(operation),
                                                    window,
                                                    cx,
                                                );
                                            })
                                            .log_err();
                                    }),
                            ),
                        ),
                )
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

    pub fn open_index_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let column_names = self.staged.live_column_names();
        let form = cx.new(|cx| IndexForm::new(&self.table_name, column_names, window, cx));
        let title = format!("Add index to \"{}\"", self.table_name);
        let description =
            "Columns are indexed in table order. The index is created when you apply it.";
        let this_handle = cx.entity().downgrade();

        window.open_dialog(cx, move |dialog, _, _| {
            let form = form.clone();
            let this_handle = this_handle.clone();
            dialog
                .title(title.clone())
                .w(px(460.))
                .child(div().text_sm().child(description))
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
                                    .primary()
                                    .label("Add")
                                    .on_click(move |_, window, cx| {
                                        let operation = form.read(cx).operation(cx);
                                        this_handle
                                            .update(cx, |this, cx| {
                                                this.stage(
                                                    StructureChange::Index(operation),
                                                    window,
                                                    cx,
                                                );
                                            })
                                            .log_err();
                                    }),
                            ),
                        ),
                )
        });
    }

    pub fn open_foreign_key_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let column_names = self.staged.live_column_names();
        let context = self.context.clone();
        let form = cx.new(|cx| ForeignKeyForm::new(context, column_names, window, cx));
        let title = format!("Add foreign key to \"{}\"", self.table_name);
        let description = "Existing rows must already match the referenced table. The foreign key is created when you apply it.";
        let this_handle = cx.entity().downgrade();

        window.open_dialog(cx, move |dialog, _, _| {
            let form = form.clone();
            let this_handle = this_handle.clone();
            dialog
                .title(title.clone())
                .w(px(520.))
                .child(div().text_sm().child(description))
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("foreign-key-cancel").label("Cancel").outline()),
                        )
                        .child(
                            DialogAction::new().child(
                                Button::new("foreign-key-confirm")
                                    .primary()
                                    .label("Add")
                                    .on_click(move |_, window, cx| {
                                        let operation = form.read(cx).operation(cx);
                                        this_handle
                                            .update(cx, |this, cx| {
                                                this.stage(
                                                    StructureChange::ForeignKey(operation),
                                                    window,
                                                    cx,
                                                );
                                            })
                                            .log_err();
                                    }),
                            ),
                        ),
                )
        });
    }

    /// Fill the preview with every pending statement, each change's under a
    /// comment naming it.
    fn fill_apply_preview(&self, cx: &mut Context<Self>) {
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

    /// Run every pending statement as one batch, in one transaction where
    /// the backend can roll DDL back, then reload the tab and the sidebar's
    /// view of the connection. A failure reloads too: where DDL commits on
    /// its own the server may hold part of the batch, and the reload drops
    /// the changes that no longer apply.
    pub fn apply_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_empty() || self.applying {
            return;
        }
        let operations: Vec<blanco_core::WriteOperation> = self
            .pending
            .iter()
            .flat_map(|pending| &pending.statements)
            .map(|sql| sql.as_str().into())
            .collect();
        let change_count = self.pending.len();
        let db_service = database::DatabaseService::global(cx).clone();
        let connection_id = self.context.connection_id;
        let database_name = self.context.database_name.clone();
        let table_name = self.table_name.clone();
        let activity = ActivityReporter::global(cx).begin(format!(
            "{}: applying structure changes",
            self.context.connection_name
        ));
        self.applying = true;
        self.show_staged(cx);

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
                        let changes = if change_count == 1 {
                            "1 change".to_string()
                        } else {
                            format!("{change_count} changes")
                        };
                        window.push_notification(
                            (
                                NotificationType::Success,
                                format!("Applied {changes} to {table_name}"),
                            ),
                            cx,
                        );
                    }
                    Err(error) => {
                        tracing::error!("Applying changes to {table_name} failed: {error:#}");
                        window.push_notification(
                            (
                                NotificationType::Error,
                                format!("Applying changes to {table_name} failed: {error:#}"),
                            ),
                            cx,
                        );
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

    /// The Apply button and the preview it opens, with Confirm and Cancel.
    fn render_apply_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let button = Button::new("apply-structure-changes")
            .outline()
            .small()
            .icon(IconName::Check)
            .label("Apply Changes")
            .disabled(self.applying);
        if self.applying {
            return button.into_any_element();
        }

        let tab = cx.entity().downgrade();
        let preview = self.apply_preview.clone();
        let is_prod = self.context.confirms_writes();
        let confirm_label = self.confirm_label("Confirm");
        let prod_note = self.prod_note();
        let dialect = self.context.dialect();
        let partial_note = (!dialect.rolls_back_ddl()).then(|| {
            format!(
                "{} cannot roll back DDL, so if a statement fails the ones before it stay applied.",
                self.context.db_type
            )
        });

        Popover::new("apply-structure-changes-popover")
            .trigger(button)
            .on_open_change({
                let tab = tab.clone();
                move |open, _window, cx| {
                    if *open {
                        tab.update(cx, |tab, cx| tab.fill_apply_preview(cx))
                            .log_err();
                    }
                }
            })
            .content(move |_state, _window, cx| {
                let tab = tab.clone();
                let summary = format!("The statements run in this order{prod_note}.");
                v_flex()
                    .p_2()
                    .gap_2()
                    .w(px(520.))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .child("Preview SQL"),
                    )
                    .child(div().text_sm().child(summary))
                    .when_some(partial_note.clone(), |this, note| {
                        this.child(div().text_sm().text_color(cx.theme().warning).child(note))
                    })
                    .child(
                        v_flex()
                            .h(px(280.))
                            .min_h_0()
                            .overflow_hidden()
                            .child(preview.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("apply-structure-cancel")
                                    .outline()
                                    .small()
                                    .label("Cancel")
                                    .on_click(cx.listener(
                                        |state: &mut PopoverState, _, window, cx| {
                                            state.dismiss(window, cx);
                                        },
                                    )),
                            )
                            .child(
                                Button::new("apply-structure-confirm")
                                    .when(is_prod, |this| this.danger())
                                    .when(!is_prod, |this| this.primary())
                                    .small()
                                    .icon(IconName::Check)
                                    .label(confirm_label.clone())
                                    .on_click(cx.listener(
                                        move |state: &mut PopoverState, _, window, cx| {
                                            tab.update(cx, |tab, cx| tab.apply_changes(window, cx))
                                                .log_err();
                                            state.dismiss(window, cx);
                                        },
                                    )),
                            ),
                    )
                    .into_any_element()
            })
            .into_any_element()
    }
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

/// One referencing column and the column it points at.
struct ColumnPair {
    column: Entity<SelectState<Vec<String>>>,
    referenced: Entity<SelectState<Vec<String>>>,
}

/// The add foreign key dialog body. Referenced tables are the ones in the
/// tab's own schema; picking one loads its columns into every pair and
/// prefills them with its primary key.
struct ForeignKeyForm {
    context: ConnectionContext,
    columns: Vec<String>,
    name: Entity<InputState>,
    table: Entity<SelectState<SearchableVec<String>>>,
    referenced_columns: Vec<String>,
    referenced_primary_key: Vec<String>,
    pairs: Vec<ColumnPair>,
    on_update: Entity<SelectState<Vec<String>>>,
    on_delete: Entity<SelectState<Vec<String>>>,
    load_error: Option<String>,
    _subscriptions: Vec<gpui::Subscription>,
}

impl ForeignKeyForm {
    fn new(
        context: ConnectionContext,
        columns: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Generated by the server"));
        let table = cx.new(|cx| {
            SelectState::new(SearchableVec::new(Vec::<String>::new()), None, window, cx)
                .searchable(true)
        });
        let actions: Vec<String> = ReferentialAction::ALL
            .iter()
            .map(|action| action.sql().to_string())
            .collect();
        let on_update =
            cx.new(|cx| SelectState::new(actions.clone(), Some(IndexPath::new(0)), window, cx));
        let on_delete = cx.new(|cx| SelectState::new(actions, Some(IndexPath::new(0)), window, cx));
        let subscription = cx.subscribe_in(
            &table,
            window,
            |this, _, event: &SelectEvent<SearchableVec<String>>, window, cx| {
                let SelectEvent::Confirm(Some(table)) = event else {
                    return;
                };
                this.load_referenced_columns(table.clone(), window, cx);
            },
        );
        let mut form = Self {
            context,
            columns,
            name,
            table,
            referenced_columns: Vec::new(),
            referenced_primary_key: Vec::new(),
            pairs: Vec::new(),
            on_update,
            on_delete,
            load_error: None,
            _subscriptions: vec![subscription],
        };
        form.add_pair(window, cx);
        form.load_tables(window, cx);
        form
    }

    fn add_pair(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let position = self.pairs.len();
        let referenced_ix = self
            .referenced_primary_key
            .get(position)
            .and_then(|key| self.referenced_columns.iter().position(|c| c == key))
            .map(IndexPath::new);
        let columns = self.columns.clone();
        let referenced_columns = self.referenced_columns.clone();
        self.pairs.push(ColumnPair {
            column: cx.new(|cx| SelectState::new(columns, None, window, cx)),
            referenced: cx
                .new(|cx| SelectState::new(referenced_columns, referenced_ix, window, cx)),
        });
        cx.notify();
    }

    fn remove_pair(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.pairs.len() > 1 && ix < self.pairs.len() {
            self.pairs.remove(ix);
            cx.notify();
        }
    }

    fn load_tables(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let db_service = database::DatabaseService::global(cx).clone();
        let connection_id = self.context.connection_id;
        let database_name = self.context.database_name.clone();
        let schema_name = self.context.schema_name.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = gpui_tokio::Tokio::spawn_result(cx, async move {
                let connection = db_service
                    .get_or_create_connection(connection_id, Some(&database_name))
                    .await?;
                connection.get_tables(schema_name.as_deref()).await
            })
            .await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(tables) => this.table.update(cx, |select, cx| {
                        select.set_items(SearchableVec::new(tables), window, cx)
                    }),
                    Err(error) => this.load_error = Some(format!("{error:#}")),
                }
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }

    fn load_referenced_columns(
        &mut self,
        table: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let db_service = database::DatabaseService::global(cx).clone();
        let connection_id = self.context.connection_id;
        let database_name = self.context.database_name.clone();
        let schema_name = self.context.schema_name.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = gpui_tokio::Tokio::spawn_result(cx, async move {
                let connection = db_service
                    .get_or_create_connection(connection_id, Some(&database_name))
                    .await?;
                connection
                    .get_columns_for_table(&table, schema_name.as_deref())
                    .await
            })
            .await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(columns) => {
                        this.load_error = None;
                        this.referenced_primary_key = columns
                            .iter()
                            .filter(|column| column.is_primary_key)
                            .map(|column| column.name.clone())
                            .collect();
                        this.referenced_columns =
                            columns.into_iter().map(|column| column.name).collect();
                        // A different table invalidates every earlier pick, so
                        // reset the pairs to its primary key.
                        for (position, pair) in this.pairs.iter().enumerate() {
                            let ix = this
                                .referenced_primary_key
                                .get(position)
                                .and_then(|key| {
                                    this.referenced_columns.iter().position(|c| c == key)
                                })
                                .map(IndexPath::new);
                            let items = this.referenced_columns.clone();
                            pair.referenced.update(cx, |select, cx| {
                                select.set_items(items, window, cx);
                                select.set_selected_index(ix, window, cx);
                            });
                        }
                    }
                    Err(error) => this.load_error = Some(format!("{error:#}")),
                }
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }

    fn operation(&self, cx: &App) -> ForeignKeyOperation {
        let selected = |select: &Entity<SelectState<Vec<String>>>| {
            select
                .read(cx)
                .selected_value()
                .cloned()
                .unwrap_or_default()
        };
        let action = |select: &Entity<SelectState<Vec<String>>>| {
            let selected = selected(select);
            ReferentialAction::ALL
                .into_iter()
                .find(|action| action.sql() == selected)
                .unwrap_or_default()
        };
        let name = self.name.read(cx).value().trim().to_string();
        ForeignKeyOperation::Create {
            name: (!name.is_empty()).then_some(name),
            columns: self
                .pairs
                .iter()
                .map(|pair| selected(&pair.column))
                .collect(),
            referenced_schema: None,
            referenced_table: self
                .table
                .read(cx)
                .selected_value()
                .cloned()
                .unwrap_or_default(),
            referenced_columns: self
                .pairs
                .iter()
                .map(|pair| selected(&pair.referenced))
                .collect(),
            on_update: action(&self.on_update),
            on_delete: action(&self.on_delete),
        }
    }
}

impl Render for ForeignKeyForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let can_remove = self.pairs.len() > 1;
        v_flex()
            .gap_3()
            .py_2()
            .child(labeled("Name", Input::new(&self.name)))
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().child("Referenced Table"))
                    .child(Select::new(&self.table).placeholder("Choose a table")),
            )
            .when_some(self.load_error.clone(), |this, error| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().danger_foreground)
                        .child(error),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().text_sm().child("Column"))
                    .child(div().flex_1().text_sm().child("Referenced Column"))
                    .when(can_remove, |this| this.child(div().w(px(24.)))),
            )
            .child(
                v_flex()
                    .gap_2()
                    .children(self.pairs.iter().enumerate().map(|(ix, pair)| {
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .child(Select::new(&pair.column).placeholder("Column")),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .child(Select::new(&pair.referenced).placeholder("Column")),
                            )
                            .when(can_remove, |this| {
                                this.child(
                                    Button::new(("foreign-key-remove-pair", ix))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Close)
                                        .tooltip("Remove Column")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.remove_pair(ix, cx);
                                        })),
                                )
                            })
                    })),
            )
            .child(
                h_flex().child(
                    Button::new("foreign-key-add-pair")
                        .ghost()
                        .small()
                        .icon(IconName::Plus)
                        .label("Add Column")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.add_pair(window, cx);
                        })),
                ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .child(div().text_sm().child("On Update"))
                            .child(Select::new(&self.on_update)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .child(div().text_sm().child("On Delete"))
                            .child(Select::new(&self.on_delete)),
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
        let apply_button = self.render_apply_button(cx);
        let theme = cx.theme();
        let can_edit = self.can_edit_structure() && !self.applying;

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
                                        .when(
                                            self.shows_foreign_keys(),
                                            |this| {
                                                this.child(
                                                    Button::new("add-foreign-key")
                                                        .outline()
                                                        .small()
                                                        .label("Add Foreign Key")
                                                        .on_click(cx.listener(
                                                            |this, _, window, cx| {
                                                                this.open_foreign_key_editor(
                                                                    window, cx,
                                                                );
                                                            },
                                                        )),
                                                )
                                            },
                                        )
                                    })
                                    .when(self.read_only, |this| {
                                        this.child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child("Read-only connection"),
                                        )
                                    })
                                    .when(!self.pending.is_empty(), |this| {
                                        let count = match self.pending.len() {
                                            1 => "1 pending change".to_string(),
                                            count => format!("{count} pending changes"),
                                        };
                                        this.child(div().flex_1())
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(theme.muted_foreground)
                                                    .child(count),
                                            )
                                            .child(apply_button)
                                            .child(
                                                Button::new("discard-structure-changes")
                                                    .outline()
                                                    .small()
                                                    .icon(IconName::CircleX)
                                                    .label("Discard Changes")
                                                    .disabled(self.applying)
                                                    .on_click(cx.listener(
                                                        |this, _, window, cx| {
                                                            this.discard_changes(window, cx);
                                                        },
                                                    )),
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
                                        h_flex()
                                            .justify_between()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_lg()
                                                    .font_semibold()
                                                    .text_color(theme.foreground)
                                                    .child("Columns"),
                                            )
                                            .child(
                                                Input::new(&self.column_search)
                                                    .small()
                                                    .w(px(220.))
                                                    .cleanable(true)
                                                    .prefix(
                                                        Icon::new(IconName::Search)
                                                            .small()
                                                            .text_color(theme.muted_foreground),
                                                    ),
                                            ),
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
                            .when(self.shows_foreign_keys(), |this| {
                                this.child(
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
                                                .child("Foreign Keys"),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .border_1()
                                                .border_color(theme.border)
                                                .overflow_hidden()
                                                .child(
                                                    DataTable::new(&self.foreign_keys_table_state)
                                                        .bordered(false),
                                                ),
                                        ),
                                )
                            })
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
    marks: Vec<RowMark>,
    /// Indices into `columns` in display order, after the search and sort.
    rows: Vec<usize>,
    filter: String,
    sort: Option<(usize, ColumnSort)>,
    column_defs: Vec<Column>,
    editable: bool,
    tab: WeakEntity<TableStructureTab>,
}

impl ColumnsTableDelegate {
    pub fn new(tab: WeakEntity<TableStructureTab>) -> Self {
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
            Column {
                name: "Comment".into(),
                width: px(200.),
                ..Default::default()
            },
        ];

        let column_defs = column_defs.into_iter().map(Column::sortable).collect();
        Self {
            columns: Vec::new(),
            marks: Vec::new(),
            rows: Vec::new(),
            filter: String::new(),
            sort: None,
            column_defs,
            editable: false,
            tab,
        }
    }

    fn set_rows(&mut self, columns: Vec<ColumnInfo>, marks: Vec<RowMark>, editable: bool) {
        self.columns = columns;
        self.marks = marks;
        self.editable = editable;
        self.rows = column_rows(&self.columns, &self.filter, self.sort);
    }

    fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        self.rows = column_rows(&self.columns, &self.filter, self.sort);
    }

    fn set_sort(&mut self, col_ix: usize, sort: ColumnSort) {
        // The table asks for column definitions again on refresh, so they
        // carry the sort state.
        for (ix, def) in self.column_defs.iter_mut().enumerate() {
            *def = def.clone().sort(if ix == col_ix {
                sort
            } else {
                ColumnSort::Default
            });
        }
        self.sort = (sort != ColumnSort::Default).then_some((col_ix, sort));
        self.rows = column_rows(&self.columns, &self.filter, self.sort);
    }
}

/// The text a cell of the columns grid shows. `ordinal` is the column's
/// position in the table, which the `#` column keeps under any sort.
fn column_cell(col: &ColumnInfo, ordinal: usize, col_ix: usize) -> String {
    match col_ix {
        0 => (ordinal + 1).to_string(),
        1 => col.name.clone(),
        2 => col.display_type(),
        3 => if col.is_nullable { "Yes" } else { "No" }.to_string(),
        4 => col.default_value.clone().unwrap_or_default(),
        5 => col
            .foreign_key
            .as_ref()
            .map(|fk| format!("{}({})", fk.foreign_table_name, fk.foreign_column_name))
            .unwrap_or_default(),
        6 => col.comment.clone().unwrap_or_default(),
        _ => String::new(),
    }
}

/// The columns grid's rows: indices into `columns` whose name matches
/// `filter` (the sidebar's syntax: `^`, `$` and `|`, ignoring case), ordered
/// by `sort`. Without a sort, or between equal values, rows keep the table's
/// column order.
fn column_rows(
    columns: &[ColumnInfo],
    filter: &str,
    sort: Option<(usize, ColumnSort)>,
) -> Vec<usize> {
    let filter = crate::name_filter::NameFilter::parse(filter);
    let mut rows: Vec<usize> = (0..columns.len())
        .filter(|&ix| filter.is_empty() || filter.matches(&columns[ix].name))
        .collect();
    let Some((col_ix, sort)) = sort else {
        return rows;
    };
    let compare = |a: &usize, b: &usize| match col_ix {
        0 => a.cmp(b),
        _ => compare_text(
            &column_cell(&columns[*a], *a, col_ix),
            &column_cell(&columns[*b], *b, col_ix),
        ),
    };
    match sort {
        ColumnSort::Ascending => rows.sort_by(compare),
        ColumnSort::Descending => rows.sort_by(|a, b| compare(b, a)),
        ColumnSort::Default => {}
    }
    rows
}

/// Case-insensitive, falling back to the exact text so the order is total.
fn compare_text(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// The indexes grid's order: the primary key first, then the rest by name.
/// Backends return indexes in whatever order their catalog yields, which can
/// change between reloads.
fn order_indexes(mut indexes: Vec<IndexInfo>, columns: &[ColumnInfo]) -> Vec<IndexInfo> {
    let mut primary_key: Vec<&str> = columns
        .iter()
        .filter(|col| col.is_primary_key)
        .map(|col| col.name.as_str())
        .collect();
    primary_key.sort_unstable();
    // Named like a key by its backend (MySQL `PRIMARY`, Postgres `<table>_pkey`,
    // SQL Server `PK__…`) ranks before a unique index that merely covers the
    // key's columns, so a duplicate unique index never takes the key's place.
    let rank = |index: &IndexInfo| {
        let name = index.name.to_ascii_lowercase();
        let covers_key = index.is_unique && !primary_key.is_empty() && {
            let mut names: Vec<&str> = index.column_names.iter().map(String::as_str).collect();
            names.sort_unstable();
            names == primary_key
        };
        if name == "primary" || (covers_key && (name.ends_with("_pkey") || name.starts_with("pk_")))
        {
            0
        } else if covers_key {
            1
        } else {
            2
        }
    };
    indexes
        .sort_by_cached_key(|index| (rank(index), index.name.to_lowercase(), index.name.clone()));
    indexes
}

impl TableDelegate for ColumnsTableDelegate {
    fn context_menu(
        &mut self,
        (row_ix, _): (usize, usize),
        menu: PopupMenu,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        if !self.editable {
            return menu;
        }
        // Actions address the tab's column list, not the filtered and sorted
        // row the menu was opened on.
        let Some(&column_ix) = self.rows.get(row_ix) else {
            return menu;
        };
        let mark = self.marks.get(column_ix).cloned().unwrap_or_default();
        if matches!(mark.state, RowState::Added | RowState::Dropped) {
            return undo_items(menu, &mark, &self.tab);
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
                tab.update(cx, |tab, cx| action(tab, column_ix, window, cx))
                    .log_err();
            })
        };
        let menu = menu
            .item(item("Edit Column", |tab, row, window, cx| {
                tab.open_column_editor(Some(row), window, cx)
            }))
            .item(item("Rename Column", TableStructureTab::open_rename_column))
            .separator()
            .item(item("Drop Column", TableStructureTab::drop_column));
        if mark.changes.is_empty() {
            menu
        } else {
            undo_items(menu.separator(), &mark, &self.tab)
        }
    }

    fn columns_count(&self, _: &App) -> usize {
        self.column_defs.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) {
        self.set_sort(col_ix, sort);
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
        let value = self
            .rows
            .get(row_ix)
            .map(|&ix| column_cell(&self.columns[ix], ix, col_ix))
            .unwrap_or_default();
        let state = self
            .rows
            .get(row_ix)
            .and_then(|&ix| self.marks.get(ix))
            .map(|mark| mark.state)
            .unwrap_or_default();

        staged_cell(value, state, cx)
    }
}

#[derive(Clone)]
pub struct IndexesTableDelegate {
    indexes: Vec<IndexInfo>,
    marks: Vec<RowMark>,
    column_defs: Vec<Column>,
    editable: bool,
    tab: WeakEntity<TableStructureTab>,
}

impl IndexesTableDelegate {
    pub fn new(tab: WeakEntity<TableStructureTab>) -> Self {
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
            indexes: Vec::new(),
            marks: Vec::new(),
            column_defs,
            editable: false,
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
        let mark = self.marks.get(row_ix).cloned().unwrap_or_default();
        if mark.state != RowState::Unchanged {
            return undo_items(menu, &mark, &self.tab);
        }
        let tab = self.tab.clone();
        menu.item(
            PopupMenuItem::new("Drop Index").on_click(move |_, window, cx| {
                tab.update(cx, |tab, cx| tab.drop_index(row_ix, window, cx))
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
        let Some(idx) = self.indexes.get(row_ix) else {
            return staged_cell(String::new(), RowState::Unchanged, cx);
        };
        let value = match col_ix {
            0 => idx.name.clone(),
            1 => idx.algorithm.clone(),
            2 => if idx.is_unique { "Yes" } else { "No" }.to_string(),
            3 => idx.column_names.join(", "),
            4 => idx.condition.clone().unwrap_or_default(),
            5 => idx.comment.clone().unwrap_or_default(),
            _ => String::new(),
        };
        let state = self
            .marks
            .get(row_ix)
            .map(|mark| mark.state)
            .unwrap_or_default();

        staged_cell(value, state, cx)
    }
}

#[derive(Clone)]
pub struct ForeignKeysTableDelegate {
    foreign_keys: Vec<ForeignKeyConstraint>,
    marks: Vec<RowMark>,
    /// The tab's schema; references into it are shown without a prefix.
    schema: Option<String>,
    column_defs: Vec<Column>,
    editable: bool,
    tab: WeakEntity<TableStructureTab>,
}

impl ForeignKeysTableDelegate {
    pub fn new(schema: Option<String>, tab: WeakEntity<TableStructureTab>) -> Self {
        let column = |name: &'static str, width: f32| Column {
            name: name.into(),
            width: px(width),
            ..Default::default()
        };
        Self {
            foreign_keys: Vec::new(),
            marks: Vec::new(),
            schema,
            column_defs: vec![
                column("Name", 200.),
                column("Columns", 150.),
                column("References", 220.),
                column("On Update", 100.),
                column("On Delete", 100.),
            ],
            editable: false,
            tab,
        }
    }

    fn references(&self, foreign_key: &ForeignKeyConstraint) -> String {
        let table = match foreign_key.referenced_schema.as_deref() {
            Some(schema) if Some(schema) != self.schema.as_deref() => {
                format!("{schema}.{}", foreign_key.referenced_table)
            }
            _ => foreign_key.referenced_table.clone(),
        };
        // SQLite leaves the column out when the reference names only the
        // table's primary key.
        if foreign_key.referenced_columns.iter().all(String::is_empty) {
            table
        } else {
            format!("{table}({})", foreign_key.referenced_columns.join(", "))
        }
    }
}

impl TableDelegate for ForeignKeysTableDelegate {
    fn context_menu(
        &mut self,
        (row_ix, _): (usize, usize),
        menu: PopupMenu,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        if !self.editable || row_ix >= self.foreign_keys.len() {
            return menu;
        }
        let mark = self.marks.get(row_ix).cloned().unwrap_or_default();
        if mark.state != RowState::Unchanged {
            return undo_items(menu, &mark, &self.tab);
        }
        let tab = self.tab.clone();
        menu.item(
            PopupMenuItem::new("Drop Foreign Key").on_click(move |_, window, cx| {
                tab.update(cx, |tab, cx| tab.drop_foreign_key(row_ix, window, cx))
                    .log_err();
            }),
        )
    }

    fn columns_count(&self, _: &App) -> usize {
        self.column_defs.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.foreign_keys.len()
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
        let Some(foreign_key) = self.foreign_keys.get(row_ix) else {
            return staged_cell(String::new(), RowState::Unchanged, cx);
        };
        let state = self
            .marks
            .get(row_ix)
            .map(|mark| mark.state)
            .unwrap_or_default();
        let value = match col_ix {
            // A new key without a name gets one from the server.
            0 if foreign_key.name.is_empty() && state == RowState::Added => {
                "(generated)".to_string()
            }
            0 => foreign_key.name.clone(),
            1 => foreign_key.columns.join(", "),
            2 => self.references(foreign_key),
            3 => foreign_key.on_update.clone(),
            4 => foreign_key.on_delete.clone(),
            _ => String::new(),
        };

        staged_cell(value, state, cx)
    }
}

/// One cell of a structure list, tinted like the results grid tints pending
/// edits: green for an addition, yellow for a change, red and struck
/// through for a drop.
fn staged_cell(
    value: String,
    state: RowState,
    cx: &Context<TableState<impl TableDelegate>>,
) -> Div {
    let theme = cx.theme();
    div()
        .h_full()
        .flex()
        .items_center()
        .px_2()
        .font_family(theme.mono_font_family.clone())
        .text_size(px(12.))
        .text_color(theme.foreground)
        .map(|cell| match state {
            RowState::Unchanged => cell,
            RowState::Added => cell.text_color(theme.green).bg(theme.green.opacity(0.15)),
            RowState::Modified => cell.bg(theme.yellow.opacity(0.3)),
            RowState::Dropped => cell
                .line_through()
                .text_color(theme.red)
                .bg(theme.red.opacity(0.15)),
        })
        .child(value)
}

/// An Undo item for each pending change on a row.
fn undo_items(menu: PopupMenu, mark: &RowMark, tab: &WeakEntity<TableStructureTab>) -> PopupMenu {
    mark.changes
        .iter()
        .fold(menu, |menu, (change_ix, description)| {
            let tab = tab.clone();
            let change_ix = *change_ix;
            menu.item(PopupMenuItem::new(format!("Undo {description}")).on_click(
                move |_, window, cx| {
                    tab.update(cx, |tab, cx| tab.undo_change(change_ix, window, cx))
                        .log_err();
                },
            ))
        })
}

#[cfg(test)]
#[path = "table_structure_test.rs"]
mod foreign_key_tests;

#[cfg(test)]
mod tests {
    use super::{column_rows, order_indexes};
    use blanco_core::{ColumnInfo, IndexInfo};
    use gpui_component::table::ColumnSort;

    fn column(name: &str, data_type: &str, is_primary_key: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.to_string(),
            data_type: data_type.to_string(),
            is_nullable: !is_primary_key,
            is_primary_key,
            default_value: None,
            character_maximum_length: None,
            foreign_key: None,
            comment: None,
            numeric_precision: None,
            numeric_scale: None,
        }
    }

    fn index(name: &str, is_unique: bool, columns: &[&str]) -> IndexInfo {
        IndexInfo {
            name: name.to_string(),
            algorithm: "btree".to_string(),
            is_unique,
            column_names: columns.iter().map(|c| c.to_string()).collect(),
            condition: None,
            comment: None,
        }
    }

    fn columns() -> Vec<ColumnInfo> {
        vec![
            column("id", "integer", true),
            column("Email", "text", false),
            column("created_at", "timestamp", false),
            column("email_verified", "boolean", false),
        ]
    }

    #[test]
    fn column_rows_sort_and_restore_table_order() {
        let columns = columns();
        assert_eq!(column_rows(&columns, "", None), vec![0, 1, 2, 3]);
        // Name ignores case.
        assert_eq!(
            column_rows(&columns, "", Some((1, ColumnSort::Ascending))),
            vec![2, 1, 3, 0]
        );
        assert_eq!(
            column_rows(&columns, "", Some((1, ColumnSort::Descending))),
            vec![0, 3, 1, 2]
        );
        // Type.
        assert_eq!(
            column_rows(&columns, "", Some((2, ColumnSort::Ascending))),
            vec![3, 0, 1, 2]
        );
        // The `#` column sorts by position, not as text.
        let many: Vec<ColumnInfo> = (0..12)
            .map(|ix| column(&format!("c{ix}"), "text", false))
            .collect();
        assert_eq!(
            column_rows(&many, "", Some((0, ColumnSort::Descending)))[..3],
            [11, 10, 9]
        );
        assert_eq!(
            column_rows(&columns, "", Some((1, ColumnSort::Default))),
            vec![0, 1, 2, 3]
        );
    }

    #[test]
    fn column_rows_filter_names_ignoring_case() {
        let columns = columns();
        assert_eq!(column_rows(&columns, "EMAIL", None), vec![1, 3]);
        assert_eq!(column_rows(&columns, "  at ", None), vec![2]);
        assert_eq!(
            column_rows(&columns, "email", Some((1, ColumnSort::Descending))),
            vec![3, 1]
        );
        assert!(column_rows(&columns, "missing", None).is_empty());
        // The sidebar filter's anchors and alternatives.
        assert_eq!(column_rows(&columns, "^email$", None), vec![1]);
        assert_eq!(column_rows(&columns, "^id | _at$", None), vec![0, 2]);
    }

    #[test]
    fn order_indexes_puts_primary_key_first() {
        let names = |indexes: Vec<IndexInfo>| {
            indexes
                .into_iter()
                .map(|index| index.name)
                .collect::<Vec<_>>()
        };
        let columns = columns();

        // Postgres: the key is named `<table>_pkey`; a unique index over the
        // same column follows it, and the rest go by name.
        let indexes = vec![
            index("users_email_key", true, &["Email"]),
            index("a_id_unique", true, &["id"]),
            index("users_pkey", true, &["id"]),
            index("Created_idx", false, &["created_at"]),
        ];
        assert_eq!(
            names(order_indexes(indexes, &columns)),
            [
                "users_pkey",
                "a_id_unique",
                "Created_idx",
                "users_email_key"
            ]
        );

        // MySQL names the key PRIMARY.
        let indexes = vec![
            index("email", true, &["Email"]),
            index("PRIMARY", true, &["id"]),
        ];
        assert_eq!(
            names(order_indexes(indexes, &columns)),
            ["PRIMARY", "email"]
        );

        // SQLite's key index has a generated name; it is found by its columns.
        let indexes = vec![
            index("idx_email", false, &["Email"]),
            index("sqlite_autoindex_users_1", true, &["id"]),
        ];
        assert_eq!(
            names(order_indexes(indexes, &columns)),
            ["sqlite_autoindex_users_1", "idx_email"]
        );
    }
}
