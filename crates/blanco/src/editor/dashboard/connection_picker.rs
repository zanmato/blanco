//! Where a new panel runs: a connection and, on a server with several, one
//! of its databases, picked from a tree like the connections sidebar's.

use std::collections::HashMap;

use app_database::ConnectionData;
use blanco_core::ConnectionContext;
use database::DatabaseService;
use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, Sizable as _, h_flex,
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement as _,
    v_flex,
};

use super::super::EditorPanel;
use super::connection_context;
use crate::result_ext::ResultExt as _;
use blanco_ui::IconName;

const ROW_HEIGHT: gpui::Pixels = px(28.);

/// A connection's databases, as far as they are known.
enum Databases {
    Loading,
    Loaded(Vec<String>),
    Failed(String),
}

/// A row of the tree, in the order it is drawn.
enum Row {
    Connection(usize),
    Database(usize, String),
    Note(usize, SharedString),
}

pub(super) struct ConnectionPicker {
    connections: Vec<ConnectionData>,
    filter_input: Entity<InputState>,
    /// The databases of connections opened in the tree, by connection id.
    databases: HashMap<i64, Databases>,
    expanded: Vec<i64>,
    selected: Option<ConnectionContext>,
    scroll_handle: ScrollHandle,
    /// Run when a row is double clicked, to take the pick at once.
    on_confirm: Option<Box<dyn Fn(&mut Window, &mut App)>>,
    _subscriptions: Vec<Subscription>,
}

impl ConnectionPicker {
    /// Offer the SQL connections, with `preferred` selected and, when its
    /// server has several databases, opened.
    pub(super) fn new(
        connections: Vec<ConnectionData>,
        preferred: Option<ConnectionContext>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let connections: Vec<ConnectionData> = connections
            .into_iter()
            .filter(|connection| connection.db_type.supports_sql() && connection.id.is_some())
            .collect();
        let filter_input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        let filter = cx.subscribe(&filter_input, |_, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                cx.notify();
            }
        });
        let mut this = Self {
            connections,
            filter_input,
            databases: HashMap::new(),
            expanded: Vec::new(),
            selected: None,
            scroll_handle: ScrollHandle::new(),
            on_confirm: None,
            _subscriptions: vec![filter],
        };
        let preferred = preferred.or_else(|| this.connections.first().and_then(connection_context));
        if let Some(context) = preferred {
            if this.has_databases(context.connection_id) {
                this.toggle(context.connection_id, window, cx);
            }
            this.selected = Some(context);
        }
        this
    }

    pub(super) fn on_confirm(&mut self, confirm: impl Fn(&mut Window, &mut App) + 'static) {
        self.on_confirm = Some(Box::new(confirm));
    }

    /// The connection and database picked, `None` before one is.
    pub(super) fn selected(&self) -> Option<ConnectionContext> {
        self.selected.clone()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.connections.is_empty()
    }

    fn connection(&self, connection_id: i64) -> Option<&ConnectionData> {
        self.connections
            .iter()
            .find(|connection| connection.id == Some(connection_id))
    }

    /// Whether the connection's server holds databases to pick among. A
    /// backend whose connection is one database file has none.
    fn has_databases(&self, connection_id: i64) -> bool {
        self.connection(connection_id).is_some_and(|connection| {
            connection.db_type.dialect().file_database_name().is_empty()
                && connection.pinned_database().is_none()
        })
    }

    /// Open or close a connection's databases, listing them the first time.
    fn toggle(&mut self, connection_id: i64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.expanded.iter().position(|id| *id == connection_id) {
            self.expanded.remove(ix);
            cx.notify();
            return;
        }
        self.expanded.push(connection_id);
        if matches!(
            self.databases.get(&connection_id),
            Some(Databases::Loaded(_) | Databases::Loading)
        ) {
            cx.notify();
            return;
        }
        self.databases.insert(connection_id, Databases::Loading);
        cx.notify();
        let service = DatabaseService::global(cx).clone();
        let connection_name = self
            .connection(connection_id)
            .map(|connection| connection.name.clone())
            .unwrap_or_default();
        let activity = crate::status_bar::ActivityReporter::global(cx)
            .begin(format!("{connection_name}: listing databases"));
        let task = gpui_tokio::Tokio::spawn_result(cx, async move {
            // Dropped once the databases are listed.
            let _activity = activity;
            // Listing them is the only thing it is opened for.
            let connection = service
                .open_short_lived_read_connection(connection_id, None)
                .await?;
            let databases = connection.connection.get_databases().await;
            service.close_short_lived_connection(connection).await;
            databases
        });
        cx.spawn_in(window, async move |this, cx| {
            let databases = task.await;
            this.update(cx, |this, cx| {
                let databases = match databases {
                    Ok(databases) => Databases::Loaded(databases),
                    Err(error) => Databases::Failed(format!("{error:#}")),
                };
                this.databases.insert(connection_id, databases);
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }

    fn select(&mut self, connection_id: i64, database: Option<String>, cx: &mut Context<Self>) {
        let Some(mut context) = self.connection(connection_id).and_then(connection_context) else {
            return;
        };
        if let Some(database) = database {
            context.database_name = database;
        }
        self.selected = Some(context);
        cx.notify();
    }

    fn rows(&self, cx: &App) -> Vec<Row> {
        let needle = self.filter_input.read(cx).value().to_lowercase();
        let mut rows = Vec::new();
        for (ix, connection) in self.connections.iter().enumerate() {
            let Some(id) = connection.id else {
                continue;
            };
            let databases = match self.databases.get(&id) {
                Some(Databases::Loaded(databases)) => databases.as_slice(),
                _ => &[],
            };
            let name_matches = connection.name.to_lowercase().contains(&needle);
            let matching: Vec<&String> = databases
                .iter()
                .filter(|database| name_matches || database.to_lowercase().contains(&needle))
                .collect();
            if !needle.is_empty() && !name_matches && matching.is_empty() {
                continue;
            }
            rows.push(Row::Connection(ix));
            if !self.expanded.contains(&id) {
                continue;
            }
            match self.databases.get(&id) {
                Some(Databases::Loading) => rows.push(Row::Note(ix, "Connecting…".into())),
                Some(Databases::Failed(error)) => rows.push(Row::Note(ix, error.clone().into())),
                Some(Databases::Loaded(_)) if matching.is_empty() => {
                    rows.push(Row::Note(ix, "No databases".into()))
                }
                Some(Databases::Loaded(_)) => rows.extend(
                    matching
                        .into_iter()
                        .map(|database| Row::Database(ix, database.clone())),
                ),
                None => {}
            }
        }
        rows
    }

    fn render_row(&self, row_ix: usize, row: Row, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = cx.theme();
        let (connection_ix, database, note) = match row {
            Row::Connection(ix) => (ix, None, None),
            Row::Database(ix, database) => (ix, Some(database), None),
            Row::Note(ix, note) => (ix, None, Some(note)),
        };
        let Some(connection) = self.connections.get(connection_ix) else {
            return div().into_any_element();
        };
        let Some(connection_id) = connection.id else {
            return div().into_any_element();
        };
        if let Some(note) = note {
            return div()
                .h(ROW_HEIGHT)
                .pl(px(44.))
                .flex()
                .items_center()
                .text_xs()
                .text_color(theme.muted_foreground)
                .truncate()
                .child(note)
                .into_any_element();
        }
        let has_databases = self.has_databases(connection_id);
        let is_connection_row = database.is_none();
        let selected = self.selected.as_ref().is_some_and(|selected| {
            selected.connection_id == connection_id
                && match &database {
                    Some(database) => &selected.database_name == database,
                    // A connection without databases to pick is itself the pick.
                    None => !has_databases,
                }
        });
        let expanded = self.expanded.contains(&connection_id);
        let label: SharedString = match &database {
            Some(database) => database.clone().into(),
            None => connection.name.clone().into(),
        };
        let click_database = database;
        h_flex()
            .id(("connection-picker-row", row_ix))
            .h(ROW_HEIGHT)
            .px_2()
            .gap_2()
            .items_center()
            .rounded(theme.radius)
            .text_sm()
            .cursor_pointer()
            .when(!is_connection_row, |row| row.pl(px(30.)))
            .when(selected, |row| row.bg(theme.list_active))
            .when(!selected, |row| row.hover(|row| row.bg(theme.list_hover)))
            .when(is_connection_row, |row| {
                row.child(div().w(px(14.)).when(has_databases, |chevron| {
                    chevron.child(
                        Icon::new(if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .xsmall()
                        .text_color(theme.muted_foreground),
                    )
                }))
                .child(
                    Icon::new(crate::connections::connection_type_icon(connection.db_type))
                        .small()
                        .text_color(theme.foreground),
                )
            })
            .when(!is_connection_row, |row| {
                row.child(
                    Icon::new(IconName::Database)
                        .small()
                        .text_color(theme.muted_foreground),
                )
            })
            .child(div().flex_1().min_w_0().truncate().child(label))
            .when(is_connection_row, |row| {
                row.child(EditorPanel::environment_badge(
                    connection.environment_type,
                    cx,
                ))
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                match &click_database {
                    Some(database) => this.select(connection_id, Some(database.clone()), cx),
                    None if this.has_databases(connection_id) => {
                        this.toggle(connection_id, window, cx);
                        return;
                    }
                    None => this.select(connection_id, None, cx),
                }
                if event.click_count() == 2
                    && let Some(confirm) = &this.on_confirm
                {
                    confirm(window, cx);
                }
            }))
            .into_any_element()
    }
}

impl Render for ConnectionPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<_> = self
            .rows(cx)
            .into_iter()
            .enumerate()
            .map(|(ix, row)| self.render_row(ix, row, cx))
            .collect();
        let muted = cx.theme().muted_foreground;
        let picked = match &self.selected {
            Some(context)
                if context.database_name.is_empty()
                    || !self.has_databases(context.connection_id) =>
            {
                context.connection_name.clone()
            }
            Some(context) => format!("{} / {}", context.connection_name, context.database_name),
            None => "Pick a connection".to_string(),
        };
        v_flex()
            .gap_2()
            .child(Input::new(&self.filter_input).small().cleanable(true))
            .child(
                div()
                    .h(px(260.))
                    .relative()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .child(
                        v_flex()
                            .id("connection-picker-rows")
                            .size_full()
                            .p_1()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll_handle)
                            .children(rows),
                    )
                    .vertical_scrollbar(&self.scroll_handle),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("Runs read only on {picked}")),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_harness::{LiveServer, TestHarness};
    use database::DatabaseType;
    use gpui::{TestAppContext, VisualTestContext};

    #[gpui::test]
    async fn test_a_server_offers_its_databases(cx: &mut TestAppContext) {
        let harness = TestHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        const CONFIG_ID: i64 = 9_101;
        if LiveServer::connect(
            (DatabaseType::PostgreSQL, 5488),
            CONFIG_ID,
            "SELECT 1",
            &mut cx,
        )
        .is_none()
        {
            return;
        }
        let file = ConnectionData::new_sqlite("Notes".to_string(), "/tmp/notes.db".to_string());
        let server = ConnectionData {
            id: Some(CONFIG_ID),
            name: "Warehouse".to_string(),
            db_type: DatabaseType::PostgreSQL,
            database_name: Some("blanco".to_string()),
            database_path: None,
            ..file.clone()
        };
        let file = ConnectionData {
            id: Some(CONFIG_ID + 1),
            ..file
        };
        let picker = cx.update(|window, cx| {
            cx.new(|cx| ConnectionPicker::new(vec![server, file], None, window, cx))
        });

        // The first connection is picked and, holding databases, opened.
        let mut databases = Vec::new();
        for _ in 0..200 {
            cx.run_until_parked();
            if let Some(Databases::Loaded(loaded)) =
                picker.read_with(&cx, |picker, _| match picker.databases.get(&CONFIG_ID) {
                    Some(Databases::Loaded(loaded)) => Some(Databases::Loaded(loaded.clone())),
                    Some(Databases::Failed(error)) => panic!("{error}"),
                    _ => None,
                })
            {
                databases = loaded;
                break;
            }
            cx.executor()
                .timer(std::time::Duration::from_millis(20))
                .await;
        }
        assert!(
            databases.iter().any(|name| name == "postgres"),
            "{databases:?}"
        );

        picker.update(&mut cx, |picker, cx| {
            picker.select(CONFIG_ID, Some("postgres".to_string()), cx)
        });
        let picked = picker.read_with(&cx, |picker, _| picker.selected());
        assert_eq!(
            picked.map(|context| (context.connection_name, context.database_name)),
            Some(("Warehouse".to_string(), "postgres".to_string()))
        );

        // A database file is picked as it is.
        picker.update(&mut cx, |picker, cx| picker.select(CONFIG_ID + 1, None, cx));
        let picked = picker.read_with(&cx, |picker, _| picker.selected());
        assert_eq!(
            picked.map(|context| context.database_name),
            Some("main".to_string())
        );
    }
}
