//! Cells whose value was left on the server when the table was read.
//!
//! [`blanco_core::deferred`] rewrites the read; this is the grid's side of
//! it. A deferred cell holds a preview, and is known by its row's primary
//! key and its column's name, which stay put while rows are sorted, filtered
//! and removed and columns are moved. Anything that needs the value (opening
//! the cell, copying it, duplicating its row) loads it first.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use blanco_core::deferred::{DeferredCell, DeferredPlan};
use blanco_core::{ColumnInfo, ColumnType, Connection, DatabaseType, QueryResult};
use database::DatabaseService;
use gpui::{AppContext as _, Context, SharedString, Window};
use gpui_component::WindowExt as _;
use gpui_component::notification::NotificationType;

use super::{ResultsPanel, ResultsTableDelegate};

/// The size in bytes above which values are deferred, 0 when the setting is
/// off.
pub(crate) fn threshold(cx: &gpui::App) -> u64 {
    u64::from(
        crate::app_settings::AppSettings::global(cx)
            .settings
            .results
            .defer_values_over_kb,
    ) * 1024
}

/// A read with large values left out, and what it takes to make sense of
/// its result.
#[derive(Clone)]
pub(crate) struct DeferredRead {
    plan: DeferredPlan,
    threshold: u64,
    key_columns: Vec<String>,
}

impl DeferredRead {
    /// The read to run in place of `query`, `None` when the query or the
    /// table does not qualify.
    pub fn plan(
        db_type: DatabaseType,
        query: &str,
        table_columns: &[ColumnInfo],
        threshold: u64,
    ) -> Option<Self> {
        let plan = db_type
            .dialect()
            .deferred_read(query, table_columns, threshold)?;
        Some(Self {
            plan,
            threshold,
            key_columns: table_columns
                .iter()
                .filter(|column| column.is_primary_key)
                .map(|column| column.name.clone())
                .collect(),
        })
    }

    /// As [`Self::plan`], looking the table's columns up first.
    pub async fn resolve(
        connection: &Arc<dyn Connection>,
        db_type: DatabaseType,
        query: &str,
        threshold: u64,
    ) -> Option<Self> {
        if !db_type.dialect().rewrites_full_reads(threshold) {
            return None;
        }
        let table = connection
            .extract_table_name_from_query(query, false)
            .ok()
            .flatten()?;
        let table_columns = connection.get_columns_for_table(&table, None).await.ok()?;
        Self::plan(db_type, query, &table_columns, threshold)
    }

    pub fn query(&self) -> &str {
        &self.plan.query
    }

    /// Turn the result of [`Self::query`] back into the result of the query
    /// it stands in for, and report the cells that hold previews.
    pub fn finish(&self, result: &mut QueryResult) -> DeferredValues {
        let cells = self.plan.apply(result, self.threshold);
        DeferredValues::from_result(result, &cells, &self.key_columns)
    }
}

/// Run `query` for the grid, with large values deferred when it qualifies.
/// A rewritten read that the server refuses is run again as written, so the
/// rewrite can only ever cost a round trip.
pub(crate) async fn execute_read(
    connection: &Arc<dyn Connection>,
    query: &str,
    database: &str,
    read: Option<&DeferredRead>,
) -> anyhow::Result<(QueryResult, DeferredValues)> {
    if let Some(read) = read {
        match connection
            .execute_query(read.query(), Some(database), None)
            .await
        {
            Ok(mut result) => {
                let deferred = read.finish(&mut result);
                return Ok((result, deferred));
            }
            Err(error) => {
                tracing::warn!("Reading with deferred values failed, reading in full: {error:#}");
            }
        }
    }
    let result = connection
        .execute_query(query, Some(database), None)
        .await?;
    Ok((result, DeferredValues::default()))
}

/// The cells of a result that hold a preview, with the size of the value
/// each stands for.
#[derive(Clone, Debug, Default)]
pub(crate) struct DeferredValues {
    /// Row key, then column name.
    sizes: HashMap<Vec<String>, HashMap<String, u64>>,
    /// Rows whose values are being fetched.
    loading: HashSet<Vec<String>>,
}

impl DeferredValues {
    fn from_result(result: &QueryResult, cells: &[DeferredCell], key_columns: &[String]) -> Self {
        let key_indices: Option<Vec<usize>> = key_columns
            .iter()
            .map(|key| result.columns.iter().position(|column| column == key))
            .collect();
        let Some(key_indices) = key_indices else {
            return Self::default();
        };
        let mut sizes: HashMap<Vec<String>, HashMap<String, u64>> = HashMap::new();
        for cell in cells {
            let key: Option<Vec<String>> = result.rows.get(cell.row).and_then(|row| {
                key_indices
                    .iter()
                    .map(|&index| row.get(index).cloned().flatten())
                    .collect()
            });
            if let (Some(key), Some(column)) = (key, result.columns.get(cell.column)) {
                sizes
                    .entry(key)
                    .or_default()
                    .insert(column.clone(), cell.size);
            }
        }
        Self {
            sizes,
            loading: HashSet::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.sizes.is_empty()
    }

    pub fn size(&self, key: &[String], column: &str) -> Option<u64> {
        self.sizes.get(key)?.get(column).copied()
    }

    pub fn is_loading(&self, key: &[String]) -> bool {
        self.loading.contains(key)
    }

    fn columns(&self, key: &[String]) -> impl Iterator<Item = &String> {
        self.sizes.get(key).into_iter().flat_map(HashMap::keys)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.sizes.values().map(HashMap::len).sum()
    }
}

impl ResultsTableDelegate {
    pub(crate) fn set_deferred_values(&mut self, deferred: DeferredValues) {
        self.deferred = deferred;
    }

    #[cfg(test)]
    pub(crate) fn deferred_values(&self) -> &DeferredValues {
        &self.deferred
    }

    /// The primary key of a row as it is on the server: a key cell edited
    /// here still reads as it was loaded.
    fn row_key(&self, row: &[Option<String>], row_ix: Option<usize>) -> Option<Vec<String>> {
        self.primary_key_column_indices()?
            .into_iter()
            .map(|index| {
                row_ix
                    .and_then(|row_ix| self.edit_state.original_values.get(&(row_ix, index)))
                    .cloned()
                    .unwrap_or_else(|| row.get(index).cloned().flatten())
            })
            .collect()
    }

    fn grid_row_key(&self, row_ix: usize) -> Option<Vec<String>> {
        // A row added here is not on the server under any key.
        if self.deferred.is_empty() || self.edit_state.is_new_row(row_ix) {
            return None;
        }
        self.row_key(self.rows.get(row_ix)?, Some(row_ix))
    }

    /// The size of the value a cell stands for, when the cell holds a
    /// preview. A cell with a pending edit holds that edit.
    pub(crate) fn deferred_size(&self, row_ix: usize, col_ix: usize) -> Option<u64> {
        if self.deferred.is_empty() || self.edit_state.is_edited(row_ix, col_ix) {
            return None;
        }
        let key = self.grid_row_key(row_ix)?;
        self.deferred
            .size(&key, self.columns.get(col_ix)?.name.as_ref())
    }

    /// Whether the row's values are on their way.
    pub(crate) fn is_loading_deferred(&self, row_ix: usize) -> bool {
        self.grid_row_key(row_ix)
            .is_some_and(|key| self.deferred.is_loading(&key))
    }

    /// The rows among `rows` that hold at least one preview.
    pub(crate) fn rows_with_deferred_values(
        &self,
        rows: impl IntoIterator<Item = usize>,
    ) -> Vec<usize> {
        if self.deferred.is_empty() {
            return Vec::new();
        }
        rows.into_iter()
            .filter(|&row_ix| {
                self.grid_row_key(row_ix)
                    .is_some_and(|key| self.deferred.sizes.contains_key(&key))
            })
            .collect()
    }

    /// The query fetching the deferred values of `rows`, and the columns it
    /// returns after the key columns: those of `only_column` when given,
    /// otherwise of every column.
    fn deferred_values_request(
        &self,
        rows: &[usize],
        only_column: Option<usize>,
    ) -> Option<DeferredRequest> {
        let only_column = match only_column {
            Some(col_ix) => Some(self.columns.get(col_ix)?.name.to_string()),
            None => None,
        };
        let table = self.table_name.as_deref()?;
        let dialect = self.db_type?.dialect();
        let key_columns: Vec<(String, ColumnType)> = self
            .primary_key_column_indices()?
            .into_iter()
            .map(|index| {
                Some((
                    self.columns.get(index)?.name.to_string(),
                    self.column_type(index),
                ))
            })
            .collect::<Option<_>>()?;
        let mut keys: Vec<Vec<String>> = Vec::new();
        let mut columns: Vec<String> = Vec::new();
        for &row_ix in rows {
            let Some(key) = self.grid_row_key(row_ix) else {
                continue;
            };
            let mut wanted = false;
            for column in self.deferred.columns(&key) {
                if only_column.as_ref().is_some_and(|only| only != column) {
                    continue;
                }
                wanted = true;
                if !columns.contains(column) {
                    columns.push(column.clone());
                }
            }
            if wanted && !keys.contains(&key) {
                keys.push(key);
            }
        }
        if keys.is_empty() || columns.is_empty() {
            return None;
        }
        // In the table's order, so the same request reads the same.
        columns.sort_by_key(|column| {
            self.columns
                .iter()
                .position(|grid_column| grid_column.name.as_ref() == column.as_str())
        });
        Some(DeferredRequest {
            query: dialect.deferred_values_query(table, &key_columns, &columns, &keys),
            key_length: key_columns.len(),
            columns,
            keys,
        })
    }

    /// Put fetched values into the grid: every row under a fetched key gets
    /// them, including rows the client-side filter is hiding.
    ///
    /// Returns whether every requested row came back. One that did not is
    /// gone from the table, and its cells keep their previews.
    fn apply_deferred_values(&mut self, request: &DeferredRequest, result: QueryResult) -> bool {
        let column_indices: Vec<Option<usize>> = request
            .columns
            .iter()
            .map(|column| {
                self.columns
                    .iter()
                    .position(|grid_column| grid_column.name.as_ref() == column.as_str())
            })
            .collect();
        let Some(key_indices) = self.primary_key_column_indices() else {
            return false;
        };
        let mut fetched: HashMap<Vec<String>, Vec<Option<String>>> = HashMap::new();
        for mut row in result.rows {
            let values = row.split_off(request.key_length.min(row.len()));
            if let Some(key) = row.into_iter().collect::<Option<Vec<String>>>() {
                fetched.insert(key, values);
            }
        }

        let original_values = &self.edit_state.original_values;
        let edited_values = &self.edit_state.edited_values;
        let deferred = &self.deferred;
        let fill = |rows: &mut Vec<Vec<Option<String>>>, visible: bool| {
            for (row_ix, row) in rows.iter_mut().enumerate() {
                let key: Option<Vec<String>> = key_indices
                    .iter()
                    .map(|&index| {
                        visible
                            .then(|| original_values.get(&(row_ix, index)))
                            .flatten()
                            .cloned()
                            .unwrap_or_else(|| row.get(index).cloned().flatten())
                    })
                    .collect();
                let Some(values) = key.as_ref().and_then(|key| fetched.get(key)) else {
                    continue;
                };
                let key = key.unwrap_or_default();
                for (value, column_index) in values.iter().zip(&column_indices) {
                    let Some(column_index) = *column_index else {
                        continue;
                    };
                    let Some(column) = self.columns.get(column_index) else {
                        continue;
                    };
                    let column = column.name.as_ref();
                    // Only cells still holding a preview: a value edited here
                    // while the fetch ran is the user's.
                    let pending_edit =
                        visible && edited_values.contains_key(&(row_ix, column_index));
                    if deferred.size(&key, column).is_some()
                        && !pending_edit
                        && let Some(cell) = row.get_mut(column_index)
                    {
                        *cell = value.clone();
                    }
                }
            }
        };
        fill(&mut self.rows, true);
        if let Some(unfiltered_rows) = self.unfiltered_rows.as_mut() {
            fill(unfiltered_rows, false);
        }

        for key in &request.keys {
            self.deferred.loading.remove(key);
            if fetched.contains_key(key)
                && let Some(sizes) = self.deferred.sizes.get_mut(key)
            {
                for column in &request.columns {
                    sizes.remove(column);
                }
                if sizes.is_empty() {
                    self.deferred.sizes.remove(key);
                }
            }
        }
        request.keys.iter().all(|key| fetched.contains_key(key))
    }
}

struct DeferredRequest {
    query: String,
    key_length: usize,
    columns: Vec<String>,
    keys: Vec<Vec<String>>,
}

impl ResultsPanel {
    /// Run an action handler again once `rows` hold their full values.
    /// Returns whether the handler has to wait for that, in which case it
    /// returns and is called again with the same action.
    pub(crate) fn defer_action<A: Clone + 'static>(
        &mut self,
        rows: impl IntoIterator<Item = usize>,
        only_column: Option<usize>,
        action: &A,
        window: &mut Window,
        cx: &mut Context<Self>,
        handler: fn(&mut Self, &A, &mut Window, &mut Context<Self>),
    ) -> bool {
        let action = action.clone();
        self.load_deferred_values_then(rows, only_column, window, cx, move |panel, window, cx| {
            handler(panel, &action, window, cx)
        })
    }

    /// Make sure `rows` hold their full values, in `only_column` when given
    /// and in every column otherwise, then run `then`. Returns whether a
    /// fetch was needed: when it was, `then` runs once the values are in the
    /// grid, and not at all if they could not be fetched.
    pub(crate) fn load_deferred_values_then(
        &mut self,
        rows: impl IntoIterator<Item = usize>,
        only_column: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> bool {
        let table_state = self.table_state.clone();
        let request = {
            let delegate = table_state.read(cx).delegate();
            let rows = delegate.rows_with_deferred_values(rows);
            delegate.deferred_values_request(&rows, only_column)
        };
        let Some(request) = request else {
            return false;
        };
        let (connection_id, database_name) = {
            let delegate = table_state.read(cx).delegate();
            (delegate.connection_id, delegate.database_name.to_string())
        };
        table_state.update(cx, |state, cx| {
            let deferred = &mut state.delegate_mut().deferred;
            deferred.loading.extend(request.keys.iter().cloned());
            state.refresh(cx);
        });

        let db_service = DatabaseService::global(cx).clone();
        let fetch = cx.background_spawn({
            let query = request.query.clone();
            async move {
                let connection = db_service
                    .get_or_create_connection(connection_id, Some(&database_name))
                    .await?;
                connection
                    .execute_query(&query, Some(&database_name), None)
                    .await
            }
        });
        cx.spawn_in(window, async move |panel, cx| {
            let result = fetch.await;
            panel
                .update_in(cx, |panel, window, cx| {
                    let loaded = match result {
                        Ok(result) => {
                            let complete = table_state.update(cx, |state, cx| {
                                let complete =
                                    state.delegate_mut().apply_deferred_values(&request, result);
                                state.refresh(cx);
                                complete
                            });
                            if !complete {
                                window.push_notification(
                                    (
                                        NotificationType::Error,
                                        SharedString::from(
                                            "Could not load the full value: the row is no longer in the table",
                                        ),
                                    ),
                                    cx,
                                );
                            }
                            complete
                        }
                        Err(error) => {
                            table_state.update(cx, |state, cx| {
                                let deferred = &mut state.delegate_mut().deferred;
                                for key in &request.keys {
                                    deferred.loading.remove(key);
                                }
                                state.refresh(cx);
                            });
                            window.push_notification(
                                (
                                    NotificationType::Error,
                                    SharedString::from(format!(
                                        "Could not load the full value: {error:#}"
                                    )),
                                ),
                                cx,
                            );
                            false
                        }
                    };
                    cx.notify();
                    if loaded {
                        then(panel, window, cx);
                    }
                })
                .ok();
        })
        .detach();
        true
    }
}
