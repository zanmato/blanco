use gpui::{
    App, BorderStyle, Bounds, ContentMask, Corners, Edges, ElementId, Font, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, RenderOnce, SharedString,
    Styled, TextAlign, TextRun, Window, canvas, div, fill, font, point, px, quad,
    transparent_black,
};

/// A horizontal bordered rectangle that conveys a relative size with a fill
/// bar and a centered size label. The label color changes where it overlaps
/// the fill so the text remains legible on both halves.
#[derive(IntoElement)]
pub struct SizeIndicator {
    id: ElementId,
    text: SharedString,
    relative: f32,
    width: Pixels,
    height: Pixels,
    fill_color: Hsla,
    border_color: Hsla,
    text_color: Hsla,
    fill_text_color: Hsla,
    font_family: SharedString,
    font_size: Pixels,
    corner_radius: Pixels,
    horizontal_padding: Pixels,
}

impl SizeIndicator {
    pub fn new(id: impl Into<ElementId>, text: impl Into<SharedString>, relative: f32) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            relative: relative.clamp(0.0, 1.0),
            width: px(64.),
            height: px(14.),
            fill_color: gpui::blue(),
            border_color: transparent_black(),
            text_color: gpui::black(),
            fill_text_color: gpui::white(),
            font_family: ".SystemUIFont".into(),
            font_size: px(10.),
            corner_radius: px(3.),
            horizontal_padding: px(4.),
        }
    }

    pub fn size(mut self, width: Pixels, height: Pixels) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    pub fn fill_color(mut self, color: Hsla) -> Self {
        self.fill_color = color;
        self
    }

    pub fn border_color(mut self, color: Hsla) -> Self {
        self.border_color = color;
        self
    }

    pub fn text_color(mut self, color: Hsla) -> Self {
        self.text_color = color;
        self
    }

    pub fn fill_text_color(mut self, color: Hsla) -> Self {
        self.fill_text_color = color;
        self
    }

    pub fn font(mut self, family: impl Into<SharedString>, size: Pixels) -> Self {
        self.font_family = family.into();
        self.font_size = size;
        self
    }

    pub fn corner_radius(mut self, radius: Pixels) -> Self {
        self.corner_radius = radius;
        self
    }
}

impl RenderOnce for SizeIndicator {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let SizeIndicator {
            id,
            text,
            relative,
            width,
            height,
            fill_color,
            border_color,
            text_color,
            fill_text_color,
            font_family,
            font_size,
            corner_radius,
            horizontal_padding,
        } = self;

        div().w(width).h(height).id(id).child(
            canvas(
                |_bounds, _window, _cx| {},
                move |bounds, _prepaint, window: &mut Window, cx: &mut App| {
                    paint_size_indicator(
                        bounds,
                        relative,
                        fill_color,
                        border_color,
                        text_color,
                        fill_text_color,
                        font(font_family.clone()),
                        font_size,
                        corner_radius,
                        horizontal_padding,
                        text,
                        window,
                        cx,
                    );
                },
            )
            .size_full(),
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_size_indicator(
    bounds: Bounds<Pixels>,
    relative: f32,
    fill_color: Hsla,
    border_color: Hsla,
    text_color: Hsla,
    fill_text_color: Hsla,
    text_font: Font,
    font_size: Pixels,
    corner_radius: Pixels,
    horizontal_padding: Pixels,
    text: SharedString,
    window: &mut Window,
    cx: &mut App,
) {
    // Snap to whole pixels so the 1px border doesn't get split across two
    // rows by antialiasing when the parent's flex centering puts the bar at a
    // fractional vertical offset.
    let bounds = Bounds::from_corners(
        point(bounds.left().round(), bounds.top().round()),
        point(bounds.right().round(), bounds.bottom().round()),
    );
    let fill_width = bounds.size.width * relative.clamp(0.0, 1.0);

    // Border + transparent background frame.
    window.paint_quad(quad(
        bounds,
        Corners::all(corner_radius),
        transparent_black(),
        Edges::all(px(1.)),
        border_color,
        BorderStyle::default(),
    ));

    // Fill bar, inset 1px so it sits inside the border. Painted with the same
    // corner radius so the left-side rounding is preserved; the right side is
    // clipped by `with_content_mask` against `filled_mask` further below.
    if fill_width > px(0.) {
        let inset = px(1.);
        let inset_top = bounds.top() + inset;
        let inset_bottom = bounds.bottom() - inset;
        let inset_left = bounds.left() + inset;
        let inset_right = (bounds.left() + fill_width).min(bounds.right() - inset);
        if inset_right > inset_left && inset_bottom > inset_top {
            let bar_bounds = Bounds::from_corners(
                point(inset_left, inset_top),
                point(inset_right, inset_bottom),
            );
            window.paint_quad(fill(bar_bounds, fill_color));
        }
    }

    // Shape the label once per color so we can paint with two different
    // foregrounds and let `with_content_mask` decide which half is visible.
    let text_len = text.len();
    let muted_run = TextRun {
        len: text_len,
        font: text_font,
        color: text_color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let contrast_run = TextRun {
        color: fill_text_color,
        ..muted_run.clone()
    };
    let shaped_muted = window
        .text_system()
        .shape_line(text.clone(), font_size, &[muted_run], None);
    let shaped_contrast =
        window
            .text_system()
            .shape_line(text, font_size, &[contrast_run], None);

    // Right-align text inside the bar with `horizontal_padding` of right inset.
    let text_origin = point(
        bounds.right() - horizontal_padding - shaped_muted.width,
        bounds.top(),
    );
    let line_height = bounds.size.height;

    // Unfilled half: muted color.
    let unfilled_origin_x = bounds.left() + fill_width;
    if unfilled_origin_x < bounds.right() {
        let mask_bounds = Bounds::from_corners(
            point(unfilled_origin_x, bounds.top()),
            point(bounds.right(), bounds.bottom()),
        );
        window.with_content_mask(
            Some(ContentMask {
                bounds: mask_bounds,
            }),
            |window| {
                let _ =
                    shaped_muted.paint(text_origin, line_height, TextAlign::Left, None, window, cx);
            },
        );
    }

    // Filled half: contrasting color.
    if fill_width > px(0.) {
        let mask_bounds = Bounds::from_corners(
            point(bounds.left(), bounds.top()),
            point(bounds.left() + fill_width, bounds.bottom()),
        );
        window.with_content_mask(
            Some(ContentMask {
                bounds: mask_bounds,
            }),
            |window| {
                let _ = shaped_contrast.paint(
                    text_origin,
                    line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            },
        );
    }
}
