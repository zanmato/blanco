//! A dashboard's variables: their choices, the pickers under the header,
//! and the dialog that adds or changes one.

use std::time::Duration;

use database::DatabaseService;
use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, IndexPath, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    form::field,
    h_flex,
    input::{Input, InputState, Textarea, TextareaState},
    menu::{DropdownMenu as _, PopupMenuItem},
    notification::NotificationType,
    select::{Select, SelectEvent, SelectState},
    v_flex,
};

use super::parameters::{
    DashboardParameters, DashboardVariable, VariableSource, is_variable_name, variables_to_json,
};
use super::{DashboardTab, TabClosed, bounded_run};
use crate::app_settings::AppSettings;
use crate::result_ext::ResultExt as _;
use blanco_ui::IconName;

/// A query variable offers at most this many of its rows.
const MAX_OPTIONS: usize = 1000;

impl DashboardTab {
    /// Find every variable's choices, then refresh once the tab is shown.
    /// A query variable runs its query on a connection opened for it.
    pub(super) fn resolve_variables(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut queries = Vec::new();
        for variable in &self.variables {
            match &variable.source {
                VariableSource::Values { values } => {
                    self.variable_options
                        .insert(variable.name.clone(), Ok(values.clone()));
                }
                VariableSource::Query {
                    connection_id,
                    database_name,
                    query,
                } => queries.push((
                    variable.name.clone(),
                    *connection_id,
                    database_name.clone(),
                    query.clone(),
                )),
            }
        }
        self.variable_options
            .retain(|name, _| self.variables.iter().any(|variable| &variable.name == name));
        if queries.is_empty() {
            self.rebuild_variable_selects(window, cx);
            self.refresh_due = true;
            cx.notify();
            return;
        }

        let service = DatabaseService::global(cx).clone();
        let timeout = query_timeout(cx);
        let activity = crate::status_bar::ActivityReporter::global(cx)
            .begin(format!("{}: loading variable choices", self.title));
        // A variable query may filter on the time range, though not on
        // another variable.
        let parameters = DashboardParameters::new(self.time_range, Vec::new());
        let closed = self.closed.clone();
        // Variables on one server and database share a connection, as panels do.
        let mut groups: Vec<VariableGroup> = Vec::new();
        for (name, connection_id, database_name, query) in queries {
            match groups.iter_mut().find(|group| {
                group.connection_id == connection_id && group.database_name == database_name
            }) {
                Some(group) => group.queries.push((name, query)),
                None => groups.push(VariableGroup {
                    connection_id,
                    database_name,
                    queries: vec![(name, query)],
                }),
            }
        }
        let task = gpui_tokio::Tokio::spawn(cx, async move {
            let resolved =
                futures::future::join_all(groups.into_iter().map(|group| {
                    group_options(&service, group, &parameters, timeout, closed.clone())
                }))
                .await;
            resolved.into_iter().flatten().collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let resolved = task.await;
            drop(activity);
            this.update_in(cx, |this, window, cx| {
                match resolved {
                    Ok(resolved) => this.variable_options.extend(resolved),
                    Err(error) => tracing::error!("Dashboard variables did not resolve: {error}"),
                }
                this.rebuild_variable_selects(window, cx);
                this.refresh_due = true;
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }

    fn rebuild_variable_selects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.variable_subscriptions.clear();
        self.variable_selects.clear();
        for variable in self.variables.clone() {
            let Some(Ok(options)) = self.variable_options.get(&variable.name) else {
                continue;
            };
            let selected = self
                .variable_value(&variable)
                .and_then(|value| options.iter().position(|option| *option == value));
            let options = options.clone();
            let select = cx.new(|cx| {
                SelectState::new(options, selected.map(IndexPath::new), window, cx).searchable(true)
            });
            let name = variable.name.clone();
            self.variable_subscriptions.push(cx.subscribe_in(
                &select,
                window,
                move |this, _, event: &SelectEvent<Vec<String>>, window, cx| {
                    let SelectEvent::Confirm(Some(value)) = event else {
                        return;
                    };
                    if let Some(variable) = this
                        .variables
                        .iter_mut()
                        .find(|variable| variable.name == name)
                    {
                        variable.current = Some(value.clone());
                    }
                    this.save_variables(cx);
                    this.refresh(window, cx);
                },
            ));
            self.variable_selects.push((variable.name, select));
        }
    }

    fn save_variables(&mut self, cx: &mut Context<Self>) {
        if let Some(dashboard) = &mut self.dashboard {
            dashboard.variables = variables_to_json(&self.variables);
        }
        self.save_dashboard(cx);
    }

    fn remove_variable(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.variables.retain(|variable| variable.name != name);
        self.save_variables(cx);
        self.resolve_variables(window, cx);
    }

    /// Add `variable`, or put it in the place of the one named `replacing`.
    fn put_variable(
        &mut self,
        replacing: Option<String>,
        variable: DashboardVariable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match replacing.and_then(|name| {
            self.variables
                .iter()
                .position(|existing| existing.name == name)
        }) {
            Some(ix) => self.variables[ix] = variable,
            None => self.variables.push(variable),
        }
        self.save_variables(cx);
        self.resolve_variables(window, cx);
    }

    /// The connection a new query variable runs on: the dashboard's default
    /// one where a panel uses it, else the first panel's.
    fn query_variable_connection(&self, cx: &App) -> Option<(i64, String, String)> {
        let default = self
            .dashboard
            .as_ref()
            .and_then(|dashboard| dashboard.default_connection_id);
        let contexts: Vec<_> = self
            .panels
            .iter()
            .filter_map(|panel| panel.read(cx).context())
            .collect();
        contexts
            .iter()
            .find(|context| Some(context.connection_id) == default)
            .or(contexts.first())
            .map(|context| {
                (
                    context.connection_id,
                    context.database_name.clone(),
                    context.connection_name.clone(),
                )
            })
    }

    /// Open the dialog for a new variable, or for the one named `editing`.
    pub(super) fn open_variable_dialog(
        &mut self,
        editing: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = editing.as_ref().and_then(|name| {
            self.variables
                .iter()
                .find(|variable| &variable.name == name)
                .cloned()
        });
        let connection = match &existing {
            Some(DashboardVariable {
                source:
                    VariableSource::Query {
                        connection_id,
                        database_name,
                        ..
                    },
                ..
            }) => {
                let name = self
                    .panels
                    .iter()
                    .filter_map(|panel| panel.read(cx).context())
                    .find(|context| context.connection_id == *connection_id)
                    .map(|context| context.connection_name)
                    .unwrap_or_else(|| "its connection".to_string());
                Some((*connection_id, database_name.clone(), name))
            }
            _ => self.query_variable_connection(cx),
        };
        let taken: Vec<String> = self
            .variables
            .iter()
            .map(|variable| variable.name.clone())
            .filter(|name| Some(name) != editing.as_ref())
            .collect();
        let form =
            cx.new(|cx| VariableForm::new(existing.as_ref(), connection.clone(), window, cx));
        let tab = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let form = form.clone();
            let tab = tab.clone();
            let taken = taken.clone();
            let editing = editing.clone();
            let existing_current = existing
                .as_ref()
                .and_then(|variable| variable.current.clone());
            dialog
                .title(if editing.is_some() {
                    "Edit Variable"
                } else {
                    "Add Variable"
                })
                .w(px(460.))
                .child(form.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("cancel").label("Cancel").outline()),
                        )
                        .child(
                            DialogAction::new().child(Button::new("ok").primary().label("Save")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let variable = match form.read(cx).variable(&taken, cx) {
                        Ok(mut variable) => {
                            variable.current = existing_current.clone();
                            variable
                        }
                        Err(problem) => {
                            window.push_notification((NotificationType::Warning, problem), cx);
                            return false;
                        }
                    };
                    let editing = editing.clone();
                    tab.update(cx, |tab, cx| {
                        tab.put_variable(editing, variable, window, cx)
                    })
                    .log_err();
                    true
                })
        });
    }

    /// The pickers of the dashboard's variables, under its header.
    pub(super) fn render_variables(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.variables.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let border = theme.border;
        let chips: Vec<_> = self
            .variables
            .iter()
            .enumerate()
            .map(|(ix, variable)| {
                let name = variable.name.clone();
                let select = self
                    .variable_selects
                    .iter()
                    .find(|(select_name, _)| *select_name == name)
                    .map(|(_, select)| select.clone());
                let failed = match self.variable_options.get(&name) {
                    Some(Err(error)) => Some(error.clone()),
                    _ => None,
                };
                let tab = cx.entity().downgrade();
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(div().text_sm().text_color(muted).child(format!("${name}")))
                    .map(|this| match (select, failed) {
                        (Some(select), _) => {
                            this.child(div().w(px(160.)).child(Select::new(&select).small()))
                        }
                        (None, Some(error)) => this.child(
                            div()
                                .max_w(px(260.))
                                .truncate()
                                .text_xs()
                                .text_color(danger)
                                .child(error),
                        ),
                        (None, None) => this.child(div().text_xs().text_color(muted).child("…")),
                    })
                    .child(
                        Button::new(("dashboard-variable-menu", ix))
                            .xsmall()
                            .ghost()
                            .icon(IconName::ChevronDown)
                            .dropdown_menu(move |menu, _, _| {
                                let edit = (tab.clone(), name.clone());
                                let remove = (tab.clone(), name.clone());
                                menu.item(PopupMenuItem::new("Edit…").on_click(
                                    move |_, window, cx| {
                                        let (tab, name) = edit.clone();
                                        tab.update(cx, |tab, cx| {
                                            tab.open_variable_dialog(Some(name), window, cx)
                                        })
                                        .log_err();
                                    },
                                ))
                                .item(
                                    PopupMenuItem::new("Remove").on_click(move |_, window, cx| {
                                        let (tab, name) = remove.clone();
                                        tab.update(cx, |tab, cx| {
                                            tab.remove_variable(&name, window, cx)
                                        })
                                        .log_err();
                                    }),
                                )
                            }),
                    )
            })
            .collect();
        Some(
            h_flex()
                .w_full()
                .flex_shrink_0()
                .flex_wrap()
                .px_3()
                .py_1p5()
                .gap_x_4()
                .gap_y_1()
                .items_center()
                .border_b_1()
                .border_color(border)
                .children(chips),
        )
    }
}

pub(super) fn query_timeout(cx: &App) -> Option<Duration> {
    let seconds = AppSettings::global(cx)
        .settings
        .database
        .query_timeout_seconds;
    (seconds > 0).then(|| Duration::from_secs(u64::from(seconds)))
}

/// The query variables that run on one connection and database.
struct VariableGroup {
    connection_id: i64,
    database_name: String,
    /// Each variable's name and query.
    queries: Vec<(String, String)>,
}

type VariableOptions = (String, Result<Vec<String>, String>);

/// Each variable's choices in `group`, read on one connection opened for
/// them and closed again.
async fn group_options(
    service: &DatabaseService,
    group: VariableGroup,
    parameters: &DashboardParameters,
    timeout: Option<Duration>,
    closed: TabClosed,
) -> Vec<VariableOptions> {
    let fail_all = |queries: Vec<(String, String)>, message: String| {
        queries
            .into_iter()
            .map(|(name, _)| (name, Err(message.clone())))
            .collect()
    };
    let Some(config) = service.get_connection_config(group.connection_id).await else {
        return fail_all(
            group.queries,
            "The variable's connection was deleted.".to_string(),
        );
    };
    let dialect = config.db_type.dialect();
    let database = (!group.database_name.is_empty()).then_some(group.database_name.as_str());
    let connection = match service
        .open_short_lived_read_connection(group.connection_id, database)
        .await
    {
        Ok(connection) => connection,
        Err(error) => return fail_all(group.queries, format!("Could not connect: {error:#}")),
    };
    let mut resolved = Vec::with_capacity(group.queries.len());
    for (name, query) in group.queries {
        let sql = parameters.apply(&query, dialect);
        let run = connection.connection.execute_query(&sql, database, None);
        let options = bounded_run(run, timeout, closed.clone())
            .await
            .map(first_column_values)
            .map_err(|error| format!("{error:#}"));
        resolved.push((name, options));
    }
    service.close_short_lived_connection(connection).await;
    resolved
}

/// The distinct values of the first column of `result`'s rows, in order.
fn first_column_values(result: blanco_core::QueryResult) -> Vec<String> {
    let mut options: Vec<String> = Vec::new();
    for value in result
        .rows
        .into_iter()
        .filter_map(|row| row.into_iter().next().flatten())
    {
        if !options.contains(&value) {
            options.push(value);
        }
        if options.len() == MAX_OPTIONS {
            break;
        }
    }
    options
}

/// The fields of the variable dialog.
pub(super) struct VariableForm {
    name: Entity<InputState>,
    kind: Entity<SelectState<Vec<String>>>,
    text: Entity<TextareaState>,
    /// What a query runs on: connection id, database and the connection's
    /// name, `None` when the dashboard has no panel to take one from.
    connection: Option<(i64, String, String)>,
}

const KIND_VALUES: &str = "Values";
const KIND_QUERY: &str = "Query";

impl VariableForm {
    fn new(
        existing: Option<&DashboardVariable>,
        connection: Option<(i64, String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (name, is_query, text) = match existing {
            Some(variable) => match &variable.source {
                VariableSource::Values { values } => {
                    (variable.name.clone(), false, values.join("\n"))
                }
                VariableSource::Query { query, .. } => (variable.name.clone(), true, query.clone()),
            },
            None => (String::new(), false, String::new()),
        };
        let name = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("region");
            state.set_value(name, window, cx);
            state
        });
        let kind = cx.new(|cx| {
            SelectState::new(
                vec![KIND_VALUES.to_string(), KIND_QUERY.to_string()],
                Some(IndexPath::new(usize::from(is_query))),
                window,
                cx,
            )
        });
        let text = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx);
            state.set_value(text, window, cx);
            state
        });
        cx.subscribe(&kind, |_, _, _: &SelectEvent<Vec<String>>, cx| cx.notify())
            .detach();
        Self {
            name,
            kind,
            text,
            connection,
        }
    }

    fn is_query(&self, cx: &App) -> bool {
        self.kind
            .read(cx)
            .selected_value()
            .is_some_and(|kind| kind == KIND_QUERY)
    }

    /// The variable the fields describe, or what is wrong with them.
    fn variable(&self, taken: &[String], cx: &App) -> Result<DashboardVariable, String> {
        let name = self
            .name
            .read(cx)
            .value()
            .trim()
            .trim_start_matches('$')
            .to_string();
        if !is_variable_name(&name) {
            return Err(
                "A name is letters, digits and underscores, and starts with a letter.".to_string(),
            );
        }
        if taken.contains(&name) {
            return Err(format!("The dashboard already has a ${name}."));
        }
        let text = self.text.read(cx).value().to_string();
        let source = if self.is_query(cx) {
            let query = text.trim().to_string();
            if query.is_empty() {
                return Err("Write the query whose first column gives the choices.".to_string());
            }
            let Some((connection_id, database_name, _)) = self.connection.clone() else {
                return Err(
                    "Add a panel first: a query variable runs on a panel's connection.".to_string(),
                );
            };
            VariableSource::Query {
                connection_id,
                database_name,
                query,
            }
        } else {
            let values: Vec<String> = text
                .lines()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect();
            if values.is_empty() {
                return Err("List at least one value, one per line.".to_string());
            }
            VariableSource::Values { values }
        };
        Ok(DashboardVariable {
            name,
            source,
            current: None,
        })
    }
}

impl Render for VariableForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_query = self.is_query(cx);
        let muted = cx.theme().muted_foreground;
        let hint = if is_query {
            match &self.connection {
                Some((_, _, name)) => format!(
                    "The first column of the rows is the choices. Runs read only on {name}, and may use $__timeFilter."
                ),
                None => "A query variable runs on a panel's connection, and this dashboard has no panel yet.".to_string(),
            }
        } else {
            "One value per line.".to_string()
        };
        v_flex()
            .gap_3()
            .child(
                field()
                    .label("Name")
                    .child(Input::new(&self.name).w_full()),
            )
            .child(field().label("Choices").child(Select::new(&self.kind).small()))
            .child(
                field()
                    .label(if is_query { "Query" } else { "Values" })
                    .child(Textarea::new(&self.text).h(px(140.))),
            )
            .child(div().text_xs().text_color(muted).child(hint))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("Panels write $name or ${name} for the value as a string, ${name:raw} for it as it is."),
            )
    }
}
