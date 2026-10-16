//! Running the filter bar's condition on the server and showing the result in
//! the tab it was typed in.

use gpui::{AppContext as _, ClipboardItem, Context, Entity, Focusable as _, Window};

use database::DatabaseService;

use super::deferred_values::{self, DeferredRead};
use super::filter_bar::{FilterBar, FilterBarEvent, FilterColumn};
use super::{ResultViewMode, ResultsPanel};
use crate::app::FilterByCellValue;
use crate::app_settings::AppSettings;
use crate::result_ext::ResultExt as _;

impl ResultsPanel {
    /// Whether the result tab at `ix` can be filtered on the server: a table
    /// view of one table's rows, not a Redis key or a plan.
    pub(super) fn tab_supports_server_filter(&self, ix: usize, cx: &gpui::App) -> bool {
        self.result_tabs.get(ix).is_some_and(|tab| {
            tab.key_value.is_none()
                && tab.view_mode == ResultViewMode::Table
                && tab.filter_base_query.is_some()
                && tab.table_state.read(cx).delegate().supports_server_filter()
        })
    }

    /// The active tab's filter bar when the bar is open and the tab can be
    /// filtered, created on first use.
    pub(super) fn visible_filter_bar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<FilterBar>> {
        if !self.filter_bar_open || !self.tab_supports_server_filter(self.active_tab, cx) {
            return None;
        }
        self.ensure_filter_bar(self.active_tab, window, cx)
    }

    fn ensure_filter_bar(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<FilterBar>> {
        let tab = self.result_tabs.get(ix)?;
        if let Some(bar) = &tab.filter_bar {
            return Some(bar.clone());
        }
        let columns = {
            let delegate = tab.table_state.read(cx).delegate();
            delegate
                .columns
                .iter()
                .enumerate()
                .map(|(col_ix, column)| FilterColumn {
                    name: column.name.to_string(),
                    column_type: delegate
                        .column_types
                        .get(col_ix)
                        .copied()
                        .unwrap_or(blanco_core::ColumnType::Unknown),
                })
                .collect::<Vec<_>>()
        };
        let dialect = self.db_type.dialect();
        let default_operator = AppSettings::global(cx).settings.results.default_operator();
        let bar = cx.new(|cx| FilterBar::new(dialect, columns, default_operator, window, cx));
        let tab_state = tab.table_state.clone();
        let subscription = cx.subscribe_in(
            &bar,
            window,
            move |panel, bar, event: &FilterBarEvent, window, cx| match event {
                FilterBarEvent::Apply(predicate) => {
                    panel.run_server_filter(&tab_state, bar, predicate.clone(), window, cx);
                }
                FilterBarEvent::DefaultOperatorChanged(operator) => {
                    AppSettings::global_mut(cx).settings.results.filter_operator =
                        operator.key().to_string();
                    crate::settings::persist_setting(
                        "results.filter_operator",
                        operator.key().to_string(),
                        cx,
                    );
                }
                FilterBarEvent::CopySql => {
                    if let Some(query) = tab_state.read(cx).delegate().original_query.clone() {
                        cx.write_to_clipboard(ClipboardItem::new_string(query));
                    }
                }
                FilterBarEvent::Close => panel.set_filter_bar_open(false, window, cx),
            },
        );
        let tab = self.result_tabs.get_mut(ix)?;
        tab.filter_bar = Some(bar.clone());
        tab._subscriptions.push(subscription);
        Some(bar)
    }

    #[cfg(any(test, feature = "screenshots"))]
    pub fn active_filter_bar(&self) -> Option<Entity<FilterBar>> {
        self.result_tabs.get(self.active_tab)?.filter_bar.clone()
    }

    pub fn toggle_filter_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_filter_bar_open(!self.filter_bar_open, window, cx);
    }

    fn set_filter_bar_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.filter_bar_open == open {
            return;
        }
        self.filter_bar_open = open;
        AppSettings::global_mut(cx).settings.results.filter_bar_open = open;
        crate::settings::persist_setting("results.filter_bar_open", open.to_string(), cx);
        if open && let Some(bar) = self.visible_filter_bar(window, cx) {
            bar.focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    /// Re-run the tab's base query with `predicate` added and show the rows
    /// in place, keeping the table identity so the rows stay editable.
    fn run_server_filter(
        &mut self,
        table_state: &Entity<gpui_component::table::TableState<super::ResultsTableDelegate>>,
        bar: &Entity<FilterBar>,
        predicate: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self
            .result_tabs
            .iter()
            .find(|tab| tab.table_state.entity_id() == table_state.entity_id())
        else {
            return;
        };
        let Some(base_query) = tab.filter_base_query.clone() else {
            return;
        };
        let (pending_edits, table_name, table_columns) = {
            let delegate = table_state.read(cx).delegate();
            (
                delegate.has_pending_changes(),
                delegate.table_name.clone(),
                delegate.table_columns().to_vec(),
            )
        };
        if pending_edits {
            bar.update(cx, |bar, cx| {
                bar.finish(
                    Err("Commit or discard pending edits before filtering".to_string()),
                    cx,
                )
            });
            return;
        }
        let query = match predicate {
            Some(predicate) => match self.db_type.dialect().with_filter(&base_query, &predicate) {
                Ok(query) => query,
                Err(error) => {
                    bar.update(cx, |bar, cx| bar.finish(Err(error), cx));
                    return;
                }
            },
            None => base_query,
        };

        let filter_input = tab.filter_input.clone();
        let chart_view = tab.chart_view.clone();
        let db_service = DatabaseService::global(cx).clone();
        let connection_id = self.connection_id;
        let database_name = self.database_name.clone();
        let table_state = table_state.clone();
        let bar = bar.downgrade();
        let deferred_read = DeferredRead::plan(
            self.db_type,
            &query,
            &table_columns,
            deferred_values::threshold(cx),
        );
        let task = cx.background_spawn({
            let query = query.clone();
            async move {
                let connection = db_service
                    .get_or_create_connection(connection_id, Some(&database_name))
                    .await?;
                deferred_values::execute_read(
                    &connection,
                    &query,
                    &database_name,
                    deferred_read.as_ref(),
                )
                .await
            }
        });
        cx.spawn_in(window, async move |panel, cx| {
            let result = task.await;
            panel
                .update_in(cx, |_panel, window, cx| {
                    let outcome = match result {
                        Ok((mut result, deferred)) => {
                            let rows = result.rows.len();
                            result.query_text = Some(query.clone());
                            result.table_name = table_name;
                            result.table_columns = Some(table_columns);
                            // The client-side filter applied to the old rows.
                            filter_input.update(cx, |input, cx| input.set_value("", window, cx));
                            table_state.update(cx, |state, cx| {
                                let delegate = state.delegate_mut();
                                delegate.set_original_query(query);
                                delegate.set_query_result(result, window, cx);
                                delegate.set_deferred_values(deferred);
                                state.clear_selection(cx);
                                state.refresh(cx);
                            });
                            chart_view
                                .update(cx, |view, cx| view.rebuild_column_selects(window, cx));
                            Ok(rows)
                        }
                        Err(error) => Err(format!("{error:#}")),
                    };
                    bar.update(cx, |bar, cx| bar.finish(outcome, cx)).ok();
                    cx.notify();
                })
                .log_err();
        })
        .detach();
    }

    /// Open the filter bar on the clicked cell's column and value.
    pub(super) fn on_filter_by_cell_value(
        &mut self,
        action: &FilterByCellValue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.defer_action(
            [action.row],
            Some(action.col),
            action,
            window,
            cx,
            Self::on_filter_by_cell_value,
        ) {
            return;
        }
        let (column, value) = {
            let delegate = self.table_state.read(cx).delegate();
            let Some(column) = delegate.columns.get(action.col) else {
                return;
            };
            let value = delegate
                .rows
                .get(action.row)
                .and_then(|row| row.get(action.col))
                .cloned()
                .flatten();
            (column.name.to_string(), value)
        };
        self.set_filter_bar_open(true, window, cx);
        if let Some(bar) = self.visible_filter_bar(window, cx) {
            bar.update(cx, |bar, cx| {
                bar.filter_by_value(&column, value.as_deref(), window, cx)
            });
        }
    }
}
