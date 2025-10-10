use gpui::{
    div, px, prelude::FluentBuilder, App, AppContext, ClickEvent, Context, Entity, FocusHandle,
    Focusable, IntoElement, Keystroke, ParentElement, Render, Styled, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants},
    h_flex,
    input::{InputState, TabSize, TextInput},
    tab::{Tab, TabBar},
    v_flex, ActiveTheme, IconName, Kbd, Sizable,
};

pub struct QueryTab {
    pub id: usize,
    pub title: String,
    pub editor: Entity<InputState>,
}

pub struct EditorPanel {
    focus_handle: FocusHandle,
    tabs: Vec<QueryTab>,
    active_tab_ix: usize,
    next_tab_id: usize,
}

impl EditorPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let first_tab = QueryTab {
            id: 0,
            title: "Query 1".to_string(),
            editor: cx.new(|cx| {
                InputState::new(window, cx)
                    .code_editor("sql".to_string())
                    .line_number(true)
                    .tab_size(TabSize {
                        tab_size: 4,
                        hard_tabs: false,
                    })
                    .soft_wrap(false)
                    .placeholder("Enter your SQL query here...")
            }),
        };

        Self {
            focus_handle: cx.focus_handle(),
            tabs: vec![first_tab],
            active_tab_ix: 0,
            next_tab_id: 1,
        }
    }

    fn add_new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        let new_tab = QueryTab {
            id: tab_id,
            title: format!("Query {}", tab_id + 1),
            editor: cx.new(|cx| {
                InputState::new(window, cx)
                    .code_editor("sql".to_string())
                    .line_number(true)
                    .tab_size(TabSize {
                        tab_size: 4,
                        hard_tabs: false,
                    })
                    .soft_wrap(false)
                    .placeholder("Enter your SQL query here...")
            }),
        };

        self.tabs.push(new_tab);
        self.active_tab_ix = self.tabs.len() - 1;
        cx.notify();
    }

    fn set_active_tab(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Self>) {
        if ix < self.tabs.len() {
            self.active_tab_ix = ix;
            cx.notify();
        }
    }

    fn run_query(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(self.active_tab_ix) {
            let query = tab.editor.read(cx).text().to_string();
            println!("Executing query: {}", query);
            // TODO: Execute the query and update results panel
        }
        cx.notify();
    }

    fn current_editor(&self) -> Option<&Entity<InputState>> {
        self.tabs.get(self.active_tab_ix).map(|tab| &tab.editor)
    }
}

impl Focusable for EditorPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for EditorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(
                // Tab bar
                TabBar::new("query-tabs")
                    .w_full()
                    .selected_index(self.active_tab_ix)
                    .on_click(cx.listener(|this, ix: &usize, window, cx| {
                        this.set_active_tab(*ix, window, cx);
                    }))
                    .children(self.tabs.iter().map(|tab| Tab::new(&tab.title)))
                    .suffix(
                        Button::new("add-tab")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Plus)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_new_tab(window, cx);
                            })),
                    ),
            )
            .child(
                // Editor
                v_flex()
                    .flex_1()
                    .min_h(px(200.))
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .when_some(self.current_editor(), |this, editor| {
                        this.child(
                            TextInput::new(editor)
                                .bordered(false)
                                .p_0()
                                .h_full()
                                .font_family("Monaco")
                                .text_size(px(14.))
                                .focus_bordered(false),
                        )
                    }),
            )
            .child(
                // Run button bar
                h_flex()
                    .p_3()
                    .gap_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().muted.opacity(0.5))
                    .child(
                        Button::new("format-query")
                            .outline()
                            .icon(IconName::Asterisk)
                            .label("Format")
                            .children(vec![Kbd::new(Keystroke::parse("shift-f").unwrap()).into_any_element()]),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("run-query")
                            .primary()
                            .icon(IconName::Check)
                            .label("Run Query")
                            .children(vec![Kbd::new(Keystroke::parse("shift-enter").unwrap()).into_any_element()])
                            .on_click(cx.listener(Self::run_query)),
                    ),
            )
    }
}
