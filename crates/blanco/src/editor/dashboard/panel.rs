//! One panel of a dashboard: the result of its query shown as a table, a
//! single number or a chart.

use std::collections::HashSet;

use app_database::DashboardPanelData;
use blanco_core::{ConnectionContext, DatabaseType, QueryResult};
use gpui::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, IntoElement, ParentElement,
    Render, SharedString, Styled, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _,
    table::{DataTable, TableSelection, TableState},
    v_flex,
};
use serde::{Deserialize, Serialize};

use crate::copy_handler::CopyHandler;
use crate::results_panel::{
    Aggregate, ChartKind, ChartSpec, ChartView, ColumnLayout, ResultsTableDelegate,
    selected_table_data,
};

/// How a panel shows its result, kept as JSON in the panel's `view`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PanelView {
    Table {
        #[serde(default)]
        layout: ColumnLayout,
        /// Colors one column's numbers.
        #[serde(default)]
        thresholds: Option<Thresholds>,
    },
    Stat(StatSpec),
    Chart(ChartSpec),
}

/// When a number turns yellow and when red. A critical value below the
/// warning one means lower is worse.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Thresholds {
    /// The column a table colors. A stat colors its own value.
    #[serde(default)]
    pub column: Option<String>,
    pub warning: f64,
    pub critical: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThresholdLevel {
    Ok,
    Warning,
    Critical,
}

impl Thresholds {
    pub fn level(&self, value: f64) -> ThresholdLevel {
        let reached = |limit: f64| {
            if self.critical >= self.warning {
                value >= limit
            } else {
                value <= limit
            }
        };
        if reached(self.critical) {
            ThresholdLevel::Critical
        } else if reached(self.warning) {
            ThresholdLevel::Warning
        } else {
            ThresholdLevel::Ok
        }
    }

    /// The level of a cell's text, `None` when it is no number.
    pub fn level_of(&self, text: &str) -> Option<ThresholdLevel> {
        text.trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(|value| self.level(value))
    }
}

impl ThresholdLevel {
    pub fn color(self, cx: &App) -> gpui::Hsla {
        let theme = cx.theme();
        match self {
            ThresholdLevel::Ok => theme.green,
            ThresholdLevel::Warning => theme.yellow,
            ThresholdLevel::Critical => theme.red,
        }
    }
}

/// A single number: one column of the first row, or of every row folded
/// into one value.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StatSpec {
    /// The column, `None` for the first numeric one.
    pub value: Option<String>,
    #[serde(default)]
    pub aggregate: Aggregate,
    #[serde(default)]
    pub thresholds: Option<Thresholds>,
}

/// What a panel can be switched to on the dashboard, in menu order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelKind {
    Table,
    Stat,
    Chart(ChartKind),
}

impl PanelKind {
    pub fn all() -> [PanelKind; 5] {
        [
            PanelKind::Table,
            PanelKind::Stat,
            PanelKind::Chart(ChartKind::Line),
            PanelKind::Chart(ChartKind::Bar),
            PanelKind::Chart(ChartKind::Pie),
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            PanelKind::Table => "Table",
            PanelKind::Stat => "Stat",
            PanelKind::Chart(kind) => kind.label(),
        }
    }
}

impl Default for PanelView {
    fn default() -> Self {
        PanelView::Table {
            layout: ColumnLayout::default(),
            thresholds: None,
        }
    }
}

impl PanelView {
    /// Read a saved view. A view this build cannot read shows as a table.
    pub fn from_json(text: &str) -> Self {
        serde_json::from_str(text).unwrap_or_else(|error| {
            tracing::warn!("Showing a dashboard panel as a table, its view did not read: {error}");
            PanelView::default()
        })
    }

    pub fn to_json(&self) -> String {
        // The view is plain data and always serializes.
        serde_json::to_string(self).unwrap_or_else(|_| "{\"type\":\"table\"}".to_string())
    }

    /// This view, saved from a query tab, with the settings the panel had
    /// that a query tab does not show: a stat's spec, and thresholds.
    pub fn keeping_settings_of(self, previous: &PanelView) -> PanelView {
        match (self, previous) {
            (PanelView::Table { .. }, PanelView::Stat(_)) => previous.clone(),
            (PanelView::Table { layout, .. }, PanelView::Table { thresholds, .. }) => {
                PanelView::Table {
                    layout,
                    thresholds: thresholds.clone(),
                }
            }
            (view, _) => view,
        }
    }

    /// The thresholds the view colors with, when it can have them.
    pub fn thresholds(&self) -> Option<&Thresholds> {
        match self {
            PanelView::Table { thresholds, .. } => thresholds.as_ref(),
            PanelView::Stat(stat) => stat.thresholds.as_ref(),
            PanelView::Chart(_) => None,
        }
    }

    pub fn with_thresholds(&self, thresholds: Option<Thresholds>) -> PanelView {
        match self {
            PanelView::Table { layout, .. } => PanelView::Table {
                layout: layout.clone(),
                thresholds,
            },
            PanelView::Stat(stat) => PanelView::Stat(StatSpec {
                thresholds,
                ..stat.clone()
            }),
            PanelView::Chart(_) => self.clone(),
        }
    }

    pub fn kind(&self) -> PanelKind {
        match self {
            PanelView::Table { .. } => PanelKind::Table,
            PanelView::Stat(_) => PanelKind::Stat,
            PanelView::Chart(spec) => PanelKind::Chart(spec.kind),
        }
    }

    /// The view as `kind`, keeping what carries over: a chart's columns when
    /// it becomes another chart, its value column when it becomes a stat.
    pub fn with_kind(&self, kind: PanelKind) -> Self {
        match (self, kind) {
            (_, PanelKind::Table) => match self {
                PanelView::Table { .. } => self.clone(),
                _ => PanelView::default(),
            },
            (PanelView::Chart(spec), PanelKind::Stat) => PanelView::Stat(StatSpec {
                value: spec.value.clone(),
                aggregate: spec.aggregate,
                thresholds: None,
            }),
            (PanelView::Stat(stat), PanelKind::Stat) => PanelView::Stat(stat.clone()),
            (_, PanelKind::Stat) => PanelView::Stat(StatSpec::default()),
            (PanelView::Chart(spec), PanelKind::Chart(chart)) => PanelView::Chart(ChartSpec {
                kind: chart,
                ..spec.clone()
            }),
            (PanelView::Stat(stat), PanelKind::Chart(chart)) => PanelView::Chart(ChartSpec {
                value: stat.value.clone(),
                aggregate: stat.aggregate,
                ..ChartSpec::new(chart)
            }),
            (PanelView::Table { .. }, PanelKind::Chart(chart)) => {
                PanelView::Chart(ChartSpec::new(chart))
            }
        }
    }
}

/// A panel's query and where it runs.
#[derive(Clone, Debug)]
pub struct PanelQuery {
    pub panel_id: i64,
    pub db_type: DatabaseType,
    pub connection_id: i64,
    pub connection_name: String,
    pub database_name: String,
    pub query: String,
}

pub struct DashboardPanel {
    pub data: DashboardPanelData,
    pub view: PanelView,
    table_state: Entity<TableState<ResultsTableDelegate>>,
    chart_view: Entity<ChartView>,
    loading: bool,
    error: Option<String>,
    has_result: bool,
}

impl DashboardPanel {
    pub fn new(data: DashboardPanelData, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let view = PanelView::from_json(&data.view);
        let table_state = cx.new(|cx| {
            let mut delegate = ResultsTableDelegate::default();
            delegate.presentation = true;
            TableState::new(delegate, window, cx)
                .row_selectable(true)
                .cell_selectable(true)
                .row_numbers(false)
        });
        let chart_view = cx.new(|cx| ChartView::presentation(table_state.clone(), window, cx));
        Self {
            data,
            view,
            table_state,
            chart_view,
            loading: false,
            error: None,
            has_result: false,
        }
    }

    /// The connection the panel runs on, `None` once it was deleted.
    pub fn context(&self) -> Option<ConnectionContext> {
        let connection = self.data.connection.as_ref()?;
        Some(ConnectionContext {
            connection_id: connection.id,
            connection_name: connection.name.clone(),
            db_type: DatabaseType::from_db_type_str(&connection.db_type)?,
            database_name: self.data.database_name.clone(),
            schema_name: self.data.schema_name.clone(),
            environment_type: Some(connection.environment_type),
        })
    }

    pub fn query(&self) -> Option<PanelQuery> {
        let connection = self.data.connection.as_ref()?;
        Some(PanelQuery {
            panel_id: self.data.id,
            db_type: DatabaseType::from_db_type_str(&connection.db_type)?,
            connection_id: connection.id,
            connection_name: connection.name.clone(),
            database_name: self.data.database_name.clone(),
            query: self.data.query.clone(),
        })
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn row_count(&self, cx: &App) -> Option<usize> {
        self.has_result
            .then(|| self.table_state.read(cx).delegate().rows.len())
    }

    #[cfg(test)]
    pub fn cell(&self, row: usize, column: usize, cx: &App) -> Option<String> {
        self.table_state
            .read(cx)
            .delegate()
            .rows
            .get(row)?
            .get(column)?
            .clone()
    }

    /// The result's column names, empty before it ran.
    pub fn column_names(&self, cx: &App) -> Vec<String> {
        self.table_state
            .read(cx)
            .delegate()
            .columns
            .iter()
            .map(|column| column.name.to_string())
            .collect()
    }

    pub fn set_loading(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        cx.notify();
    }

    pub fn set_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.loading = false;
        self.error = Some(error);
        cx.notify();
    }

    /// Show `result`, read only: the grid is told of no table to edit.
    pub fn set_result(
        &mut self,
        mut result: QueryResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        result.table_name = None;
        result.table_columns = None;
        let context = self.context();
        self.table_state.update(cx, |state, cx| {
            if let Some(context) = &context {
                state.delegate_mut().set_connection_id(
                    context.connection_id,
                    &context.database_name,
                    context.db_type,
                );
            }
            state.clear_selection(cx);
            state.delegate_mut().set_query_result(result, window, cx);
            state.refresh(cx);
        });
        self.loading = false;
        self.error = None;
        self.has_result = true;
        self.show_view(window, cx);
    }

    /// Show the result as `view`, which is also what the panel saves.
    pub fn set_view(&mut self, view: PanelView, window: &mut Window, cx: &mut Context<Self>) {
        self.view = view;
        self.data.view = self.view.to_json();
        self.show_view(window, cx);
    }

    fn show_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.view {
            PanelView::Table { layout, thresholds } => {
                let layout = layout.clone();
                let thresholds = thresholds.clone();
                self.table_state.update(cx, |state, cx| {
                    let delegate = state.delegate_mut();
                    delegate.apply_column_layout(&layout);
                    delegate.thresholds = thresholds.and_then(|thresholds| {
                        let column = delegate.columns.iter().position(|column| {
                            Some(column.name.as_ref()) == thresholds.column.as_deref()
                        })?;
                        Some((column, thresholds))
                    });
                    state.refresh(cx);
                });
            }
            PanelView::Chart(spec) => {
                let spec = spec.clone();
                self.chart_view.update(cx, |chart, cx| {
                    chart.rebuild_column_selects(window, cx);
                    chart.apply_spec(&spec, window, cx);
                });
            }
            PanelView::Stat(_) => {}
        }
        cx.notify();
    }

    /// Copy the selection as `format` ("csv", "json", ...), or the cell or
    /// rows the copy shortcut takes when `format` is `None`.
    pub fn copy(&self, format: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.table_state.read(cx);
        let delegate = state.delegate();
        if format.is_none()
            && let TableSelection::Cell(row, col) = state.selection()
        {
            if let Some(Some(value)) = delegate.rows.get(row).and_then(|row| row.get(col)) {
                cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
            }
            return;
        }
        let mut rows: HashSet<usize> = state.selected_rows().clone();
        if rows.is_empty() {
            match state.selection() {
                TableSelection::Cell(row, _) | TableSelection::Row(row) => {
                    rows.insert(row);
                }
                _ => return,
            }
        }
        let data = selected_table_data(&rows, delegate);
        CopyHandler::new().copy_as_format(&data, format.unwrap_or("tsv"), window, cx);
    }

    /// One column's values, one per line or as an IN list.
    pub fn column_text(&self, column: usize, distinct: bool, as_in_list: bool, cx: &App) -> String {
        let delegate = self.table_state.read(cx).delegate();
        let values = delegate
            .rows
            .iter()
            .map(|row| row.get(column).and_then(|value| value.as_deref()));
        if as_in_list {
            let numeric = delegate
                .column_types
                .get(column)
                .is_some_and(|column_type| column_type.is_numeric());
            let dialect = delegate
                .db_type
                .map(|db_type| db_type.dialect())
                .unwrap_or_else(|| DatabaseType::SQLite.dialect());
            crate::results_panel::column_copy::in_list(values, numeric, dialect, distinct)
        } else {
            crate::results_panel::column_copy::values_list(values, distinct)
        }
    }

    fn render_stat(&self, spec: &StatSpec, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let delegate = self.table_state.read(cx).delegate();
        let column = match &spec.value {
            Some(name) => delegate
                .columns
                .iter()
                .position(|column| column.name.as_ref() == name.as_str()),
            None => delegate
                .column_types
                .iter()
                .position(|column_type| column_type.is_numeric()),
        };
        let Some(column) = column else {
            return placeholder("No numeric column to show.", cx);
        };
        let label: SharedString = delegate
            .columns
            .get(column)
            .map(|column| column.name.to_string())
            .unwrap_or_default()
            .into();
        let shown = stat_value(&delegate.rows, column, spec.aggregate);
        let Some((shown, number)) = shown else {
            return placeholder("No rows.", cx);
        };
        let color = spec
            .thresholds
            .as_ref()
            .zip(number)
            .map(|(thresholds, number)| thresholds.level(number).color(cx))
            .unwrap_or(theme.foreground);
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_1()
            .overflow_hidden()
            .child(
                div()
                    .text_size(px(40.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(color)
                    .whitespace_nowrap()
                    .child(shown),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(label),
            )
            .into_any_element()
    }
}

fn placeholder(message: &'static str, cx: &App) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(message)
        .into_any_element()
}

/// A stat's text: a number with its thousands grouped, anything else as is.
/// What a stat shows of `column`: its text, and its number for the
/// thresholds when it is one.
fn stat_value(
    rows: &[Vec<Option<String>>],
    column: usize,
    aggregate: Aggregate,
) -> Option<(String, Option<f64>)> {
    let values: Vec<&str> = rows
        .iter()
        .filter_map(|row| row.get(column).and_then(|value| value.as_deref()))
        .collect();
    match aggregate {
        // The first row's value, which may be NULL, not the first value
        // there is.
        Aggregate::None => {
            rows.first().map(
                |row| match row.get(column).and_then(|value| value.as_deref()) {
                    Some(value) => (
                        format_stat_text(value),
                        value
                            .trim()
                            .parse::<f64>()
                            .ok()
                            .filter(|number| number.is_finite()),
                    ),
                    None => ("NULL".to_string(), None),
                },
            )
        }
        Aggregate::Count => Some((
            format_number(values.len() as f64),
            Some(values.len() as f64),
        )),
        aggregate => {
            let numbers: Vec<f64> = values
                .iter()
                .filter_map(|value| value.trim().parse::<f64>().ok())
                // NaN and infinity would make the whole stat NaN, which
                // no threshold catches.
                .filter(|number| number.is_finite())
                .collect();
            (!numbers.is_empty()).then(|| {
                let sum: f64 = numbers.iter().sum();
                let value = if aggregate == Aggregate::Avg {
                    sum / numbers.len() as f64
                } else {
                    sum
                };
                (format_number(value), Some(value))
            })
        }
    }
}

fn format_stat_text(value: &str) -> String {
    match value.trim().parse::<f64>() {
        Ok(number) if number.is_finite() => format_number(number),
        _ => value.to_string(),
    }
}

/// `number` with its thousands grouped and at most two decimals.
fn format_number(number: f64) -> String {
    let text = if number.fract() == 0.0 && number.abs() < 1e15 {
        format!("{}", number as i64)
    } else {
        format!("{number:.2}")
    };
    let (sign, unsigned) = match text.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", text.as_str()),
    };
    let (whole, fraction) = match unsigned.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (unsigned, None),
    };
    let mut grouped = String::new();
    for (ix, digit) in whole.chars().enumerate() {
        if ix > 0 && (whole.len() - ix) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    match fraction {
        Some(fraction) => format!("{sign}{grouped}.{fraction}"),
        None => format!("{sign}{grouped}"),
    }
}

impl Render for DashboardPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if let Some(error) = &self.error {
            div()
                .size_full()
                .p_3()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(error.clone())
                .into_any_element()
        } else if !self.has_result {
            placeholder(
                if self.loading {
                    "Running…"
                } else if self.data.connection.is_none() {
                    "The panel's connection was deleted."
                } else {
                    "Not run yet."
                },
                cx,
            )
        } else {
            match &self.view {
                PanelView::Table { .. } => DataTable::new(&self.table_state)
                    .bordered(false)
                    .into_any_element(),
                PanelView::Chart(_) => self.chart_view.clone().into_any_element(),
                PanelView::Stat(spec) => self.render_stat(spec, cx),
            }
        };
        div()
            .size_full()
            .overflow_hidden()
            .when(self.loading && self.has_result, |this| this.opacity(0.6))
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stat_shows_the_first_row_and_ignores_values_that_are_not_numbers() {
        let rows: Vec<Vec<Option<String>>> = [None, Some("7"), Some("NaN"), Some("3")]
            .into_iter()
            .map(|value| vec![value.map(str::to_string)])
            .collect();
        assert_eq!(
            stat_value(&rows, 0, Aggregate::None),
            Some(("NULL".to_string(), None)),
            "the first row is NULL, whatever follows"
        );
        assert_eq!(
            stat_value(&rows, 0, Aggregate::Sum),
            Some(("10".to_string(), Some(10.0)))
        );
        assert_eq!(
            stat_value(&rows, 0, Aggregate::Avg),
            Some(("5".to_string(), Some(5.0)))
        );
        assert_eq!(stat_value(&[], 0, Aggregate::None), None);
    }

    #[test]
    fn views_round_trip_and_unknown_ones_show_as_tables() {
        let views = [
            PanelView::default(),
            PanelView::Stat(StatSpec {
                value: Some("pending".to_string()),
                aggregate: Aggregate::Sum,
                thresholds: Some(Thresholds {
                    column: None,
                    warning: 100.,
                    critical: 500.,
                }),
            }),
            PanelView::Chart(ChartSpec {
                axis: Some("genre".to_string()),
                value: Some("revenue".to_string()),
                ..ChartSpec::new(ChartKind::Bar)
            }),
        ];
        for view in views {
            assert_eq!(PanelView::from_json(&view.to_json()), view);
        }
        assert_eq!(
            PanelView::from_json(r#"{"type":"heatmap"}"#),
            PanelView::default()
        );
    }

    #[test]
    fn switching_kinds_keeps_the_columns_that_carry_over() {
        let bar = PanelView::Chart(ChartSpec {
            axis: Some("month".to_string()),
            value: Some("revenue".to_string()),
            aggregate: Aggregate::Sum,
            ..ChartSpec::new(ChartKind::Bar)
        });
        let PanelView::Chart(line) = bar.with_kind(PanelKind::Chart(ChartKind::Line)) else {
            panic!("a chart stays a chart");
        };
        assert_eq!(line.kind, ChartKind::Line);
        assert_eq!(line.axis.as_deref(), Some("month"));
        assert_eq!(
            bar.with_kind(PanelKind::Stat),
            PanelView::Stat(StatSpec {
                value: Some("revenue".to_string()),
                aggregate: Aggregate::Sum,
                thresholds: None,
            })
        );
        assert_eq!(bar.with_kind(PanelKind::Table), PanelView::default());
    }

    #[test]
    fn thresholds_read_their_direction_from_the_numbers() {
        let higher_is_worse = Thresholds {
            column: None,
            warning: 100.,
            critical: 500.,
        };
        assert_eq!(higher_is_worse.level(50.), ThresholdLevel::Ok);
        assert_eq!(higher_is_worse.level(100.), ThresholdLevel::Warning);
        assert_eq!(higher_is_worse.level(900.), ThresholdLevel::Critical);
        let lower_is_worse = Thresholds {
            column: None,
            warning: 10.,
            critical: 2.,
        };
        assert_eq!(lower_is_worse.level(50.), ThresholdLevel::Ok);
        assert_eq!(lower_is_worse.level(5.), ThresholdLevel::Warning);
        assert_eq!(lower_is_worse.level(0.), ThresholdLevel::Critical);
        assert_eq!(lower_is_worse.level_of("n/a"), None);
    }

    #[test]
    fn stats_group_thousands() {
        assert_eq!(format_number(1234567.0), "1,234,567");
        assert_eq!(format_number(-1234.5), "-1,234.50");
        assert_eq!(format_number(12.0), "12");
        assert_eq!(format_stat_text("2026-10-26"), "2026-10-26");
    }
}
