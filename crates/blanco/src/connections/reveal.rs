//! Opening the tree down to an object and selecting it, so the sidebar can
//! follow the active tab.

use gpui::{Context, ScrollStrategy};

use super::ConnectionsPanel;
use super::data_loading::DatabaseItemType;
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

/// A reveal waiting on the tree to load a level.
#[derive(Debug, Clone)]
pub(super) struct PendingReveal {
    target: RevealTarget,
    /// The level whose load was requested last. A reveal that needs the same
    /// load twice gives up, so a failing load cannot loop.
    requested_load: Option<String>,
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
            target,
            requested_load: None,
        });
        self.advance_reveal(cx);
    }

    /// Take the reveal as far as the loaded tree allows.
    pub(super) fn advance_reveal(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_reveal.clone() else {
            return;
        };
        let target = &pending.target;
        let connection_id = target.connection_id;

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
                self.load_connection_children(connection_id, true, cx);
            }
            return;
        }
        self.expanded_connections.insert(connection_id);

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
    /// last load asked for too, which ends the reveal.
    fn request_reveal_load(&mut self, key: &str) -> bool {
        let Some(pending) = self.pending_reveal.as_mut() else {
            return false;
        };
        if pending.requested_load.as_deref() == Some(key) {
            self.pending_reveal = None;
            return false;
        }
        pending.requested_load = Some(key.to_string());
        true
    }

    /// Find the target in the loaded tree, expanding its database and schema
    /// on the way. `Err(Some(load))` names the level to load first,
    /// `Err(None)` means the object is not there.
    fn locate(&mut self, target: &RevealTarget) -> Result<Located, Option<RevealLoad>> {
        let connection_id = target.connection_id;
        let metadata = self.database_metadata.get_mut(&connection_id).ok_or(None)?;

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

        let database = metadata
            .databases
            .iter_mut()
            .find(|database| database.name == target.database_name)
            .ok_or(None)?;
        if database.schemas.is_empty() {
            return Err(Some(RevealLoad::Database {
                connection_id,
                database_name: database.name.clone(),
            }));
        }
        database.is_expanded = true;

        let schema_name = target.schema_name.as_deref().unwrap_or("public");
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
}
