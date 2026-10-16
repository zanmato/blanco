# Results

Results show in a grid below the editor. When a run returns several results,
each gets its own tab. Pin a result tab to keep it when you run again.

## The grid

- Click a column header to sort. <kbd>Shift</kbd> click another header to
  sort by it as well.
- The **Columns** button hides, reorders and pins columns. The layout is
  remembered per table.
- Type in **Filter rows** to show only rows that contain some text. This
  filters what is already loaded. To filter on the server, see
  [Filtering](filtering.md).
- Settings can show row numbers, and choose how dates and time zones are
  shown.

## Copying

<kbd>Ctrl+C</kbd> copies the selection: a cell's value, selected rows as
tab separated text with a header row, or a selected column's values one per
line.

Right click a cell for more:

- **Copy as** CSV, TSV, JSON, SQL `INSERT`, `VALUES`, Markdown or HTML.
  HTML pastes as a table in documents and email.
- **Copy Column** copies a column's values, its distinct values, or an `IN (...)`
  list.

CSV delimiter, quoting and the header row are set under **Copy as CSV** on the
Database page in Settings.

## Foreign keys

A cell that references another table shows the referenced row in a popover.
**Open Referenced Row**, or <kbd>Alt+Enter</kbd>, opens it in a new tab.

## Large and binary values

Binary columns show as `0x` hex. Expanding a binary cell shows it as hex, or as
an image when it is one, and lets you save it to a file or load a file into
it.

JSON cells open in an editor with **Format** and **Minify**.

With **Defer Values Larger Than (KB)** set in Settings, a `SELECT *` on a table loads a
preview of large text and binary values and fetches the rest when you open,
copy or export them.

**Show Invisible Characters** in Settings marks control characters, zero width
characters and trailing spaces in cells.

## Charts

The **Chart view** button draws the result as a line, bar or pie chart. Choose
the column for the x axis, the value column, and optionally a column that
splits the data into series.

<figure class="screenshot">
<img class="screenshot-light" src="images/chart-latte.webp" alt="Revenue per genre drawn as a bar chart">
<img class="screenshot-dark" src="images/chart-mocha.webp" alt="">
</figure>

## Query plan

After an [explain](running-queries.md#explain), the **Query plan** view shows
the plan as a tree.

<figure class="screenshot">
<img class="screenshot-light" src="images/explain-latte.webp" alt="The query plan of a join as a tree, with bars showing where the time went">
<img class="screenshot-dark" src="images/explain-mocha.webp" alt="">
</figure>
