use std::collections::HashMap;

use gpui::{
    AnyElement, App, AppContext, Context, Entity, FocusHandle, Focusable, Hsla, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, Styled, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme, IndexPath, Sizable as _,
    chart::{AreaChart, BarChart, LineChart, PieChart},
    checkbox::Checkbox,
    h_flex,
    plot::{
        AXIS_GAP, AxisText, Grid, IntoPlot, Plot, PlotAxis,
        scale::{Scale, ScaleBand, ScaleLinear},
        shape::{Bar, Stack},
    },
    scroll::ScrollableElement as _,
    select::{Select, SelectEvent, SelectState},
    table::TableState,
    v_flex,
};

use blanco_core::connection_trait::ColumnType;

use super::ResultsTableDelegate;

const MAX_LABEL_CHARS: usize = 15;

fn truncate_label(s: &str) -> SharedString {
    truncate_label_to(s, MAX_LABEL_CHARS)
}

fn truncate_label_to(s: &str, max_chars: usize) -> SharedString {
    if s.chars().count() <= max_chars {
        SharedString::from(s.to_string())
    } else {
        let end = s
            .char_indices()
            .nth(max_chars)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        SharedString::from(format!("{}…", &s[..end]))
    }
}

fn apply_aggregate(values: &[f64], agg: Aggregate) -> f64 {
    match agg {
        Aggregate::Sum => values.iter().sum(),
        Aggregate::Avg => {
            if values.is_empty() {
                0.0
            } else {
                values.iter().sum::<f64>() / values.len() as f64
            }
        }
        Aggregate::Count => values.len() as f64,
        Aggregate::None => values.first().copied().unwrap_or(0.0),
    }
}

/// Assign a unique display label to each raw label, truncating long labels and
/// appending a " (N)" suffix on any collision. Collisions are detected on the
/// *truncated* form, so two distinct labels that share a 15-char prefix still
/// get distinct display labels and won't collapse onto the same point in a
/// `ScalePoint`/`ScaleBand` (which resolve a value to the first matching entry).
/// Returns `(display_label, full_label)` per input in the same order.
fn assign_display_labels(labels: &[String]) -> Vec<(SharedString, SharedString)> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    labels
        .iter()
        .map(|label| {
            let truncated = truncate_label(label);
            let count = seen.entry(truncated.to_string()).or_insert(0);
            *count += 1;
            if *count > 1 {
                let suffix = format!(" ({})", count);
                let budget = MAX_LABEL_CHARS.saturating_sub(suffix.chars().count());
                let display =
                    SharedString::from(format!("{}{}", truncate_label_to(label, budget), suffix));
                let full = SharedString::from(format!("{}{}", label, suffix));
                (display, full)
            } else {
                (truncated, SharedString::from(label.clone()))
            }
        })
        .collect()
}

/// Pick a tick margin so a band/point axis shows at most ~40 labels, avoiding
/// overlapping text when there are many data points.
fn tick_margin_for(count: usize) -> usize {
    const MAX_LABELS: usize = 40;
    (count / MAX_LABELS).max(1)
}

#[derive(Copy, Clone, Eq, PartialEq, Debug, Default)]
pub enum ResultViewMode {
    #[default]
    Table,
    Chart,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum ChartKind {
    Line,
    Bar,
    Pie,
}

impl ChartKind {
    fn label(self) -> &'static str {
        match self {
            ChartKind::Line => "Line",
            ChartKind::Bar => "Bar",
            ChartKind::Pie => "Pie",
        }
    }

    fn all() -> [ChartKind; 3] {
        [ChartKind::Line, ChartKind::Bar, ChartKind::Pie]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aggregate {
    None,
    Sum,
    Avg,
    Count,
}

impl Aggregate {
    fn label(self) -> &'static str {
        match self {
            Aggregate::None => "None",
            Aggregate::Sum => "Sum",
            Aggregate::Avg => "Avg",
            Aggregate::Count => "Count",
        }
    }

    fn all() -> [Aggregate; 4] {
        [
            Aggregate::None,
            Aggregate::Sum,
            Aggregate::Avg,
            Aggregate::Count,
        ]
    }
}

#[derive(Clone, Debug, Default)]
pub struct ChartConfig {
    pub line_stepped: bool,
    pub line_show_points: bool,
}

/// A single data point used by single-series charts (LineChart, BarChart, PieChart).
#[derive(Clone)]
struct ChartPoint {
    display_label: SharedString,
    full_label: SharedString,
    value: f64,
    color: Hsla,
}

/// Pivoted row used by multi-series line charts (AreaChart). Each row has an
/// x-axis label and one y-value per series.
#[derive(Clone)]
struct PivotedRow {
    x_label: SharedString,
    values: Vec<f64>,
}

struct SeriesInfo {
    names: Vec<SharedString>,
    colors: Vec<Hsla>,
}

struct GroupedData {
    /// One entry per unique x value, in the order they first appeared.
    rows: Vec<PivotedRow>,
    series: SeriesInfo,
}

/// A stacked bar chart built from pivoted data. Uses the same low-level
/// primitives as the gpui-component story (`Stack`, `Bar`, `ScaleBand`).
#[derive(IntoPlot)]
struct StackedBar {
    x_labels: Vec<SharedString>,
    series: Vec<gpui_component::plot::shape::StackSeries<PivotedRow>>,
    colors: Vec<Hsla>,
}

impl StackedBar {
    fn new(grouped: GroupedData) -> Self {
        let keys: Vec<String> = grouped.series.names.iter().map(|n| n.to_string()).collect();
        let num_series = keys.len();
        let rows = grouped.rows;
        let x_labels: Vec<SharedString> = rows.iter().map(|r| r.x_label.clone()).collect();

        let series = Stack::new()
            .data(rows)
            .keys(keys)
            .value(move |r: &PivotedRow, key| {
                let si = (0..num_series).find(|&i| {
                    grouped
                        .series
                        .names
                        .get(i)
                        .map_or(false, |n| n.as_ref() == key)
                });
                si.and_then(|i| {
                    r.values
                        .get(i)
                        .copied()
                        .filter(|&v| v != 0.0)
                        .map(|v| v as f32)
                })
            })
            .series();

        Self {
            x_labels,
            series,
            colors: grouped.series.colors,
        }
    }
}

impl Plot for StackedBar {
    fn paint(&mut self, bounds: gpui::Bounds<gpui::Pixels>, window: &mut Window, cx: &mut App) {
        let width = bounds.size.width.as_f32();
        let height = bounds.size.height.as_f32() - AXIS_GAP;

        let x = ScaleBand::new(self.x_labels.clone(), vec![0., width])
            .padding_inner(0.4)
            .padding_outer(0.2);
        let band_width = x.band_width();

        let max = self
            .series
            .iter()
            .flat_map(|s| s.points.iter().map(|p| p.y1))
            .fold(0., f32::max) as f64;

        let y = ScaleLinear::new(vec![0., max], vec![height, 10.]);

        let tick_margin = tick_margin_for(self.x_labels.len());
        let x_label = self
            .x_labels
            .iter()
            .enumerate()
            .filter(|(i, _)| i % tick_margin == 0)
            .filter_map(|(_, label)| {
                x.tick(label).map(|x_tick| {
                    AxisText::new(
                        label.clone(),
                        x_tick + band_width / 2.,
                        cx.theme().muted_foreground,
                    )
                    .align(gpui::TextAlign::Center)
                })
            });
        PlotAxis::new()
            .x(height)
            .x_label(x_label)
            .stroke(cx.theme().border)
            .paint(&bounds, window, cx);

        Grid::new()
            .y((0..=3).map(|i| height * i as f32 / 4.0).collect())
            .stroke(cx.theme().border)
            .dash_array(&[gpui::px(4.), gpui::px(2.)])
            .paint(&bounds, window);

        for (si, series) in self.series.iter().enumerate() {
            let x = x.clone();
            let y0 = y.clone();
            let y1 = y.clone();
            let fill = self.colors.get(si).copied().unwrap_or(cx.theme().chart_1);

            Bar::new()
                .data(&series.points)
                .band_width(band_width)
                .cross(move |d| x.tick(&d.data.x_label))
                .base(move |d| y0.tick(&(d.y0 as f64)).unwrap_or(height))
                .value(move |d| y1.tick(&(d.y1 as f64)))
                .fill(move |_, _, _| fill)
                .paint(&bounds, window, cx);
        }
    }
}

pub struct ChartView {
    focus_handle: FocusHandle,
    table_state: Entity<TableState<ResultsTableDelegate>>,
    kind: ChartKind,
    config: ChartConfig,
    kind_select: Entity<SelectState<Vec<SharedString>>>,
    axis_select: Entity<SelectState<Vec<SharedString>>>,
    value_select: Entity<SelectState<Vec<SharedString>>>,
    aggregate_select: Entity<SelectState<Vec<SharedString>>>,
    series_select: Entity<SelectState<Vec<SharedString>>>,
    axis_column_indices: Vec<usize>,
    value_column_indices: Vec<usize>,
    series_column_indices: Vec<usize>,
    _subscriptions: Vec<Subscription>,
}

impl ChartView {
    pub fn new(
        table_state: Entity<TableState<ResultsTableDelegate>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let kind_items: Vec<SharedString> =
            ChartKind::all().iter().map(|k| k.label().into()).collect();
        let kind_select =
            cx.new(|cx| SelectState::new(kind_items, Some(IndexPath::new(0)), window, cx));

        let axis_select =
            cx.new(|cx| SelectState::new(Vec::<SharedString>::new(), None, window, cx));
        let value_select =
            cx.new(|cx| SelectState::new(Vec::<SharedString>::new(), None, window, cx));

        let agg_items: Vec<SharedString> =
            Aggregate::all().iter().map(|a| a.label().into()).collect();
        let aggregate_select =
            cx.new(|cx| SelectState::new(agg_items, Some(IndexPath::new(0)), window, cx));

        let series_select =
            cx.new(|cx| SelectState::new(Vec::<SharedString>::new(), None, window, cx));

        let subs = vec![
            cx.subscribe_in(&kind_select, window, Self::on_kind_changed),
            cx.subscribe(&axis_select, |_, _, _ev: &SelectEvent<_>, cx| cx.notify()),
            cx.subscribe(&value_select, |_, _, _ev: &SelectEvent<_>, cx| cx.notify()),
            cx.subscribe(&aggregate_select, |_, _, _ev: &SelectEvent<_>, cx| {
                cx.notify()
            }),
            cx.subscribe(&series_select, |_, _, _ev: &SelectEvent<_>, cx| cx.notify()),
        ];

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            table_state,
            kind: ChartKind::Line,
            config: ChartConfig::default(),
            kind_select,
            axis_select,
            value_select,
            aggregate_select,
            series_select,
            axis_column_indices: Vec::new(),
            value_column_indices: Vec::new(),
            series_column_indices: Vec::new(),
            _subscriptions: subs,
        };
        this.rebuild_column_selects(window, cx);
        this
    }

    pub fn rebuild_column_selects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (axis_items, axis_indices, value_items, value_indices, all_items, all_indices) =
            self.table_state.read_with(cx, |state, _| {
                let delegate = state.delegate();
                let pairs: Vec<(usize, SharedString)> = delegate
                    .columns
                    .iter()
                    .enumerate()
                    .skip(1)
                    .map(|(i, c)| (i - 1, SharedString::from(c.name.to_string())))
                    .collect();

                let numeric_pairs: Vec<(usize, SharedString)> = pairs
                    .iter()
                    .filter(|(idx, _)| {
                        delegate
                            .column_types
                            .get(*idx)
                            .map(ColumnType::is_numeric)
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect();

                let axis_items: Vec<SharedString> = pairs.iter().map(|(_, n)| n.clone()).collect();
                let axis_indices: Vec<usize> = pairs.iter().map(|(i, _)| *i).collect();
                let value_items: Vec<SharedString> =
                    numeric_pairs.iter().map(|(_, n)| n.clone()).collect();
                let value_indices: Vec<usize> = numeric_pairs.iter().map(|(i, _)| *i).collect();
                // Series: all columns (like axis), prepended with "None".
                let mut all_items = vec![SharedString::from("None")];
                all_items.extend(pairs.iter().map(|(_, n)| n.clone()));
                let mut all_indices = vec![usize::MAX];
                all_indices.extend(pairs.iter().map(|(i, _)| *i));

                (
                    axis_items,
                    axis_indices,
                    value_items,
                    value_indices,
                    all_items,
                    all_indices,
                )
            });

        self.axis_column_indices = axis_indices;
        self.value_column_indices = value_indices;
        self.series_column_indices = all_indices;

        let axis_default = (!self.axis_column_indices.is_empty()).then(|| IndexPath::new(0));
        let value_default = (!self.value_column_indices.is_empty()).then(|| IndexPath::new(0));

        self.axis_select.update(cx, |state, cx| {
            state.set_items(axis_items, window, cx);
            state.set_selected_index(axis_default, window, cx);
        });
        self.value_select.update(cx, |state, cx| {
            state.set_items(value_items, window, cx);
            state.set_selected_index(value_default, window, cx);
        });
        self.series_select.update(cx, |state, cx| {
            state.set_items(all_items, window, cx);
            state.set_selected_index(Some(IndexPath::new(0)), window, cx);
        });
        cx.notify();
    }

    fn on_kind_changed(
        &mut self,
        _: &Entity<SelectState<Vec<SharedString>>>,
        event: &SelectEvent<Vec<SharedString>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SelectEvent::Confirm(_) = event;
        let kinds = ChartKind::all();
        let idx = self
            .kind_select
            .read(cx)
            .selected_index(cx)
            .map(|ip| ip.row)
            .unwrap_or(0);
        self.kind = kinds.get(idx).copied().unwrap_or(ChartKind::Line);
        // The available columns don't depend on chart kind, so we only re-render
        // (which re-labels the axis fields) instead of rebuilding the selects,
        // which would reset the user's column choices.
        cx.notify();
    }

    fn selected_axis_column(&self, cx: &App) -> Option<usize> {
        let row = self.axis_select.read(cx).selected_index(cx)?.row;
        self.axis_column_indices.get(row).copied()
    }

    fn selected_value_column(&self, cx: &App) -> Option<usize> {
        let row = self.value_select.read(cx).selected_index(cx)?.row;
        self.value_column_indices.get(row).copied()
    }

    fn selected_aggregate(&self, cx: &App) -> Aggregate {
        let row = self
            .aggregate_select
            .read(cx)
            .selected_index(cx)
            .map(|ip| ip.row)
            .unwrap_or(0);
        Aggregate::all()
            .get(row)
            .copied()
            .unwrap_or(Aggregate::None)
    }

    fn selected_series_column(&self, cx: &App) -> Option<usize> {
        let row = self.series_select.read(cx).selected_index(cx)?.row;
        let col = self.series_column_indices.get(row).copied()?;
        if col == usize::MAX { None } else { Some(col) }
    }

    fn extract_raw(&self, cx: &App) -> Option<(usize, usize, Vec<(String, f64, Option<String>)>)> {
        let axis_col = self.selected_axis_column(cx)?;
        let value_col = self.selected_value_column(cx)?;
        let series_col = self.selected_series_column(cx);
        let axis_row_idx = axis_col + 1;
        let value_row_idx = value_col + 1;
        let series_row_idx = series_col.map(|c| c + 1);

        let (mut rows, axis_numeric) = self.table_state.read_with(cx, |state, _| {
            let delegate = state.delegate();
            let rows = delegate
                .rows
                .iter()
                .filter_map(|row| {
                    let axis = row.get(axis_row_idx)?.as_ref()?;
                    let value = row.get(value_row_idx)?.as_ref()?;
                    let value: f64 = value.parse().ok()?;
                    let series = series_row_idx.and_then(|si| row.get(si).and_then(|v| v.clone()));
                    Some((axis.clone(), value, series))
                })
                .collect::<Vec<_>>();
            let axis_numeric = delegate
                .column_types
                .get(axis_col)
                .map(ColumnType::is_numeric)
                .unwrap_or(false);
            (rows, axis_numeric)
        });

        // Line charts connect points in data order via a ScalePoint, so an
        // unsorted x-axis produces a zig-zagging line. Sort by the x value
        // (numerically when the column is numeric, lexically otherwise). Bar
        // and pie charts keep first-appearance order.
        if self.kind == ChartKind::Line {
            if axis_numeric {
                rows.sort_by(|a, b| {
                    let pa = a.0.parse::<f64>().unwrap_or(f64::INFINITY);
                    let pb = b.0.parse::<f64>().unwrap_or(f64::INFINITY);
                    pa.partial_cmp(&pb).unwrap_or(std::cmp::Ordering::Equal)
                });
            } else {
                rows.sort_by(|a, b| a.0.cmp(&b.0));
            }
        }

        Some((axis_row_idx, value_row_idx, rows))
    }

    fn extract_points(&self, cx: &App) -> Option<Vec<ChartPoint>> {
        let (_, _, raw) = self.extract_raw(cx)?;
        let aggregate = self.selected_aggregate(cx);

        // Build the (label, value) list. When aggregating, identical labels are
        // collapsed into one group (preserving first-appearance order); without
        // aggregation each row stays a distinct point.
        let pairs: Vec<(String, f64)> = if aggregate != Aggregate::None {
            let mut groups: Vec<(String, Vec<f64>)> = Vec::new();
            let mut index: HashMap<String, usize> = HashMap::new();
            for (label, value, _) in raw {
                let idx = if let Some(&i) = index.get(&label) {
                    i
                } else {
                    let i = groups.len();
                    groups.push((label.clone(), Vec::new()));
                    index.insert(label, i);
                    i
                };
                groups[idx].1.push(value);
            }
            groups
                .into_iter()
                .map(|(label, values)| (label, apply_aggregate(&values, aggregate)))
                .collect()
        } else {
            raw.into_iter()
                .map(|(label, value, _)| (label, value))
                .collect()
        };

        let palette = pie_palette(cx, pairs.len());
        let labels: Vec<String> = pairs.iter().map(|(label, _)| label.clone()).collect();
        let display = assign_display_labels(&labels);
        let points: Vec<ChartPoint> = pairs
            .into_iter()
            .zip(display)
            .enumerate()
            .map(
                |(i, ((_, value), (display_label, full_label)))| ChartPoint {
                    display_label,
                    full_label,
                    value,
                    color: palette[i % palette.len()],
                },
            )
            .collect();

        Some(points)
    }

    fn extract_grouped(&self, cx: &App) -> Option<GroupedData> {
        let (_, _, raw) = self.extract_raw(cx)?;
        let aggregate = self.selected_aggregate(cx);

        let mut series_set: Vec<String> = Vec::new();
        let mut series_index: HashMap<String, usize> = HashMap::new();
        let mut x_order: Vec<String> = Vec::new();
        let mut x_index: HashMap<String, usize> = HashMap::new();
        // (x_idx, series_idx) → Vec<f64>
        let mut cells: HashMap<(usize, usize), Vec<f64>> = HashMap::new();

        for (x_label, value, series_label) in &raw {
            let series_label = series_label.as_deref().unwrap_or("");
            let si = if let Some(&i) = series_index.get(series_label) {
                i
            } else {
                let i = series_set.len();
                series_set.push(series_label.to_string());
                series_index.insert(series_label.to_string(), i);
                i
            };
            let xi = if let Some(&i) = x_index.get(x_label) {
                i
            } else {
                let i = x_order.len();
                x_order.push(x_label.clone());
                x_index.insert(x_label.clone(), i);
                i
            };
            cells.entry((xi, si)).or_default().push(*value);
        }

        let num_series = series_set.len();
        let palette = pie_palette(cx, num_series);
        let x_display = assign_display_labels(&x_order);
        let rows: Vec<PivotedRow> = x_order
            .iter()
            .enumerate()
            .map(|(xi, _)| {
                let values: Vec<f64> = (0..num_series)
                    .map(|si| {
                        let vals = cells.get(&(xi, si));
                        match vals {
                            Some(vs) if !vs.is_empty() => {
                                if aggregate == Aggregate::None {
                                    // No aggregate selected, but a pivot cell may
                                    // still hold several rows; sum them rather than
                                    // silently dropping all but the first.
                                    vs.iter().sum()
                                } else {
                                    apply_aggregate(vs, aggregate)
                                }
                            }
                            _ => 0.0,
                        }
                    })
                    .collect();
                PivotedRow {
                    x_label: x_display[xi].0.clone(),
                    values,
                }
            })
            .collect();

        Some(GroupedData {
            rows,
            series: SeriesInfo {
                names: series_set
                    .into_iter()
                    .map(|s| SharedString::from(s))
                    .collect(),
                colors: palette,
            },
        })
    }

    fn render_axis_field(
        &self,
        label: &str,
        select: &Entity<SelectState<Vec<SharedString>>>,
        empty_text: &str,
    ) -> gpui::Div {
        v_flex()
            .gap_1()
            .child(div().text_sm().child(label.to_string()))
            .child(
                Select::new(select)
                    .small()
                    .placeholder(SharedString::from(empty_text.to_string())),
            )
    }

    fn render_settings(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let kind = self.kind;
        v_flex()
            .w(px(240.))
            .h_full()
            .p_3()
            .gap_3()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().child("Chart"))
                    .child(Select::new(&self.kind_select).small()),
            )
            .child(match kind {
                ChartKind::Line => {
                    self.render_axis_field("X-axis", &self.axis_select, "Select a column")
                }
                ChartKind::Bar => {
                    self.render_axis_field("Categories", &self.axis_select, "Select a column")
                }
                ChartKind::Pie => {
                    self.render_axis_field("Keys", &self.axis_select, "Select a column")
                }
            })
            .child(match kind {
                ChartKind::Line => {
                    self.render_axis_field("Y-axis", &self.value_select, "Select a numeric column")
                }
                ChartKind::Bar => {
                    self.render_axis_field("Values", &self.value_select, "Select a numeric column")
                }
                ChartKind::Pie => {
                    self.render_axis_field("Values", &self.value_select, "Select a numeric column")
                }
            })
            .child(self.render_axis_field("Aggregate", &self.aggregate_select, ""))
            .child(self.render_axis_field("Series", &self.series_select, "None"))
            .when(matches!(kind, ChartKind::Line), |this| {
                this.child(
                    Checkbox::new("chart-line-stepped")
                        .label("Stepped")
                        .small()
                        .checked(self.config.line_stepped)
                        .on_click(cx.listener(|view, checked: &bool, _window, cx| {
                            view.config.line_stepped = *checked;
                            cx.notify();
                        })),
                )
                .child(
                    Checkbox::new("chart-line-points")
                        .label("Show points")
                        .small()
                        .checked(self.config.line_show_points)
                        .on_click(cx.listener(|view, checked: &bool, _window, cx| {
                            view.config.line_show_points = *checked;
                            cx.notify();
                        })),
                )
            })
    }

    fn render_series_legend(series: &SeriesInfo, border_color: Hsla) -> gpui::Div {
        h_flex()
            .p_2()
            .px_4()
            .gap_3()
            .flex_wrap()
            .border_t_1()
            .border_color(border_color)
            .children(
                series
                    .names
                    .iter()
                    .zip(series.colors.iter())
                    .map(|(name, color)| {
                        h_flex()
                            .gap_1()
                            .items_center()
                            .text_sm()
                            .child(div().w(px(10.)).h(px(10.)).rounded_sm().bg(*color))
                            .child(name.clone())
                    }),
            )
    }

    fn render_chart_canvas(&self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted_fg = theme.muted_foreground;
        let border_color = theme.border;
        let make_placeholder = |message: &'static str| -> AnyElement {
            div()
                .flex_1()
                .h_full()
                .p_4()
                .flex()
                .items_center()
                .justify_center()
                .text_color(muted_fg)
                .child(message)
                .into_any_element()
        };

        let has_series = self.selected_series_column(cx).is_some();

        if has_series {
            return match self.render_multi_series(cx, border_color) {
                Some(el) => el,
                None => make_placeholder("Select an axis and a numeric value column."),
            };
        }

        let Some(points) = self.extract_points(cx) else {
            return make_placeholder("Select an axis and a numeric value column.");
        };

        if points.is_empty() {
            return make_placeholder("No plottable rows (all values are NULL or non-numeric).");
        }

        match self.kind {
            ChartKind::Line | ChartKind::Bar => {
                const MIN_WIDTH_PER_POINT: f32 = 80.0;
                let min_width = px(points.len() as f32 * MIN_WIDTH_PER_POINT);
                let tick_margin = tick_margin_for(points.len());
                let chart_inner = div().h_full().w_full().min_w(min_width);
                let chart_inner = match self.kind {
                    ChartKind::Line => {
                        let mut line = LineChart::new(points)
                            .x(|p: &ChartPoint| p.display_label.clone())
                            .y(|p: &ChartPoint| p.value)
                            .tick_margin(tick_margin);
                        if self.config.line_stepped {
                            line = line.step_after();
                        }
                        if self.config.line_show_points {
                            line = line.dot();
                        }
                        chart_inner.child(line)
                    }
                    ChartKind::Bar => {
                        let bar = BarChart::new(points)
                            .band(|p: &ChartPoint| p.display_label.clone())
                            .value(|p: &ChartPoint| p.value)
                            .tick_margin(tick_margin)
                            .fill(|p: &ChartPoint, _, _, _| p.color);
                        chart_inner.child(bar)
                    }
                    ChartKind::Pie => unreachable!(),
                };
                div()
                    .id("chart-canvas")
                    .flex_1()
                    .h_full()
                    .p_4()
                    .overflow_x_scrollbar()
                    .child(chart_inner)
                    .into_any_element()
            }
            ChartKind::Pie => {
                // Pie slices can only represent positive magnitudes; zero and
                // negative values produce degenerate or nonsensical wedges.
                let points: Vec<ChartPoint> =
                    points.into_iter().filter(|p| p.value > 0.0).collect();
                if points.is_empty() {
                    return make_placeholder("Pie charts require positive values.");
                }
                let pie = PieChart::new(points.clone())
                    .outer_radius(140.0)
                    .value(|p: &ChartPoint| p.value as f32)
                    .color(|p: &ChartPoint| p.color);
                let legend = v_flex()
                    .gap_1()
                    .min_w(px(160.))
                    .children(points.iter().map(|p| {
                        h_flex()
                            .gap_2()
                            .items_center()
                            .text_sm()
                            .child(div().w(px(10.)).h(px(10.)).rounded_sm().bg(p.color))
                            .child(div().flex_1().truncate().child(p.full_label.clone()))
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(format_legend_value(p.value)),
                            )
                    }));
                div()
                    .id("chart-canvas")
                    .flex_1()
                    .h_full()
                    .p_4()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_6()
                    .overflow_hidden()
                    .child(div().w(px(320.)).h(px(320.)).child(pie))
                    .child(legend)
                    .into_any_element()
            }
        }
    }

    fn render_multi_series(
        &self,
        cx: &mut Context<Self>,
        border_color: Hsla,
    ) -> Option<AnyElement> {
        let grouped = self.extract_grouped(cx)?;
        if grouped.rows.is_empty() || grouped.series.names.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let num_series = grouped.series.names.len();

        match self.kind {
            ChartKind::Line => {
                const MIN_WIDTH_PER_POINT: f32 = 80.0;
                let min_width = px(grouped.rows.len() as f32 * MIN_WIDTH_PER_POINT);

                let tick_margin = tick_margin_for(grouped.rows.len());
                // AreaChart defaults every series to the same translucent fill,
                // so without an explicit per-series fill the series are
                // indistinguishable. Give each series its own color, lightly
                // filled so overlapping areas stay readable.
                let mut chart = AreaChart::new(grouped.rows.clone())
                    .tick_margin(tick_margin)
                    .x(|r: &PivotedRow| r.x_label.clone())
                    .y(|r: &PivotedRow| r.values[0])
                    .stroke(grouped.series.colors[0])
                    .fill(grouped.series.colors[0].opacity(0.15));

                for i in 1..num_series {
                    let color = grouped.series.colors[i];
                    chart = chart
                        .y(move |r: &PivotedRow| r.values[i])
                        .stroke(color)
                        .fill(color.opacity(0.15));
                }

                let legend = Self::render_series_legend(&grouped.series, border_color);
                Some(
                    v_flex()
                        .flex_1()
                        .h_full()
                        .child(
                            div()
                                .id("chart-canvas")
                                .flex_1()
                                .h_full()
                                .p_4()
                                .overflow_x_scrollbar()
                                .child(div().h_full().w_full().min_w(min_width).child(chart)),
                        )
                        .child(legend)
                        .into_any_element(),
                )
            }
            ChartKind::Bar => {
                let legend_info = SeriesInfo {
                    names: grouped.series.names.clone(),
                    colors: grouped.series.colors.clone(),
                };
                let chart = StackedBar::new(grouped);
                let legend = Self::render_series_legend(&legend_info, border_color);
                Some(
                    v_flex()
                        .flex_1()
                        .h_full()
                        .child(
                            div()
                                .id("chart-canvas")
                                .flex_1()
                                .h_full()
                                .p_4()
                                .child(div().h_full().w_full().child(chart)),
                        )
                        .child(legend)
                        .into_any_element(),
                )
            }
            ChartKind::Pie => {
                let series_names = grouped.series.names.clone();
                let series_colors = grouped.series.colors.clone();
                let points: Vec<ChartPoint> = grouped
                    .rows
                    .into_iter()
                    .flat_map(|r| {
                        let x = r.x_label;
                        let sn = &series_names;
                        let sc = &series_colors;
                        r.values.into_iter().enumerate().filter_map(move |(si, v)| {
                            if v <= 0.0 {
                                None
                            } else {
                                Some(ChartPoint {
                                    display_label: x.clone(),
                                    full_label: SharedString::from(format!(
                                        "{} – {}",
                                        x,
                                        sn.get(si).unwrap_or(&x),
                                    )),
                                    value: v,
                                    color: sc[si % sc.len()],
                                })
                            }
                        })
                    })
                    .collect();

                let pie = PieChart::new(points.clone())
                    .outer_radius(140.0)
                    .value(|p: &ChartPoint| p.value as f32)
                    .color(|p: &ChartPoint| p.color);
                let legend = v_flex()
                    .gap_1()
                    .min_w(px(160.))
                    .children(points.iter().map(|p| {
                        h_flex()
                            .gap_2()
                            .items_center()
                            .text_sm()
                            .child(div().w(px(10.)).h(px(10.)).rounded_sm().bg(p.color))
                            .child(div().flex_1().truncate().child(p.full_label.clone()))
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(format_legend_value(p.value)),
                            )
                    }));
                Some(
                    div()
                        .id("chart-canvas")
                        .flex_1()
                        .h_full()
                        .p_4()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap_6()
                        .overflow_hidden()
                        .child(div().w(px(320.)).h(px(320.)).child(pie))
                        .child(legend)
                        .into_any_element(),
                )
            }
        }
    }
}

fn pie_palette(cx: &App, n: usize) -> Vec<Hsla> {
    let t = cx.theme();
    let base = [t.chart_1, t.chart_2, t.chart_3, t.chart_4, t.chart_5];
    if n <= base.len() {
        return base.to_vec();
    }

    let avg_s = base.iter().map(|c| c.s).sum::<f32>() / base.len() as f32;
    let avg_l = base.iter().map(|c| c.l).sum::<f32>() / base.len() as f32;
    let alpha = base[0].a;

    const GOLDEN_STEP: f32 = 0.618_034;
    let mut hue = base[base.len() - 1].h;

    let mut palette: Vec<Hsla> = base.to_vec();
    for _ in base.len()..n {
        hue = (hue + GOLDEN_STEP).fract();
        palette.push(Hsla {
            h: hue,
            s: avg_s,
            l: avg_l,
            a: alpha,
        });
    }
    palette
}

fn format_legend_value(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v:.2}")
    }
}

impl Focusable for ChartView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ChartView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .size_full()
            .child(self.render_settings(window, cx))
            .child(self.render_chart_canvas(window, cx))
    }
}
