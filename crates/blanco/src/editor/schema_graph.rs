use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement, Render,
    Styled, Window,
};
use gpui_component::{ActiveTheme, v_flex};

use crate::app_database::EnvironmentType;
use blanco_core::connection_trait::TableSchemaInfo;
use blanco_ui::graph_view::{
    FieldBadge, GraphEdge, GraphModel, GraphNode, GraphNodeField, GraphView, compute_node_sizes,
    layout_dag,
};

#[derive(Clone)]
pub struct SchemaGraphParams {
    pub connection_id: i64,
    pub connection_name: String,
    pub db_type: database::DatabaseType,
    pub database_name: String,
    pub schema_name: Option<String>,
    pub environment_type: Option<EnvironmentType>,
}

pub struct SchemaGraphTab {
    pub title: String,
    pub _connection_id: i64,
    pub _db_type: database::DatabaseType,
    pub _connection_name: Option<String>,
    pub _database_name: String,
    pub _schema_name: Option<String>,
    pub _environment_type: Option<EnvironmentType>,
    graph_view: Entity<GraphView>,
    focus_handle: FocusHandle,
}

impl SchemaGraphTab {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        connection_id: i64,
        db_type: database::DatabaseType,
        connection_name: Option<String>,
        database_name: String,
        schema_name: Option<String>,
        environment_type: Option<EnvironmentType>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let graph_view = cx.new(|cx| {
            let mut view = GraphView::new(cx);
            view.loading = true;
            view
        });

        let title = match &schema_name {
            Some(schema) => format!("Schema: {}", schema),
            None => format!("Schema: {}", database_name),
        };

        Self {
            title,
            _connection_id: connection_id,
            _db_type: db_type,
            _connection_name: connection_name,
            _database_name: database_name,
            _schema_name: schema_name,
            _environment_type: environment_type,
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .flex_1()
            .h_full()
            .w_full()
            .overflow_hidden()
            .bg(cx.theme().background)
            .child(self.graph_view.clone())
    }
}
