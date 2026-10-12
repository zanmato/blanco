//! The filter bar above a table's rows. It turns column, operator and value
//! rows, a typed `WHERE` condition or an any-column search into a predicate
//! that `ResultsPanel` adds to the result's query and runs on the server.

use std::rc::Rc;

use anyhow::Result;
use blanco_core::{ColumnType, Dialect, FilterCondition, FilterJoin, FilterOperator};
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyBinding, ParentElement, Render, SharedString, Styled,
    Subscription, Task, Window, actions, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, IconName, IndexPath, Selectable as _, Sizable as _,
    button::{Button, ButtonGroup, ButtonVariants as _},
    h_flex,
    input::{
        CompletionProvider, Editor, EditorState, Input, InputEvent, InputState, Rope, RopeExt as _,
    },
    select::{Select, SelectEvent, SelectState},
    v_flex,
};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Range, TextEdit,
};

/// Key context of the filter bar, which its shortcuts are limited to.
pub const FILTER_BAR_CONTEXT: &str = "ResultsFilterBar";

actions!(
    results_filter_bar,
    [
        /// Add a condition row to the filter bar.
        AddFilterCondition,
        /// Remove the condition row that has focus.
        RemoveFilterCondition,
    ]
);

/// The fixed shortcuts of the bar. Its toggle is a rebindable app action.
pub fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("alt-enter", AddFilterCondition, Some(FILTER_BAR_CONTEXT)),
        KeyBinding::new(
            "alt-shift-backspace",
            RemoveFilterCondition,
            Some(FILTER_BAR_CONTEXT),
        ),
    ]
}

/// How the bar describes the rows it keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterMode {
    /// Column, operator and value rows.
    Conditions,
    /// A `WHERE` condition typed as SQL.
    Where,
    /// A search of every text column.
    AnyColumn,
}

pub enum FilterBarEvent {
    /// Run the result's query with this predicate, or without a filter when
    /// `None`.
    Apply(Option<String>),
    /// The user picked another operator, which new rows now start with.
    DefaultOperatorChanged(FilterOperator),
    /// Copy the SQL the current view runs.
    CopySql,
    /// Hide the bar.
    Close,
}

/// One result column as the bar knows it.
#[derive(Clone, Debug)]
pub struct FilterColumn {
    pub name: String,
    pub column_type: ColumnType,
}

impl FilterColumn {
    fn is_text(&self) -> bool {
        matches!(
            self.column_type,
            ColumnType::Text | ColumnType::Json | ColumnType::Uuid
        )
    }
}

struct ConditionRow {
    column: Entity<SelectState<Vec<String>>>,
    operator: Entity<SelectState<Vec<String>>>,
    value: Entity<InputState>,
}

pub struct FilterBar {
    dialect: Dialect,
    columns: Vec<FilterColumn>,
    mode: FilterMode,
    join: FilterJoin,
    default_operator: FilterOperator,
    rows: Vec<ConditionRow>,
    where_editor: Entity<EditorState>,
    any_column_input: Entity<InputState>,
    /// What the last run reported: an error, or the number of rows.
    status: Option<FilterStatus>,
    running: bool,
    /// A filter is applied to the rows on screen.
    active: bool,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

enum FilterStatus {
    Error(SharedString),
    Rows(usize),
}

impl EventEmitter<FilterBarEvent> for FilterBar {}

impl FilterBar {
    pub fn new(
        dialect: Dialect,
        columns: Vec<FilterColumn>,
        default_operator: FilterOperator,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let names = columns.iter().map(|column| column.name.clone()).collect();
        let where_editor = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx)
                .language("sql")
                .single_line(true)
                .placeholder("WHERE condition, e.g. status = 'open' AND total > 100");
            editor.lsp_mut().completion_provider =
                Some(Rc::new(ColumnCompletionProvider { columns: names }));
            editor
        });
        let any_column_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search all text columns"));
        let subscriptions = vec![
            cx.subscribe_in(&where_editor, window, Self::on_input_event),
            cx.subscribe_in(&any_column_input, window, Self::on_input_event),
        ];

        let mut bar = Self {
            dialect,
            columns,
            mode: FilterMode::Conditions,
            join: FilterJoin::And,
            default_operator,
            rows: Vec::new(),
            where_editor,
            any_column_input,
            status: None,
            running: false,
            active: false,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        bar.add_condition(window, cx);
        bar
    }

    fn on_input_event<E>(
        &mut self,
        _input: &Entity<E>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let InputEvent::PressEnter { .. } = event {
            self.apply(window, cx);
        }
    }

    /// Add an empty condition row on the first column and focus it.
    pub fn add_condition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let column = self.rows.len().min(self.columns.len().saturating_sub(1));
        self.push_condition(column, self.default_operator, "", window, cx);
        if let Some(row) = self.rows.last() {
            row.column.focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    fn push_condition(
        &mut self,
        column: usize,
        operator: FilterOperator,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let names: Vec<String> = self.columns.iter().map(|c| c.name.clone()).collect();
        let column_select = cx.new(|cx| {
            SelectState::new(names, Some(IndexPath::new(column)), window, cx).searchable(true)
        });
        let operators: Vec<String> = FilterOperator::ALL
            .iter()
            .map(|operator| operator.label().to_string())
            .collect();
        let operator_ix = FilterOperator::ALL
            .iter()
            .position(|candidate| *candidate == operator)
            .unwrap_or(0);
        let operator_select =
            cx.new(|cx| SelectState::new(operators, Some(IndexPath::new(operator_ix)), window, cx));
        let value_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Value")
                .default_value(value)
        });
        self._subscriptions.extend([
            cx.subscribe_in(&value_input, window, Self::on_input_event),
            cx.subscribe(
                &operator_select,
                |bar, select, event: &SelectEvent<Vec<String>>, cx| {
                    let SelectEvent::Confirm(Some(_)) = event else {
                        return;
                    };
                    if let Some(operator) = selected_operator(&select.read(cx).selected_index(cx))
                        && operator != bar.default_operator
                    {
                        bar.default_operator = operator;
                        cx.emit(FilterBarEvent::DefaultOperatorChanged(operator));
                    }
                    cx.notify();
                },
            ),
        ]);
        self.rows.push(ConditionRow {
            column: column_select,
            operator: operator_select,
            value: value_input,
        });
    }

    pub fn remove_condition(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.rows.len() {
            self.rows.remove(ix);
        }
        if self.rows.is_empty() {
            self.add_condition(window, cx);
        }
        cx.notify();
    }

    fn on_add_condition(
        &mut self,
        _: &AddFilterCondition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_mode(FilterMode::Conditions, cx);
        self.add_condition(window, cx);
    }

    fn on_remove_condition(
        &mut self,
        _: &RemoveFilterCondition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focused = self.rows.iter().position(|row| {
            row.value.focus_handle(cx).contains_focused(window, cx)
                || row.column.focus_handle(cx).contains_focused(window, cx)
                || row.operator.focus_handle(cx).contains_focused(window, cx)
        });
        if let Some(ix) = focused {
            self.remove_condition(ix, window, cx);
            let next = ix.min(self.rows.len().saturating_sub(1));
            if let Some(row) = self.rows.get(next) {
                row.value.focus_handle(cx).focus(window, cx);
            }
        }
    }

    pub fn set_mode(&mut self, mode: FilterMode, cx: &mut Context<Self>) {
        if self.mode != mode {
            self.mode = mode;
            self.status = None;
            cx.notify();
        }
    }

    /// Replace the conditions with one for `column` matching `value`, as the
    /// cell menu's Filter by Value does, and apply it. A NULL cell filters on
    /// `IS NULL`.
    pub fn filter_by_value(
        &mut self,
        column: &str,
        value: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(column_ix) = self.columns.iter().position(|c| c.name == column) else {
            return;
        };
        let operator = match value {
            Some(_) => FilterOperator::Equals,
            None => FilterOperator::IsNull,
        };
        self.mode = FilterMode::Conditions;
        self.rows.clear();
        self.push_condition(column_ix, operator, value.unwrap_or_default(), window, cx);
        self.apply(window, cx);
    }

    /// The conditions as entered, skipping rows whose value is still empty
    /// where the operator needs one.
    fn conditions(&self, cx: &App) -> Vec<FilterCondition> {
        self.rows
            .iter()
            .filter_map(|row| {
                let column = row
                    .column
                    .read(cx)
                    .selected_index(cx)
                    .and_then(|ix| self.columns.get(ix.row))?;
                let operator = selected_operator(&row.operator.read(cx).selected_index(cx))?;
                let value = row.value.read(cx).value().to_string();
                if operator.takes_value() && value.trim().is_empty() {
                    return None;
                }
                Some(FilterCondition {
                    column: column.name.clone(),
                    operator,
                    value,
                    numeric: column.column_type.is_numeric(),
                    binary: column.column_type == blanco_core::ColumnType::Binary,
                    geometry: column.column_type == blanco_core::ColumnType::Geometry,
                })
            })
            .collect()
    }

    /// The predicate for the current mode, `None` when nothing is entered.
    fn predicate(&self, cx: &App) -> Result<Option<String>, String> {
        match self.mode {
            FilterMode::Conditions => {
                let conditions = self.conditions(cx);
                Ok((!conditions.is_empty())
                    .then(|| self.dialect.filter_conditions(&conditions, self.join)))
            }
            FilterMode::Where => {
                let text = self.where_editor.read(cx).value().trim().to_string();
                Ok((!text.is_empty()).then_some(text))
            }
            FilterMode::AnyColumn => {
                let needle = self.any_column_input.read(cx).value().to_string();
                if needle.is_empty() {
                    return Ok(None);
                }
                let text_columns: Vec<String> = self
                    .columns
                    .iter()
                    .filter(|column| column.is_text())
                    .map(|column| column.name.clone())
                    .collect();
                if text_columns.is_empty() {
                    return Err("This result has no text columns to search".to_string());
                }
                Ok(Some(
                    self.dialect.any_column_contains(&text_columns, &needle),
                ))
            }
        }
    }

    pub fn apply(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        match self.predicate(cx) {
            Ok(predicate) => {
                self.active = predicate.is_some();
                self.running = true;
                self.status = None;
                cx.emit(FilterBarEvent::Apply(predicate));
            }
            Err(error) => self.status = Some(FilterStatus::Error(error.into())),
        }
        cx.notify();
    }

    /// Clear what is entered in the current mode and show every row again.
    fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.mode {
            FilterMode::Conditions => {
                self.rows.clear();
                self.add_condition(window, cx);
            }
            FilterMode::Where => self
                .where_editor
                .update(cx, |editor, cx| editor.set_value("", window, cx)),
            FilterMode::AnyColumn => self
                .any_column_input
                .update(cx, |input, cx| input.set_value("", window, cx)),
        }
        if self.active {
            self.active = false;
            self.running = true;
            self.status = None;
            cx.emit(FilterBarEvent::Apply(None));
        }
        cx.notify();
    }

    #[cfg(test)]
    pub fn apply_where(&mut self, condition: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.set_mode(FilterMode::Where, cx);
        self.where_editor
            .update(cx, |editor, cx| editor.set_value(condition, window, cx));
        self.apply(window, cx);
    }

    #[cfg(test)]
    pub fn apply_any_column(&mut self, needle: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.set_mode(FilterMode::AnyColumn, cx);
        self.any_column_input
            .update(cx, |input, cx| input.set_value(needle, window, cx));
        self.apply(window, cx);
    }

    #[cfg(test)]
    pub fn is_running(&self) -> bool {
        self.running
    }

    #[cfg(test)]
    pub fn error(&self) -> Option<SharedString> {
        match &self.status {
            Some(FilterStatus::Error(error)) => Some(error.clone()),
            _ => None,
        }
    }

    /// Report how a run went.
    pub fn finish(&mut self, outcome: Result<usize, String>, cx: &mut Context<Self>) {
        self.running = false;
        self.status = Some(match outcome {
            Ok(rows) => FilterStatus::Rows(rows),
            Err(error) => {
                self.active = false;
                FilterStatus::Error(error.into())
            }
        });
        cx.notify();
    }

    fn render_condition_row(
        &self,
        ix: usize,
        row: &ConditionRow,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let takes_value = selected_operator(&row.operator.read(cx).selected_index(cx))
            .is_none_or(FilterOperator::takes_value);
        h_flex()
            .gap_1()
            .items_center()
            .child(
                div()
                    .w(px(56.))
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(match (ix, self.join) {
                        (0, _) => "WHERE",
                        (_, FilterJoin::And) => "AND",
                        (_, FilterJoin::Or) => "OR",
                    }),
            )
            .child(
                div()
                    .w(px(180.))
                    .child(Select::new(&row.column).xsmall().menu_width(px(240.))),
            )
            .child(
                div()
                    .w(px(140.))
                    .child(Select::new(&row.operator).xsmall().menu_width(px(180.))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(120.))
                    .child(Input::new(&row.value).xsmall().disabled(!takes_value)),
            )
            .child(
                Button::new(("remove-filter-condition", ix))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Minus)
                    .tooltip("Remove Condition")
                    .on_click(cx.listener(move |bar, _, window, cx| {
                        bar.remove_condition(ix, window, cx);
                    })),
            )
            .into_any_element()
    }
}

fn selected_operator(index: &Option<IndexPath>) -> Option<FilterOperator> {
    index.and_then(|ix| FilterOperator::ALL.get(ix.row).copied())
}

impl Focusable for FilterBar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for FilterBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.mode;
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        // Same layering as the result tab strip: `theme.table` underneath and
        // `theme.table_head` on top, so the bar composites like the active
        // tab above it and the column header below it.
        let table = theme.table;
        let table_head = theme.table_head;
        let mode_button = |id: &'static str, label: &'static str, target: FilterMode| {
            Button::new(id)
                .label(label)
                .xsmall()
                .selected(mode == target)
        };

        let bar = v_flex()
            .key_context(FILTER_BAR_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_add_condition))
            .on_action(cx.listener(Self::on_remove_condition))
            .w_full()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(theme.border)
            .bg(table_head)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        ButtonGroup::new("filter-mode")
                            .outline()
                            .xsmall()
                            .child(mode_button(
                                "filter-mode-conditions",
                                "Conditions",
                                FilterMode::Conditions,
                            ))
                            .child(mode_button("filter-mode-where", "WHERE", FilterMode::Where))
                            .child(mode_button(
                                "filter-mode-any",
                                "Any Column",
                                FilterMode::AnyColumn,
                            ))
                            .on_click(cx.listener(|bar, selected: &Vec<usize>, _, cx| {
                                let mode = match selected.first() {
                                    Some(1) => FilterMode::Where,
                                    Some(2) => FilterMode::AnyColumn,
                                    _ => FilterMode::Conditions,
                                };
                                bar.set_mode(mode, cx);
                            })),
                    )
                    .when(
                        mode == FilterMode::Conditions && self.rows.len() > 1,
                        |this| {
                            let join = self.join;
                            this.child(
                                ButtonGroup::new("filter-join")
                                    .outline()
                                    .xsmall()
                                    .child(
                                        Button::new("filter-join-and")
                                            .label("Match All")
                                            .selected(join == FilterJoin::And),
                                    )
                                    .child(
                                        Button::new("filter-join-or")
                                            .label("Match Any")
                                            .selected(join == FilterJoin::Or),
                                    )
                                    .on_click(cx.listener(|bar, selected: &Vec<usize>, _, cx| {
                                        bar.join = match selected.first() {
                                            Some(1) => FilterJoin::Or,
                                            _ => FilterJoin::And,
                                        };
                                        cx.notify();
                                    })),
                            )
                        },
                    )
                    .child(div().flex_1())
                    .when_some(self.status.as_ref(), |this, status| {
                        this.child(match status {
                            FilterStatus::Error(error) => div()
                                .text_xs()
                                .text_color(danger)
                                .truncate()
                                .max_w(px(420.))
                                .child(error.clone()),
                            FilterStatus::Rows(rows) => {
                                div().text_xs().text_color(muted).child(match rows {
                                    1 => "1 row".to_string(),
                                    rows => format!("{rows} rows"),
                                })
                            }
                        })
                    })
                    .child(
                        Button::new("filter-copy-sql")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Copy)
                            .tooltip("Copy the SQL of This View")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(FilterBarEvent::CopySql))),
                    )
                    .child(
                        Button::new("filter-clear")
                            .ghost()
                            .xsmall()
                            .label("Clear")
                            .disabled(self.running)
                            .on_click(cx.listener(|bar, _, window, cx| bar.clear(window, cx))),
                    )
                    .child(
                        Button::new("filter-apply")
                            .primary()
                            .xsmall()
                            .label("Apply")
                            .loading(self.running)
                            .on_click(cx.listener(|bar, _, window, cx| bar.apply(window, cx))),
                    )
                    .child(
                        Button::new("filter-close")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip("Hide Filter Bar")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(FilterBarEvent::Close))),
                    ),
            )
            .map(|this| match mode {
                FilterMode::Conditions => this
                    .children(
                        self.rows
                            .iter()
                            .enumerate()
                            .map(|(ix, row)| self.render_condition_row(ix, row, cx))
                            .collect::<Vec<_>>(),
                    )
                    .child(
                        h_flex().pl(px(60.)).child(
                            Button::new("add-filter-condition")
                                .ghost()
                                .xsmall()
                                .icon(IconName::Plus)
                                .label("Add Condition")
                                .on_click(
                                    cx.listener(|bar, _, window, cx| bar.add_condition(window, cx)),
                                ),
                        ),
                    ),
                FilterMode::Where => this.child(
                    div()
                        .w_full()
                        .border_1()
                        .border_color(cx.theme().input)
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().background)
                        .px_2()
                        .child(Editor::new(&self.where_editor).bordered(false)),
                ),
                FilterMode::AnyColumn => this.child(
                    Input::new(&self.any_column_input).xsmall().prefix(
                        gpui_component::Icon::new(IconName::Search)
                            .xsmall()
                            .text_color(muted),
                    ),
                ),
            });

        div().w_full().bg(table).child(bar)
    }
}

/// Completes the result's column names in the `WHERE` field.
struct ColumnCompletionProvider {
    columns: Vec<String>,
}

impl ColumnCompletionProvider {
    fn items(&self, text: &str, offset: usize) -> (Vec<CompletionItem>, usize) {
        let offset = offset.min(text.len());
        let left = &text[..offset];
        let start = left
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
            .map_or(0, |ix| ix + 1);
        let word = left[start..].to_lowercase();
        let items = self
            .columns
            .iter()
            .filter(|column| column.to_lowercase().starts_with(&word))
            .map(|column| CompletionItem {
                label: column.clone(),
                kind: Some(CompletionItemKind::FIELD),
                ..Default::default()
            })
            .collect();
        (items, start)
    }
}

impl CompletionProvider for ColumnCompletionProvider {
    fn completions(
        &self,
        rope: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let text = rope.to_string();
        let (mut items, start) = self.items(&text, offset);
        let range = Range::new(
            rope.offset_to_position(start),
            rope.offset_to_position(offset),
        );
        for item in &mut items {
            item.text_edit = Some(CompletionTextEdit::Edit(TextEdit::new(
                range,
                item.label.clone(),
            )));
        }
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _cx: &mut App) -> bool {
        new_text.chars().all(|c| c.is_alphanumeric() || c == '_')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_completion_matches_the_word_being_typed() {
        let provider = ColumnCompletionProvider {
            columns: vec!["id".into(), "Name".into(), "name_lower".into()],
        };
        let labels = |text: &str| {
            let (items, start) = provider.items(text, text.len());
            (
                items.into_iter().map(|item| item.label).collect::<Vec<_>>(),
                start,
            )
        };
        assert_eq!(
            labels("total > 1 AND na"),
            (vec!["Name".to_string(), "name_lower".to_string()], 14)
        );
        assert_eq!(labels("").0.len(), 3);
        assert!(labels("x = 1 AND zz").0.is_empty());
    }
}
