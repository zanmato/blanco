use std::cmp::Reverse;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::hash::{Hash, Hasher};

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

/// Resolution (graph units per cell) of the grid used for edge path-finding.
/// Smaller is more precise but quadratically more expensive.
const ROUTE_CELL: f32 = 16.0;
/// Clearance kept around every table when rasterising obstacles, so routed
/// lines never hug a node's border.
const ROUTE_NODE_PADDING: f32 = 12.0;
/// Extra cost added by A* whenever the path changes direction, biasing it
/// toward long straight runs with few bends (the "step" look) instead of
/// staircases through open space.
const ROUTE_TURN_PENALTY: i32 = 3;
/// Corner radius applied to routed edges, in graph units (scaled by zoom at
/// paint time).
const ROUTE_CORNER_RADIUS: f32 = 10.0;

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

/// Per-frame layout values shared by every node, captured once in `render` so
/// `render_graph_node` stays a plain function (nodes carry no event handlers).
struct NodeRenderParams {
    theme: gpui_component::Theme,
    border_radius: Pixels,
    header_height: f32,
    field_row_height: f32,
    font_size: f32,
    badge_font_size: f32,
    zoom: f32,
}

/// Render a single table node from precomputed layout data. Returns an owned
/// `AnyElement` so the result does not borrow `params` (it is reused per node).
fn render_graph_node(node_data: NodeRenderData, params: &NodeRenderParams) -> gpui::AnyElement {
    let theme = &params.theme;
    let zoom = params.zoom;
    let font_size = params.font_size;
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
        .rounded(params.border_radius)
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
                .h(px(params.header_height))
                .flex()
                .items_center()
                .px(px(NODE_PADDING * zoom))
                .bg(theme.title_bar)
                .rounded_t(params.border_radius)
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
            div()
                .w_full()
                .flex()
                .flex_col()
                .children(
                    node_data
                        .fields
                        .into_iter()
                        .map(|(label, type_label, badges)| {
                            div()
                                .w_full()
                                .h(px(params.field_row_height))
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
                                .children(badges.into_iter().map(|(badge_label, badge_color)| {
                                    div()
                                        .px(px(3.0 * zoom))
                                        .rounded(px(3.0 * zoom))
                                        .bg(badge_color.opacity(0.2))
                                        .text_size(px(params.badge_font_size))
                                        .text_color(badge_color)
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(badge_label)
                                }))
                        }),
                ),
        )
        .into_any_element()
}

/// A fully routed edge in graph coordinates, cached between renders. Panning
/// and zooming only transform these points; they are recomputed solely when a
/// node moves or the visible edge set changes.
#[derive(Clone)]
struct RoutedEdge {
    /// Index into `GraphModel::edges`, so an individual route can be refreshed
    /// (e.g. while its node is dragged) without recomputing the whole set.
    edge_ix: usize,
    points: Vec<Point<f32>>,
    is_highlighted: bool,
}

struct EdgeRenderData {
    /// Screen-space (pre-origin) poly-line, ready to stroke.
    points: Vec<Point<Pixels>>,
    is_highlighted: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum AnchorSide {
    Right,
    Left,
    Bottom,
    Top,
}

impl AnchorSide {
    /// Unit vector pointing out of the node along this side.
    fn out_dir(self) -> (i32, i32) {
        match self {
            Self::Right => (1, 0),
            Self::Left => (-1, 0),
            Self::Bottom => (0, 1),
            Self::Top => (0, -1),
        }
    }
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
    /// Cached node-avoiding edge routes (graph space) and the hash of the inputs
    /// they were computed from. Recomputed lazily when the hash changes.
    routed_edges: Vec<RoutedEdge>,
    route_cache_key: Option<u64>,
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
            routed_edges: Vec::new(),
            route_cache_key: None,
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

    /// Hash of everything the routed paths depend on. Node positions/sizes, the
    /// selection, and the show-all toggle all change routing; pan and zoom do
    /// not (they are applied as a transform afterwards).
    fn route_cache_key(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.show_all_edges.hash(&mut hasher);

        let mut selected: Vec<usize> = self.selected_nodes.iter().copied().collect();
        selected.sort_unstable();
        selected.hash(&mut hasher);

        for node in &self.model.nodes {
            node.id.hash(&mut hasher);
            node.position.x.to_bits().hash(&mut hasher);
            node.position.y.to_bits().hash(&mut hasher);
            node.size.width.to_bits().hash(&mut hasher);
            node.size.height.to_bits().hash(&mut hasher);
        }
        self.model.edges.len().hash(&mut hasher);
        hasher.finish()
    }

    /// Recompute and cache the node-avoiding routes if any routing input has
    /// changed since the last call. Cheap no-op when nothing relevant moved.
    fn ensure_routes(&mut self) {
        let key = self.route_cache_key();
        if self.route_cache_key == Some(key) {
            return;
        }
        self.route_cache_key = Some(key);
        self.routed_edges = self.compute_routes();
    }

    fn selected_node_ids(&self) -> HashSet<&str> {
        self.selected_nodes
            .iter()
            .filter_map(|&ix| self.model.nodes.get(ix).map(|n| n.id.as_str()))
            .collect()
    }

    /// Route every currently-active edge around the tables using grid A*.
    fn compute_routes(&self) -> Vec<RoutedEdge> {
        let node_map: HashMap<&str, &GraphNode> = self
            .model
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n))
            .collect();

        let selected_ids = self.selected_node_ids();

        // On-demand mode with no selection draws nothing.
        if !self.show_all_edges && selected_ids.is_empty() {
            return Vec::new();
        }

        let touches_selection = |e: &GraphEdge| {
            selected_ids.contains(e.from_node.as_str()) || selected_ids.contains(e.to_node.as_str())
        };

        let Some(grid) = RouteGrid::build(&self.model.nodes) else {
            return Vec::new();
        };
        let mut search = AStarSearch::new(grid.cell_count());

        self.model
            .edges
            .iter()
            .enumerate()
            .filter(|(_, e)| self.show_all_edges || touches_selection(e))
            .filter_map(|(edge_ix, e)| {
                let from_node = *node_map.get(e.from_node.as_str())?;
                let to_node = *node_map.get(e.to_node.as_str())?;
                let is_highlighted = touches_selection(e);

                let (from_anchor, from_side, to_anchor, to_side) =
                    Self::edge_anchors(e, from_node, to_node);

                let points = grid
                    .route(from_anchor, from_side, to_anchor, to_side, &mut search)
                    .unwrap_or_else(|| vec![from_anchor, to_anchor]);

                Some(RoutedEdge {
                    edge_ix,
                    points,
                    is_highlighted,
                })
            })
            .collect()
    }

    /// Re-route only the edges touching `node_ix`, leaving every other cached
    /// route untouched. Used during a drag so each move re-runs A* for a handful
    /// of edges instead of the whole graph. Other edges keep their (now slightly
    /// stale) routes until the drag ends and a full recompute runs.
    fn reroute_node_edges(&mut self, node_ix: usize) {
        let Some(dragged_id) = self.model.nodes.get(node_ix).map(|n| n.id.clone()) else {
            return;
        };

        // Nothing routed touches the moved node: skip the grid build entirely
        // and just keep the cache marked current.
        let touches = self.routed_edges.iter().any(|r| {
            self.model
                .edges
                .get(r.edge_ix)
                .is_some_and(|e| e.from_node == dragged_id || e.to_node == dragged_id)
        });
        if !touches {
            self.route_cache_key = Some(self.route_cache_key());
            return;
        }

        let Some(grid) = RouteGrid::build(&self.model.nodes) else {
            return;
        };
        let mut search = AStarSearch::new(grid.cell_count());

        let node_map: HashMap<&str, &GraphNode> = self
            .model
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n))
            .collect();

        for routed in &mut self.routed_edges {
            let Some(e) = self.model.edges.get(routed.edge_ix) else {
                continue;
            };
            if e.from_node != dragged_id && e.to_node != dragged_id {
                continue;
            }
            let (Some(&from_node), Some(&to_node)) = (
                node_map.get(e.from_node.as_str()),
                node_map.get(e.to_node.as_str()),
            ) else {
                continue;
            };
            let (from_anchor, from_side, to_anchor, to_side) =
                Self::edge_anchors(e, from_node, to_node);
            routed.points = grid
                .route(from_anchor, from_side, to_anchor, to_side, &mut search)
                .unwrap_or_else(|| vec![from_anchor, to_anchor]);
        }

        // Mark the cache as current so the upcoming render's `ensure_routes` is a
        // no-op instead of triggering a full recompute for the moved node.
        self.route_cache_key = Some(self.route_cache_key());
    }

    /// Pick which side of each node the edge leaves/enters and the exact anchor
    /// point on that side, based on their relative position.
    fn edge_anchors(
        e: &GraphEdge,
        from_node: &GraphNode,
        to_node: &GraphNode,
    ) -> (Point<f32>, AnchorSide, Point<f32>, AnchorSide) {
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

        (from_anchor, from_side, to_anchor, to_side)
    }

    /// Transform the cached graph-space routes into screen-space poly-lines,
    /// dropping any that fall entirely outside the viewport.
    fn collect_edge_data(&self) -> Vec<EdgeRenderData> {
        let visible = self.visible_graph_bounds();

        self.routed_edges
            .iter()
            .filter(|edge| edge.points.len() >= 2)
            .filter(|edge| match visible {
                None => true,
                Some((top_left, bottom_right)) => {
                    let (mut min_x, mut min_y) = (f32::MAX, f32::MAX);
                    let (mut max_x, mut max_y) = (f32::MIN, f32::MIN);
                    for p in &edge.points {
                        min_x = min_x.min(p.x);
                        min_y = min_y.min(p.y);
                        max_x = max_x.max(p.x);
                        max_y = max_y.max(p.y);
                    }
                    max_x >= top_left.x
                        && min_x <= bottom_right.x
                        && max_y >= top_left.y
                        && min_y <= bottom_right.y
                }
            })
            .map(|edge| {
                let points = edge
                    .points
                    .iter()
                    .map(|p| {
                        let s = self.graph_to_screen(*p);
                        point(px(s.x), px(s.y))
                    })
                    .collect();
                EdgeRenderData {
                    points,
                    is_highlighted: edge.is_highlighted,
                }
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
            // Only re-route the moved node's own edges this frame; a full
            // recompute over every edge per drag frame is too slow on large
            // schemas.
            self.reroute_node_edges(node_ix);
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

        // A drag only re-routed the moved node's own edges each frame. Now that
        // it has settled, invalidate the cache so the next render does one full
        // recompute and the rest of the graph reacts to the new position.
        if self.node_dragged {
            self.route_cache_key = None;
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
        self.ensure_routes();
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
        // Tint the canvas with the secondary surface so the background-filled
        // table nodes read as distinct panels instead of blending in.
        let canvas_bg = cx.theme().secondary;
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger_foreground;
        let foreground = cx.theme().foreground;

        let zoom = self.viewport.zoom_level;
        let node_params = NodeRenderParams {
            theme: cx.theme().clone(),
            border_radius: px(NODE_BORDER_RADIUS * zoom),
            header_height: NODE_HEADER_HEIGHT * zoom,
            field_row_height: NODE_FIELD_ROW_HEIGHT * zoom,
            font_size: (13.0 * zoom).max(6.0),
            badge_font_size: (10.0 * zoom).max(5.0),
            zoom,
        };

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
                // Corner radius scales with zoom so bends stay proportional to
                // the nodes at any magnification.
                let radius = ROUTE_CORNER_RADIUS * zoom;

                for edge in &edges_data {
                    let waypoints: Vec<Point<Pixels>> = edge
                        .points
                        .iter()
                        .map(|p| point(p.x + origin.x, p.y + origin.y))
                        .collect();

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

                    if let Some(path) = build_rounded_path(&waypoints, radius, stroke_width) {
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
            .bg(canvas_bg)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(edges_canvas)
            .children(
                nodes_data
                    .into_iter()
                    .map(move |node_data| render_graph_node(node_data, &node_params)),
            )
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

/// A rasterised occupancy grid over the graph's bounding box. Cells covered by
/// a table (plus padding) are blocked; A* threads edges through the rest. The
/// grid is built once per route recomputation and shared by every edge.
struct RouteGrid {
    cols: i32,
    rows: i32,
    origin_x: f32,
    origin_y: f32,
    cell: f32,
    blocked: Vec<bool>,
}

impl RouteGrid {
    fn build(nodes: &[GraphNode]) -> Option<Self> {
        if nodes.is_empty() {
            return None;
        }

        let pad = ROUTE_NODE_PADDING;
        let mut x_min = f32::MAX;
        let mut y_min = f32::MAX;
        let mut x_max = f32::MIN;
        let mut y_max = f32::MIN;
        for n in nodes {
            x_min = x_min.min(n.position.x);
            y_min = y_min.min(n.position.y);
            x_max = x_max.max(n.position.x + n.size.width);
            y_max = y_max.max(n.position.y + n.size.height);
        }

        // Outer margin so a route can travel around the outside of the graph
        // when no interior channel is available.
        let margin = pad + ROUTE_CELL * 3.0;
        let origin_x = x_min - margin;
        let origin_y = y_min - margin;
        let cell = ROUTE_CELL;
        let cols = (((x_max + margin - origin_x) / cell).ceil() as i32).max(1) + 1;
        let rows = (((y_max + margin - origin_y) / cell).ceil() as i32).max(1) + 1;

        let mut grid = RouteGrid {
            cols,
            rows,
            origin_x,
            origin_y,
            cell,
            blocked: vec![false; (cols * rows) as usize],
        };

        for n in nodes {
            let (c0, r0) = grid.to_cell(n.position.x - pad, n.position.y - pad);
            let (c1, r1) = grid.to_cell(
                n.position.x + n.size.width + pad,
                n.position.y + n.size.height + pad,
            );
            for r in r0..=r1 {
                for c in c0..=c1 {
                    if let Some(i) = grid.index(c, r) {
                        grid.blocked[i] = true;
                    }
                }
            }
        }

        Some(grid)
    }

    fn cell_count(&self) -> usize {
        (self.cols * self.rows) as usize
    }

    fn to_cell(&self, x: f32, y: f32) -> (i32, i32) {
        (
            ((x - self.origin_x) / self.cell).round() as i32,
            ((y - self.origin_y) / self.cell).round() as i32,
        )
    }

    fn to_point(&self, c: i32, r: i32) -> Point<f32> {
        Point::new(
            self.origin_x + c as f32 * self.cell,
            self.origin_y + r as f32 * self.cell,
        )
    }

    fn index(&self, c: i32, r: i32) -> Option<usize> {
        if c < 0 || r < 0 || c >= self.cols || r >= self.rows {
            None
        } else {
            Some((r * self.cols + c) as usize)
        }
    }

    fn walkable(&self, c: i32, r: i32) -> bool {
        self.index(c, r).is_some_and(|i| !self.blocked[i])
    }

    /// Step out from `anchor` along `side` until a walkable cell is reached, so
    /// pathfinding starts in free space just outside the node border.
    fn endpoint_cell(&self, anchor: Point<f32>, side: AnchorSide) -> Option<(i32, i32)> {
        let (mut c, mut r) = self.to_cell(anchor.x, anchor.y);
        let (dc, dr) = side.out_dir();
        for _ in 0..64 {
            if self.walkable(c, r) {
                return Some((c, r));
            }
            c += dc;
            r += dr;
            if self.index(c, r).is_none() {
                return None;
            }
        }
        None
    }

    /// Route a single edge: A* between the two endpoints, then convert the grid
    /// path back to graph-space points with the real anchors at the ends. Returns
    /// `None` if no path exists (the caller falls back to a straight line).
    fn route(
        &self,
        from_anchor: Point<f32>,
        from_side: AnchorSide,
        to_anchor: Point<f32>,
        to_side: AnchorSide,
        search: &mut AStarSearch,
    ) -> Option<Vec<Point<f32>>> {
        let start = self.endpoint_cell(from_anchor, from_side)?;
        let end = self.endpoint_cell(to_anchor, to_side)?;
        let cells = search.find(self, start, end)?;
        let cells = compress_collinear(&cells);

        let mut points = Vec::with_capacity(cells.len() + 2);
        points.push(from_anchor);
        for &(c, r) in &cells {
            points.push(self.to_point(c, r));
        }
        points.push(to_anchor);

        // The grid-snapped path can sit up to half a cell off the real anchor,
        // giving a slight slant where the line meets a table. Pull the leading
        // and trailing straight runs onto the anchor so edges enter/leave
        // perpendicular to the node border.
        align_endpoint(&mut points, from_anchor, from_side, true);
        align_endpoint(&mut points, to_anchor, to_side, false);

        dedup_points(&mut points);
        Some(points)
    }
}

/// Reusable buffers for repeated A* searches over a grid of fixed size. A
/// per-search generation counter avoids clearing the score arrays each time.
struct AStarSearch {
    g_score: Vec<i32>,
    came_from: Vec<i32>,
    came_dir: Vec<u8>,
    generation: Vec<u32>,
    current_gen: u32,
    heap: BinaryHeap<Reverse<(i32, usize)>>,
}

/// 4-connected moves: +x, -x, +y, -y. Index doubles as the direction id used
/// for the turn penalty.
const STEPS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

impl AStarSearch {
    fn new(cells: usize) -> Self {
        Self {
            g_score: vec![0; cells],
            came_from: vec![-1; cells],
            came_dir: vec![u8::MAX; cells],
            generation: vec![0; cells],
            current_gen: 0,
            heap: BinaryHeap::new(),
        }
    }

    fn find(
        &mut self,
        grid: &RouteGrid,
        start: (i32, i32),
        end: (i32, i32),
    ) -> Option<Vec<(i32, i32)>> {
        let start_i = grid.index(start.0, start.1)?;
        let end_i = grid.index(end.0, end.1)?;

        self.current_gen += 1;
        let generation_id = self.current_gen;
        self.heap.clear();

        let heuristic = |c: i32, r: i32| (c - end.0).abs() + (r - end.1).abs();

        self.g_score[start_i] = 0;
        self.came_from[start_i] = -1;
        self.came_dir[start_i] = u8::MAX;
        self.generation[start_i] = generation_id;
        self.heap
            .push(Reverse((heuristic(start.0, start.1), start_i)));

        while let Some(Reverse((f, ci))) = self.heap.pop() {
            let c = (ci as i32) % grid.cols;
            let r = (ci as i32) / grid.cols;

            // Lazy deletion: skip stale heap entries.
            if f > self.g_score[ci] + heuristic(c, r) {
                continue;
            }
            if ci == end_i {
                return Some(self.reconstruct(grid, end_i));
            }

            for (dir, (dc, dr)) in STEPS.iter().enumerate() {
                let (nc, nr) = (c + dc, r + dr);
                if !grid.walkable(nc, nr) {
                    continue;
                }
                let ni = (nr * grid.cols + nc) as usize;

                let turn = if self.came_dir[ci] != u8::MAX && self.came_dir[ci] != dir as u8 {
                    ROUTE_TURN_PENALTY
                } else {
                    0
                };
                let tentative = self.g_score[ci] + 1 + turn;

                if self.generation[ni] != generation_id || tentative < self.g_score[ni] {
                    self.g_score[ni] = tentative;
                    self.came_from[ni] = ci as i32;
                    self.came_dir[ni] = dir as u8;
                    self.generation[ni] = generation_id;
                    self.heap.push(Reverse((tentative + heuristic(nc, nr), ni)));
                }
            }
        }

        None
    }

    fn reconstruct(&self, grid: &RouteGrid, end_i: usize) -> Vec<(i32, i32)> {
        let mut path = Vec::new();
        let mut i = end_i as i32;
        while i >= 0 {
            let ui = i as usize;
            path.push((ui as i32 % grid.cols, ui as i32 / grid.cols));
            i = self.came_from[ui];
        }
        path.reverse();
        path
    }
}

/// Collapse runs of cells travelling in the same direction down to their
/// turning points, so the path has a vertex only where it actually bends.
fn compress_collinear(cells: &[(i32, i32)]) -> Vec<(i32, i32)> {
    if cells.len() <= 2 {
        return cells.to_vec();
    }
    let mut out = vec![cells[0]];
    for window in cells.windows(3) {
        let (a, b, c) = (window[0], window[1], window[2]);
        let d1 = (b.0 - a.0, b.1 - a.1);
        let d2 = (c.0 - b.0, c.1 - b.1);
        if d1 != d2 {
            out.push(b);
        }
    }
    out.push(*cells.last().unwrap());
    out
}

/// Snap the straight run at one end of a routed path onto its anchor's
/// perpendicular axis, so the edge meets the table border square-on. The first
/// turn stays orthogonal because every point in the run shifts by the same
/// amount on the same axis (mirrors react-flow-smart-edge's `alignEndpoints`).
fn align_endpoint(points: &mut [Point<f32>], anchor: Point<f32>, side: AnchorSide, leading: bool) {
    // Need at least the anchor plus two interior points to have a run to align.
    if points.len() < 4 {
        return;
    }
    let horizontal = matches!(side, AnchorSide::Left | AnchorSide::Right);

    // Interior points run from index 1 to len-2 (0 and len-1 are the anchors).
    let run_start = if leading { 1 } else { points.len() - 2 };
    let key = if horizontal {
        points[run_start].y
    } else {
        points[run_start].x
    };

    let mut i = run_start as isize;
    let limit = if leading {
        points.len() as isize - 1
    } else {
        0
    };
    let step = if leading { 1 } else { -1 };
    while i != limit {
        let p = &mut points[i as usize];
        let v = if horizontal { p.y } else { p.x };
        if (v - key).abs() > 0.5 {
            break;
        }
        if horizontal {
            p.y = anchor.y;
        } else {
            p.x = anchor.x;
        }
        i += step;
    }
}

/// Drop coincident points so corner rounding never sees a zero-length segment
/// (which would produce NaN directions).
fn dedup_points(pts: &mut Vec<Point<f32>>) {
    pts.dedup_by(|a, b| (a.x - b.x).abs() < 0.5 && (a.y - b.y).abs() < 0.5);
}

/// Stroke a poly-line with rounded corners. Each interior vertex is replaced by
/// a quadratic curve whose radius is clamped to half of the shorter adjoining
/// segment so tight elbows stay sane.
fn build_rounded_path(
    pts: &[Point<Pixels>],
    radius: f32,
    width: Pixels,
) -> Option<gpui::Path<Pixels>> {
    if pts.len() < 2 {
        return None;
    }

    let distance = |a: Point<Pixels>, b: Point<Pixels>| {
        let dx = (a.x - b.x).as_f32();
        let dy = (a.y - b.y).as_f32();
        (dx * dx + dy * dy).sqrt()
    };
    let lerp = |from: Point<Pixels>, to: Point<Pixels>, t: f32| {
        point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t)
    };

    let mut builder = PathBuilder::stroke(width);
    builder.move_to(pts[0]);

    for i in 1..pts.len() - 1 {
        let (a, v, b) = (pts[i - 1], pts[i], pts[i + 1]);
        let len_av = distance(v, a);
        let len_vb = distance(v, b);
        let r = radius.min(len_av / 2.0).min(len_vb / 2.0);

        if r < 0.5 {
            builder.line_to(v);
            continue;
        }

        builder.line_to(lerp(v, a, r / len_av));
        builder.curve_to(lerp(v, b, r / len_vb), v);
    }

    builder.line_to(pts[pts.len() - 1]);
    builder.build().ok()
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
    fn route_avoids_a_blocking_node() {
        // An obstacle sits directly between source and target. A* must thread
        // the route around it: no interior vertex may land inside the obstacle's
        // padded rectangle.
        let mut nodes = vec![node("src"), node("dst"), node("block")];
        nodes[0].position = Point::new(0.0, 100.0);
        nodes[0].size = Size {
            width: 80.0,
            height: 40.0,
        };
        nodes[1].position = Point::new(400.0, 100.0);
        nodes[1].size = Size {
            width: 80.0,
            height: 40.0,
        };
        nodes[2].position = Point::new(180.0, 60.0);
        nodes[2].size = Size {
            width: 80.0,
            height: 120.0,
        };

        let grid = RouteGrid::build(&nodes).expect("grid");
        let mut search = AStarSearch::new(grid.cell_count());

        let from = Point::new(80.0, 120.0); // right edge of src
        let to = Point::new(400.0, 120.0); // left edge of dst
        let path = grid
            .route(from, AnchorSide::Right, to, AnchorSide::Left, &mut search)
            .expect("a path should exist around the obstacle");

        // The padded obstacle rectangle.
        let pad = ROUTE_NODE_PADDING;
        let (bx0, by0) = (180.0 - pad, 60.0 - pad);
        let (bx1, by1) = (180.0 + 80.0 + pad, 60.0 + 120.0 + pad);
        for p in &path {
            let inside = p.x > bx0 && p.x < bx1 && p.y > by0 && p.y < by1;
            assert!(!inside, "routed point {p:?} fell inside the obstacle");
        }
    }

    #[test]
    fn intervening_column_does_not_change_layer_order() {
        // Sanity: a chain a->b->c lays out c left of b left of a, so the edge
        // a->c spans an intervening column (b) and would be flagged for
        // perimeter routing at render time.
        let mut nodes = vec![node("a"), node("b"), node("c")];
        let edges = vec![edge("a", "b"), edge("b", "c")];
        compute_node_sizes(&mut nodes);
        layout_dag(&mut nodes, &edges);

        let (ax, bx, cx) = (
            position_of(&nodes, "a").x,
            position_of(&nodes, "b").x,
            position_of(&nodes, "c").x,
        );
        assert!(cx < bx && bx < ax, "b's column sits between a and c");
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
