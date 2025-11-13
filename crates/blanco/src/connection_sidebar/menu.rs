use gpui::{
    AnyElement, App, ClickEvent, Context, ElementId, FocusHandle, InteractiveElement, IntoElement,
    ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, div,
    percentage, prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, Collapsible, StyledExt, h_flex, v_flex};
use std::rc::Rc;

// Import context menu components
use blanco_ui::IconName;
use gpui_component::Icon;
use gpui_component::{menu::ContextMenuExt, menu::PopupMenu};

// Type aliases to reduce complexity
type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type ContextMenuBuilder =
    Option<Box<dyn Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static>>;

#[derive(IntoElement)]
pub struct SidebarMenu {
    collapsed: bool,
    items: Vec<SidebarMenuItem>,
}

impl SidebarMenu {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            collapsed: false,
        }
    }

    pub fn child(mut self, child: impl Into<SidebarMenuItem>) -> Self {
        self.items.push(child.into());
        self
    }

    pub fn children(
        mut self,
        children: impl IntoIterator<Item = impl Into<SidebarMenuItem>>,
    ) -> Self {
        self.items = children.into_iter().map(Into::into).collect();
        self
    }
}
impl Collapsible for SidebarMenu {
    fn is_collapsed(&self) -> bool {
        self.collapsed
    }

    fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }
}
impl RenderOnce for SidebarMenu {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        v_flex().gap_1().children(
            self.items
                .into_iter()
                .enumerate()
                .map(|(ix, item)| item.id(ix).collapsed(self.collapsed)),
        )
    }
}

/// A sidebar menu item
#[derive(IntoElement)]
pub struct SidebarMenuItem {
    id: ElementId,
    icon: Option<Icon>,
    label: SharedString,
    handler: ClickHandler,
    active: bool,
    collapsed: bool,
    children: Vec<Self>,
    suffix: Option<AnyElement>,
    // NEW: Add context menu support
    context_menu: ContextMenuBuilder,
}

impl Clone for SidebarMenuItem {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),     // ElementId should implement Clone
            icon: self.icon.clone(), // Icon should implement Clone
            label: self.label.clone(),
            handler: Rc::clone(&self.handler),
            active: self.active,
            collapsed: self.collapsed,
            children: self.children.clone(),
            suffix: None,       // We'll clone the suffix if needed in a future iteration
            context_menu: None, // Context menu functions are complex to clone, we'll rebuild these as needed
        }
    }
}

impl SidebarMenuItem {
    /// Create a new SidebarMenuItem with a label
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            id: ElementId::Integer(0),
            icon: None,
            label: label.into(),
            handler: Rc::new(|_, _, _| {}),
            active: false,
            collapsed: false,
            children: Vec::new(),
            suffix: None,
            context_menu: None,
        }
    }

    /// Set the icon for the menu item
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Set id to the menu item.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }

    /// Set the active state of the menu item
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Add a click handler to the menu item
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.handler = Rc::new(handler);
        self
    }

    /// Set the collapsed state of the menu item
    pub fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }

    pub fn children(mut self, children: impl IntoIterator<Item = impl Into<Self>>) -> Self {
        self.children = children.into_iter().map(Into::into).collect();
        self
    }

    /// Set the suffix for the menu item.
    pub fn suffix(mut self, suffix: impl IntoElement) -> Self {
        self.suffix = Some(suffix.into_any_element());
        self
    }

    /// Set a context menu for the menu item.
    pub fn context_menu<F>(mut self, builder: F) -> Self
    where
        F: Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    {
        self.context_menu = Some(Box::new(builder));
        self
    }

    /// Set a context menu for the menu item with action context.
    pub fn context_menu_with_focus<F>(mut self, focus_handle: FocusHandle, builder: F) -> Self
    where
        F: Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    {
        self.context_menu = Some(Box::new(move |menu, window, cx| {
            let menu_with_context = menu.action_context(focus_handle.clone());
            builder(menu_with_context, window, cx)
        }));
        self
    }

    fn is_submenu(&self) -> bool {
        !self.children.is_empty()
    }

    fn is_open(&self) -> bool {
        if self.is_submenu() {
            self.active
        } else {
            false
        }
    }
}

impl RenderOnce for SidebarMenuItem {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let handler = self.handler.clone();
        let is_collapsed = self.collapsed;
        let is_active = self.active;
        let is_open = self.is_open();
        let is_submenu = self.is_submenu();
        let context_menu = self.context_menu;
        let label = self.label.clone();
        let click_label = label.clone();
        let context_label = label.clone();
        let children = self.children;

        let main_item = h_flex()
            .id(self.id.clone())
            .size_full()
            .overflow_x_hidden()
            .flex_shrink_0()
            .p_2()
            .gap_x_2()
            .rounded(cx.theme().radius)
            .text_sm()
            .hover(|this| {
                if is_active {
                    return this;
                }

                this.bg(cx.theme().sidebar_accent.opacity(0.8))
                    .text_color(cx.theme().sidebar_accent_foreground)
            })
            .when(is_active && !is_submenu, |this| {
                this.font_medium()
                    .bg(cx.theme().sidebar_accent)
                    .text_color(cx.theme().sidebar_accent_foreground)
            })
            .when_some(self.icon.clone(), |this, icon| this.child(icon))
            .when(is_collapsed, |this| {
                this.justify_center().when(is_active, |this| {
                    this.bg(cx.theme().sidebar_accent)
                        .text_color(cx.theme().sidebar_accent_foreground)
                })
            })
            .when(!is_collapsed, |this| {
                this.h_7()
                    .child(
                        h_flex()
                            .flex_1()
                            .gap_x_2()
                            .justify_between()
                            .overflow_x_hidden()
                            .child(
                                h_flex()
                                    .flex_1()
                                    .overflow_x_hidden()
                                    .child(self.label.clone()),
                            )
                            .when_some(self.suffix, |this, suffix| this.child(suffix)),
                    )
                    .when(is_submenu, |this| {
                        this.child(
                            Icon::new(IconName::ChevronRight)
                                .size_4()
                                .when(is_open, |this| this.rotate(percentage(90. / 360.))),
                        )
                    })
            })
            .on_click(move |ev, window, cx| {
                log::info!("SidebarMenuItem clicked: label={}", click_label);
                handler(ev, window, cx)
            });

        div()
            .w_full()
            .child(
                main_item.context_menu(move |menu, window, cx| {
                    if let Some(menu_builder) = &context_menu {
                        log::info!("Context menu TRIGGERED for item: {}", context_label);
                        let built_menu = menu_builder(menu, window, cx);
                        log::info!("Context menu BUILT successfully for: {}", context_label);
                        built_menu
                    } else {
                        menu
                    }
                })
            )
            .when(is_submenu && is_open && !is_collapsed, |this| {
                this.child(
                    v_flex()
                        .id("submenu")
                        .border_l_1()
                        .border_color(cx.theme().sidebar_border)
                        .gap_0p5()
                        .ml_3p5()
                        .pl_2p5()
                        .py_0p5()
                        .children(
                            children
                                .into_iter()
                                .enumerate()
                                .map(|(ix, item)| item.id(ix)),
                        ),
                )
            })
    }
}
