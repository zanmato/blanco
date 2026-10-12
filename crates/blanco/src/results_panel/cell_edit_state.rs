use std::collections::{HashMap, HashSet};

use gpui::{App, Entity, Pixels, Point, Size, px, size};
use gpui_component::input::{EditorState, InputState, InvisibleCharacter, invisible_character};

use super::binary_viewer::BinaryViewer;
use super::date_time_picker::DateTimePicker;
use super::key_value_editor::KeyValueEditor;

/// Starting size of the expanded cell editor, before the user drags it.
pub const EXPANDED_EDITOR_SIZE: Size<Pixels> = Size {
    width: px(600.),
    height: px(200.),
};

/// Smallest the expanded cell editor can be dragged to. Below this the gutter
/// and the minimize affordance crowd out the text.
const EXPANDED_EDITOR_MIN_SIZE: Size<Pixels> = Size {
    width: px(240.),
    height: px(80.),
};

/// The size of the expanded cell editor, and the drag that is changing it.
///
/// The overlay is anchored to the cell's top-right corner, so the grip sits on
/// the opposite (bottom-left) corner: dragging left widens it and dragging down
/// makes it taller. Sizes accumulate from pointer deltas rather than from the
/// pointer position, so the grip does not jump to the cursor on the first move.
#[derive(Clone, Debug)]
pub struct ExpandedEditorSize {
    size: Size<Pixels>,
    last_drag_position: Option<Point<Pixels>>,
}

impl Default for ExpandedEditorSize {
    fn default() -> Self {
        Self {
            size: EXPANDED_EDITOR_SIZE,
            last_drag_position: None,
        }
    }
}

impl ExpandedEditorSize {
    pub fn size(&self) -> Size<Pixels> {
        self.size
    }

    /// Begin a drag, so the next move measures from where the pointer is now.
    pub fn start_drag(&mut self) {
        self.last_drag_position = None;
    }

    /// Grow or shrink by how far the pointer moved since the last call.
    pub fn drag_to(&mut self, position: Point<Pixels>) {
        if let Some(last) = self.last_drag_position {
            self.size = size(
                (self.size.width + (last.x - position.x)).max(EXPANDED_EDITOR_MIN_SIZE.width),
                (self.size.height + (position.y - last.y)).max(EXPANDED_EDITOR_MIN_SIZE.height),
            );
        }
        self.last_drag_position = Some(position);
    }
}

/// The state behind the cell editor.
///
/// A cell edits inline in a single-line input, and expands into a larger
/// multi-line editor. Those are two different state types, so the cell holds
/// whichever one it currently shows; maximize and minimize swap it.
#[derive(Clone, Debug)]
pub enum CellInput {
    Inline(Entity<InputState>),
    Expanded(Entity<EditorState>),
}

impl CellInput {
    pub fn text(&self, cx: &App) -> String {
        match self {
            Self::Inline(input) => input.read(cx).text().to_string(),
            Self::Expanded(editor) => editor.read(cx).text().to_string(),
        }
    }
}

/// The most characters of a value a cell draws. More than the widest column
/// shows at the grid font, so cells look the same, but a value of megabytes
/// costs no more to format and lay out on every frame than a short one.
pub const DISPLAY_CHARACTERS: usize = 1000;

/// A cell's text and the byte ranges in it that mark invisible characters,
/// which the grid tints.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DisplayText {
    pub text: String,
    pub markers: Vec<std::ops::Range<usize>>,
}

impl From<String> for DisplayText {
    fn from(text: String) -> Self {
        Self {
            text,
            markers: Vec::new(),
        }
    }
}

/// A value as a cell shows it on one line: line breaks and tabs become
/// `⏎`, `␍` and `⇥`, and only the first [`DISPLAY_CHARACTERS`] are formatted,
/// a longer value ending in `…`. With `show_invisible` it also marks
/// characters that would otherwise look like nothing or like a plain space,
/// by the rule the editor boxes them with (`invisible_character`):
/// other control characters by code point (`U+0007`), zero width characters
/// and the byte order mark by name (`ZWSP`, `BOM`), Unicode spaces and the
/// spaces that end the value as themselves. Each mark is a range of
/// [`DisplayText::markers`].
pub fn display_text(value: &str, show_invisible: bool) -> DisplayText {
    let (shown, cut) = match value.char_indices().nth(DISPLAY_CHARACTERS) {
        Some((end, _)) => (&value[..end], true),
        None => (value, false),
    };
    display_text_of(shown, cut, show_invisible)
}

/// [`display_text`] for the first `characters` of a value known to go on
/// past `preview`, as a deferred value's preview does: it always ends in `…`
/// and its last spaces are not the value's.
pub fn display_preview(preview: &str, characters: usize, show_invisible: bool) -> DisplayText {
    let shown = match preview.char_indices().nth(characters) {
        Some((end, _)) => &preview[..end],
        None => preview,
    };
    display_text_of(shown, true, show_invisible)
}

fn display_text_of(shown: &str, cut: bool, show_invisible: bool) -> DisplayText {
    // Spaces that end the value. A value cut short ends past what is shown.
    let trailing_spaces = if show_invisible && !cut {
        shown.trim_end_matches(' ').len()
    } else {
        shown.len()
    };
    let mut text = String::with_capacity(shown.len() + '…'.len_utf8());
    let mut markers = Vec::new();
    let mut mark = |text: &mut String, shown_as: &str| {
        let start = text.len();
        text.push_str(shown_as);
        markers.push(start..text.len());
    };
    for (ix, character) in shown.char_indices() {
        match character {
            '\r' => text.push('␍'),
            '\n' => text.push('⏎'),
            '\t' => text.push('⇥'),
            _ if !show_invisible => text.push(character),
            ' ' if ix >= trailing_spaces => mark(&mut text, " "),
            character => match invisible_character(character) {
                None => text.push(character),
                Some(InvisibleCharacter::Space) => {
                    mark(&mut text, character.encode_utf8(&mut [0; 4]))
                }
                Some(InvisibleCharacter::Named(name)) => mark(&mut text, name),
                Some(InvisibleCharacter::CodePoint) => {
                    mark(&mut text, &format!("U+{:04X}", u32::from(character)))
                }
            },
        }
    }
    if cut {
        text.push('…');
    }
    DisplayText { text, markers }
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
    pub primary_key_values: Vec<(String, Option<String>)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChangeType {
    UpdateCell,
    InsertRow,
    DeleteRow,
}

/// The GeoJSON of the WKT in an expanded geometry cell, or why the WKT does
/// not parse.
#[derive(Debug, Clone, PartialEq)]
pub enum GeometryPreview {
    GeoJson(String),
    Invalid(String),
}

impl GeometryPreview {
    /// `None` for what PostGIS reads but the parser does not: hex EWKB and
    /// curve types.
    pub fn of(text: &str) -> Option<Self> {
        if let Some(error) = blanco_core::geometry::edit_error(text) {
            return Some(Self::Invalid(error));
        }
        let geometry = blanco_core::geometry::parse_ewkt(text).ok()?;
        serde_json::to_string_pretty(&geometry.to_geojson())
            .ok()
            .map(Self::GeoJson)
    }
}

#[derive(Clone, Debug, Default)]
pub struct CellEditState {
    pub editing_cell: Option<(usize, usize)>,  // (row, col)
    pub expanded_cell: Option<(usize, usize)>, // (row, col) - cell in expanded multi-line mode
    pub original_values: HashMap<(usize, usize), Option<String>>,
    pub edited_values: HashMap<(usize, usize), Option<String>>,
    /// Cells whose pending value is SQL for the server to evaluate rather
    /// than data to quote.
    pub expression_cells: HashSet<(usize, usize)>,
    /// The expression toggle of the cell being edited. It only reaches
    /// `expression_cells` when the edit is committed, so cancelling leaves the
    /// cell's pending value and its kind in step.
    pub editing_as_expression: bool,
    pub pending_new_rows: Vec<usize>, // Track rows that are newly added
    pub pending_deleted_rows: HashSet<usize>, // Track rows marked for deletion
    pub editing_input: Option<CellInput>, // Store input state per delegate
    /// Size of the expanded editor overlay, kept while a cell stays expanded.
    pub expanded_size: ExpandedEditorSize,
    /// The expanded editor holds JSON: a JSON column, or a text value that
    /// is a JSON object or array. It then offers Format and Minify.
    pub expanded_as_json: bool,
    /// Why the expanded JSON text does not parse, shown under the editor.
    pub json_error: Option<String>,
    /// What the expanded editor of a geometry cell shows under its WKT.
    pub geometry_preview: Option<GeometryPreview>,
    /// The open date and time picker of the cell being edited.
    pub date_picker: Option<Entity<DateTimePicker>>,
    /// The list of enum labels of the cell being edited is open.
    pub enum_picker_open: bool,
    /// The viewer a binary cell is expanded into. It takes the place of the
    /// text editors, so `editing_input` is empty while it is open.
    pub binary_viewer: Option<Entity<BinaryViewer>>,
    /// The pair editor an hstore cell is expanded into, which also takes the
    /// place of the text editors.
    pub key_value_editor: Option<Entity<KeyValueEditor>>,
    pub changes: Vec<TableChange>, // Track all changes for SQL generation
    pub selected_rows: HashSet<usize>, // Track selected rows
    pub current_column: usize,     // Track current column for selection
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
            tracing::debug!("Collapsing cell at ({}, {})", row, col);
            self.expanded_cell = None;
            self.expanded_size = ExpandedEditorSize::default();
        } else {
            tracing::debug!("Expanding cell at ({}, {})", row, col);
            self.expanded_cell = Some((row, col));
        }
    }

    pub fn is_expression(&self, row: usize, col: usize) -> bool {
        self.expression_cells.contains(&(row, col))
    }

    pub fn get_edited_value(&self, row: usize, col: usize) -> Option<&Option<String>> {
        self.edited_values.get(&(row, col))
    }

    pub fn start_editing(&mut self, row: usize, col: usize, input: CellInput) {
        self.editing_cell = Some((row, col));
        self.editing_input = Some(input);
        self.editing_as_expression = self.is_expression(row, col);
        self.date_picker = None;
        self.enum_picker_open = false;
    }

    pub fn stop_editing(&mut self) {
        self.editing_cell = None;
        self.editing_input = None;
        self.expanded_cell = None;
        self.date_picker = None;
        self.enum_picker_open = false;
        self.binary_viewer = None;
        self.key_value_editor = None;
        self.editing_as_expression = false;
    }

    /// Whether the cell is edited in the binary viewer or the pair editor
    /// rather than a text input.
    pub fn has_overlay_editor(&self) -> bool {
        self.binary_viewer.is_some() || self.key_value_editor.is_some()
    }

    pub fn get_editing_input(&self) -> Option<CellInput> {
        self.editing_input.clone()
    }

    /// Reset every trace of pending edits after they have been persisted.
    /// `changes` must go with them: it is the list the SQL is generated from,
    /// so leaving it populated makes the next commit replay already-applied
    /// edits even though the cells no longer render as uncommitted.
    pub fn clear_edits(&mut self) {
        self.changes.clear();
        self.edited_values.clear();
        self.expression_cells.clear();
        self.original_values.clear();
        self.pending_new_rows.clear();
        self.pending_deleted_rows.clear();
        self.editing_cell = None;
        self.editing_input = None;
        self.expanded_cell = None;
        self.date_picker = None;
        self.enum_picker_open = false;
        self.binary_viewer = None;
    }

    pub fn clear_all(&mut self) {
        self.editing_cell = None;
        self.original_values.clear();
        self.edited_values.clear();
        self.expression_cells.clear();
        self.pending_new_rows.clear();
        self.pending_deleted_rows.clear();
        self.editing_input = None;
        self.expanded_cell = None;
        self.date_picker = None;
        self.enum_picker_open = false;
        self.binary_viewer = None;
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
        self.expression_cells.clear();
        self.original_values.clear();
        self.pending_new_rows.clear();
        self.pending_deleted_rows.clear();
    }

    /// Remove a new row from tracking and reindex all row-dependent state.
    /// After removing a row at `row_index`, all rows above it shift down by 1.
    pub fn remove_new_row(&mut self, row_index: usize) {
        self.changes.retain(|c| c.row_index != row_index);
        for change in &mut self.changes {
            if change.row_index > row_index {
                change.row_index -= 1;
            }
        }

        self.pending_new_rows.retain(|&r| r != row_index);
        for row in &mut self.pending_new_rows {
            if *row > row_index {
                *row -= 1;
            }
        }

        self.edited_values = reindex_row_keys(&self.edited_values, row_index);
        self.original_values = reindex_row_keys(&self.original_values, row_index);
        self.expression_cells = self
            .expression_cells
            .iter()
            .filter(|(row, _)| *row != row_index)
            .map(|&(row, col)| (if row > row_index { row - 1 } else { row }, col))
            .collect();
        self.pending_deleted_rows = reindex_row_set(&self.pending_deleted_rows, row_index);
        self.selected_rows = reindex_row_set(&self.selected_rows, row_index);

        if let Some((r, c)) = self.editing_cell {
            if r == row_index {
                self.editing_cell = None;
            } else if r > row_index {
                self.editing_cell = Some((r - 1, c));
            }
        }
        if let Some((r, c)) = self.expanded_cell {
            if r == row_index {
                self.expanded_cell = None;
                self.binary_viewer = None;
            } else if r > row_index {
                self.expanded_cell = Some((r - 1, c));
            }
        }
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
    primary_key_values: Vec<(String, Option<String>)>,
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
            primary_key_values: Vec::new(),
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

    pub fn primary_key_values(mut self, primary_key_values: Vec<(String, Option<String>)>) -> Self {
        self.primary_key_values = primary_key_values;
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
            primary_key_values: self.primary_key_values,
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
        primary_key_values: Vec<(String, Option<String>)>,
        insert_values: Option<Vec<Option<String>>>,
    ) -> Self {
        TableChangeBuilder::new(change_type, table_name, row_index)
            .column_index(column_index)
            .old_value(old_value)
            .new_value(new_value)
            .primary_key_values(primary_key_values)
            .insert_values(insert_values)
            .build()
    }
}

fn reindex_row_keys(
    map: &HashMap<(usize, usize), Option<String>>,
    removed_row: usize,
) -> HashMap<(usize, usize), Option<String>> {
    map.iter()
        .map(|((row, col), value)| {
            let new_row = if *row > removed_row { *row - 1 } else { *row };
            ((new_row, *col), value.clone())
        })
        .collect()
}

fn reindex_row_set(set: &HashSet<usize>, removed_row: usize) -> HashSet<usize> {
    set.iter()
        .map(|&r| if r > removed_row { r - 1 } else { r })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{DISPLAY_CHARACTERS, display_text};

    fn format_value_for_display(value: &str) -> String {
        display_text(value, false).text
    }

    #[test]
    fn display_marks_line_breaks_and_tabs() {
        assert_eq!(format_value_for_display("a\r\nb\tc"), "a␍⏎b⇥c");
    }

    #[test]
    fn display_stops_after_the_characters_a_cell_can_show() {
        let short = "å".repeat(DISPLAY_CHARACTERS);
        assert_eq!(format_value_for_display(&short), short);

        let long = format!("{short}\nmore");
        assert_eq!(format_value_for_display(&long), format!("{short}…"));
    }

    #[test]
    fn invisible_characters_are_marked_only_when_asked() {
        let value = "a\u{00A0}b\u{200B}c\u{0007}\u{FEFF}d\u{009B}e  ";
        assert_eq!(format_value_for_display(value), value);

        let shown = display_text(value, true);
        assert_eq!(shown.text, "a\u{00A0}bZWSPcU+0007BOMdU+009Be  ");
        let marked: Vec<&str> = shown
            .markers
            .iter()
            .map(|range| &shown.text[range.clone()])
            .collect();
        assert_eq!(
            marked,
            ["\u{00A0}", "ZWSP", "U+0007", "BOM", "U+009B", " ", " "]
        );
    }

    #[test]
    fn inner_spaces_and_spaces_past_the_cut_are_not_marked() {
        assert!(display_text("two words", true).markers.is_empty());
        let long = format!("{} ", "x".repeat(DISPLAY_CHARACTERS));
        assert!(display_text(&long, true).markers.is_empty());
    }
}
