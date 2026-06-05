use gpui::ClickEvent;
use gpui::{
    AnyElement, Anchor, App, Div, Edges, ElementId, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, ScrollHandle, Stateful, StatefulInteractiveElement as _, StyleRefinement, Styled,
    Window, div, prelude::FluentBuilder as _, px,
};
type GroupLabelFn = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement + 'static>;
use smallvec::SmallVec;
use std::rc::Rc;

use super::{Tab, TabVariant};
use gpui::SharedString;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::{ActiveTheme, IconName, Selectable, Sizable, Size, StyledExt, h_flex};

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

        let mut item_labels: Vec<(
            Option<SharedString>,
            bool,
            Option<SharedString>,
            Option<GroupLabelFn>,
        )> = Vec::new();
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
                        item_labels.push((
                            child.label.clone(),
                            child.disabled,
                            child.group.clone(),
                            child.group_label.clone(),
                        ));
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
                        .dropdown_menu(move |mut this, _, _| {
                            this = this.scrollable(true);

                            let mut groups: Vec<(
                                Option<SharedString>,
                                Option<GroupLabelFn>,
                                Vec<usize>,
                            )> = Vec::new();
                            for (ix, (_, _, group, group_label)) in item_labels.iter().enumerate() {
                                let key = group.clone();
                                if let Some((_, header, indices)) =
                                    groups.iter_mut().find(|(k, _, _)| k == &key)
                                {
                                    indices.push(ix);
                                    if header.is_none() {
                                        *header = group_label.clone();
                                    }
                                } else {
                                    groups.push((key, group_label.clone(), vec![ix]));
                                }
                            }

                            let render_grouped =
                                groups.len() > 1 || groups.iter().any(|(k, _, _)| k.is_some());

                            for (group_ix, (group_key, group_label, indices)) in
                                groups.into_iter().enumerate()
                            {
                                if render_grouped {
                                    if group_ix > 0 {
                                        this = this.separator();
                                    }
                                    if let Some(builder) = group_label {
                                        this = this.item(
                                            PopupMenuItem::element(move |window, cx| {
                                                builder(window, cx)
                                            })
                                            .disabled(true),
                                        );
                                    } else {
                                        let header = group_key.unwrap_or_else(|| "Other".into());
                                        this = this.label(header);
                                    }
                                }

                                for ix in indices {
                                    let (label, disabled, _, _) = &item_labels[ix];
                                    this = this.item(
                                        PopupMenuItem::new(label.clone().unwrap_or_default())
                                            .checked(selected_index == Some(ix))
                                            .disabled(*disabled)
                                            .when_some(on_click.clone(), |this, on_click| {
                                                this.on_click(
                                                    move |event: &ClickEvent, window, cx| {
                                                        on_click(&ix, event, window, cx)
                                                    },
                                                )
                                            }),
                                    );
                                }
                            }

                            this
                        })
                        .anchor(Anchor::TopRight),
                )
            })
            .when_some(self.suffix, |this, suffix| this.child(suffix))
    }
}
