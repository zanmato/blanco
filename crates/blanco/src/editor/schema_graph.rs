use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement, Render,
    SharedString, Styled, Subscription, Task, Window,
};
use gpui_component::{WindowExt as _, notification::NotificationType, v_flex};

use std::collections::{BTreeSet, HashMap};

use app_database::{AppDatabase, VirtualForeignKey};
use blanco_core::ConnectionContext;
use blanco_core::connection_trait::{Connection, ForeignKeyConstraint, SchemaColumn};
use blanco_ui::graph_view::{
    FieldBadge, GraphEdge, GraphModel, GraphNode, GraphNodeField, GraphView, GraphViewEvent,
    compute_node_sizes, layout_dag,
};

use crate::result_ext::ResultExt as _;

#[derive(Clone)]
pub struct SchemaGraphParams {
    pub context: ConnectionContext,
}

pub struct SchemaGraphTab {
    pub title: String,
    pub context: ConnectionContext,
    graph_view: Entity<GraphView>,
    focus_handle: FocusHandle,
    load: Option<Task<()>>,
    _subscription: Subscription,
}

/// Where loading a graph is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LoadStep {
    Connecting,
    ForeignKeys,
    Columns { tables_elsewhere: usize },
    Layout { tables: usize },
}

impl LoadStep {
    fn label(self) -> SharedString {
        match self {
            Self::Connecting => "Connecting…".into(),
            Self::ForeignKeys => "Reading foreign keys…".into(),
            Self::Columns {
                tables_elsewhere: 0,
            } => "Reading columns…".into(),
            Self::Columns { tables_elsewhere } => format!(
                "Reading columns, with {tables_elsewhere} table{} of other schemas…",
                if tables_elsewhere == 1 { "" } else { "s" }
            )
            .into(),
            Self::Layout { tables } => format!("Laying out {tables} tables…").into(),
        }
    }

    /// How far along loading is at the start of this step, in percent.
    fn percent(self) -> f32 {
        match self {
            Self::Connecting => 5.0,
            Self::ForeignKeys => 25.0,
            Self::Columns { .. } => 50.0,
            Self::Layout { .. } => 85.0,
        }
    }
}

impl SchemaGraphTab {
    #[cfg(feature = "screenshots")]
    pub(crate) fn show_all_edges(&mut self, cx: &mut Context<Self>) {
        self.graph_view
            .update(cx, |view, cx| view.set_show_all_edges(true, cx));
    }

    pub fn new(context: ConnectionContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let graph_view = cx.new(|cx| GraphView::new(window, cx));
        let subscription = cx.subscribe_in(
            &graph_view,
            window,
            |this, _, event: &GraphViewEvent, window, cx| match event {
                GraphViewEvent::ExportSvg => this.export_svg(window, cx),
            },
        );

        let title = match &context.schema_name {
            Some(schema) => format!("Schema: {}", schema),
            None => format!("Schema: {}", context.database_name),
        };

        let mut tab = Self {
            title,
            context,
            graph_view,
            focus_handle: cx.focus_handle(),
            load: None,
            _subscription: subscription,
        };
        tab.load(cx);
        tab
    }

    /// Ask where to save the graph as SVG and write it there.
    fn export_svg(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let document = self.graph_view.read(cx).export_svg(cx);
        let file_name = export_file_name(&self.context);
        let directory = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        let path = cx.prompt_for_new_path(&directory, Some(&file_name));
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = path.await else {
                return;
            };
            let result = smol::fs::write(&path, document).await;
            cx.update(|window, cx| {
                let note = match result {
                    Ok(()) => (
                        NotificationType::Success,
                        SharedString::from(format!("Saved the graph to {}.", path.display())),
                    ),
                    Err(error) => (
                        NotificationType::Error,
                        SharedString::from(format!("Saving the graph failed: {error}")),
                    ),
                };
                window.push_notification(note, cx);
            })
            .log_err();
        })
        .detach();
    }

    fn show_step(&mut self, step: LoadStep, cx: &mut Context<Self>) {
        self.graph_view.update(cx, |view, cx| {
            view.set_loading(Some((step.label(), step.percent())), cx)
        });
    }

    /// Read the schema and lay it out off the main thread, showing each
    /// step.
    fn load(&mut self, cx: &mut Context<Self>) {
        use database::DatabaseServiceTrait as _;

        let context = self.context.clone();
        let db_service = database::DatabaseService::global(cx).clone();
        let app_database = AppDatabase::global(cx).clone();
        self.show_step(LoadStep::Connecting, cx);
        self.load = Some(cx.spawn(async move |this, cx| {
            let virtual_keys = crate::virtual_keys::load(
                &app_database,
                context.connection_id,
                &context.database_name,
                context.schema_name.as_deref(),
            )
            .await;
            let tables = match db_service
                .get_or_create_connection_by_id(context.connection_id, Some(&context.database_name))
                .await
            {
                Ok(connection) => {
                    load_graph_tables(connection.as_ref(), &context, &virtual_keys, &mut |step| {
                        this.update(cx, |tab, cx| tab.show_step(step, cx)).ok();
                    })
                    .await
                    .map_err(|error| format!("Failed to read the schema: {error:#}"))
                }
                Err(error) => Err(format!("Failed to connect: {error}")),
            };
            let model = match tables {
                Ok(tables) => {
                    this.update(cx, |tab, cx| {
                        tab.show_step(
                            LoadStep::Layout {
                                tables: tables.len(),
                            },
                            cx,
                        )
                    })
                    .ok();
                    let namespace = graph_namespace(&context);
                    // Laying out a large schema takes a while, which the
                    // main thread must not wait for.
                    Ok(cx
                        .background_spawn(async move {
                            build_graph_model(&tables, &virtual_keys, namespace)
                        })
                        .await)
                }
                Err(error) => Err(error),
            };
            this.update(cx, |tab, cx| tab.show_model(model, cx))
                .log_err();
        }));
    }

    fn show_model(&mut self, model: Result<GraphModel, String>, cx: &mut Context<Self>) {
        self.load = None;
        self.graph_view.update(cx, |view, cx| {
            view.set_loading(None, cx);
            match model {
                Ok(model) if model.nodes.is_empty() => {
                    view.error = Some("No tables found in schema".to_string());
                }
                Ok(model) => view.set_model(model, cx),
                Err(error) => view.error = Some(error),
            }
        });
    }
}

/// A table drawn in the graph.
pub struct GraphTable {
    /// Its schema, or its database where databases are schemas, `None` on
    /// backends with neither.
    pub schema: Option<String>,
    pub name: String,
    pub comment: Option<String>,
    pub columns: Vec<SchemaColumn>,
    pub foreign_keys: Vec<ForeignKeyConstraint>,
    /// A table of another schema, drawn because a foreign key of the
    /// graph's schema references it.
    pub referenced_only: bool,
}

/// The file name an export suggests: the connection, database and schema,
/// since a schema name alone (`public`) says little about where it came
/// from. A part repeating the one before it, as a MySQL database repeats
/// itself as the schema, is left out.
fn export_file_name(context: &ConnectionContext) -> String {
    let mut parts: Vec<String> = Vec::new();
    let names = [
        Some(context.connection_name.as_str()),
        Some(context.database_name.as_str()),
        context.schema_name.as_deref(),
    ];
    for name in names.into_iter().flatten() {
        let part = file_name_part(name);
        if part.is_empty()
            || parts
                .last()
                .is_some_and(|last| last.eq_ignore_ascii_case(&part))
        {
            continue;
        }
        parts.push(part);
    }
    if parts.is_empty() {
        parts.push("schema".to_string());
    }
    format!("{}.svg", parts.join("-"))
}

/// A connection, database or schema name made safe for a file name.
fn file_name_part(name: &str) -> String {
    let mut cleaned = String::new();
    for character in name.chars() {
        if character.is_alphanumeric() || matches!(character, '-' | '.') {
            cleaned.push(character);
        } else if !cleaned.ends_with('_') {
            cleaned.push('_');
        }
    }
    cleaned.trim_matches(['.', '_']).to_string()
}

/// The schema the graph shows, or its database where databases are schemas.
/// `None` on backends with neither, where every table is in one namespace.
pub fn graph_namespace(context: &ConnectionContext) -> Option<String> {
    let dialect = context.db_type.dialect();
    context
        .schema_name
        .clone()
        .or_else(|| dialect.default_schema().map(str::to_string))
        .or_else(|| {
            dialect
                .databases_are_schemas()
                .then(|| context.database_name.clone())
        })
}

/// The tables of the context's schema with their columns and foreign keys,
/// followed by the tables of other schemas that those keys, or the virtual
/// keys, reference. Two queries read them: the schema's foreign keys, then
/// the columns of its tables and of the tables they reference.
pub async fn load_graph_tables(
    connection: &dyn Connection,
    context: &ConnectionContext,
    virtual_keys: &[VirtualForeignKey],
    on_step: &mut dyn FnMut(LoadStep),
) -> anyhow::Result<Vec<GraphTable>> {
    let schema = context.schema_name.as_deref();
    let namespace = graph_namespace(context);

    on_step(LoadStep::ForeignKeys);
    let foreign_keys = connection.schema_foreign_keys(schema).await?;

    let declared = foreign_keys.iter().map(|key| {
        (
            key.key.referenced_schema.as_ref(),
            &key.key.referenced_table,
        )
    });
    let declared_virtually = virtual_keys
        .iter()
        .map(|key| (key.referenced_schema.as_ref(), &key.referenced_table));
    let elsewhere: BTreeSet<(String, String)> = declared
        .chain(declared_virtually)
        .filter_map(|(schema, table)| {
            let schema = schema.filter(|schema| Some(*schema) != namespace.as_ref())?;
            Some((schema.clone(), table.clone()))
        })
        .collect();
    let elsewhere: Vec<(String, String)> = elsewhere.into_iter().collect();

    on_step(LoadStep::Columns {
        tables_elsewhere: elsewhere.len(),
    });
    let columns = connection.schema_columns(schema, &elsewhere).await?;

    let mut keys_by_table: HashMap<String, Vec<ForeignKeyConstraint>> = HashMap::new();
    for key in foreign_keys {
        keys_by_table.entry(key.table).or_default().push(key.key);
    }
    Ok(columns
        .into_iter()
        .map(|table| {
            let referenced_only = table.schema.is_some();
            GraphTable {
                foreign_keys: if referenced_only {
                    Vec::new()
                } else {
                    keys_by_table.remove(&table.table).unwrap_or_default()
                },
                schema: table.schema.or_else(|| namespace.clone()),
                name: table.table,
                comment: table.comment,
                columns: table.columns,
                referenced_only,
            }
        })
        .collect())
}

/// A node's id, its table qualified by its schema, so tables of the same
/// name in two schemas stay apart.
fn node_id(schema: Option<&str>, table: &str) -> String {
    match schema {
        Some(schema) => format!("{schema}.{table}"),
        None => table.to_string(),
    }
}

/// One column pair of a foreign key, from a table of the graph.
struct Reference<'a> {
    column: &'a str,
    to_schema: Option<&'a str>,
    to_table: &'a str,
    to_column: &'a str,
    dashed: bool,
}

/// The graph of `tables` with an edge per column pair of each declared
/// foreign key, and a dashed one per pair of a virtual key on a column no
/// declared key holds. `namespace` is the schema the virtual keys were
/// declared in, which a key without a referenced schema references.
fn build_graph_model(
    tables: &[GraphTable],
    virtual_keys: &[VirtualForeignKey],
    namespace: Option<String>,
) -> GraphModel {
    let references: Vec<Vec<Reference>> = tables
        .iter()
        .map(|table| references(table, virtual_keys, namespace.as_deref()))
        .collect();

    let mut nodes: Vec<GraphNode> = tables
        .iter()
        .zip(&references)
        .map(|(table, references)| {
            let fields: Vec<GraphNodeField> = table
                .columns
                .iter()
                .map(|column| {
                    let mut badges = Vec::new();
                    if column.is_primary_key {
                        badges.push(FieldBadge::PrimaryKey);
                    }
                    if references
                        .iter()
                        .any(|reference| reference.column == column.name)
                    {
                        badges.push(FieldBadge::ForeignKey);
                    }
                    GraphNodeField {
                        label: column.name.clone().into(),
                        type_label: column.data_type.clone().into(),
                        badges,
                        comment: column.comment.clone().map(Into::into),
                    }
                })
                .collect();
            let id = node_id(table.schema.as_deref(), &table.name);
            // A table of another schema is named with its schema, since
            // the graph is titled with its own.
            let title = if table.referenced_only {
                id.clone()
            } else {
                table.name.clone()
            };
            GraphNode {
                id,
                title: title.into(),
                comment: table.comment.clone().map(Into::into),
                schema: table.schema.clone().map(Into::into),
                referenced_only: table.referenced_only,
                fields,
                position: gpui::Point::default(),
                size: gpui::Size::default(),
            }
        })
        .collect();

    let node_index: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(ix, node)| (node.id.clone(), ix))
        .collect();
    let field_index = |node_ix: usize, column: &str| {
        nodes
            .get(node_ix)
            .and_then(|node| node.fields.iter().position(|field| field.label == column))
            .unwrap_or(0)
    };

    let mut edges: Vec<GraphEdge> = Vec::new();
    for (table, references) in tables.iter().zip(&references) {
        let from_node = node_id(table.schema.as_deref(), &table.name);
        let Some(&from_ix) = node_index.get(&from_node) else {
            continue;
        };
        for reference in references {
            let to_node = node_id(reference.to_schema, reference.to_table);
            let Some(&to_ix) = node_index.get(&to_node) else {
                continue;
            };
            edges.push(GraphEdge {
                from_node: from_node.clone(),
                from_field_index: field_index(from_ix, reference.column),
                to_node,
                to_field_index: field_index(to_ix, reference.to_column),
                dashed: reference.dashed,
            });
        }
    }

    compute_node_sizes(&mut nodes);
    layout_dag(&mut nodes, &edges);

    GraphModel { nodes, edges }
}

/// The column pairs of a table's foreign keys: the declared ones, then
/// those of its virtual keys on columns no declared key holds.
fn references<'a>(
    table: &'a GraphTable,
    virtual_keys: &'a [VirtualForeignKey],
    namespace: Option<&'a str>,
) -> Vec<Reference<'a>> {
    let mut references: Vec<Reference> = table
        .foreign_keys
        .iter()
        .flat_map(|key| {
            let to_schema = key.referenced_schema.as_deref().or(table.schema.as_deref());
            key.columns
                .iter()
                .zip(&key.referenced_columns)
                .map(move |(column, to_column)| Reference {
                    column,
                    to_schema,
                    to_table: &key.referenced_table,
                    to_column,
                    dashed: false,
                })
        })
        .collect();
    // Virtual keys are loaded for the graph's schema, so they belong to its
    // own tables only.
    if table.referenced_only {
        return references;
    }
    for key in virtual_keys
        .iter()
        .filter(|key| key.table_name == table.name)
    {
        let to_schema = key.referenced_schema.as_deref().or(namespace);
        for (column, to_column) in key.columns.iter().zip(&key.referenced_columns) {
            if references
                .iter()
                .any(|reference| reference.column == column)
            {
                continue;
            }
            references.push(Reference {
                column,
                to_schema,
                to_table: &key.referenced_table,
                to_column,
                dashed: true,
            });
        }
    }
    references
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
    use super::{
        GraphTable, build_graph_model, export_file_name, graph_namespace, load_graph_tables,
    };
    use app_database::VirtualForeignKey;
    use blanco_core::ConnectionContext;
    use blanco_core::connection_trait::{ForeignKeyConstraint, SchemaColumn};
    use blanco_ui::graph_view::{FieldBadge, GraphModel};

    fn column(name: &str) -> SchemaColumn {
        SchemaColumn {
            name: name.to_string(),
            data_type: "integer".to_string(),
            is_primary_key: name == "id",
            comment: None,
        }
    }

    fn key(column: &str, schema: Option<&str>, table: &str) -> ForeignKeyConstraint {
        ForeignKeyConstraint {
            name: format!("{column}_fkey"),
            columns: vec![column.to_string()],
            referenced_schema: schema.map(str::to_string),
            referenced_table: table.to_string(),
            referenced_columns: vec!["id".to_string()],
            on_update: "NO ACTION".to_string(),
            on_delete: "NO ACTION".to_string(),
        }
    }

    fn table(
        schema: Option<&str>,
        name: &str,
        columns: &[&str],
        foreign_keys: Vec<ForeignKeyConstraint>,
    ) -> GraphTable {
        GraphTable {
            schema: schema.map(str::to_string),
            name: name.to_string(),
            comment: None,
            columns: columns.iter().map(|name| column(name)).collect(),
            foreign_keys,
            referenced_only: false,
        }
    }

    fn edges(model: &GraphModel) -> Vec<(&str, &str, bool)> {
        let mut edges: Vec<_> = model
            .edges
            .iter()
            .map(|edge| (edge.from_node.as_str(), edge.to_node.as_str(), edge.dashed))
            .collect();
        edges.sort();
        edges
    }

    #[test]
    fn the_export_is_named_after_connection_database_and_schema() {
        let context = |connection: &str, database: &str, schema: Option<&str>| ConnectionContext {
            connection_name: connection.to_string(),
            database_name: database.to_string(),
            schema_name: schema.map(str::to_string),
            connection_id: 1,
            db_type: blanco_core::DatabaseType::PostgreSQL,
            environment_type: None,
        };
        assert_eq!(
            export_file_name(&context("Prod", "shop", Some("public"))),
            "Prod-shop-public.svg"
        );
        // MySQL's database is its schema too.
        assert_eq!(
            export_file_name(&context("Local", "shop", Some("shop"))),
            "Local-shop.svg"
        );
        assert_eq!(
            export_file_name(&context("Staging / EU", "app db", None)),
            "Staging_EU-app_db.svg"
        );
        assert_eq!(export_file_name(&context("..", "", None)), "schema.svg");
    }

    #[test]
    fn comments_reach_the_nodes_and_their_rows() {
        let mut orders = table(None, "orders", &["id", "placed_at"], Vec::new());
        orders.comment = Some("One per checkout".to_string());
        if let Some(column) = orders.columns.get_mut(1) {
            column.comment = Some("When it was paid".to_string());
        }
        let plain = table(None, "notes", &["id"], Vec::new());
        let model = build_graph_model(&[orders, plain], &[], None);
        let node = |id: &str| model.nodes.iter().find(|node| node.id == id);
        let orders = node("orders").expect("orders is drawn");
        let notes = node("notes").expect("notes is drawn");
        assert_eq!(orders.comment.as_deref(), Some("One per checkout"));
        let comments: Vec<Option<&str>> = orders
            .fields
            .iter()
            .map(|field| field.comment.as_deref())
            .collect();
        assert_eq!(comments, [None, Some("When it was paid")]);
        assert_eq!(notes.comment, None);
        // The subtitle makes room for itself, so routes still pass around it.
        assert!(orders.size.height > notes.size.height + 22.0);
    }

    #[test]
    fn virtual_keys_add_dashed_edges_where_none_is_declared() {
        let tables = vec![
            table(None, "customers", &["id"], Vec::new()),
            table(None, "regions", &["id"], Vec::new()),
            table(
                None,
                "orders",
                &["id", "customer_id", "region_id"],
                vec![key("region_id", None, "regions")],
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
        let model = build_graph_model(&tables, &keys, None);
        assert_eq!(
            edges(&model),
            [("orders", "customers", true), ("orders", "regions", false)]
        );
        let orders = model
            .nodes
            .iter()
            .find(|node| node.id == "orders")
            .expect("orders node");
        assert!(orders.fields[1].badges.contains(&FieldBadge::ForeignKey));
    }

    /// A key into another schema reaches the table of that schema, not the
    /// one of the same name in the graph's own.
    #[test]
    fn keys_into_other_schemas_reach_their_own_table() {
        let mut customers_elsewhere = table(Some("crm"), "customers", &["id"], Vec::new());
        customers_elsewhere.referenced_only = true;
        let tables = vec![
            table(Some("sales"), "customers", &["id"], Vec::new()),
            table(
                Some("sales"),
                "orders",
                &["id", "customer_id", "buyer_id"],
                vec![
                    key("customer_id", Some("crm"), "customers"),
                    // The backend names the graph's own schema too.
                    key("buyer_id", Some("sales"), "customers"),
                ],
            ),
            customers_elsewhere,
        ];
        let model = build_graph_model(&tables, &[], Some("sales".to_string()));
        assert_eq!(
            edges(&model),
            [
                ("sales.orders", "crm.customers", false),
                ("sales.orders", "sales.customers", false),
            ]
        );
        let titles: Vec<(&str, bool)> = model
            .nodes
            .iter()
            .map(|node| (node.title.as_ref(), node.referenced_only))
            .collect();
        assert_eq!(
            titles,
            [
                ("customers", false),
                ("orders", false),
                ("crm.customers", true)
            ]
        );
    }

    /// Keys into another schema on the compose servers bring that schema's
    /// table along, and a table of the same name in the graph's own schema
    /// stays apart. MySQL names the referenced database even for a table
    /// of its own, which must still count as the graph's own.
    #[gpui::test]
    async fn the_graph_follows_keys_into_other_schemas(cx: &mut gpui::TestAppContext) {
        use blanco_core::DatabaseService as _;
        use database::DatabaseType;

        use crate::test_harness::{LiveServer, TestHarness};

        let harness = TestHarness::new(cx);
        let mut cx = gpui::VisualTestContext::from_window(harness.window_handle.into(), cx);
        cx.executor().allow_parking();

        let schemas = r#"DROP SCHEMA IF EXISTS graph_sales CASCADE;
            DROP SCHEMA IF EXISTS graph_crm CASCADE;
            CREATE SCHEMA graph_crm;
            CREATE SCHEMA graph_sales;
            CREATE TABLE graph_crm.customers (id integer PRIMARY KEY, name text);
            CREATE TABLE graph_sales.customers (id integer PRIMARY KEY);
            CREATE TABLE graph_sales.orders (
                id integer PRIMARY KEY,
                customer_id integer REFERENCES graph_crm.customers (id),
                buyer_id integer REFERENCES graph_sales.customers (id)
            );"#;
        let databases = "DROP TABLE IF EXISTS graph_orders;
            DROP TABLE IF EXISTS graph_customers;
            CREATE TABLE graph_customers (id integer PRIMARY KEY);
            CREATE TABLE graph_orders (
                id integer PRIMARY KEY,
                customer_id integer,
                FOREIGN KEY (customer_id) REFERENCES graph_customers (id)
            );";
        let drop_mssql = "DROP TABLE IF EXISTS graph_sales.orders;
            DROP TABLE IF EXISTS graph_sales.customers;
            DROP TABLE IF EXISTS graph_crm.customers;
            DROP SCHEMA IF EXISTS graph_sales;
            DROP SCHEMA IF EXISTS graph_crm;";
        // CREATE SCHEMA has to start a batch of its own.
        let mssql_schemas = format!(
            "{drop_mssql}
            EXEC('CREATE SCHEMA graph_crm');
            EXEC('CREATE SCHEMA graph_sales');
            CREATE TABLE graph_crm.customers (id int PRIMARY KEY, name nvarchar(50));
            CREATE TABLE graph_sales.customers (id int PRIMARY KEY);
            CREATE TABLE graph_sales.orders (
                id int PRIMARY KEY,
                customer_id int REFERENCES graph_crm.customers (id),
                buyer_id int REFERENCES graph_sales.customers (id)
            );"
        );
        let drop_schemas = "DROP SCHEMA graph_sales CASCADE; DROP SCHEMA graph_crm CASCADE;";
        let drop_tables = "DROP TABLE graph_orders; DROP TABLE graph_customers;";
        let cases = [
            (
                DatabaseType::PostgreSQL,
                5488,
                9501,
                schemas,
                drop_schemas,
                Some("graph_sales"),
            ),
            (
                DatabaseType::CockroachDb,
                26258,
                9502,
                schemas,
                drop_schemas,
                Some("graph_sales"),
            ),
            (
                DatabaseType::MsSql,
                1433,
                9504,
                mssql_schemas.as_str(),
                drop_mssql,
                Some("graph_sales"),
            ),
            (
                DatabaseType::MySQL,
                3306,
                9503,
                databases,
                drop_tables,
                None,
            ),
        ];
        for (db_type, port, config_id, setup, teardown, schema) in cases {
            let database = LiveServer::login(db_type).2;
            let Some(server) = LiveServer::connect((db_type, port), config_id, setup, &mut cx)
            else {
                continue;
            };
            let context = ConnectionContext {
                connection_id: config_id,
                db_type,
                connection_name: "it-schemas".into(),
                database_name: database.into(),
                schema_name: schema.map(str::to_string),
                environment_type: None,
            };
            let tables = server
                .handle
                .block_on(async {
                    let connection = server
                        .service
                        .get_or_create_connection_by_id(config_id, Some(database))
                        .await?;
                    load_graph_tables(connection.as_ref(), &context, &[], &mut |_| {}).await
                })
                .unwrap_or_else(|error| panic!("{db_type:?}: {error:#}"));
            let model = build_graph_model(&tables, &[], graph_namespace(&context));
            let mut edges: Vec<(&str, &str)> = model
                .edges
                .iter()
                .filter(|edge| edge.from_node.contains("orders"))
                .map(|edge| (edge.from_node.as_str(), edge.to_node.as_str()))
                .collect();
            edges.sort();

            if schema.is_some() {
                assert_eq!(
                    edges,
                    [
                        ("graph_sales.orders", "graph_crm.customers"),
                        ("graph_sales.orders", "graph_sales.customers"),
                    ],
                    "{db_type:?}"
                );
                let elsewhere = model
                    .nodes
                    .iter()
                    .find(|node| node.id == "graph_crm.customers")
                    .expect("the referenced table is drawn");
                assert!(elsewhere.referenced_only);
                let columns: Vec<&str> = elsewhere
                    .fields
                    .iter()
                    .map(|field| field.label.as_ref())
                    .collect();
                assert_eq!(columns, ["id", "name"], "{db_type:?}");
            } else {
                assert_eq!(
                    edges,
                    [("blanco.graph_orders", "blanco.graph_customers")],
                    "{db_type:?}"
                );
                assert!(model.nodes.iter().all(|node| !node.referenced_only));
            }
            server.script(teardown);
        }
    }
}
