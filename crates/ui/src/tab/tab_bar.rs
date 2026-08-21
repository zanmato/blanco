use gpui::ClickEvent;
use gpui::{
    Anchor, AnyElement, App, Div, Edges, ElementId, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, ScrollHandle, Stateful, StatefulInteractiveElement as _, StyleRefinement, Styled,
    Window, div, prelude::FluentBuilder as _, px,
};
type GroupLabelFn = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement + 'static>;
type CloseFn = Rc<dyn Fn(&usize, &ClickEvent, &mut Window, &mut App) + 'static>;

/// The per-tab data the overflow menu needs, captured while the tabs are
/// consumed into elements.
#[derive(Clone)]
struct MenuItemInfo {
    label: Option<SharedString>,
    disabled: bool,
    group: Option<SharedString>,
    group_label: Option<GroupLabelFn>,
    menu_detail: Option<SharedString>,
    on_close: Option<CloseFn>,
}
use smallvec::SmallVec;
use std::rc::Rc;

use super::{Tab, TabVariant};
use gpui::SharedString;
use gpui::WeakEntity;
use gpui_component::Icon;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::PopupMenu;
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::{ActiveTheme, IconName, Selectable, Sizable, Size, StyledExt, h_flex};
use std::cell::RefCell;

type TabClickFn = Rc<dyn Fn(&usize, &ClickEvent, &mut Window, &mut App) + 'static>;

/// Snapshot of the tabs shown by the overflow menu. Shared with the menu's
/// close buttons so a closed tab can be dropped from the open menu without
/// re-rendering the tab bar.
struct OverflowMenuState {
    items: Vec<MenuItemInfo>,
    selected_index: Option<usize>,
}

impl OverflowMenuState {
    /// Mirror what closing tab `ix` does to the tab list: later tabs shift
    /// down one, and a selected tab that was removed hands selection to the
    /// tab now at its index (or the last one).
    fn remove(&mut self, ix: usize) {
        if ix >= self.items.len() {
            return;
        }
        self.items.remove(ix);
        self.selected_index = self.selected_index.map(|selected| {
            if selected > ix {
                selected - 1
            } else {
                selected.min(self.items.len().saturating_sub(1))
            }
        });
    }
}

fn build_overflow_menu(
    mut menu: PopupMenu,
    state: Rc<RefCell<OverflowMenuState>>,
    on_click: Option<TabClickFn>,
    menu_entity: WeakEntity<PopupMenu>,
) -> PopupMenu {
    menu = menu.scrollable(true).min_w(px(280.));

    let (items, selected_index) = {
        let state = state.borrow();
        (state.items.clone(), state.selected_index)
    };

    let mut groups: Vec<(Option<SharedString>, Option<GroupLabelFn>, Vec<usize>)> = Vec::new();
    for (ix, item) in items.iter().enumerate() {
        let key = item.group.clone();
        if let Some((_, header, indices)) = groups.iter_mut().find(|(k, _, _)| k == &key) {
            indices.push(ix);
            if header.is_none() {
                *header = item.group_label.clone();
            }
        } else {
            groups.push((key, item.group_label.clone(), vec![ix]));
        }
    }

    let render_grouped = groups.len() > 1 || groups.iter().any(|(k, _, _)| k.is_some());

    for (group_ix, (group_key, group_label, indices)) in groups.into_iter().enumerate() {
        if render_grouped {
            if group_ix > 0 {
                menu = menu.separator();
            }
            if let Some(builder) = group_label {
                menu = menu.item(
                    PopupMenuItem::element(move |window, cx| builder(window, cx)).disabled(true),
                );
            } else {
                let header = group_key.unwrap_or_else(|| "Other".into());
                menu = menu.label(header);
            }
        }

        for ix in indices {
            let Some(item) = items.get(ix) else {
                continue;
            };
            let label = item.label.clone().unwrap_or_default();
            let menu_detail = item.menu_detail.clone();
            let on_close = item.on_close.clone();
            let state = state.clone();
            let on_click_for_rebuild = on_click.clone();
            let menu_entity = menu_entity.clone();
            menu = menu.item(
                PopupMenuItem::element(move |_, cx| {
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(label.clone()),
                        )
                        .when_some(menu_detail.clone(), |this, detail| {
                            this.child(
                                div()
                                    .text_xs()
                                    .whitespace_nowrap()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(detail),
                            )
                        })
                        .when_some(on_close.clone(), |this, on_close| {
                            let state = state.clone();
                            let on_click = on_click_for_rebuild.clone();
                            let menu_entity = menu_entity.clone();
                            this.child(
                                div()
                                    .id(("close-menu-tab", ix))
                                    .flex_shrink_0()
                                    .p_0p5()
                                    .rounded(cx.theme().radius)
                                    .cursor_pointer()
                                    .text_color(cx.theme().muted_foreground)
                                    .hover(|this| {
                                        this.bg(cx.theme().secondary_hover)
                                            .text_color(cx.theme().foreground)
                                    })
                                    .child(Icon::new(IconName::Close).xsmall())
                                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                        // The row activates its tab on mouse down as
                                        // well as click, so swallow both here.
                                        cx.stop_propagation();
                                    })
                                    .on_click(move |event, window, cx| {
                                        cx.stop_propagation();
                                        on_close(&ix, event, window, cx);
                                        state.borrow_mut().remove(ix);
                                        // Keep the menu open so several tabs can be
                                        // closed in a row, but rebuild it so the
                                        // remaining rows point at their new indices.
                                        menu_entity
                                            .update(cx, |menu, cx| {
                                                let state = state.clone();
                                                let on_click = on_click.clone();
                                                let menu_entity = cx.entity().downgrade();
                                                menu.rebuild(window, cx, move |menu, _, _| {
                                                    build_overflow_menu(
                                                        menu,
                                                        state,
                                                        on_click,
                                                        menu_entity,
                                                    )
                                                });
                                            })
                                            .ok();
                                    }),
                            )
                        })
                })
                .checked(selected_index == Some(ix))
                .disabled(item.disabled)
                .when_some(on_click.clone(), |this, on_click| {
                    this.on_click(move |event: &ClickEvent, window, cx| {
                        on_click(&ix, event, window, cx)
                    })
                }),
            );
        }
    }

    menu
}

/// A TabBar element that contains multiple [`Tab`] items.
#[derive(IntoElement)]
pub struct TabBar {
    base: Stateful<Div>,
    style: StyleRefinement,
    scroll_handle: Option<ScrollHandle>,
    prefix: Option<AnyElement>,
    suffix: Option<AnyElement>,
    children: SmallVec<[Tab; 2]>,
    last_empty_space: AnyElement,
    selected_index: Option<usize>,
    variant: TabVariant,
    size: Size,
    menu: bool,
    on_click: Option<Rc<dyn Fn(&usize, &ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl TabBar {
    /// Create a new TabBar.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            base: div().id(id).px(px(-1.)),
            style: StyleRefinement::default(),
            children: SmallVec::new(),
            scroll_handle: None,
            prefix: None,
            suffix: None,
            variant: TabVariant::default(),
            size: Size::default(),
            last_empty_space: div().w_3().into_any_element(),
            selected_index: None,
            on_click: None,
            menu: false,
        }
    }

    /// Set the Tab variant, all children will inherit the variant.
    pub fn with_variant(mut self, variant: TabVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Set the Tab variant to Pill, all children will inherit the variant.
    pub fn pill(mut self) -> Self {
        self.variant = TabVariant::Pill;
        self
    }

    /// Set the Tab variant to Outline, all children will inherit the variant.
    pub fn outline(mut self) -> Self {
        self.variant = TabVariant::Outline;
        self
    }

    /// Set the Tab variant to Segmented, all children will inherit the variant.
    pub fn segmented(mut self) -> Self {
        self.variant = TabVariant::Segmented;
        self
    }

    /// Set the Tab variant to Underline, all children will inherit the variant.
    pub fn underline(mut self) -> Self {
        self.variant = TabVariant::Underline;
        self
    }

    /// Set whether to show the menu button when tabs overflow, default is false.
    pub fn menu(mut self, menu: bool) -> Self {
        self.menu = menu;
        self
    }

    /// Track the scroll of the TabBar.
    pub fn track_scroll(mut self, scroll_handle: &ScrollHandle) -> Self {
        self.scroll_handle = Some(scroll_handle.clone());
        self
    }

    /// Set the prefix element of the TabBar
    pub fn prefix(mut self, prefix: impl IntoElement) -> Self {
        self.prefix = Some(prefix.into_any_element());
        self
    }

    /// Set the suffix element of the TabBar
    pub fn suffix(mut self, suffix: impl IntoElement) -> Self {
        self.suffix = Some(suffix.into_any_element());
        self
    }

    /// Add children of the TabBar, all children will inherit the variant.
    pub fn children(mut self, children: impl IntoIterator<Item = impl Into<Tab>>) -> Self {
        self.children.extend(children.into_iter().map(Into::into));
        self
    }

    /// Add child of the TabBar, tab will inherit the variant.
    pub fn child(mut self, child: impl Into<Tab>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Set the selected index of the TabBar.
    pub fn selected_index(mut self, index: usize) -> Self {
        self.selected_index = Some(index);
        self
    }

    /// Set the last empty space element of the TabBar.
    pub fn last_empty_space(mut self, last_empty_space: impl IntoElement) -> Self {
        self.last_empty_space = last_empty_space.into_any_element();
        self
    }

    /// Set the on_click callback of the TabBar, the first parameter is the index of the clicked tab.
    ///
    /// When this is set, the children's on_click will be ignored.
    pub fn on_click<F>(mut self, on_click: F) -> Self
    where
        F: Fn(&usize, &ClickEvent, &mut Window, &mut App) + 'static,
    {
        self.on_click = Some(Rc::new(on_click));
        self
    }
}

impl Styled for TabBar {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Sizable for TabBar {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self
    }
}

impl RenderOnce for TabBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let default_gap = match self.size {
            Size::Small | Size::XSmall => px(8.),
            Size::Large => px(16.),
            _ => px(12.),
        };
        let (bg, paddings, gap) = match self.variant {
            TabVariant::Tab => {
                let padding = Edges::all(px(0.));
                (cx.theme().tab_bar, padding, px(0.))
            }
            TabVariant::Outline => {
                let padding = Edges::all(px(0.));
                (cx.theme().transparent, padding, default_gap)
            }
            TabVariant::Pill => {
                let padding = Edges::all(px(0.));
                (cx.theme().transparent, padding, px(4.))
            }
            TabVariant::Segmented => {
                let padding_x = match self.size {
                    Size::XSmall => px(2.),
                    Size::Small => px(3.),
                    _ => px(4.),
                };
                let padding = Edges {
                    left: padding_x,
                    right: padding_x,
                    ..Default::default()
                };

                (cx.theme().tab_bar_segmented, padding, px(2.))
            }
            TabVariant::Underline => {
                // This gap is same as the tab inner_paddings
                let gap = match self.size {
                    Size::XSmall => px(10.),
                    Size::Small => px(12.),
                    Size::Large => px(20.),
                    _ => px(16.),
                };

                (cx.theme().transparent, Edges::all(px(0.)), gap)
            }
        };

        let mut item_labels: Vec<MenuItemInfo> = Vec::new();
        let selected_index = self.selected_index;
        let on_click = self.on_click.clone();

        self.base
            .group("tab-bar")
            .relative()
            .flex()
            .items_center()
            .bg(bg)
            .text_color(cx.theme().tab_foreground)
            .when(
                self.variant == TabVariant::Underline || self.variant == TabVariant::Tab,
                |this| {
                    this.child(
                        div()
                            .id("border-b")
                            .absolute()
                            .left_0()
                            .bottom_0()
                            .size_full()
                            .border_b_1()
                            .border_color(cx.theme().border),
                    )
                },
            )
            .rounded(self.variant.tab_bar_radius(self.size, cx))
            .paddings(paddings)
            .refine_style(&self.style)
            .when_some(self.prefix, |this, prefix| this.child(prefix))
            .child(
                h_flex()
                    .id("tabs")
                    .flex_1()
                    .overflow_x_scroll()
                    .when_some(self.scroll_handle, |this, scroll_handle| {
                        this.track_scroll(&scroll_handle)
                    })
                    .gap(gap)
                    .children(self.children.into_iter().enumerate().map(|(ix, child)| {
                        item_labels.push(MenuItemInfo {
                            label: child.label.clone(),
                            disabled: child.disabled,
                            group: child.group.clone(),
                            group_label: child.group_label.clone(),
                            menu_detail: child.menu_detail.clone(),
                            on_close: child.on_close.clone(),
                        });
                        let tab_bar_prefix = child.tab_bar_prefix.unwrap_or(true);
                        child
                            .ix(ix)
                            .tab_bar_prefix(tab_bar_prefix)
                            .with_variant(self.variant)
                            .with_size(self.size)
                            .when_some(self.selected_index, |this, selected_ix| {
                                this.selected(selected_ix == ix)
                            })
                            .when_some(self.on_click.clone(), move |this, on_click| {
                                this.on_click(move |event: &ClickEvent, window, cx| {
                                    on_click(&ix, event, window, cx)
                                })
                            })
                    }))
                    .when(self.suffix.is_some() || self.menu, |this| {
                        this.child(self.last_empty_space)
                    }),
            )
            .when(self.menu, |this| {
                this.child(
                    Button::new("more")
                        .xsmall()
                        .ghost()
                        .icon(IconName::ChevronDown)
                        .dropdown_menu(move |menu, _, cx| {
                            let state = Rc::new(RefCell::new(OverflowMenuState {
                                items: item_labels.clone(),
                                selected_index,
                            }));
                            build_overflow_menu(
                                menu,
                                state,
                                on_click.clone(),
                                cx.entity().downgrade(),
                            )
                        })
                        .anchor(Anchor::TopRight),
                )
            })
            .when_some(self.suffix, |this, suffix| this.child(suffix))
    }
}
