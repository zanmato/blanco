//! What a table has selected.

use std::ops::Range;

/// A set of row indices, kept as sorted, disjoint ranges that never touch.
///
/// Selecting every row of a large result, or a Shift range, stays one range
/// however many rows it covers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowSet {
    ranges: Vec<Range<usize>>,
}

impl RowSet {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// The rows in `range`.
    pub fn from_range(range: Range<usize>) -> Self {
        let mut set = Self::new();
        set.insert_range(range);
        set
    }

    /// Whether the set holds no rows.
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// How many rows the set holds.
    pub fn len(&self) -> usize {
        self.ranges.iter().map(|range| range.len()).sum()
    }

    /// Whether `row_ix` is in the set.
    pub fn contains(&self, row_ix: usize) -> bool {
        let ix = self.ranges.partition_point(|range| range.end <= row_ix);
        self.ranges
            .get(ix)
            .is_some_and(|range| range.start <= row_ix)
    }

    /// The lowest row in the set.
    pub fn first(&self) -> Option<usize> {
        self.ranges.first().map(|range| range.start)
    }

    /// The highest row in the set.
    pub fn last(&self) -> Option<usize> {
        self.ranges.last().map(|range| range.end - 1)
    }

    /// The rows in ascending order.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.ranges.iter().flat_map(|range| range.clone())
    }

    /// The rows as sorted, disjoint ranges.
    pub fn ranges(&self) -> &[Range<usize>] {
        &self.ranges
    }

    /// Adds `row_ix`.
    pub fn insert(&mut self, row_ix: usize) {
        self.insert_range(row_ix..row_ix + 1);
    }

    /// Adds every row in `range`, merging it with the ranges it meets.
    pub fn insert_range(&mut self, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        // The ranges that overlap `range` or touch either end of it.
        let lo = self.ranges.partition_point(|r| r.end < range.start);
        let hi = self.ranges.partition_point(|r| r.start <= range.end);
        let (start, end) = match self.ranges.get(lo..hi) {
            Some([first, .., last]) => (first.start.min(range.start), last.end.max(range.end)),
            Some([only]) => (only.start.min(range.start), only.end.max(range.end)),
            _ => (range.start, range.end),
        };
        self.ranges.splice(lo..hi, [start..end]);
    }

    /// Takes `row_ix` out, splitting its range when it sits inside one.
    pub fn remove(&mut self, row_ix: usize) {
        let ix = self.ranges.partition_point(|range| range.end <= row_ix);
        let Some(range) = self.ranges.get(ix).cloned() else {
            return;
        };
        if range.start > row_ix {
            return;
        }
        let parts = [range.start..row_ix, row_ix + 1..range.end];
        self.ranges.splice(
            ix..ix + 1,
            parts.into_iter().filter(|part| !part.is_empty()),
        );
    }

    /// Adds `row_ix` when it is missing, takes it out when present.
    pub fn toggle(&mut self, row_ix: usize) {
        if self.contains(row_ix) {
            self.remove(row_ix);
        } else {
            self.insert(row_ix);
        }
    }

    /// Drops the rows at `rows_count` and past it, after rows went away.
    pub fn truncate(&mut self, rows_count: usize) {
        self.ranges.retain_mut(|range| {
            range.end = range.end.min(rows_count);
            range.start < range.end
        });
    }
}

impl FromIterator<usize> for RowSet {
    fn from_iter<I: IntoIterator<Item = usize>>(rows: I) -> Self {
        let mut set = Self::new();
        for row_ix in rows {
            set.insert(row_ix);
        }
        set
    }
}

/// What a table has selected: rows, one column or one cell, never a mix.
///
/// The table keeps nothing selected besides this value, so whatever it
/// shows as selected is exactly what [`TableSelection::rows`] and the other
/// readers report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum TableSelection {
    /// Nothing is selected.
    #[default]
    None,
    /// Whole rows. Never empty: no rows is [`TableSelection::None`].
    Rows(RowSet),
    /// A column.
    Column(usize),
    /// A cell, as `(row_ix, col_ix)`.
    Cell(usize, usize),
}

impl TableSelection {
    /// One row.
    pub fn row(row_ix: usize) -> Self {
        Self::Rows(RowSet::from_range(row_ix..row_ix + 1))
    }

    /// The given rows, or [`TableSelection::None`] when there are none.
    pub fn from_rows(rows: RowSet) -> Self {
        if rows.is_empty() {
            Self::None
        } else {
            Self::Rows(rows)
        }
    }

    /// Whether nothing is selected.
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// The selected rows, when whole rows are selected.
    pub fn selected_rows(&self) -> Option<&RowSet> {
        match self {
            Self::Rows(rows) => Some(rows),
            _ => None,
        }
    }

    /// Whether `row_ix` is selected as a whole row.
    pub fn is_row_selected(&self, row_ix: usize) -> bool {
        self.selected_rows()
            .is_some_and(|rows| rows.contains(row_ix))
    }

    /// The selected column, when a column is selected.
    pub fn column(&self) -> Option<usize> {
        match self {
            Self::Column(col_ix) => Some(*col_ix),
            _ => None,
        }
    }

    /// The selected cell, when a cell is selected.
    pub fn cell(&self) -> Option<(usize, usize)> {
        match self {
            Self::Cell(row_ix, col_ix) => Some((*row_ix, *col_ix)),
            _ => None,
        }
    }

    /// The rows the selection lies in, in ascending order: every selected
    /// row, or the row of the selected cell. Empty for a column or nothing.
    ///
    /// Row actions such as copy, export, duplicate and delete act on these.
    pub fn rows(&self) -> Vec<usize> {
        match self {
            Self::Rows(rows) => rows.iter().collect(),
            Self::Cell(row_ix, _) => vec![*row_ix],
            Self::Column(_) | Self::None => Vec::new(),
        }
    }

    /// Whether the selection lies in `row_ix`, see [`Self::rows`].
    pub fn covers_row(&self, row_ix: usize) -> bool {
        match self {
            Self::Rows(rows) => rows.contains(row_ix),
            Self::Cell(cell_row, _) => *cell_row == row_ix,
            Self::Column(_) | Self::None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges(set: &RowSet) -> Vec<Range<usize>> {
        set.ranges().to_vec()
    }

    #[test]
    fn row_set_merges_and_splits_ranges() {
        let mut set = RowSet::from_range(2..5);
        set.insert(8);
        assert_eq!(ranges(&set), [2..5, 8..9]);

        // Touching ranges merge, so each row lives in one range.
        set.insert_range(5..8);
        assert_eq!(ranges(&set), [2..9]);
        assert_eq!(set.len(), 7);

        set.remove(4);
        assert_eq!(ranges(&set), [2..4, 5..9]);
        set.remove(2);
        set.remove(8);
        assert_eq!(ranges(&set), [3..4, 5..8]);
        assert!(!set.contains(4) && set.contains(5) && !set.contains(8));

        set.toggle(4);
        assert_eq!(ranges(&set), [3..8]);
        set.insert_range(0..20);
        assert_eq!(ranges(&set), [0..20]);

        set.truncate(10);
        assert_eq!((set.first(), set.last()), (Some(0), Some(9)));
        set.truncate(0);
        assert!(set.is_empty());
    }

    #[test]
    fn rows_cover_a_cell_but_not_a_column() {
        assert_eq!(TableSelection::Cell(3, 1).rows(), [3]);
        assert!(TableSelection::Column(1).rows().is_empty());
        let rows = TableSelection::from_rows([4, 1, 2].into_iter().collect());
        assert_eq!(rows.rows(), [1, 2, 4]);
        assert!(rows.is_row_selected(2) && !rows.is_row_selected(3));
        assert!(TableSelection::from_rows(RowSet::new()).is_none());
        assert!(!TableSelection::Cell(3, 1).is_row_selected(3));
        assert!(TableSelection::Cell(3, 1).covers_row(3));
    }
}
