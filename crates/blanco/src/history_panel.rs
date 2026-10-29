use std::future::Future;
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use blanco_core::SqlGrammar;
use blanco_ui::{IconName, highlight_each};
use gpui::{
    AnyElement, App, AppContext, ClipboardItem, Context, Entity, EventEmitter, HighlightStyle,
    InteractiveElement, IntoElement, ParentElement, Pixels, Render, SharedString, Size,
    StatefulInteractiveElement, Styled, StyledText, Subscription, Task, Window, div,
    prelude::FluentBuilder, px, size,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, InteractiveElementExt as _, Sizable as _,
    VirtualListScrollHandle,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    scroll::{ScrollableElement as _, ScrollbarAxis},
    tooltip::Tooltip,
    v_flex, v_virtual_list,
};
use gpui_component::{Theme, highlighter::HighlightTheme};

use crate::result_ext::ResultExt as _;
use crate::time_format;
use app_database::{AppDatabase, QueryHistoryData};

/// Maximum number of history rows loaded into the panel at once.
const HISTORY_LIMIT: i64 = 200;

/// Characters of each query loaded for its preview. Whitespace is collapsed
/// after loading, so this leaves room for indentation and line breaks.
const PREVIEW_LOAD_CHARS: i64 = 150;

/// Characters a preview keeps: a row shows one truncated line, about 30
/// characters in a sidebar of the default width, and the text system shapes
/// all of a line to truncate it. This leaves room for a wider sidebar.
const PREVIEW_CHARS: usize = 100;

/// Fixed height of a single history row. Each row renders a one-line query
/// preview plus a one-line meta row, so the height is constant, which lets the
/// virtual list compute item offsets without measuring every row.
const ROW_HEIGHT: Pixels = px(50.);

/// Emitted when the user picks a query from the history list. The app wires
/// this into the active editor tab.
#[derive(Clone, Debug)]
pub enum HistoryPanelEvent {
    InsertQuery(String),
}

impl EventEmitter<HistoryPanelEvent> for HistoryPanel {}

/// A loaded history entry. Its `query_text` holds only the start of the
/// query; copying or inserting it reads the whole from the database.
struct HistoryRow {
    entry: QueryHistoryData,
    preview: SharedString,
    highlights: Box<[(Range<usize>, HighlightStyle)]>,
}

impl HistoryRow {
    /// The rows of `entries` with their previews highlighted as `language`.
    /// Meant for a background thread.
    fn highlighted(
        entries: Vec<QueryHistoryData>,
        language: &str,
        theme: &HighlightTheme,
    ) -> Vec<Self> {
        let mut rows: Vec<Self> = entries.into_iter().map(Self::new).collect();
        let highlights =
            highlight_each(language, rows.iter().map(|row| row.preview.as_ref()), theme);
        for (row, highlights) in rows.iter_mut().zip(highlights) {
            row.highlights = highlights;
        }
        rows
    }

    fn new(entry: QueryHistoryData) -> Self {
        // Collapse whitespace so the one-line preview stays compact
        // regardless of the original formatting.
        let mut preview = String::new();
        let mut chars = 0;
        for word in entry.query_text.split_whitespace() {
            if !preview.is_empty() {
                preview.push(' ');
                chars += 1;
            }
            preview.push_str(word);
            chars += word.chars().count();
            if chars >= PREVIEW_CHARS {
                break;
            }
        }
        if let Some((cut, _)) = preview.char_indices().nth(PREVIEW_CHARS) {
            preview.truncate(cut);
        }
        Self {
            entry,
            preview: preview.into(),
            highlights: Box::default(),
        }
    }
}

pub struct HistoryPanel {
    entries: Vec<HistoryRow>,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    search_input: Entity<InputState>,
    scroll_handle: VirtualListScrollHandle,
    /// In-flight history load. Replaced on every reload so a newer search
    /// cancels the previous query and stale results never land after fresh ones.
    load_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl HistoryPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search history"));

        let search_subscription =
            cx.subscribe(&search_input, |this, _input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.reload(cx);
                }
            });

        let mut this = Self {
            entries: Vec::new(),
            item_sizes: row_sizes(0),
            search_input,
            scroll_handle: VirtualListScrollHandle::new(),
            load_task: None,
            // Previews keep the colors of the theme they were highlighted
            // with, so a theme change loads them again.
            _subscriptions: vec![
                search_subscription,
                cx.observe_global::<Theme>(|this, cx| this.reload(cx)),
            ],
        };
        this.reload(cx);
        this
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let search = self.search_input.read(cx).value().to_string();
        let search = if search.trim().is_empty() {
            None
        } else {
            Some(search)
        };
        let load = load_history(cx, search);
        let theme = cx.theme().highlight_theme.clone();
        // The ANSI grammar is registered at startup, and entries come from
        // every backend, so the list loads no backend's grammar for them.
        let language = crate::sql::grammar_language(SqlGrammar::Ansi);
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let Some(entries) = load.await.log_err() else {
                return;
            };
            let rows = cx
                .background_spawn(async move { HistoryRow::highlighted(entries, language, &theme) })
                .await;
            this.update(cx, |this, cx| {
                this.item_sizes = row_sizes(rows.len());
                this.entries = rows;
                cx.notify();
            })
            .log_err();
        }));
    }

    /// Run a history mutation on tokio, then reload the list once it finishes.
    fn mutate_then_reload(
        &mut self,
        mutation: impl Future<Output = anyhow::Result<()>> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let task = gpui_tokio::Tokio::spawn_result(cx, mutation);
        cx.spawn(async move |this, cx| {
            task.await.log_err();
            this.update(cx, |this, cx| this.reload(cx)).log_err();
        })
        .detach();
    }

    fn copy_entry(&mut self, index: usize, cx: &mut Context<Self>) {
        self.with_query_text(index, cx, |_, query, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(query));
        });
    }

    fn insert_entry(&mut self, index: usize, cx: &mut Context<Self>) {
        self.with_query_text(index, cx, |_, query, cx| {
            cx.emit(HistoryPanelEvent::InsertQuery(query));
        });
    }

    /// Read the whole query of the entry at `index` and hand it to `then`.
    fn with_query_text(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, String, &mut Context<Self>) + 'static,
    ) {
        let Some(id) = self.entries.get(index).and_then(|row| row.entry.id) else {
            return;
        };
        let app_database = AppDatabase::global(cx).clone();
        let load = gpui_tokio::Tokio::spawn_result(cx, async move {
            app_database
                .query_history_text(id)
                .await
                .map_err(anyhow::Error::from)
        });
        cx.spawn(async move |this, cx| {
            let Some(Some(query)) = load.await.log_err() else {
                return;
            };
            this.update(cx, |this, cx| then(this, query, cx)).log_err();
        })
        .detach();
    }

    fn delete_entry(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(id) = self.entries.get(index).and_then(|row| row.entry.id) else {
            return;
        };
        let app_database = AppDatabase::global(cx).clone();
        self.mutate_then_reload(
            async move {
                app_database
                    .delete_query_history_entry(id)
                    .await
                    .map_err(anyhow::Error::from)
            },
            cx,
        );
    }

    fn clear_all(&mut self, cx: &mut Context<Self>) {
        let app_database = AppDatabase::global(cx).clone();
        self.mutate_then_reload(
            async move {
                app_database
                    .clear_query_history()
                    .await
                    .map_err(anyhow::Error::from)
            },
            cx,
        );
    }

    fn render_entry(&self, index: usize, cx: &Context<Self>) -> AnyElement {
        let Some(row) = self.entries.get(index) else {
            return div().into_any_element();
        };
        let entry = &row.entry;

        let (status_icon, status_color) = if entry.success {
            (IconName::CircleCheck, cx.theme().green)
        } else {
            (IconName::CircleX, cx.theme().red)
        };

        let preview =
            StyledText::new(row.preview.clone()).with_highlights(row.highlights.iter().cloned());

        let mut meta = vec![time_format::format_relative(entry.executed_at)];
        if entry.success
            && let Some(rows) = entry.row_count
        {
            meta.push(format!("{rows} rows"));
        }
        if let Some(name) = &entry.connection_name {
            meta.push(name.clone());
        }
        let meta_line = meta.join("  ·  ");
        let timing = entry
            .execution_us
            .map(|execution_us| blanco_core::QueryTiming {
                execution: Duration::from_micros(execution_us.max(0) as u64),
                fetch: entry
                    .fetch_us
                    .map(|fetch_us| Duration::from_micros(fetch_us.max(0) as u64)),
            });

        h_flex()
            .id(("history-entry", index))
            .group("history-entry")
            .relative()
            .w_full()
            .h(ROW_HEIGHT)
            .px_3()
            .py_1p5()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .hover(|this| this.bg(cx.theme().accent))
            .cursor_pointer()
            .on_double_click(cx.listener(move |this, _event, _window, cx| {
                this.insert_entry(index, cx);
            }))
            .child(
                Icon::new(status_icon)
                    .size(px(13.))
                    .text_color(status_color),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(12.))
                            .text_color(cx.theme().foreground)
                            .truncate()
                            .child(preview),
                    )
                    .child(
                        div()
                            .id(("history-meta", index))
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(meta_line)
                            .when_some(timing, |this, timing| {
                                let text = time_format::format_timing(&timing);
                                this.tooltip(move |window, cx| {
                                    Tooltip::new(text.clone()).build(window, cx)
                                })
                            }),
                    ),
            )
            // The action buttons overlay the top-right corner on hover so they
            // never reserve horizontal space from the preview/meta text.
            .child(
                h_flex()
                    .absolute()
                    .top_1()
                    // Offset from the right edge so the buttons clear the
                    // virtual list's scrollbar instead of overlapping it.
                    .right_3()
                    .gap_1()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().accent)
                    .invisible()
                    .group_hover("history-entry", |this| this.visible())
                    .child(
                        Button::new(("history-copy", index))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Copy)
                            .tooltip("Copy")
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.copy_entry(index, cx);
                            })),
                    )
                    .child(
                        Button::new(("history-delete", index))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Trash)
                            .tooltip("Delete")
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.delete_entry(index, cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}

/// Build the per-row size vector consumed by the virtual list. Width is ignored
/// for a vertical list (it measures the first row instead), so only the height
/// matters here.
fn row_sizes(count: usize) -> Rc<Vec<Size<Pixels>>> {
    Rc::new(vec![size(px(0.), ROW_HEIGHT); count])
}

fn load_history(cx: &App, search: Option<String>) -> Task<anyhow::Result<Vec<QueryHistoryData>>> {
    let app_database = AppDatabase::global(cx).clone();
    gpui_tokio::Tokio::spawn_result(cx, async move {
        app_database
            .load_query_history_previews(HISTORY_LIMIT, search, PREVIEW_LOAD_CHARS)
            .await
            .map_err(anyhow::Error::from)
    })
}

impl Render for HistoryPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_entries = !self.entries.is_empty();

        v_flex()
            .id("history-panel")
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .px_2()
                    .pb_1p5()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(div().flex_1().child(Input::new(&self.search_input).small()))
                    .child(
                        Button::new("history-clear")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Trash)
                            .tooltip("Clear all history")
                            .disabled(!has_entries)
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.clear_all(cx);
                            })),
                    ),
            )
            // The list lives inside a `flex_1`/`min_h_0` container so it's
            // bounded to the remaining height; the virtual list only renders the
            // rows currently in view, which keeps scrolling smooth for large
            // histories.
            .child(
                div().flex_1().min_h_0().child(
                    div()
                        .relative()
                        .size_full()
                        .when(!has_entries, |this| {
                            this.flex().items_center().justify_center().child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child("No query history yet"),
                            )
                        })
                        .when(has_entries, |this| {
                            this.child(
                                v_virtual_list(
                                    cx.entity(),
                                    "history-list",
                                    self.item_sizes.clone(),
                                    move |this, visible_range, _window, cx| {
                                        visible_range
                                            .map(|index| this.render_entry(index, cx))
                                            .collect()
                                    },
                                )
                                .track_scroll(&self.scroll_handle),
                            )
                            .scrollbar(&self.scroll_handle, ScrollbarAxis::Vertical)
                        }),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_collapse_whitespace_and_stop_at_their_length() {
        let mut entry = QueryHistoryData {
            id: Some(1),
            query_text: "SELECT *\n    FROM   albums\n\tWHERE id = 1".to_string(),
            executed_at: 0,
            duration_ms: None,
            execution_us: None,
            fetch_us: None,
            rows_affected: None,
            row_count: None,
            success: true,
            error_message: None,
            connection_id: None,
            connection_name: None,
            database_name: None,
        };
        let row = HistoryRow::new(entry.clone());
        assert_eq!(row.preview.as_ref(), "SELECT * FROM albums WHERE id = 1");

        entry.query_text = "INSERT INTO t VALUES ".to_string() + &"(1, 'å'), ".repeat(100);
        let row = HistoryRow::new(entry);
        assert_eq!(row.preview.chars().count(), PREVIEW_CHARS);
    }
}
