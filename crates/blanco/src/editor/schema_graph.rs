use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement, Render,
    Styled, Window,
};
use gpui_component::v_flex;

use app_database::VirtualForeignKey;
use blanco_core::ColumnInfo;
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
    #[cfg(feature = "screenshots")]
    pub(crate) fn show_all_edges(&mut self, cx: &mut Context<Self>) {
        self.graph_view
            .update(cx, |view, cx| view.set_show_all_edges(true, cx));
    }

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
        virtual_keys: &[VirtualForeignKey],
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = build_graph_model(tables, virtual_keys);
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

/// The graph of `tables` with an edge per declared foreign key column, and
/// a dashed one per column of a virtual key the server does not declare.
fn build_graph_model(
    tables: Vec<TableSchemaInfo>,
    virtual_keys: &[VirtualForeignKey],
) -> GraphModel {
    // Column pairs of the virtual keys that reach a table in this graph,
    // keyed by the referencing table and column.
    let virtual_references: std::collections::HashMap<(&str, &str), (&str, &str)> = virtual_keys
        .iter()
        .filter(|key| key.referenced_schema.is_none())
        .flat_map(|key| {
            key.columns
                .iter()
                .zip(&key.referenced_columns)
                .map(move |(column, referenced)| {
                    (
                        (key.table_name.as_str(), column.as_str()),
                        (key.referenced_table.as_str(), referenced.as_str()),
                    )
                })
        })
        .collect();
    let virtual_reference = |table: &str, column: &ColumnInfo| {
        column
            .foreign_key
            .is_none()
            .then(|| virtual_references.get(&(table, column.name.as_str())))
            .flatten()
            .copied()
    };

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
                    if col.foreign_key.is_some() || virtual_reference(&table.name, col).is_some() {
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
        let Some(&from_index) = node_name_to_index.get(&table.name) else {
            continue;
        };
        for col in &table.columns {
            let (to_table, to_column, dashed) =
                match (&col.foreign_key, virtual_reference(&table.name, col)) {
                    (Some(fk), _) => (
                        fk.foreign_table_name.as_str(),
                        fk.foreign_column_name.as_str(),
                        false,
                    ),
                    (None, Some((to_table, to_column))) => (to_table, to_column, true),
                    (None, None) => continue,
                };
            let Some(&to_index) = node_name_to_index.get(to_table) else {
                continue;
            };

            let from_field_index = nodes[from_index]
                .fields
                .iter()
                .position(|f| f.label == col.name)
                .unwrap_or(0);

            let to_field_index = nodes[to_index]
                .fields
                .iter()
                .position(|f| f.label == to_column)
                .unwrap_or(0);

            edges.push(GraphEdge {
                from_node: table.name.clone(),
                from_field_index,
                to_node: to_table.to_string(),
                to_field_index,
                dashed,
            });
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

#[cfg(test)]
mod tests {
    use super::build_graph_model;
    use app_database::VirtualForeignKey;
    use blanco_core::connection_trait::TableSchemaInfo;
    use blanco_core::{ColumnInfo, ForeignKeyInfo};

    fn column(name: &str, foreign_key: Option<(&str, &str)>) -> ColumnInfo {
        ColumnInfo {
            name: name.to_string(),
            data_type: "integer".to_string(),
            is_nullable: true,
            is_primary_key: name == "id",
            default_value: None,
            character_maximum_length: None,
            foreign_key: foreign_key.map(|(table, column)| ForeignKeyInfo {
                foreign_table_name: table.to_string(),
                foreign_column_name: column.to_string(),
                constraint_name: None,
            }),
            comment: None,
            numeric_precision: None,
            numeric_scale: None,
            enum_labels: Vec::new(),
        }
    }

    fn table(name: &str, columns: Vec<ColumnInfo>) -> TableSchemaInfo {
        TableSchemaInfo {
            name: name.to_string(),
            schema: String::new(),
            object_type: "table".to_string(),
            column_count: columns.len(),
            columns,
            referenced_by: Vec::new(),
        }
    }

    #[test]
    fn virtual_keys_add_dashed_edges_where_none_is_declared() {
        let tables = vec![
            table("customers", vec![column("id", None)]),
            table("regions", vec![column("id", None)]),
            table(
                "orders",
                vec![
                    column("id", None),
                    column("customer_id", None),
                    column("region_id", Some(("regions", "id"))),
                ],
            ),
        ];
        let virtual_key = |column: &str, referenced_table: &str| VirtualForeignKey {
            id: 1,
            connection_id: 1,
            database_name: "main".to_string(),
            schema_name: None,
            table_name: "orders".to_string(),
            name: format!("{column}_key"),
            columns: vec![column.to_string()],
            referenced_schema: None,
            referenced_table: referenced_table.to_string(),
            referenced_columns: vec!["id".to_string()],
        };
        // The declared key on region_id wins over a virtual one.
        let keys = [
            virtual_key("customer_id", "customers"),
            virtual_key("region_id", "customers"),
        ];
        let model = build_graph_model(tables, &keys);
        let mut edges: Vec<(&str, &str, bool)> = model
            .edges
            .iter()
            .map(|edge| (edge.from_node.as_str(), edge.to_node.as_str(), edge.dashed))
            .collect();
        edges.sort();
        assert_eq!(
            edges,
            [("orders", "customers", true), ("orders", "regions", false)]
        );
        let orders = model
            .nodes
            .iter()
            .find(|node| node.id == "orders")
            .expect("orders node");
        assert!(
            orders.fields[1]
                .badges
                .contains(&blanco_ui::graph_view::FieldBadge::ForeignKey)
        );
    }
}
