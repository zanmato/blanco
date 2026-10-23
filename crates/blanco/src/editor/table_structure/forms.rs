//! The forms the structure tab's side panel shows: a column, an index or a
//! foreign key, new or as it is staged.

use std::rc::Rc;

use anyhow::Result;
use app_database::VirtualForeignKey;
use blanco_core::ddl::{
    ColumnOperation, ColumnPosition, ColumnSpec, ForeignKeyOperation, IndexOperation, IndexSpec,
    ReferentialAction,
};
use blanco_core::{ConnectionContext, Dialect};
use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled, Task,
    Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, IconName, IndexPath, Sizable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{CompletionProvider, Editor, EditorState, Input, InputState, Rope, RopeExt as _},
    select::{SearchableVec, Select, SelectEvent, SelectState},
    v_flex,
};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    InsertTextFormat, Range, TextEdit,
};

use crate::result_ext::ResultExt as _;

/// The form for a new column or for one the table has.
pub(crate) struct ColumnForm {
    dialect: Dialect,
    /// The column as listed, `None` for a new one.
    original: Option<ColumnSpec>,
    name: Entity<InputState>,
    data_type: Entity<EditorState>,
    default: Entity<InputState>,
    /// Write the default as a quoted string rather than an expression.
    default_is_text: bool,
    comment: Entity<InputState>,
    nullable: bool,
    /// Offered only for a table being created. An existing table's key is
    /// changed in a query.
    offers_primary_key: bool,
    primary_key: bool,
    /// Where a new column goes, offered where the backend can place it: at
    /// the end, first, or after one of `position_columns`.
    position: Option<Entity<SelectState<Vec<String>>>>,
    position_columns: Vec<String>,
}

impl ColumnForm {
    /// `positions` lists the columns a new column may follow, `None` where
    /// it can only be added after them all.
    pub(crate) fn new(
        dialect: Dialect,
        original: Option<ColumnSpec>,
        user_types: Vec<String>,
        offers_primary_key: bool,
        positions: Option<Vec<String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut text = |value: Option<&str>, placeholder: &'static str, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let state = InputState::new(window, cx).placeholder(placeholder);
                match value {
                    Some(value) => state.default_value(value),
                    None => state,
                }
            })
        };
        let name = text(
            original.as_ref().map(|spec| spec.name.as_str()),
            "Column name",
            cx,
        );
        let default = text(
            original.as_ref().and_then(|spec| spec.default.as_deref()),
            "None",
            cx,
        );
        let comment = text(
            original.as_ref().and_then(|spec| spec.comment.as_deref()),
            "None",
            cx,
        );
        let data_type = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx)
                .single_line(true)
                .line_number(false)
                .folding(false)
                .placeholder("Type, e.g. integer or varchar(120)");
            if let Some(spec) = &original {
                editor = editor.default_value(spec.data_type.clone());
            }
            editor.lsp_mut().completion_provider = Some(Rc::new(DataTypeCompletionProvider::new(
                dialect, user_types,
            )));
            editor
        });
        let position_columns = positions.clone().unwrap_or_default();
        let position = positions.map(|columns| {
            let labels = ["At the end".to_string(), "First".to_string()]
                .into_iter()
                .chain(columns.iter().map(|column| format!("After {column}")))
                .collect::<Vec<_>>();
            cx.new(|cx| SelectState::new(labels, Some(IndexPath::new(0)), window, cx))
        });
        Self {
            dialect,
            position,
            position_columns,
            nullable: original.as_ref().is_none_or(|spec| spec.nullable),
            offers_primary_key,
            primary_key: original.as_ref().is_some_and(|spec| spec.primary_key),
            original,
            name,
            data_type,
            default,
            default_is_text: false,
            comment,
        }
    }

    /// The field that takes the focus when the form opens.
    pub(crate) fn first_input(&self) -> Entity<InputState> {
        self.name.clone()
    }

    pub(crate) fn spec(&self, cx: &App) -> ColumnSpec {
        let optional = |input: &Entity<InputState>| {
            let value = input.read(cx).value().trim().to_string();
            (!value.is_empty()).then_some(value)
        };
        let default = if self.default_is_text {
            Some(
                self.dialect
                    .string_literal(self.default.read(cx).value().as_ref()),
            )
        } else {
            optional(&self.default)
        };
        ColumnSpec {
            name: self.name.read(cx).value().trim().to_string(),
            data_type: self.data_type.read(cx).value().trim().to_string(),
            nullable: self.nullable,
            default,
            comment: optional(&self.comment),
            // Not edited here, but repeated when the column is restated.
            attributes: self
                .original
                .as_ref()
                .and_then(|original| original.attributes.clone()),
            generated: self
                .original
                .as_ref()
                .is_some_and(|original| original.generated),
            primary_key: if self.offers_primary_key {
                self.primary_key
            } else {
                self.original
                    .as_ref()
                    .is_some_and(|original| original.primary_key)
            },
        }
    }

    /// Where the new column goes, `None` for after the others.
    fn position(&self, cx: &App) -> Option<ColumnPosition> {
        let row = self.position.as_ref()?.read(cx).selected_index(cx)?.row;
        match row {
            0 => None,
            1 => Some(ColumnPosition::First),
            row => self
                .position_columns
                .get(row - 2)
                .map(|column| ColumnPosition::After(column.clone())),
        }
    }

    /// The change the form describes, `None` when it changes nothing. A
    /// change of the name alone is a rename, which every backend can make.
    pub(crate) fn operation(&self, cx: &App) -> Option<ColumnOperation> {
        let spec = self.spec(cx);
        match &self.original {
            None => Some(ColumnOperation::Add {
                column: spec,
                position: self.position(cx),
            }),
            Some(original) if *original == spec => None,
            Some(original)
                if ColumnSpec {
                    name: spec.name.clone(),
                    ..original.clone()
                } == spec =>
            {
                Some(ColumnOperation::Rename {
                    from: original.name.clone(),
                    to: spec.name,
                })
            }
            Some(original) => Some(ColumnOperation::Alter {
                from: original.clone(),
                to: spec,
            }),
        }
    }
}

impl Render for ColumnForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .gap_3()
            .child(labeled("Name", Input::new(&self.name).small()))
            .child(
                v_flex().gap_1().child(field_label("Type")).child(
                    // Drawn like the small inputs around it: the editor
                    // brings no frame of its own, and keeps a 6px margin
                    // before its text, which the left padding makes up for.
                    div()
                        .w_full()
                        .h_6()
                        .flex()
                        .items_center()
                        .border_1()
                        .border_color(theme.input)
                        .rounded(theme.radius)
                        .bg(theme.input_background())
                        .pl(px(2.))
                        .pr_2()
                        .text_sm()
                        .child(
                            Editor::new(&self.data_type)
                                .appearance(false)
                                .bordered(false),
                        ),
                ),
            )
            .when(self.offers_primary_key, |this| {
                this.child(
                    Checkbox::new("column-primary-key")
                        .label("Primary key")
                        .checked(self.primary_key)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.primary_key = *checked;
                            // A key column takes no NULLs.
                            if *checked {
                                this.nullable = false;
                            }
                            cx.notify();
                        })),
                )
            })
            .child(
                Checkbox::new("column-nullable")
                    .label("Nullable")
                    .checked(self.nullable)
                    .disabled(self.offers_primary_key && self.primary_key)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.nullable = *checked;
                        cx.notify();
                    })),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(labeled("Default", Input::new(&self.default).small()))
                    .child(
                        Checkbox::new("column-default-text")
                            .label("Write the default as text")
                            .checked(self.default_is_text)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.default_is_text = *checked;
                                cx.notify();
                            })),
                    ),
            )
            .when(self.dialect.supports_comments(), |this| {
                this.child(labeled("Comment", Input::new(&self.comment).small()))
            })
            .when_some(self.position.clone(), |this, position| {
                this.child(labeled("Position", Select::new(&position).small()))
            })
    }
}

/// The form for a new index.
pub(crate) struct IndexForm {
    dialect: Dialect,
    /// The server's `server_version_num`, which decides whether NULLS NOT
    /// DISTINCT is offered.
    server_version_num: Option<u32>,
    name: Entity<InputState>,
    unique: bool,
    nulls_not_distinct: bool,
    /// The table's columns, in table order, and whether each is indexed.
    columns: Vec<(String, bool)>,
    method: Option<Entity<SelectState<Vec<String>>>>,
    condition: Entity<InputState>,
}

impl IndexForm {
    pub(crate) fn new(
        dialect: Dialect,
        server_version_num: Option<u32>,
        table_name: &str,
        columns: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Index name")
                .default_value(format!("{table_name}_idx"))
        });
        let methods: Vec<String> = dialect
            .index_methods()
            .iter()
            .map(|method| method.to_string())
            .collect();
        let method = (!methods.is_empty())
            .then(|| cx.new(|cx| SelectState::new(methods, Some(IndexPath::new(0)), window, cx)));
        let condition = cx.new(|cx| InputState::new(window, cx).placeholder("None"));
        Self {
            dialect,
            server_version_num,
            name,
            unique: false,
            nulls_not_distinct: false,
            columns: columns.into_iter().map(|column| (column, false)).collect(),
            method,
            condition,
        }
    }

    /// The field that takes the focus when the form opens.
    pub(crate) fn first_input(&self) -> Entity<InputState> {
        self.name.clone()
    }

    fn offers_nulls_not_distinct(&self) -> bool {
        self.dialect
            .supports_nulls_not_distinct(self.server_version_num)
    }

    pub(crate) fn operation(&self, cx: &App) -> IndexOperation {
        let condition = self.condition.read(cx).value().trim().to_string();
        // The first method is the backend's default, which needs no clause.
        let method = self.method.as_ref().and_then(|select| {
            let select = select.read(cx);
            (select.selected_index(cx).is_some_and(|ix| ix.row > 0))
                .then(|| select.selected_value().cloned())
                .flatten()
        });
        IndexOperation::Create(IndexSpec {
            name: self.name.read(cx).value().trim().to_string(),
            columns: self
                .columns
                .iter()
                .filter(|(_, selected)| *selected)
                .map(|(name, _)| name.clone())
                .collect(),
            unique: self.unique,
            nulls_not_distinct: self.unique && self.nulls_not_distinct,
            method,
            condition: (!condition.is_empty()).then_some(condition),
        })
    }
}

impl Render for IndexForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let offers_nulls = self.offers_nulls_not_distinct();
        v_flex()
            .gap_3()
            .child(labeled("Name", Input::new(&self.name).small()))
            .child(
                v_flex()
                    .gap_1()
                    .child(field_label("Columns, in table order"))
                    .child(
                        v_flex()
                            .gap_1()
                            .children(self.columns.iter().enumerate().map(
                                |(ix, (name, selected))| {
                                    Checkbox::new(("index-column", ix))
                                        .label(name.clone())
                                        .checked(*selected)
                                        .on_click(cx.listener(
                                            move |this, checked: &bool, _, cx| {
                                                if let Some(column) = this.columns.get_mut(ix) {
                                                    column.1 = *checked;
                                                }
                                                cx.notify();
                                            },
                                        ))
                                },
                            )),
                    ),
            )
            .child(
                Checkbox::new("index-unique")
                    .label("Unique")
                    .checked(self.unique)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.unique = *checked;
                        cx.notify();
                    })),
            )
            .when(self.dialect.has_nulls_not_distinct(), |this| {
                let note = if offers_nulls {
                    "Only one row may leave the columns NULL."
                } else {
                    "Needs PostgreSQL 15 or later."
                };
                this.child(
                    v_flex()
                        .pl_5()
                        .gap_1()
                        .child(
                            Checkbox::new("index-nulls-not-distinct")
                                .label("NULLs not distinct")
                                .checked(self.unique && self.nulls_not_distinct)
                                .disabled(!offers_nulls || !self.unique)
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    this.nulls_not_distinct = *checked;
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(note),
                        ),
                )
            })
            .when_some(self.method.clone(), |this, method| {
                this.child(
                    v_flex()
                        .gap_1()
                        .child(field_label(
                            if self.dialect.database_type() == blanco_core::DatabaseType::ClickHouse
                            {
                                "Index type"
                            } else {
                                "Method"
                            },
                        ))
                        .child(Select::new(&method).small()),
                )
            })
            .when(self.dialect.supports_partial_indexes(), |this| {
                this.child(labeled(
                    "Condition (partial index)",
                    Input::new(&self.condition).small(),
                ))
            })
    }
}

/// One referencing column and the column it points at.
pub(crate) struct ColumnPair {
    pub(crate) column: Entity<SelectState<Vec<String>>>,
    pub(crate) referenced: Entity<SelectState<Vec<String>>>,
}

/// A virtual key being edited: its row id, and its column pairs to select
/// once the referenced table's columns are loaded.
struct VirtualKeyEdit {
    id: i64,
    pairs: Option<(Vec<String>, Vec<String>)>,
}

/// The form for a new foreign key, or for a virtual one, new or edited.
/// Referenced tables are the ones in the tab's own schema (and its views,
/// for a virtual key). Picking one loads its columns into every pair and
/// prefills them with its primary key.
pub(crate) struct ForeignKeyForm {
    context: ConnectionContext,
    /// Kept in Blanco rather than declared on the server.
    is_virtual: bool,
    editing: Option<VirtualKeyEdit>,
    /// The table the key belongs to.
    table_name: String,
    columns: Vec<String>,
    pub(crate) name: Entity<InputState>,
    pub(crate) table: Entity<SelectState<SearchableVec<String>>>,
    pub(crate) referenced_columns: Vec<String>,
    pub(crate) referenced_primary_key: Vec<String>,
    pub(crate) pairs: Vec<ColumnPair>,
    pub(crate) on_update: Entity<SelectState<Vec<String>>>,
    pub(crate) on_delete: Entity<SelectState<Vec<String>>>,
    load_error: Option<String>,
    _subscriptions: Vec<gpui::Subscription>,
}

impl ForeignKeyForm {
    pub(crate) fn new(
        context: ConnectionContext,
        table_name: String,
        columns: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::build(context, table_name, columns, false, None, window, cx)
    }

    /// The form for a virtual key, prefilled from `existing` when one is
    /// edited.
    pub(crate) fn new_virtual(
        context: ConnectionContext,
        table_name: String,
        columns: Vec<String>,
        existing: Option<&VirtualForeignKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut form = Self::build(context, table_name, columns, true, existing, window, cx);
        form.editing = existing.map(|key| VirtualKeyEdit {
            id: key.id,
            pairs: Some((key.columns.clone(), key.referenced_columns.clone())),
        });
        form
    }

    fn build(
        context: ConnectionContext,
        table_name: String,
        columns: Vec<String>,
        is_virtual: bool,
        existing: Option<&VirtualForeignKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| {
            let state = InputState::new(window, cx).placeholder(if is_virtual {
                "Required"
            } else {
                "Generated by the server"
            });
            match existing {
                Some(key) => state.default_value(key.name.clone()),
                None => state,
            }
        });
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
            is_virtual,
            editing: None,
            table_name,
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
        form.load_tables(existing.map(|key| key.referenced_table.clone()), window, cx);
        form
    }

    pub(crate) fn add_pair(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Load the tables a key may reference, and select `selected` among
    /// them once they are in.
    fn load_tables(
        &mut self,
        selected: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let db_service = database::DatabaseService::global(cx).clone();
        let connection_id = self.context.connection_id;
        let database_name = self.context.database_name.clone();
        let schema_name = self.context.schema_name.clone();
        let with_views = self.is_virtual;
        cx.spawn_in(window, async move |this, cx| {
            let result = gpui_tokio::Tokio::spawn_result(cx, async move {
                let connection = db_service
                    .get_or_create_connection(connection_id, Some(&database_name))
                    .await?;
                let mut tables = connection.get_tables(schema_name.as_deref()).await?;
                // A virtual key may point at a view, which declares none.
                if with_views {
                    tables.extend(connection.get_views(schema_name.as_deref()).await?);
                    tables.sort_by_key(|name| name.to_lowercase());
                }
                anyhow::Ok(tables)
            })
            .await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(tables) => {
                        this.table.update(cx, |select, cx| {
                            select.set_items(SearchableVec::new(tables), window, cx);
                            if let Some(selected) = &selected {
                                select.set_selected_value(selected, window, cx);
                            }
                        });
                        if let Some(selected) = selected {
                            this.load_referenced_columns(selected, window, cx);
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

    pub(crate) fn load_referenced_columns(
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
                        // An edited virtual key gets its own pairs back, once.
                        if let Some((columns, referenced)) = this
                            .editing
                            .as_mut()
                            .and_then(|editing| editing.pairs.take())
                        {
                            this.select_pairs(&columns, &referenced, window, cx);
                            cx.notify();
                            return;
                        }
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

    /// Make the form's pairs these columns and referenced columns.
    fn select_pairs(
        &mut self,
        columns: &[String],
        referenced: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        while self.pairs.len() < columns.len() {
            self.add_pair(window, cx);
        }
        self.pairs.truncate(columns.len().max(1));
        let referenced_columns = self.referenced_columns.clone();
        for ((pair, column), referenced) in self.pairs.iter().zip(columns).zip(referenced) {
            pair.column.update(cx, |select, cx| {
                select.set_selected_value(column, window, cx)
            });
            let referenced_columns = referenced_columns.clone();
            pair.referenced.update(cx, |select, cx| {
                select.set_items(referenced_columns, window, cx);
                select.set_selected_value(referenced, window, cx);
            });
        }
    }

    /// The virtual key the form describes, or what is missing from it.
    pub(crate) fn virtual_key(&self, cx: &App) -> Result<VirtualForeignKey, String> {
        let ForeignKeyOperation::Create {
            name,
            columns,
            referenced_table,
            referenced_columns,
            ..
        } = self.operation(cx)
        else {
            return Err("Not a new foreign key".to_string());
        };
        let Some(name) = name else {
            return Err("A virtual key needs a name".to_string());
        };
        if referenced_table.is_empty() {
            return Err("Choose the referenced table".to_string());
        }
        if columns
            .iter()
            .chain(&referenced_columns)
            .any(|column| column.is_empty())
        {
            return Err("Choose a column and a referenced column in every row".to_string());
        }
        Ok(VirtualForeignKey {
            id: self.editing.as_ref().map_or(0, |editing| editing.id),
            connection_id: self.context.connection_id,
            database_name: self.context.database_name.clone(),
            schema_name: self.context.schema_name.clone(),
            table_name: self.table_name.clone(),
            name,
            columns,
            referenced_schema: None,
            referenced_table,
            referenced_columns,
        })
    }

    pub(crate) fn operation(&self, cx: &App) -> ForeignKeyOperation {
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
            .child(labeled("Name", Input::new(&self.name).small()))
            .child(
                v_flex()
                    .gap_1()
                    .child(field_label(if self.is_virtual {
                        "Referenced table or view"
                    } else {
                        "Referenced table"
                    }))
                    .child(
                        Select::new(&self.table)
                            .small()
                            .placeholder("Choose a table"),
                    ),
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
                    .child(div().flex_1().child(field_label("Column")))
                    .child(div().flex_1().child(field_label("Referenced column")))
                    .when(can_remove, |this| this.child(div().w(px(24.)))),
            )
            .child(
                v_flex()
                    .gap_2()
                    .children(self.pairs.iter().enumerate().map(|(ix, pair)| {
                        h_flex()
                            .gap_2()
                            .child(
                                div().flex_1().min_w_0().child(
                                    Select::new(&pair.column).small().placeholder("Column"),
                                ),
                            )
                            .child(
                                div().flex_1().min_w_0().child(
                                    Select::new(&pair.referenced).small().placeholder("Column"),
                                ),
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
            .when(self.is_virtual, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "Saved in Blanco for this connection, database and schema. The server never sees it, so it is saved at once and enforces nothing.",
                        ),
                )
            })
            .when(!self.is_virtual, |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child(field_label("On update"))
                                .child(Select::new(&self.on_update).small()),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child(field_label("On delete"))
                                .child(Select::new(&self.on_delete).small()),
                        ),
                )
            })
    }
}

pub(super) fn field_label(label: &'static str) -> impl IntoElement {
    div().text_xs().child(label)
}

fn labeled(label: &'static str, input: impl IntoElement) -> impl IntoElement {
    v_flex().gap_1().child(field_label(label)).child(input)
}

/// Suggests the backend's type names, and on PostgreSQL the database's own
/// types, in the column form's type field. Free text stays allowed.
struct DataTypeCompletionProvider {
    /// Label, the text inserted (a snippet for a type with arguments) and
    /// what the type is.
    types: Vec<(String, String, String)>,
}

impl DataTypeCompletionProvider {
    fn new(dialect: Dialect, user_types: Vec<String>) -> Self {
        let mut types: Vec<(String, String, String)> = dialect
            .data_types()
            .iter()
            .map(|data_type| {
                let insert = match data_type.arguments {
                    Some(_) => format!("{}($0)", data_type.name),
                    None => data_type.name.to_string(),
                };
                (data_type.label(), insert, data_type.description.to_string())
            })
            .collect();
        types.extend(
            user_types
                .into_iter()
                .map(|name| (name.clone(), name, "type in this database".to_string())),
        );
        Self { types }
    }

    /// The suggestions for the type being typed before `offset`, and where
    /// it starts. A type starts at the beginning or after `(` or `,`, so a
    /// ClickHouse wrapper's inner type is suggested too.
    fn items(&self, text: &str, offset: usize) -> (Vec<CompletionItem>, usize) {
        let offset = offset.min(text.len());
        let left = &text[..offset];
        let start = left.rfind(['(', ',']).map_or(0, |ix| ix + 1);
        let start = start + (left[start..].len() - left[start..].trim_start().len());
        let typed = left[start..].to_lowercase();
        let items = self
            .types
            .iter()
            .filter(|(label, _, _)| label.to_lowercase().starts_with(&typed))
            .map(|(label, insert, description)| CompletionItem {
                label: label.clone(),
                kind: Some(CompletionItemKind::TYPE_PARAMETER),
                detail: Some(description.clone()),
                insert_text: Some(insert.clone()),
                insert_text_format: insert.contains("$0").then_some(InsertTextFormat::SNIPPET),
                ..Default::default()
            })
            .collect();
        (items, start)
    }
}

impl CompletionProvider for DataTypeCompletionProvider {
    fn completions(
        &self,
        rope: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let text = rope.to_string();
        let (mut items, start) = self.items(&text, offset);
        let range = Range::new(
            rope.offset_to_position(start),
            rope.offset_to_position(offset),
        );
        for item in &mut items {
            let new_text = item
                .insert_text
                .clone()
                .unwrap_or_else(|| item.label.clone());
            item.text_edit = Some(CompletionTextEdit::Edit(TextEdit::new(range, new_text)));
        }
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _cx: &mut App) -> bool {
        // A deletion arrives as empty text and never opens the list.
        !new_text.is_empty()
            && new_text
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '(')
    }
}

#[cfg(test)]
mod tests {
    use super::DataTypeCompletionProvider;
    use blanco_core::DatabaseType;
    use lsp_types::InsertTextFormat;

    #[test]
    fn type_suggestions_follow_the_typed_prefix() {
        let provider = DataTypeCompletionProvider::new(
            DatabaseType::PostgreSQL.dialect(),
            vec!["order_status".to_string()],
        );
        let (items, start) = provider.items("VAR", 3);
        assert_eq!(start, 0);
        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(labels, ["varchar(n)", "varbit(n)"]);
        assert_eq!(items[0].insert_text.as_deref(), Some("varchar($0)"));
        assert_eq!(items[0].insert_text_format, Some(InsertTextFormat::SNIPPET));

        let (items, _) = provider.items("ord", 3);
        assert_eq!(items[0].label, "order_status");
        assert_eq!(items[0].insert_text_format, None);
    }

    #[test]
    fn clickhouse_wrappers_suggest_their_inner_type() {
        let provider =
            DataTypeCompletionProvider::new(DatabaseType::ClickHouse.dialect(), Vec::new());
        let text = "Nullable(Str";
        let (items, start) = provider.items(text, text.len());
        assert_eq!(start, "Nullable(".len());
        assert_eq!(items[0].label, "String");
        let text = "Map(String, UI";
        let (items, start) = provider.items(text, text.len());
        assert_eq!(start, "Map(String, ".len());
        assert!(items.iter().all(|item| item.label.starts_with("UInt")));
    }
}
