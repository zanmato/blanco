use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity, WeakFocusHandle,
    Window, actions, anchored, deferred, div, point, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, IndexPath, Selectable, h_flex,
    input::{Input, InputEvent, InputState},
    list::{List, ListDelegate, ListEvent, ListState},
    v_flex, window_paddings,
};

use database::{DatabaseService, DatabaseType};

use crate::{
    app::{
        CommitChanges, ConnectToConnection, CopyAsCSV, CopyAsJSON, CopyAsMarkdown, CopyAsSQL,
        CopyAsTSV, CreateNewQueryTab, ExplainQuery, ExportAsCSV, ExportAsJSON, ExportAsMarkdown,
        ExportAsSQL, ExportAsTSV, FormatQuery, NewSnippet, OpenNewConnectionModal, OpenSettings,
        RollbackChanges, RunQuery, ToggleRenderWhitespace, ToggleSidebar, ToggleWordWrap,
    },
    app_database::EnvironmentType,
    connections::ConnectionsPanel,
    result_ext::ResultExt as _,
};

const COMMAND_PALETTE_CONTEXT: &str = "CommandPalette";

actions!(
    command_palette,
    [
        CancelCommandPalette,
        SelectNextCommand,
        SelectPrevCommand,
        ConfirmCommand,
        NavigateForward,
        NavigateBack
    ]
);

#[derive(Clone, Debug)]
pub struct CommandItem {
    pub label: String,
    pub group: String,
    pub kind: ItemKind,
}

impl CommandItem {
    /// Whether selecting this item drills into another level rather than
    /// running a leaf action and closing the palette.
    fn is_navigable(&self) -> bool {
        matches!(self.kind, ItemKind::Connection(_) | ItemKind::Database(_))
    }
}

/// What activating a `CommandItem` does.
#[derive(Clone, Debug)]
pub enum ItemKind {
    /// Leaf: run the action and close the palette (existing behavior).
    Command(CommandType),
    /// Navigable: connect, then drill into databases (schema-aware backends) or
    /// straight to the leaf action (schemaless backends).
    Connection(ConnectionRef),
    /// Navigable: push a level holding this database's leaf actions.
    Database(DatabaseRef),
}

/// A connection the palette can connect to and descend into.
#[derive(Clone, Debug)]
pub struct ConnectionRef {
    pub connection_id: i64,
    pub connection_name: String,
    pub db_type: DatabaseType,
    pub environment_type: EnvironmentType,
    /// `ConnectionData.database_name.clone().unwrap_or_default()`; the database
    /// to scope schemaless backends to.
    pub default_database: String,
}

/// A database within a connection that the palette can scope an action to.
#[derive(Clone, Debug)]
pub struct DatabaseRef {
    pub connection_id: i64,
    pub connection_name: String,
    pub db_type: DatabaseType,
    pub environment_type: EnvironmentType,
    pub database_name: String,
}

#[derive(Clone, Debug)]
pub enum CommandType {
    RunQuery,
    ExplainQuery,
    FormatQuery,
    NewSnippet,
    OpenSettings,
    OpenNewConnectionModal,
    ToggleSidebar,
    CommitChanges,
    RollbackChanges,
    CopyAsCSV,
    CopyAsTSV,
    CopyAsJSON,
    CopyAsSQL,
    CopyAsMarkdown,
    ExportAsCSV,
    ExportAsTSV,
    ExportAsJSON,
    ExportAsSQL,
    ExportAsMarkdown,
    ToggleRenderWhitespace,
    ToggleWordWrap,
    NewQueryForDatabase(DatabaseRef),
}

/// One pushed level of the navigation stack. `crumb` labels the breadcrumb
/// chip; `items` are the level's commands; `placeholder` hints the search box.
struct NavLevel {
    crumb: SharedString,
    items: Vec<CommandItem>,
    placeholder: SharedString,
}

struct ScoredItem {
    command: CommandItem,
    score: i64,
}

pub struct CommandPalette {
    focus_handle: FocusHandle,
    query_input: Entity<InputState>,
    list_state: Entity<ListState<CommandPaletteDelegate>>,
    visible: bool,
    previous_focus: Option<WeakFocusHandle>,
    _list_subscription: Subscription,
    _query_subscription: Subscription,
}

impl EventEmitter<()> for CommandPalette {}

impl Focusable for CommandPalette {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CommandPalette {
    pub fn new(
        sidebar: WeakEntity<ConnectionsPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Type a command or search..."));

        let delegate = CommandPaletteDelegate::new(sidebar, cx);
        let list_state = cx.new(|cx| ListState::new(delegate, window, cx).searchable(false));

        let list_subscription = cx.subscribe_in(
            &list_state,
            window,
            move |palette, _, event, window, cx| match event {
                ListEvent::Confirm(_) => {
                    palette.activate_selected(window, cx);
                }
                ListEvent::Cancel => {
                    palette.hide(window, cx);
                }
                _ => {}
            },
        );

        let query_clone = query_input.clone();
        let list_clone = list_state.clone();
        let query_subscription = cx.subscribe_in(
            &query_input,
            window,
            move |_this, _input, event, window, cx| {
                if let InputEvent::Change = event {
                    let query = query_clone.read(cx).value().to_string();
                    list_clone.update(cx, |state, cx| {
                        state.delegate_mut().set_query(&query, cx);
                        let ix = if state.delegate().items_count(0, cx) > 0 {
                            Some(IndexPath::new(0))
                        } else {
                            None
                        };
                        state.set_selected_index(ix, window, cx);
                    });
                }
            },
        );

        Self {
            focus_handle: cx.focus_handle(),
            query_input,
            list_state,
            visible: false,
            previous_focus: None,
            _list_subscription: list_subscription,
            _query_subscription: query_subscription,
        }
    }

    pub fn init(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new(
                "escape",
                CancelCommandPalette,
                Some(COMMAND_PALETTE_CONTEXT),
            ),
            KeyBinding::new("down", SelectNextCommand, Some(COMMAND_PALETTE_CONTEXT)),
            KeyBinding::new("up", SelectPrevCommand, Some(COMMAND_PALETTE_CONTEXT)),
            KeyBinding::new("enter", ConfirmCommand, Some(COMMAND_PALETTE_CONTEXT)),
            // The focused search input handles these for cursor movement and
            // editing; the patched gpui-component input propagates them only at
            // the relevant boundary (cursor at end for right, at start for
            // left, empty for backspace), so they reach us only to navigate.
            KeyBinding::new("right", NavigateForward, Some(COMMAND_PALETTE_CONTEXT)),
            KeyBinding::new("left", NavigateBack, Some(COMMAND_PALETTE_CONTEXT)),
            KeyBinding::new("backspace", NavigateBack, Some(COMMAND_PALETTE_CONTEXT)),
        ]);
    }

    fn move_selection(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        self.list_state.update(cx, |state, cx| {
            let count = state.delegate().items_count(0, cx);
            if count == 0 {
                return;
            }
            let current = state
                .selected_index()
                .map(|ix| ix.row as isize)
                .unwrap_or(-1);
            let next = if current < 0 && delta < 0 {
                (count as isize) - 1
            } else {
                (current + delta).rem_euclid(count as isize)
            };
            let new_ix = IndexPath::new(next as usize);
            state.set_selected_index(Some(new_ix), window, cx);
            state.scroll_to_item(new_ix, gpui::ScrollStrategy::Center, window, cx);
        });
    }

    /// Activate the currently selected item: run a leaf and close, or drill
    /// into a navigable connection/database.
    fn activate_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Ignore activation while an async connect is in flight.
        if self.list_state.read(cx).delegate().loading {
            return;
        }
        let Some(command) = self.list_state.read(cx).delegate().selected_command() else {
            return;
        };
        self.activate_item(command, window, cx);
    }

    /// Activate a specific item, independent of list selection.
    fn activate_item(&mut self, command: CommandItem, window: &mut Window, cx: &mut Context<Self>) {
        match command.kind {
            ItemKind::Command(cmd) => {
                self.hide(window, cx);
                execute_command(&cmd, window, cx);
            }
            ItemKind::Connection(connection) => {
                self.begin_connect_and_descend(connection, window, cx);
            }
            ItemKind::Database(database) => {
                let crumb: SharedString = database.database_name.clone().into();
                let leaf = new_query_leaf(&database);
                self.push_level(crumb, vec![leaf], "Search actions...", window, cx);
            }
        }
    }

    #[cfg(test)]
    pub fn activate_item_for_test(
        &mut self,
        command: CommandItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_item(command, window, cx);
    }

    #[cfg(test)]
    pub fn breadcrumbs_for_test(&self, cx: &App) -> Vec<String> {
        self.list_state
            .read(cx)
            .delegate()
            .breadcrumbs()
            .into_iter()
            .map(|c| c.to_string())
            .collect()
    }

    /// `Right` at the end of the filter: descend only into navigable items. A
    /// leaf is left for the input to no-op on.
    fn navigate_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let navigable = self
            .list_state
            .read(cx)
            .delegate()
            .selected_command()
            .map(|c| c.is_navigable())
            .unwrap_or(false);
        if navigable {
            self.activate_selected(window, cx);
        }
    }

    /// `Left` at the start / `Backspace` on an empty filter: go back one level.
    fn navigate_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.list_state.read(cx).delegate().loading {
            return;
        }
        if self.list_state.read(cx).delegate().stack.is_empty() {
            return;
        }
        self.list_state.update(cx, |state, cx| {
            state.delegate_mut().pop_level();
            cx.notify();
        });
        self.after_level_change(window, cx);
    }

    /// Jump back to breadcrumb depth `depth` (0 == root).
    fn navigate_to_depth(&mut self, depth: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.list_state.read(cx).delegate().loading {
            return;
        }
        self.list_state.update(cx, |state, cx| {
            state.delegate_mut().pop_to(depth);
            cx.notify();
        });
        self.after_level_change(window, cx);
    }

    /// Push a new level and reset the filter/selection to its top.
    fn push_level(
        &mut self,
        crumb: SharedString,
        items: Vec<CommandItem>,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder: SharedString = placeholder.to_string().into();
        self.list_state.update(cx, |state, cx| {
            state.delegate_mut().push_level(crumb, items, placeholder);
            cx.notify();
        });
        self.after_level_change(window, cx);
    }

    /// Shared bookkeeping after the navigation stack changes: clear the input,
    /// update its placeholder, reset selection to the first row and refocus.
    fn after_level_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = self.list_state.read(cx).delegate().placeholder();
        self.query_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_placeholder(placeholder, window, cx);
        });
        self.list_state.update(cx, |state, cx| {
            state.delegate_mut().set_query("", cx);
            let ix = if state.delegate().items_count(0, cx) > 0 {
                Some(IndexPath::new(0))
            } else {
                None
            };
            state.set_selected_index(ix, window, cx);
            if ix.is_some() {
                state.scroll_to_item(IndexPath::new(0), gpui::ScrollStrategy::Top, window, cx);
            }
        });
        self.query_input.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// Connect to `connection` (reusing the sidebar's cached connection), show a
    /// loading state, then drill into its databases or straight to the leaf
    /// action for schemaless backends.
    fn begin_connect_and_descend(
        &mut self,
        connection: ConnectionRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.list_state.update(cx, |state, cx| {
            state.delegate_mut().set_loading(true);
            cx.notify();
        });
        cx.notify();

        // Reflect the connection in the sidebar tree. `get_or_create_connection`
        // is cached, so this does not connect twice.
        window.dispatch_action(
            Box::new(ConnectToConnection {
                connection_id: connection.connection_id,
            }),
            cx,
        );

        let db_service = DatabaseService::global(cx).clone();
        cx.spawn_in(window, async move |this, cx| {
            // `Some(databases)` for schema-aware backends, `None` for schemaless
            // ones (which skip the database level).
            let result: anyhow::Result<Option<Vec<String>>> = async {
                let conn = db_service
                    .get_or_create_connection(connection.connection_id, None)
                    .await?;
                conn.ping().await?;
                if conn.supports_schemas() {
                    Ok(Some(conn.get_databases().await?))
                } else {
                    Ok(None)
                }
            }
            .await;

            this.update_in(cx, |this, window, cx| match result {
                Ok(databases) => {
                    let crumb: SharedString = connection.connection_name.clone().into();
                    this.list_state.update(cx, |state, cx| {
                        state.delegate_mut().set_loading(false);
                        cx.notify();
                    });
                    match databases {
                        Some(databases) => {
                            let items = databases
                                .into_iter()
                                .map(|name| database_item(&connection, name))
                                .collect();
                            this.push_level(crumb, items, "Search databases...", window, cx);
                        }
                        None => {
                            // Schemaless backend: skip the database level entirely.
                            let database = DatabaseRef {
                                connection_id: connection.connection_id,
                                connection_name: connection.connection_name.clone(),
                                db_type: connection.db_type,
                                environment_type: connection.environment_type,
                                database_name: connection.default_database.clone(),
                            };
                            let leaf = new_query_leaf(&database);
                            this.push_level(crumb, vec![leaf], "Search actions...", window, cx);
                        }
                    }
                }
                Err(e) => {
                    this.list_state.update(cx, |state, cx| {
                        let delegate = state.delegate_mut();
                        delegate.set_loading(false);
                        delegate.set_error(Some(format!("{e:#}")));
                        cx.notify();
                    });
                    this.query_input.focus_handle(cx).focus(window, cx);
                    cx.notify();
                }
            })
            .log_err();
        })
        .detach();
    }

    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.previous_focus = window.focused(cx).map(|h| h.downgrade());
        self.visible = true;
        self.query_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_placeholder("Type a command or search...", window, cx);
        });
        self.list_state.update(cx, |state, cx| {
            state.delegate_mut().refresh_commands(cx);
            state.delegate_mut().set_query("", cx);
            let ix = if state.delegate().items_count(0, cx) > 0 {
                Some(IndexPath::new(0))
            } else {
                None
            };
            state.set_selected_index(ix, window, cx);
            // The list retains its scroll offset between opens, so explicitly
            // scroll back to the top to keep the freshly selected first item
            // visible instead of leaving it off-screen above a stale offset.
            if ix.is_some() {
                state.scroll_to_item(IndexPath::new(0), gpui::ScrollStrategy::Top, window, cx);
            }
        });
        self.query_input.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    pub fn hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = false;
        if let Some(handle) = self.previous_focus.take().and_then(|h| h.upgrade()) {
            window.focus(&handle, cx);
        } else {
            window.blur();
        }
        cx.notify();
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.visible {
            self.hide(window, cx);
        } else {
            self.show(window, cx);
        }
    }

    #[cfg(test)]
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    #[cfg(test)]
    pub fn filtered_labels(&self, cx: &App) -> Vec<String> {
        self.list_state
            .read(cx)
            .delegate()
            .filtered
            .iter()
            .map(|item| item.command.label.clone())
            .collect()
    }

    #[cfg(test)]
    pub fn set_query_for_test(&self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        // `InputState::set_value` suppresses `InputEvent::Change`, so drive the
        // list delegate the same way the input subscription does instead of
        // relying on the editor to emit a change event.
        self.query_input.update(cx, |input, cx| {
            input.set_value(query, window, cx);
        });
        self.list_state.update(cx, |state, cx| {
            state.delegate_mut().set_query(query, cx);
            let ix = if state.delegate().items_count(0, cx) > 0 {
                Some(IndexPath::new(0))
            } else {
                None
            };
            state.set_selected_index(ix, window, cx);
        });
    }
}

impl Render for CommandPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let base = div()
            .key_context(COMMAND_PALETTE_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &CancelCommandPalette, window, cx| {
                this.hide(window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectNextCommand, window, cx| {
                this.move_selection(1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectPrevCommand, window, cx| {
                this.move_selection(-1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ConfirmCommand, window, cx| {
                this.activate_selected(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NavigateForward, window, cx| {
                this.navigate_forward(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NavigateBack, window, cx| {
                this.navigate_back(window, cx);
            }));

        if !self.visible {
            return base.into_any_element();
        }

        let input_entity = self.query_input.clone();

        let paddings = window_paddings(window);
        let viewport = window.viewport_size();
        let view_size = gpui::size(
            viewport.width - paddings.left - paddings.right,
            viewport.height - paddings.top - paddings.bottom,
        );

        let overlay = div()
            .id("palette-overlay")
            .absolute()
            .inset_0()
            .bg(gpui::black().opacity(0.5))
            .on_click(cx.listener(|this, _, window, cx| {
                this.hide(window, cx);
            }));

        let (breadcrumbs, loading, error) = {
            let delegate = self.list_state.read(cx).delegate();
            (
                delegate.breadcrumbs(),
                delegate.loading,
                delegate.error.clone(),
            )
        };

        let muted = cx.theme().muted_foreground;
        let accent = cx.theme().accent_foreground;

        // The breadcrumb row only shows once we have descended at least one
        // level (i.e. there is more than just the root crumb).
        let breadcrumb_row = (breadcrumbs.len() > 1).then(|| {
            let last = breadcrumbs.len() - 1;
            let mut row = h_flex()
                .px_2()
                .py_1()
                .gap_1()
                .text_sm()
                .border_b_1()
                .border_color(cx.theme().border);
            for (i, crumb) in breadcrumbs.into_iter().enumerate() {
                if i > 0 {
                    row = row.child(div().text_color(muted).child("/"));
                }
                let is_last = i == last;
                let chip = div()
                    .id(("crumb", i))
                    .when(!is_last, |el| {
                        el.cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.navigate_to_depth(i, window, cx);
                            }))
                    })
                    .text_color(if is_last { accent } else { muted })
                    .child(crumb);
                row = row.child(chip);
            }
            row
        });

        let list_area = div().px_1().py_1().h(px(360.));
        let list_area = if loading {
            list_area.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size_full()
                    .text_sm()
                    .text_color(muted)
                    .child("Connecting…"),
            )
        } else if let Some(error) = error {
            list_area.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size_full()
                    .px_2()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(error),
            )
        } else {
            list_area.child(List::new(&self.list_state))
        };

        let palette_content = div()
            .id("palette-content")
            .absolute()
            .top(px(4.))
            .left(px(0.))
            .right(px(0.))
            .mx_auto()
            .w(px(520.))
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius_lg)
            .shadow_lg()
            .occlude()
            .on_click(|_, _, _| {})
            .child(
                v_flex()
                    .when_some(breadcrumb_row, |this, row| this.child(row))
                    .child(
                        div()
                            .py_2()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(Input::new(&input_entity).appearance(false)),
                    )
                    .child(list_area),
            );

        base.child(
            deferred(
                anchored()
                    .position(point(paddings.left, paddings.top))
                    .snap_to_window()
                    .child(
                        div()
                            .occlude()
                            .w(view_size.width)
                            .h(view_size.height)
                            .child(overlay)
                            .child(palette_content),
                    ),
            )
            .with_priority(2),
        )
        .into_any_element()
    }
}

fn keybinding_for_command(cmd: &CommandType, window: &Window) -> Option<gpui_component::kbd::Kbd> {
    use gpui_component::kbd::Kbd;
    let action: Box<dyn gpui::Action> = match cmd {
        CommandType::FormatQuery => Box::new(FormatQuery),
        CommandType::OpenSettings => Box::new(OpenSettings),
        CommandType::CommitChanges => Box::new(CommitChanges),
        CommandType::RollbackChanges => Box::new(RollbackChanges),
        _ => return None,
    };
    Kbd::binding_for_action(action.as_ref(), None, window)
}

fn execute_command(cmd: &CommandType, window: &mut Window, cx: &mut App) {
    match cmd {
        CommandType::RunQuery => window.dispatch_action(Box::new(RunQuery), cx),
        CommandType::ExplainQuery => window.dispatch_action(Box::new(ExplainQuery), cx),
        CommandType::FormatQuery => window.dispatch_action(Box::new(FormatQuery), cx),
        CommandType::NewSnippet => window.dispatch_action(Box::new(NewSnippet), cx),
        CommandType::OpenSettings => window.dispatch_action(Box::new(OpenSettings), cx),
        CommandType::OpenNewConnectionModal => {
            window.dispatch_action(Box::new(OpenNewConnectionModal), cx)
        }
        CommandType::ToggleSidebar => window.dispatch_action(Box::new(ToggleSidebar), cx),
        CommandType::CommitChanges => window.dispatch_action(Box::new(CommitChanges), cx),
        CommandType::RollbackChanges => window.dispatch_action(Box::new(RollbackChanges), cx),
        CommandType::CopyAsCSV => window.dispatch_action(Box::new(CopyAsCSV), cx),
        CommandType::CopyAsTSV => window.dispatch_action(Box::new(CopyAsTSV), cx),
        CommandType::CopyAsJSON => window.dispatch_action(Box::new(CopyAsJSON), cx),
        CommandType::CopyAsSQL => window.dispatch_action(Box::new(CopyAsSQL), cx),
        CommandType::CopyAsMarkdown => window.dispatch_action(Box::new(CopyAsMarkdown), cx),
        CommandType::ExportAsCSV => window.dispatch_action(Box::new(ExportAsCSV), cx),
        CommandType::ExportAsTSV => window.dispatch_action(Box::new(ExportAsTSV), cx),
        CommandType::ExportAsJSON => window.dispatch_action(Box::new(ExportAsJSON), cx),
        CommandType::ExportAsSQL => window.dispatch_action(Box::new(ExportAsSQL), cx),
        CommandType::ExportAsMarkdown => window.dispatch_action(Box::new(ExportAsMarkdown), cx),
        CommandType::ToggleRenderWhitespace => {
            window.dispatch_action(Box::new(ToggleRenderWhitespace), cx)
        }
        CommandType::ToggleWordWrap => window.dispatch_action(Box::new(ToggleWordWrap), cx),
        CommandType::NewQueryForDatabase(database) => window.dispatch_action(
            Box::new(CreateNewQueryTab {
                connection_id: database.connection_id,
                connection_name: database.connection_name.clone(),
                db_type: database.db_type,
                database_name: database.database_name.clone(),
                schema_name: None,
                table_name: None,
                environment_type: Some(database.environment_type),
                inspect_key: false,
            }),
            cx,
        ),
    }
}

/// Label shown for the first (root) breadcrumb chip.
const ROOT_CRUMB: &str = "Commands";

pub struct CommandPaletteDelegate {
    sidebar: WeakEntity<ConnectionsPanel>,
    /// The root level: base commands plus per-connection navigable items.
    root_commands: Vec<CommandItem>,
    /// Pushed navigation levels; empty == at the root.
    stack: Vec<NavLevel>,
    filtered: Vec<ScoredItem>,
    selected_index: Option<IndexPath>,
    query: String,
    loading: bool,
    error: Option<String>,
}

impl CommandPaletteDelegate {
    fn new(sidebar: WeakEntity<ConnectionsPanel>, cx: &App) -> Self {
        let mut delegate = Self {
            sidebar,
            root_commands: Vec::new(),
            stack: Vec::new(),
            filtered: Vec::new(),
            selected_index: None,
            query: String::new(),
            loading: false,
            error: None,
        };
        delegate.refresh_commands(cx);
        delegate
    }

    /// The items for the current level (root or the top of the stack).
    fn current_items(&self) -> &[CommandItem] {
        self.stack
            .last()
            .map(|level| level.items.as_slice())
            .unwrap_or(&self.root_commands)
    }

    fn refresh_commands(&mut self, cx: &App) {
        self.root_commands = build_commands(&self.sidebar, cx);
        self.stack.clear();
        self.loading = false;
        self.error = None;
        self.query.clear();
        self.apply_filter();
    }

    fn push_level(
        &mut self,
        crumb: SharedString,
        items: Vec<CommandItem>,
        placeholder: SharedString,
    ) {
        self.stack.push(NavLevel {
            crumb,
            items,
            placeholder,
        });
        self.error = None;
        self.query.clear();
        self.apply_filter();
    }

    fn pop_level(&mut self) {
        self.stack.pop();
        self.error = None;
        self.query.clear();
        self.apply_filter();
    }

    /// Truncate the stack to `depth` levels (0 == root).
    fn pop_to(&mut self, depth: usize) {
        self.stack.truncate(depth);
        self.error = None;
        self.query.clear();
        self.apply_filter();
    }

    fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
        if loading {
            self.error = None;
        }
    }

    fn set_error(&mut self, error: Option<String>) {
        self.error = error;
    }

    /// Breadcrumb labels including the root, from root to the current level.
    fn breadcrumbs(&self) -> Vec<SharedString> {
        let mut crumbs = Vec::with_capacity(self.stack.len() + 1);
        crumbs.push(SharedString::new_static(ROOT_CRUMB));
        crumbs.extend(self.stack.iter().map(|level| level.crumb.clone()));
        crumbs
    }

    /// The search placeholder for the current level.
    fn placeholder(&self) -> SharedString {
        self.stack
            .last()
            .map(|level| level.placeholder.clone())
            .unwrap_or_else(|| SharedString::new_static("Type a command or search..."))
    }

    fn set_query(&mut self, query: &str, cx: &mut Context<ListState<Self>>) {
        self.query = query.to_string();
        // Typing dismisses a stale connection error so the list reappears.
        self.error = None;
        self.apply_filter();
        self.selected_index = if self.filtered.is_empty() {
            None
        } else {
            Some(IndexPath::new(0))
        };
        cx.notify();
    }

    fn apply_filter(&mut self) {
        if self.query.is_empty() {
            self.filtered = self
                .current_items()
                .iter()
                .enumerate()
                .map(|(i, cmd)| ScoredItem {
                    command: cmd.clone(),
                    score: 10000 - i as i64,
                })
                .collect();
            return;
        }

        let query_lower = self.query.to_lowercase();
        let mut results: Vec<ScoredItem> = self
            .current_items()
            .iter()
            .filter_map(|cmd| {
                let label_lower = cmd.label.to_lowercase();
                let score = fuzzy_score(&label_lower, &query_lower)?;
                Some(ScoredItem {
                    command: cmd.clone(),
                    score,
                })
            })
            .collect();

        results.sort_by_key(|item| std::cmp::Reverse(item.score));
        self.filtered = results;
    }

    pub fn selected_command(&self) -> Option<CommandItem> {
        let ix = self.selected_index?;
        self.filtered.get(ix.row).map(|item| item.command.clone())
    }
}

fn fuzzy_score(text: &str, query: &str) -> Option<i64> {
    let text_chars: Vec<char> = text.chars().collect();
    let query_chars: Vec<char> = query.chars().collect();

    if query_chars.is_empty() {
        return Some(0);
    }
    if query_chars.len() > text_chars.len() {
        return None;
    }

    let mut score: i64 = 0;
    let mut query_pos = 0;
    let mut last_match_pos: Option<usize> = None;

    for (i, &ch) in text_chars.iter().enumerate() {
        if query_pos >= query_chars.len() {
            break;
        }
        if ch == query_chars[query_pos] {
            let mut bonus: i64 = 1;
            if let Some(last) = last_match_pos
                && last + 1 == i
            {
                bonus += 10;
            }
            if i == 0 || text_chars[i - 1] == ' ' || text_chars[i - 1] == '-' {
                bonus += 5;
            }
            score += bonus;
            last_match_pos = Some(i);
            query_pos += 1;
        }
    }

    if query_pos == query_chars.len() {
        Some(score)
    } else {
        None
    }
}

/// Build a leaf command item.
fn leaf(label: &str, group: &str, command: CommandType) -> CommandItem {
    CommandItem {
        label: label.into(),
        group: group.into(),
        kind: ItemKind::Command(command),
    }
}

/// The leaf action shown at the bottom of a navigation drill-down.
fn new_query_leaf(database: &DatabaseRef) -> CommandItem {
    CommandItem {
        label: "New query".into(),
        group: "Action".into(),
        kind: ItemKind::Command(CommandType::NewQueryForDatabase(database.clone())),
    }
}

/// A navigable database item under a connection.
fn database_item(connection: &ConnectionRef, name: String) -> CommandItem {
    CommandItem {
        label: name.clone(),
        group: "Database".into(),
        kind: ItemKind::Database(DatabaseRef {
            connection_id: connection.connection_id,
            connection_name: connection.connection_name.clone(),
            db_type: connection.db_type,
            environment_type: connection.environment_type,
            database_name: name,
        }),
    }
}

fn build_commands(sidebar: &WeakEntity<ConnectionsPanel>, cx: &App) -> Vec<CommandItem> {
    let mut commands = vec![
        leaf("Run Query", "Query", CommandType::RunQuery),
        leaf("Explain Query", "Query", CommandType::ExplainQuery),
        leaf("Format Query", "Query", CommandType::FormatQuery),
        leaf("New Snippet", "Tab", CommandType::NewSnippet),
        leaf(
            "New Connection",
            "Connection",
            CommandType::OpenNewConnectionModal,
        ),
        leaf("Open Settings", "View", CommandType::OpenSettings),
        leaf("Toggle Sidebar", "View", CommandType::ToggleSidebar),
        leaf("Commit Changes", "Edit", CommandType::CommitChanges),
        leaf("Rollback Changes", "Edit", CommandType::RollbackChanges),
        leaf("Copy as CSV", "Results", CommandType::CopyAsCSV),
        leaf("Copy as TSV", "Results", CommandType::CopyAsTSV),
        leaf("Copy as JSON", "Results", CommandType::CopyAsJSON),
        leaf("Copy as SQL", "Results", CommandType::CopyAsSQL),
        leaf("Copy as Markdown", "Results", CommandType::CopyAsMarkdown),
        leaf("Export as CSV", "Results", CommandType::ExportAsCSV),
        leaf("Export as TSV", "Results", CommandType::ExportAsTSV),
        leaf("Export as JSON", "Results", CommandType::ExportAsJSON),
        leaf("Export as SQL", "Results", CommandType::ExportAsSQL),
        leaf(
            "Export as Markdown",
            "Results",
            CommandType::ExportAsMarkdown,
        ),
        leaf(
            "Render Whitespace",
            "View",
            CommandType::ToggleRenderWhitespace,
        ),
        leaf("Word Wrap", "View", CommandType::ToggleWordWrap),
    ];

    add_connection_commands(&mut commands, sidebar, cx);

    commands
}

fn add_connection_commands(
    commands: &mut Vec<CommandItem>,
    sidebar: &WeakEntity<ConnectionsPanel>,
    cx: &App,
) {
    let Some(sidebar) = sidebar.upgrade() else {
        return;
    };
    for connection in &sidebar.read(cx).connections {
        let Some(connection_id) = connection.id else {
            continue;
        };
        commands.push(CommandItem {
            label: connection.name.clone(),
            group: "Connection".into(),
            kind: ItemKind::Connection(ConnectionRef {
                connection_id,
                connection_name: connection.name.clone(),
                db_type: connection.db_type,
                environment_type: connection.environment_type,
                default_database: connection.database_name.clone().unwrap_or_default(),
            }),
        });
    }
}

pub struct CommandPaletteItemElement {
    ix: IndexPath,
    command: CommandItem,
    selected: bool,
    muted_color: gpui::Hsla,
    selected_bg: gpui::Hsla,
    selected_fg: gpui::Hsla,
    keybinding: Option<gpui_component::kbd::Kbd>,
}

impl CommandPaletteItemElement {
    pub fn new(
        ix: IndexPath,
        command: CommandItem,
        selected: bool,
        muted_color: gpui::Hsla,
        selected_bg: gpui::Hsla,
        selected_fg: gpui::Hsla,
        keybinding: Option<gpui_component::kbd::Kbd>,
    ) -> Self {
        Self {
            ix,
            command,
            selected,
            muted_color,
            selected_bg,
            selected_fg,
            keybinding,
        }
    }
}

impl Selectable for CommandPaletteItemElement {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl IntoElement for CommandPaletteItemElement {
    type Element = gpui::Stateful<gpui::Div>;

    fn into_element(self) -> Self::Element {
        let id = gpui::ElementId::Name(format!("cmd-{}", self.ix.row).into());
        let selected = self.selected;
        let selected_bg = self.selected_bg;
        let selected_fg = self.selected_fg;
        let group_color = if selected {
            selected_fg
        } else {
            self.muted_color
        };
        let navigable = self.command.is_navigable();
        div()
            .id(id)
            .w_full()
            .px_2()
            .py_1()
            .rounded(px(4.))
            .cursor_pointer()
            .when(selected, |el| el.bg(selected_bg).text_color(selected_fg))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_sm()
                    .child(div().text_color(group_color).child(self.command.group))
                    .child(div().text_color(group_color).child("/"))
                    .child(div().flex_1().child(self.command.label))
                    .when_some(self.keybinding, |this, kbd| this.child(kbd))
                    // A chevron marks items that drill into another level.
                    .when(navigable, |this| {
                        this.child(div().text_color(group_color).child("›"))
                    }),
            )
    }
}

impl ListDelegate for CommandPaletteDelegate {
    type Item = CommandPaletteItemElement;

    fn items_count(&self, _section: usize, _cx: &App) -> usize {
        self.filtered.len()
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let selected = self.selected_index == Some(ix);
        let command = self.filtered.get(ix.row)?.command.clone();
        let theme = cx.theme();
        let muted_color = theme.muted_foreground;
        let selected_bg = theme.accent;
        let selected_fg = theme.accent_foreground;
        let keybinding = match &command.kind {
            ItemKind::Command(ct) => keybinding_for_command(ct, window),
            _ => None,
        };
        Some(CommandPaletteItemElement::new(
            ix,
            command,
            selected,
            muted_color,
            selected_bg,
            selected_fg,
            keybinding,
        ))
    }

    fn perform_search(
        &mut self,
        query: &str,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> gpui::Task<()> {
        self.query = query.to_string();
        self.apply_filter();
        cx.notify();
        gpui::Task::ready(())
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        self.selected_index = ix;
        cx.notify();
    }

    fn confirm(
        &mut self,
        _secondary: bool,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) {
    }

    fn cancel(&mut self, _window: &mut Window, _cx: &mut Context<ListState<Self>>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_score_exact_match() {
        assert!(fuzzy_score("run query", "run query").is_some());
    }

    #[test]
    fn test_fuzzy_score_prefix() {
        assert!(fuzzy_score("run query", "run").is_some());
    }

    #[test]
    fn test_fuzzy_score_subsequence() {
        assert!(fuzzy_score("switch to dark theme", "std").is_some());
    }

    #[test]
    fn test_fuzzy_score_no_match() {
        assert!(fuzzy_score("run query", "xyz").is_none());
    }

    #[test]
    fn test_fuzzy_score_empty_query() {
        assert!(fuzzy_score("run query", "").is_some());
    }

    #[test]
    fn test_fuzzy_score_consecutive_beats_spread() {
        let consecutive = fuzzy_score("close tab", "clot").unwrap();
        let spread = fuzzy_score("clear history", "clt").unwrap();
        assert!(consecutive > spread);
    }
}

#[cfg(test)]
mod visual_tests {
    use database::DatabaseService;
    use gpui::{AppContext, Focusable, TestAppContext, VisualTestContext};
    use gpui_component::Root;

    use crate::app::{BlancoApp, ToggleCommandPalette};
    use crate::app_database::AppDatabase;
    use crate::app_settings::AppSettings;
    use crate::settings::Settings;

    fn setup_app(cx: &mut TestAppContext) -> (gpui::Entity<BlancoApp>, gpui::WindowHandle<Root>) {
        cx.executor().allow_parking();

        let mut app: Option<gpui::Entity<BlancoApp>> = None;
        let window_handle = cx.update(|cx| {
            gpui_component::init(cx);
            gpui_tokio::init(cx);
            super::CommandPalette::init(cx);

            let runtime_handle = gpui_tokio::Tokio::handle(cx);

            let app_database = runtime_handle
                .block_on(AppDatabase::new_in_memory(runtime_handle.clone()))
                .expect("failed to create in-memory database");
            cx.set_global(app_database);

            let db_service = DatabaseService::new(runtime_handle);
            cx.set_global(db_service);

            let settings = AppSettings::new(cx, Settings::default());
            cx.set_global(settings);

            cx.open_window(Default::default(), |window, cx| {
                let blanco_app = cx.new(|cx| BlancoApp::new(window, cx));
                app = Some(blanco_app.clone());
                cx.new(|cx| Root::new(blanco_app, window, cx))
            })
            .expect("failed to open window")
        });

        (app.expect("app should be set"), window_handle)
    }

    #[gpui::test]
    async fn test_toggle_command_palette_shows_and_filters(cx: &mut TestAppContext) {
        let (app, window_handle) = setup_app(cx);
        let mut cx = VisualTestContext::from_window(window_handle.into(), cx);

        let palette = app.read_with(&cx, |app, _| app.command_palette().clone());

        assert!(
            !palette.read_with(&cx, |p, _| p.is_visible()),
            "palette should start hidden"
        );

        // The dispatched action only routes to BlancoApp's handler when its
        // tracked focus handle (or a descendant) is focused, so mirror the real
        // app by focusing it first.
        cx.update(|window, cx| {
            let handle = app.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        });
        cx.run_until_parked();

        cx.update(|window, cx| {
            window.dispatch_action(Box::new(ToggleCommandPalette), cx);
        });
        cx.run_until_parked();

        assert!(
            palette.read_with(&cx, |p, _| p.is_visible()),
            "palette should be visible after toggle"
        );

        palette.update_in(&mut cx, |p, window, cx| {
            p.set_query_for_test("format query", window, cx);
        });
        cx.run_until_parked();

        let labels = palette.read_with(&cx, |p, cx| p.filtered_labels(cx));
        assert!(
            labels.iter().any(|l| l == "Format Query"),
            "filtered list should contain 'Format Query', got {labels:?}"
        );
        assert!(
            !labels.iter().any(|l| l == "Run Query"),
            "non-matching commands should be filtered out, got {labels:?}"
        );
    }

    #[gpui::test]
    async fn test_execute_command_toggles_sidebar(cx: &mut TestAppContext) {
        let (app, window_handle) = setup_app(cx);
        let mut cx = VisualTestContext::from_window(window_handle.into(), cx);

        let palette = app.read_with(&cx, |app, _| app.command_palette().clone());

        let collapsed_before = app.read_with(&cx, |app, _| app.sidebar_collapsed());

        // Mirror the real app where the editor holds focus when the palette is
        // opened, so hide() restores focus to a node under BlancoApp's action
        // handlers and the dispatched command is routed correctly.
        cx.update(|window, cx| {
            let handle = app.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        });
        cx.run_until_parked();

        cx.update(|window, cx| {
            window.dispatch_action(Box::new(ToggleCommandPalette), cx);
        });
        cx.run_until_parked();

        palette.update_in(&mut cx, |p, window, cx| {
            p.set_query_for_test("toggle sidebar", window, cx);
        });
        cx.run_until_parked();

        palette.update_in(&mut cx, |p, window, cx| {
            p.activate_selected(window, cx);
        });
        cx.run_until_parked();

        let collapsed_after = app.read_with(&cx, |app, _| app.sidebar_collapsed());
        assert_ne!(
            collapsed_before, collapsed_after,
            "executing Toggle Sidebar command should flip sidebar_collapsed"
        );
    }

    #[gpui::test]
    async fn test_database_item_pushes_new_query_leaf(cx: &mut TestAppContext) {
        use super::{CommandItem, DatabaseRef, ItemKind};
        use database::DatabaseType;

        let (app, window_handle) = setup_app(cx);
        let mut cx = VisualTestContext::from_window(window_handle.into(), cx);

        let palette = app.read_with(&cx, |app, _| app.command_palette().clone());

        cx.update(|window, cx| {
            window.dispatch_action(Box::new(ToggleCommandPalette), cx);
        });
        cx.run_until_parked();

        // Activating a database item drills into a level holding a single
        // "New query" leaf, with the database name in the breadcrumb trail.
        palette.update_in(&mut cx, |p, window, cx| {
            let item = CommandItem {
                label: "mydb".into(),
                group: "Database".into(),
                kind: ItemKind::Database(DatabaseRef {
                    connection_id: 1,
                    connection_name: "Postgres Test".into(),
                    db_type: DatabaseType::PostgreSQL,
                    environment_type: Default::default(),
                    database_name: "mydb".into(),
                }),
            };
            p.activate_item_for_test(item, window, cx);
        });
        cx.run_until_parked();

        let labels = palette.read_with(&cx, |p, cx| p.filtered_labels(cx));
        assert_eq!(
            labels,
            vec!["New query".to_string()],
            "database level should hold exactly the New query leaf, got {labels:?}"
        );

        let crumbs = palette.read_with(&cx, |p, cx| p.breadcrumbs_for_test(cx));
        assert!(
            crumbs.iter().any(|c| c == "mydb"),
            "breadcrumb trail should include the database name, got {crumbs:?}"
        );
    }
}
