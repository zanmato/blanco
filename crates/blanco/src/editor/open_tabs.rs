use blanco_ui::IconName;
use gpui::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Render, ScrollHandle, StatefulInteractiveElement as _, Styled,
    Subscription, Window, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::ContextMenuExt as _,
    scroll::ScrollableElement as _,
    tooltip::Tooltip,
    v_flex,
};

use super::EditorPanel;
use super::render::DraggedTab;
use crate::app::NewQueryTab;

const ROW_HEIGHT: gpui::Pixels = px(28.);

/// The sidebar's list of the window's tabs, grouped by connection like the
/// strip's overflow menu. It holds no tab state of its own: everything is read
/// from and done through the `EditorPanel`, so the strip and the list always
/// agree.
pub struct OpenTabsPanel {
    editor_panel: Entity<EditorPanel>,
    scroll_handle: ScrollHandle,
    /// The tab the list last scrolled to. The list follows the active tab
    /// only when it changes, so scrolling by hand is not undone on every
    /// render.
    scrolled_to_tab: Option<usize>,
    _subscription: Subscription,
}

impl OpenTabsPanel {
    pub fn new(editor_panel: Entity<EditorPanel>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&editor_panel, |_, _, cx| cx.notify());
        Self {
            editor_panel,
            scroll_handle: ScrollHandle::new(),
            scrolled_to_tab: None,
            _subscription: subscription,
        }
    }
}

impl Render for OpenTabsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (rows, active) = self
            .editor_panel
            .update(cx, |panel, cx| panel.render_open_tabs(cx));
        if let Some((tab_ix, row_ix)) = active
            && self.scrolled_to_tab != Some(tab_ix)
        {
            self.scrolled_to_tab = Some(tab_ix);
            self.scroll_handle.scroll_to_item(row_ix);
        }

        v_flex()
            .id("open-tabs-panel")
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .px_3()
                    .pb_1p5()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Open Tabs"),
                    )
                    .child(
                        Button::new("open-tabs-new-query")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Plus)
                            .tooltip("New Query Tab")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(NewQueryTab), cx);
                            }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(
                        v_flex()
                            .id("open-tabs-scroll")
                            .size_full()
                            .px_1()
                            .py_1()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll_handle)
                            .children(rows),
                    )
                    // Beside the list rather than inside it, so the list's
                    // children are exactly the rows `scroll_to_item` counts.
                    .vertical_scrollbar(&self.scroll_handle),
            )
    }
}

impl EditorPanel {
    /// Tab indices in strip order, bucketed by connection in the order each
    /// connection first appears. Tabs without one go into a trailing bucket.
    fn open_tab_groups(&self, cx: &gpui::App) -> Vec<(Option<i64>, Vec<usize>)> {
        let mut groups: Vec<(Option<i64>, Vec<usize>)> = Vec::new();
        let mut unbound = Vec::new();
        for (ix, tab) in self.tabs.iter().enumerate() {
            match tab.context(cx).map(|context| context.connection_id) {
                Some(connection_id) => {
                    match groups
                        .iter_mut()
                        .find(|(key, _)| *key == Some(connection_id))
                    {
                        Some((_, indices)) => indices.push(ix),
                        None => groups.push((Some(connection_id), vec![ix])),
                    }
                }
                None => unbound.push(ix),
            }
        }
        if !unbound.is_empty() {
            groups.push((None, unbound));
        }
        groups
    }

    /// The list's rows, group headers included, and the active tab's index
    /// with the position of its row.
    pub(super) fn render_open_tabs(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (Vec<AnyElement>, Option<(usize, usize)>) {
        let groups = self.open_tab_groups(cx);
        let mut rows = Vec::new();
        let mut active = None;
        for (group_ix, (connection_id, indices)) in groups.into_iter().enumerate() {
            let header_context = connection_id
                .and_then(|_| indices.first())
                .and_then(|ix| self.tabs.get(*ix))
                .and_then(|tab| tab.context(cx));
            rows.push(
                h_flex()
                    .flex_none()
                    .px_2()
                    .gap_2()
                    .items_center()
                    .h(px(24.))
                    .when(group_ix > 0, |this| this.mt_2())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child(match &header_context {
                                Some(context) => context.connection_name.clone(),
                                None => "Other".to_string(),
                            }),
                    )
                    .when_some(
                        header_context.and_then(|context| context.environment_type),
                        |this, env_type| this.child(Self::environment_badge(env_type, cx)),
                    )
                    .into_any_element(),
            );
            for ix in indices {
                if let Some(row) = self.render_open_tab_row(ix, cx) {
                    if ix == self.active_tab_ix {
                        active = Some((ix, rows.len()));
                    }
                    rows.push(row);
                }
            }
        }
        (rows, active)
    }

    fn render_open_tab_row(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tab = self.tabs.get(ix)?;
        let title = tab.title(cx);
        let label = if tab.unsaved_name(cx).is_some() {
            format!("● {title}")
        } else {
            title.clone()
        };
        let icon = Self::tab_icon(tab, cx)
            .unwrap_or_else(|| Icon::new(IconName::Code).text_color(cx.theme().muted_foreground));
        let detail = tab
            .connection_tab()
            .and_then(|tab| tab.last_run_at)
            .map(crate::time_format::format_relative);
        let tooltip = Self::tab_tooltip(tab, cx);
        let selected = ix == self.active_tab_ix;
        let pinned = self.is_pinned(ix);
        let closable = self.tabs.len() > 1;
        let on_click = Self::tab_bar_click_handler(cx.entity().downgrade());
        let dragged = DraggedTab {
            ix,
            label: title.into(),
        };

        let row = h_flex()
            .id(("open-tab", ix))
            .group("open-tab")
            .w_full()
            .flex_none()
            .h(ROW_HEIGHT)
            .px_2()
            .gap_2()
            .items_center()
            .rounded(cx.theme().radius)
            .cursor_pointer()
            .text_sm()
            .text_color(cx.theme().foreground)
            .map(|this| {
                if selected {
                    this.bg(cx.theme().list_active)
                } else {
                    this.hover(|this| this.bg(cx.theme().list_hover))
                }
            })
            .on_click(move |event, window, cx| on_click(&ix, event, window, cx))
            .when(closable, |this| {
                this.on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.request_close_tab(ix, window, cx);
                    }),
                )
            })
            .when_some(tooltip, |this, tooltip| {
                this.tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            })
            .on_drag(dragged, |dragged, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| dragged.clone())
            })
            // A tab dragged downwards lands after the one it is dropped on.
            .drag_over::<DraggedTab>(move |style, dragged, _, cx| {
                if dragged.ix < ix {
                    style.border_b_2()
                } else {
                    style.border_t_2()
                }
                .border_color(cx.theme().drag_border)
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                this.move_tab(dragged.ix, ix, cx);
            }))
            .child(icon.small().flex_none())
            .child(div().flex_1().min_w_0().truncate().child(label))
            .when_some(detail, |this, detail| {
                this.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .when(closable && !pinned, |this| {
                            this.group_hover("open-tab", |this| this.invisible())
                        })
                        .child(detail),
                )
            })
            .map(|this| {
                if pinned {
                    this.child(
                        Icon::new(IconName::Pin)
                            .xsmall()
                            .flex_none()
                            .text_color(cx.theme().muted_foreground),
                    )
                } else if closable {
                    // Overlaid on the row's right end so showing it on hover
                    // does not shift the title.
                    this.relative().child(
                        div()
                            .absolute()
                            .right_1()
                            .invisible()
                            .group_hover("open-tab", |this| this.visible())
                            .child(self.close_tab_button(("open-tab-close", ix), ix, cx)),
                    )
                } else {
                    this
                }
            })
            .context_menu(self.tab_context_menu(ix, cx));

        Some(row.into_any_element())
    }
}

#[cfg(test)]
impl OpenTabsPanel {
    pub fn scroll_offset_y(&self) -> gpui::Pixels {
        self.scroll_handle.offset().y
    }

    pub fn group_titles(&self, cx: &gpui::App) -> Vec<(Option<i64>, Vec<String>)> {
        let panel = self.editor_panel.read(cx);
        panel
            .open_tab_groups(cx)
            .into_iter()
            .map(|(key, indices)| {
                let titles = indices
                    .into_iter()
                    .filter_map(|ix| panel.tabs.get(ix).map(|tab| tab.title(cx)))
                    .collect();
                (key, titles)
            })
            .collect()
    }
}
