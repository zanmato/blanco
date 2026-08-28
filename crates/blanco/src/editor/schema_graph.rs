use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement, Render,
    Styled, Window,
};
use gpui_component::v_flex;

use blanco_core::ConnectionContext;
use blanco_core::connection_trait::TableSchemaInfo;
use blanco_ui::graph_view::{
    FieldBadge, GraphEdge, GraphModel, GraphNode, GraphNodeField, GraphView, compute_node_sizes,
    layout_dag,
};

#[derive(Clone)]
pub struct SchemaGraphParams {
    pub context: ConnectionContext,
}

pub struct SchemaGraphTab {
    pub title: String,
    pub context: ConnectionContext,
    graph_view: Entity<GraphView>,
    focus_handle: FocusHandle,
}

impl SchemaGraphTab {
    pub fn new(context: ConnectionContext, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let graph_view = cx.new(|cx| {
            let mut view = GraphView::new(cx);
            view.loading = true;
            view
        });

        let title = match &context.schema_name {
            Some(schema) => format!("Schema: {}", schema),
            None => format!("Schema: {}", context.database_name),
        };

        Self {
            title,
            context,
            graph_view,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn update_with_schema(
        &mut self,
        tables: Vec<TableSchemaInfo>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = build_graph_model(tables);
        self.graph_view.update(cx, |view, cx| {
            view.loading = false;
            if model.nodes.is_empty() {
                view.error = Some("No tables found in schema".to_string());
            } else {
                view.set_model(model, cx);
            }
        });
    }

    pub fn set_error(&mut self, error: String, _window: &mut Window, cx: &mut Context<Self>) {
        self.graph_view.update(cx, |view, cx| {
            view.loading = false;
            view.error = Some(error);
            cx.notify();
        });
    }
}

fn build_graph_model(tables: Vec<TableSchemaInfo>) -> GraphModel {
    let mut nodes: Vec<GraphNode> = tables
        .iter()
        .map(|table| {
            let fields: Vec<GraphNodeField> = table
                .columns
                .iter()
                .map(|col| {
                    let mut badges = Vec::new();
                    if col.is_primary_key {
                        badges.push(FieldBadge::PrimaryKey);
                    }
                    if col.foreign_key.is_some() {
                        badges.push(FieldBadge::ForeignKey);
                    }
                    GraphNodeField {
                        label: col.name.clone().into(),
                        type_label: col.data_type.clone().into(),
                        badges,
                    }
                })
                .collect();

            GraphNode {
                id: table.name.clone(),
                title: table.name.clone().into(),
                fields,
                position: gpui::Point::default(),
                size: gpui::Size::default(),
            }
        })
        .collect();

    let node_name_to_index: std::collections::HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.clone(), i))
        .collect();

    let mut edges: Vec<GraphEdge> = Vec::new();
    for table in &tables {
        for col in &table.columns {
            if let Some(fk) = &col.foreign_key {
                let from_index = match node_name_to_index.get(&table.name) {
                    Some(&ix) => ix,
                    None => continue,
                };
                let to_index = match node_name_to_index.get(&fk.foreign_table_name) {
                    Some(&ix) => ix,
                    None => continue,
                };

                let from_field_index = nodes[from_index]
                    .fields
                    .iter()
                    .position(|f| f.label == col.name)
                    .unwrap_or(0);

                let to_field_index = nodes[to_index]
                    .fields
                    .iter()
                    .position(|f| f.label == fk.foreign_column_name)
                    .unwrap_or(0);

                edges.push(GraphEdge {
                    from_node: table.name.clone(),
                    from_field_index,
                    to_node: fk.foreign_table_name.clone(),
                    to_field_index,
                });
            }
        }
    }

    compute_node_sizes(&mut nodes);
    layout_dag(&mut nodes, &edges);

    GraphModel { nodes, edges }
}

impl Focusable for SchemaGraphTab {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SchemaGraphTab {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .flex_1()
            .h_full()
            .w_full()
            .overflow_hidden()
            // No background: the editor card owns this surface, and a square
            // fill here would cover its rounded corners even in the same colour.
            .child(self.graph_view.clone())
    }
}
