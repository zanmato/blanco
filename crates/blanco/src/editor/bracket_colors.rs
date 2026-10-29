//! Colors the brackets of the statement under the cursor by how deeply they
//! nest, and brackets without a partner in the error color
//! (`editor.colorize_brackets`).
//!
//! Statements are divided by the lexer's `;`s rather than taken from the
//! outline's syntax tree, which while a statement is typed can leave out a
//! stray `)` or find no statement at all behind an open string. Each
//! document version is scanned once on a background thread, so a multi
//! megabyte INSERT never stalls typing, and moving the cursor only picks
//! another statement from the scan. The editor only gets decorations for
//! the rows around the ones on screen, since it moves every decoration it
//! holds on each edit.

use std::ops::Range;
use std::time::Duration;

use blanco_core::brackets::{ScriptBrackets, script_brackets};
use gpui::{App, AppContext as _, Context, Entity, HighlightStyle, Hsla, Task};
use gpui_component::input::{EditorState, TextDecoration, TextDecorationCollection};
use gpui_component::{ActiveTheme as _, RopeExt as _};

use crate::app_settings::AppSettings;
use crate::result_ext::ResultExt as _;

use super::{EditorPanel, TabType};

/// Documents above this size are scanned once typing pauses rather than on
/// every keystroke, since a scan takes milliseconds of a background thread
/// per megabyte.
const DEBOUNCED_SCAN_BYTES: usize = 256 * 1024;
const SCAN_DEBOUNCE: Duration = Duration::from_millis(150);

/// The editor does not syntax highlight lines longer than this, and their
/// brackets are left plain too, so one long line cannot hand the editor
/// thousands of decorations.
const MAX_LINE_BYTES: usize = 10_000;

/// The colors of a theme that brackets cycle through, outermost first, and
/// the one for brackets without a partner.
#[derive(Clone, PartialEq)]
struct Palette {
    depths: [Hsla; 5],
    unmatched: Hsla,
}

impl Palette {
    fn from_theme(cx: &App) -> Self {
        let theme = cx.theme();
        Self {
            depths: [
                theme.yellow,
                theme.magenta,
                theme.blue,
                theme.cyan,
                theme.green,
            ],
            unmatched: theme.danger,
        }
    }

    fn color(&self, depth: Option<u32>) -> Hsla {
        match depth {
            Some(depth) => self.depths[depth as usize % self.depths.len()],
            None => self.unmatched,
        }
    }
}

/// What the editor's decorations were made from.
#[derive(PartialEq)]
struct Shown {
    version: u64,
    statement: Range<usize>,
    rows: Range<usize>,
    palette: Palette,
}

/// A query tab's bracket colors.
pub struct BracketColors {
    decorations: TextDecorationCollection,
    /// The document version the latest scan was started for.
    scanning: Option<u64>,
    scan: Option<Task<()>>,
    scanned: Option<(u64, ScriptBrackets)>,
    shown: Option<Shown>,
}

impl BracketColors {
    pub fn new(editor: &Entity<EditorState>, cx: &mut App) -> Self {
        let decorations = editor.update(cx, |state, cx| {
            state.create_decorations_collection(vec![], cx)
        });
        Self {
            decorations,
            scanning: None,
            scan: None,
            scanned: None,
            shown: None,
        }
    }

    /// The byte ranges the editor colors.
    #[cfg(test)]
    pub fn decorated_ranges(&self, cx: &App) -> Vec<Range<usize>> {
        self.decorations.get_ranges(cx)
    }

    /// The offsets the editor colors with the depth of the bracket at each,
    /// `None` for a bracket without a partner.
    #[cfg(test)]
    pub fn decorated_depths(&self, cx: &App) -> Vec<(usize, Option<u32>)> {
        let brackets = self
            .scanned
            .as_ref()
            .map(|(_, script)| script.brackets.as_slice())
            .unwrap_or_default();
        self.decorations
            .get_ranges(cx)
            .into_iter()
            .map(|range| {
                let depth = brackets
                    .iter()
                    .find(|bracket| bracket.offset == range.start)
                    .and_then(|bracket| bracket.depth);
                (range.start, depth)
            })
            .collect()
    }

    fn clear(&mut self, cx: &mut App) {
        self.scanning = None;
        self.scan = None;
        self.scanned = None;
        if self.shown.take().is_some() {
            self.decorations.set(Vec::new(), cx);
        }
    }

    /// The decorations for the brackets of the statement under the cursor
    /// on the rows around the visible ones, when they differ from what the
    /// editor shows. `None` while the latest scan is still running, which
    /// leaves the editor's decorations moving with the edits.
    fn plan(&self, state: &EditorState, palette: &Palette) -> Option<(Vec<TextDecoration>, Shown)> {
        let (version, script) = self
            .scanned
            .as_ref()
            .filter(|(version, _)| *version == state.document_version())?;
        let visible = state.visible_row_range()?;
        let statement = script.statement_at(state.cursor()).unwrap_or_default();
        if let Some(shown) = &self.shown
            && shown.version == *version
            && shown.statement == statement
            && &shown.palette == palette
            && shown.rows.start <= visible.start
            && visible.end <= shown.rows.end
        {
            return None;
        }

        // A screen of rows either side, so scrolling a little does not need
        // new decorations.
        let text = state.text();
        let margin = visible.len().max(1);
        let rows = visible.start.saturating_sub(margin)
            ..visible.end.saturating_add(margin).min(text.lines_len());
        let mut decorations = Vec::new();
        for row in rows.clone() {
            let line = text.line_start_offset(row)..text.line_end_offset(row);
            if line.len() > MAX_LINE_BYTES {
                continue;
            }
            let within = line.start.max(statement.start)..line.end.min(statement.end);
            if within.is_empty() {
                continue;
            }
            decorations.extend(script.brackets_in(&within).iter().map(|bracket| {
                TextDecoration::new(
                    bracket.offset..bracket.offset + 1,
                    HighlightStyle {
                        color: Some(palette.color(bracket.depth)),
                        ..HighlightStyle::default()
                    },
                )
            }));
        }
        Some((
            decorations,
            Shown {
                version: *version,
                statement,
                rows,
                palette: palette.clone(),
            },
        ))
    }
}

impl EditorPanel {
    /// Scan a query tab's document again when it changed, and give the
    /// editor the decorations for the statement under the cursor on the rows
    /// it shows.
    ///
    /// Driven from an observer of the editor, which also fires on every
    /// repaint, so it does nothing unless the document, the cursor's
    /// statement, the rows on screen or the theme changed.
    pub(super) fn refresh_bracket_colors(
        &mut self,
        editor: &Entity<EditorState>,
        cx: &mut Context<Self>,
    ) {
        let enabled = AppSettings::global(cx).settings.editor.colorize_brackets;
        let palette = Palette::from_theme(cx);
        let Some(query_tab) = self.tabs.iter_mut().find_map(|tab| match tab {
            TabType::Query(query_tab) if &query_tab.base.editor == editor => Some(query_tab),
            _ => None,
        }) else {
            return;
        };
        let dialect = query_tab.context.db_type.dialect();
        let Some(colors) = query_tab.bracket_colors.as_mut() else {
            return;
        };
        if !enabled {
            colors.clear(cx);
            return;
        }

        let state = editor.read(cx);
        let version = state.document_version();
        if colors.scanning != Some(version) {
            colors.scanning = Some(version);
            // Cloning the rope shares its chunks, the copy into one string
            // happens on the background thread.
            let text = state.text().clone();
            let debounce = text.len() > DEBOUNCED_SCAN_BYTES;
            let editor = editor.clone();
            colors.scan = Some(cx.spawn(async move |panel, cx| {
                if debounce {
                    cx.background_executor().timer(SCAN_DEBOUNCE).await;
                }
                let scanned = cx
                    .background_spawn(async move { script_brackets(dialect, &text.to_string()) })
                    .await;
                panel
                    .update(cx, |panel, cx| {
                        panel.finish_bracket_scan(&editor, version, scanned, cx)
                    })
                    .log_err();
            }));
        }

        let state = editor.read(cx);
        let Some((decorations, shown)) = colors.plan(state, &palette) else {
            return;
        };
        colors.shown = Some(shown);
        // Setting what the collection already holds does not notify the
        // editor, so this cannot loop through the observer.
        colors.decorations.set(decorations, cx);
    }

    fn finish_bracket_scan(
        &mut self,
        editor: &Entity<EditorState>,
        version: u64,
        scanned: ScriptBrackets,
        cx: &mut Context<Self>,
    ) {
        let Some(colors) = self.tabs.iter_mut().find_map(|tab| match tab {
            TabType::Query(query_tab) if &query_tab.base.editor == editor => {
                query_tab.bracket_colors.as_mut()
            }
            _ => None,
        }) else {
            return;
        };
        if colors.scanning != Some(version) {
            return;
        }
        colors.scanned = Some((version, scanned));
        self.refresh_bracket_colors(editor, cx);
    }

    /// Apply `editor.colorize_brackets` to every query tab.
    pub(super) fn refresh_all_bracket_colors(&mut self, cx: &mut Context<Self>) {
        let editors: Vec<_> = self
            .tabs
            .iter()
            .filter_map(|tab| match tab {
                TabType::Query(query_tab) => Some(query_tab.base.editor.clone()),
                _ => None,
            })
            .collect();
        for editor in editors {
            self.refresh_bracket_colors(&editor, cx);
        }
    }
}
