use gpui_base::TestSupportExt as _;
use std::{ops::Range, rc::Rc, time::Duration};

use crate::actions::SelectAll;
use gpui::{
    AppContext, Axis, Bounds, ClickEvent, Context, Div, DragMoveEvent, ElementId, EventEmitter,
    FocusHandle, Focusable, InteractiveElement, IntoElement, ListSizingBehavior, Modifiers,
    MouseButton, MouseDownEvent, ParentElement, Pixels, Point, Render, ScrollStrategy,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled, Task, TextAlign,
    UniformListScrollHandle, Window, div, prelude::FluentBuilder, px, rems, uniform_list,
};
use gpui_base::actions::{
    Cancel, SelectDown, SelectFirst, SelectLast, SelectNextColumn, SelectPageDown, SelectPageUp,
    SelectPrevColumn, SelectUp,
};
use gpui_component::{
    ActiveTheme, ElementExt, Icon, IconName, StyleSized as _, StyledExt, VirtualListScrollHandle,
    h_flex,
    menu::{ContextMenuExt, PopupMenu},
    scroll::{ScrollableMask, Scrollbar},
    v_flex,
};
use rust_i18n::t;

use super::*;

/// Where the column at `ix` ends up after the column at `from` moves to `to`.
fn moved_index(ix: usize, from: usize, to: usize) -> usize {
    if ix == from {
        to
    } else if from < to && (from + 1..=to).contains(&ix) {
        ix - 1
    } else if to < from && (to..from).contains(&ix) {
        ix + 1
    } else {
        ix
    }
}

/// Grab room on each side of a resize handle's hairline, so the divider can be
/// caught without pixel-hunting. Both halves of the band are this wide, so
/// widening the grab area stays symmetric about the boundary. `resizable`'s
/// handle pads a hairline the same way, and by the same amount, see
/// `crates/base/src/resizable/resize_handle.rs`.
const HANDLE_PADDING: Pixels = px(4.);

/// What a right click opened the context menu over.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum MenuTarget {
    /// A row outside any cell, in a table without cell selection.
    Row(usize),
    /// A cell, as `(row_ix, col_ix)`.
    Cell(usize, usize),
    /// A column header.
    Column(usize),
}

/// The Table event.
#[derive(Clone)]
pub enum TableEvent {
    /// The selection was set, by a click, a keyboard move or a call. Read
    /// the new value with [`TableState::selection`].
    SelectionChanged,
    /// Double click on the row.
    DoubleClickedRow(usize),
    /// A cell has been double-clicked, as `(row_ix, col_ix)`.
    ///
    /// Use this event to trigger actions like opening a detail view or
    /// editing the cell content.
    DoubleClickedCell(usize, usize),
    /// The column widths have changed.
    ///
    /// The `Vec<Pixels>` contains the new widths of all columns.
    ColumnWidthsChanged(Vec<Pixels>),
    /// A column has been moved.
    ///
    /// The first `usize` is the original index of the column,
    /// and the second `usize` is the new index of the column.
    MoveColumn(usize, usize),
}

/// The visible range of the rows and columns.
#[derive(Debug, Default)]
pub struct TableVisibleRange {
    /// The visible range of the rows.
    rows: Range<usize>,
    /// The visible range of the columns.
    cols: Range<usize>,
}

impl TableVisibleRange {
    /// Returns the visible range of the rows.
    pub fn rows(&self) -> &Range<usize> {
        &self.rows
    }

    /// Returns the visible range of the columns.
    pub fn cols(&self) -> &Range<usize> {
        &self.cols
    }
}

/// Where a column resize started: the pointer's x at the press on the
/// resize handle, and the column's width then.
#[derive(Clone, Copy)]
struct ResizeOrigin {
    x: Pixels,
    width: Pixels,
}

#[derive(Clone)]
pub(crate) struct HeaderCell {
    pub label: SharedString,
    pub width: Pixels,
    col_span: usize,
    is_leaf: bool,
    leaf_col_ix: Option<usize>,
    start_leaf_col_ix: usize,
}

/// The state for [`DataTable`].
///
/// # Selection
///
/// The table holds one [`TableSelection`]: whole rows, a column or a cell.
/// Nothing else counts as selected, so the highlighted rows and cells are
/// exactly what [`Self::selection`] reports.
///
/// - A click selects the row or cell under the pointer. Without the row
///   header, a second click on the selected cell selects its row.
/// - The secondary modifier (⌘ on macOS, Ctrl elsewhere) adds or removes
///   one row, Shift selects the rows from the anchor to the clicked row,
///   and both together add that range.
/// - A right click outside the selection selects what it landed on, so the
///   context menu acts on it. Inside the selection it keeps the selection.
/// - The arrows, Home, End and the page keys move a cell within its row or
///   column, or one row. Shift is not needed for ranges from the keyboard.
///
/// Every change emits [`TableEvent::SelectionChanged`].
pub struct TableState<D: TableDelegate> {
    focus_handle: FocusHandle,
    delegate: D,
    pub(super) options: TableOptions,
    /// The bounds of the table container.
    bounds: Bounds<Pixels>,
    /// The bounds of the fixed head cols.
    fixed_head_cols_bounds: Bounds<Pixels>,

    col_groups: Vec<ColGroup>,
    /// The sorted columns in key order, the first one sorting first.
    sort_keys: Vec<usize>,
    header_layout: Vec<Vec<HeaderCell>>,

    /// Whether the table can loop selection, default is true.
    ///
    /// When the prev/next selection is out of the table bounds, the selection will loop to the other side.
    pub loop_selection: bool,
    /// Whether the table can select column.
    pub col_selectable: bool,
    /// Whether the table can select row.
    pub row_selectable: bool,
    /// Whether the table can select cell, default is false.
    ///
    /// When enabled:
    /// - Users can click on individual cells to select them
    /// - A row header column appears on the left for selecting entire rows
    ///   (can be hidden via [`Self::row_header`])
    /// - Keyboard navigation works at the cell level (arrow keys move between cells)
    /// - Right-click and double-click events are supported for cells
    pub cell_selectable: bool,
    /// Whether the row header column is visible when `cell_selectable` is enabled,
    /// default is `true`.
    ///
    /// Set to `false` to hide the narrow leftmost header column while keeping cell
    /// selection — useful when you want to put your own content (e.g. a row index
    /// column) on the left. When hidden, clicking the already-selected cell again
    /// escalates the selection to the whole row so users can still pick rows; row
    /// escalation requires `row_selectable` to be enabled.
    pub row_header: bool,
    /// Whether the row header column displays the 1-based row number, default is `false`.
    ///
    /// Only effective when `cell_selectable` and `row_header` are enabled. When set,
    /// the leftmost row header column widens to fit the largest row number and renders
    /// it, replacing the need for a separate data column to show row numbers.
    pub row_numbers: bool,
    /// Whether the table can sort.
    pub sortable: bool,
    /// Whether the table can resize columns.
    pub col_resizable: bool,
    /// Whether the table can move columns.
    pub col_movable: bool,
    /// Enable/disable fixed columns feature.
    pub col_fixed: bool,

    pub vertical_scroll_handle: UniformListScrollHandle,
    pub horizontal_scroll_handle: VirtualListScrollHandle,

    selection: TableSelection,
    /// Where a Shift click's range starts: the row of the last plain or
    /// secondary click, or of the last keyboard move.
    anchor_row: Option<usize>,
    /// The row the arrows move from. Unlike the selection it outlasts a
    /// switch to a column, so the arrows pick up where the rows left off.
    active_row: Option<usize>,
    /// What the open or last opened context menu belongs to.
    menu_target: Option<MenuTarget>,

    /// The column index that is being resized.
    resizing_col: Option<usize>,
    /// Where the press on a resize handle was, and the column's width then.
    resize_origin: Option<ResizeOrigin>,
    /// The column a double-click on its resize handle just fitted.
    ///
    /// The double-click's second press also starts a drag on the handle, which
    /// would move the fitted edge back under the pointer, so resize drags on
    /// this column are ignored until the next single press.
    fitted_col: Option<usize>,

    /// The insertion gap index (`0..=cols_count`) while dragging a column
    /// header: the dragged column will be inserted between the columns
    /// `gap - 1` and `gap` on drop.
    col_drag_gap: Option<usize>,

    /// The visible range of the rows and columns.
    visible_range: TableVisibleRange,

    _measure: Vec<Duration>,
    _load_more_task: Task<()>,
}

impl<D> TableState<D>
where
    D: TableDelegate,
{
    /// Create a new TableState with the given delegate.
    pub fn new(delegate: D, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            options: TableOptions::default(),
            delegate,
            col_groups: Vec::new(),
            sort_keys: Vec::new(),
            header_layout: Vec::new(),
            horizontal_scroll_handle: VirtualListScrollHandle::new(),
            vertical_scroll_handle: UniformListScrollHandle::new(),
            selection: TableSelection::None,
            anchor_row: None,
            active_row: None,
            menu_target: None,
            resizing_col: None,
            resize_origin: None,
            fitted_col: None,
            col_drag_gap: None,
            bounds: Bounds::default(),
            fixed_head_cols_bounds: Bounds::default(),
            visible_range: TableVisibleRange::default(),
            loop_selection: true,
            col_selectable: true,
            row_selectable: true,
            cell_selectable: false,
            row_header: true,
            row_numbers: false,
            sortable: true,
            col_movable: true,
            col_resizable: true,
            col_fixed: true,
            _load_more_task: Task::ready(()),
            _measure: Vec::new(),
        };

        this.prepare_col_groups(cx);
        this
    }

    /// Returns a reference to the delegate.
    pub fn delegate(&self) -> &D {
        &self.delegate
    }

    /// Returns a mutable reference to the delegate.
    pub fn delegate_mut(&mut self) -> &mut D {
        &mut self.delegate
    }

    /// Set to loop selection, default to true.
    pub fn loop_selection(mut self, loop_selection: bool) -> Self {
        self.loop_selection = loop_selection;
        self
    }

    /// Set to enable/disable column movable, default to true.
    pub fn col_movable(mut self, col_movable: bool) -> Self {
        self.col_movable = col_movable;
        self
    }

    /// Set to enable/disable column resizable, default to true.
    pub fn col_resizable(mut self, col_resizable: bool) -> Self {
        self.col_resizable = col_resizable;
        self
    }

    /// Set to enable/disable column sortable, default true
    pub fn sortable(mut self, sortable: bool) -> Self {
        self.sortable = sortable;
        self
    }

    /// Set to enable/disable row selectable, default true
    pub fn row_selectable(mut self, row_selectable: bool) -> Self {
        self.row_selectable = row_selectable;
        self
    }

    /// Set to enable/disable column selectable, default true
    pub fn col_selectable(mut self, col_selectable: bool) -> Self {
        self.col_selectable = col_selectable;
        self
    }

    /// Set to enable/disable cell selection, default is false.
    ///
    /// When enabled:
    /// - Individual cells become selectable by clicking
    /// - A row header column appears on the left side (can be hidden via [`Self::row_header`])
    /// - Keyboard navigation operates at the cell level
    /// - Double clicks on cells emit [`TableEvent::DoubleClickedCell`]
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let table_state = cx.new(|cx| {
    ///     TableState::new(delegate, window, cx)
    ///         .cell_selectable(true)  // Enable cell selection
    ///         .row_selectable(true)   // Also allow row selection via row header
    /// });
    /// ```
    pub fn cell_selectable(mut self, cell_selectable: bool) -> Self {
        self.cell_selectable = cell_selectable;
        self
    }

    /// Set whether the row header column is shown, default is `true`.
    ///
    /// Only effective when `cell_selectable` is `true` — otherwise the row header
    /// column is never rendered. Hide it when you want to use the leftmost column
    /// for your own content (e.g. a row index column).
    ///
    /// When hidden, the first click on a cell selects the cell; clicking the
    /// already-selected cell again escalates to selecting the whole row, so users
    /// can still pick rows without the dedicated header column. The row escalation
    /// requires `row_selectable` to be enabled.
    pub fn row_header(mut self, row_header: bool) -> Self {
        self.row_header = row_header;
        self
    }

    /// Set whether the row header column shows 1-based row numbers, default `false`.
    ///
    /// Only effective when both `cell_selectable` and `row_header` are enabled. The
    /// row header column widens to fit the largest row number so callers don't need
    /// a dedicated data column for row numbers.
    pub fn row_numbers(mut self, row_numbers: bool) -> Self {
        self.row_numbers = row_numbers;
        self
    }

    /// When we update columns or rows, we need to refresh the table.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.prepare_col_groups(cx);
    }

    /// Scroll to the row at the given index.
    pub fn scroll_to_row(&mut self, row_ix: usize, cx: &mut Context<Self>) {
        self.vertical_scroll_handle
            .scroll_to_item(row_ix, ScrollStrategy::Top);
        cx.notify();
    }

    // Scroll to the column at the given index.
    pub fn scroll_to_col(&mut self, col_ix: usize, cx: &mut Context<Self>) {
        let col_ix = col_ix.saturating_sub(self.fixed_left_cols_count());

        // Resolve the offset here instead of deferring it to the virtual list.
        //
        // The header and the rows share `horizontal_scroll_handle`, but only
        // the rows are a `VirtualList`, and a deferred `scroll_to_item` is
        // applied during that list's prepaint — which runs *after* the header
        // has already been rendered and prepainted with the old offset. The
        // header would therefore stay one frame behind the rows.
        match self.horizontal_offset_for_col(col_ix) {
            Some(offset_x) => {
                let mut offset = self.horizontal_scroll_handle.offset();
                offset.x = offset_x;
                self.horizontal_scroll_handle.set_offset(offset);
            }
            // Before the first layout the viewport size is unknown, let the
            // virtual list resolve the offset once it has been laid out.
            None => self
                .horizontal_scroll_handle
                .scroll_to_item(col_ix, ScrollStrategy::Top),
        }

        cx.notify();
    }

    /// The horizontal scroll offset that brings the scrollable (non-fixed)
    /// column at `col_ix` fully into view.
    ///
    /// This mirrors the nearest-edge behavior of [`VirtualListScrollHandle::scroll_to_item`]
    /// with [`ScrollStrategy::Top`]: an already visible column keeps the current
    /// offset, otherwise the column is aligned to the edge it overflows. Both
    /// of those move the offset towards a column that exists, so the result
    /// never runs past the content and needs no extra clamping.
    ///
    /// Returns `None` when the viewport size is not known yet, or when `col_ix`
    /// is out of range.
    fn horizontal_offset_for_col(&self, col_ix: usize) -> Option<Pixels> {
        let viewport_width = self.horizontal_scroll_handle.bounds().size.width;
        if viewport_width <= px(0.) {
            return None;
        }

        let cols = self.col_groups.get(self.fixed_left_cols_count()..)?;
        let col_left: Pixels = cols.get(..col_ix)?.iter().map(|col| col.width).sum();
        let col_right = col_left + cols.get(col_ix)?.width;
        let offset_x = self.horizontal_scroll_handle.offset().x;

        Some(if col_left + offset_x < px(0.) {
            -col_left
        } else if col_right + offset_x > viewport_width {
            viewport_width - col_right
        } else {
            offset_x
        })
    }

    /// The selection, see [`TableSelection`].
    pub fn selection(&self) -> &TableSelection {
        &self.selection
    }

    /// Sets the selection and scrolls it into view. [`TableSelection::None`]
    /// clears it.
    ///
    /// A later Shift click ranges from the first selected row, or from the
    /// selected cell's row.
    pub fn set_selection(&mut self, selection: TableSelection, cx: &mut Context<Self>) {
        let previous_row = self.active_row;
        match &selection {
            TableSelection::Rows(rows) => {
                self.anchor_row = rows.first();
                self.active_row = rows.last();
            }
            TableSelection::Cell(row_ix, _) => {
                self.anchor_row = Some(*row_ix);
                self.active_row = Some(*row_ix);
            }
            TableSelection::Column(_) => {}
            TableSelection::None => {
                self.anchor_row = None;
                self.active_row = None;
            }
        }
        match &selection {
            TableSelection::Rows(_) => {
                if let Some(row_ix) = self.active_row {
                    self.reveal_row(row_ix, previous_row);
                }
            }
            TableSelection::Cell(row_ix, col_ix) => {
                self.vertical_scroll_handle
                    .scroll_to_item(*row_ix, ScrollStrategy::Center);
                self.scroll_to_col(*col_ix, cx);
            }
            TableSelection::Column(col_ix) => self.scroll_to_col(*col_ix, cx),
            TableSelection::None => {}
        }
        self.select(selection, cx);
    }

    /// Clears the selection.
    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.set_selection(TableSelection::None, cx);
    }

    /// Selects every row.
    pub fn select_all_rows(&mut self, cx: &mut Context<Self>) {
        let rows_count = self.delegate.rows_count(cx);
        if rows_count == 0 {
            return;
        }
        self.anchor_row = Some(0);
        self.active_row = Some(rows_count - 1);
        self.select(TableSelection::Rows(RowSet::from_range(0..rows_count)), cx);
    }

    /// Replaces the selection and tells listeners, without scrolling. The
    /// callers keep the anchor and the active row in step.
    fn select(&mut self, selection: TableSelection, cx: &mut Context<Self>) {
        self.selection = selection;
        self.menu_target = None;
        cx.emit(TableEvent::SelectionChanged);
        cx.notify();
    }

    /// Scrolls `row_ix` into view at the edge it is reached from.
    fn reveal_row(&mut self, row_ix: usize, from_row: Option<usize>) {
        let is_down = from_row.is_none_or(|from_row| row_ix > from_row);
        self.vertical_scroll_handle.scroll_to_item(
            row_ix,
            if is_down {
                ScrollStrategy::Bottom
            } else {
                ScrollStrategy::Top
            },
        );
    }

    /// Returns the visible range of the rows and columns.
    ///
    /// See [`TableVisibleRange`].
    pub fn visible_range(&self) -> &TableVisibleRange {
        &self.visible_range
    }

    /// Dump the header row of the table.
    ///
    /// Batched exporters can read the headers once with this, then stream the
    /// rows with [`Self::dump_range`].
    pub fn headers(&self, cx: &App) -> Vec<String> {
        let columns_count = self.delegate.columns_count(cx);
        let mut headers = Vec::with_capacity(columns_count);
        for col_ix in 0..columns_count {
            let column = self.delegate.column(col_ix, cx);
            headers.push(column.name.to_string());
        }

        headers
    }

    /// Dump table data.
    ///
    /// Returns a tuple of (headers, rows) where each row is a vector of cell values.
    ///
    /// This materializes the complete table in memory. For large tables, prefer
    /// [`Self::dump_range`] and process rows in batches.
    pub fn dump(&self, cx: &App) -> (Vec<String>, Vec<Vec<String>>) {
        self.dump_range(0..self.delegate.rows_count(cx), cx)
    }

    /// Dump table data for the specified row range.
    ///
    /// Returns the same `(headers, rows)` shape as [`Self::dump`], with only
    /// the rows inside the clamped range.
    ///
    /// The requested range is clamped to the table's current row count. For
    /// large tables, callers can invoke this repeatedly with bounded ranges.
    pub fn dump_range(&self, range: Range<usize>, cx: &App) -> (Vec<String>, Vec<Vec<String>>) {
        let columns_count = self.delegate.columns_count(cx);
        let rows_count = self.delegate.rows_count(cx);
        let start = range.start.min(rows_count);
        let end = range.end.min(rows_count).max(start);

        let mut rows = Vec::with_capacity(end - start);
        for row_ix in start..end {
            let mut row = Vec::with_capacity(columns_count);
            for col_ix in 0..columns_count {
                row.push(self.delegate.cell_text(row_ix, col_ix, cx));
            }
            rows.push(row);
        }

        (self.headers(cx), rows)
    }

    /// Reads the column names from the delegate again, keeping the widths,
    /// order and sorting that [`Self::refresh`] would reset. For names that
    /// follow the interface language.
    pub fn refresh_column_names(&mut self, cx: &mut Context<Self>) {
        for (col_ix, col_group) in self.col_groups.iter_mut().enumerate() {
            col_group.column.name = self.delegate.column(col_ix, cx).name;
        }
        self.refresh_header_layout(cx);
    }

    /// Re-compute the header layout from the current delegate.
    ///
    /// Call this after changing delegate state that affects `group_headers`.
    pub fn refresh_header_layout(&mut self, cx: &mut Context<Self>) {
        self.update_header_layout(cx);
        cx.notify();
    }

    fn prepare_col_groups(&mut self, cx: &mut Context<Self>) {
        self.sort_keys.clear();
        self.col_groups = (0..self.delegate.columns_count(cx))
            .map(|col_ix| {
                let column = self.delegate().column(col_ix, cx);
                ColGroup {
                    width: column.width,
                    bounds: Bounds::default(),
                    column,
                }
            })
            .collect();

        self.update_header_layout(cx);
    }

    fn update_header_layout(&mut self, cx: &mut Context<Self>) {
        let group_rows = self.delegate.group_headers(cx);

        let mut layout = match group_rows.as_ref() {
            Some(rows) => Vec::with_capacity(rows.len() + 1),
            None => Vec::with_capacity(1),
        };

        if let Some(group_rows) = group_rows {
            for row in group_rows {
                let mut cell_row = Vec::with_capacity(row.len());
                let mut current_leaf_ix = 0;
                for group in row {
                    let mut width = px(0.);
                    let start_leaf_col_ix = current_leaf_ix;
                    for i in 0..group.span {
                        if current_leaf_ix + i < self.col_groups.len() {
                            width += self.col_groups[current_leaf_ix + i].width;
                        }
                    }
                    current_leaf_ix += group.span;
                    cell_row.push(HeaderCell {
                        label: group.label.clone(),
                        width,
                        col_span: group.span,
                        is_leaf: false,
                        leaf_col_ix: None,
                        start_leaf_col_ix,
                    });
                }
                layout.push(cell_row);
            }
        }

        let mut leaf_row = Vec::with_capacity(self.col_groups.len());
        for (ix, group) in self.col_groups.iter().enumerate() {
            leaf_row.push(HeaderCell {
                label: group.column.name.clone(),
                width: group.width,
                col_span: 1,
                is_leaf: true,
                leaf_col_ix: Some(ix),
                start_leaf_col_ix: ix,
            });
        }
        layout.push(leaf_row);

        self.header_layout = layout;
        self.col_groups.shrink_to_fit();
        cx.notify();
    }

    fn fixed_left_cols_count(&self) -> usize {
        if !self.col_fixed {
            return 0;
        }

        self.col_groups
            .iter()
            .filter(|col| col.column.fixed == Some(ColumnFixed::Left))
            .count()
    }

    /// One header row. An unset header height matches the body rows.
    fn header_row_height(&self) -> Pixels {
        self.options
            .header_row_height
            .unwrap_or(self.options.size.table_row_height())
    }

    /// Every header row, including column-group rows. An empty layout still
    /// reserves one row, matching the vertical scrollbar's top inset.
    fn header_band_height(&self) -> Pixels {
        self.header_row_height() * self.header_layout.len().max(1) as f32
    }

    fn page_item_count(&self) -> usize {
        let row_height = self.options.size.table_row_height();
        let body_height = (self.bounds.size.height - self.header_band_height()).max(px(0.));
        let count = (body_height / row_height).floor() as usize;
        count.max(1)
    }

    fn on_row_right_click(&mut self, row_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        // A cell inside the row has already taken this press.
        if window.default_prevented() {
            return;
        }
        self.open_menu_on(MenuTarget::Row(row_ix), window, cx);
    }

    fn on_cell_right_click(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.cell_selectable {
            return;
        }
        // Claim the press without stopping it: the row then leaves it to the
        // cell, and the context menu further out still opens.
        window.prevent_default();
        self.open_menu_on(MenuTarget::Cell(row_ix, col_ix), window, cx);
    }

    fn on_col_head_right_click(
        &mut self,
        col_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_menu_on(MenuTarget::Column(col_ix), window, cx);
    }

    /// Prepares the context menu a right press on `target` opens. The menu
    /// acts on the selection, so a target outside it becomes the selection
    /// first, as in Finder or Excel. Inside the selection, the selection
    /// stays whole and the target is only outlined.
    fn open_menu_on(&mut self, target: MenuTarget, window: &mut Window, cx: &mut Context<Self>) {
        // A claimed press skips the table's own focus on press, and the menu
        // returns focus to where it was when it opened, so focus first.
        window.focus(&self.focus_handle, cx);

        let selection = &self.selection;
        let in_selection = match target {
            MenuTarget::Row(row_ix) => selection.is_row_selected(row_ix),
            MenuTarget::Cell(row_ix, col_ix) => {
                selection.cell() == Some((row_ix, col_ix))
                    || selection.is_row_selected(row_ix)
                    || selection.column() == Some(col_ix)
            }
            MenuTarget::Column(col_ix) => selection.column() == Some(col_ix),
        };
        if !in_selection {
            match target {
                MenuTarget::Row(row_ix) if self.row_selectable => {
                    self.anchor_row = Some(row_ix);
                    self.active_row = Some(row_ix);
                    self.select(TableSelection::row(row_ix), cx);
                }
                MenuTarget::Cell(row_ix, col_ix) => {
                    self.anchor_row = Some(row_ix);
                    self.active_row = Some(row_ix);
                    self.select(TableSelection::Cell(row_ix, col_ix), cx);
                }
                MenuTarget::Column(col_ix) if self.is_col_selectable(col_ix) => {
                    self.set_selection(TableSelection::Column(col_ix), cx);
                }
                _ => {}
            }
        }
        self.menu_target = Some(target);
        cx.notify();
    }

    fn on_row_left_click(
        &mut self,
        e: &ClickEvent,
        row_ix: usize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.row_selectable {
            return;
        }

        cx.stop_propagation();
        self.click_row(row_ix, e.modifiers(), cx);

        if e.click_count() == 2 {
            cx.emit(TableEvent::DoubleClickedRow(row_ix));
        }
    }

    /// Selects rows for a click on `row_ix`. A plain click selects the row,
    /// the secondary modifier adds or removes it, and Shift selects the rows
    /// from the anchor to it, added to the selection with both held.
    fn click_row(&mut self, row_ix: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        let extend = modifiers.secondary(cx);
        let mut rows = match self.selection.selected_rows() {
            Some(rows) if extend => rows.clone(),
            _ => RowSet::new(),
        };
        if modifiers.shift {
            let anchor = self.anchor_row.unwrap_or(row_ix);
            rows.insert_range(anchor.min(row_ix)..anchor.max(row_ix) + 1);
        } else {
            if extend {
                rows.toggle(row_ix);
            } else {
                rows.insert(row_ix);
            }
            self.anchor_row = Some(row_ix);
        }
        self.active_row = Some(row_ix);
        self.select(TableSelection::from_rows(rows), cx);
    }

    /// Whether the column at `col_ix` can be selected.
    fn is_col_selectable(&self, col_ix: usize) -> bool {
        self.col_selectable
            && self
                .col_groups
                .get(col_ix)
                .is_some_and(|col_group| col_group.column.selectable)
    }

    fn on_col_head_click(&mut self, col_ix: usize, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_col_selectable(col_ix) {
            self.set_selection(TableSelection::Column(col_ix), cx);
        }
    }

    fn on_cell_click(
        &mut self,
        e: &ClickEvent,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.cell_selectable {
            return;
        }
        cx.stop_propagation();

        let modifiers = e.modifiers();
        let is_double_click = e.click_count() == 2;
        if self.row_selectable && (modifiers.secondary(cx) || modifiers.shift) {
            // A modified click picks rows, wherever in the row it lands.
            self.click_row(row_ix, modifiers, cx);
        } else {
            // Without the row header, a second click on the selected cell
            // selects its row instead. A double click never does, so it
            // reaches `DoubleClickedCell` with the cell still selected.
            let to_row = !self.row_header
                && self.row_selectable
                && !is_double_click
                && self.selection.cell() == Some((row_ix, col_ix));
            self.anchor_row = Some(row_ix);
            self.active_row = Some(row_ix);
            if to_row {
                self.select(TableSelection::row(row_ix), cx);
                return;
            }
            // The cell is under the pointer, but its column may be cut off
            // at an edge.
            self.scroll_to_col(col_ix, cx);
            self.select(TableSelection::Cell(row_ix, col_ix), cx);
        }

        if is_double_click {
            cx.emit(TableEvent::DoubleClickedCell(row_ix, col_ix));
        }
    }

    pub(super) fn action_cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if self.selection.is_none() {
            cx.propagate();
            return;
        }
        self.clear_selection(cx);
    }

    pub(super) fn action_select_all(
        &mut self,
        _: &SelectAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.row_selectable || self.delegate.rows_count(cx) == 0 {
            cx.propagate();
            return;
        }
        self.select_all_rows(cx);
    }

    /// Moves the selected cell, or one selected row, to `row_ix`, as the
    /// vertical keys do. `row_ix` maps the current row to the new one.
    fn move_vertically(
        &mut self,
        cx: &mut Context<Self>,
        row_ix: impl FnOnce(Option<usize>, usize) -> usize,
    ) {
        let rows_count = self.delegate.rows_count(cx);
        if rows_count == 0 {
            return;
        }
        let clamp = |row_ix: usize| row_ix.min(rows_count - 1);
        if let Some((cell_row, col_ix)) = self.selection.cell() {
            let target = clamp(row_ix(Some(cell_row), rows_count));
            self.set_selection(TableSelection::Cell(target, col_ix), cx);
        } else if self.row_selectable {
            let target = clamp(row_ix(self.active_row.map(clamp), rows_count));
            self.set_selection(TableSelection::row(target), cx);
        }
    }

    /// Moves the selected cell or column to `col_ix`, as the horizontal keys
    /// do. From rows or nothing it starts a cell in the active row, or a
    /// column, and leaves the key to the window when neither is possible.
    fn move_horizontally(
        &mut self,
        cx: &mut Context<Self>,
        col_ix: impl FnOnce(Option<usize>, usize) -> usize,
    ) {
        let columns_count = self.delegate.columns_count(cx);
        if columns_count == 0 {
            cx.propagate();
            return;
        }
        let clamp = |col_ix: usize| col_ix.min(columns_count - 1);
        match self.selection {
            TableSelection::Cell(row_ix, cell_col) => {
                let target = clamp(col_ix(Some(cell_col), columns_count));
                self.set_selection(TableSelection::Cell(row_ix, target), cx);
            }
            TableSelection::Column(selected_col) => {
                let target = clamp(col_ix(Some(selected_col), columns_count));
                self.set_selection(TableSelection::Column(target), cx);
            }
            _ if self.cell_selectable && self.delegate.rows_count(cx) > 0 => {
                let rows_count = self.delegate.rows_count(cx);
                let row_ix = self.active_row.unwrap_or(0).min(rows_count - 1);
                let target = clamp(col_ix(None, columns_count));
                self.set_selection(TableSelection::Cell(row_ix, target), cx);
            }
            _ if self.col_selectable => {
                let target = clamp(col_ix(None, columns_count));
                self.set_selection(TableSelection::Column(target), cx);
            }
            _ => cx.propagate(),
        }
    }

    pub(super) fn action_select_prev(
        &mut self,
        _: &SelectUp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let loop_selection = self.loop_selection;
        // Nothing selected starts from the top, so up wraps to the last row.
        self.move_vertically(cx, |row_ix, count| match row_ix.unwrap_or(0) {
            0 if loop_selection => count - 1,
            0 => 0,
            row_ix => row_ix - 1,
        });
    }

    pub(super) fn action_select_next(
        &mut self,
        _: &SelectDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let loop_selection = self.loop_selection;
        self.move_vertically(cx, |row_ix, count| match row_ix {
            Some(row_ix) if row_ix + 1 < count => row_ix + 1,
            Some(row_ix) if !loop_selection => row_ix,
            _ => 0,
        });
    }

    pub(super) fn action_select_page_up(
        &mut self,
        _: &SelectPageUp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let step = self.page_item_count();
        self.move_vertically(cx, |row_ix, _| row_ix.unwrap_or(0).saturating_sub(step));
    }

    pub(super) fn action_select_page_down(
        &mut self,
        _: &SelectPageDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let step = self.page_item_count();
        self.move_vertically(cx, |row_ix, _| row_ix.unwrap_or(0) + step);
    }

    /// Home: the first cell of the row, the first column, or the first row.
    pub(super) fn action_select_first_column(
        &mut self,
        _: &SelectFirst,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.selection {
            TableSelection::Cell(..) | TableSelection::Column(_) => {
                self.move_horizontally(cx, |_, _| 0)
            }
            _ if self.row_selectable => self.move_vertically(cx, |_, _| 0),
            _ => self.move_horizontally(cx, |_, _| 0),
        }
    }

    /// End: the last cell of the row, the last column, or the last row.
    pub(super) fn action_select_last_column(
        &mut self,
        _: &SelectLast,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.selection {
            TableSelection::Cell(..) | TableSelection::Column(_) => {
                self.move_horizontally(cx, |_, count| count - 1)
            }
            _ if self.row_selectable => self.move_vertically(cx, |_, count| count - 1),
            _ => self.move_horizontally(cx, |_, count| count - 1),
        }
    }

    pub(super) fn action_select_prev_col(
        &mut self,
        _: &SelectPrevColumn,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let loop_selection = self.loop_selection;
        self.move_horizontally(cx, |col_ix, count| match col_ix {
            Some(0) if !loop_selection => 0,
            Some(0) | None => count - 1,
            Some(col_ix) => col_ix - 1,
        });
    }

    pub(super) fn action_select_next_col(
        &mut self,
        _: &SelectNextColumn,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let loop_selection = self.loop_selection;
        self.move_horizontally(cx, |col_ix, count| match col_ix {
            Some(col_ix) if col_ix + 1 < count => col_ix + 1,
            Some(col_ix) if !loop_selection => col_ix,
            _ => 0,
        });
    }

    /// Scroll table when mouse position is near the edge of the table bounds.
    fn scroll_table_by_col_resizing(
        &mut self,
        mouse_position: Point<Pixels>,
        col_group: &ColGroup,
    ) {
        // Do nothing if pos out of the table bounds right for avoid scroll to the right.
        if mouse_position.x > self.bounds.right() {
            return;
        }

        let mut offset = self.horizontal_scroll_handle.offset();
        let col_bounds = col_group.bounds;

        if mouse_position.x < self.bounds.left()
            && col_bounds.right() < self.bounds.left() + px(20.)
        {
            offset.x += px(1.);
        } else if mouse_position.x > self.bounds.right()
            && col_bounds.right() > self.bounds.right() - px(20.)
        {
            offset.x -= px(1.);
        }

        self.horizontal_scroll_handle.set_offset(offset);
    }

    /// The width column `col_ix` is drawn at now, or `None` past the last
    /// column.
    ///
    /// This is the table's own width, which follows a drag or a fit as it
    /// happens; the delegate's [`Column::width`] is read again only on
    /// [`Self::refresh`].
    pub fn col_width(&self, col_ix: usize) -> Option<Pixels> {
        self.col_groups.get(col_ix).map(|group| group.width)
    }

    /// Fits column `col_ix` to its content, as a double-click on its resize
    /// handle does.
    ///
    /// The width is [`TableDelegate::fit_column`] plus the cell padding,
    /// clamped to the column's `min_width` and `max_width`, and
    /// [`TableEvent::ColumnWidthsChanged`] is emitted when it changed. Returns
    /// whether the delegate gave a width: `false` when the column cannot be
    /// resized or the delegate answered `None`.
    pub fn fit_col(&mut self, col_ix: usize, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.is_col_resizable(col_ix) {
            return false;
        }
        let Some(content) = self.delegate.fit_column(col_ix, window, cx) else {
            return false;
        };
        let padding = self
            .col_groups
            .get(col_ix)
            .and_then(|group| group.column.paddings)
            .unwrap_or_else(|| self.options.size.table_cell_padding());
        // Layout rounds a width to the nearest device pixel, and text even a
        // fraction of a pixel wider than its cell is truncated, so round every
        // part up to a whole device pixel.
        let scale_factor = window.scale_factor();
        let device_pixels = |length: Pixels| (f32::from(length) * scale_factor).ceil();
        let mut content_pixels = device_pixels(content);
        // Dividing back can land one ulp below the content at a fractional
        // scale, and the text is cut for that ulp too.
        if px(content_pixels / scale_factor) < content {
            content_pixels += 1.;
        }
        let width = content_pixels + device_pixels(padding.left) + device_pixels(padding.right);
        if self.resize_cols(col_ix, px(width / scale_factor), window, cx) {
            let new_widths = self.col_groups.iter().map(|g| g.width).collect();
            cx.emit(TableEvent::ColumnWidthsChanged(new_widths));
        }
        true
    }

    /// The `ix`` is the index of the col to resize,
    /// and the `size` is the new size for the col.
    ///
    /// Returns whether the width changed.
    fn resize_cols(
        &mut self,
        ix: usize,
        size: Pixels,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.col_resizable {
            return false;
        }

        let mut changed = false;
        if let Some(col_group) = self.col_groups.get_mut(ix) {
            if col_group.is_resizable() {
                let new_width = size
                    .max(col_group.column.min_width)
                    .min(col_group.column.max_width);
                if col_group.width != new_width {
                    col_group.width = new_width;
                    changed = true;
                }
            }
        }

        if changed {
            self.update_header_layout(cx);
            cx.notify();
        }
        changed
    }

    /// The sort keys in order, each a column index and its direction.
    pub fn sort_keys(&self) -> Vec<(usize, ColumnSort)> {
        self.sort_keys
            .iter()
            .filter_map(|&col_ix| {
                let sort = self.col_groups.get(col_ix)?.column.sort?;
                Some((col_ix, sort))
            })
            .collect()
    }

    /// Step the sort of the column at `col_ix`. With `add_key`, and when the
    /// delegate sorts by several columns, the other keys stay and the column
    /// is added as the next key; otherwise it becomes the only one.
    fn perform_sort(
        &mut self,
        col_ix: usize,
        add_key: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sortable {
            return;
        }

        let Some(sort) = self.col_groups.get(col_ix).and_then(|g| g.column.sort) else {
            return;
        };

        let sort = match sort {
            ColumnSort::Ascending => ColumnSort::Default,
            ColumnSort::Descending => ColumnSort::Ascending,
            ColumnSort::Default => ColumnSort::Descending,
        };
        let add_key = add_key && self.delegate.multi_sortable();

        if !add_key {
            for (ix, col_group) in self.col_groups.iter_mut().enumerate() {
                if ix == col_ix {
                    col_group.column.sort = Some(sort);
                } else if col_group.column.sort.is_some() {
                    col_group.column.sort = Some(ColumnSort::Default);
                }
            }
            self.sort_keys.clear();
            if sort != ColumnSort::Default {
                self.sort_keys.push(col_ix);
            }
            self.delegate_mut().perform_sort(col_ix, sort, window, cx);
            cx.notify();
            return;
        }

        self.col_groups[col_ix].column.sort = Some(sort);
        if sort == ColumnSort::Default {
            self.sort_keys.retain(|&ix| ix != col_ix);
        } else if !self.sort_keys.contains(&col_ix) {
            self.sort_keys.push(col_ix);
        }
        let keys = self.sort_keys();
        self.delegate_mut().perform_multi_sort(&keys, window, cx);

        cx.notify();
    }

    fn move_column(
        &mut self,
        col_ix: usize,
        to_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if col_ix == to_ix {
            return;
        }

        self.delegate.move_column(col_ix, to_ix, window, cx);
        let col_group = self.col_groups.remove(col_ix);
        self.col_groups.insert(to_ix, col_group);
        for key in &mut self.sort_keys {
            *key = moved_index(*key, col_ix, to_ix);
        }

        cx.emit(TableEvent::MoveColumn(col_ix, to_ix));
        cx.notify();
    }

    /// Resolve the insertion gap for a column-header drag at the window
    /// coordinate `x`, or `None` when dropping there would not move the
    /// dragged column at `drag_col_ix`.
    fn drag_gap_at(&self, x: Pixels, drag_col_ix: usize) -> Option<usize> {
        let fixed_count = self.fixed_left_cols_count();

        // A column can only be reordered within its own region: rendering
        // pins the first `fixed_count` columns, so a cross-region move would
        // change which columns are pinned without updating their `fixed`
        // flags.
        let drag_in_fixed = drag_col_ix < fixed_count;
        let pointer_in_fixed = fixed_count > 0 && x < self.fixed_head_cols_bounds.right();
        if drag_in_fixed != pointer_in_fixed {
            return None;
        }

        // Columns scrolled beneath the fixed region keep stale bounds, so
        // resolve `x` against the fixed columns alone when it falls in that
        // region, and against the visible scrollable columns otherwise.
        let candidates = if pointer_in_fixed {
            0..fixed_count
        } else {
            self.calculate_visible_leaf_col_range(fixed_count).0
        };

        // The gap sits after the last candidate column whose center is left of `x`.
        let mut gap = candidates.start;
        for ix in candidates {
            if x < self.col_groups[ix].bounds.center().x {
                break;
            }
            gap = ix + 1;
        }

        // No gap if dropping there would put the dragged column back to
        // where it already is.
        if gap == drag_col_ix || gap == drag_col_ix + 1 {
            None
        } else {
            Some(gap)
        }
    }

    /// Dispatch delegate's `load_more` method when the visible range is near the end.
    fn load_more_if_need(
        &mut self,
        rows_count: usize,
        visible_end: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let threshold = self.delegate.load_more_threshold();
        // Securely handle subtract logic to prevent attempt to subtract with overflow
        if visible_end >= rows_count.saturating_sub(threshold) {
            if !self.delegate.has_more(cx) {
                return;
            }

            self._load_more_task = cx.spawn_in(window, async move |view, window| {
                _ = view.update_in(window, |view, window, cx| {
                    view.delegate.load_more(window, cx);
                });
            });
        }
    }

    fn update_visible_range_if_need(
        &mut self,
        visible_range: Range<usize>,
        items_count: usize,
        axis: Axis,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Skip when visible range is only 1 item, unless there is at most 1 item.
        // The visual_list will use first item to measure.
        if visible_range.len() <= 1 && items_count > 1 {
            return;
        }

        // Stripe filler rows are rendered past the last row; never report them.
        let end = visible_range.end.min(items_count);
        let visible_range = visible_range.start.min(end)..end;
        // A range wholly past the last item is stale; the list scrolls back next frame.
        if visible_range.is_empty() && items_count > 0 {
            return;
        }

        if axis == Axis::Vertical {
            if self.visible_range.rows == visible_range {
                return;
            }
            self.delegate_mut()
                .visible_rows_changed(visible_range.clone(), window, cx);
            self.visible_range.rows = visible_range;
        } else {
            if self.visible_range.cols == visible_range {
                return;
            }
            self.delegate_mut()
                .visible_columns_changed(visible_range.clone(), window, cx);
            self.visible_range.cols = visible_range;
        }
    }

    fn render_cell(
        &self,
        _row_ix: Option<usize>,
        col_ix: usize,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Div {
        let Some(col_group) = self.col_groups.get(col_ix) else {
            return div();
        };

        let col_width = col_group.width;
        let col_padding = col_group.column.paddings;

        div()
            .w(col_width)
            .h_full()
            .flex_shrink_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .table_cell_size(self.options.size)
            // Text alignment is inherited, so this aligns the header name and
            // the delegate's cell text without changing how cells lay out.
            .map(|this| match col_group.column.align {
                TextAlign::Center => this.text_center(),
                TextAlign::Right => this.text_right(),
                _ => this,
            })
            .map(|this| match col_padding {
                Some(padding) => this
                    .pl(padding.left)
                    .pr(padding.right)
                    .pt(padding.top)
                    .pb(padding.bottom),
                None => this,
            })
    }

    /// Expose a body cell to assistive technology: its role, its column and
    /// the delegate's [`cell_label`](TableDelegate::cell_label).
    ///
    /// In cell selection mode the cell is a `GridCell` that reports whether it
    /// is selected; otherwise it is a plain `Cell`.
    fn a11y_cell<E>(&self, cell: E, row_ix: usize, col_ix: usize, window: &Window, cx: &App) -> E
    where
        E: gpui::StatefulInteractiveElement + FluentBuilder,
    {
        let cell = if self.cell_selectable {
            let is_selected = self.selection.cell() == Some((row_ix, col_ix));
            cell.role(gpui::Role::GridCell)
                .aria_selected(is_selected)
                // While the table has focus, the selected cell is the
                // focused node for assistive technology.
                .when(
                    is_selected && self.focus_handle.is_focused(window),
                    |this| this.aria_active_descendant(),
                )
        } else {
            cell.role(gpui::Role::Cell)
        };
        // Labels are only read through the accessibility tree, so skip
        // building them when no assistive technology is connected (tests
        // read them through test support).
        let label = if window.is_a11y_active() || cfg!(test) {
            self.delegate.cell_label(row_ix, col_ix, cx)
        } else {
            None
        };
        cell.aria_column_index(col_ix)
            .when_some(label, |this, label| this.aria_label(label))
    }

    /// Show Column selection style, when the column is selected and the selection state is Column.
    /// Note: When a cell is selected, column selection style is not shown.
    fn render_col_wrap(
        &self,
        _row_ix: Option<usize>,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let el = h_flex().h_full();
        let selectable = self.col_selectable
            && self
                .col_groups
                .get(col_ix)
                .map(|col_group| col_group.column.selectable)
                .unwrap_or(false);

        if selectable && self.selection.column() == Some(col_ix) {
            el.bg(cx.theme().tokens.table_active)
        } else {
            el
        }
    }

    /// The other half of that band, for the boundary on the *left* of column
    /// `ix` — the one owned by column `ix - 1`.
    ///
    /// It is positioned absolutely so that reaching back over the boundary
    /// costs the header row no width, and it paints after this column's own
    /// cell so that it, and not a column reorder, receives the drag.
    fn render_leading_resize_handle(
        &self,
        ix: usize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(prev) = ix.checked_sub(1).filter(|ix| self.is_col_resizable(*ix)) else {
            return div().into_any_element();
        };

        self.resize_handle_band(prev, ("resizable-handle-leading", ix).into(), cx)
            .absolute()
            .top_0()
            .left_0()
            .h_full()
            .w(HANDLE_PADDING)
            .into_any_element()
    }

    /// The half of column `ix`'s resize handle that lies inside column `ix`,
    /// along with the hairline that marks the boundary.
    ///
    /// It is [`HANDLE_PADDING`] wide, like the half on the far side, so the
    /// grab area is symmetric about the boundary. The negative margin keeps
    /// the band's net contribution to the header row zero, and `justify_end`
    /// pins the hairline to the column edge.
    fn render_resize_handle(
        &self,
        ix: usize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if !self.is_col_resizable(ix) {
            return div().into_any_element();
        }

        let group_id = SharedString::from(format!("resizable-handle:{}", ix));

        self.resize_handle_band(ix, ("resizable-handle", ix).into(), cx)
            .group(group_id.clone())
            .h_full()
            .w(HANDLE_PADDING)
            .ml(-HANDLE_PADDING)
            .justify_end()
            .items_center()
            .child(
                div()
                    .h_full()
                    .justify_center()
                    .bg(cx.theme().table_row_border)
                    .group_hover(&group_id, |this| this.bg(cx.theme().border).h_full())
                    .w(px(1.)),
            )
            .into_any_element()
    }

    fn is_col_resizable(&self, ix: usize) -> bool {
        self.col_resizable
            && self
                .col_groups
                .get(ix)
                .map(|col| col.is_resizable())
                .unwrap_or(false)
    }

    /// The interactive band that drags the boundary on the right of column
    /// `ix`, carrying no styling that places or paints it.
    ///
    /// Paint order decides which element is offered a drag first: later
    /// siblings win, and a header cell starts a column reorder. So a single
    /// band straddling the boundary would lose its outer half to the next
    /// column's cell. Each boundary is covered by two bands instead, one in
    /// the `th` on either side, each rendered after that `th`'s own cell and
    /// so above everything it overlaps.
    fn resize_handle_band(
        &self,
        ix: usize,
        id: ElementId,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        h_flex()
            .id(id)
            .occlude()
            .cursor_col_resize()
            .on_drag_move(
                cx.listener(move |view, e: &DragMoveEvent<ResizeColumn>, window, cx| {
                    match e.drag(cx) {
                        ResizeColumn((entity_id, ix)) => {
                            if cx.entity_id() != *entity_id {
                                return;
                            }

                            // sync col widths into real widths
                            // TODO: Consider to remove this, this may not need now.
                            // for (_, col_group) in view.col_groups.iter_mut().enumerate() {
                            //     col_group.width = col_group.bounds.size.width;
                            // }

                            let ix = *ix;
                            if view.fitted_col == Some(ix) {
                                return;
                            }
                            view.resizing_col = Some(ix);

                            let col_group = view
                                .col_groups
                                .get(ix)
                                .expect("BUG: invalid col index")
                                .clone();

                            // The width at the press, plus how far the pointer
                            // has moved since. The column's left edge isn't a
                            // fixed point: it moves when the table scrolls,
                            // and the scroll offset clamps as the columns
                            // narrow.
                            let Some(origin) = view.resize_origin else {
                                return;
                            };
                            view.resize_cols(
                                ix,
                                origin.width + (e.event.position.x - origin.x),
                                window,
                                cx,
                            );

                            // scroll the table if the drag is near the edge
                            view.scroll_table_by_col_resizing(e.event.position, &col_group);
                        }
                    };
                }),
            )
            .on_drag(ResizeColumn((cx.entity_id(), ix)), |drag, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, e: &MouseDownEvent, window, cx| {
                    view.resize_origin = view.col_groups.get(ix).map(|col_group| ResizeOrigin {
                        x: e.position.x,
                        width: col_group.width,
                    });
                    if e.click_count == 1 {
                        view.fitted_col = None;
                    } else if e.click_count == 2 && view.fit_col(ix, window, cx) {
                        view.fitted_col = Some(ix);
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| {
                    view.resize_origin = None;
                    if view.resizing_col.is_none() {
                        return;
                    }

                    view.resizing_col = None;

                    let new_widths = view.col_groups.iter().map(|g| g.width).collect();
                    cx.emit(TableEvent::ColumnWidthsChanged(new_widths));
                    cx.notify();
                }),
            )
    }

    /// The border on the right edge of the fixed columns, unless turned off
    /// with [`DataTable::fixed_cols_border`].
    fn render_fixed_cols_border(&self, cx: &App) -> Option<Div> {
        self.options.fixed_cols_border.then(|| {
            div()
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .w_0()
                .flex_shrink_0()
                .border_r_1()
                .border_color(cx.theme().border)
        })
    }

    /// The width of the row header column. When `row_numbers` is enabled, the width
    /// grows to fit the largest row number; otherwise it's a narrow selection handle.
    fn row_header_width(&self, cx: &App) -> Pixels {
        if !self.row_numbers {
            return px(12.);
        }

        let count = self.delegate.rows_count(cx).max(1);
        let digits = count.to_string().len().max(2) as f32;
        px(16. + digits * 8.)
    }

    /// Render the row header cell (when cell_selectable is enabled)
    fn render_row_header_cell(
        &self,
        row_ix: usize,
        is_head: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let width = self.row_header_width(cx);

        div()
            .id(("row-header", row_ix))
            .w(width)
            .h_full()
            .border_r_1()
            // The head cell needs its own bottom border in the header-separator
            // color, since the header's bottom border only spans the columns area
            // to the right. Without it the first row looks like it's missing a top
            // border under the row-number column. Body cells inherit their bottom
            // border from the parent row, so they only need the right border.
            .map(|this| {
                if is_head {
                    this.border_b_1().border_color(cx.theme().border)
                } else {
                    this.border_color(cx.theme().table_row_border)
                }
            })
            .bg(cx.theme().tokens.table_head)
            .flex_shrink_0()
            .table_cell_size(self.options.size)
            .when(self.row_numbers && !is_head, |this| {
                this.flex()
                    .items_center()
                    .justify_end()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child((row_ix + 1).to_string())
            })
            .when(self.row_selectable, |this| {
                this.cursor_pointer().on_click(cx.listener(
                    move |table, event: &ClickEvent, window, cx| {
                        if is_head {
                            table.select_all_rows(cx);
                        } else {
                            table.on_row_left_click(event, row_ix, window, cx);
                        }
                    },
                ))
            })
    }

    fn render_sort_icon(
        &self,
        col_ix: usize,
        col_group: &ColGroup,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        if !self.sortable {
            return None;
        }

        let Some(sort) = col_group.column.sort else {
            return None;
        };

        let (icon, is_on) = match sort {
            ColumnSort::Ascending => (IconName::SortAscending, true),
            ColumnSort::Descending => (IconName::SortDescending, true),
            ColumnSort::Default => (IconName::ChevronsUpDown, false),
        };
        // With more than one key, each sorted column shows its place.
        let key_position = (self.sort_keys.len() > 1)
            .then(|| self.sort_keys.iter().position(|&ix| ix == col_ix))
            .flatten()
            .map(|position| position + 1);

        Some(
            div()
                .id(("icon-sort", col_ix))
                .test_support()
                .role(gpui::Role::Button)
                .aria_label(t!("ui.table.sort_by", column = col_group.column.name).to_string())
                .p(px(2.))
                .rounded(cx.theme().radius / 2.)
                .map(|this| match is_on {
                    true => this,
                    false => this.opacity(0.5),
                })
                .hover(|this| this.bg(cx.theme().tokens.secondary).opacity(7.))
                .active(|this| this.bg(cx.theme().tokens.secondary_active).opacity(1.))
                .on_click(cx.listener(move |table, e: &ClickEvent, window, cx| {
                    table.perform_sort(col_ix, e.modifiers().shift, window, cx)
                }))
                .when(key_position.is_some(), |this| {
                    this.flex().items_center().gap(px(1.))
                })
                .child(
                    Icon::new(icon)
                        .size_3()
                        .text_color(cx.theme().secondary_foreground),
                )
                .when_some(key_position, |this, position| {
                    this.child(
                        div()
                            .text_xs()
                            .line_height(rems(0.75))
                            .text_color(cx.theme().secondary_foreground)
                            .child(position.to_string()),
                    )
                }),
        )
    }

    /// Render the column header.
    /// The children must be one by one items.
    /// Because the horizontal scroll handle will use the child_item_bounds to
    /// calculate the item position for itself's `scroll_to_item` method.
    fn render_th(&mut self, col_ix: usize, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let entity_id = cx.entity_id();
        let col_group = self.col_groups.get(col_ix).expect("BUG: invalid col index");

        let movable = self.col_movable && col_group.column.movable;
        let paddings = col_group.column.paddings;
        let name = col_group.column.name.clone();

        h_flex()
            .h_full()
            .child(
                self.render_cell(None, col_ix, window, cx)
                    .id(("col-header", col_ix))
                    .role(gpui::Role::ColumnHeader)
                    .aria_label(name.clone())
                    .aria_column_index(col_ix)
                    .test_support()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.on_col_head_click(col_ix, window, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _, window, cx| {
                            this.on_col_head_right_click(col_ix, window, cx);
                        }),
                    )
                    .child(
                        h_flex()
                            .size_full()
                            .justify_between()
                            .items_center()
                            .child(self.delegate.render_th(col_ix, window, cx))
                            .when_some(paddings, |this, paddings| {
                                // Leave right space for the sort icon, if this column have custom padding
                                let offset_pr =
                                    self.options.size.table_cell_padding().right - paddings.right;
                                this.pr(offset_pr.max(px(0.)))
                            })
                            .children(self.render_sort_icon(col_ix, &col_group, window, cx)),
                    )
                    .when(movable, |this| {
                        this.on_drag(
                            DragColumn {
                                entity_id,
                                col_ix,
                                name,
                                width: col_group.width,
                            },
                            |drag, _, _, cx| {
                                cx.stop_propagation();
                                cx.new(|_| drag.clone())
                            },
                        )
                    })
                    .map(|this| {
                        // Draw the insertion indicator on the left edge of the gap
                        // column, or on the right edge of the last column for the
                        // trailing gap. Use an absolutely positioned overlay instead
                        // of a border, to avoid shifting the cell content.
                        let last_gap = col_ix + 1 == self.col_groups.len();
                        match self.col_drag_gap {
                            Some(gap)
                                if cx.has_active_drag()
                                    && (gap == col_ix || (last_gap && gap == col_ix + 1)) =>
                            {
                                let right_side = gap == col_ix + 1;
                                this.relative().child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .bottom_0()
                                        .w(px(2.))
                                        .map(|d| if right_side { d.right_0() } else { d.left_0() })
                                        .bg(cx.theme().drag_border),
                                )
                            }
                            _ => this,
                        }
                    }),
            )
            // resize handle cell right side
            .child(self.render_resize_handle(col_ix, window, cx))
            // resize handle cell left side
            .child(self.render_leading_resize_handle(col_ix, window, cx))
            // to save the bounds of this col.
            .on_prepaint({
                let view = cx.entity();
                move |bounds, _, cx| view.update(cx, |r, _| r.col_groups[col_ix].bounds = bounds)
            })
    }

    /// Compute the visible non-fixed leaf-column range for header rendering.
    ///
    /// Returns `(visible_range, left_spacer_width)` where:
    /// - `visible_range` is the column-index range that should be rendered.
    /// - `left_spacer_width` is the total width of the off-screen left columns,
    ///   used as a spacer div to keep visible columns at the correct position.
    ///
    /// On the first frame `self.bounds` is zero, so a fallback that covers all
    /// columns is returned to avoid a blank header on initial paint.
    fn calculate_visible_leaf_col_range(
        &self,
        left_columns_count: usize,
    ) -> (Range<usize>, Pixels) {
        let total_cols = self.col_groups.len();

        if self.bounds.size.width == px(0.) {
            return (left_columns_count..total_cols, px(0.));
        }

        let fixed_width = self.fixed_head_cols_bounds.size.width;
        let available_width = (self.bounds.size.width - fixed_width).max(px(0.));
        // The scroll handle offset is negative when scrolled right; negate it
        // to obtain a positive distance from the left edge of the scroll area.
        let scroll_x = (-self.horizontal_scroll_handle.offset().x).max(px(0.));

        // Walk left-to-right through non-fixed columns to find the first one
        // whose right edge enters the viewport. The accumulated width of the
        // skipped columns becomes the left spacer width.
        let mut range_start = left_columns_count;
        let mut left_spacer = px(0.);
        let mut cumulative = px(0.);
        for i in left_columns_count..total_cols {
            let right_edge = cumulative + self.col_groups[i].width;
            if right_edge > scroll_x {
                range_start = i;
                left_spacer = cumulative;
                break;
            }
            cumulative = right_edge;
        }

        // Continue from `range_start` (skipping already-scanned columns) to
        // find the last column still within the viewport. The 200 px overdraw
        // buffer prevents a visible flash when the user scrolls quickly.
        let right_bound = scroll_x + available_width + px(200.);
        let mut range_end = total_cols;
        let mut cumulative = left_spacer; // already summed widths before `range_start`
        for i in range_start..total_cols {
            cumulative += self.col_groups[i].width;
            if cumulative > right_bound {
                range_end = (i + 1).min(total_cols);
                break;
            }
        }

        (range_start..range_end, left_spacer)
    }

    fn render_table_header(
        &mut self,
        left_columns_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let view = cx.entity();
        let horizontal_scroll_handle = self.horizontal_scroll_handle.clone();

        // Header leaf-column virtualization.
        //
        // `render_th` creates interactive elements with resize-handle listeners.
        // Calling it for every column every frame is O(n) in column count; with
        // 1000+ columns this alone drops FPS below 60 even in release mode.
        //
        // We restrict rendering to the columns currently visible inside the
        // overflow-scroll viewport, surrounding them with inert spacer divs:
        //
        //   [left_spacer] [visible columns…] [right_spacer] [last_empty_col]
        //
        // The spacers preserve the flex container's total content width so that
        // the scrollbar range stays correct.
        let total_cols = self.col_groups.len();
        let (visible_col_range, left_spacer) =
            self.calculate_visible_leaf_col_range(left_columns_count);

        let layout_len = self.header_layout.len();
        let header_row_height = self.header_row_height();

        // Reset fixed head columns bounds, if no fixed columns are present
        if left_columns_count == 0 {
            self.fixed_head_cols_bounds = Bounds::default();
        }

        let mut header = self.delegate_mut().render_header(window, cx);
        let style = header.style().clone();
        let layout = self.header_layout.clone();

        header
            // One header row holds every column header, including the pinned
            // ones, which are laid out in a separate container.
            .role(gpui::Role::Row)
            .aria_row_index(0)
            .h_flex()
            .w_full()
            .flex_shrink_0()
            .bg(cx.theme().tokens.table_head)
            .text_color(cx.theme().table_head_foreground)
            .refine_style(&style)
            .on_drag_move(cx.listener(|table, e: &DragMoveEvent<DragColumn>, _, cx| {
                let drag = e.drag(cx);
                let (drag_entity_id, drag_col_ix) = (drag.entity_id, drag.col_ix);

                let gap =
                    if drag_entity_id == cx.entity_id() && e.bounds.contains(&e.event.position) {
                        table.drag_gap_at(e.event.position.x, drag_col_ix)
                    } else {
                        None
                    };

                if table.col_drag_gap != gap {
                    table.col_drag_gap = gap;
                    cx.notify();
                }
            }))
            .on_drop(cx.listener(|table, drag: &DragColumn, window, cx| {
                if drag.entity_id != cx.entity_id() {
                    return;
                }

                // Insert the dragged column into the indicated gap.
                let Some(gap) = table.col_drag_gap.take() else {
                    return;
                };
                let to_ix = if drag.col_ix < gap { gap - 1 } else { gap };
                table.move_column(drag.col_ix, to_ix, window, cx);
            }))
            .when(self.cell_selectable && self.row_header, |this| {
                this.child(self.render_row_header_cell(0, true, cx))
            })
            .when(left_columns_count > 0, |this| {
                let view = view.clone();
                // Render left fixed columns
                this.child(
                    h_flex()
                        .relative()
                        .h_full()
                        .bg(cx.theme().tokens.table_head)
                        .child(v_flex().min_w_full().flex_shrink_0().children(
                            layout.iter().enumerate().map(|(_row_ix, row_cells)| {
                                h_flex()
                                    .min_w_full()
                                    .h(header_row_height)
                                    .border_b_1()
                                    .border_color(cx.theme().border)
                                    .children(row_cells.iter().filter_map(|cell| {
                                        if cell.start_leaf_col_ix < left_columns_count {
                                            if cell.is_leaf {
                                                if let Some(ix) = cell.leaf_col_ix {
                                                    return Some(
                                                        self.render_th(ix, window, cx)
                                                            .into_any_element(),
                                                    );
                                                }
                                            } else {
                                                return Some(
                                                    self.delegate_mut()
                                                        .render_group_th(
                                                            &cell.label,
                                                            cell.col_span,
                                                            cell.width,
                                                            window,
                                                            cx,
                                                        )
                                                        .into_any_element(),
                                                );
                                            }
                                        }
                                        None
                                    }))
                            }),
                        ))
                        .children(self.render_fixed_cols_border(cx))
                        .on_prepaint(move |bounds, _, cx| {
                            view.update(cx, |r, _| r.fixed_head_cols_bounds = bounds)
                        }),
                )
            })
            .child(
                // Columns
                h_flex()
                    .id("table-head")
                    .size_full()
                    .overflow_scroll()
                    .relative()
                    .track_scroll(&horizontal_scroll_handle)
                    .bg(cx.theme().tokens.table_head)
                    .child(v_flex().min_w_full().flex_shrink_0().children(
                        layout.iter().enumerate().map(|(row_ix, row_cells)| {
                            let is_leaf_row = row_ix + 1 == layout_len;
                            h_flex()
                                .min_w_full()
                                .h(header_row_height)
                                .border_b_1()
                                .border_color(cx.theme().border)
                                .map(|this| {
                                    if is_leaf_row {
                                        // Leaf row: apply the spacer virtualization pattern.
                                        // Only columns in `visible_range` are rendered; the two
                                        // spacer divs preserve the container's total content width
                                        // so the scrollbar range stays correct.
                                        this.when(left_spacer > px(0.), |r| {
                                            r.child(div().w(left_spacer).h_full().flex_shrink_0())
                                        })
                                        .children(row_cells.iter().filter_map(|cell| {
                                            if cell.is_leaf {
                                                let ix = cell.leaf_col_ix?;
                                                if !visible_col_range.contains(&ix) {
                                                    return None;
                                                }
                                                Some(
                                                    self.render_th(ix, window, cx)
                                                        .into_any_element(),
                                                )
                                            } else {
                                                None
                                            }
                                        }))
                                        .when(visible_col_range.end < total_cols, |r| {
                                            let right_spacer: Pixels = self.col_groups
                                                [visible_col_range.end..total_cols]
                                                .iter()
                                                .map(|g| g.width)
                                                .sum();
                                            r.child(div().w(right_spacer).h_full().flex_shrink_0())
                                        })
                                        .child(self.delegate.render_last_empty_col(window, cx))
                                    } else {
                                        // Group header rows have far fewer cells (one per group),
                                        // so the cost of rendering all of them is negligible.
                                        this.children(row_cells.iter().filter_map(|cell| {
                                            if cell.start_leaf_col_ix >= left_columns_count {
                                                if cell.is_leaf {
                                                    if let Some(ix) = cell.leaf_col_ix {
                                                        return Some(
                                                            self.render_th(ix, window, cx)
                                                                .into_any_element(),
                                                        );
                                                    }
                                                } else {
                                                    return Some(
                                                        self.delegate_mut()
                                                            .render_group_th(
                                                                &cell.label,
                                                                cell.col_span,
                                                                cell.width,
                                                                window,
                                                                cx,
                                                            )
                                                            .into_any_element(),
                                                    );
                                                }
                                            }
                                            None
                                        }))
                                        .child(self.delegate.render_last_empty_col(window, cx))
                                    }
                                })
                        }),
                    )),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_table_row(
        &mut self,
        row_ix: usize,
        rows_count: usize,
        left_columns_count: usize,
        col_sizes: Rc<Vec<gpui::Size<Pixels>>>,
        columns_count: usize,
        is_filled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let is_stripe_row = self.options.stripe && row_ix % 2 != 0;
        // Only selected rows highlight, not the row of a selected cell.
        let is_selected = self.selection.is_row_selected(row_ix);
        let is_row_menu_target = self.menu_target == Some(MenuTarget::Row(row_ix));
        let view = cx.entity();
        let row_height = self.options.size.table_row_height();

        if row_ix < rows_count {
            let is_last_row = row_ix + 1 == rows_count;
            let need_render_border = is_selected || !is_last_row || !is_filled;

            let mut tr = self.delegate.render_tr(row_ix, window, cx);
            let style = tr.style().clone();
            // As with cell labels, only build the name when it can be read.
            let label = if window.is_a11y_active() || cfg!(test) {
                self.delegate.row_label(row_ix, cx)
            } else {
                None
            };

            tr.test_support()
                .role(gpui::Role::Row)
                // Row 0 is the header row.
                .aria_row_index(row_ix + 1)
                .when_some(label, |this, label| this.aria_label(label))
                .aria_selected(is_selected)
                // While the table has focus, the selected row is the focused
                // node for assistive technology.
                .when(
                    is_selected && self.focus_handle.is_focused(window),
                    |this| this.aria_active_descendant(),
                )
                .h_flex()
                .w_full()
                .h(row_height)
                .when(need_render_border, |this| {
                    this.border_b_1().border_color(cx.theme().table_row_border)
                })
                .when(is_stripe_row, |this| this.bg(cx.theme().tokens.table_even))
                .refine_style(&style)
                .hover(|this| {
                    if is_selected || is_row_menu_target {
                        this
                    } else {
                        this.bg(cx.theme().tokens.table_hover)
                    }
                })
                .when(self.cell_selectable && self.row_header, |this| {
                    this.child(self.render_row_header_cell(row_ix, false, cx))
                })
                .when(left_columns_count > 0, |this| {
                    // Left fixed columns
                    this.child(
                        h_flex()
                            .relative()
                            .h_full()
                            .children({
                                let mut items = Vec::with_capacity(left_columns_count);

                                (0..left_columns_count).for_each(|col_ix| {
                                    let is_cell_selected =
                                        self.selection.cell() == Some((row_ix, col_ix));
                                    let is_cell_right_clicked =
                                        self.menu_target == Some(MenuTarget::Cell(row_ix, col_ix));

                                    items.push(
                                        self.render_col_wrap(Some(row_ix), col_ix, window, cx)
                                            .child(
                                                self.a11y_cell(
                                                    self.render_cell(
                                                        Some(row_ix),
                                                        col_ix,
                                                        window,
                                                        cx,
                                                    )
                                                    .id(format!("table-cell:{}:{}", row_ix, col_ix))
                                                    .test_support(),
                                                    row_ix,
                                                    col_ix,
                                                    window,
                                                    cx,
                                                )
                                                .relative()
                                                .child(
                                                    self.measure_render_td(
                                                        row_ix, col_ix, window, cx,
                                                    ),
                                                )
                                                .when(is_cell_selected, |this| {
                                                    this.child(
                                                        div()
                                                            .absolute()
                                                            .inset_0()
                                                            .bg(cx.theme().tokens.table_active),
                                                    )
                                                })
                                                .when(
                                                    is_cell_right_clicked && !is_cell_selected,
                                                    |this| {
                                                        this.child(
                                                            div()
                                                                .absolute()
                                                                .inset_0()
                                                                .border_1()
                                                                .border_color(
                                                                    cx.theme()
                                                                        .table_active_border
                                                                        .opacity(0.5),
                                                                ),
                                                        )
                                                    },
                                                )
                                                .when(self.cell_selectable, |this| {
                                                    this.on_click(cx.listener(
                                                        move |table, e, window, cx| {
                                                            table.on_cell_click(
                                                                e, row_ix, col_ix, window, cx,
                                                            );
                                                        },
                                                    ))
                                                    .on_mouse_down(
                                                        MouseButton::Right,
                                                        cx.listener(move |table, _, window, cx| {
                                                            table.on_cell_right_click(
                                                                row_ix, col_ix, window, cx,
                                                            );
                                                        }),
                                                    )
                                                }),
                                            ),
                                    );
                                });

                                items
                            })
                            .children(self.render_fixed_cols_border(cx)),
                    )
                })
                .child(
                    h_flex()
                        .flex_1()
                        .h_full()
                        .overflow_hidden()
                        .relative()
                        .child(
                            gpui_base::virtual_list(view, row_ix, Axis::Horizontal, col_sizes, {
                                move |table, visible_range: Range<usize>, window, cx| {
                                    table.update_visible_range_if_need(
                                        visible_range.clone(),
                                        columns_count.saturating_sub(left_columns_count),
                                        Axis::Horizontal,
                                        window,
                                        cx,
                                    );

                                    let mut items =
                                        Vec::with_capacity(visible_range.end - visible_range.start);

                                    visible_range.for_each(|col_ix| {
                                        let col_ix = col_ix + left_columns_count;
                                        let is_cell_selected =
                                            table.selection.cell() == Some((row_ix, col_ix));
                                        let is_cell_right_clicked = table.menu_target
                                            == Some(MenuTarget::Cell(row_ix, col_ix));

                                        let el = table
                                            .render_col_wrap(Some(row_ix), col_ix, window, cx)
                                            .child(
                                                table
                                                    .a11y_cell(
                                                        table
                                                            .render_cell(
                                                                Some(row_ix),
                                                                col_ix,
                                                                window,
                                                                cx,
                                                            )
                                                            .id(format!(
                                                                "table-cell-{}:{}",
                                                                row_ix, col_ix
                                                            ))
                                                            .test_support(),
                                                        row_ix,
                                                        col_ix,
                                                        window,
                                                        cx,
                                                    )
                                                    .relative()
                                                    .child(table.measure_render_td(
                                                        row_ix, col_ix, window, cx,
                                                    ))
                                                    .when(is_cell_selected, |this| {
                                                        this.child(
                                                            div()
                                                                .absolute()
                                                                .inset_0()
                                                                .bg(cx.theme().tokens.table_active),
                                                        )
                                                    })
                                                    .when(
                                                        is_cell_right_clicked && !is_cell_selected,
                                                        |this| {
                                                            this.child(
                                                                div()
                                                                    .absolute()
                                                                    .inset_0()
                                                                    .border_1()
                                                                    .border_color(
                                                                        cx.theme()
                                                                            .table_active_border
                                                                            .opacity(0.5),
                                                                    ),
                                                            )
                                                        },
                                                    )
                                                    .when(table.cell_selectable, |this| {
                                                        this.on_click(cx.listener(
                                                            move |table, e, window, cx| {
                                                                cx.stop_propagation();
                                                                table.on_cell_click(
                                                                    e, row_ix, col_ix, window, cx,
                                                                );
                                                            },
                                                        ))
                                                        .on_mouse_down(
                                                            MouseButton::Right,
                                                            cx.listener(
                                                                move |table, _, window, cx| {
                                                                    table.on_cell_right_click(
                                                                        row_ix, col_ix, window, cx,
                                                                    );
                                                                },
                                                            ),
                                                        )
                                                    }),
                                            );

                                        items.push(el);
                                    });

                                    items
                                }
                            })
                            .with_scroll_handle(&self.horizontal_scroll_handle),
                        )
                        .child(self.delegate.render_last_empty_col(window, cx)),
                )
                // Row selected style
                .when(is_selected, |this| {
                    this.map(|this| {
                        if cx.theme().list.active_highlight {
                            this.border_color(gpui::transparent_white()).child(
                                div()
                                    .top(if row_ix == 0 { px(0.) } else { px(-1.) })
                                    .left(px(0.))
                                    .right(px(0.))
                                    .bottom(px(-1.))
                                    .absolute()
                                    .bg(cx.theme().tokens.table_active)
                                    .border_1()
                                    .border_color(cx.theme().table_active_border),
                            )
                        } else {
                            this.bg(cx.theme().tokens.accent)
                        }
                    })
                })
                // Row right click row style
                .when(is_row_menu_target, |this| {
                    this.border_color(gpui::transparent_white()).child(
                        div()
                            .top(if row_ix == 0 { px(0.) } else { px(-1.) })
                            .left(px(0.))
                            .right(px(0.))
                            .bottom(px(-1.))
                            .absolute()
                            .border_1()
                            .border_color(cx.theme().selection),
                    )
                })
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _, window, cx| {
                        this.on_row_right_click(row_ix, window, cx);
                    }),
                )
                .on_click(cx.listener(move |this, e, window, cx| {
                    this.on_row_left_click(e, row_ix, window, cx);
                }))
                .into_any_element()
        } else {
            let cols_width: Pixels = self
                .col_groups
                .iter()
                .take(columns_count)
                .map(|col_group| col_group.width)
                .sum();

            // Render fake rows to fill the rest table space
            self.delegate
                .render_tr(row_ix, window, cx)
                .h_flex()
                .w_full()
                .h(row_height)
                .border_b_1()
                .border_color(cx.theme().table_row_border)
                .when(is_stripe_row, |this| this.bg(cx.theme().tokens.table_even))
                .when(self.cell_selectable && self.row_header, |this| {
                    // Render empty row header cell for fake rows
                    let width = self.row_header_width(cx);
                    this.child(
                        div()
                            .w(width)
                            .h_full()
                            .flex_shrink_0()
                            .border_r_1()
                            .border_color(cx.theme().table_row_border)
                            .bg(cx.theme().table_head)
                            .table_cell_size(self.options.size),
                    )
                })
                // The fake cells paint nothing, so a single spacer as wide as all
                // the columns keeps the last empty column in place without laying
                // out one element per column.
                .child(div().flex_shrink_0().h_full().w(cols_width))
                .child(self.delegate.render_last_empty_col(window, cx))
                .into_any_element()
        }
    }

    /// Calculate the extra rows needed to fill the table empty space when `stripe` is true.
    fn calculate_extra_rows_needed(
        &self,
        total_height: Pixels,
        actual_height: Pixels,
        row_height: Pixels,
    ) -> usize {
        let mut extra_rows_needed = 0;

        let remaining_height = total_height - actual_height;
        if remaining_height > px(0.) {
            extra_rows_needed = (remaining_height / row_height).floor() as usize;
        }

        extra_rows_needed
    }

    #[inline]
    fn measure_render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if !super::measure_enable() {
            return self
                .delegate
                .render_td(row_ix, col_ix, window, cx)
                .into_any_element();
        }

        let start = std::time::Instant::now();
        let el = self.delegate.render_td(row_ix, col_ix, window, cx);
        self._measure.push(start.elapsed());
        el.into_any_element()
    }

    fn measure(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        if !super::measure_enable() {
            return;
        }

        // Print avg measure time of each td
        if self._measure.len() > 0 {
            let total = self
                ._measure
                .iter()
                .fold(Duration::default(), |acc, d| acc + *d);
            let avg = total / self._measure.len() as u32;
            tracing::debug!(
                "last render {} cells total: {:?}, avg: {:?}",
                self._measure.len(),
                total,
                avg,
            );
        }
        self._measure.clear();
    }

    fn render_vertical_scrollbar(
        &mut self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        Some(
            div()
                .occlude()
                .absolute()
                .top(self.header_band_height())
                .right_0()
                .bottom_0()
                .w(Scrollbar::width())
                .child(
                    Scrollbar::vertical(&self.vertical_scroll_handle)
                        .viewport_from_layout()
                        .max_fps(60),
                ),
        )
    }

    fn render_horizontal_scrollbar(
        &mut self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .occlude()
            .absolute()
            .left(self.fixed_head_cols_bounds.size.width)
            .right_0()
            .bottom_0()
            .h(Scrollbar::width())
            .child(Scrollbar::horizontal(&self.horizontal_scroll_handle).viewport_from_layout())
    }
}

impl<D> Focusable for TableState<D>
where
    D: TableDelegate,
{
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
impl<D> EventEmitter<TableEvent> for TableState<D> where D: TableDelegate {}

impl<D> Render for TableState<D>
where
    D: TableDelegate,
{
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.measure(window, cx);

        let columns_count = self.delegate.columns_count(cx);
        let left_columns_count = self
            .col_groups
            .iter()
            .filter(|col| self.col_fixed && col.column.fixed == Some(ColumnFixed::Left))
            .count();
        let rows_count = self.delegate.rows_count(cx);
        let loading = self.delegate.loading(cx);

        let row_height = self.options.size.table_row_height();
        let total_height = self
            .vertical_scroll_handle
            .0
            .borrow()
            .base_handle
            .bounds()
            .size
            .height;
        let actual_height = row_height * rows_count as f32;
        let extra_rows_count =
            self.calculate_extra_rows_needed(total_height, actual_height, row_height);
        let render_rows_count = if self.options.stripe {
            rows_count + extra_rows_count
        } else {
            rows_count
        };
        // A row or cell keeps its menu outline until the next press elsewhere.
        let has_menu_outline = matches!(
            self.menu_target,
            Some(MenuTarget::Row(_) | MenuTarget::Cell(..))
        );
        let is_filled = total_height > Pixels::ZERO && total_height <= actual_height;

        let loading_view = if loading {
            Some(
                self.delegate
                    .render_loading(self.options.size, window, cx)
                    .into_any_element(),
            )
        } else {
            None
        };

        let empty_view = if rows_count == 0 {
            // The rows list is not rendered, so report the empty range here.
            self.update_visible_range_if_need(0..0, 0, Axis::Vertical, window, cx);
            Some(
                div()
                    .size_full()
                    .child(self.delegate.render_empty(window, cx))
                    .into_any_element(),
            )
        } else {
            None
        };

        let inner_table = v_flex()
            .id("table-inner")
            .test_support()
            .size_full()
            .overflow_hidden()
            .child(self.render_table_header(left_columns_count, window, cx))
            .context_menu({
                let view = cx.entity();
                move |this, window: &mut Window, cx: &mut Context<PopupMenu>| {
                    let target = view.read(cx).menu_target;
                    match target {
                        Some(MenuTarget::Column(col_ix)) => view.update(cx, |table, cx| {
                            table
                                .delegate_mut()
                                .column_context_menu(col_ix, this, window, cx)
                        }),
                        Some(MenuTarget::Cell(row_ix, col_ix)) => view.update(cx, |table, cx| {
                            table
                                .delegate_mut()
                                .context_menu((row_ix, col_ix), this, window, cx)
                        }),
                        Some(MenuTarget::Row(_)) | None => this,
                    }
                }
            })
            .map(|this| {
                if rows_count == 0 {
                    this.children(empty_view)
                } else {
                    this.child(
                        h_flex().id("table-body").flex_grow_1().size_full().child(
                            uniform_list(
                                "table-uniform-list",
                                render_rows_count,
                                cx.processor(
                                    move |table, visible_range: Range<usize>, window, cx| {
                                        // Use `col.width` (always up-to-date) rather than
                                        // `col.bounds.size.width`, which is only set after
                                        // prepaint and is therefore zero on the first frame.
                                        let col_sizes: Rc<Vec<gpui::Size<Pixels>>> = Rc::new(
                                            table
                                                .col_groups
                                                .iter()
                                                .skip(left_columns_count)
                                                .map(|col| gpui::Size {
                                                    width: col.width,
                                                    height: px(0.),
                                                })
                                                .collect(),
                                        );

                                        table.load_more_if_need(
                                            rows_count,
                                            visible_range.end,
                                            window,
                                            cx,
                                        );
                                        table.update_visible_range_if_need(
                                            visible_range.clone(),
                                            rows_count,
                                            Axis::Vertical,
                                            window,
                                            cx,
                                        );

                                        if visible_range.end > rows_count {
                                            table.scroll_to_row(
                                                std::cmp::min(
                                                    visible_range.start,
                                                    rows_count.saturating_sub(1),
                                                ),
                                                cx,
                                            );
                                        }

                                        let mut items = Vec::with_capacity(
                                            visible_range.end.saturating_sub(visible_range.start),
                                        );

                                        // Render fake rows to fill the table
                                        visible_range.for_each(|row_ix| {
                                            // Render real rows for available data
                                            items.push(table.render_table_row(
                                                row_ix,
                                                rows_count,
                                                left_columns_count,
                                                col_sizes.clone(),
                                                columns_count,
                                                is_filled,
                                                window,
                                                cx,
                                            ));
                                        });

                                        items
                                    },
                                ),
                            )
                            .flex_grow_1()
                            .size_full()
                            .with_sizing_behavior(ListSizingBehavior::Auto)
                            .track_scroll(&self.vertical_scroll_handle)
                            .map(|rows| {
                                gpui_base::ScrollBounce::new(
                                    "table-scroll-bounce",
                                    &self.vertical_scroll_handle,
                                    rows,
                                )
                                .when_some(self.options.scroll_bounce, |bounce, on| {
                                    bounce.enabled(on)
                                })
                            })
                            .into_any_element(),
                        ),
                    )
                }
            });

        div()
            .size_full()
            .children(loading_view)
            .when(!loading, |this| {
                this.child(inner_table)
                    .child(ScrollableMask::new(
                        Axis::Horizontal,
                        &self.horizontal_scroll_handle,
                    ))
                    // Keep vertical wheel scrolling from leaking into an
                    // ancestor scroller. Skipped when the table is empty:
                    // the `uniform_list` is not rendered then, so the
                    // handle's offset and `max_offset` are stale.
                    .when(rows_count > 0, |this| {
                        this.child(ScrollableMask::new(
                            Axis::Vertical,
                            &self.vertical_scroll_handle.0.borrow().base_handle,
                        ))
                    })
                    .when(has_menu_outline, |this| {
                        this.on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.menu_target = None;
                            cx.notify();
                        }))
                    })
            })
            .on_prepaint({
                let state = cx.entity();
                move |bounds, _, cx| state.update(cx, |state, _| state.bounds = bounds)
            })
            .when(!window.is_inspector_picking(cx), |this| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .size_full()
                        .when(self.options.scrollbar_visible.bottom, |this| {
                            this.child(self.render_horizontal_scrollbar(window, cx))
                        })
                        .when(
                            self.options.scrollbar_visible.right && rows_count > 0,
                            |this| this.children(self.render_vertical_scrollbar(window, cx)),
                        ),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use gpui::{App, Entity, Modifiers, TestAppContext, WindowHandle};

    use super::*;

    struct WidthsDelegate;

    struct BounceDelegate;

    impl TableDelegate for BounceDelegate {
        fn columns_count(&self, _: &App) -> usize {
            1
        }
        fn rows_count(&self, _: &App) -> usize {
            40
        }
        fn column(&self, _: usize, _: &App) -> Column {
            Column::new("value", "Value")
        }
        fn render_td(
            &mut self,
            row: usize,
            _: usize,
            _: &mut Window,
            _: &mut Context<TableState<Self>>,
        ) -> impl IntoElement {
            div().child(row.to_string())
        }
    }

    struct BounceTable(Entity<TableState<BounceDelegate>>);

    impl Render for BounceTable {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(240.))
                .h(px(200.))
                .child(DataTable::new(&self.0).scroll_bounce(true))
        }
    }

    #[gpui::test]
    fn scroll_bounce_table_keeps_headers_fixed_and_accepts_new_wheel_input(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::table::test_init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            BounceTable(cx.new(|cx| TableState::new(BounceDelegate, window, cx)))
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let state = view.read_with(cx, |view, _| view.0.clone());
        let header = state.read_with(cx, |state, _| state.col_groups[0].bounds);
        let wheel = |cx: &mut gpui::VisualTestContext, dy| {
            cx.simulate_event(gpui::ScrollEvent {
                position: gpui::point(px(100.), px(100.)),
                delta: gpui::ScrollDelta::Lines(gpui::point(0., dy)),
                ..Default::default()
            });
        };
        for (dy, phase) in [
            (-10000., gpui::TouchPhase::Started),
            (0., gpui::TouchPhase::Ended),
        ] {
            cx.simulate_event(gpui::ScrollEvent {
                position: gpui::point(px(100.), px(100.)),
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(dy))),
                touch_phase: phase,
                ..Default::default()
            });
        }
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let end = state.read_with(cx, |state, _| {
            state
                .vertical_scroll_handle
                .0
                .borrow()
                .base_handle
                .offset()
                .y
        });
        assert!(end < px(0.));
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(
            state.read_with(cx, |state, _| state.col_groups[0].bounds),
            header
        );
        wheel(cx, 2.);
        assert!(
            state.read_with(cx, |state, _| state
                .vertical_scroll_handle
                .0
                .borrow()
                .base_handle
                .offset()
                .y)
                > end
        );
    }

    impl TableDelegate for WidthsDelegate {
        fn columns_count(&self, _: &App) -> usize {
            6
        }

        fn rows_count(&self, _: &App) -> usize {
            0
        }

        fn column(&self, col_ix: usize, _: &App) -> Column {
            match col_ix {
                0 => Column::new("narrow", "Narrow")
                    .min_width(px(80.))
                    .width(px(40.)),
                1 => Column::new("wide", "Wide")
                    .max_width(px(120.))
                    .width(px(300.)),
                // `width` alone below the default `min_width` of 20px.
                2 => Column::new("tiny", "Tiny").width(px(10.)),
                // The default resize protection does not limit initial width.
                3 => Column::new("wide_default", "Wide Default").width(px(2000.)),
                // Applications can explicitly allow wider resize ranges.
                4 => Column::new("wide_explicit", "Wide Explicit")
                    .max_width(px(2400.))
                    .width(px(2000.)),
                // `min_width` above `max_width` must not panic.
                _ => Column::new("inverted", "Inverted")
                    .min_width(px(200.))
                    .max_width(px(100.)),
            }
        }

        fn render_td(
            &mut self,
            _: usize,
            _: usize,
            _: &mut Window,
            _: &mut Context<TableState<Self>>,
        ) -> impl IntoElement {
            div()
        }
    }

    #[gpui::test]
    fn column_resize_bounds_preserve_initial_widths(cx: &mut TestAppContext) {
        cx.skip_drawing();
        let window = cx.add_window(|window, cx| TableState::new(WidthsDelegate, window, cx));

        window
            .update(cx, |table, window, cx| {
                let widths = |table: &TableState<WidthsDelegate>| {
                    table
                        .col_groups
                        .iter()
                        .map(|col_group| col_group.width)
                        .collect::<Vec<_>>()
                };
                assert_eq!(
                    widths(table),
                    [px(40.), px(300.), px(10.), px(2000.), px(2000.), px(100.)]
                );

                for ix in 0..table.col_groups.len() {
                    table.resize_cols(ix, px(10000.), window, cx);
                }
                assert_eq!(
                    widths(table),
                    [
                        px(1200.),
                        px(120.),
                        px(1200.),
                        px(1200.),
                        px(2400.),
                        px(100.)
                    ]
                );

                for ix in 0..table.col_groups.len() {
                    table.resize_cols(ix, px(-100.), window, cx);
                }
                assert_eq!(
                    widths(table),
                    [px(80.), px(20.), px(20.), px(20.), px(20.), px(100.)]
                );
            })
            .unwrap();
    }

    type Calls = Rc<RefCell<Vec<Vec<(usize, ColumnSort)>>>>;

    struct SortDelegate {
        multi: bool,
        calls: Calls,
    }

    impl TableDelegate for SortDelegate {
        fn columns_count(&self, _: &App) -> usize {
            3
        }

        fn rows_count(&self, _: &App) -> usize {
            0
        }

        fn column(&self, col_ix: usize, _: &App) -> Column {
            let name = format!("c{col_ix}");
            Column::new(name.clone(), name).sortable()
        }

        fn render_td(
            &mut self,
            _: usize,
            _: usize,
            _: &mut Window,
            _: &mut Context<TableState<Self>>,
        ) -> impl IntoElement {
            div()
        }

        fn perform_sort(
            &mut self,
            col_ix: usize,
            sort: ColumnSort,
            _: &mut Window,
            _: &mut Context<TableState<Self>>,
        ) {
            self.calls.borrow_mut().push(vec![(col_ix, sort)]);
        }

        fn multi_sortable(&self) -> bool {
            self.multi
        }

        fn perform_multi_sort(
            &mut self,
            keys: &[(usize, ColumnSort)],
            _: &mut Window,
            _: &mut Context<TableState<Self>>,
        ) {
            self.calls.borrow_mut().push(keys.to_vec());
        }
    }

    fn table(
        multi: bool,
        cx: &mut TestAppContext,
    ) -> (WindowHandle<TableState<SortDelegate>>, Calls) {
        cx.skip_drawing();
        let calls = Calls::default();
        let delegate = SortDelegate {
            multi,
            calls: calls.clone(),
        };
        let window = cx.add_window(move |window, cx| TableState::new(delegate, window, cx));
        (window, calls)
    }

    #[gpui::test]
    fn shift_click_adds_sort_keys(cx: &mut TestAppContext) {
        use ColumnSort::*;
        let (window, calls) = table(true, cx);
        window
            .update(cx, |table, window, cx| {
                table.perform_sort(1, false, window, cx);
                table.perform_sort(0, true, window, cx);
                table.perform_sort(2, true, window, cx);
                assert_eq!(
                    table.sort_keys(),
                    [(1, Descending), (0, Descending), (2, Descending)]
                );

                // Stepping a key keeps its place, and its last step drops it.
                table.perform_sort(0, true, window, cx);
                assert_eq!(
                    table.sort_keys(),
                    [(1, Descending), (0, Ascending), (2, Descending)]
                );
                table.perform_sort(0, true, window, cx);
                assert_eq!(table.sort_keys(), [(1, Descending), (2, Descending)]);

                // Moving a column carries its key along.
                table.move_column(2, 0, window, cx);
                assert_eq!(table.sort_keys(), [(2, Descending), (0, Descending)]);

                // A plain click goes back to one key.
                table.perform_sort(1, false, window, cx);
                assert_eq!(table.sort_keys(), [(1, Descending)]);
            })
            .unwrap();
        assert_eq!(
            calls.borrow().last(),
            Some(&vec![(1, Descending)]),
            "a plain click sorts through perform_sort"
        );
    }

    #[gpui::test]
    fn shift_click_is_a_plain_click_without_multi_sort(cx: &mut TestAppContext) {
        let (window, calls) = table(false, cx);
        window
            .update(cx, |table, window, cx| {
                table.perform_sort(1, false, window, cx);
                table.perform_sort(0, true, window, cx);
                assert_eq!(table.sort_keys(), [(0, ColumnSort::Descending)]);
            })
            .unwrap();
        assert_eq!(calls.borrow().len(), 2);
    }

    #[gpui::test]
    fn right_click_on_a_header_selects_its_column(cx: &mut TestAppContext) {
        let (window, _) = table(false, cx);
        window
            .update(cx, |table, window, cx| {
                table.cell_selectable = true;
                table.on_col_head_right_click(1, window, cx);
                assert_eq!(table.menu_target, Some(MenuTarget::Column(1)));
                assert_eq!(table.selection(), &TableSelection::Column(1));

                // The menu follows the last press, so a cell takes it back.
                table.on_cell_right_click(0, 2, window, cx);
                assert_eq!(table.menu_target, Some(MenuTarget::Cell(0, 2)));

                table.col_selectable = false;
                table.clear_selection(cx);
                table.on_col_head_right_click(2, window, cx);
                assert_eq!(table.menu_target, Some(MenuTarget::Column(2)));
                assert!(table.selection().is_none());
            })
            .unwrap();
    }

    struct RowsDelegate;

    impl TableDelegate for RowsDelegate {
        fn columns_count(&self, _: &App) -> usize {
            2
        }

        fn rows_count(&self, _: &App) -> usize {
            10
        }

        fn column(&self, col_ix: usize, _: &App) -> Column {
            let name = format!("c{col_ix}");
            Column::new(name.clone(), name)
        }

        fn render_td(
            &mut self,
            _: usize,
            _: usize,
            _: &mut Window,
            _: &mut Context<TableState<Self>>,
        ) -> impl IntoElement {
            div()
        }
    }

    fn click(modifiers: Modifiers) -> ClickEvent {
        ClickEvent::Mouse(gpui::MouseClickEvent {
            down: MouseDownEvent {
                modifiers,
                click_count: 1,
                ..Default::default()
            },
            up: gpui::MouseUpEvent {
                modifiers,
                click_count: 1,
                ..Default::default()
            },
        })
    }

    fn rows_table(cx: &mut TestAppContext) -> WindowHandle<TableState<RowsDelegate>> {
        cx.skip_drawing();
        cx.add_window(|window, cx| TableState::new(RowsDelegate, window, cx).cell_selectable(true))
    }

    fn rows(table: &TableState<RowsDelegate>) -> Vec<usize> {
        table
            .selection()
            .selected_rows()
            .map(|rows| rows.iter().collect())
            .unwrap_or_default()
    }

    #[gpui::test]
    fn clicks_select_rows_from_the_anchor(cx: &mut TestAppContext) {
        let window = rows_table(cx);
        window
            .update(cx, |table, window, cx| {
                let plain = Modifiers::none();
                let shift = Modifiers::shift();
                let secondary = Modifiers::secondary_key(cx);
                let both = Modifiers {
                    shift: true,
                    ..secondary
                };

                table.on_row_left_click(&click(plain), 2, window, cx);
                assert_eq!(rows(table), [2]);
                table.on_row_left_click(&click(shift), 5, window, cx);
                assert_eq!(rows(table), [2, 3, 4, 5]);
                // A second Shift click moves the range's far end, it does
                // not add to the first range.
                table.on_row_left_click(&click(shift), 3, window, cx);
                assert_eq!(rows(table), [2, 3]);

                table.on_row_left_click(&click(secondary), 7, window, cx);
                assert_eq!(rows(table), [2, 3, 7]);
                table.on_row_left_click(&click(secondary), 2, window, cx);
                assert_eq!(rows(table), [3, 7]);
                // A secondary click moves the anchor even when it takes the
                // row out, so both modifiers add the range from row 2.
                table.on_row_left_click(&click(both), 9, window, cx);
                assert_eq!(rows(table), [2, 3, 4, 5, 6, 7, 8, 9]);

                // Taking the last row out leaves nothing selected.
                table.on_row_left_click(&click(plain), 4, window, cx);
                table.on_row_left_click(&click(secondary), 4, window, cx);
                assert!(table.selection().is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_cell_replaces_rows_and_anchors_the_next_range(cx: &mut TestAppContext) {
        let window = rows_table(cx);
        window
            .update(cx, |table, window, cx| {
                table.select_all_rows(cx);
                assert_eq!(rows(table).len(), 10);

                // The cell is the whole selection, nothing of the rows stays.
                table.on_cell_click(&click(Modifiers::none()), 2, 0, window, cx);
                assert_eq!(table.selection(), &TableSelection::Cell(2, 0));
                assert_eq!(table.selection().rows(), [2]);

                table.on_cell_click(&click(Modifiers::shift()), 4, 1, window, cx);
                assert_eq!(rows(table), [2, 3, 4]);

                table.select_all_rows(cx);
                table.on_cell_click(&click(Modifiers::none()), 5, 0, window, cx);
                table.on_cell_click(&click(Modifiers::secondary_key(cx)), 7, 0, window, cx);
                assert_eq!(rows(table), [7]);
            })
            .unwrap();
    }

    #[gpui::test]
    fn keys_move_the_cell_or_one_row(cx: &mut TestAppContext) {
        let window = rows_table(cx);
        window
            .update(cx, |table, window, cx| {
                table.loop_selection = false;
                table.on_cell_click(&click(Modifiers::none()), 3, 0, window, cx);
                table.action_select_next(&SelectDown, window, cx);
                table.action_select_next_col(&SelectNextColumn, window, cx);
                assert_eq!(table.selection(), &TableSelection::Cell(4, 1));
                // Without looping the cell stops at the last column.
                table.action_select_next_col(&SelectNextColumn, window, cx);
                assert_eq!(table.selection(), &TableSelection::Cell(4, 1));
                table.action_select_first_column(&SelectFirst, window, cx);
                assert_eq!(table.selection(), &TableSelection::Cell(4, 0));

                // From several rows the arrows move one row from the last
                // clicked one.
                table.on_row_left_click(&click(Modifiers::none()), 6, window, cx);
                table.on_row_left_click(&click(Modifiers::shift()), 8, window, cx);
                table.action_select_prev(&SelectUp, window, cx);
                assert_eq!(rows(table), [7]);

                // From a row, the horizontal keys step into its cells.
                table.action_select_next_col(&SelectNextColumn, window, cx);
                assert_eq!(table.selection(), &TableSelection::Cell(7, 0));

                table.action_cancel(&Cancel, window, cx);
                assert!(table.selection().is_none());
                table.action_select_last_column(&SelectLast, window, cx);
                assert_eq!(rows(table), [9]);
            })
            .unwrap();
    }

    #[gpui::test]
    fn without_the_row_header_a_second_click_selects_the_row(cx: &mut TestAppContext) {
        let window = rows_table(cx);
        window
            .update(cx, |table, window, cx| {
                table.row_header = false;
                table.on_cell_click(&click(Modifiers::none()), 1, 1, window, cx);
                assert_eq!(table.selection(), &TableSelection::Cell(1, 1));
                table.on_cell_click(&click(Modifiers::none()), 1, 1, window, cx);
                assert_eq!(rows(table), [1]);
            })
            .unwrap();
    }

    struct MenuDelegate(Rc<RefCell<Vec<(usize, usize)>>>);

    impl TableDelegate for MenuDelegate {
        fn columns_count(&self, _: &App) -> usize {
            1
        }

        fn rows_count(&self, _: &App) -> usize {
            3
        }

        fn column(&self, _: usize, _: &App) -> Column {
            Column::new("value", "Value")
        }

        fn render_td(
            &mut self,
            row: usize,
            _: usize,
            _: &mut Window,
            _: &mut Context<TableState<Self>>,
        ) -> impl IntoElement {
            div().child(row.to_string())
        }

        fn context_menu(
            &mut self,
            cell: (usize, usize),
            menu: PopupMenu,
            _: &mut Window,
            _: &mut Context<TableState<Self>>,
        ) -> PopupMenu {
            self.0.borrow_mut().push(cell);
            menu
        }
    }

    struct MenuTable(Entity<TableState<MenuDelegate>>);

    impl Render for MenuTable {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(240.)).h(px(200.)).child(DataTable::new(&self.0))
        }
    }

    #[gpui::test]
    fn right_click_on_a_cell_opens_its_context_menu(cx: &mut TestAppContext) {
        cx.update(crate::table::test_init);
        let built = Rc::new(RefCell::new(Vec::new()));
        let (view, cx) = cx.add_window_view({
            let built = built.clone();
            move |window, cx| {
                MenuTable(cx.new(|cx| {
                    TableState::new(MenuDelegate(built), window, cx).cell_selectable(true)
                }))
            }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let state = view.read_with(cx, |view, _| view.0.clone());
        let header = state.read_with(cx, |state, _| state.col_groups[0].bounds);
        let row_height = header.size.height;
        let press = gpui::point(
            header.center().x,
            header.bottom() + row_height + row_height / 2.,
        );
        let right_click = |cx: &mut gpui::VisualTestContext| {
            cx.simulate_mouse_down(press, MouseButton::Right, Modifiers::none());
            cx.simulate_mouse_up(press, MouseButton::Right, Modifiers::none());
            cx.run_until_parked();
        };
        right_click(cx);

        assert_eq!(built.borrow().last(), Some(&(1, 0)));
        // The menu acts on the cell it opened over.
        assert_eq!(
            state.read_with(cx, |state, _| state.selection().clone()),
            TableSelection::Cell(1, 0)
        );

        // Inside a row selection the press keeps every selected row.
        state.update(cx, |state, cx| {
            state.set_selection(TableSelection::from_rows([0, 1].into_iter().collect()), cx);
        });
        right_click(cx);
        assert_eq!(
            state.read_with(cx, |state, _| state.selection().rows()),
            [0, 1]
        );
        assert_eq!(
            state.read_with(cx, |state, _| state.menu_target),
            Some(MenuTarget::Cell(1, 0))
        );
    }
}
