use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity, WeakFocusHandle,
    Window, actions, anchored, deferred, div, point, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, IndexPath, Selectable,
    input::{Input, InputEvent, InputState},
    list::{List, ListDelegate, ListEvent, ListState},
    v_flex, window_paddings,
};

use crate::{
    app::{
        CommitChanges, ConnectToConnection, CopyAsCSV, CopyAsJSON, CopyAsMarkdown, CopyAsSQL,
        CopyAsTSV, ExplainQuery, ExportAsCSV, ExportAsJSON, ExportAsMarkdown, ExportAsSQL,
        ExportAsTSV, FormatQuery, NewSnippet, OpenNewConnectionModal, OpenSettings,
        RollbackChanges, RunQuery, ToggleRenderWhitespace, ToggleSidebar, ToggleWordWrap,
    },
    connections::ConnectionsPanel,
};

const COMMAND_PALETTE_CONTEXT: &str = "CommandPalette";

actions!(
    command_palette,
    [
        CancelCommandPalette,
        SelectNextCommand,
        SelectPrevCommand,
        ConfirmCommand
    ]
);

#[derive(Clone, Debug)]
pub struct CommandItem {
    pub label: String,
    pub group: String,
    pub action_type: CommandType,
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
    ConnectToConnection { connection_id: i64 },
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

        let list_entity = list_state.clone();
        let list_subscription = cx.subscribe_in(
            &list_state,
            window,
            move |palette, _, event, window, cx| match event {
                ListEvent::Confirm(_) => {
                    let command = list_entity.read(cx).delegate().selected_command();
                    palette.hide(window, cx);
                    if let Some(cmd) = command {
                        execute_command(&cmd.action_type, window, cx);
                    }
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

    fn confirm_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let command = self.list_state.read(cx).delegate().selected_command();
        self.hide(window, cx);
        if let Some(cmd) = command {
            execute_command(&cmd.action_type, window, cx);
        }
    }

    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.previous_focus = window.focused(cx).map(|h| h.downgrade());
        self.visible = true;
        self.query_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
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
                this.confirm_selection(window, cx);
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
                    .child(
                        div()
                            .py_2()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(Input::new(&input_entity).appearance(false)),
                    )
                    .child(
                        div()
                            .px_1()
                            .py_1()
                            .h(px(360.))
                            .child(List::new(&self.list_state)),
                    ),
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
        CommandType::ConnectToConnection { connection_id } => window.dispatch_action(
            Box::new(ConnectToConnection {
                connection_id: *connection_id,
            }),
            cx,
        ),
    }
}

pub struct CommandPaletteDelegate {
    sidebar: WeakEntity<ConnectionsPanel>,
    all_commands: Vec<CommandItem>,
    filtered: Vec<ScoredItem>,
    selected_index: Option<IndexPath>,
    query: String,
}

impl CommandPaletteDelegate {
    fn new(sidebar: WeakEntity<ConnectionsPanel>, cx: &App) -> Self {
        let mut delegate = Self {
            sidebar,
            all_commands: Vec::new(),
            filtered: Vec::new(),
            selected_index: None,
            query: String::new(),
        };
        delegate.refresh_commands(cx);
        delegate
    }

    fn refresh_commands(&mut self, cx: &App) {
        self.all_commands = build_commands(&self.sidebar, cx);
        self.apply_filter();
    }

    fn set_query(&mut self, query: &str, cx: &mut Context<ListState<Self>>) {
        self.query = query.to_string();
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
                .all_commands
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
            .all_commands
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

fn build_commands(sidebar: &WeakEntity<ConnectionsPanel>, cx: &App) -> Vec<CommandItem> {
    let mut commands = vec![
        CommandItem {
            label: "Run Query".into(),
            group: "Query".into(),
            action_type: CommandType::RunQuery,
        },
        CommandItem {
            label: "Explain Query".into(),
            group: "Query".into(),
            action_type: CommandType::ExplainQuery,
        },
        CommandItem {
            label: "Format Query".into(),
            group: "Query".into(),
            action_type: CommandType::FormatQuery,
        },
        CommandItem {
            label: "New Snippet".into(),
            group: "Tab".into(),
            action_type: CommandType::NewSnippet,
        },
        CommandItem {
            label: "New Connection".into(),
            group: "Connection".into(),
            action_type: CommandType::OpenNewConnectionModal,
        },
        CommandItem {
            label: "Open Settings".into(),
            group: "View".into(),
            action_type: CommandType::OpenSettings,
        },
        CommandItem {
            label: "Toggle Sidebar".into(),
            group: "View".into(),
            action_type: CommandType::ToggleSidebar,
        },
        CommandItem {
            label: "Commit Changes".into(),
            group: "Edit".into(),
            action_type: CommandType::CommitChanges,
        },
        CommandItem {
            label: "Rollback Changes".into(),
            group: "Edit".into(),
            action_type: CommandType::RollbackChanges,
        },
        CommandItem {
            label: "Copy as CSV".into(),
            group: "Results".into(),
            action_type: CommandType::CopyAsCSV,
        },
        CommandItem {
            label: "Copy as TSV".into(),
            group: "Results".into(),
            action_type: CommandType::CopyAsTSV,
        },
        CommandItem {
            label: "Copy as JSON".into(),
            group: "Results".into(),
            action_type: CommandType::CopyAsJSON,
        },
        CommandItem {
            label: "Copy as SQL".into(),
            group: "Results".into(),
            action_type: CommandType::CopyAsSQL,
        },
        CommandItem {
            label: "Copy as Markdown".into(),
            group: "Results".into(),
            action_type: CommandType::CopyAsMarkdown,
        },
        CommandItem {
            label: "Export as CSV".into(),
            group: "Results".into(),
            action_type: CommandType::ExportAsCSV,
        },
        CommandItem {
            label: "Export as TSV".into(),
            group: "Results".into(),
            action_type: CommandType::ExportAsTSV,
        },
        CommandItem {
            label: "Export as JSON".into(),
            group: "Results".into(),
            action_type: CommandType::ExportAsJSON,
        },
        CommandItem {
            label: "Export as SQL".into(),
            group: "Results".into(),
            action_type: CommandType::ExportAsSQL,
        },
        CommandItem {
            label: "Export as Markdown".into(),
            group: "Results".into(),
            action_type: CommandType::ExportAsMarkdown,
        },
        CommandItem {
            label: "Render Whitespace".into(),
            group: "View".into(),
            action_type: CommandType::ToggleRenderWhitespace,
        },
        CommandItem {
            label: "Word Wrap".into(),
            group: "View".into(),
            action_type: CommandType::ToggleWordWrap,
        },
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
            label: format!("Connect to {}", connection.name),
            group: "Connection".into(),
            action_type: CommandType::ConnectToConnection { connection_id },
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
                    .when_some(self.keybinding, |this, kbd| this.child(kbd)),
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
        let keybinding = keybinding_for_command(&command.action_type, window);
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

            let db_service = DatabaseService::new(runtime_handle.clone());
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
            p.confirm_selection(window, cx);
        });
        cx.run_until_parked();

        let collapsed_after = app.read_with(&cx, |app, _| app.sidebar_collapsed());
        assert_ne!(
            collapsed_before, collapsed_after,
            "executing Toggle Sidebar command should flip sidebar_collapsed"
        );
    }
}
