# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Features

- Sort a result by several columns: Shift-click another column's sort icon
  to add it as the next sort key. With more than one key, each sorted header
  shows its place, and a plain click goes back to sorting by one column
- Follow System Appearance in the Appearance settings switches between a
  light and a dark theme of your choice as the system appearance changes,
  live and at startup
- Expand * into columns: with the caret on a `*` in a select list, Show
  Code Actions (Ctrl+. or Cmd+.) replaces it with the columns of the tables
  it covers. `alias.*` keeps its alias, and with several tables each column
  is prefixed with its table. Names are quoted only where the database needs
  it
- A filter bar above a table's rows narrows them on the server. Build
  conditions from column, operator and value joined by Match All or Match
  Any, type a WHERE condition with column completion, or search every text
  column. The condition is added to the result's own query, which keeps its
  columns, ordering and limit, and the rows stay editable. WHERE in the result
  strip, `secondary-shift-f` or Toggle Filter Bar in the command palette shows
  it; Filter by Value in a cell's menu filters on that value, and the copy
  button copies the SQL the view runs. The bar's open state and the last
  operator picked are remembered
- CSV options in the export dialog: delimiter (comma, semicolon, tab, pipe or
  a custom character), header row, text for NULL, quote character, when to
  quote, and replacing line breaks inside values. Copy as CSV uses the same
  options, with defaults under Copy as CSV in the Database settings, which the
  export dialog starts from. Exports can be written as UTF-8, UTF-8 with a
  byte order mark, or UTF-16, and SQL exports can write one INSERT per row
  instead of one for all rows
- A search field in the title bar opens the command palette and shows its
  shortcut
- Blanco checks for new releases hourly and shows an Update button in the
  title bar when one is out. It downloads the release, then turns into Restart
  to apply it. On a .deb install it opens the release page instead. Check for
  Updates in the General settings turns the check off
- Duplicate in the connection context menu opens the new connection dialog
  filled in from that connection, with its passwords left out
- A system notification when an agent asks to run a write over MCP while
  Blanco is in the background. Clicking it brings Blanco forward, where the
  confirmation dialog is waiting
- Disconnect When Idle in the Database settings closes a connection, with its
  SSH tunnel, after a number of minutes without a query, metadata fetch or
  pending grid edit. The next action reconnects. A connection can set its own
  value under Show Advanced Options in the connection dialog, where 0 keeps it
  open. A connection with a query running is never closed
- The expanded cell editor opens JSON formatted, keeping key order and
  numbers exactly as stored, and has Format and Minify buttons. Text values
  that hold a JSON object or array get the same treatment. Invalid JSON is
  pointed out under the editor while typing, and applying edits stops at a
  JSON column whose new value does not parse, naming the row and column
- Export and import connections as a JSON file, from the new menu next to
  the sidebar filter, the File menu and the command palette. Export… in a
  connection's context menu writes just that one. Passwords are never
  written. Import shows what it will add first, skips connections that are
  already saved with the same settings and renames ones whose name is taken
- Accept Suggestion With in the Editor settings chooses whether Enter, Tab
  or either accepts a completion suggestion. When Enter does not accept, it
  starts a new line and closes the list
- Click a column header in the results to select the column. Copy a
  column's values from the column header's context menu or a cell's:
  one per line, or as an IN list such as `('a', 'b')` with strings escaped
  for the dialect, numbers left bare and NULL left out, each also as distinct
  values. From a cell, a selection of several rows copies just those rows
- More actions can be rebound in the Keybindings settings, now listed by
  group with search: cancel the running query, new query tab, close tab,
  next and previous tab (Ctrl+Tab and Ctrl+Shift+Tab), toggle the sidebar,
  toggle the results pane, import into or export the selected table, and
  refresh the connection. A shortcut is recorded by clicking its field and
  pressing the keys, so only a real key combination can be stored: Escape
  cancels, Backspace unbinds, and each shortcut has its own reset. A shortcut
  that is also used by another action is flagged. Wrap Tab Navigation in the
  Editor settings decides whether next and previous tab go around or stop at
  the ends
- Toggle Results Pane hides the results under the editor and keeps the
  toolbar with Run
- Open a SQLite database directly: choose it under Open in the File menu or
  from the command palette, pass its path when starting Blanco, drop the
  file on the window, or open it from the file manager. Files are
  recognised by their header, so any extension works. A file without a saved
  connection opens as an unsaved connection in the sidebar, which Save
  Connection in its context menu turns into a saved one, tabs included. The
  .deb and the macOS app register Blanco as a handler for SQLite files

### Fixes

- Formatting JSON in the cell editor and the cell comparison no longer sorts
  object keys or rounds large numbers
- Exporting an empty table as SQL wrote an INSERT without any rows; the file
  is now empty. Copy as SQL escapes strings for the result's database rather
  than always for PostgreSQL
- Clicking a cell in the results table no longer starts a text selection in
  the SQL view that follows the pointer with no button held

## [0.1.2] - 2026-10-08

### Features

- Editor font size setting for the query, script and snippet editors, with
  shortcuts to increase, decrease and reset it (rebindable, also in the View
  menu)
- Connection option to show only the configured database in the sidebar and
  the command palette (PostgreSQL and SQL Server)
- Text in the SQL view and the script log can be selected with the mouse and
  copied

### Fixes

- Completion on slow or flaky connections: a metadata fetch is no longer
  restarted on every keystroke, a failed fetch is no longer cached as an empty
  result, and suggestions stay available while the cache refreshes
- Metadata fetches for completion now respect the query timeout setting
- Word Wrap and Render Whitespace toggled from the menu or the command palette
  are now remembered across restarts

## [0.1.1] - 2026-10-02

### Fixes

- Fixed a text rendering issue

## [0.1.0] - 2026-09-29

Initial release. Supported backends: PostgreSQL, MySQL/MariaDB, SQLite, SQL
Server, ClickHouse and Redis.

### Features

- Schema aware completion, formatting and linting for SQL, plus command
  completion and highlighting for Redis
- Statement at cursor and selection execution, bind parameter prompts, query
  history and snippets
- Editable results grid with atomic commits, row filter, copy as
  CSV/TSV/JSON/SQL/Markdown, export to CSV/TSV/JSON/SQL/Markdown, charts and
  foreign key peeking
- EXPLAIN results rendered as a plan tree (PostgreSQL, MySQL, SQLite) with time
  bars for hot nodes
- Sidebar schema tree with filter, rename/truncate/drop from the tree, table
  structure view, object DDL and an ER diagram
- CSV and JSON (array or NDJSON) import with column mapping and conflict
  strategies, applied as one transaction
- Read only connections, and a confirmation before writes on connections tagged
  PROD
- JavaScript scripting (`db.query`, `db.execute`, `db.transaction`,
  `db.display`) running in a sandboxed QuickJS runtime
- SSH tunnels with `known_hosts` verification, TLS, credentials in the OS
  keychain
