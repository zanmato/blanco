//! Hiding, reordering and pinning result columns.
//!
//! The arrangement is physical: `columns`, `column_types` and every row hold
//! the pinned columns first, then the other visible ones, then the hidden
//! ones, and the table is told only about the visible part. Every index the
//! grid, the editor and the change tracking use therefore stays a plain
//! index into the row, and a hidden column still carries its values for the
//! primary key of an UPDATE.

use std::collections::HashMap;

use gpui::{Pixels, px};
use gpui_component::table::ColumnFixed;
use serde::{Deserialize, Serialize};

use super::ResultsTableDelegate;

/// A result's column arrangement by column name, as remembered for a table
/// or a query and applied again to the next result with those columns.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ColumnLayout {
    /// Visible columns in display order, the pinned ones first.
    pub order: Vec<String>,
    /// How many of `order` are pinned to the left edge.
    pub pinned: usize,
    pub hidden: Vec<String>,
    /// Widths the user gave columns, in pixels.
    #[serde(default)]
    pub widths: HashMap<String, f32>,
}

impl ResultsTableDelegate {
    /// Columns the grid shows; the rest of `columns` are hidden.
    pub fn visible_column_count(&self) -> usize {
        self.columns.len().saturating_sub(self.hidden_columns)
    }

    /// Whether the result is a whole table, every column of the one table it
    /// reads and nothing else, as opening a table shows. Its layout is
    /// remembered for the table rather than for the query.
    pub fn shows_whole_table(&self) -> bool {
        if self.table_name.is_none() || self.table_columns.is_empty() {
            return false;
        }
        let mut result: Vec<&str> = self.columns.iter().map(|c| c.name.as_str()).collect();
        let mut table: Vec<&str> = self.table_columns.iter().map(|c| c.name.as_str()).collect();
        result.sort_unstable();
        table.sort_unstable();
        result == table
    }

    /// Columns pinned to the left edge, the first ones shown.
    pub fn pinned_column_count(&self) -> usize {
        self.pinned_columns.min(self.visible_column_count())
    }

    /// Put rows read in the query's own column order into this layout.
    pub fn arrange_source_rows(&self, rows: Vec<Vec<Option<String>>>) -> Vec<Vec<Option<String>>> {
        if self.source_order.len() != self.columns.len()
            || self
                .source_order
                .iter()
                .enumerate()
                .all(|(ix, source)| ix == *source)
        {
            return rows;
        }
        rows.into_iter()
            .map(|row| {
                self.source_order
                    .iter()
                    .map(|source| row.get(*source).cloned().flatten())
                    .collect()
            })
            .collect()
    }

    /// Whether columns can move. Pending edits and the open cell editor are
    /// keyed by column index, so the layout is fixed until they are applied
    /// or discarded, as sorting and filtering are.
    pub fn can_rearrange_columns(&self) -> bool {
        !self.has_pending_changes()
            && self.edit_state.edited_values.is_empty()
            && self.edit_state.editing_cell.is_none()
    }

    /// Reorder every per-column structure so that column `order[new]` ends
    /// up at `new`. `order` must be a permutation of all columns.
    fn permute_columns(&mut self, order: &[usize]) {
        debug_assert_eq!(order.len(), self.columns.len());
        if self.source_order.len() != self.columns.len() {
            self.source_order = (0..self.columns.len()).collect();
        }
        fn permute<T: Clone>(values: &mut Vec<T>, order: &[usize]) {
            if values.len() == order.len() {
                *values = order.iter().map(|ix| values[*ix].clone()).collect();
            }
        }
        permute(&mut self.columns, order);
        permute(&mut self.column_types, order);
        permute(&mut self.source_order, order);
        for row in self.rows.iter_mut() {
            permute(row, order);
        }
        if let Some(rows) = self.unfiltered_rows.as_mut() {
            for row in rows.iter_mut() {
                permute(row, order);
            }
        }
        self.mark_pinned_columns();
    }

    fn mark_pinned_columns(&mut self) {
        let pinned = self.pinned_column_count();
        for (ix, column) in self.columns.iter_mut().enumerate() {
            column.fixed = (ix < pinned).then_some(ColumnFixed::Left);
        }
    }

    /// Move the column at `from` to `to`, both display positions. Returns
    /// whether anything moved.
    fn move_to(&mut self, from: usize, to: usize) -> bool {
        let len = self.columns.len();
        if from >= len || to >= len || from == to {
            return false;
        }
        let mut order: Vec<usize> = (0..len).collect();
        let moved = order.remove(from);
        order.insert(to, moved);
        self.permute_columns(&order);
        true
    }

    /// Drag a visible column within its region: pinned columns among the
    /// pinned, the rest among the rest.
    pub fn move_visible_column(&mut self, from: usize, to: usize) -> bool {
        let visible = self.visible_column_count();
        let pinned = self.pinned_column_count();
        if !self.can_rearrange_columns() || from >= visible || to >= visible {
            return false;
        }
        if (from < pinned) != (to < pinned) {
            return false;
        }
        self.move_to(from, to)
    }

    /// Hide the visible column at `ix`. The last visible column stays.
    pub fn hide_column(&mut self, ix: usize) -> bool {
        let visible = self.visible_column_count();
        if !self.can_rearrange_columns() || ix >= visible || visible <= 1 {
            return false;
        }
        if ix < self.pinned_column_count() {
            self.pinned_columns -= 1;
        }
        // The column goes to the front of the hidden ones.
        self.move_to(ix, visible - 1);
        self.hidden_columns += 1;
        self.mark_pinned_columns();
        true
    }

    /// Show the hidden column at `ix`, after the visible ones.
    pub fn show_column(&mut self, ix: usize) -> bool {
        let visible = self.visible_column_count();
        if !self.can_rearrange_columns() || ix < visible || ix >= self.columns.len() {
            return false;
        }
        self.move_to(ix, visible);
        self.hidden_columns -= 1;
        self.mark_pinned_columns();
        true
    }

    /// Pin the visible column at `ix` after the pinned ones, or unpin a
    /// pinned one to the start of the rest.
    pub fn set_column_pinned(&mut self, ix: usize, pinned: bool) -> bool {
        let count = self.pinned_column_count();
        if !self.can_rearrange_columns() || ix >= self.visible_column_count() {
            return false;
        }
        match (pinned, ix < count) {
            (true, false) => {
                self.pinned_columns = count + 1;
                self.move_to(ix, count);
            }
            (false, true) => {
                self.pinned_columns = count - 1;
                self.move_to(ix, count - 1);
            }
            _ => return false,
        }
        self.mark_pinned_columns();
        true
    }

    /// Keep the widths the table reports after a resize, so they survive a
    /// rearrangement and end up in the layout.
    pub fn set_column_widths(&mut self, widths: &[Pixels]) {
        for (column, width) in self.columns.iter_mut().zip(widths) {
            column.width = *width;
        }
    }

    /// The current arrangement by column name. `widths` holds the columns the
    /// user resized, compared with the widths the result opened with.
    pub fn column_layout(&self) -> ColumnLayout {
        let visible = self.visible_column_count();
        let name = |ix: usize| self.columns[ix].name.to_string();
        ColumnLayout {
            order: (0..visible).map(name).collect(),
            pinned: self.pinned_column_count(),
            hidden: (visible..self.columns.len()).map(name).collect(),
            widths: self
                .columns
                .iter()
                .zip(&self.measured_widths)
                .filter(|(column, measured)| column.width != **measured)
                .map(|(column, _)| (column.name.to_string(), f32::from(column.width)))
                .collect(),
        }
    }

    /// Arrange this result's columns as `layout` says, by name. Columns the
    /// layout does not know keep their place after the ones it orders, and
    /// names it has that the result lacks are skipped, so a layout carries
    /// over when a table gains or loses a column.
    pub fn apply_column_layout(&mut self, layout: &ColumnLayout) {
        if !self.can_rearrange_columns() || self.columns.is_empty() {
            return;
        }
        let position = |name: &str| {
            self.columns
                .iter()
                .position(|column| column.name.as_str() == name)
        };
        let mut placed = vec![false; self.columns.len()];
        let mut order = Vec::with_capacity(self.columns.len());
        let mut take = |ix: usize, order: &mut Vec<usize>| {
            if !placed[ix] {
                placed[ix] = true;
                order.push(ix);
            }
        };
        let ordered: Vec<usize> = layout.order.iter().filter_map(|n| position(n)).collect();
        let pinned = layout
            .order
            .iter()
            .take(layout.pinned)
            .filter(|n| position(n).is_some())
            .count();
        for ix in &ordered {
            take(*ix, &mut order);
        }
        let hidden: Vec<usize> = layout.hidden.iter().filter_map(|n| position(n)).collect();
        // Columns the layout does not mention are new to it, so show them.
        for ix in 0..self.columns.len() {
            if !hidden.contains(&ix) {
                take(ix, &mut order);
            }
        }
        let mut visible = order.len();
        for ix in hidden {
            take(ix, &mut order);
        }
        // Keep at least one column on screen.
        if visible == 0 {
            visible = 1;
        }
        for column in self.columns.iter_mut() {
            if let Some(width) = layout.widths.get(column.name.as_str()) {
                column.width = px(*width);
            }
        }
        self.pinned_columns = pinned.min(visible);
        self.hidden_columns = self.columns.len() - visible;
        self.permute_columns(&order);
    }

    /// Start a new result in its query's own order, everything shown and
    /// nothing pinned, remembering the widths it opened with so a layout only
    /// stores the ones the user changed.
    pub(super) fn reset_column_layout(&mut self) {
        self.hidden_columns = 0;
        self.pinned_columns = 0;
        self.source_order = (0..self.columns.len()).collect();
        self.measured_widths = self.columns.iter().map(|column| column.width).collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blanco_core::connection_trait::ColumnType;
    use gpui_component::table::Column;

    fn delegate(names: &[&str]) -> ResultsTableDelegate {
        let mut delegate = ResultsTableDelegate {
            columns: names
                .iter()
                .map(|name| Column::new(*name, *name).width(px(100.)))
                .collect(),
            column_types: names.iter().map(|_| ColumnType::Text).collect(),
            rows: vec![
                names.iter().map(|n| Some(format!("{n}1"))).collect(),
                names.iter().map(|n| Some(format!("{n}2"))).collect(),
            ],
            ..Default::default()
        };
        delegate.reset_column_layout();
        delegate
    }

    fn names(delegate: &ResultsTableDelegate) -> Vec<String> {
        delegate
            .columns
            .iter()
            .map(|c| c.name.to_string())
            .collect()
    }

    #[test]
    fn hiding_pinning_and_moving_keep_rows_aligned() {
        let mut grid = delegate(&["id", "name", "email", "notes"]);
        assert!(grid.hide_column(2));
        assert_eq!(names(&grid), ["id", "name", "notes", "email"]);
        assert_eq!(grid.visible_column_count(), 3);
        assert_eq!(grid.rows[0][3].as_deref(), Some("email1"));

        assert!(grid.set_column_pinned(1, true));
        assert_eq!(names(&grid), ["name", "id", "notes", "email"]);
        assert_eq!(grid.pinned_column_count(), 1);
        assert_eq!(grid.columns[0].fixed, Some(ColumnFixed::Left));
        assert_eq!(grid.columns[1].fixed, None);

        // A drag cannot carry a column across the pinned edge.
        assert!(!grid.move_visible_column(2, 0));
        assert!(grid.move_visible_column(2, 1));
        assert_eq!(names(&grid), ["name", "notes", "id", "email"]);
        assert_eq!(grid.rows[1][1].as_deref(), Some("notes2"));

        assert!(grid.show_column(3));
        assert_eq!(grid.visible_column_count(), 4);
        // Fresh rows from the same query land in this order.
        let fresh = grid.arrange_source_rows(vec![vec![
            Some("7".into()),
            Some("n".into()),
            Some("e".into()),
            Some("x".into()),
        ]]);
        assert_eq!(
            fresh[0]
                .iter()
                .map(|c| c.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["n", "x", "7", "e"]
        );
    }

    #[test]
    fn layout_round_trips_by_name_and_tolerates_changed_columns() {
        let mut grid = delegate(&["id", "name", "email"]);
        grid.hide_column(1);
        grid.set_column_pinned(1, true);
        grid.columns[0].width = px(240.);
        let layout = grid.column_layout();
        assert_eq!(layout.order, ["email", "id"]);
        assert_eq!(layout.pinned, 1);
        assert_eq!(layout.hidden, ["name"]);
        assert_eq!(layout.widths.get("email"), Some(&240.));
        assert_eq!(layout.widths.len(), 1);

        // The table gained `created_at` and lost `email`.
        let mut next = delegate(&["id", "name", "created_at"]);
        next.apply_column_layout(&layout);
        assert_eq!(names(&next), ["id", "created_at", "name"]);
        assert_eq!(next.visible_column_count(), 2);
        assert_eq!(next.pinned_column_count(), 0);
        assert_eq!(next.rows[0][1].as_deref(), Some("created_at1"));
    }

    #[test]
    fn columns_stay_put_while_edits_are_pending() {
        let mut grid = delegate(&["id", "name"]);
        grid.edit_state
            .edited_values
            .insert((0, 1), Some("x".into()));
        assert!(!grid.hide_column(1));
        assert!(!grid.move_visible_column(0, 1));
        assert_eq!(names(&grid), ["id", "name"]);
    }
}
