# Windows, tabs and the sidebar

## Sidebar

The sidebar has three panels: **Connections**, **Snippets** and
**History**. Toggle it with <kbd>Ctrl+B</kbd>.

The connections tree lists each connection's databases, schemas, tables,
views, materialized views, functions, procedures and triggers, loaded as you
expand them. Tables show their size, except on CockroachDB, which does not
report it. Partitions of a PostgreSQL table are grouped under it.

Type in the filter field to narrow the tree. `^` matches the start of a
name, `$` the end, and `|` separates alternatives, so `^order|item$` finds
names that start with `order` or end with `item`.

## Tabs

Opening a table, a new query or a snippet adds a tab. Tabs are grouped by
connection in the tab strip.

- <kbd>Ctrl+T</kbd> opens a query tab, <kbd>Ctrl+W</kbd> closes one, and
  <kbd>Ctrl+Shift+T</kbd> brings back the last one closed.
- <kbd>Ctrl+Tab</kbd> and <kbd>Ctrl+Shift+Tab</kbd> move between tabs. This
  is <kbd>Ctrl</kbd> on macOS too.
- Drag a tab to reorder it. Right click it to rename, duplicate, pin or move
  it to a new window, or to close the tabs to its left, its right, all but it
  or all of them. Pinned tabs stay open. Closing a tab with unsaved changes to
  its file asks to save them first.
- Middle click closes a tab.

What you type is saved as you go. Tabs and their text come back when you
start Blanco again.

## Windows

**File, New Window** (<kbd>Ctrl+Shift+N</kbd>) opens another window with its
own tabs and sidebar. Every window, with its size, position and tabs, is
restored on the next start.

## Command palette

<kbd>Ctrl+K</kbd> opens the command palette. It lists commands, open tabs,
themes and connections. Pick a connection to see its databases, and pick a
database to back it up, restore it or open a query on it. Type to filter,
<kbd>Backspace</kbd> on an empty field goes back a level.

<figure class="screenshot">
<img class="screenshot-light" src="images/palette-latte.webp" alt="The command palette open over the editor">
<img class="screenshot-dark" src="images/palette-mocha.webp" alt="">
</figure>

## SQL log

Each query tab can show a log of the statements it ran, with row counts and
timings. Turn it on with the **SQL Log** button on the tab. Text in the log
can be selected and copied.

## Status bar

The status bar at the bottom shows work in progress, such as a running
query, an import or a backup, and how it ended.
