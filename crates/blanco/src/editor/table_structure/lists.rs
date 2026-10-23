//! The structure tab's grids of columns, indexes and foreign keys. Rows
//! are the staged structure, tinted by what the pending changes do to them.

use std::cmp::Ordering;

use app_database::VirtualForeignKey;
use blanco_core::{ColumnInfo, ColumnType, Dialect, ForeignKeyConstraint, IndexInfo};
use gpui::{
    App, Context, Div, Hsla, IntoElement, ParentElement, SharedString, Styled, TextRun, WeakEntity,
    Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _,
    menu::{PopupMenu, PopupMenuItem},
    table::{Column, ColumnSort, TableDelegate, TableState},
};

use super::TableStructureTab;
use super::staging::{RowMark, RowState};
use crate::result_ext::ResultExt as _;

#[derive(Clone)]
pub struct ColumnsTableDelegate {
    /// Names the colours of the type column, as the results grid colours
    /// values of the same types.
    dialect: Dialect,
    columns: Vec<ColumnInfo>,
    pub(super) marks: Vec<RowMark>,
    /// Indices into `columns` in display order, after the search and sort.
    rows: Vec<usize>,
    filter: String,
    sort: Option<(usize, ColumnSort)>,
    column_defs: Vec<Column>,
    editable: bool,
    tab: WeakEntity<TableStructureTab>,
}

impl ColumnsTableDelegate {
    pub fn new(dialect: Dialect, tab: WeakEntity<TableStructureTab>) -> Self {
        let column_defs = vec![
            Column {
                name: "#".into(),
                width: px(40.),
                ..Default::default()
            },
            Column {
                name: "Name".into(),
                width: px(150.),
                ..Default::default()
            },
            Column {
                name: "Type".into(),
                width: px(120.),
                ..Default::default()
            },
            Column {
                name: "Nullable".into(),
                width: px(90.),
                ..Default::default()
            },
            Column {
                name: "Default".into(),
                width: px(100.),
                ..Default::default()
            },
            Column {
                name: "Key".into(),
                width: px(190.),
                ..Default::default()
            },
            Column {
                name: "Comment".into(),
                width: px(200.),
                ..Default::default()
            },
        ];

        let column_defs = column_defs.into_iter().map(Column::sortable).collect();
        Self {
            dialect,
            columns: Vec::new(),
            marks: Vec::new(),
            rows: Vec::new(),
            filter: String::new(),
            sort: None,
            column_defs,
            editable: false,
            tab,
        }
    }

    /// The staged column a displayed row shows, as an index into the tab's
    /// column list.
    pub(super) fn column_at(&self, row_ix: usize) -> Option<usize> {
        self.rows.get(row_ix).copied()
    }

    /// The displayed row of a staged column, if the search shows it.
    pub(super) fn row_of(&self, column_ix: usize) -> Option<usize> {
        self.rows.iter().position(|&ix| ix == column_ix)
    }

    pub(super) fn set_rows(
        &mut self,
        columns: Vec<ColumnInfo>,
        marks: Vec<RowMark>,
        editable: bool,
    ) {
        self.columns = columns;
        self.marks = marks;
        self.editable = editable;
        self.rows = column_rows(&self.columns, &self.filter, self.sort);
    }

    /// Size every column to its widest text, header included.
    pub(super) fn fit_columns(&mut self, window: &Window, cx: &App) {
        let cells: Vec<Vec<String>> = (0..self.columns.len())
            .map(|ix| {
                (0..self.column_defs.len())
                    .map(|col_ix| column_cell(&self.columns[ix], ix, col_ix))
                    .collect()
            })
            .collect();
        fit_widths(&mut self.column_defs, &cells, true, window, cx);
    }

    pub(super) fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        self.rows = column_rows(&self.columns, &self.filter, self.sort);
    }

    fn set_sort(&mut self, col_ix: usize, sort: ColumnSort) {
        // The table asks for column definitions again on refresh, so they
        // carry the sort state.
        for (ix, def) in self.column_defs.iter_mut().enumerate() {
            *def = def.clone().sort(if ix == col_ix {
                sort
            } else {
                ColumnSort::Default
            });
        }
        self.sort = (sort != ColumnSort::Default).then_some((col_ix, sort));
        self.rows = column_rows(&self.columns, &self.filter, self.sort);
    }
}

/// The text a cell of the columns grid shows. `ordinal` is the column's
/// position in the table, which the `#` column keeps under any sort.
fn column_cell(col: &ColumnInfo, ordinal: usize, col_ix: usize) -> String {
    match col_ix {
        0 => (ordinal + 1).to_string(),
        1 => col.name.clone(),
        2 => col.display_type(),
        3 => if col.is_nullable { "Yes" } else { "No" }.to_string(),
        4 => col.default_value.clone().unwrap_or_default(),
        5 => {
            let reference = col
                .foreign_key
                .as_ref()
                .map(|fk| format!("FK {}({})", fk.foreign_table_name, fk.foreign_column_name));
            match (col.is_primary_key, reference) {
                (true, Some(reference)) => format!("PK, {reference}"),
                (true, None) => "PK".to_string(),
                (false, Some(reference)) => reference,
                (false, None) => String::new(),
            }
        }
        6 => col.comment.clone().unwrap_or_default(),
        _ => String::new(),
    }
}

/// The columns grid's rows: indices into `columns` whose name matches
/// `filter` (the sidebar's syntax: `^`, `$` and `|`, ignoring case), ordered
/// by `sort`. Without a sort, or between equal values, rows keep the table's
/// column order.
pub(super) fn column_rows(
    columns: &[ColumnInfo],
    filter: &str,
    sort: Option<(usize, ColumnSort)>,
) -> Vec<usize> {
    let filter = crate::name_filter::NameFilter::parse(filter);
    let mut rows: Vec<usize> = (0..columns.len())
        .filter(|&ix| filter.is_empty() || filter.matches(&columns[ix].name))
        .collect();
    let Some((col_ix, sort)) = sort else {
        return rows;
    };
    let compare = |a: &usize, b: &usize| match col_ix {
        0 => a.cmp(b),
        _ => compare_text(
            &column_cell(&columns[*a], *a, col_ix),
            &column_cell(&columns[*b], *b, col_ix),
        ),
    };
    match sort {
        ColumnSort::Ascending => rows.sort_by(compare),
        ColumnSort::Descending => rows.sort_by(|a, b| compare(b, a)),
        ColumnSort::Default => {}
    }
    rows
}

/// Case-insensitive, falling back to the exact text so the order is total.
fn compare_text(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// The indexes grid's order: the primary key first, then the rest by name.
/// Backends return indexes in whatever order their catalog yields, which can
/// change between reloads.
pub(super) fn order_indexes(mut indexes: Vec<IndexInfo>, columns: &[ColumnInfo]) -> Vec<IndexInfo> {
    let mut primary_key: Vec<&str> = columns
        .iter()
        .filter(|col| col.is_primary_key)
        .map(|col| col.name.as_str())
        .collect();
    primary_key.sort_unstable();
    // Named like a key by its backend (MySQL `PRIMARY`, Postgres `<table>_pkey`,
    // SQL Server `PK__…`) ranks before a unique index that merely covers the
    // key's columns, so a duplicate unique index never takes the key's place.
    let rank = |index: &IndexInfo| {
        let name = index.name.to_ascii_lowercase();
        let covers_key = index.is_unique && !primary_key.is_empty() && {
            let mut names: Vec<&str> = index.column_names.iter().map(String::as_str).collect();
            names.sort_unstable();
            names == primary_key
        };
        if name == "primary" || (covers_key && (name.ends_with("_pkey") || name.starts_with("pk_")))
        {
            0
        } else if covers_key {
            1
        } else {
            2
        }
    };
    indexes
        .sort_by_cached_key(|index| (rank(index), index.name.to_lowercase(), index.name.clone()));
    indexes
}

impl TableDelegate for ColumnsTableDelegate {
    fn context_menu(
        &mut self,
        (row_ix, _): (usize, usize),
        menu: PopupMenu,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        if !self.editable {
            return menu;
        }
        // Actions address the tab's column list, not the filtered and sorted
        // row the menu was opened on.
        let Some(&column_ix) = self.rows.get(row_ix) else {
            return menu;
        };
        let mark = self.marks.get(column_ix).cloned().unwrap_or_default();
        if matches!(mark.state, RowState::Added | RowState::Dropped) {
            return undo_items(menu, &mark, &self.tab);
        }
        let tab = self.tab.clone();
        let item = |label: &'static str,
                    action: fn(
            &mut TableStructureTab,
            usize,
            &mut Window,
            &mut Context<TableStructureTab>,
        )| {
            let tab = tab.clone();
            PopupMenuItem::new(label).on_click(move |_, window, cx| {
                tab.update(cx, |tab, cx| action(tab, column_ix, window, cx))
                    .log_err();
            })
        };
        let menu = menu
            .item(item("Edit Column", TableStructureTab::edit_column))
            .separator()
            .item(item("Drop Column", TableStructureTab::drop_column));
        if mark.changes.is_empty() {
            menu
        } else {
            undo_items(menu.separator(), &mark, &self.tab)
        }
    }

    fn columns_count(&self, _: &App) -> usize {
        self.column_defs.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) {
        self.set_sort(col_ix, sort);
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.column_defs[col_ix].clone()
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let col = &self.column_defs[col_ix];
        div()
            .h_full()
            .flex()
            .items_center()
            .px_2()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(px(12.))
            .child(col.name.clone())
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let value = self
            .rows
            .get(row_ix)
            .map(|&ix| column_cell(&self.columns[ix], ix, col_ix))
            .unwrap_or_default();
        let state = self
            .rows
            .get(row_ix)
            .and_then(|&ix| self.marks.get(ix))
            .map(|mark| mark.state)
            .unwrap_or_default();
        let type_color = (col_ix == 2)
            .then(|| self.rows.get(row_ix).and_then(|&ix| self.columns.get(ix)))
            .flatten()
            .and_then(|column| {
                type_color(self.dialect.declared_column_type(&column.data_type), cx)
            });

        // A pending change's tint says more than the type's colour.
        staged_cell(value, state, cx).when_some(
            type_color.filter(|_| matches!(state, RowState::Unchanged | RowState::Modified)),
            |cell, color| cell.text_color(color),
        )
    }
}

/// What a column of the indexes grid shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum IndexColumn {
    Name,
    Columns,
    Unique,
    Nulls,
    Method,
    Condition,
    Comment,
}

#[derive(Clone)]
pub struct IndexesTableDelegate {
    pub(super) indexes: Vec<IndexInfo>,
    pub(super) marks: Vec<RowMark>,
    shown: Vec<IndexColumn>,
    column_defs: Vec<Column>,
    pub(super) editable: bool,
    tab: WeakEntity<TableStructureTab>,
}

impl IndexesTableDelegate {
    pub fn new(dialect: Dialect, tab: WeakEntity<TableStructureTab>) -> Self {
        let mut shown = vec![
            (IndexColumn::Name, "Name", 220.),
            (IndexColumn::Columns, "Columns", 200.),
            (IndexColumn::Unique, "Unique", 70.),
        ];
        if dialect.has_nulls_not_distinct() {
            shown.push((IndexColumn::Nulls, "NULLs", 110.));
        }
        shown.extend([
            (IndexColumn::Method, "Method", 100.),
            (IndexColumn::Condition, "Condition", 200.),
            (IndexColumn::Comment, "Comment", 160.),
        ]);
        Self {
            indexes: Vec::new(),
            marks: Vec::new(),
            column_defs: shown
                .iter()
                .map(|(_, name, width)| Column {
                    name: (*name).into(),
                    width: px(*width),
                    ..Default::default()
                })
                .collect(),
            shown: shown.into_iter().map(|(column, _, _)| column).collect(),
            editable: false,
            tab,
        }
    }
}

impl IndexesTableDelegate {
    fn index_cell(&self, row_ix: usize, col_ix: usize) -> String {
        let Some(idx) = self.indexes.get(row_ix) else {
            return String::new();
        };
        match self.shown.get(col_ix) {
            Some(IndexColumn::Name) => idx.name.clone(),
            Some(IndexColumn::Columns) => idx.column_names.join(", "),
            Some(IndexColumn::Unique) => if idx.is_unique { "Yes" } else { "No" }.to_string(),
            Some(IndexColumn::Nulls) if idx.is_unique => if idx.nulls_not_distinct {
                "Not distinct"
            } else {
                "Distinct"
            }
            .to_string(),
            Some(IndexColumn::Method) => idx.algorithm.clone(),
            Some(IndexColumn::Condition) => idx.condition.clone().unwrap_or_default(),
            Some(IndexColumn::Comment) => idx.comment.clone().unwrap_or_default(),
            Some(IndexColumn::Nulls) | None => String::new(),
        }
    }

    pub(super) fn fit_columns(&mut self, window: &Window, cx: &App) {
        let cells: Vec<Vec<String>> = (0..self.indexes.len())
            .map(|row| {
                (0..self.column_defs.len())
                    .map(|col| self.index_cell(row, col))
                    .collect()
            })
            .collect();
        fit_widths(&mut self.column_defs, &cells, false, window, cx);
    }
}

impl TableDelegate for IndexesTableDelegate {
    fn context_menu(
        &mut self,
        (row_ix, _): (usize, usize),
        menu: PopupMenu,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        if !self.editable || row_ix >= self.indexes.len() {
            return menu;
        }
        let mark = self.marks.get(row_ix).cloned().unwrap_or_default();
        if mark.state != RowState::Unchanged {
            return undo_items(menu, &mark, &self.tab);
        }
        let tab = self.tab.clone();
        menu.item(
            PopupMenuItem::new("Drop Index").on_click(move |_, window, cx| {
                tab.update(cx, |tab, cx| tab.drop_index(row_ix, window, cx))
                    .log_err();
            }),
        )
    }

    fn columns_count(&self, _: &App) -> usize {
        self.column_defs.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.indexes.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.column_defs[col_ix].clone()
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let col = &self.column_defs[col_ix];
        div()
            .h_full()
            .flex()
            .items_center()
            .px_2()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(px(12.))
            .child(col.name.clone())
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let value = self.index_cell(row_ix, col_ix);
        let state = self
            .marks
            .get(row_ix)
            .map(|mark| mark.state)
            .unwrap_or_default();

        staged_cell(value, state, cx)
    }
}

#[derive(Clone)]
pub struct ForeignKeysTableDelegate {
    pub(super) foreign_keys: Vec<ForeignKeyConstraint>,
    pub(super) marks: Vec<RowMark>,
    /// Listed after the declared keys.
    pub(super) virtual_keys: Vec<VirtualForeignKey>,
    /// The tab's schema; references into it are shown without a prefix.
    schema: Option<String>,
    column_defs: Vec<Column>,
    pub(super) editable: bool,
    tab: WeakEntity<TableStructureTab>,
}

impl ForeignKeysTableDelegate {
    pub fn new(schema: Option<String>, tab: WeakEntity<TableStructureTab>) -> Self {
        let column = |name: &'static str, width: f32| Column {
            name: name.into(),
            width: px(width),
            ..Default::default()
        };
        Self {
            foreign_keys: Vec::new(),
            marks: Vec::new(),
            virtual_keys: Vec::new(),
            schema,
            column_defs: vec![
                column("Name", 200.),
                column("Kind", 90.),
                column("Columns", 150.),
                column("References", 220.),
                column("On Update", 100.),
                column("On Delete", 100.),
            ],
            editable: false,
            tab,
        }
    }

    /// The virtual key a row shows, for rows after the declared keys.
    pub(super) fn virtual_key_at(&self, row_ix: usize) -> Option<&VirtualForeignKey> {
        row_ix
            .checked_sub(self.foreign_keys.len())
            .and_then(|ix| self.virtual_keys.get(ix))
    }

    fn foreign_key_cell(&self, row_ix: usize, col_ix: usize) -> String {
        if let Some(key) = self.virtual_key_at(row_ix) {
            return match col_ix {
                0 => key.name.clone(),
                1 => "Virtual".to_string(),
                2 => key.columns.join(", "),
                3 => format!(
                    "{}({})",
                    crate::virtual_keys::referenced_table(key),
                    key.referenced_columns.join(", ")
                ),
                _ => String::new(),
            };
        }
        let Some(foreign_key) = self.foreign_keys.get(row_ix) else {
            return String::new();
        };
        let added = self
            .marks
            .get(row_ix)
            .is_some_and(|mark| mark.state == RowState::Added);
        match col_ix {
            // A new key without a name gets one from the server.
            0 if foreign_key.name.is_empty() && added => "(generated)".to_string(),
            0 => foreign_key.name.clone(),
            1 => "Declared".to_string(),
            2 => foreign_key.columns.join(", "),
            3 => self.references(foreign_key),
            4 => foreign_key.on_update.clone(),
            5 => foreign_key.on_delete.clone(),
            _ => String::new(),
        }
    }

    pub(super) fn fit_columns(&mut self, window: &Window, cx: &App) {
        let cells: Vec<Vec<String>> = (0..self.foreign_keys.len() + self.virtual_keys.len())
            .map(|row| {
                (0..self.column_defs.len())
                    .map(|col| self.foreign_key_cell(row, col))
                    .collect()
            })
            .collect();
        fit_widths(&mut self.column_defs, &cells, false, window, cx);
    }

    fn references(&self, foreign_key: &ForeignKeyConstraint) -> String {
        let table = match foreign_key.referenced_schema.as_deref() {
            Some(schema) if Some(schema) != self.schema.as_deref() => {
                format!("{schema}.{}", foreign_key.referenced_table)
            }
            _ => foreign_key.referenced_table.clone(),
        };
        // SQLite leaves the column out when the reference names only the
        // table's primary key.
        if foreign_key.referenced_columns.iter().all(String::is_empty) {
            table
        } else {
            format!("{table}({})", foreign_key.referenced_columns.join(", "))
        }
    }
}

impl TableDelegate for ForeignKeysTableDelegate {
    fn context_menu(
        &mut self,
        (row_ix, _): (usize, usize),
        menu: PopupMenu,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        // Virtual keys change nothing on the server, so a read-only
        // connection may have them too.
        if let Some(key) = self.virtual_key_at(row_ix) {
            let id = key.id;
            let edit_tab = self.tab.clone();
            let delete_tab = self.tab.clone();
            return menu
                .item(
                    PopupMenuItem::new("Edit Virtual Key").on_click(move |_, window, cx| {
                        edit_tab
                            .update(cx, |tab, cx| tab.edit_virtual_key(id, window, cx))
                            .log_err();
                    }),
                )
                .item(
                    PopupMenuItem::new("Delete Virtual Key").on_click(move |_, window, cx| {
                        delete_tab
                            .update(cx, |tab, cx| tab.delete_virtual_key(id, window, cx))
                            .log_err();
                    }),
                );
        }
        if !self.editable || row_ix >= self.foreign_keys.len() {
            return menu;
        }
        let mark = self.marks.get(row_ix).cloned().unwrap_or_default();
        if mark.state != RowState::Unchanged {
            return undo_items(menu, &mark, &self.tab);
        }
        let tab = self.tab.clone();
        menu.item(
            PopupMenuItem::new("Drop Foreign Key").on_click(move |_, window, cx| {
                tab.update(cx, |tab, cx| tab.drop_foreign_key(row_ix, window, cx))
                    .log_err();
            }),
        )
    }

    fn columns_count(&self, _: &App) -> usize {
        self.column_defs.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.foreign_keys.len() + self.virtual_keys.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.column_defs[col_ix].clone()
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let col = &self.column_defs[col_ix];
        div()
            .h_full()
            .flex()
            .items_center()
            .px_2()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(px(12.))
            .child(col.name.clone())
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let value = self.foreign_key_cell(row_ix, col_ix);
        if self.virtual_key_at(row_ix).is_some() {
            return staged_cell(value, RowState::Unchanged, cx).italic();
        }
        let state = self
            .marks
            .get(row_ix)
            .map(|mark| mark.state)
            .unwrap_or_default();
        staged_cell(value, state, cx)
    }
}

/// The colour the results grid gives values of `column_type`, `None` for
/// the plain text colour.
fn type_color(column_type: ColumnType, cx: &App) -> Option<Hsla> {
    let theme = cx.theme();
    match column_type {
        ColumnType::Uuid | ColumnType::Array => Some(theme.blue),
        ColumnType::DateTime => Some(theme.green),
        ColumnType::Json | ColumnType::KeyValue => Some(theme.yellow),
        ColumnType::Integer
        | ColumnType::UnsignedInteger
        | ColumnType::Numeric
        | ColumnType::Text
        | ColumnType::Boolean
        | ColumnType::Binary
        | ColumnType::Unknown => None,
    }
}

/// Set each column's width to fit its header and its widest cell, as the
/// results grid sizes its columns. A sortable header needs room for its
/// sort icon.
fn fit_widths(
    column_defs: &mut [Column],
    cells: &[Vec<String>],
    sortable: bool,
    window: &Window,
    cx: &App,
) {
    let font = gpui::font(cx.theme().mono_font_family.clone());
    let measure = |text: &str| {
        let widest_line = text
            .lines()
            .max_by_key(|line| line.len())
            .unwrap_or_default();
        window
            .text_system()
            .shape_line(
                SharedString::from(widest_line.to_string()),
                px(12.),
                &[TextRun {
                    len: widest_line.len(),
                    font: font.clone(),
                    color: gpui::black(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            )
            .width
    };
    for (col_ix, def) in column_defs.iter_mut().enumerate() {
        let header = measure(def.name.as_ref()) + if sortable { px(24.) } else { px(0.) };
        let widest = cells
            .iter()
            .filter_map(|row| row.get(col_ix))
            .map(|cell| measure(cell))
            .fold(header, |widest, width| widest.max(width));
        // The cell's padding on both sides and its border.
        def.width = (widest + px(18.)).clamp(px(48.), px(400.));
    }
}

/// One cell of a structure list, tinted like the results grid tints pending
/// edits: green for an addition, yellow for a change, red and struck
/// through for a drop.
fn staged_cell(
    value: String,
    state: RowState,
    cx: &Context<TableState<impl TableDelegate>>,
) -> Div {
    let theme = cx.theme();
    div()
        .h_full()
        .flex()
        .items_center()
        .px_2()
        .font_family(theme.mono_font_family.clone())
        .text_size(px(12.))
        .text_color(theme.foreground)
        .map(|cell| match state {
            RowState::Unchanged => cell,
            RowState::Added => cell.text_color(theme.green).bg(theme.green.opacity(0.15)),
            RowState::Modified => cell.bg(theme.yellow.opacity(0.3)),
            RowState::Dropped => cell
                .line_through()
                .text_color(theme.red)
                .bg(theme.red.opacity(0.15)),
        })
        .child(value)
}

/// An Undo item for each pending change on a row.
fn undo_items(menu: PopupMenu, mark: &RowMark, tab: &WeakEntity<TableStructureTab>) -> PopupMenu {
    mark.changes
        .iter()
        .fold(menu, |menu, (change_ix, description)| {
            let tab = tab.clone();
            let change_ix = *change_ix;
            menu.item(PopupMenuItem::new(format!("Undo {description}")).on_click(
                move |_, window, cx| {
                    tab.update(cx, |tab, cx| tab.undo_change(change_ix, window, cx))
                        .log_err();
                },
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::{column_rows, order_indexes};
    use blanco_core::{ColumnInfo, IndexInfo};
    use gpui_component::table::ColumnSort;

    fn column(name: &str, data_type: &str, is_primary_key: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.to_string(),
            data_type: data_type.to_string(),
            is_nullable: !is_primary_key,
            is_primary_key,
            default_value: None,
            character_maximum_length: None,
            foreign_key: None,
            comment: None,
            numeric_precision: None,
            numeric_scale: None,
            enum_labels: Vec::new(),
        }
    }

    fn index(name: &str, is_unique: bool, columns: &[&str]) -> IndexInfo {
        IndexInfo {
            name: name.to_string(),
            algorithm: "btree".to_string(),
            is_unique,
            column_names: columns.iter().map(|c| c.to_string()).collect(),
            condition: None,
            comment: None,
            nulls_not_distinct: false,
        }
    }

    fn columns() -> Vec<ColumnInfo> {
        vec![
            column("id", "integer", true),
            column("Email", "text", false),
            column("created_at", "timestamp", false),
            column("email_verified", "boolean", false),
        ]
    }

    #[test]
    fn column_rows_sort_and_restore_table_order() {
        let columns = columns();
        assert_eq!(column_rows(&columns, "", None), vec![0, 1, 2, 3]);
        // Name ignores case.
        assert_eq!(
            column_rows(&columns, "", Some((1, ColumnSort::Ascending))),
            vec![2, 1, 3, 0]
        );
        assert_eq!(
            column_rows(&columns, "", Some((1, ColumnSort::Descending))),
            vec![0, 3, 1, 2]
        );
        // Type.
        assert_eq!(
            column_rows(&columns, "", Some((2, ColumnSort::Ascending))),
            vec![3, 0, 1, 2]
        );
        // The `#` column sorts by position, not as text.
        let many: Vec<ColumnInfo> = (0..12)
            .map(|ix| column(&format!("c{ix}"), "text", false))
            .collect();
        assert_eq!(
            column_rows(&many, "", Some((0, ColumnSort::Descending)))[..3],
            [11, 10, 9]
        );
        assert_eq!(
            column_rows(&columns, "", Some((1, ColumnSort::Default))),
            vec![0, 1, 2, 3]
        );
    }

    #[test]
    fn column_rows_filter_names_ignoring_case() {
        let columns = columns();
        assert_eq!(column_rows(&columns, "EMAIL", None), vec![1, 3]);
        assert_eq!(column_rows(&columns, "  at ", None), vec![2]);
        assert_eq!(
            column_rows(&columns, "email", Some((1, ColumnSort::Descending))),
            vec![3, 1]
        );
        assert!(column_rows(&columns, "missing", None).is_empty());
        // The sidebar filter's anchors and alternatives.
        assert_eq!(column_rows(&columns, "^email$", None), vec![1]);
        assert_eq!(column_rows(&columns, "^id | _at$", None), vec![0, 2]);
    }

    #[test]
    fn order_indexes_puts_primary_key_first() {
        let names = |indexes: Vec<IndexInfo>| {
            indexes
                .into_iter()
                .map(|index| index.name)
                .collect::<Vec<_>>()
        };
        let columns = columns();

        // Postgres: the key is named `<table>_pkey`; a unique index over the
        // same column follows it, and the rest go by name.
        let indexes = vec![
            index("users_email_key", true, &["Email"]),
            index("a_id_unique", true, &["id"]),
            index("users_pkey", true, &["id"]),
            index("Created_idx", false, &["created_at"]),
        ];
        assert_eq!(
            names(order_indexes(indexes, &columns)),
            [
                "users_pkey",
                "a_id_unique",
                "Created_idx",
                "users_email_key"
            ]
        );

        // MySQL names the key PRIMARY.
        let indexes = vec![
            index("email", true, &["Email"]),
            index("PRIMARY", true, &["id"]),
        ];
        assert_eq!(
            names(order_indexes(indexes, &columns)),
            ["PRIMARY", "email"]
        );

        // SQLite's key index has a generated name; it is found by its columns.
        let indexes = vec![
            index("idx_email", false, &["Email"]),
            index("sqlite_autoindex_users_1", true, &["id"]),
        ];
        assert_eq!(
            names(order_indexes(indexes, &columns)),
            ["sqlite_autoindex_users_1", "idx_email"]
        );
    }
}
