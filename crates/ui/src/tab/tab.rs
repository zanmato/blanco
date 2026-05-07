use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Bounds, ClickEvent, Div, Edges, Hsla, InteractiveElement, IntoElement,
    MouseButton, ParentElement, PathBuilder, Pixels, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Window, canvas, div, point, px, relative,
};
use gpui_component::{ActiveTheme, Icon, IconName, Selectable, Sizable, Size, StyledExt, h_flex};

/// Tab variants.
#[derive(Debug, Clone, Default, Copy, PartialEq, Eq, Hash)]
pub enum TabVariant {
    #[default]
    Tab,
    Outline,
    Pill,
    Segmented,
    Underline,
}

impl TabVariant {
    fn height(&self, size: Size) -> Pixels {
        match size {
            Size::XSmall => match self {
                TabVariant::Underline => px(26.),
                _ => px(20.),
            },
            Size::Small => match self {
                TabVariant::Underline => px(30.),
                _ => px(24.),
            },
            Size::Large => match self {
                TabVariant::Underline => px(44.),
                _ => px(36.),
            },
            _ => match self {
                TabVariant::Underline => px(36.),
                _ => px(32.),
            },
        }
    }

    fn inner_height(&self, size: Size) -> Pixels {
        match size {
            Size::XSmall => match self {
                TabVariant::Tab | TabVariant::Outline | TabVariant::Pill => px(18.),
                TabVariant::Segmented => px(16.),
                TabVariant::Underline => px(20.),
            },
            Size::Small => match self {
                TabVariant::Tab | TabVariant::Outline | TabVariant::Pill => px(22.),
                TabVariant::Segmented => px(18.),
                TabVariant::Underline => px(22.),
            },
            Size::Large => match self {
                TabVariant::Tab | TabVariant::Outline | TabVariant::Pill => px(36.),
                TabVariant::Segmented => px(28.),
                TabVariant::Underline => px(32.),
            },
            _ => match self {
                TabVariant::Tab => px(30.),
                TabVariant::Outline | TabVariant::Pill => px(26.),
                TabVariant::Segmented => px(24.),
                TabVariant::Underline => px(26.),
            },
        }
    }

    /// Default px(12) to match panel px_3, See [`crate::dock::TabPanel`]
    fn inner_paddings(&self, size: Size) -> Edges<Pixels> {
        let mut padding_x = match size {
            Size::XSmall => px(8.),
            Size::Small => px(10.),
            Size::Large => px(16.),
            _ => px(12.),
        };

        if matches!(self, TabVariant::Underline) {
            padding_x = px(0.);
        }

        Edges {
            left: padding_x,
            right: padding_x,
            ..Default::default()
        }
    }

    fn inner_margins(&self, size: Size) -> Edges<Pixels> {
        match size {
            Size::XSmall => match self {
                TabVariant::Underline => Edges {
                    top: px(1.),
                    bottom: px(2.),
                    ..Default::default()
                },
                _ => Edges::all(px(0.)),
            },
            Size::Small => match self {
                TabVariant::Underline => Edges {
                    top: px(2.),
                    bottom: px(3.),
                    ..Default::default()
                },
                _ => Edges::all(px(0.)),
            },
            Size::Large => match self {
                TabVariant::Underline => Edges {
                    top: px(5.),
                    bottom: px(6.),
                    ..Default::default()
                },
                _ => Edges::all(px(0.)),
            },
            _ => match self {
                TabVariant::Underline => Edges {
                    top: px(3.),
                    bottom: px(4.),
                    ..Default::default()
                },
                _ => Edges::all(px(0.)),
            },
        }
    }

    fn normal(&self, cx: &App) -> TabStyle {
        match self {
            TabVariant::Tab => TabStyle {
                fg: cx.theme().tab_foreground,
                bg: cx.theme().transparent,
                borders: Edges {
                    left: px(1.),
                    right: px(1.),
                    ..Default::default()
                },
                border_color: cx.theme().transparent,
                ..Default::default()
            },
            TabVariant::Outline => TabStyle {
                fg: cx.theme().tab_foreground,
                bg: cx.theme().transparent,
                borders: Edges::all(px(1.)),
                border_color: cx.theme().border,
                ..Default::default()
            },
            TabVariant::Pill => TabStyle {
                fg: cx.theme().foreground,
                bg: cx.theme().transparent,
                ..Default::default()
            },
            TabVariant::Segmented => TabStyle {
                fg: cx.theme().tab_foreground,
                bg: cx.theme().transparent,
                ..Default::default()
            },
            TabVariant::Underline => TabStyle {
                fg: cx.theme().tab_foreground,
                bg: cx.theme().transparent,
                inner_bg: cx.theme().transparent,
                borders: Edges {
                    bottom: px(2.),
                    ..Default::default()
                },
                border_color: cx.theme().transparent,
                ..Default::default()
            },
        }
    }

    fn hovered(&self, selected: bool, cx: &App) -> TabStyle {
        match self {
            TabVariant::Tab => TabStyle {
                fg: cx.theme().tab_active_foreground,
                bg: cx.theme().transparent,
                borders: Edges {
                    left: px(1.),
                    right: px(1.),
                    ..Default::default()
                },
                border_color: cx.theme().transparent,
                ..Default::default()
            },
            TabVariant::Outline => TabStyle {
                fg: cx.theme().secondary_foreground,
                bg: cx.theme().secondary_hover,
                borders: Edges::all(px(1.)),
                border_color: cx.theme().border,
                ..Default::default()
            },
            TabVariant::Pill => TabStyle {
                fg: cx.theme().secondary_foreground,
                bg: cx.theme().secondary,
                ..Default::default()
            },
            TabVariant::Segmented => TabStyle {
                fg: cx.theme().tab_active_foreground,
                bg: cx.theme().transparent,
                inner_bg: if selected {
                    cx.theme().background
                } else {
                    cx.theme().transparent
                },
                ..Default::default()
            },
            TabVariant::Underline => TabStyle {
                fg: cx.theme().tab_active_foreground,
                bg: cx.theme().transparent,
                inner_bg: cx.theme().transparent,
                borders: Edges {
                    bottom: px(2.),
                    ..Default::default()
                },
                border_color: cx.theme().transparent,
                ..Default::default()
            },
        }
    }

    fn selected(&self, cx: &App) -> TabStyle {
        match self {
            TabVariant::Tab => TabStyle {
                fg: cx.theme().tab_active_foreground,
                bg: cx.theme().tab_active,
                borders: Edges {
                    left: px(1.),
                    right: px(1.),
                    ..Default::default()
                },
                border_color: cx.theme().border,
                ..Default::default()
            },
            TabVariant::Outline => TabStyle {
                fg: cx.theme().primary,
                bg: cx.theme().transparent,
                borders: Edges::all(px(1.)),
                border_color: cx.theme().primary,
                ..Default::default()
            },
            TabVariant::Pill => TabStyle {
                fg: cx.theme().primary_foreground,
                bg: cx.theme().primary,
                ..Default::default()
            },
            TabVariant::Segmented => TabStyle {
                fg: cx.theme().tab_active_foreground,
                bg: cx.theme().transparent,
                inner_bg: cx.theme().background,
                shadow: true,
                ..Default::default()
            },
            TabVariant::Underline => TabStyle {
                fg: cx.theme().tab_active_foreground,
                bg: cx.theme().transparent,
                borders: Edges {
                    bottom: px(2.),
                    ..Default::default()
                },
                border_color: cx.theme().primary,
                ..Default::default()
            },
        }
    }

    fn disabled(&self, selected: bool, cx: &App) -> TabStyle {
        match self {
            TabVariant::Tab => TabStyle {
                fg: cx.theme().muted_foreground,
                bg: cx.theme().transparent,
                border_color: if selected {
                    cx.theme().border
                } else {
                    cx.theme().transparent
                },
                borders: Edges {
                    left: px(1.),
                    right: px(1.),
                    ..Default::default()
                },
                ..Default::default()
            },
            TabVariant::Outline => TabStyle {
                fg: cx.theme().muted_foreground,
                bg: cx.theme().transparent,
                borders: Edges::all(px(1.)),
                border_color: if selected {
                    cx.theme().primary
                } else {
                    cx.theme().border
                },
                ..Default::default()
            },
            TabVariant::Pill => TabStyle {
                fg: if selected {
                    cx.theme().primary_foreground.opacity(0.5)
                } else {
                    cx.theme().muted_foreground
                },
                bg: if selected {
                    cx.theme().primary.opacity(0.5)
                } else {
                    cx.theme().transparent
                },
                ..Default::default()
            },
            TabVariant::Segmented => TabStyle {
                fg: cx.theme().muted_foreground,
                bg: cx.theme().tab_bar,
                inner_bg: if selected {
                    cx.theme().background
                } else {
                    cx.theme().transparent
                },
                ..Default::default()
            },
            TabVariant::Underline => TabStyle {
                fg: cx.theme().muted_foreground,
                bg: cx.theme().transparent,
                border_color: if selected {
                    cx.theme().border
                } else {
                    cx.theme().transparent
                },
                borders: Edges {
                    bottom: px(2.),
                    ..Default::default()
                },
                ..Default::default()
            },
        }
    }

    pub(super) fn tab_bar_radius(&self, size: Size, cx: &App) -> Pixels {
        if *self != TabVariant::Segmented {
            return px(0.);
        }

        match size {
            Size::XSmall | Size::Small => cx.theme().radius,
            Size::Large => cx.theme().radius_lg,
            _ => cx.theme().radius_lg,
        }
    }

    fn radius(&self, size: Size, cx: &App) -> Pixels {
        match self {
            TabVariant::Outline | TabVariant::Pill => px(99.),
            TabVariant::Segmented => match size {
                Size::XSmall | Size::Small => cx.theme().radius,
                Size::Large => cx.theme().radius_lg,
                _ => cx.theme().radius_lg,
            },
            _ => px(0.),
        }
    }

    fn inner_radius(&self, size: Size, cx: &App) -> Pixels {
        match self {
            TabVariant::Segmented => match size {
                Size::Large => self.tab_bar_radius(size, cx) - px(3.),
                _ => self.tab_bar_radius(size, cx) - px(2.),
            },
            _ => px(0.),
        }
    }
}

#[allow(dead_code)]
struct TabStyle {
    borders: Edges<Pixels>,
    border_color: Hsla,
    bg: Hsla,
    fg: Hsla,
    shadow: bool,
    inner_bg: Hsla,
}

impl Default for TabStyle {
    fn default() -> Self {
        TabStyle {
            borders: Edges::all(px(0.)),
            border_color: gpui::transparent_white(),
            bg: gpui::transparent_white(),
            fg: gpui::transparent_white(),
            shadow: false,
            inner_bg: gpui::transparent_white(),
        }
    }
}

const CHROME_TOP_RADIUS: Pixels = px(4.);
const CHROME_FOOT_W: Pixels = px(8.);
const CHROME_FOOT_H: Pixels = px(8.);

fn chrome_tab_fill_path(bounds: &Bounds<Pixels>) -> gpui::Path<Pixels> {
    let x = bounds.origin.x;
    let y = bounds.origin.y;
    let w = bounds.size.width;
    let h = bounds.size.height;

    let top_r = CHROME_TOP_RADIUS;
    let foot_w = CHROME_FOOT_W;
    let foot_h = CHROME_FOOT_H;

    let bx = x + foot_w;
    let bw = w - foot_w * 2.;

    let mut builder = PathBuilder::fill();

    builder.move_to(point(bx + top_r, y));
    builder.line_to(point(bx + bw - top_r, y));
    builder.curve_to(point(bx + bw, y + top_r), point(bx + bw, y));
    builder.line_to(point(bx + bw, y + h - foot_h));
    builder.curve_to(point(bx + bw + foot_w, y + h), point(bx + bw, y + h));
    builder.line_to(point(bx - foot_w, y + h));
    builder.curve_to(point(bx, y + h - foot_h), point(bx, y + h));
    builder.line_to(point(bx, y + top_r));
    builder.curve_to(point(bx + top_r, y), point(bx, y));
    builder.close();

    builder
        .build()
        .unwrap_or_else(|_| PathBuilder::fill().build().unwrap())
}

fn chrome_tab_border_path(bounds: &Bounds<Pixels>, include_left: bool) -> gpui::Path<Pixels> {
    let x = bounds.origin.x + px(0.5);
    let y = bounds.origin.y + px(0.5);
    let w = bounds.size.width - px(1.);
    let h = bounds.size.height - px(1.);

    let top_r = CHROME_TOP_RADIUS;
    let foot_w = CHROME_FOOT_W;
    let foot_h = CHROME_FOOT_H;

    let bx = x + foot_w;
    let bw = w - foot_w * 2.;

    let mut builder = PathBuilder::stroke(px(1.));

    if include_left {
        builder.move_to(point(bx - foot_w, y + h));
        builder.curve_to(point(bx, y + h - foot_h), point(bx, y + h));
        builder.line_to(point(bx, y + top_r));
        builder.curve_to(point(bx + top_r, y), point(bx, y));
    } else {
        builder.move_to(point(bx + top_r, y));
    }
    builder.line_to(point(bx + bw - top_r, y));
    builder.curve_to(point(bx + bw, y + top_r), point(bx + bw, y));
    builder.line_to(point(bx + bw, y + h - foot_h));
    builder.curve_to(point(bx + bw + foot_w, y + h), point(bx + bw, y + h));

    builder
        .build()
        .unwrap_or_else(|_| PathBuilder::stroke(px(1.)).build().unwrap())
}

/// A Tab element for the [`super::TabBar`].
#[derive(IntoElement)]
pub struct Tab {
    ix: usize,
    base: Div,
    pub(super) label: Option<SharedString>,
    icon: Option<Icon>,
    prefix: Option<AnyElement>,
    pub(super) tab_bar_prefix: Option<bool>,
    suffix: Option<AnyElement>,
    children: Vec<AnyElement>,
    variant: TabVariant,
    size: Size,
    pub(super) disabled: bool,
    pub(super) selected: bool,
    pub(super) group: Option<SharedString>,
    pub(super) group_label: Option<Rc<dyn Fn(&mut Window, &mut App) -> AnyElement + 'static>>,
    on_click: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl From<&'static str> for Tab {
    fn from(label: &'static str) -> Self {
        Self::new().label(label)
    }
}

impl From<String> for Tab {
    fn from(label: String) -> Self {
        Self::new().label(label)
    }
}

impl From<SharedString> for Tab {
    fn from(label: SharedString) -> Self {
        Self::new().label(label)
    }
}

impl From<Icon> for Tab {
    fn from(icon: Icon) -> Self {
        Self::default().icon(icon)
    }
}

impl From<IconName> for Tab {
    fn from(icon_name: IconName) -> Self {
        Self::default().icon(Icon::new(icon_name))
    }
}

impl Default for Tab {
    fn default() -> Self {
        Self {
            ix: 0,
            base: div(),
            label: None,
            icon: None,
            tab_bar_prefix: None,
            children: Vec::new(),
            disabled: false,
            selected: false,
            group: None,
            group_label: None,
            prefix: None,
            suffix: None,
            variant: TabVariant::default(),
            size: Size::default(),
            on_click: None,
        }
    }
}

impl Tab {
    /// Create a new tab with a label.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set label for the tab.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set icon for the tab.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Set Tab Variant.
    pub fn with_variant(mut self, variant: TabVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Use Pill variant.
    pub fn pill(mut self) -> Self {
        self.variant = TabVariant::Pill;
        self
    }

    /// Use outline variant.
    pub fn outline(mut self) -> Self {
        self.variant = TabVariant::Outline;
        self
    }

    /// Use Segmented variant.
    pub fn segmented(mut self) -> Self {
        self.variant = TabVariant::Segmented;
        self
    }

    /// Use Underline variant.
    pub fn underline(mut self) -> Self {
        self.variant = TabVariant::Underline;
        self
    }

    /// Set the left side of the tab
    pub fn prefix(mut self, prefix: impl IntoElement) -> Self {
        self.prefix = Some(prefix.into_any_element());
        self
    }

    /// Set the right side of the tab
    pub fn suffix(mut self, suffix: impl IntoElement) -> Self {
        self.suffix = Some(suffix.into_any_element());
        self
    }

    /// Set the group key used by [`super::TabBar`] when rendering its
    /// overflow menu. Tabs sharing a group key are clustered under a labeled
    /// section in the dropdown.
    pub fn group(mut self, group: impl Into<SharedString>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// Provide a custom render for the group header in [`super::TabBar`]'s
    /// overflow menu. Only the first non-`None` renderer encountered in a
    /// group is used; tabs without a renderer fall back to plain text using
    /// the group key.
    pub fn group_label<F, E>(mut self, builder: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        self.group_label = Some(Rc::new(move |window, cx| {
            builder(window, cx).into_any_element()
        }));
        self
    }

    /// Set disabled state to the tab, default false.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Set the click handler for the tab.
    pub fn on_click(
        mut self,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(on_click));
        self
    }

    /// Set index to the tab.
    pub(crate) fn ix(mut self, ix: usize) -> Self {
        self.ix = ix;
        self
    }

    /// Set if the tab bar has a prefix.
    pub(crate) fn tab_bar_prefix(mut self, tab_bar_prefix: bool) -> Self {
        self.tab_bar_prefix = Some(tab_bar_prefix);
        self
    }
}

impl ParentElement for Tab {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Selectable for Tab {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl InteractiveElement for Tab {
    fn interactivity(&mut self) -> &mut gpui::Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for Tab {}

impl Styled for Tab {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        self.base.style()
    }
}

impl Sizable for Tab {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self
    }
}

impl RenderOnce for Tab {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let mut tab_style = if self.selected {
            self.variant.selected(cx)
        } else {
            self.variant.normal(cx)
        };
        let mut hover_style = self.variant.hovered(self.selected, cx);
        if self.disabled {
            tab_style = self.variant.disabled(self.selected, cx);
            hover_style = self.variant.disabled(self.selected, cx);
        }
        let tab_bar_prefix = self.tab_bar_prefix.unwrap_or_default();
        if !tab_bar_prefix {
            if self.ix == 0 && self.variant == TabVariant::Tab {
                tab_style.borders.left = px(0.);
                hover_style.borders.left = px(0.);
            }
        }
        let radius = self.variant.radius(self.size, cx);
        let inner_radius = self.variant.inner_radius(self.size, cx);
        let inner_paddings = self.variant.inner_paddings(self.size);
        let inner_margins = self.variant.inner_margins(self.size);
        let inner_height = self.variant.inner_height(self.size);
        let height = self.variant.height(self.size);

        let use_chrome_shape = self.selected && self.variant == TabVariant::Tab && !self.disabled;

        let inner_content = h_flex()
            .flex_1()
            .h(inner_height)
            .line_height(relative(1.))
            .whitespace_nowrap()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .margins(inner_margins)
            .flex_shrink_0()
            .map(|this| match self.icon {
                Some(icon) => this
                    .w(inner_height * 1.25)
                    .child(icon.map(|this| match self.size {
                        Size::XSmall => this.size_2p5(),
                        Size::Small => this.size_3p5(),
                        Size::Large => this.size_4(),
                        _ => this.size_4(),
                    })),
                None => this
                    .paddings(inner_paddings)
                    .map(|this| match self.label {
                        Some(label) => this.child(label),
                        None => this,
                    })
                    .children(self.children),
            })
            .bg(tab_style.inner_bg)
            .rounded(inner_radius)
            .when(tab_style.shadow, |this| this.shadow_xs())
            .hover(|this| this.bg(hover_style.inner_bg).rounded(inner_radius));

        let include_left = !(self.ix == 0 && !tab_bar_prefix);

        let base = self
            .base
            .id(self.ix)
            .flex()
            .flex_shrink_0()
            .h(height)
            .text_color(tab_style.fg)
            .map(|this| match self.size {
                Size::XSmall => this.text_xs(),
                Size::Large => this.text_base(),
                _ => this.text_sm(),
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .when(!self.disabled, |this| {
                this.when_some(self.on_click.clone(), |this, on_click| {
                    this.on_click(move |event, window, cx| on_click(event, window, cx))
                })
            });

        if use_chrome_shape {
            let fill_color = tab_style.bg;
            let stroke_color = tab_style.border_color;

            base.relative()
                .child(
                    canvas(
                        move |_bounds, _window, _cx| {},
                        move |bounds, _, window, _| {
                            let fill = chrome_tab_fill_path(&bounds);
                            window.paint_path(fill, fill_color);
                            let border = chrome_tab_border_path(&bounds, include_left);
                            window.paint_path(border, stroke_color);
                        },
                    )
                    .absolute()
                    .left_0()
                    .right_0()
                    .top_0()
                    .h(height),
                )
                .child(
                    div()
                        .relative()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .items_center()
                        .size_full()
                        .px(CHROME_FOOT_W)
                        .when_some(self.prefix, |this, prefix| this.child(prefix))
                        .child(inner_content)
                        .when_some(self.suffix, |this, suffix| this.child(suffix)),
                )
        } else {
            base.flex_wrap()
                .gap_1()
                .items_center()
                .overflow_hidden()
                .when(self.variant == TabVariant::Tab, |this| {
                    this.px(CHROME_FOOT_W - px(1.))
                })
                .bg(tab_style.bg)
                .border_l(tab_style.borders.left)
                .border_r(tab_style.borders.right)
                .border_t(tab_style.borders.top)
                .border_b(tab_style.borders.bottom)
                .border_color(tab_style.border_color)
                .rounded(radius)
                .when(!self.selected && !self.disabled, |this| {
                    this.hover(|this| {
                        this.text_color(hover_style.fg)
                            .bg(hover_style.bg)
                            .border_l(hover_style.borders.left)
                            .border_r(hover_style.borders.right)
                            .border_t(hover_style.borders.top)
                            .border_b(hover_style.borders.bottom)
                            .border_color(hover_style.border_color)
                            .rounded(radius)
                    })
                })
                .when_some(self.prefix, |this, prefix| this.child(prefix))
                .child(inner_content)
                .when_some(self.suffix, |this, suffix| this.child(suffix))
        }
    }
}
