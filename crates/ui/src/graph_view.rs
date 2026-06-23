use std::collections::{HashMap, HashSet};

use gpui::{
    App, Bounds, Context, FocusHandle, Focusable, Hsla, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, PathBuilder, Pixels,
    Point, Render, ScrollDelta, ScrollWheelEvent, SharedString, Size, Styled, Window, canvas, div,
    point, prelude::FluentBuilder, px,
};
use gpui_component::{ActiveTheme, v_flex};

const NODE_HEADER_HEIGHT: f32 = 32.0;
const NODE_FIELD_ROW_HEIGHT: f32 = 22.0;
const NODE_PADDING: f32 = 8.0;
const NODE_MIN_WIDTH: f32 = 180.0;
const NODE_BORDER_RADIUS: f32 = 6.0;
const PAN_THRESHOLD: f32 = 3.0;
const MIN_ZOOM: f32 = 0.15;
const MAX_ZOOM: f32 = 3.0;

const LAYOUT_START: f32 = 50.0;
const LAYOUT_NODE_GAP_Y: f32 = 40.0;
/// Horizontal gap between adjacent layer bands (FK depth levels).
const LAYOUT_LAYER_GAP_X: f32 = 130.0;
/// Horizontal gap between sub-columns within a single layer that has been
/// wrapped because it was too tall.
const LAYOUT_SUBCOL_GAP_X: f32 = 60.0;
/// A layer (or the isolated-tables block) is wrapped into additional columns
/// once stacking its nodes vertically would exceed this height.
const LAYOUT_MAX_COLUMN_HEIGHT: f32 = 1600.0;
/// Number of alternating barycenter sweeps used to reduce edge crossings.
const LAYOUT_BARYCENTER_SWEEPS: usize = 4;

#[derive(Clone, Debug)]
pub struct GraphNodeField {
    pub label: SharedString,
    pub type_label: SharedString,
    pub badges: Vec<FieldBadge>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldBadge {
    PrimaryKey,
    ForeignKey,
}

impl FieldBadge {
    fn label(&self) -> &'static str {
        match self {
            Self::PrimaryKey => "PK",
            Self::ForeignKey => "FK",
        }
    }

    fn color(&self, cx: &App) -> Hsla {
        match self {
            Self::PrimaryKey => cx.theme().blue,
            Self::ForeignKey => cx.theme().green,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GraphNode {
    pub id: String,
    pub title: SharedString,
    pub fields: Vec<GraphNodeField>,
    pub position: Point<f32>,
    pub size: Size<f32>,
}

#[derive(Clone, Debug)]
pub struct GraphEdge {
    pub from_node: String,
    pub from_field_index: usize,
    pub to_node: String,
    pub to_field_index: usize,
}

#[derive(Clone, Debug, Default)]
pub struct GraphModel {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Clone, Debug, Default)]
pub struct GraphViewport {
    pub pan_offset: Point<f32>,
    pub zoom_level: f32,
}

struct NodeRenderData {
    #[allow(dead_code)]
    ix: usize,
    screen_x: f32,
    screen_y: f32,
    width: f32,
    height: f32,
    is_dragging: bool,
    is_selected: bool,
    is_connected: bool,
    title: SharedString,
    fields: Vec<(SharedString, SharedString, Vec<(SharedString, Hsla)>)>,
}

struct EdgeRenderData {
    from: Point<Pixels>,
    to: Point<Pixels>,
    from_side: AnchorSide,
    to_side: AnchorSide,
    is_highlighted: bool,
}

enum AnchorSide {
    Right,
    Left,
    Bottom,
    Top,
}

pub struct GraphView {
    pub model: GraphModel,
    pub viewport: GraphViewport,
    focus_handle: FocusHandle,
    dragging_node: Option<(usize, Point<f32>)>,
    /// True once a press-on-node has moved past the drag threshold, so a plain
    /// click (press + release without movement) can be distinguished from a
    /// drag and used for selection instead.
    node_dragged: bool,
    selected_nodes: HashSet<usize>,
    is_panning: bool,
    pan_start: Point<f32>,
    pan_start_offset: Point<f32>,
    mouse_down_pos: Option<Point<f32>>,
    element_bounds: Option<Bounds<Pixels>>,
    /// When false (default), foreign-key edges are only drawn for the
    /// currently selected node. This keeps large schemas legible instead of
    /// rendering every relationship at once. Toggled via the on-screen control.
    show_all_edges: bool,
    pub loading: bool,
    pub error: Option<String>,
}

impl GraphView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            model: GraphModel::default(),
            viewport: GraphViewport {
                pan_offset: Point::default(),
                zoom_level: 1.0,
            },
            focus_handle: cx.focus_handle(),
            dragging_node: None,
            node_dragged: false,
            selected_nodes: HashSet::new(),
            is_panning: false,
            pan_start: Point::default(),
            pan_start_offset: Point::default(),
            mouse_down_pos: None,
            element_bounds: None,
            show_all_edges: false,
            loading: false,
            error: None,
        }
    }

    pub fn set_model(&mut self, model: GraphModel, cx: &mut Context<Self>) {
        self.model = model;
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.model = GraphModel::default();
        self.viewport = GraphViewport {
            pan_offset: Point::default(),
            zoom_level: 1.0,
        };
        self.error = None;
        cx.notify();
    }

    /// Indices of every node directly connected by a foreign key to any node in
    /// the current selection, in either direction (inbound or outbound).
    fn connected_to_selection(&self) -> HashSet<usize> {
        if self.selected_nodes.is_empty() {
            return HashSet::new();
        }

        let node_index: HashMap<&str, usize> = self
            .model
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.as_str(), i))
            .collect();

        let selected_ids: HashSet<&str> = self
            .selected_nodes
            .iter()
            .filter_map(|&ix| self.model.nodes.get(ix).map(|n| n.id.as_str()))
            .collect();

        let mut connected = HashSet::new();
        for edge in &self.model.edges {
            if selected_ids.contains(edge.from_node.as_str()) {
                if let Some(&ix) = node_index.get(edge.to_node.as_str()) {
                    connected.insert(ix);
                }
            }
            if selected_ids.contains(edge.to_node.as_str()) {
                if let Some(&ix) = node_index.get(edge.from_node.as_str()) {
                    connected.insert(ix);
                }
            }
        }
        connected
    }

    fn graph_to_screen(&self, graph_pos: Point<f32>) -> Point<f32> {
        Point::new(
            (graph_pos.x + self.viewport.pan_offset.x) * self.viewport.zoom_level,
            (graph_pos.y + self.viewport.pan_offset.y) * self.viewport.zoom_level,
        )
    }

    fn screen_to_graph(&self, screen_pos: Point<Pixels>) -> Point<f32> {
        Point::new(
            screen_pos.x.as_f32() / self.viewport.zoom_level - self.viewport.pan_offset.x,
            screen_pos.y.as_f32() / self.viewport.zoom_level - self.viewport.pan_offset.y,
        )
    }

    fn visible_graph_bounds(&self) -> Option<(Point<f32>, Point<f32>)> {
        let bounds = self.element_bounds?;
        let padding = 200.0 / self.viewport.zoom_level;
        let top_left = self.screen_to_graph(point(px(-padding), px(-padding)));
        let bottom_right = self.screen_to_graph(point(
            bounds.size.width + px(padding),
            bounds.size.height + px(padding),
        ));
        Some((top_left, bottom_right))
    }

    fn is_node_visible(&self, node: &GraphNode) -> bool {
        let Some((top_left, bottom_right)) = self.visible_graph_bounds() else {
            return true;
        };
        node.position.x + node.size.width >= top_left.x
            && node.position.x <= bottom_right.x
            && node.position.y + node.size.height >= top_left.y
            && node.position.y <= bottom_right.y
    }

    fn field_y_offset(field_index: usize) -> f32 {
        NODE_HEADER_HEIGHT
            + field_index as f32 * NODE_FIELD_ROW_HEIGHT
            + NODE_FIELD_ROW_HEIGHT / 2.0
    }

    fn collect_edge_data(&self) -> Vec<EdgeRenderData> {
        let visible_ids: HashSet<String> = self
            .model
            .nodes
            .iter()
            .filter(|n| self.is_node_visible(n))
            .map(|n| n.id.clone())
            .collect();

        let node_map: HashMap<&str, &GraphNode> = self
            .model
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n))
            .collect();

        let selected_ids: HashSet<&str> = self
            .selected_nodes
            .iter()
            .filter_map(|&ix| self.model.nodes.get(ix).map(|n| n.id.as_str()))
            .collect();

        // In on-demand mode (the default) only edges touching a selected node
        // are drawn. With nothing selected there is nothing to draw, so bail.
        if !self.show_all_edges && selected_ids.is_empty() {
            return Vec::new();
        }

        let touches_selection = |e: &GraphEdge| {
            selected_ids.contains(e.from_node.as_str()) || selected_ids.contains(e.to_node.as_str())
        };

        self.model
            .edges
            .iter()
            .filter(|e| {
                if self.show_all_edges {
                    visible_ids.contains(&e.from_node) || visible_ids.contains(&e.to_node)
                } else {
                    touches_selection(e)
                }
            })
            .filter_map(|e| {
                let from_node = node_map.get(e.from_node.as_str())?;
                let to_node = node_map.get(e.to_node.as_str())?;

                let is_highlighted = touches_selection(e);

                let from_center_y = from_node.position.y + from_node.size.height / 2.0;
                let to_center_y = to_node.position.y + to_node.size.height / 2.0;
                let from_right = from_node.position.x + from_node.size.width;
                let to_right = to_node.position.x + to_node.size.width;

                let dx = to_node.position.x - from_right;
                let dy = to_center_y - from_center_y;

                let (from_anchor, from_side) = if dx > from_node.size.width * 0.3 {
                    let p = Point::new(
                        from_right,
                        from_node.position.y + Self::field_y_offset(e.from_field_index),
                    );
                    (p, AnchorSide::Right)
                } else if dx < -(to_node.size.width * 0.3) {
                    let p = Point::new(
                        from_node.position.x,
                        from_node.position.y + Self::field_y_offset(e.from_field_index),
                    );
                    (p, AnchorSide::Left)
                } else {
                    let p = Point::new(
                        from_node.position.x + from_node.size.width / 2.0,
                        from_node.position.y + from_node.size.height,
                    );
                    (p, AnchorSide::Bottom)
                };

                let (to_anchor, to_side) = if dx > from_node.size.width * 0.3 {
                    let p = Point::new(
                        to_node.position.x,
                        to_node.position.y + Self::field_y_offset(e.to_field_index),
                    );
                    (p, AnchorSide::Left)
                } else if dx < -(to_node.size.width * 0.3) {
                    let p = Point::new(
                        to_right,
                        to_node.position.y + Self::field_y_offset(e.to_field_index),
                    );
                    (p, AnchorSide::Right)
                } else if dy > 0.0 {
                    let p = Point::new(
                        to_node.position.x + to_node.size.width / 2.0,
                        to_node.position.y,
                    );
                    (p, AnchorSide::Top)
                } else {
                    let p = Point::new(
                        to_node.position.x + to_node.size.width / 2.0,
                        to_node.position.y + to_node.size.height,
                    );
                    (p, AnchorSide::Bottom)
                };

                let from_screen = self.graph_to_screen(from_anchor);
                let to_screen = self.graph_to_screen(to_anchor);

                Some(EdgeRenderData {
                    from: point(px(from_screen.x), px(from_screen.y)),
                    to: point(px(to_screen.x), px(to_screen.y)),
                    from_side,
                    to_side,
                    is_highlighted,
                })
            })
            .collect()
    }

    fn collect_node_data(&self, cx: &App) -> Vec<NodeRenderData> {
        let zoom = self.viewport.zoom_level;
        let connected = self.connected_to_selection();

        self.model
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| self.is_node_visible(node))
            .map(|(ix, node)| {
                let screen_pos = self.graph_to_screen(node.position);
                NodeRenderData {
                    ix,
                    screen_x: screen_pos.x,
                    screen_y: screen_pos.y,
                    width: node.size.width * zoom,
                    height: node.size.height * zoom,
                    is_dragging: self.dragging_node.map_or(false, |(dix, _)| dix == ix),
                    is_selected: self.selected_nodes.contains(&ix),
                    is_connected: connected.contains(&ix),
                    title: node.title.clone(),
                    fields: node
                        .fields
                        .iter()
                        .map(|f| {
                            let badges: Vec<(SharedString, Hsla)> = f
                                .badges
                                .iter()
                                .map(|b| (SharedString::from(b.label()), b.color(cx)))
                                .collect();
                            (f.label.clone(), f.type_label.clone(), badges)
                        })
                        .collect(),
                }
            })
            .collect()
    }

    fn hit_test_node(&self, graph_pos: Point<f32>) -> Option<usize> {
        for (i, node) in self.model.nodes.iter().enumerate() {
            if graph_pos.x >= node.position.x
                && graph_pos.x <= node.position.x + node.size.width
                && graph_pos.y >= node.position.y
                && graph_pos.y <= node.position.y + node.size.height
            {
                return Some(i);
            }
        }
        None
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let element_pos =
            event.position - self.element_bounds.map_or(Point::default(), |b| b.origin);
        let graph_pos = self.screen_to_graph(element_pos);

        self.mouse_down_pos = Some(event.position.map(|p| p.as_f32()));
        self.pan_start = event.position.map(|p| p.as_f32());
        self.pan_start_offset = self.viewport.pan_offset;
        self.node_dragged = false;

        if event.button == MouseButton::Left {
            if let Some(node_ix) = self.hit_test_node(graph_pos) {
                let node = &self.model.nodes[node_ix];
                let offset =
                    Point::new(graph_pos.x - node.position.x, graph_pos.y - node.position.y);
                self.dragging_node = Some((node_ix, offset));
            }
        }
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mouse_pos = event.position.map(|p| p.as_f32());

        // A press only becomes a drag (node move or pan) once the pointer has
        // travelled past the threshold. Below it, a release is treated as a
        // click for selection.
        let past_threshold = self.mouse_down_pos.is_some_and(|down| {
            ((mouse_pos.x - down.x).powi(2) + (mouse_pos.y - down.y).powi(2)).sqrt() > PAN_THRESHOLD
        });

        if let Some((node_ix, offset)) = self.dragging_node {
            if !past_threshold {
                return;
            }
            self.node_dragged = true;
            let element_pos =
                event.position - self.element_bounds.map_or(Point::default(), |b| b.origin);
            let graph_pos = self.screen_to_graph(element_pos);
            self.model.nodes[node_ix].position =
                Point::new(graph_pos.x - offset.x, graph_pos.y - offset.y);
            cx.notify();
            return;
        }

        if self.is_panning {
            let delta = Point::new(
                mouse_pos.x - self.pan_start.x,
                mouse_pos.y - self.pan_start.y,
            );
            self.viewport.pan_offset = Point::new(
                self.pan_start_offset.x + delta.x / self.viewport.zoom_level,
                self.pan_start_offset.y + delta.y / self.viewport.zoom_level,
            );
            cx.notify();
            return;
        }

        // Press started on empty space; begin panning once past the threshold.
        if past_threshold && self.mouse_down_pos.is_some() {
            self.is_panning = true;
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let was_dragging = self.node_dragged || self.is_panning;

        if !was_dragging && event.button == MouseButton::Left {
            let element_pos =
                event.position - self.element_bounds.map_or(Point::default(), |b| b.origin);
            let graph_pos = self.screen_to_graph(element_pos);
            // Cmd (macOS) / Ctrl adds or removes from the current selection;
            // a plain click selects a single table (or clears the selection).
            let additive = event.modifiers.secondary();

            if let Some(node_ix) = self.hit_test_node(graph_pos) {
                if additive {
                    if !self.selected_nodes.insert(node_ix) {
                        self.selected_nodes.remove(&node_ix);
                    }
                } else if self.selected_nodes.len() == 1 && self.selected_nodes.contains(&node_ix) {
                    self.selected_nodes.clear();
                } else {
                    self.selected_nodes.clear();
                    self.selected_nodes.insert(node_ix);
                }
            } else if !additive {
                self.selected_nodes.clear();
            }
        }

        self.dragging_node = None;
        self.node_dragged = false;
        self.is_panning = false;
        self.mouse_down_pos = None;
        cx.notify();
    }

    fn on_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let element_pos =
            event.position - self.element_bounds.map_or(Point::default(), |b| b.origin);
        let old_zoom = self.viewport.zoom_level;

        let scroll_y: f32 = match event.delta {
            ScrollDelta::Pixels(p) => -p.y.as_f32(),
            ScrollDelta::Lines(l) => -l.y * 20.0,
        };

        let zoom_factor = if scroll_y > 0.0 { 1.1 } else { 0.9 };
        let new_zoom = (old_zoom * zoom_factor).clamp(MIN_ZOOM, MAX_ZOOM);

        let graph_pos_under_cursor = self.screen_to_graph(element_pos);
        self.viewport.zoom_level = new_zoom;

        let new_screen = self.graph_to_screen(graph_pos_under_cursor);
        self.viewport.pan_offset.x += (element_pos.x.as_f32() - new_screen.x) / new_zoom;
        self.viewport.pan_offset.y += (element_pos.y.as_f32() - new_screen.y) / new_zoom;

        cx.notify();
    }
}

impl Focusable for GraphView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for GraphView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let edges_data = self.collect_edge_data();
        let nodes_data = self.collect_node_data(cx);

        let has_content = !self.model.nodes.is_empty();
        let loading = self.loading;
        let error = self.error.clone();
        let show_all_edges = self.show_all_edges;
        let selected_count = self.selected_nodes.len();

        let edge_color = cx.theme().border;
        let highlight_color = cx.theme().cyan;
        let bg = cx.theme().background;
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger_foreground;
        let foreground = cx.theme().foreground;
        let theme = cx.theme().clone();

        let zoom = self.viewport.zoom_level;
        let header_height = NODE_HEADER_HEIGHT * zoom;
        let field_row_height = NODE_FIELD_ROW_HEIGHT * zoom;
        let font_size = (13.0 * zoom).max(6.0);
        let badge_font_size = (10.0 * zoom).max(5.0);
        let border_radius = px(NODE_BORDER_RADIUS * zoom);

        let view = cx.entity().downgrade();
        let edges_canvas = canvas(
            move |bounds: Bounds<Pixels>, _, cx| {
                // The graph element's window-space bounds are needed to map
                // mouse positions (which are in window coordinates) into graph
                // space, and to cull off-screen nodes. Capture them here and
                // re-render once if they changed.
                view.update(cx, |this, cx| {
                    if this.element_bounds != Some(bounds) {
                        this.element_bounds = Some(bounds);
                        cx.notify();
                    }
                })
                .ok();
            },
            move |bounds: Bounds<Pixels>, _, window, _cx| {
                let origin = bounds.origin;
                for edge in &edges_data {
                    let from = point(edge.from.x + origin.x, edge.from.y + origin.y);
                    let to = point(edge.to.x + origin.x, edge.to.y + origin.y);

                    let dx = (to.x - from.x).as_f32();
                    let dy = (to.y - from.y).as_f32();
                    let dist = (dx * dx + dy * dy).sqrt();

                    let (control_a, control_b) = match (&edge.from_side, &edge.to_side) {
                        (AnchorSide::Right, AnchorSide::Left) => {
                            let offset = px((dist * 0.4).clamp(50.0, 250.0));
                            (point(from.x + offset, from.y), point(to.x - offset, to.y))
                        }
                        (AnchorSide::Left, AnchorSide::Right) => {
                            let offset = px((dist * 0.4).clamp(50.0, 250.0));
                            (point(from.x - offset, from.y), point(to.x + offset, to.y))
                        }
                        (AnchorSide::Bottom, AnchorSide::Top) => {
                            let offset = px((dist * 0.4).clamp(50.0, 200.0));
                            (point(from.x, from.y + offset), point(to.x, to.y - offset))
                        }
                        (AnchorSide::Bottom, AnchorSide::Bottom) => {
                            let offset = px((dy.abs() * 0.5).clamp(40.0, 200.0));
                            (point(from.x, from.y + offset), point(to.x, to.y + offset))
                        }
                        (AnchorSide::Right, AnchorSide::Top) => {
                            let h = px((dx.abs() * 0.4).clamp(40.0, 150.0));
                            let v = px((dy.abs() * 0.4).clamp(40.0, 150.0));
                            (point(from.x + h, from.y), point(to.x, to.y - v))
                        }
                        (AnchorSide::Right, AnchorSide::Bottom) => {
                            let h = px((dx.abs() * 0.4).clamp(40.0, 150.0));
                            let v = px((dy.abs() * 0.4).clamp(40.0, 150.0));
                            (point(from.x + h, from.y), point(to.x, to.y + v))
                        }
                        (AnchorSide::Left, AnchorSide::Top) => {
                            let h = px((dx.abs() * 0.4).clamp(40.0, 150.0));
                            let v = px((dy.abs() * 0.4).clamp(40.0, 150.0));
                            (point(from.x - h, from.y), point(to.x, to.y - v))
                        }
                        (AnchorSide::Left, AnchorSide::Bottom) => {
                            let h = px((dx.abs() * 0.4).clamp(40.0, 150.0));
                            let v = px((dy.abs() * 0.4).clamp(40.0, 150.0));
                            (point(from.x - h, from.y), point(to.x, to.y + v))
                        }
                        _ => {
                            let offset = px((dist * 0.35).clamp(40.0, 250.0));
                            (point(from.x + offset, from.y), point(to.x - offset, to.y))
                        }
                    };

                    let stroke_width = if edge.is_highlighted {
                        px(2.5)
                    } else {
                        px(1.5)
                    };
                    let color = if edge.is_highlighted {
                        highlight_color
                    } else {
                        edge_color
                    };
                    let mut builder = PathBuilder::stroke(stroke_width);
                    builder.move_to(from);
                    builder.cubic_bezier_to(to, control_a, control_b);

                    if let Ok(path) = builder.build() {
                        window.paint_path(path, color);
                    }
                }
            },
        )
        .absolute()
        .inset_0();

        v_flex()
            .id("graph-view")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_hidden()
            .bg(bg)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(edges_canvas)
            .children(nodes_data.into_iter().map(move |node_data| {
                let border_color = if node_data.is_selected {
                    theme.cyan
                } else if node_data.is_connected {
                    theme.cyan.opacity(0.6)
                } else {
                    theme.border
                };
                div()
                    .absolute()
                    .left(px(node_data.screen_x))
                    .top(px(node_data.screen_y))
                    .w(px(node_data.width))
                    .h(px(node_data.height))
                    .rounded(border_radius)
                    .border_1()
                    .when(node_data.is_selected || node_data.is_connected, |el| {
                        el.border_2()
                    })
                    .border_color(border_color)
                    .bg(theme.background)
                    .when(node_data.is_dragging, |el| el.shadow_lg())
                    .overflow_hidden()
                    .cursor_grab()
                    .child(
                        div()
                            .w_full()
                            .h(px(header_height))
                            .flex()
                            .items_center()
                            .px(px(NODE_PADDING * zoom))
                            .bg(theme.title_bar)
                            .rounded_t(border_radius)
                            .child(
                                div()
                                    .text_size(px(font_size))
                                    .text_color(theme.foreground)
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(node_data.title),
                            ),
                    )
                    .child(
                        div().w_full().flex().flex_col().children(
                            node_data
                                .fields
                                .into_iter()
                                .map(|(label, type_label, badges)| {
                                    div()
                                        .w_full()
                                        .h(px(field_row_height))
                                        .flex()
                                        .items_center()
                                        .px(px(NODE_PADDING * zoom))
                                        .gap(px(4.0 * zoom))
                                        .child(
                                            div()
                                                .flex_1()
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .text_size(px(font_size))
                                                .text_color(theme.foreground)
                                                .child(label),
                                        )
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .text_size(px(font_size * 0.85))
                                                .text_color(theme.muted_foreground)
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .child(type_label),
                                        )
                                        .children(badges.into_iter().map(
                                            |(badge_label, badge_color)| {
                                                div()
                                                    .px(px(3.0 * zoom))
                                                    .rounded(px(3.0 * zoom))
                                                    .bg(badge_color.opacity(0.2))
                                                    .text_size(px(badge_font_size))
                                                    .text_color(badge_color)
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .child(badge_label)
                                            },
                                        ))
                                }),
                        ),
                    )
                    .into_any_element()
            }))
            .when(has_content, |el| {
                let hint: Option<SharedString> = if show_all_edges {
                    None
                } else if selected_count == 0 {
                    Some("Click a table to see its relationships".into())
                } else if selected_count == 1 {
                    Some("1 selected · cmd/ctrl-click to add more".into())
                } else {
                    Some(format!("{selected_count} selected · cmd/ctrl-click to add more").into())
                };

                el.child(
                    div()
                        .absolute()
                        .top(px(8.0))
                        .right(px(8.0))
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap_1()
                        .child(
                            div()
                                .id("toggle-all-edges")
                                .px_2()
                                .py_1()
                                .rounded(px(4.0))
                                .border_1()
                                .border_color(edge_color)
                                .bg(bg)
                                .text_size(px(12.0))
                                .text_color(foreground)
                                .cursor_pointer()
                                .child(if show_all_edges {
                                    "Showing all relationships"
                                } else {
                                    "Relationships: selected only"
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.show_all_edges = !this.show_all_edges;
                                        cx.stop_propagation();
                                        cx.notify();
                                    }),
                                ),
                        )
                        .when_some(hint, |el, hint| {
                            el.child(
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(4.0))
                                    .bg(bg)
                                    .text_size(px(11.0))
                                    .text_color(muted)
                                    .child(hint),
                            )
                        }),
                )
            })
            .when(!has_content && !loading && error.is_none(), |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(muted)
                        .child("No tables found"),
                )
            })
            .when(loading, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(muted)
                        .child("Loading schema..."),
                )
            })
            .when_some(error, |el, err| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(danger)
                        .child(err),
                )
            })
    }
}

pub fn compute_node_height(fields: usize) -> f32 {
    NODE_HEADER_HEIGHT + fields as f32 * NODE_FIELD_ROW_HEIGHT + NODE_PADDING
}

pub fn compute_node_sizes(nodes: &mut Vec<GraphNode>) {
    let char_width = 8.0;
    let badge_width = 28.0;
    for node in nodes.iter_mut() {
        let title_width = node.title.len() as f32 * char_width + NODE_PADDING * 2.0;
        let mut max_field_width = 0.0f32;
        for field in &node.fields {
            let field_width = field.label.len() as f32 * char_width
                + field.type_label.len() as f32 * (char_width * 0.85)
                + field.badges.len() as f32 * badge_width
                + NODE_PADDING * 2.0;
            max_field_width = max_field_width.max(field_width);
        }
        node.size.width = title_width.max(max_field_width).max(NODE_MIN_WIDTH);
        node.size.height = compute_node_height(node.fields.len());
    }
}

pub fn layout_dag(nodes: &mut Vec<GraphNode>, edges: &[GraphEdge]) {
    if nodes.is_empty() {
        return;
    }

    let node_index: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.clone(), i))
        .collect();

    // Build adjacency in the FK direction: from_node (has FK column) -> to_node (referenced table)
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    // reverse_adjacency: to_node -> from_node (who references me)
    let mut reverse_adjacency: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];

    for edge in edges {
        if let (Some(&from_ix), Some(&to_ix)) = (
            node_index.get(&edge.from_node),
            node_index.get(&edge.to_node),
        ) {
            adjacency[from_ix].push(to_ix);
            reverse_adjacency[to_ix].push(from_ix);
        }
    }

    // Assign layers. Referenced tables (FK targets) should be in earlier layers (left).
    // A node's layer = 1 + max layer of all nodes it references.
    // Nodes with no outgoing FK edges get layer 0.
    let mut visited = vec![false; nodes.len()];
    let mut on_stack = vec![false; nodes.len()];
    let mut layers = vec![0usize; nodes.len()];

    fn assign_layers(
        node_ix: usize,
        adjacency: &[Vec<usize>],
        layers: &mut [usize],
        visited: &mut [bool],
        on_stack: &mut [bool],
    ) {
        visited[node_ix] = true;
        on_stack[node_ix] = true;
        for &neighbor in &adjacency[node_ix] {
            if on_stack[neighbor] {
                continue;
            }
            if !visited[neighbor] {
                assign_layers(neighbor, adjacency, layers, visited, on_stack);
            }
            layers[node_ix] = layers[node_ix].max(layers[neighbor] + 1);
        }
        on_stack[node_ix] = false;
    }

    // Start from nodes with no outgoing edges (leaf/reference targets)
    let has_outgoing: Vec<bool> = (0..nodes.len()).map(|i| !adjacency[i].is_empty()).collect();

    for i in 0..nodes.len() {
        if !has_outgoing[i] && !visited[i] {
            assign_layers(i, &adjacency, &mut layers, &mut visited, &mut on_stack);
        }
    }
    // Catch any remaining unvisited nodes (isolated or in cycles)
    for i in 0..nodes.len() {
        if !visited[i] {
            assign_layers(i, &adjacency, &mut layers, &mut visited, &mut on_stack);
        }
    }

    // Tables with no foreign keys in either direction are laid out separately
    // in a grid block rather than dumped into layer 0, where they would form a
    // single very tall column and clutter the relationship lines.
    let isolated: Vec<usize> = (0..nodes.len())
        .filter(|&i| adjacency[i].is_empty() && reverse_adjacency[i].is_empty())
        .collect();
    let isolated_set: HashSet<usize> = isolated.iter().copied().collect();

    let max_layer = layers.iter().copied().max().unwrap_or(0);
    let mut layer_groups: Vec<Vec<usize>> = vec![Vec::new(); max_layer + 1];
    for (i, &layer) in layers.iter().enumerate() {
        if !isolated_set.contains(&i) {
            layer_groups[layer].push(i);
        }
    }

    // Barycenter heuristic to minimize edge crossings within each layer.
    // Process layers left to right. For each node, compute the average
    // x position (via current index in the layer) of its neighbors in the previous layer.
    let mut positions_in_layer: Vec<usize> = vec![0; nodes.len()];

    for _ in 0..LAYOUT_BARYCENTER_SWEEPS {
        for (layer_ix, group) in layer_groups.iter_mut().enumerate() {
            if layer_ix == 0 {
                for (pos, &node_ix) in group.iter().enumerate() {
                    positions_in_layer[node_ix] = pos;
                }
                continue;
            }

            let mut scored: Vec<(f64, usize)> = group
                .iter()
                .map(|&node_ix| {
                    let neighbors_in_prev: Vec<usize> = reverse_adjacency[node_ix]
                        .iter()
                        .chain(adjacency[node_ix].iter())
                        .filter(|&&n| layers[n] == layer_ix - 1)
                        .map(|&n| positions_in_layer[n])
                        .collect();

                    let bary = if neighbors_in_prev.is_empty() {
                        // No connections to previous layer: use node_ix as tiebreaker to keep stable
                        node_ix as f64
                    } else {
                        neighbors_in_prev.iter().map(|&p| p as f64).sum::<f64>()
                            / neighbors_in_prev.len() as f64
                    };
                    (bary, node_ix)
                })
                .collect();

            scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            *group = scored.iter().map(|(_, ix)| *ix).collect();
            for (pos, (_, node_ix)) in scored.iter().enumerate() {
                positions_in_layer[*node_ix] = pos;
            }
        }

        // Right-to-left pass: refine positions based on neighbors in both adjacent layers
        for layer_ix in (1..layer_groups.len()).rev() {
            let group_indices: Vec<usize> = layer_groups[layer_ix].clone();

            let scored: Vec<(f64, usize)> = group_indices
                .iter()
                .map(|&node_ix| {
                    let mut neighbor_positions: Vec<usize> = Vec::new();
                    for &n in &reverse_adjacency[node_ix] {
                        if layers[n] == layer_ix - 1 || layers[n] == layer_ix + 1 {
                            neighbor_positions.push(positions_in_layer[n]);
                        }
                    }
                    for &n in &adjacency[node_ix] {
                        if layers[n] == layer_ix - 1 || layers[n] == layer_ix + 1 {
                            neighbor_positions.push(positions_in_layer[n]);
                        }
                    }

                    let bary = if neighbor_positions.is_empty() {
                        node_ix as f64
                    } else {
                        neighbor_positions.iter().map(|&p| p as f64).sum::<f64>()
                            / neighbor_positions.len() as f64
                    };
                    (bary, node_ix)
                })
                .collect();

            let mut sorted = scored;
            sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            layer_groups[layer_ix] = sorted.iter().map(|(_, ix)| *ix).collect();
            for (pos, (_, node_ix)) in sorted.iter().enumerate() {
                positions_in_layer[*node_ix] = pos;
            }
        }
    }

    // Wrap each layer into one or more vertical sub-columns so that very tall
    // layers don't become a single column running off the screen. Layer bands
    // advance horizontally by their actual width (which grows with the number
    // of sub-columns), so wrapped layers don't overlap their neighbours.
    let column_height = |column: &[usize], nodes: &[GraphNode]| -> f32 {
        column.iter().map(|&ix| nodes[ix].size.height).sum::<f32>()
            + column.len().saturating_sub(1) as f32 * LAYOUT_NODE_GAP_Y
    };

    struct LayerLayout {
        x: f32,
        node_width: f32,
        sub_columns: Vec<Vec<usize>>,
    }

    let mut layouts: Vec<LayerLayout> = Vec::with_capacity(layer_groups.len());
    let mut x_cursor = LAYOUT_START;
    let mut max_column_height = 0.0f32;

    for group in &layer_groups {
        if group.is_empty() {
            continue;
        }

        let node_width = group
            .iter()
            .map(|&ix| nodes[ix].size.width)
            .fold(NODE_MIN_WIDTH, f32::max);

        let total_height = column_height(group, nodes);
        let column_count = (total_height / LAYOUT_MAX_COLUMN_HEIGHT).ceil().max(1.0) as usize;
        let rows_per_column = group.len().div_ceil(column_count);

        let mut sub_columns: Vec<Vec<usize>> = vec![Vec::new(); column_count];
        for (position, &node_ix) in group.iter().enumerate() {
            let column = (position / rows_per_column).min(column_count - 1);
            sub_columns[column].push(node_ix);
        }

        for column in &sub_columns {
            max_column_height = max_column_height.max(column_height(column, nodes));
        }

        let band_width = column_count as f32 * node_width
            + column_count.saturating_sub(1) as f32 * LAYOUT_SUBCOL_GAP_X;

        layouts.push(LayerLayout {
            x: x_cursor,
            node_width,
            sub_columns,
        });
        x_cursor += band_width + LAYOUT_LAYER_GAP_X;
    }

    for layout in &layouts {
        for (column_ix, column) in layout.sub_columns.iter().enumerate() {
            let centering = ((max_column_height - column_height(column, nodes)) / 2.0).max(0.0);
            let x = layout.x + column_ix as f32 * (layout.node_width + LAYOUT_SUBCOL_GAP_X);
            let mut y = LAYOUT_START + centering;
            for &node_ix in column {
                nodes[node_ix].position = Point::new(x, y);
                y += nodes[node_ix].size.height + LAYOUT_NODE_GAP_Y;
            }
        }
    }

    // Grid-pack the isolated tables below the connected graph so they stay out
    // of the relationship lines instead of forming a tall column on the left.
    if !isolated.is_empty() {
        let cell_width = isolated
            .iter()
            .map(|&ix| nodes[ix].size.width)
            .fold(NODE_MIN_WIDTH, f32::max)
            + LAYOUT_SUBCOL_GAP_X;
        let cell_height = isolated
            .iter()
            .map(|&ix| nodes[ix].size.height)
            .fold(0.0f32, f32::max)
            + LAYOUT_NODE_GAP_Y;

        let columns = (isolated.len() as f32).sqrt().ceil().max(1.0) as usize;
        let connected_height = if layouts.is_empty() {
            0.0
        } else {
            max_column_height + LAYOUT_NODE_GAP_Y * 2.0
        };
        let start_y = LAYOUT_START + connected_height;

        for (k, &node_ix) in isolated.iter().enumerate() {
            let row = k / columns;
            let column = k % columns;
            nodes[node_ix].position = Point::new(
                LAYOUT_START + column as f32 * cell_width,
                start_y + row as f32 * cell_height,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            title: id.to_string().into(),
            fields: vec![GraphNodeField {
                label: "id".into(),
                type_label: "int".into(),
                badges: Vec::new(),
            }],
            position: Point::default(),
            size: Size::default(),
        }
    }

    fn edge(from: &str, to: &str) -> GraphEdge {
        GraphEdge {
            from_node: from.to_string(),
            from_field_index: 0,
            to_node: to.to_string(),
            to_field_index: 0,
        }
    }

    fn position_of(nodes: &[GraphNode], id: &str) -> Point<f32> {
        nodes.iter().find(|n| n.id == id).unwrap().position
    }

    #[test]
    fn referenced_table_is_left_of_referencing_table() {
        // `a` has a foreign key to `b`, so `b` (the target) sits in an earlier
        // layer and should be laid out to the left of `a`.
        let mut nodes = vec![node("a"), node("b")];
        let edges = vec![edge("a", "b")];
        compute_node_sizes(&mut nodes);
        layout_dag(&mut nodes, &edges);

        assert!(position_of(&nodes, "b").x < position_of(&nodes, "a").x);
    }

    #[test]
    fn isolated_tables_are_packed_below_the_connected_graph() {
        let mut nodes = vec![node("a"), node("b"), node("lonely")];
        let edges = vec![edge("a", "b")];
        compute_node_sizes(&mut nodes);
        layout_dag(&mut nodes, &edges);

        let connected_bottom = position_of(&nodes, "a").y.max(position_of(&nodes, "b").y);
        assert!(position_of(&nodes, "lonely").y > connected_bottom);
    }

    #[test]
    fn tall_layer_wraps_into_multiple_sub_columns() {
        // One hub table references many others; the referenced tables all land
        // in a single layer that is too tall and must wrap horizontally.
        let mut nodes: Vec<GraphNode> = (0..40).map(|i| node(&format!("t{i}"))).collect();
        nodes.push(node("hub"));
        let edges: Vec<GraphEdge> = (0..40).map(|i| edge("hub", &format!("t{i}"))).collect();

        compute_node_sizes(&mut nodes);
        layout_dag(&mut nodes, &edges);

        let distinct_columns: HashSet<i32> = nodes
            .iter()
            .filter(|n| n.id.starts_with('t'))
            .map(|n| n.position.x as i32)
            .collect();
        assert!(
            distinct_columns.len() > 1,
            "a layer taller than the cap should wrap into multiple columns"
        );
    }
}
