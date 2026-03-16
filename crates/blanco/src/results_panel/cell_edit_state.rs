use std::collections::{HashMap, HashSet};

use gpui::Entity;
use gpui_component::input::InputState;

/// Format a value for display in table cells, replacing whitespace with visual indicators
/// to maintain table layout while showing multi-line content.
/// - \n (newline) becomes ⏎
/// - \r (carriage return) becomes ␍
/// - \t (tab) becomes ⇥
pub fn format_value_for_display(value: &str) -> String {
    value
        .replace('\r', "␍")
        .replace('\n', "⏎")
        .replace('\t', "⇥")
}

/// Compare two string values as numeric values.
/// Returns ordering treating empty/null as largest (sorts to end for ascending).
pub fn compare_numeric(a: &str, b: &str) -> std::cmp::Ordering {
    let a_parsed = a.parse::<f64>();
    let b_parsed = b.parse::<f64>();

    match (a_parsed, b_parsed) {
        (Ok(a_num), Ok(b_num)) => a_num
            .partial_cmp(&b_num)
            .unwrap_or(std::cmp::Ordering::Equal),
        (Ok(_), Err(_)) => std::cmp::Ordering::Less, // Valid number < invalid
        (Err(_), Ok(_)) => std::cmp::Ordering::Greater, // Invalid > valid number
        (Err(_), Err(_)) => a.cmp(b),                // Both invalid, fall back to string
    }
}

#[derive(Clone, Debug)]
pub struct TableChange {
    pub change_type: ChangeType,
    pub table_name: String,
    pub row_index: usize,
    pub column_index: Option<usize>,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub primary_key_value: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChangeType {
    UpdateCell,
    InsertRow,
    DeleteRow,
}

#[derive(Clone, Debug, Default)]
pub struct CellEditState {
    pub editing_cell: Option<(usize, usize)>,  // (row, col)
    pub expanded_cell: Option<(usize, usize)>, // (row, col) - cell in expanded multi-line mode
    pub original_values: HashMap<(usize, usize), Option<String>>,
    pub edited_values: HashMap<(usize, usize), Option<String>>,
    pub pending_new_rows: Vec<usize>, // Track rows that are newly added
    pub pending_deleted_rows: HashSet<usize>, // Track rows marked for deletion
    pub editing_input: Option<Entity<InputState>>, // Store input state per delegate
    pub changes: Vec<TableChange>,    // Track all changes for SQL generation
    pub selected_rows: HashSet<usize>, // Track selected rows
    pub current_column: usize,        // Track current column for selection
}

impl CellEditState {
    pub fn is_editing(&self, row: usize, col: usize) -> bool {
        self.editing_cell == Some((row, col))
    }

    pub fn is_edited(&self, row: usize, col: usize) -> bool {
        self.edited_values.contains_key(&(row, col))
    }

    pub fn is_expanded(&self, row: usize, col: usize) -> bool {
        self.expanded_cell == Some((row, col))
    }

    pub fn toggle_expanded(&mut self, row: usize, col: usize) {
        if self.expanded_cell == Some((row, col)) {
            tracing::info!("Collapsing cell at ({}, {})", row, col);
            self.expanded_cell = None;
        } else {
            tracing::info!("Expanding cell at ({}, {})", row, col);
            self.expanded_cell = Some((row, col));
        }
    }

    pub fn get_edited_value(&self, row: usize, col: usize) -> Option<&Option<String>> {
        self.edited_values.get(&(row, col))
    }

    #[allow(dead_code)]
    pub fn get_original_value(&self, row: usize, col: usize) -> Option<&Option<String>> {
        self.original_values.get(&(row, col))
    }

    pub fn start_editing(&mut self, row: usize, col: usize, input: Entity<InputState>) {
        self.editing_cell = Some((row, col));
        self.editing_input = Some(input);
    }

    pub fn stop_editing(&mut self) {
        self.editing_cell = None;
        self.editing_input = None;
        self.expanded_cell = None;
    }

    pub fn get_editing_input(&self) -> Option<Entity<InputState>> {
        self.editing_input.clone()
    }

    pub fn has_unsaved_changes(&self) -> bool {
        !self.edited_values.is_empty()
            || !self.pending_new_rows.is_empty()
            || !self.pending_deleted_rows.is_empty()
            || !self.changes.is_empty()
    }

    pub fn clear_edits(&mut self) {
        self.edited_values.clear();
        self.original_values.clear();
        self.pending_new_rows.clear();
        self.pending_deleted_rows.clear();
        self.editing_cell = None;
        self.editing_input = None;
        self.expanded_cell = None;
    }

    pub fn clear_all(&mut self) {
        self.editing_cell = None;
        self.original_values.clear();
        self.edited_values.clear();
        self.pending_new_rows.clear();
        self.pending_deleted_rows.clear();
        self.editing_input = None;
        self.expanded_cell = None;
        self.changes.clear();
        self.selected_rows.clear();
        self.current_column = 1; // Start with first data column
    }

    pub fn add_change(&mut self, change: TableChange) {
        // For UpdateCell changes, replace any existing change for the same cell
        // This prevents duplicate SET clauses for the same column
        if change.change_type == ChangeType::UpdateCell
            && let Some(col_idx) = change.column_index
        {
            // Remove any existing UpdateCell change for this same (row, column) combination
            self.changes.retain(|existing_change| {
                existing_change.change_type != ChangeType::UpdateCell
                    || existing_change.row_index != change.row_index
                    || existing_change.column_index != Some(col_idx)
            });
        }
        self.changes.push(change);
    }

    pub fn clear_changes(&mut self) {
        self.changes.clear();
        self.edited_values.clear();
        self.original_values.clear();
        self.pending_new_rows.clear();
        self.pending_deleted_rows.clear();
    }

    pub fn is_new_row(&self, row_index: usize) -> bool {
        self.pending_new_rows.contains(&row_index)
    }

    pub fn is_row_deleted(&self, row_index: usize) -> bool {
        self.pending_deleted_rows.contains(&row_index)
    }

    pub fn clear_selection(&mut self) {
        self.selected_rows.clear();
    }
}

/// Builder for creating TableChange instances
pub struct TableChangeBuilder {
    change_type: ChangeType,
    table_name: String,
    row_index: usize,
    column_index: Option<usize>,
    old_value: Option<String>,
    new_value: Option<String>,
    primary_key_value: Option<String>,
    insert_values: Option<Vec<Option<String>>>,
}

impl TableChangeBuilder {
    pub fn new(change_type: ChangeType, table_name: String, row_index: usize) -> Self {
        Self {
            change_type,
            table_name,
            row_index,
            column_index: None,
            old_value: None,
            new_value: None,
            primary_key_value: None,
            insert_values: None,
        }
    }

    pub fn column_index(mut self, column_index: Option<usize>) -> Self {
        self.column_index = column_index;
        self
    }

    pub fn old_value(mut self, old_value: Option<String>) -> Self {
        self.old_value = old_value;
        self
    }

    pub fn new_value(mut self, new_value: Option<String>) -> Self {
        self.new_value = new_value;
        self
    }

    pub fn primary_key_value(mut self, primary_key_value: Option<String>) -> Self {
        self.primary_key_value = primary_key_value;
        self
    }

    pub fn insert_values(mut self, values: Option<Vec<Option<String>>>) -> Self {
        self.insert_values = values;
        self
    }

    pub fn build(self) -> TableChange {
        TableChange {
            change_type: self.change_type,
            table_name: self.table_name,
            row_index: self.row_index,
            column_index: self.column_index,
            old_value: self.old_value,
            new_value: self.new_value,
            primary_key_value: self.primary_key_value,
        }
    }
}

impl TableChange {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        change_type: ChangeType,
        table_name: String,
        row_index: usize,
        column_index: Option<usize>,
        old_value: Option<String>,
        new_value: Option<String>,
        primary_key_value: Option<String>,
        insert_values: Option<Vec<Option<String>>>,
    ) -> Self {
        TableChangeBuilder::new(change_type, table_name, row_index)
            .column_index(column_index)
            .old_value(old_value)
            .new_value(new_value)
            .primary_key_value(primary_key_value)
            .insert_values(insert_values)
            .build()
    }
}
