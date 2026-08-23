use std::ops::Range;
use std::sync::Arc;

use gpui::{
    Context, HighlightStyle, Hsla, IntoElement, ListHorizontalSizingBehavior, ListSizingBehavior,
    ParentElement, Render, SharedString, Styled, StyledText, UniformListScrollHandle, Window, div,
    prelude::FluentBuilder as _, px, uniform_list,
};
use gpui_component::highlighter::{HighlightTheme, SyntaxHighlighter};
use gpui_component::scroll::Scrollbar;
use gpui_component::{ActiveTheme, StyledExt as _};
use similar::{ChangeTag, DiffOp, TextDiff};

/// One side of a comparison: the text to diff plus how to present it.
/// `language` is a registered highlighter language name (e.g. `"json"`), or
/// `None` for plain text.
pub struct DiffSource {
    pub title: SharedString,
    pub text: String,
    pub language: Option<SharedString>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffRowKind {
    Equal,
    Added,
    Removed,
    Modified,
}

/// One line on one side of an aligned diff row. `line_number` is `None` for
/// the padding slot opposite an added or removed line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiffLine {
    pub line_number: Option<usize>,
    pub text: String,
    /// Byte ranges within `text` that changed, for word level emphasis inside
    /// a modified line.
    pub emphasis: Vec<Range<usize>>,
}

impl DiffLine {
    fn gap() -> Self {
        Self::default()
    }

    fn is_gap(&self) -> bool {
        self.line_number.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    pub kind: DiffRowKind,
    pub left: DiffLine,
    pub right: DiffLine,
}

fn strip_newline(line: &str) -> &str {
    line.strip_suffix("\r\n")
        .or_else(|| line.strip_suffix('\n'))
        .unwrap_or(line)
}

/// Emphasis byte ranges for each line of one side of a `Replace` op, in order.
/// `similar` yields inline changes as `(emphasized, segment)` pairs per line,
/// so the ranges are rebuilt by walking the segment lengths.
fn inline_emphasis<'a>(
    diff: &'a TextDiff<'a, 'a, 'a, str>,
    op: &DiffOp,
    tag: ChangeTag,
) -> Vec<Vec<Range<usize>>> {
    diff.iter_inline_changes(op)
        .filter(|change| change.tag() == tag)
        .map(|change| {
            let mut offset = 0;
            let mut ranges = Vec::new();
            for (emphasized, segment) in change.iter_strings_lossy() {
                let segment = strip_newline(&segment);
                let end = offset + segment.len();
                if emphasized && end > offset {
                    ranges.push(offset..end);
                }
                offset = end;
            }
            ranges
        })
        .collect()
}

/// Align `left` and `right` line by line the way a split diff does: equal
/// lines pair up, replaced blocks pair positionally with word emphasis, and
/// the shorter side of an unbalanced block is padded with gap slots.
pub fn build_rows(left: &str, right: &str) -> Vec<DiffRow> {
    // A missing trailing newline would otherwise make the last lines differ
    // ("a\n" vs "a"), which is noise when comparing values.
    let normalize = |text: &str| {
        if text.is_empty() || text.ends_with('\n') {
            text.to_string()
        } else {
            format!("{text}\n")
        }
    };
    let (left, right) = (normalize(left), normalize(right));
    let diff = TextDiff::from_lines(left.as_str(), right.as_str());
    let old_lines = diff.old_slices();
    let new_lines = diff.new_slices();
    let mut rows = Vec::new();

    let old_line = |index: usize| DiffLine {
        line_number: Some(index + 1),
        text: old_lines
            .get(index)
            .map(|line| strip_newline(line).to_string())
            .unwrap_or_default(),
        emphasis: Vec::new(),
    };
    let new_line = |index: usize| DiffLine {
        line_number: Some(index + 1),
        text: new_lines
            .get(index)
            .map(|line| strip_newline(line).to_string())
            .unwrap_or_default(),
        emphasis: Vec::new(),
    };

    for op in diff.ops() {
        match *op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => {
                for offset in 0..len {
                    rows.push(DiffRow {
                        kind: DiffRowKind::Equal,
                        left: old_line(old_index + offset),
                        right: new_line(new_index + offset),
                    });
                }
            }
            DiffOp::Delete {
                old_index, old_len, ..
            } => {
                for offset in 0..old_len {
                    rows.push(DiffRow {
                        kind: DiffRowKind::Removed,
                        left: old_line(old_index + offset),
                        right: DiffLine::gap(),
                    });
                }
            }
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                for offset in 0..new_len {
                    rows.push(DiffRow {
                        kind: DiffRowKind::Added,
                        left: DiffLine::gap(),
                        right: new_line(new_index + offset),
                    });
                }
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                let old_emphasis = inline_emphasis(&diff, op, ChangeTag::Delete);
                let new_emphasis = inline_emphasis(&diff, op, ChangeTag::Insert);
                for offset in 0..old_len.max(new_len) {
                    let left = (offset < old_len).then(|| {
                        let mut line = old_line(old_index + offset);
                        line.emphasis = old_emphasis.get(offset).cloned().unwrap_or_default();
                        line
                    });
                    let right = (offset < new_len).then(|| {
                        let mut line = new_line(new_index + offset);
                        line.emphasis = new_emphasis.get(offset).cloned().unwrap_or_default();
                        line
                    });
                    let kind = match (&left, &right) {
                        (Some(_), Some(_)) => DiffRowKind::Modified,
                        (Some(_), None) => DiffRowKind::Removed,
                        _ => DiffRowKind::Added,
                    };
                    rows.push(DiffRow {
                        kind,
                        left: left.unwrap_or_else(DiffLine::gap),
                        right: right.unwrap_or_else(DiffLine::gap),
                    });
                }
            }
        }
    }

    rows
}

/// Syntax highlight runs for every line of `text`, rebased to line local byte
/// offsets. Index `i` corresponds to the `i`th line of `text`.
fn highlight_lines(
    text: &str,
    language: &str,
    theme: &HighlightTheme,
) -> Vec<Vec<(Range<usize>, HighlightStyle)>> {
    let mut highlighter = SyntaxHighlighter::new(language);
    let rope = ropey::Rope::from(text);
    highlighter.update(None, &rope, None);
    let styles = highlighter.styles(&(0..text.len()), theme);

    let mut lines = Vec::new();
    let mut line_start = 0;
    for segment in text.split_inclusive('\n') {
        let line_end = line_start + strip_newline(segment).len();
        let runs = styles
            .iter()
            .filter(|(range, _)| range.start < line_end && range.end > line_start)
            .map(|(range, style)| {
                let start = range.start.max(line_start) - line_start;
                let end = range.end.min(line_end) - line_start;
                (start..end, *style)
            })
            .collect();
        lines.push(runs);
        line_start += segment.len();
    }
    lines
}

/// Merge syntax runs with emphasis ranges into sorted, non overlapping runs,
/// which is what `StyledText::with_highlights` requires. Emphasis only adds a
/// background so the syntax colour of the changed word is preserved.
fn merge_highlights(
    syntax: &[(Range<usize>, HighlightStyle)],
    emphasis: &[Range<usize>],
    emphasis_background: Hsla,
    text_len: usize,
) -> Vec<(Range<usize>, HighlightStyle)> {
    if emphasis.is_empty() {
        return syntax.to_vec();
    }

    let mut boundaries: Vec<usize> = syntax
        .iter()
        .flat_map(|(range, _)| [range.start, range.end])
        .chain(emphasis.iter().flat_map(|range| [range.start, range.end]))
        .chain([0, text_len])
        .collect();
    boundaries.sort_unstable();
    boundaries.dedup();

    let mut runs = Vec::new();
    for pair in boundaries.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        let syntax_style = syntax
            .iter()
            .find(|(range, _)| range.start <= start && range.end >= end)
            .map(|(_, style)| *style);
        let emphasized = emphasis
            .iter()
            .any(|range| range.start <= start && range.end >= end);
        let mut style = match (syntax_style, emphasized) {
            (None, false) => continue,
            (Some(style), _) => style,
            (None, true) => HighlightStyle::default(),
        };
        if emphasized {
            style.background_color = Some(emphasis_background);
        }
        runs.push((start..end, style));
    }
    runs
}

struct RenderedLine {
    line: DiffLine,
    highlights: Box<[(Range<usize>, HighlightStyle)]>,
}

struct RenderedRow {
    kind: DiffRowKind,
    left: RenderedLine,
    right: RenderedLine,
}

/// Side by side diff of two texts with red/green row backgrounds, word level
/// emphasis and syntax highlighting. Rows are virtualised so large values
/// (pretty printed JSON documents) stay cheap to show.
pub struct DiffView {
    left_title: SharedString,
    right_title: SharedString,
    rows: Vec<RenderedRow>,
    added: usize,
    removed: usize,
    gutter_width: f32,
    scroll_handle: UniformListScrollHandle,
}

impl DiffView {
    pub fn new(left: DiffSource, right: DiffSource, cx: &mut Context<Self>) -> Self {
        let theme: Arc<HighlightTheme> = cx.theme().highlight_theme.clone();
        let removed_emphasis = cx.theme().danger.opacity(0.4);
        let added_emphasis = cx.theme().success.opacity(0.4);

        let rows = build_rows(&left.text, &right.text);
        let left_highlights = left
            .language
            .as_ref()
            .map(|language| highlight_lines(&left.text, language, &theme));
        let right_highlights = right
            .language
            .as_ref()
            .map(|language| highlight_lines(&right.text, language, &theme));

        let render_line = |line: DiffLine,
                           highlights: &Option<Vec<Vec<(Range<usize>, HighlightStyle)>>>,
                           emphasis_background: Hsla| {
            let syntax = line
                .line_number
                .and_then(|number| highlights.as_ref()?.get(number - 1))
                .map(|runs| runs.as_slice())
                .unwrap_or(&[]);
            let highlights =
                merge_highlights(syntax, &line.emphasis, emphasis_background, line.text.len())
                    .into_boxed_slice();
            RenderedLine { line, highlights }
        };

        let mut added = 0;
        let mut removed = 0;
        let rows = rows
            .into_iter()
            .map(|row| {
                match row.kind {
                    DiffRowKind::Equal => {}
                    DiffRowKind::Added => added += 1,
                    DiffRowKind::Removed => removed += 1,
                    DiffRowKind::Modified => {
                        added += 1;
                        removed += 1;
                    }
                }
                RenderedRow {
                    kind: row.kind,
                    left: render_line(row.left, &left_highlights, removed_emphasis),
                    right: render_line(row.right, &right_highlights, added_emphasis),
                }
            })
            .collect::<Vec<_>>();

        let max_line_number = rows
            .iter()
            .flat_map(|row| [row.left.line.line_number, row.right.line.line_number])
            .flatten()
            .max()
            .unwrap_or(1);
        // Digits times a mono glyph width at 12px plus padding.
        let gutter_width = (max_line_number.to_string().len() as f32) * 7.5 + 16.;

        Self {
            left_title: left.title,
            right_title: right.title,
            rows,
            added,
            removed,
            gutter_width,
            scroll_handle: UniformListScrollHandle::default(),
        }
    }

    pub fn row_kinds(&self) -> Vec<DiffRowKind> {
        self.rows.iter().map(|row| row.kind).collect()
    }

    pub fn has_changes(&self) -> bool {
        self.added > 0 || self.removed > 0
    }

    fn render_side(
        &self,
        rendered: &RenderedLine,
        background: Option<Hsla>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let gutter_color = cx.theme().muted_foreground;
        div()
            .flex()
            .flex_1()
            .flex_basis(px(0.))
            .min_w(px(0.))
            .h_full()
            .when_some(background, |this, background| this.bg(background))
            .child(
                div()
                    .flex_shrink_0()
                    .w(px(self.gutter_width))
                    .pr_2()
                    .text_right()
                    .text_color(gutter_color)
                    .child(
                        rendered
                            .line
                            .line_number
                            .map(|number| number.to_string())
                            .unwrap_or_default(),
                    ),
            )
            .child(
                div()
                    .pl_1()
                    .pr_3()
                    .whitespace_nowrap()
                    .when(!rendered.line.is_gap(), |this| {
                        this.child(
                            StyledText::new(SharedString::from(rendered.line.text.clone()))
                                .with_highlights(rendered.highlights.iter().cloned()),
                        )
                    }),
            )
    }

    fn render_header(&self, cx: &Context<Self>) -> impl IntoElement {
        let title = |text: SharedString| {
            div()
                .flex_1()
                .min_w(px(0.))
                .px_3()
                .py_1()
                .text_sm()
                .font_semibold()
                .truncate()
                .child(text)
        };
        div()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary)
            .child(title(self.left_title.clone()))
            .child(div().w(px(1.)).h_full().bg(cx.theme().border))
            .child(title(self.right_title.clone()))
            .child(
                div()
                    .flex_shrink_0()
                    .px_3()
                    .text_xs()
                    .font_family(cx.theme().mono_font_family.clone())
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_color(cx.theme().success)
                                    .child(format!("+{}", self.added)),
                            )
                            .child(
                                div()
                                    .text_color(cx.theme().danger)
                                    .child(format!("-{}", self.removed)),
                            ),
                    ),
            )
    }
}

impl Render for DiffView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let editor_background = cx
            .theme()
            .highlight_theme
            .style
            .editor_background
            .unwrap_or(cx.theme().background);
        let removed_background = cx.theme().danger.opacity(0.15);
        let added_background = cx.theme().success.opacity(0.15);
        let gap_background = cx.theme().muted.opacity(0.3);
        let divider = cx.theme().border;

        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h(px(0.))
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius)
            .overflow_hidden()
            .child(self.render_header(cx))
            .when(!self.has_changes(), |this| {
                this.child(
                    div()
                        .py_2()
                        .text_center()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child("No differences"),
                )
            })
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .bg(editor_background)
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(12.))
                    .text_color(cx.theme().foreground)
                    .child(
                        uniform_list(
                            "diff-rows",
                            self.rows.len(),
                            cx.processor(move |this, visible_range: Range<usize>, _window, cx| {
                                visible_range
                                    .filter_map(|index| {
                                        let row = this.rows.get(index)?;
                                        let (left_background, right_background) = match row.kind {
                                            DiffRowKind::Equal => (None, None),
                                            DiffRowKind::Added => {
                                                (Some(gap_background), Some(added_background))
                                            }
                                            DiffRowKind::Removed => {
                                                (Some(removed_background), Some(gap_background))
                                            }
                                            DiffRowKind::Modified => {
                                                (Some(removed_background), Some(added_background))
                                            }
                                        };
                                        Some(
                                            div()
                                                .flex()
                                                .min_w_full()
                                                .h(px(20.))
                                                .items_center()
                                                .child(this.render_side(
                                                    &row.left,
                                                    left_background,
                                                    cx,
                                                ))
                                                .child(
                                                    div()
                                                        .flex_shrink_0()
                                                        .w(px(1.))
                                                        .h_full()
                                                        .bg(divider),
                                                )
                                                .child(this.render_side(
                                                    &row.right,
                                                    right_background,
                                                    cx,
                                                )),
                                        )
                                    })
                                    .collect()
                            }),
                        )
                        .size_full()
                        .track_scroll(&self.scroll_handle)
                        .with_sizing_behavior(ListSizingBehavior::Auto)
                        .with_horizontal_sizing_behavior(
                            ListHorizontalSizingBehavior::Unconstrained,
                        ),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .bottom_0()
                            .w(px(16.))
                            .child(Scrollbar::vertical(&self.scroll_handle)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(rows: &[DiffRow]) -> Vec<DiffRowKind> {
        rows.iter().map(|row| row.kind).collect()
    }

    #[test]
    fn identical_text_is_all_equal() {
        let rows = build_rows("a\nb\nc", "a\nb\nc");
        assert_eq!(kinds(&rows), vec![DiffRowKind::Equal; 3]);
        assert_eq!(rows[2].left.line_number, Some(3));
        assert_eq!(rows[2].right.line_number, Some(3));
        assert_eq!(rows[2].left.text, "c");
    }

    #[test]
    fn insert_pads_left_with_gaps() {
        let rows = build_rows("a\nc", "a\nb\nc");
        assert_eq!(
            kinds(&rows),
            vec![DiffRowKind::Equal, DiffRowKind::Added, DiffRowKind::Equal]
        );
        assert!(rows[1].left.is_gap());
        assert_eq!(rows[1].right.line_number, Some(2));
        assert_eq!(rows[1].right.text, "b");
        assert_eq!(rows[2].left.line_number, Some(2));
        assert_eq!(rows[2].right.line_number, Some(3));
    }

    #[test]
    fn delete_pads_right_with_gaps() {
        let rows = build_rows("a\nb", "a");
        assert_eq!(kinds(&rows), vec![DiffRowKind::Equal, DiffRowKind::Removed]);
        assert!(rows[1].right.is_gap());
        assert_eq!(rows[1].left.text, "b");
    }

    #[test]
    fn replace_pairs_positionally_and_pads_the_rest() {
        let rows = build_rows("x1\nx2", "y1\ny2\ny3");
        assert_eq!(
            kinds(&rows),
            vec![
                DiffRowKind::Modified,
                DiffRowKind::Modified,
                DiffRowKind::Added
            ]
        );
        assert_eq!(rows[0].left.text, "x1");
        assert_eq!(rows[0].right.text, "y1");
        assert!(rows[2].left.is_gap());
        assert_eq!(rows[2].right.line_number, Some(3));
    }

    #[test]
    fn inline_emphasis_covers_only_the_changed_word() {
        let rows = build_rows("\"plan\": \"free\"", "\"plan\": \"pro\"");
        assert_eq!(kinds(&rows), vec![DiffRowKind::Modified]);
        let left = &rows[0].left;
        let right = &rows[0].right;
        assert_eq!(
            left.emphasis
                .iter()
                .map(|range| &left.text[range.clone()])
                .collect::<Vec<_>>(),
            vec!["free"]
        );
        assert_eq!(
            right
                .emphasis
                .iter()
                .map(|range| &right.text[range.clone()])
                .collect::<Vec<_>>(),
            vec!["pro"]
        );
    }

    #[test]
    fn crlf_lines_are_trimmed() {
        let rows = build_rows("a\r\nb\r\n", "a\r\nb\r\n");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].left.text, "b");
    }

    #[test]
    fn merged_highlights_are_sorted_and_non_overlapping() {
        let syntax = vec![
            (0..4, HighlightStyle::default()),
            (6..10, HighlightStyle::default()),
        ];
        let emphasis = vec![2..8];
        let runs = merge_highlights(&syntax, &emphasis, Hsla::default(), 10);
        let ranges: Vec<_> = runs.iter().map(|(range, _)| range.clone()).collect();
        assert_eq!(ranges, vec![0..2, 2..4, 4..6, 6..8, 8..10]);
        assert!(runs[0].1.background_color.is_none());
        assert!(runs[1].1.background_color.is_some());
        assert!(runs[2].1.background_color.is_some());
        assert!(runs[4].1.background_color.is_none());
    }
}
