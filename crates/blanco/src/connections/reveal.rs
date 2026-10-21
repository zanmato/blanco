//! Opening the tree down to an object and selecting it, so the sidebar can
//! follow the active tab, or down to a schema when a connection is opened.

use gpui::{Context, ScrollStrategy};

use super::ConnectionsPanel;
use super::data_loading::{ConnectionLoad, DatabaseItemType};
use super::tree_builder::category_slug;

/// A table, view or routine to select in the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevealTarget {
    pub connection_id: i64,
    pub database_name: String,
    /// The schema on backends with schemas, `public` when unset.
    pub schema_name: Option<String>,
    pub name: String,
}

/// A database, and a schema in it when there is one, to open in the tree
/// without selecting anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NamespaceTarget {
    pub connection_id: i64,
    pub database_name: String,
    pub schema_name: Option<String>,
}

#[derive(Debug, Clone)]
enum RevealGoal {
    Object(RevealTarget),
    Namespace(NamespaceTarget),
}

impl RevealGoal {
    fn connection_id(&self) -> i64 {
        match self {
            RevealGoal::Object(target) => target.connection_id,
            RevealGoal::Namespace(target) => target.connection_id,
        }
    }
}

/// A reveal waiting on the tree to load a level.
#[derive(Debug, Clone)]
pub(super) struct PendingReveal {
    goal: RevealGoal,
    /// The level whose load was requested last. A reveal that needs the same
    /// load again once it has landed gives up, so a failing load cannot loop.
    requested_load: Option<String>,
    /// The requested load has finished. Until then the tree may be rebuilt
    /// for other reasons, such as database sizes arriving, and the reveal
    /// keeps waiting.
    load_landed: bool,
}

/// Where the object sits in the tree once its parents are open.
struct Located {
    item_id: String,
    category_id: String,
    /// The tables the item is nested under as a partition or inheritance
    /// child, which are closed by default.
    ancestor_ids: Vec<String>,
    item_type: DatabaseItemType,
}

impl ConnectionsPanel {
    /// Select `target` in the tree: load and expand its connection, database,
    /// schema and category, and scroll it into view. Levels that are not
    /// loaded yet are loaded first, so the selection may land a moment later.
    /// An object the tree does not have is left alone.
    pub fn reveal(&mut self, target: RevealTarget, cx: &mut Context<Self>) {
        self.pending_reveal = Some(PendingReveal {
            goal: RevealGoal::Object(target),
            requested_load: None,
            load_landed: false,
        });
        self.advance_reveal(cx);
    }

    /// Open the database and schema of `target`, loading them first, the way
    /// a reveal opens the parents of an object. An object reveal already on
    /// its way wins, since it opens its own schema and selects something.
    pub(super) fn open_namespace(&mut self, target: NamespaceTarget, cx: &mut Context<Self>) {
        if self.pending_reveal.is_some() {
            return;
        }
        self.pending_reveal = Some(PendingReveal {
            goal: RevealGoal::Namespace(target),
            requested_load: None,
            load_landed: false,
        });
        self.advance_reveal(cx);
    }

    /// Take the reveal as far as the loaded tree allows.
    pub(super) fn advance_reveal(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_reveal.clone() else {
            return;
        };
        let connection_id = pending.goal.connection_id();

        if !self
            .connections
            .iter()
            .any(|connection| connection.id == Some(connection_id))
        {
            // Before the saved connections are in, wait for them.
            if self.initial_load_completed {
                self.pending_reveal = None;
            }
            return;
        }
        if !self.loaded_connections.contains(&connection_id) {
            let item_id = format!("connection:{connection_id}");
            if self.request_reveal_load(&item_id) {
                self.set_item_loading(&item_id, true, cx);
                self.load_connection_children(connection_id, ConnectionLoad::Expand, cx);
            }
            return;
        }
        self.expanded_connections.insert(connection_id);

        let target = match &pending.goal {
            RevealGoal::Object(target) => target,
            RevealGoal::Namespace(target) => {
                match self.open_levels(
                    connection_id,
                    &target.database_name,
                    target.schema_name.as_deref(),
                ) {
                    Err(Some(load)) => {
                        if self.request_reveal_load(&load.key()) {
                            load.start(self, cx);
                        }
                    }
                    Ok(()) | Err(None) => {
                        self.pending_reveal = None;
                        self.rebuild_tree(cx);
                        cx.notify();
                    }
                }
                return;
            }
        };

        let located = match self.locate(target) {
            Ok(located) => located,
            Err(Some(load)) => {
                if self.request_reveal_load(&load.key()) {
                    load.start(self, cx);
                }
                return;
            }
            Err(None) => {
                self.pending_reveal = None;
                return;
            }
        };

        self.pending_reveal = None;
        // The category set records folders toggled away from their default:
        // Tables open, the rest closed.
        if located.item_type == DatabaseItemType::Table {
            self.expanded_categories.remove(&located.category_id);
        } else {
            self.expanded_categories.insert(located.category_id);
        }
        self.expanded_tables.extend(located.ancestor_ids);
        self.rebuild_tree(cx);
        self.tree_state.update(cx, |state, cx| {
            if let Some(ix) = state.select_id(located.item_id, cx) {
                state.scroll_to_item(ix, ScrollStrategy::Center);
            }
        });
        cx.notify();
    }

    /// Note that `key` is being loaded for the reveal. False when it was the
    /// last load asked for too: the reveal waits while that load runs, and
    /// ends when it landed without the level.
    fn request_reveal_load(&mut self, key: &str) -> bool {
        let Some(pending) = self.pending_reveal.as_mut() else {
            return false;
        };
        if pending.requested_load.as_deref() == Some(key) {
            if pending.load_landed {
                self.pending_reveal = None;
            }
            return false;
        }
        pending.requested_load = Some(key.to_string());
        pending.load_landed = false;
        true
    }

    /// The load of the tree level `key` finished, with or without what it
    /// was meant to bring.
    pub(super) fn note_reveal_load_landed(&mut self, key: &str) {
        if let Some(pending) = self.pending_reveal.as_mut()
            && pending.requested_load.as_deref() == Some(key)
        {
            pending.load_landed = true;
        }
    }

    /// Expand a database and, when given, a schema in it. `Err(Some(load))`
    /// names the level to load first, `Err(None)` means one is not there; a
    /// missing schema leaves its database expanded.
    fn open_levels(
        &mut self,
        connection_id: i64,
        database_name: &str,
        schema_name: Option<&str>,
    ) -> Result<(), Option<RevealLoad>> {
        let metadata = self.database_metadata.get_mut(&connection_id).ok_or(None)?;
        if !metadata.supports_schemas {
            return Err(None);
        }
        let database = metadata
            .databases
            .iter_mut()
            .find(|database| database.name == database_name)
            .ok_or(None)?;
        if database.schemas.is_empty() {
            return Err(Some(RevealLoad::Database {
                connection_id,
                database_name: database.name.clone(),
            }));
        }
        database.is_expanded = true;

        let Some(schema_name) = schema_name else {
            return Ok(());
        };
        let schema = database
            .schemas
            .iter_mut()
            .find(|schema| schema.name == schema_name)
            .ok_or(None)?;
        if schema.tables.is_empty() {
            return Err(Some(RevealLoad::Schema {
                connection_id,
                database_name: database.name.clone(),
                schema_name: schema.name.clone(),
            }));
        }
        schema.is_expanded = true;
        Ok(())
    }

    /// Find the target in the loaded tree, expanding its database and schema
    /// on the way. `Err(Some(load))` names the level to load first,
    /// `Err(None)` means the object is not there.
    fn locate(&mut self, target: &RevealTarget) -> Result<Located, Option<RevealLoad>> {
        let connection_id = target.connection_id;
        let metadata = self.database_metadata.get(&connection_id).ok_or(None)?;

        if !metadata.supports_schemas {
            // Objects of every schema are merged under the connection; prefer
            // the one in the tab's database when names repeat.
            let (schema, table) = metadata
                .schemas
                .iter()
                .flat_map(|schema| schema.tables.iter().map(move |table| (schema, table)))
                .filter(|(_, table)| table.name == target.name)
                .min_by_key(|(schema, _)| schema.name != target.database_name)
                .ok_or(None)?;
            return Ok(Located {
                // Only PostgreSQL nests tables, and it has schemas.
                ancestor_ids: Vec::new(),
                item_id: format!("table:{connection_id}:{}:{}", schema.name, table.name),
                category_id: format!(
                    "category:{connection_id}:{}",
                    category_slug(table.item_type)
                ),
                item_type: table.item_type,
            });
        }

        let schema_name = target.schema_name.as_deref().unwrap_or("public");
        self.open_levels(connection_id, &target.database_name, Some(schema_name))?;
        let (database, schema) = self
            .database_metadata
            .get(&connection_id)
            .and_then(|metadata| {
                let database = metadata
                    .databases
                    .iter()
                    .find(|database| database.name == target.database_name)?;
                let schema = database
                    .schemas
                    .iter()
                    .find(|schema| schema.name == schema_name)?;
                Some((database, schema))
            })
            .ok_or(None)?;

        let table = schema
            .tables
            .iter()
            .find(|table| table.name == target.name)
            .ok_or(None)?;
        let path = format!("{connection_id}:{}:{}", database.name, schema.name);
        let mut ancestor_ids = Vec::new();
        let mut parent = table.parent.as_ref();
        // Bounded by the table count in case the catalog ever reports a cycle.
        while let Some(table_parent) = parent
            && ancestor_ids.len() < schema.tables.len()
        {
            ancestor_ids.push(format!("table:{path}:{}", table_parent.name));
            parent = schema
                .tables
                .iter()
                .find(|candidate| {
                    candidate.item_type == DatabaseItemType::Table
                        && candidate.name == table_parent.name
                })
                .and_then(|candidate| candidate.parent.as_ref());
        }
        Ok(Located {
            ancestor_ids,
            item_id: format!("table:{path}:{}", table.name),
            category_id: format!("category:{path}:{}", category_slug(table.item_type)),
            item_type: table.item_type,
        })
    }
}

/// A tree level a reveal has to load before it can go on.
enum RevealLoad {
    Database {
        connection_id: i64,
        database_name: String,
    },
    Schema {
        connection_id: i64,
        database_name: String,
        schema_name: String,
    },
}

impl RevealLoad {
    /// The tree item id of the level.
    fn key(&self) -> String {
        match self {
            RevealLoad::Database {
                connection_id,
                database_name,
            } => format!("database:{connection_id}:{database_name}"),
            RevealLoad::Schema {
                connection_id,
                database_name,
                schema_name,
            } => format!("schema:{connection_id}:{database_name}:{schema_name}"),
        }
    }

    fn start(self, panel: &mut ConnectionsPanel, cx: &mut Context<ConnectionsPanel>) {
        panel.set_item_loading(&self.key(), true, cx);
        match self {
            RevealLoad::Database {
                connection_id,
                database_name,
            } => panel.load_database_children(connection_id, database_name, cx),
            RevealLoad::Schema {
                connection_id,
                database_name,
                schema_name,
            } => panel.load_schema_children(connection_id, database_name, schema_name, cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext as _, TestAppContext, VisualTestContext};

    use super::*;
    use crate::test_harness::{TestHarness, run_query, set_editor_text, wait_for_query};

    fn selected_id(
        panel: &gpui::Entity<ConnectionsPanel>,
        cx: &VisualTestContext,
    ) -> Option<String> {
        panel.read_with(cx, |panel, cx| {
            panel
                .tree_state
                .read(cx)
                .selected_entry()
                .map(|entry| entry.item().id.to_string())
        })
    }

    /// Let the loads on the Tokio runtime land until the reveal is done.
    async fn settle(panel: &gpui::Entity<ConnectionsPanel>, cx: &mut VisualTestContext) {
        for _ in 0..100 {
            cx.run_until_parked();
            if panel.read_with(cx, |panel, _| panel.pending_reveal.is_none()) {
                return;
            }
            cx.executor()
                .timer(std::time::Duration::from_millis(20))
                .await;
        }
        panic!("reveal did not finish");
    }

    /// Revealing a view on a connection that is not loaded yet loads it, opens
    /// the Views folder and selects the view, and a rebuild keeps it selected.
    #[gpui::test]
    async fn reveal_loads_opens_and_selects(cx: &mut TestAppContext) {
        let harness = TestHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        for sql in [
            "CREATE TABLE orders (id INTEGER)",
            "CREATE VIEW recent AS SELECT * FROM orders",
        ] {
            set_editor_text(&harness, sql, &mut cx);
            run_query(&harness, &mut cx);
            wait_for_query(&harness, &mut cx).await;
        }

        let panel = cx.update(|window, cx| cx.new(|cx| ConnectionsPanel::new(window, cx)));
        // The saved connections load first and would replace the one set below.
        for _ in 0..100 {
            cx.run_until_parked();
            if panel.read_with(&cx, |panel, _| panel.initial_load_completed) {
                break;
            }
            cx.executor()
                .timer(std::time::Duration::from_millis(20))
                .await;
        }
        let connection_id = harness.connection_id;
        panel.update(&mut cx, |panel, cx| {
            let mut connection =
                app_database::ConnectionData::new_sqlite("test".into(), String::new());
            connection.id = Some(connection_id);
            panel.connections = vec![connection];
            panel.rebuild_tree(cx);
            panel.reveal(
                RevealTarget {
                    connection_id,
                    database_name: "main".into(),
                    schema_name: None,
                    name: "recent".into(),
                },
                cx,
            );
        });
        settle(&panel, &mut cx).await;
        let expected = format!("table:{connection_id}:main:recent");
        assert_eq!(selected_id(&panel, &cx), Some(expected.clone()));

        panel.update(&mut cx, |panel, cx| panel.rebuild_tree(cx));
        assert_eq!(selected_id(&panel, &cx), Some(expected));

        // An object the tree does not have leaves the selection alone.
        panel.update(&mut cx, |panel, cx| {
            panel.reveal(
                RevealTarget {
                    connection_id,
                    database_name: "main".into(),
                    schema_name: None,
                    name: "missing".into(),
                },
                cx,
            );
        });
        settle(&panel, &mut cx).await;
        assert_eq!(
            selected_id(&panel, &cx),
            Some(format!("table:{connection_id}:main:recent"))
        );
    }
    /// Opening a PostgreSQL or CockroachDB connection opens its database and
    /// the schema a session lands in, and a connection that turns it off
    /// stays closed, against the compose servers. Skipped when they are down,
    /// unless `BLANCO_RUN_DB_TESTS=1`.
    #[gpui::test]
    async fn opening_a_connection_opens_its_default_schema(cx: &mut TestAppContext) {
        use crate::test_harness::LiveServer;
        use database::DatabaseType;

        let harness = TestHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        let panel = cx.update(|window, cx| cx.new(|cx| ConnectionsPanel::new(window, cx)));
        for _ in 0..100 {
            cx.run_until_parked();
            if panel.read_with(&cx, |panel, _| panel.initial_load_completed) {
                break;
            }
            cx.executor()
                .timer(std::time::Duration::from_millis(20))
                .await;
        }

        for (db_type, port, config_id) in [
            (DatabaseType::PostgreSQL, 5488, 9802),
            (DatabaseType::CockroachDb, 26258, 9803),
        ] {
            let Some(server) = LiveServer::connect(
                (db_type, port),
                config_id,
                "CREATE TABLE IF NOT EXISTS public.blanco_open_default (id int)",
                &mut cx,
            ) else {
                continue;
            };
            let connection_id = server.config_id;
            let expansion = |panel: &ConnectionsPanel| {
                let database = panel
                    .database_metadata
                    .get(&connection_id)?
                    .databases
                    .iter()
                    .find(|database| database.name == "blanco")?;
                let public = database
                    .schemas
                    .iter()
                    .find(|schema| schema.name == "public")
                    .is_some_and(|schema| schema.is_expanded && !schema.tables.is_empty());
                Some((database.is_expanded, public))
            };

            for (open_default_schema, expected) in [(true, (true, true)), (false, (false, false))] {
                panel.update(&mut cx, |panel, cx| {
                    let mut connection = app_database::ConnectionData::new_server(
                        db_type,
                        "it-schemas".into(),
                        "localhost".into(),
                        i32::from(port),
                        "blanco".into(),
                        "blanco".into(),
                        String::new(),
                    );
                    connection.id = Some(connection_id);
                    connection.open_default_schema = open_default_schema;
                    panel.connections = vec![connection];
                    panel.forget_connection(connection_id);
                    panel.rebuild_tree(cx);
                    panel.load_connection_children(connection_id, ConnectionLoad::Open, cx);
                });
                for _ in 0..200 {
                    cx.run_until_parked();
                    let done = panel.read_with(&cx, |panel, _| {
                        panel.loaded_connections.contains(&connection_id)
                            && panel.pending_reveal.is_none()
                    });
                    if done {
                        break;
                    }
                    cx.executor()
                        .timer(std::time::Duration::from_millis(20))
                        .await;
                }
                assert_eq!(
                    panel.read_with(&cx, |panel, _| expansion(panel)),
                    Some(expected),
                    "{db_type:?} with open_default_schema = {open_default_schema}"
                );
                assert!(
                    panel.read_with(&cx, |panel, _| panel
                        .expanded_connections
                        .contains(&connection_id)),
                    "the {db_type:?} connection is open"
                );
            }
        }
    }

    /// PostgreSQL types and sequences load into folders of their own under
    /// their schema, against the compose server. Skipped when it is down,
    /// unless `BLANCO_RUN_DB_TESTS=1`.
    #[gpui::test]
    async fn types_and_sequences_are_listed_under_their_schema(cx: &mut TestAppContext) {
        use crate::test_harness::LiveServer;

        let harness = TestHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        let Some(server) = LiveServer::connect(
            (database::DatabaseType::PostgreSQL, 5488),
            9801,
            "DROP SCHEMA IF EXISTS blanco_types_sidebar CASCADE;
             CREATE SCHEMA blanco_types_sidebar;
             CREATE TYPE blanco_types_sidebar.mood AS ENUM ('sad', 'happy');
             CREATE TYPE blanco_types_sidebar.pair AS (a int, b text);
             CREATE DOMAIN blanco_types_sidebar.score AS int CHECK (VALUE >= 0);
             CREATE SEQUENCE blanco_types_sidebar.ticket_numbers;
             CREATE TABLE blanco_types_sidebar.visits (id int PRIMARY KEY, mood blanco_types_sidebar.mood);",
            &mut cx,
        ) else {
            return;
        };

        let panel = cx.update(|window, cx| cx.new(|cx| ConnectionsPanel::new(window, cx)));
        for _ in 0..100 {
            cx.run_until_parked();
            if panel.read_with(&cx, |panel, _| panel.initial_load_completed) {
                break;
            }
            cx.executor()
                .timer(std::time::Duration::from_millis(20))
                .await;
        }
        let connection_id = server.config_id;
        panel.update(&mut cx, |panel, cx| {
            let mut connection = app_database::ConnectionData::new_server(
                database::DatabaseType::PostgreSQL,
                "it-schemas".into(),
                "localhost".into(),
                5488,
                "blanco".into(),
                "blanco".into(),
                String::new(),
            );
            connection.id = Some(connection_id);
            panel.connections = vec![connection];
            panel.rebuild_tree(cx);
            panel.reveal(
                RevealTarget {
                    connection_id,
                    database_name: "blanco".into(),
                    schema_name: Some("blanco_types_sidebar".into()),
                    name: "ticket_numbers".into(),
                },
                cx,
            );
        });
        settle(&panel, &mut cx).await;
        let kinds = panel.read_with(&cx, |panel, _| {
            panel
                .database_metadata
                .get(&connection_id)
                .and_then(|metadata| metadata.databases.iter().find(|db| db.name == "blanco"))
                .and_then(|database| {
                    database
                        .schemas
                        .iter()
                        .find(|schema| schema.name == "blanco_types_sidebar")
                })
                .map(|schema| {
                    schema
                        .tables
                        .iter()
                        .map(|item| (item.name.clone(), item.item_type))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        });
        assert_eq!(
            kinds,
            [
                ("mood".to_string(), DatabaseItemType::Enum),
                ("pair".to_string(), DatabaseItemType::CompositeType),
                ("score".to_string(), DatabaseItemType::Domain),
                ("ticket_numbers".to_string(), DatabaseItemType::Sequence),
                ("visits".to_string(), DatabaseItemType::Table),
            ]
        );
        assert_eq!(
            selected_id(&panel, &cx),
            Some(format!(
                "table:{connection_id}:blanco:blanco_types_sidebar:ticket_numbers"
            ))
        );

        assert!(
            panel.read_with(&cx, |panel, _| panel.expanded_categories.contains(
                &format!("category:{connection_id}:blanco:blanco_types_sidebar:sequences")
            )),
            "the Sequences folder is open"
        );

        server.script("DROP SCHEMA blanco_types_sidebar CASCADE");
    }
}
