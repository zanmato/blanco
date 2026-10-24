use gpui::{
    App, BorderStyle, Bounds, ClipboardItem, Context, Corners, Edges, Element, ElementId,
    FocusHandle, GlobalElementId, HighlightStyle, Hitbox, HitboxBehavior, Hsla, InspectorElementId,
    InteractiveElement, IntoElement, LayoutId, PaintQuad, ParentElement, Pixels, Point, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, StyledText, Subscription,
    TextLayout, Window, div, prelude::FluentBuilder as _, px, transparent_black,
};
use gpui_base::{
    TextSelectionEvent, TextSelectionHandle, TextSelectionProjection, TextSelectionRegistration,
    TextSelectionRun,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::highlighter::{HighlightTheme, SyntaxHighlighter};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::theme::Theme;
use gpui_component::{ActiveTheme, IconName, Sizable as _};
use std::ops::Range;
use std::time::Duration;

pub enum SqlViewMessage {
    SqlStatement(String),
    Comment(String),
}

struct LogEntry {
    /// Position in the log, counted from the first entry ever appended. Stays
    /// put when older entries are trimmed, so it serves as the entry's element
    /// id and as its place in a selection that spans several entries.
    sequence: u64,
    text: SharedString,
    highlights: Box<[(Range<usize>, HighlightStyle)]>,
    selection: TextSelectionHandle,
    /// Re-renders the log when the window's selection changes this entry. A
    /// drag moves the selection on every pointer move without drawing a frame,
    /// so without it the selection only showed once the button was released.
    _render_on_selection_change: Subscription,
}

/// SQL view entity for displaying executed statements (queries, DDL, etc.) with
/// proper syntax highlighting. `language` is the registered highlighter language
/// name (e.g. `"sql"` or `"redis"`) and also selects the statement terminator
/// and line-comment syntax so logged statements read naturally per backend.
pub struct SqlView {
    entries: Vec<LogEntry>,
    next_sequence: u64,
    max_entries: usize,
    language: SharedString,
    scroll_handle: ScrollHandle,
    /// Takes focus when a selection starts in the log, so the copy shortcut
    /// reaches the window's selection instead of the editor that had focus.
    focus_handle: FocusHandle,
    copied: bool,
    show_copy_button: bool,
    /// Entries keep the colors of the theme they were highlighted with, so a
    /// theme change highlights them again.
    _rehighlight_on_theme_change: Subscription,
}

impl SqlView {
    /// Create a new view with the specified maximum number of entries, rendering
    /// statements with the given highlighter `language`.
    pub fn new(
        max_entries: usize,
        language: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            entries: Vec::new(),
            next_sequence: 0,
            max_entries,
            language: language.into(),
            scroll_handle: ScrollHandle::default(),
            focus_handle: cx.focus_handle(),
            copied: false,
            show_copy_button: true,
            _rehighlight_on_theme_change: cx.observe_global::<Theme>(Self::rehighlight),
        }
    }

    fn rehighlight(&mut self, cx: &mut Context<Self>) {
        let theme = cx.theme().highlight_theme.clone();
        for entry in &mut self.entries {
            entry.highlights = highlight(&self.language, &entry.text, &theme);
        }
        cx.notify();
    }

    /// Line-comment prefix for the view's language. Redis config-style `#`,
    /// JavaScript `//`, SQL `--`.
    fn line_comment_prefix(&self) -> &'static str {
        match self.language.as_ref() {
            "redis" => "#",
            "javascript" => "//",
            _ => "--",
        }
    }

    /// Control whether the copy-to-clipboard button is rendered.
    pub fn show_copy_button(mut self, show: bool) -> Self {
        self.show_copy_button = show;
        self
    }

    /// Concatenate every entry's text, in order, for copying to the clipboard.
    pub fn all_text(&self) -> String {
        self.entries
            .iter()
            .map(|entry| entry.text.as_ref())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn copy_to_clipboard(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.all_text()));
        self.copied = true;
        cx.notify();

        // Revert the button back to the copy icon after a short confirmation.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Append text to the log, managing entry limits
    pub fn append_text(&mut self, text: &SqlViewMessage, cx: &mut Context<Self>) {
        // Statements are terminated with `;` for SQL so each logged entry reads
        // as a complete statement. Line-oriented backends (Redis) have no
        // terminator, so leave their commands untouched.
        let new_text = match text {
            SqlViewMessage::SqlStatement(statement) => {
                // Every SQL grammar is registered under a name starting with
                // `sql` (`sql`, `sql_postgres`, ...).
                if self.language.starts_with("sql") && !statement.trim_start().ends_with(";") {
                    format!("{};", statement)
                } else {
                    statement.clone()
                }
            }
            SqlViewMessage::Comment(comment) => comment_lines(self.line_comment_prefix(), comment),
        };

        // Highlights are kept on the entry so render() doesn't parse every frame.
        let highlights = highlight(&self.language, &new_text, &cx.theme().highlight_theme);

        let text = SharedString::from(new_text);
        let selection = TextSelectionHandle::new(text.clone(), cx);
        selection.focus_with(
            {
                let focus_handle = self.focus_handle.clone();
                move |window, cx| focus_handle.focus(window, cx)
            },
            cx,
        );
        let render_on_selection_change = selection.subscribe(
            {
                let view = cx.entity().downgrade();
                move |event, cx| {
                    if matches!(event, TextSelectionEvent::SelectionChanged(_)) {
                        view.update(cx, |_, cx| cx.notify()).ok();
                    }
                }
            },
            cx,
        );
        let entry = LogEntry {
            sequence: self.next_sequence,
            text,
            highlights,
            selection,
            _render_on_selection_change: render_on_selection_change,
        };
        self.next_sequence += 1;

        // Append the new entry
        self.entries.push(entry);

        // Check if we need to trim old entries
        self.trim_entries();

        // Set scroll-to-bottom flag. This is consumed during the next prepaint
        // triggered by cx.notify(), so no delayed task is needed.
        self.scroll_handle.scroll_to_bottom();

        cx.notify();
    }

    /// Clear all entries from the log
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.entries.clear();
        self.scroll_handle.set_offset(gpui::Point::default());
        cx.notify();
    }

    /// Set the maximum number of entries and trim if necessary
    pub fn set_max_entries(&mut self, max_entries: usize, cx: &mut Context<Self>) {
        self.max_entries = max_entries;
        self.trim_entries();
        cx.notify();
    }

    /// Get the current number of entries in the log
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Trim old entries if we exceed the maximum
    fn trim_entries(&mut self) {
        if self.entries.len() > self.max_entries {
            let entries_to_remove = self.entries.len() - self.max_entries;
            self.entries.drain(0..entries_to_remove);
        }
    }
}

impl Render for SqlView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .bg(cx
                .theme()
                .highlight_theme
                .style
                .editor_background
                .unwrap_or(cx.theme().background))
            .child(
                div()
                    .id("sql-log")
                    .track_focus(&self.focus_handle)
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .p_4()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(12.))
                    .text_color(cx.theme().foreground)
                    .children(self.entries.iter().map(|entry| {
                        div()
                            .mb_1()
                            .child(SelectableLogEntry::new(entry, cx.theme().selection))
                    })),
            )
            .vertical_scrollbar(&self.scroll_handle)
            .when(self.show_copy_button && !self.entries.is_empty(), |this| {
                let copied = self.copied;
                this.child(
                    div().absolute().top_2().right_2().child(
                        Button::new("sql-log-copy")
                            .ghost()
                            .xsmall()
                            .icon(if copied {
                                IconName::Check
                            } else {
                                IconName::Copy
                            })
                            .tooltip(if copied { "Copied" } else { "Copy" })
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.copy_to_clipboard(cx);
                            })),
                    ),
                )
            })
    }
}

/// One log entry as text that takes part in the window's text selection:
/// drag to select within or across entries, then copy with the usual shortcut.
///
/// gpui-base's `SelectableText` does the same for plain text. It has no way to
/// pass highlights to the text it lays out, which the log needs for syntax
/// colors, so this registers with the selection layer itself.
struct SelectableLogEntry {
    sequence: u64,
    text: SharedString,
    styled_text: StyledText,
    selection: TextSelectionHandle,
    selection_color: Hsla,
}

impl SelectableLogEntry {
    fn new(entry: &LogEntry, selection_color: Hsla) -> Self {
        Self {
            sequence: entry.sequence,
            text: entry.text.clone(),
            styled_text: StyledText::new(entry.text.clone())
                .with_highlights(entry.highlights.iter().cloned()),
            selection: entry.selection.clone(),
            selection_color,
        }
    }

    fn paint_selection(&self, layout: &TextLayout, range: Range<usize>, window: &mut Window) {
        let (Some(start), Some(end)) = (
            layout.position_for_index(range.start),
            layout.position_for_index(range.end),
        ) else {
            return;
        };
        for bounds in selection_quads(start, end, layout.bounds(), layout.line_height()) {
            window.paint_quad(PaintQuad {
                bounds,
                background: self.selection_color.into(),
                corner_radii: Corners::default(),
                border_widths: Edges::default(),
                border_color: transparent_black(),
                border_style: BorderStyle::default(),
            });
        }
    }
}

/// The rectangles covering a selection from `start` to `end` (the top left
/// corners of the first and one-past-last selected glyphs): one for a single
/// line, otherwise the tail of the first line, the full lines between, and the
/// head of the last line.
fn selection_quads(
    start: Point<Pixels>,
    end: Point<Pixels>,
    bounds: Bounds<Pixels>,
    line_height: Pixels,
) -> Vec<Bounds<Pixels>> {
    if start.y == end.y {
        return vec![Bounds::from_corners(
            start,
            Point::new(end.x, end.y + line_height),
        )];
    }

    let mut quads = vec![Bounds::from_corners(
        start,
        Point::new(bounds.right(), start.y + line_height),
    )];
    if end.y > start.y + line_height {
        quads.push(Bounds::from_corners(
            Point::new(bounds.left(), start.y + line_height),
            Point::new(bounds.right(), end.y),
        ));
    }
    quads.push(Bounds::from_corners(
        Point::new(bounds.left(), end.y),
        Point::new(end.x, end.y + line_height),
    ));
    quads
}

impl IntoElement for SelectableLogEntry {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SelectableLogEntry {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::NamedInteger(
            "sql-log-entry".into(),
            self.sequence,
        ))
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.styled_text
            .request_layout(global_id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.styled_text
            .prepaint(global_id, inspector_id, bounds, &mut (), window, cx);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let registration = TextSelectionRegistration::new(hitbox.clone(), bounds)
            .with_document_order(self.sequence)
            .with_text_bounds(vec![bounds])
            .with_rendered_element(&self.selection, window, cx);
        self.selection.register(registration, window, cx);
        hitbox
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.styled_text.layout().clone();
        let projection = self.selection.update_runs(
            &[TextSelectionRun::new(
                self.text.clone(),
                layout.clone(),
                bounds,
            )],
            cx,
        );
        // Projecting onto this frame's layout can change what the selection
        // covers (the log scrolled or reflowed under a held drag), which the
        // entries painted before this one have not seen yet. Only this entry's
        // projection can change here, so it is compared with the one painted
        // last frame: asking for the window's selected text before and after
        // walks every entry in the log, which made each frame cost the square
        // of the entry count.
        let changed = window.with_element_state(
            global_id.expect("SelectableLogEntry has an element id"),
            |painted: Option<TextSelectionProjection>, _| {
                (painted.as_ref() != Some(&projection), projection.clone())
            },
        );
        if changed && projection.ranges().iter().any(Option::is_some) {
            window.refresh();
        }
        for range in projection.ranges().iter().flatten().cloned() {
            self.paint_selection(&layout, range, window);
        }
        self.styled_text.paint(
            global_id,
            inspector_id,
            bounds,
            &mut (),
            &mut (),
            window,
            cx,
        );
    }
}

fn highlight(
    language: &str,
    text: &str,
    theme: &HighlightTheme,
) -> Box<[(Range<usize>, HighlightStyle)]> {
    let mut highlighter = SyntaxHighlighter::new(language);
    highlighter.update(None, &ropey::Rope::from(text), None);
    highlighter
        .styles(&(0..text.len()), theme)
        .into_boxed_slice()
}

/// Comment out every line of `text`, not just the first. Log messages carry
/// multi-line payloads (a JS stack trace, a driver error with detail lines),
/// and prefixing only the first line leaves the rest highlighted as code and
/// invalid if copied back out.
fn comment_lines(prefix: &str, text: &str) -> String {
    if text.is_empty() {
        return prefix.to_string();
    }
    text.lines()
        .map(|line| {
            // Don't leave trailing whitespace on blank lines.
            if line.is_empty() {
                prefix.to_string()
            } else {
                format!("{prefix} {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{SqlView, SqlViewMessage, comment_lines};
    use gpui::{AppContext as _, Modifiers, MouseButton, TestAppContext, VisualTestContext, px};
    use gpui_base::TextSelection;

    /// Entries are highlighted when they are appended, so a theme applied
    /// later has to recolor the ones already in the log.
    #[gpui::test]
    fn a_theme_change_recolors_logged_entries(cx: &mut TestAppContext) {
        let view = cx.update(|cx| {
            gpui_component::init(cx);
            gpui_component::Theme::change(gpui_component::ThemeMode::Light, None, cx);
            // The SQL grammars are registered by the app, so this crate's tests
            // use a language gpui-component has built in.
            let view = cx.new(|cx| SqlView::new(10, "javascript", cx));
            view.update(cx, |view, cx| {
                view.append_text(&SqlViewMessage::SqlStatement("const total = 1;".into()), cx)
            });
            view
        });
        let light = view.read_with(cx, |view, _| view.entries[0].highlights.clone());
        assert!(!light.is_empty(), "the statement should be highlighted");

        cx.update(|cx| gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx));
        cx.run_until_parked();

        let dark = view.read_with(cx, |view, _| view.entries[0].highlights.clone());
        assert_ne!(light, dark);
    }

    /// Dragging across the log selects its text through the window's selection
    /// layer, which is what the copy shortcut reads.
    #[gpui::test]
    fn dragging_over_entries_selects_their_text(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            gpui_component::init(cx);
            cx.open_window(Default::default(), |window, cx| {
                let view = cx.new(|cx| {
                    let mut view = SqlView::new(10, "sql", cx);
                    view.append_text(&SqlViewMessage::SqlStatement("SELECT 1".into()), cx);
                    view.append_text(&SqlViewMessage::Comment("1 row".into()), cx);
                    view
                });
                cx.new(|cx| gpui_component::Root::new(view, window, cx))
            })
            .expect("failed to open window")
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        cx.run_until_parked();

        // The log pads its content by 16px; 12px text puts the first entry's
        // line around y = 24 and the second entry a line below it.
        let start = gpui::point(px(17.), px(24.));
        let end = gpui::point(px(400.), px(46.));
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());

        cx.update(|window, cx| {
            let _frame = window.draw(cx);
            assert_eq!(
                TextSelection::selected_text(window, cx),
                "SELECT 1;\n-- 1 row"
            );
        });
    }

    /// The selection has to show while the button is held, not only once it is
    /// released: each pointer move of a drag must re-render the log.
    #[gpui::test]
    fn a_held_drag_re_renders_the_log(cx: &mut TestAppContext) {
        let (window, view) = cx.update(|cx| {
            gpui_component::init(cx);
            let mut view = None;
            let window = cx
                .open_window(Default::default(), |window, cx| {
                    let log = cx.new(|cx| {
                        let mut log = SqlView::new(10, "sql", cx);
                        log.append_text(&SqlViewMessage::SqlStatement("SELECT 1".into()), cx);
                        log.append_text(&SqlViewMessage::Comment("1 row".into()), cx);
                        log
                    });
                    view = Some(log.clone());
                    cx.new(|cx| gpui_component::Root::new(log, window, cx))
                })
                .expect("failed to open window");
            (window, view.expect("the window builds its view"))
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        cx.run_until_parked();

        let renders = std::rc::Rc::new(std::cell::Cell::new(0));
        let _observation = cx.update(|_, cx| {
            let renders = renders.clone();
            cx.observe(&view, move |_, _| renders.set(renders.get() + 1))
        });

        cx.simulate_mouse_down(
            gpui::point(px(17.), px(24.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_move(
            gpui::point(px(400.), px(46.)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        cx.run_until_parked();

        assert!(renders.get() > 0, "the drag did not re-render the log");
    }

    #[test]
    fn single_line_gets_one_prefix() {
        assert_eq!(comment_lines("--", "42 rows in 3ms"), "-- 42 rows in 3ms");
    }

    #[test]
    fn every_line_of_a_stack_trace_is_commented() {
        let message = "script failed: Error: not a function\n    at <eval> (eval_script:12:8)";
        assert_eq!(
            comment_lines("//", message),
            "// script failed: Error: not a function\n//     at <eval> (eval_script:12:8)"
        );
    }

    #[test]
    fn blank_lines_carry_no_trailing_whitespace() {
        assert_eq!(
            comment_lines("//", "first\n\nthird"),
            "// first\n//\n// third"
        );
        assert_eq!(comment_lines("//", ""), "//");
    }

    #[test]
    fn a_trailing_newline_does_not_add_an_empty_comment() {
        assert_eq!(comment_lines("#", "done\n"), "# done");
    }
}
