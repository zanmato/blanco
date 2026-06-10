use std::collections::HashMap;

use gpui::{
    AnyElement, App, AppContext, Context, Entity, FocusHandle, Focusable, Hsla, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, Styled, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme, IndexPath, Sizable as _,
    chart::{BarChart, LineChart, PieChart},
    checkbox::Checkbox,
    h_flex,
    scroll::ScrollableElement as _,
    select::{Select, SelectEvent, SelectState},
    table::TableState,
    v_flex,
};

use blanco_core::connection_trait::ColumnType;

use super::ResultsTableDelegate;

/// Maximum characters shown on the x-axis before truncating with "…".
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

#[derive(Clone, Debug, Default)]
pub struct ChartConfig {
    pub line_stepped: bool,
    pub line_show_points: bool,
}

#[derive(Clone)]
struct ChartPoint {
    display_label: SharedString,
    full_label: SharedString,
    value: f64,
    color: Hsla,
}

pub struct ChartView {
    focus_handle: FocusHandle,
    table_state: Entity<TableState<ResultsTableDelegate>>,
    kind: ChartKind,
    config: ChartConfig,
    kind_select: Entity<SelectState<Vec<SharedString>>>,
    /// Left-hand axis selector: X-axis (line), Categories (bar), Keys (pie).
    axis_select: Entity<SelectState<Vec<SharedString>>>,
    /// Right-hand axis selector: Y-axis (line), Values (bar), Values (pie). Always numeric.
    value_select: Entity<SelectState<Vec<SharedString>>>,
    /// Maps a row index in `axis_select` items back to a column index in the delegate.
    axis_column_indices: Vec<usize>,
    /// Maps a row index in `value_select` items back to a column index in the delegate.
    value_column_indices: Vec<usize>,
    _subscriptions: Vec<Subscription>,
}

impl ChartView {
    pub fn new(
        table_state: Entity<TableState<ResultsTableDelegate>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let kind = ChartKind::Line;
        let kind_items: Vec<SharedString> =
            ChartKind::all().iter().map(|k| k.label().into()).collect();
        let kind_select =
            cx.new(|cx| SelectState::new(kind_items, Some(IndexPath::new(0)), window, cx));

        let axis_select =
            cx.new(|cx| SelectState::new(Vec::<SharedString>::new(), None, window, cx));
        let value_select =
            cx.new(|cx| SelectState::new(Vec::<SharedString>::new(), None, window, cx));

        let subs = vec![
            cx.subscribe_in(&kind_select, window, Self::on_kind_changed),
            cx.subscribe(&axis_select, |_, _, _ev: &SelectEvent<_>, cx| cx.notify()),
            cx.subscribe(&value_select, |_, _, _ev: &SelectEvent<_>, cx| cx.notify()),
        ];

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            table_state,
            kind,
            config: ChartConfig::default(),
            kind_select,
            axis_select,
            value_select,
            axis_column_indices: Vec::new(),
            value_column_indices: Vec::new(),
            _subscriptions: subs,
        };
        this.rebuild_column_selects(window, cx);
        this
    }

    /// Rebuild axis selectors when the source result columns change (new query)
    /// or when the chart kind changes.
    pub fn rebuild_column_selects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (axis_items, axis_indices, value_items, value_indices) =
            self.table_state.read_with(cx, |state, _| {
                let delegate = state.delegate();
                // The delegate's `columns` includes a row-number column at index 0;
                // skip it. `column_types` does not include the row-number prefix.
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

                // Axis (X/Category/Key): all columns allowed.
                let axis_items: Vec<SharedString> = pairs.iter().map(|(_, n)| n.clone()).collect();
                let axis_indices: Vec<usize> = pairs.iter().map(|(i, _)| *i).collect();

                // Value (Y/Value): numeric only.
                let value_items: Vec<SharedString> =
                    numeric_pairs.iter().map(|(_, n)| n.clone()).collect();
                let value_indices: Vec<usize> = numeric_pairs.iter().map(|(i, _)| *i).collect();

                (axis_items, axis_indices, value_items, value_indices)
            });

        self.axis_column_indices = axis_indices;
        self.value_column_indices = value_indices;

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
        cx.notify();
    }

    fn on_kind_changed(
        &mut self,
        _: &Entity<SelectState<Vec<SharedString>>>,
        event: &SelectEvent<Vec<SharedString>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SelectEvent::Confirm(_) = event;
        {
            let kinds = ChartKind::all();
            let idx = self
                .kind_select
                .read(cx)
                .selected_index(cx)
                .map(|ip| ip.row)
                .unwrap_or(0);
            self.kind = kinds.get(idx).copied().unwrap_or(ChartKind::Line);
            // Axis filter doesn't depend on kind in v1 (numeric on value, anything on axis),
            // but rebuild so a fresh result-set picks up new columns.
            self.rebuild_column_selects(window, cx);
        }
    }

    fn selected_axis_column(&self, cx: &App) -> Option<usize> {
        let row = self.axis_select.read(cx).selected_index(cx)?.row;
        self.axis_column_indices.get(row).copied()
    }

    fn selected_value_column(&self, cx: &App) -> Option<usize> {
        let row = self.value_select.read(cx).selected_index(cx)?.row;
        self.value_column_indices.get(row).copied()
    }

    fn extract_points(&self, cx: &App) -> Option<Vec<ChartPoint>> {
        let axis_col = self.selected_axis_column(cx)?;
        let value_col = self.selected_value_column(cx)?;
        let axis_row_idx = axis_col + 1;
        let value_row_idx = value_col + 1;
        let raw: Vec<(String, f64)> = self.table_state.read_with(cx, |state, _| {
            let delegate = state.delegate();
            delegate
                .rows
                .iter()
                .filter_map(|row| {
                    let axis = row.get(axis_row_idx)?.as_ref()?;
                    let value = row.get(value_row_idx)?.as_ref()?;
                    let value: f64 = value.parse().ok()?;
                    Some((axis.clone(), value))
                })
                .collect()
        });

        let mut seen: HashMap<String, usize> = HashMap::new();
        let palette = pie_palette(cx, raw.len());
        let points: Vec<ChartPoint> = raw
            .into_iter()
            .enumerate()
            .map(|(i, (label, value))| {
                let count = seen.entry(label.clone()).or_insert(0);
                *count += 1;
                let suffix = if *count > 1 {
                    Some(format!(" ({})", count))
                } else {
                    None
                };
                let full_label = match &suffix {
                    Some(s) => SharedString::from(format!("{}{}", label, s)),
                    None => SharedString::from(label.clone()),
                };
                let display_label = match &suffix {
                    Some(s) => {
                        let budget = MAX_LABEL_CHARS.saturating_sub(s.chars().count());
                        let truncated = truncate_label_to(&label, budget);
                        SharedString::from(format!("{}{}", truncated, s))
                    }
                    None => truncate_label(&label),
                };
                ChartPoint {
                    display_label,
                    full_label,
                    value,
                    color: palette[i % palette.len()],
                }
            })
            .collect();

        Some(points)
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

    fn render_chart_canvas(&self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let make_placeholder = |message: &'static str| -> AnyElement {
            div()
                .flex_1()
                .h_full()
                .p_4()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.muted_foreground)
                .child(message)
                .into_any_element()
        };

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
                let chart_inner = div().h_full().w_full().min_w(min_width);
                let chart_inner = match self.kind {
                    ChartKind::Line => {
                        let mut line = LineChart::new(points)
                            .x(|p: &ChartPoint| p.display_label.clone())
                            .y(|p: &ChartPoint| p.value);
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
                            .value(|p: &ChartPoint| p.value);
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
}

/// Build a palette with at least `n` visually distinct colors. The first five
/// entries come from the theme's `chart_1..chart_5` so the chart still feels
/// like part of the app; the rest are derived by walking the hue wheel with
/// the golden-ratio step and reusing the theme palette's average saturation
/// and lightness so light/dark themes stay consistent.
fn pie_palette(cx: &App, n: usize) -> Vec<Hsla> {
    let t = cx.theme();
    let base = [t.chart_1, t.chart_2, t.chart_3, t.chart_4, t.chart_5];
    if n <= base.len() {
        return base.to_vec();
    }

    let avg_s = base.iter().map(|c| c.s).sum::<f32>() / base.len() as f32;
    let avg_l = base.iter().map(|c| c.l).sum::<f32>() / base.len() as f32;
    let alpha = base[0].a;

    // Golden ratio conjugate; spreads hues evenly without obvious banding.
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
