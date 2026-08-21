use std::future::Future;
use std::rc::Rc;

use blanco_ui::IconName;
use gpui::{
    App, AppContext, ClipboardItem, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, Size, Styled, Subscription, Task, Window, div,
    prelude::FluentBuilder, px, size,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, InteractiveElementExt as _, Sizable as _,
    VirtualListScrollHandle,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    scroll::{ScrollableElement as _, ScrollbarAxis},
    v_flex, v_virtual_list,
};

use crate::app_database::{AppDatabase, QueryHistoryData};
use crate::result_ext::ResultExt as _;
use crate::time_format;

/// Maximum number of history rows loaded into the panel at once.
const HISTORY_LIMIT: i64 = 200;

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

pub struct HistoryPanel {
    entries: Vec<QueryHistoryData>,
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
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search history..."));

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
            _subscriptions: vec![search_subscription],
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
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let Some(entries) = load.await.log_err() else {
                return;
            };
            this.update(cx, |this, cx| {
                this.item_sizes = row_sizes(entries.len());
                this.entries = entries;
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
        if let Some(entry) = self.entries.get(index) {
            cx.write_to_clipboard(ClipboardItem::new_string(entry.query_text.clone()));
        }
    }

    fn delete_entry(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(id) = self.entries.get(index).and_then(|entry| entry.id) else {
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

    fn render_entry(&self, index: usize, cx: &Context<Self>) -> impl IntoElement {
        let entry = &self.entries[index];

        let (status_icon, status_color) = if entry.success {
            (IconName::CircleCheck, cx.theme().green)
        } else {
            (IconName::CircleX, cx.theme().red)
        };

        // The query is stored verbatim; collapse whitespace so the one-line
        // preview stays compact regardless of the original formatting.
        let preview: String = entry
            .query_text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");

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

        let query_for_insert = entry.query_text.clone();

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
                let _ = this;
                cx.emit(HistoryPanelEvent::InsertQuery(query_for_insert.clone()));
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
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(meta_line),
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
            .load_query_history(HISTORY_LIMIT, search)
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
                    .py_1p5()
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
                                            .map(|index| {
                                                this.render_entry(index, cx).into_any_element()
                                            })
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
