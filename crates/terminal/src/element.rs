//! The GPUI element that paints a [`Terminal`]'s frame and routes pointer
//! input to it. Text is shaped one span at a time with the cell width forced,
//! so glyphs land on the grid whatever the font's natural advances are.

use std::ops::Range;

use gpui::{
    App, Bounds, ContentMask, Corners, DispatchPhase, Edges, Element, ElementId, Entity,
    FocusHandle, Font, FontStyle, FontWeight, GlobalElementId, Hitbox, Hsla, InputHandler,
    InspectorElementId, InteractiveElement, Interactivity, IntoElement, LayoutId, MouseButton,
    MouseMoveEvent, Pixels, Point, ShapedLine, SharedString, StrikethroughStyle, TextRun,
    UTF16Selection, UnderlineStyle, WeakEntity, Window, fill, font, point, px, quad, relative,
    size,
};

use crate::palette::TerminalPalette;
use crate::view::TerminalView;
use crate::{CursorShape, Frame, GridSize, Terminal};

/// Font and colors the host wants the grid drawn with.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalStyle {
    pub font_family: SharedString,
    pub font_size: Pixels,
    /// Line height as a multiple of the font size.
    pub line_height_scale: f32,
    pub palette: TerminalPalette,
}

impl Default for TerminalStyle {
    fn default() -> Self {
        Self {
            font_family: "monospace".into(),
            font_size: px(13.0),
            line_height_scale: 1.4,
            palette: TerminalPalette::default(),
        }
    }
}

/// Breathing room between the element edge and the grid on every side.
const GUTTER: Pixels = px(6.0);
/// Thickness of beam and underline cursors.
const THIN_CURSOR: Pixels = px(2.0);

struct ShapedSpan {
    line: ShapedLine,
    origin: Point<Pixels>,
}

struct CursorLayout {
    bounds: Bounds<Pixels>,
    shape: CursorShape,
    /// The character under a block cursor, shaped in the background color so
    /// it stays readable on top of the cursor.
    glyph: Option<ShapedLine>,
}

pub struct TerminalLayout {
    hitbox: Hitbox,
    /// Top-left of cell (0, 0) in window coordinates.
    grid_origin: Point<Pixels>,
    size: GridSize,
    background: Hsla,
    background_rects: Vec<(Bounds<Pixels>, Hsla)>,
    selection_rects: Vec<Bounds<Pixels>>,
    selection_color: Hsla,
    spans: Vec<ShapedSpan>,
    cursor: Option<CursorLayout>,
    /// Where an input method should place its candidate window.
    ime_bounds: Option<Bounds<Pixels>>,
    marked_text: Option<ShapedLine>,
}

pub struct TerminalElement {
    terminal: Entity<Terminal>,
    view: WeakEntity<TerminalView>,
    focus_handle: FocusHandle,
    focused: bool,
    cursor_visible: bool,
    style: TerminalStyle,
    marked_text: Option<String>,
    interactivity: Interactivity,
}

impl TerminalElement {
    pub fn new(
        terminal: Entity<Terminal>,
        view: WeakEntity<TerminalView>,
        focus_handle: FocusHandle,
        focused: bool,
        cursor_visible: bool,
        style: TerminalStyle,
        marked_text: Option<String>,
    ) -> Self {
        let mut interactivity = Interactivity::default();
        interactivity.element_id = Some(ElementId::from("terminal-grid"));
        Self {
            terminal,
            view,
            focus_handle: focus_handle.clone(),
            focused,
            cursor_visible,
            style,
            marked_text,
            interactivity,
        }
        .track_focus(&focus_handle)
    }

    fn base_font(style: &TerminalStyle) -> Font {
        font(style.font_family.clone())
    }

    fn span_font(style: &TerminalStyle, bold: bool, italic: bool) -> Font {
        let mut font = Self::base_font(style);
        font.weight = if bold {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        };
        font.style = if italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };
        font
    }

    /// Cell metrics for the font: the advance of a digit is the cell width,
    /// the line height a multiple of the font size rounded to whole pixels so
    /// rows never straddle a pixel boundary.
    fn cell_metrics(style: &TerminalStyle, window: &Window) -> (Pixels, Pixels) {
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&Self::base_font(style));
        let cell_width = match text_system.advance(font_id, style.font_size, '0') {
            Ok(advance) => advance.width,
            Err(error) => {
                tracing::debug!("no advance for the terminal font, estimating: {error}");
                style.font_size * 0.6
            }
        };
        let line_height = (style.font_size * style.line_height_scale).round();
        (cell_width, line_height)
    }

    fn layout_frame(
        style: &TerminalStyle,
        focused: bool,
        frame: &Frame,
        grid_origin: Point<Pixels>,
        window: &mut Window,
    ) -> (
        Vec<(Bounds<Pixels>, Hsla)>,
        Vec<Bounds<Pixels>>,
        Vec<ShapedSpan>,
        Option<CursorLayout>,
    ) {
        let GridSize {
            cell_width,
            line_height,
            ..
        } = frame.size;
        let cell_origin = |row: usize, column: usize| {
            point(
                grid_origin.x + cell_width * column as f32,
                grid_origin.y + line_height * row as f32,
            )
        };

        let mut background_rects = Vec::new();
        let mut spans = Vec::new();
        for row in &frame.rows {
            for span in &row.spans {
                let origin = cell_origin(row.row, span.column);
                if let Some(background) = span.background {
                    background_rects.push((
                        Bounds::new(
                            point(origin.x.floor(), origin.y),
                            size((cell_width * span.width as f32).ceil(), line_height),
                        ),
                        background,
                    ));
                }
                let run = TextRun {
                    len: span.text.len(),
                    font: Self::span_font(style, span.bold, span.italic),
                    color: span.foreground,
                    background_color: None,
                    underline: span.underline.then(|| UnderlineStyle {
                        color: Some(span.foreground),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    strikethrough: span.strikethrough.then(|| StrikethroughStyle {
                        color: Some(span.foreground),
                        thickness: px(1.0),
                    }),
                };
                let line = window.text_system().shape_line(
                    SharedString::from(span.text.clone()),
                    style.font_size,
                    &[run],
                    Some(cell_width),
                );
                spans.push(ShapedSpan { line, origin });
            }
        }

        let selection_rects = frame
            .selection
            .map(|selection| {
                let last_column = frame.size.columns.saturating_sub(1);
                (selection.start_row..=selection.end_row)
                    .map(|row| {
                        let (start, end) = if selection.is_block {
                            (selection.start_column, selection.end_column)
                        } else {
                            (
                                if row == selection.start_row {
                                    selection.start_column
                                } else {
                                    0
                                },
                                if row == selection.end_row {
                                    selection.end_column
                                } else {
                                    last_column
                                },
                            )
                        };
                        let origin = cell_origin(row, start);
                        Bounds::new(
                            origin,
                            size(cell_width * (end + 1 - start) as f32, line_height),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        let cursor = frame.cursor.as_ref().and_then(|cursor| {
            if cursor.shape == CursorShape::Hidden || cursor.row >= frame.size.rows {
                return None;
            }
            let width = cell_width * if cursor.wide { 2.0 } else { 1.0 };
            let bounds = Bounds::new(
                cell_origin(cursor.row, cursor.column),
                size(width, line_height),
            );
            let shape = if focused {
                cursor.shape
            } else {
                CursorShape::HollowBlock
            };
            let glyph =
                (shape == CursorShape::Block && !cursor.character.is_whitespace()).then(|| {
                    let text = cursor.character.to_string();
                    window.text_system().shape_line(
                        SharedString::from(text.clone()),
                        style.font_size,
                        &[TextRun {
                            len: text.len(),
                            font: Self::base_font(style),
                            color: frame.background,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        }],
                        Some(cell_width),
                    )
                });
            Some(CursorLayout {
                bounds,
                shape,
                glyph,
            })
        });

        (background_rects, selection_rects, spans, cursor)
    }

    fn register_mouse_listeners(&mut self, layout: &TerminalLayout, window: &mut Window) {
        let grid_origin = layout.grid_origin;
        let terminal = self.terminal.clone();
        let focus_handle = self.focus_handle.clone();

        for button in [MouseButton::Left, MouseButton::Middle, MouseButton::Right] {
            self.interactivity.on_mouse_down(button, {
                let terminal = terminal.clone();
                let focus_handle = focus_handle.clone();
                move |event, window, cx| {
                    window.focus(&focus_handle, cx);
                    terminal.update(cx, |terminal, cx| {
                        terminal.mouse_down(event, event.position - grid_origin);
                        cx.notify();
                    });
                }
            });
            self.interactivity.on_mouse_up(button, {
                let terminal = terminal.clone();
                move |event, _, cx| {
                    terminal.update(cx, |terminal, cx| {
                        terminal.mouse_up(event, event.position - grid_origin);
                        cx.notify();
                    });
                }
            });
        }

        {
            let terminal = terminal.clone();
            self.interactivity.on_scroll_wheel(move |event, _, cx| {
                terminal.update(cx, |terminal, cx| {
                    terminal.scroll_wheel(event, event.position - grid_origin);
                    cx.notify();
                });
            });
        }

        let hitbox = layout.hitbox.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            if event.pressed_button.is_some() {
                if focus_handle.is_focused(window) {
                    terminal.update(cx, |terminal, cx| {
                        terminal.mouse_drag(event.position - grid_origin, &event.modifiers);
                        cx.notify();
                    });
                }
            } else if hitbox.is_hovered(window) {
                terminal.update(cx, |terminal, _| {
                    terminal.mouse_moved(event.position - grid_origin, &event.modifiers);
                });
            }
        });
    }
}

impl InteractiveElement for TerminalElement {
    fn interactivity(&mut self) -> &mut Interactivity {
        &mut self.interactivity
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = TerminalLayout;

    fn id(&self) -> Option<ElementId> {
        self.interactivity.element_id.clone()
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let layout_id = self.interactivity.request_layout(
            id,
            inspector_id,
            window,
            cx,
            |mut style, window, cx| {
                style.size.width = relative(1.0).into();
                style.size.height = relative(1.0).into();
                window.request_layout(style, None, cx)
            },
        );
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let terminal = self.terminal.clone();
        let style = self.style.clone();
        let focused = self.focused;
        let marked_text = self.marked_text.clone();
        self.interactivity.prepaint(
            id,
            inspector_id,
            bounds,
            bounds.size,
            window,
            cx,
            |_, _, hitbox, window, cx| {
                let hitbox =
                    hitbox.unwrap_or_else(|| window.insert_hitbox(bounds, Default::default()));
                let (cell_width, line_height) = Self::cell_metrics(&style, window);
                let usable_width = (bounds.size.width - GUTTER * 2.0).max(cell_width * 2.0);
                let columns = ((usable_width / cell_width).floor() as usize).max(2);
                let usable_height = (bounds.size.height - GUTTER * 2.0).max(line_height);
                let rows = ((usable_height / line_height).floor() as usize).max(1);
                let grid_size = GridSize::new(rows, columns, cell_width, line_height);

                // Snap the origin to device pixels; a fractional origin makes
                // glyphs shimmer while the panel is being resized.
                let scale = window.scale_factor().max(1.0);
                let snap = |value: Pixels| px((f32::from(value) * scale).floor() / scale);
                let grid_origin = point(
                    snap(bounds.origin.x + GUTTER),
                    snap(bounds.origin.y + GUTTER),
                );

                let palette = style.palette.clone();
                let frame = terminal.update(cx, |terminal, _| {
                    terminal.set_palette(palette);
                    terminal.resize(grid_size);
                    terminal.frame()
                });

                let (background_rects, selection_rects, spans, cursor) =
                    Self::layout_frame(&style, focused, &frame, grid_origin, window);

                let ime_bounds = cursor.as_ref().map(|cursor| cursor.bounds).or_else(|| {
                    frame.cursor.as_ref().map(|cursor| {
                        Bounds::new(
                            point(
                                grid_origin.x + cell_width * cursor.column as f32,
                                grid_origin.y + line_height * cursor.row as f32,
                            ),
                            size(cell_width, line_height),
                        )
                    })
                });

                let marked_text =
                    marked_text
                        .as_ref()
                        .filter(|text| !text.is_empty())
                        .map(|text| {
                            window.text_system().shape_line(
                                SharedString::from(text.clone()),
                                style.font_size,
                                &[TextRun {
                                    len: text.len(),
                                    font: Self::base_font(&style),
                                    color: style.palette.foreground,
                                    background_color: None,
                                    underline: Some(UnderlineStyle {
                                        color: Some(style.palette.foreground),
                                        thickness: px(1.0),
                                        wavy: false,
                                    }),
                                    strikethrough: None,
                                }],
                                None,
                            )
                        });

                TerminalLayout {
                    hitbox,
                    grid_origin,
                    size: grid_size,
                    background: frame.background,
                    background_rects,
                    selection_rects,
                    selection_color: style.palette.selection,
                    spans,
                    cursor,
                    ime_bounds,
                    marked_text,
                }
            },
        )
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        layout: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.register_mouse_listeners(layout, window);
        let input_handler = TerminalInputHandler {
            view: self.view.clone(),
            cursor_bounds: layout.ime_bounds,
            cell_width: layout.size.cell_width,
        };
        let focus_handle = self.focus_handle.clone();
        let cursor_visible = self.cursor_visible;
        let composing = self.marked_text.is_some();
        let cursor_color = self.style.palette.cursor;

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.paint_quad(fill(bounds, layout.background));
            window.set_cursor_style(gpui::CursorStyle::IBeam, &layout.hitbox);
            self.interactivity.paint(
                id,
                inspector_id,
                bounds,
                Some(&layout.hitbox),
                window,
                cx,
                |_, window, cx| {
                    window.handle_input(&focus_handle, input_handler, cx);

                    for (rect, color) in &layout.background_rects {
                        window.paint_quad(fill(*rect, *color));
                    }
                    for rect in &layout.selection_rects {
                        window.paint_quad(fill(*rect, layout.selection_color));
                    }
                    for span in &layout.spans {
                        if let Err(error) = span.line.paint(
                            span.origin,
                            layout.size.line_height,
                            gpui::TextAlign::Left,
                            None,
                            window,
                            cx,
                        ) {
                            tracing::debug!("failed to paint a terminal span: {error}");
                        }
                    }

                    if let Some(marked) = &layout.marked_text
                        && let Some(ime_bounds) = layout.ime_bounds
                    {
                        let marked_bounds = Bounds::new(
                            ime_bounds.origin,
                            size(marked.width, layout.size.line_height),
                        );
                        window.paint_quad(fill(marked_bounds, layout.background));
                        if let Err(error) = marked.paint(
                            ime_bounds.origin,
                            layout.size.line_height,
                            gpui::TextAlign::Left,
                            None,
                            window,
                            cx,
                        ) {
                            tracing::debug!("failed to paint IME text: {error}");
                        }
                    }

                    if cursor_visible
                        && !composing
                        && let Some(cursor) = &layout.cursor
                    {
                        paint_cursor(cursor, cursor_color, layout.size, window, cx);
                    }
                },
            );
        });
    }
}

fn paint_cursor(
    cursor: &CursorLayout,
    color: Hsla,
    size: GridSize,
    window: &mut Window,
    cx: &mut App,
) {
    match cursor.shape {
        CursorShape::Block => {
            window.paint_quad(fill(cursor.bounds, color));
            if let Some(glyph) = &cursor.glyph
                && let Err(error) = glyph.paint(
                    cursor.bounds.origin,
                    size.line_height,
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                )
            {
                tracing::debug!("failed to paint the cursor glyph: {error}");
            }
        }
        CursorShape::HollowBlock => {
            window.paint_quad(quad(
                cursor.bounds,
                Corners::default(),
                Hsla::transparent_black(),
                Edges::all(px(1.0)),
                color,
                gpui::BorderStyle::Solid,
            ));
        }
        CursorShape::Underline => {
            let top = cursor.bounds.origin.y + cursor.bounds.size.height - THIN_CURSOR;
            window.paint_quad(fill(
                Bounds::new(
                    point(cursor.bounds.origin.x, top),
                    gpui::size(cursor.bounds.size.width, THIN_CURSOR),
                ),
                color,
            ));
        }
        CursorShape::Beam => {
            window.paint_quad(fill(
                Bounds::new(
                    cursor.bounds.origin,
                    gpui::size(THIN_CURSOR, cursor.bounds.size.height),
                ),
                color,
            ));
        }
        CursorShape::Hidden => {}
    }
}

/// Input-method plumbing: committed text goes to the PTY, text still being
/// composed is drawn at the cursor by the element.
struct TerminalInputHandler {
    view: WeakEntity<TerminalView>,
    cursor_bounds: Option<Bounds<Pixels>>,
    cell_width: Pixels,
}

impl InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<UTF16Selection> {
        // A terminal has no text selection the platform can edit; an empty
        // range at the start still lets the IME anchor its popup.
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(&mut self, _window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.view
            .read_with(cx, |view, _| view.marked_text_range())
            .ok()
            .flatten()
    }

    fn text_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        if self
            .view
            .update(cx, |view, cx| {
                view.set_marked_text(None, cx);
                view.commit_text(text, cx);
            })
            .is_ok()
        {
            window.invalidate_character_coordinates();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut App,
    ) {
        self.view
            .update(cx, |view, cx| {
                view.set_marked_text(Some(new_text.to_string()), cx)
            })
            .ok();
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut App) {
        self.view
            .update(cx, |view, cx| view.set_marked_text(None, cx))
            .ok();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let mut bounds = self.cursor_bounds?;
        bounds.origin.x += self.cell_width * range_utf16.start as f32;
        Some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        None
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }
}
