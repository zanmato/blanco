//! Remembering and changing how a result's columns are arranged.
//!
//! A whole-table result keeps its layout per table, saved in the settings
//! table so it outlives the session. Any other result keeps it per query
//! text for the session, so a rerun of the same query keeps its columns.

use std::collections::HashMap;

use gpui::{
    App, Context, Entity, Global, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::Input,
    notification::NotificationType,
    popover::Popover,
    table::TableState,
    v_flex,
};

use super::{ColumnLayout, ResultsPanel, ResultsTableDelegate};
use crate::app::{HideResultColumn, PinResultColumn};
use blanco_ui::IconName;

/// Settings key prefix of a table's saved column layout.
const TABLE_LAYOUT_PREFIX: &str = "column_layout.";

/// Column layouts by key: saved table layouts, and this session's layouts of
/// other queries.
#[derive(Default)]
pub struct ColumnLayouts {
    layouts: HashMap<String, ColumnLayout>,
}

impl Global for ColumnLayouts {}

impl ColumnLayouts {
    /// The saved table layouts among the stored settings.
    pub fn from_key_values(values: &[(String, String)]) -> Self {
        let layouts = values
            .iter()
            .filter(|(key, _)| key.starts_with(TABLE_LAYOUT_PREFIX))
            .filter_map(|(key, value)| {
                serde_json::from_str(value)
                    .inspect_err(|error| {
                        tracing::warn!("Ignoring unreadable column layout {key}: {error}")
                    })
                    .ok()
                    .map(|layout| (key.clone(), layout))
            })
            .collect();
        Self { layouts }
    }
}

/// Where a result's layout is kept: under its table when it shows a whole
/// table, else under its query.
fn layout_key(
    connection_id: i64,
    database: &str,
    delegate: &ResultsTableDelegate,
) -> Option<(String, bool)> {
    if delegate.shows_whole_table() {
        let table = delegate.table_name.as_deref()?;
        return Some((
            format!("{TABLE_LAYOUT_PREFIX}{connection_id}.{database}.{table}"),
            true,
        ));
    }
    let query = delegate.original_query.as_deref()?.trim();
    (!query.is_empty()).then(|| (format!("query:{connection_id}:{database}:{query}"), false))
}

impl ResultsPanel {
    /// Arrange a freshly loaded result as its table or query was last left.
    pub(super) fn restore_column_layout(
        &self,
        table_state: &Entity<TableState<ResultsTableDelegate>>,
        cx: &mut App,
    ) {
        let Some((key, _)) = layout_key(
            self.connection_id,
            &self.database_name,
            table_state.read(cx).delegate(),
        ) else {
            return;
        };
        let Some(layout) = cx
            .try_global::<ColumnLayouts>()
            .and_then(|layouts| layouts.layouts.get(&key))
            .cloned()
        else {
            return;
        };
        table_state.update(cx, |state, cx| {
            state.delegate_mut().apply_column_layout(&layout);
            state.refresh(cx);
        });
    }

    /// Remember the active result's arrangement, saving it when it belongs
    /// to a table.
    pub(super) fn remember_column_layout(
        &self,
        table_state: &Entity<TableState<ResultsTableDelegate>>,
        cx: &mut App,
    ) {
        let delegate = table_state.read(cx).delegate();
        let Some((key, persist)) = layout_key(self.connection_id, &self.database_name, delegate)
        else {
            return;
        };
        let layout = delegate.column_layout();
        let layouts = cx.default_global::<ColumnLayouts>();
        if layouts.layouts.get(&key) == Some(&layout) {
            return;
        }
        layouts.layouts.insert(key.clone(), layout.clone());
        if persist {
            let Ok(value) = serde_json::to_string(&layout) else {
                return;
            };
            let database = app_database::AppDatabase::global(cx).clone();
            let save = gpui_tokio::Tokio::spawn_result(cx, async move {
                database
                    .save_setting(&key, &value, false)
                    .await
                    .map_err(anyhow::Error::from)
            });
            cx.spawn(async move |_| {
                if let Err(error) = save.await {
                    tracing::error!("Failed to save the column layout: {error:#}");
                }
            })
            .detach();
        }
    }

    /// Change the active result's columns with `change`, then remember the
    /// new layout. Pending edits keep the columns in place; say so.
    fn rearrange_columns(
        &mut self,
        change: impl FnOnce(&mut ResultsTableDelegate) -> bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let table_state = self.table_state.clone();
        if !table_state.read(cx).delegate().can_rearrange_columns() {
            window.push_notification(
                (
                    NotificationType::Info,
                    "Apply or discard the pending edits to change the columns",
                ),
                cx,
            );
            return;
        }
        let changed = table_state.update(cx, |state, cx| {
            let changed = change(state.delegate_mut());
            if changed {
                state.clear_selection(cx);
                state.refresh(cx);
            }
            changed
        });
        if changed {
            self.remember_column_layout(&table_state, cx);
            // The chart picks its columns by position.
            if let Some(tab) = self.result_tabs.get(self.active_tab) {
                tab.chart_view
                    .update(cx, |view, cx| view.rebuild_column_selects(window, cx));
            }
            cx.notify();
        }
    }

    /// Show or hide the active result's column called `name`.
    pub fn set_result_column_visible(
        &mut self,
        name: &str,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = name.to_string();
        self.rearrange_columns(
            move |delegate| {
                let Some(ix) = delegate.columns.iter().position(|c| c.name == name) else {
                    return false;
                };
                if visible {
                    delegate.show_column(ix)
                } else {
                    delegate.hide_column(ix)
                }
            },
            window,
            cx,
        );
    }

    /// Pin or unpin the active result's column called `name`.
    pub fn set_result_column_pinned(
        &mut self,
        name: &str,
        pinned: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = name.to_string();
        self.rearrange_columns(
            move |delegate| {
                delegate
                    .columns
                    .iter()
                    .position(|c| c.name == name)
                    .is_some_and(|ix| delegate.set_column_pinned(ix, pinned))
            },
            window,
            cx,
        );
    }

    pub(super) fn on_hide_result_column(
        &mut self,
        action: &HideResultColumn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let col = action.col;
        self.rearrange_columns(move |delegate| delegate.hide_column(col), window, cx);
    }

    pub(super) fn on_pin_result_column(
        &mut self,
        action: &PinResultColumn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (col, pinned) = (action.col, action.pinned);
        self.rearrange_columns(
            move |delegate| delegate.set_column_pinned(col, pinned),
            window,
            cx,
        );
    }

    /// The Columns button of the tab strip and its menu: every column with a
    /// checkbox to show it and a pin, filtered by a search field.
    pub(super) fn render_column_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let panel = cx.entity();
        let search = self.column_search.clone();
        let table_state = self.table_state.clone();
        Popover::new("result-columns-menu")
            .trigger(
                Button::new("result-columns")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Eye)
                    .tooltip("Columns"),
            )
            .content(move |_, _window, cx| {
                let needle = search.read(cx).value().to_lowercase();
                let delegate = table_state.read(cx).delegate();
                let visible = delegate.visible_column_count();
                let pinned = delegate.pinned_column_count();
                let can_rearrange = delegate.can_rearrange_columns();
                let columns: Vec<(usize, String)> = delegate
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(ix, column)| (ix, column.name.to_string()))
                    .filter(|(_, name)| needle.is_empty() || name.to_lowercase().contains(&needle))
                    .collect();
                let muted = cx.theme().muted_foreground;
                v_flex()
                    .p_2()
                    .gap_2()
                    .w(px(280.))
                    .child(Input::new(&search).small().cleanable(true))
                    .when(!can_rearrange, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child("Apply or discard the pending edits to change the columns."),
                        )
                    })
                    .child(
                        v_flex()
                            .id("result-columns-list")
                            .max_h(px(320.))
                            .overflow_y_scroll()
                            .gap_1()
                            .children(columns.into_iter().map(|(ix, name)| {
                                let shown = ix < visible;
                                let is_pinned = ix < pinned;
                                let last_shown = shown && visible <= 1;
                                h_flex()
                                    .gap_2()
                                    .justify_between()
                                    .child(
                                        Checkbox::new(("result-column-shown", ix))
                                            .small()
                                            .label(name.clone())
                                            .checked(shown)
                                            .disabled(!can_rearrange || last_shown)
                                            .on_click({
                                                let panel = panel.clone();
                                                let name = name.clone();
                                                move |checked: &bool, window, cx| {
                                                    panel.update(cx, |panel, cx| {
                                                        panel.set_result_column_visible(
                                                            &name, *checked, window, cx,
                                                        );
                                                    });
                                                }
                                            }),
                                    )
                                    .when(shown, |this| {
                                        this.child(
                                            Button::new(("result-column-pin", ix))
                                                .ghost()
                                                .xsmall()
                                                .icon(if is_pinned {
                                                    IconName::PinOff
                                                } else {
                                                    IconName::Pin
                                                })
                                                .tooltip(if is_pinned {
                                                    "Unpin Column"
                                                } else {
                                                    "Pin Column"
                                                })
                                                .disabled(!can_rearrange)
                                                .on_click({
                                                    let panel = panel.clone();
                                                    let name = name.clone();
                                                    move |_, window, cx| {
                                                        panel.update(cx, |panel, cx| {
                                                            panel.set_result_column_pinned(
                                                                &name, !is_pinned, window, cx,
                                                            );
                                                        });
                                                    }
                                                }),
                                        )
                                    })
                            })),
                    )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_table_layouts_load_and_others_are_skipped() {
        let layout = ColumnLayout {
            order: vec!["id".into(), "name".into()],
            pinned: 1,
            hidden: vec!["notes".into()],
            widths: HashMap::from([("name".into(), 200.)]),
        };
        let values = vec![
            (
                "column_layout.1.app.users".to_string(),
                serde_json::to_string(&layout).unwrap(),
            ),
            ("column_layout.1.app.broken".to_string(), "{".to_string()),
            ("editor.font_size".to_string(), "14".to_string()),
        ];
        let layouts = ColumnLayouts::from_key_values(&values);
        assert_eq!(layouts.layouts.len(), 1);
        assert_eq!(
            layouts.layouts.get("column_layout.1.app.users"),
            Some(&layout)
        );
    }
}
