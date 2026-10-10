//! The sidebar's Copy submenu: names, CREATE statements and SELECT/INSERT
//! scripts for one or several objects.

use anyhow::Context as _;
use database::{DatabaseService, DatabaseType};
use gpui::{ClipboardItem, Context, SharedString, Window};
use gpui_component::{WindowExt as _, notification::NotificationType};

use super::{ConnectionsPanel, TreeItemKind, TreeItemMetadata};
use crate::app::CreateNewQueryTab;
use crate::result_ext::ResultExt as _;

/// What the Copy submenu copies for each object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ObjectCopy {
    Name,
    QualifiedName,
    CreateStatement,
    Select,
    InsertTemplate,
}

impl ObjectCopy {
    pub(super) fn label(self) -> &'static str {
        match self {
            ObjectCopy::Name => "Name",
            ObjectCopy::QualifiedName => "Qualified Name",
            ObjectCopy::CreateStatement => "CREATE Statement",
            ObjectCopy::Select => "SELECT with Columns",
            ObjectCopy::InsertTemplate => "INSERT Template",
        }
    }

    /// The copies offered for an object of `kind`: every one for tables and
    /// views, the names and CREATE statement for routines.
    pub(super) fn offered_for(kind: &TreeItemKind) -> &'static [ObjectCopy] {
        match kind {
            TreeItemKind::Table | TreeItemKind::View | TreeItemKind::MaterializedView => &[
                ObjectCopy::Name,
                ObjectCopy::QualifiedName,
                ObjectCopy::CreateStatement,
                ObjectCopy::Select,
                ObjectCopy::InsertTemplate,
            ],
            TreeItemKind::Procedure | TreeItemKind::Function | TreeItemKind::Trigger => &[
                ObjectCopy::Name,
                ObjectCopy::QualifiedName,
                ObjectCopy::CreateStatement,
            ],
            _ => &[],
        }
    }

    /// Whether the copy is built from the object's name alone.
    fn is_local(self) -> bool {
        matches!(self, ObjectCopy::Name | ObjectCopy::QualifiedName)
    }

    /// What goes between the copies of several objects.
    fn separator(self) -> &'static str {
        if self.is_local() { ", " } else { "\n\n" }
    }
}

/// The schema, or on backends without schemas the database, that qualifies
/// the object's name. SQLite's `main` is left out.
fn qualifier(object: &TreeItemMetadata) -> Option<&str> {
    object.schema_name.as_deref().or(match object.db_type {
        DatabaseType::SQLite => None,
        _ => object.database_name.as_deref(),
    })
}

/// The copy of `object` that needs nothing from the server.
fn local_copy(object: &TreeItemMetadata, copy: ObjectCopy) -> Option<String> {
    let name = object.table_name.as_deref()?;
    let dialect = object.db_type.dialect();
    match copy {
        ObjectCopy::Name => Some(dialect.quote_identifier(name)),
        ObjectCopy::QualifiedName => Some(dialect.quote_qualified(qualifier(object), name)),
        _ => None,
    }
}

/// The copy of `object` read from the server: its DDL, or a statement built
/// from its columns.
async fn server_copy(
    db_service: &DatabaseService,
    object: &TreeItemMetadata,
    copy: ObjectCopy,
) -> anyhow::Result<String> {
    let name = object.table_name.as_deref().context("no object name")?;
    let connection = db_service
        .get_or_create_connection(object.connection_id, object.database_name.as_deref())
        .await?;
    let schema = object.schema_name.as_deref();
    let dialect = object.db_type.dialect();
    let columns = || async {
        let columns = connection.get_columns_for_table(name, schema).await?;
        anyhow::Ok(
            columns
                .into_iter()
                .map(|column| column.name)
                .collect::<Vec<_>>(),
        )
    };
    Ok(match copy {
        ObjectCopy::CreateStatement => match object.kind.routine_kind() {
            Some(kind) => connection.object_ddl(kind, schema, name).await?,
            None => connection.table_ddl(schema, name).await?,
        },
        ObjectCopy::Select => dialect.select_columns(qualifier(object), name, &columns().await?),
        ObjectCopy::InsertTemplate => {
            dialect.insert_template(qualifier(object), name, &columns().await?)
        }
        ObjectCopy::Name | ObjectCopy::QualifiedName => {
            local_copy(object, copy).context("no object name")?
        }
    })
}

impl ConnectionsPanel {
    /// Copy `copy` of every object to the clipboard, in order.
    pub(super) fn copy_objects(
        &mut self,
        objects: Vec<TreeItemMetadata>,
        copy: ObjectCopy,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if copy.is_local() {
            let text = objects
                .iter()
                .filter_map(|object| local_copy(object, copy))
                .collect::<Vec<_>>()
                .join(copy.separator());
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            return;
        }

        let db_service = DatabaseService::global(cx).clone();
        cx.spawn_in(window, async move |_, cx| {
            let mut parts = Vec::with_capacity(objects.len());
            for object in &objects {
                match server_copy(&db_service, object, copy).await {
                    Ok(part) => parts.push(part),
                    Err(error) => {
                        let name = object.table_name.clone().unwrap_or_default();
                        cx.update(|window, cx| {
                            window.push_notification(
                                (
                                    NotificationType::Error,
                                    SharedString::from(format!(
                                        "Could not copy the {} of {name}: {error}",
                                        copy.label()
                                    )),
                                ),
                                cx,
                            );
                        })
                        .log_err();
                        return;
                    }
                }
            }
            let text = parts.join(copy.separator());
            cx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string(text)))
                .log_err();
        })
        .detach();
    }

    /// Open a query tab holding a SELECT of `object` listing every column.
    pub(super) fn open_select_with_columns(
        &mut self,
        object: TreeItemMetadata,
        action: CreateNewQueryTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let db_service = DatabaseService::global(cx).clone();
        cx.spawn_in(window, async move |_, cx| {
            let result = server_copy(&db_service, &object, ObjectCopy::Select).await;
            cx.update(|window, cx| match result {
                Ok(select) => window.dispatch_action(
                    Box::new(CreateNewQueryTab {
                        content: Some(select),
                        ..action
                    }),
                    cx,
                ),
                Err(error) => window.push_notification(
                    (
                        NotificationType::Error,
                        SharedString::from(format!("Could not list the columns: {error}")),
                    ),
                    cx,
                ),
            })
            .log_err();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(db_type: DatabaseType, database: &str, schema: Option<&str>) -> TreeItemMetadata {
        TreeItemMetadata {
            connection_id: 1,
            connection_name: "test".into(),
            kind: TreeItemKind::Table,
            db_type,
            database_name: Some(database.into()),
            schema_name: schema.map(str::to_string),
            table_name: Some("Order Items".into()),
            icon: super::super::TreeItemIcon {
                icon: blanco_ui::IconName::Database,
                color: gpui::rgba(0),
            },
            environment_type: None,
            loading: false,
            size_bytes: None,
            relative_size: None,
        }
    }

    #[test]
    fn names_are_quoted_and_qualified_for_the_dialect() {
        let pg = object(DatabaseType::PostgreSQL, "shop", Some("sales"));
        assert_eq!(
            local_copy(&pg, ObjectCopy::Name).unwrap(),
            "\"Order Items\""
        );
        assert_eq!(
            local_copy(&pg, ObjectCopy::QualifiedName).unwrap(),
            "\"sales\".\"Order Items\""
        );

        // Without schemas the database qualifies the name, except SQLite's.
        let mysql = object(DatabaseType::MySQL, "shop", None);
        assert_eq!(
            local_copy(&mysql, ObjectCopy::QualifiedName).unwrap(),
            "`shop`.`Order Items`"
        );
        let sqlite = object(DatabaseType::SQLite, "main", None);
        assert_eq!(
            local_copy(&sqlite, ObjectCopy::QualifiedName).unwrap(),
            "\"Order Items\""
        );
    }

    /// Copies read from the server list the table's columns, and several
    /// objects are copied together.
    #[gpui::test]
    async fn scripts_list_the_columns(cx: &mut gpui::TestAppContext) {
        use crate::test_harness::{TestHarness, run_query, set_editor_text, wait_for_query};
        use gpui::AppContext as _;

        let harness = TestHarness::new(cx);
        let mut cx = gpui::VisualTestContext::from_window(harness.window_handle.into(), cx);
        set_editor_text(
            &harness,
            "CREATE TABLE orders (id INTEGER PRIMARY KEY, total REAL)",
            &mut cx,
        );
        run_query(&harness, &mut cx);
        wait_for_query(&harness, &mut cx).await;

        let panel = cx.update(|window, cx| cx.new(|cx| ConnectionsPanel::new(window, cx)));
        let orders = TreeItemMetadata {
            connection_id: harness.connection_id,
            table_name: Some("orders".into()),
            ..object(DatabaseType::SQLite, "main", None)
        };

        let mut copied = |copy: ObjectCopy, objects: Vec<TreeItemMetadata>| {
            cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
            panel.update_in(&mut cx, |panel, window, cx| {
                panel.copy_objects(objects, copy, window, cx)
            });
            for _ in 0..100 {
                cx.run_until_parked();
                let text = cx
                    .read_from_clipboard()
                    .and_then(|item| item.text())
                    .unwrap_or_default();
                if !text.is_empty() {
                    return text;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            panic!("nothing was copied");
        };

        assert_eq!(
            copied(ObjectCopy::Select, vec![orders.clone()]),
            "SELECT\n    \"id\",\n    \"total\"\nFROM \"orders\";"
        );
        assert_eq!(
            copied(ObjectCopy::InsertTemplate, vec![orders.clone()]),
            "INSERT INTO \"orders\" (\"id\", \"total\")\nVALUES (?, ?);"
        );
        let items = TreeItemMetadata {
            table_name: Some("items".into()),
            ..orders.clone()
        };
        assert_eq!(
            copied(ObjectCopy::Name, vec![orders, items]),
            "\"orders\", \"items\""
        );
    }
}
