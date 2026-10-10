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
- The sidebar follows the active tab: switching tabs, or opening a table
  from the command palette, selects that tab's table, view or routine,
  loading and opening the groups above it and scrolling it into view. Follow
  Active Tab in the General settings turns it off
- Table, column, function and procedure comments stored in the database are
  shown: in the sidebar tooltip, in the result header tooltip, beside table
  and column suggestions in the editor, and in a Comment column on the
  structure tab. PostgreSQL, MySQL, SQL Server and ClickHouse; SQLite has no
  comments
- Tab strip handling: drag a tab to reorder it, and the order is kept
  across restarts. Right-click a tab to
  Duplicate it (same text and connection), Pin it so it stays at the start
  and survives Close Others and Close to the Right, or close it and its
  neighbours. Middle-click closes a tab, and the tooltip shows the
  connection, database, schema and, for structure tabs, the table
- The structure tab sorts columns by any header, with a third click
  restoring the table order, and a search field filters them by name. Types
  show their length, precision and scale, and indexes list the primary key
  first, then the rest by name
- `.sql` files: Open SQL File in a database's context menu in the sidebar,
  or under the database in the command palette, opens them in query tabs on
  that database. Files dropped on the window open on the active tab's
  connection. Save (`secondary-s`) and Save As (`secondary-shift-s`) write
  them back, and the tab is named after its file with a ● while it has
  unsaved changes. Closing such a tab asks whether to save. A file that is
  already open is brought forward rather than read into a second tab. Gzip
  compressed files are read and written directly
- Run SQL File, next to Open SQL File on a database, runs a script straight
  from disk, statement by statement, without loading it into the editor. It
  opens a tab on that database for the run, shows the statement count and how
  far through the file it is, stops on the first error with the failing
  statement in the log, and can be stopped. It understands PostgreSQL dollar
  quotes, MySQL `DELIMITER` lines and SQL Server `GO` batches
- SSH tunnels authenticate with the connection's key, then its password,
  then the host's IdentityFiles or the default keys in `~/.ssh`, then the
  keys of the SSH agent. Host aliases, User, Port, IdentityFile and ProxyJump
  come from `~/.ssh/config`, following Include lines, and one or more jump
  hosts can be set in the connection dialog. known_hosts is checked for every
  hop. On Windows the SSH agent is not used
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
- Result columns can be hidden, reordered and pinned. The Columns button in
  the results tab strip lists every column with a checkbox and a pin, with a
  search; the menu of a cell or a column header hides or pins that column;
  dragging a header moves it.
  A whole table remembers its layout and column widths per table across
  sessions, any other query keeps them for reruns. Copy and export leave
  hidden columns out and follow the order. Settings > Database > Show Row
  Numbers turns the row number column off
- A cell being edited shows a small bubble at its corner with the actions on
  its value. SQL sends the text as a SQL expression instead of a value, and
  date, time and timestamp cells get a button that opens a picker with an
  editable time and a Now shortcut
- Dates and timestamps in results are shown in ISO 8601 by default, with SQL,
  day first and month first formats in the new Results settings, and
  timestamps with a time zone can be shown in local time or UTC. Copy, export
  and editing still use the value as the database returns it
- Back Up and Restore for PostgreSQL and MySQL/MariaDB databases, in the
  sidebar and the command palette. They run `pg_dump`, `pg_restore`, `psql`,
  `mysqldump` and `mysql` from PATH or the paths in the Database settings,
  and explain when a tool is older than the server. Backups take a format,
  tables, structure only tables, compression, extra arguments and a file name
  pattern (`{database}_{date}` by default). Restores read plain and custom
  dumps, gzip included, and confirm again on PROD. Progress, live output,
  Stop and a notification when done. SSH tunnels work, and the password goes
  to the tool through its environment, never its command line
- PostgreSQL partitions and inheritance children are grouped under their
  parent table in the sidebar, collapsed, with the number of partitions next
  to the parent. The sidebar filter still finds a partition by name
- Tab text is saved shortly after typing stops, not only when a query runs,
  so a crash or restart no longer loses it. A restart opens every tab with
  its text and connection, on the tab that was active
- Reopen Closed Tab (Ctrl+Shift+T, Cmd+Shift+T on macOS, rebindable, also in
  the command palette) brings back the most recently closed tabs with their
  text and connection
- The structure tab lists a table's foreign keys with their columns,
  referenced table and ON UPDATE and ON DELETE actions. Add Foreign Key picks
  the referenced table and pairs each column with a referenced one, prefilled
  from its primary key; Drop Foreign Key is in the row's context menu.
  SQLite lists its foreign keys but reports changes as unsupported
- Open Referenced Row, in the foreign key popover of a cell and in the cell
  context menu (Alt+Enter, rebindable), opens the referenced table in a new
  tab with a query limited to the matching key and runs it
- Ask on connect next to a connection's password, SSH password and key
  passphrase leaves that secret unsaved and asks for it in a dialog when the
  connection opens. The answer is kept in memory for reconnects until you
  disconnect. A connection saved without a password also asks when the
  server wants one, and asks again when it refuses what was typed. An
  exported connection keeps the setting
- A snippet can have a trigger word. Completion offers the SQL snippets in
  query tabs and the script snippets in script tabs whose trigger starts with
  the word being typed. Snippet text may hold tab stops such as `$1`,
  `${1:default}` and `$0`; the caret lands on the first one, with its default
  selected. Write `\$` for a literal dollar sign before a digit
- Environment tags beyond DEV, TEST and PROD: built-in LOCAL and STAGING, and
  custom tags with a name and a colour, managed in the new Environments
  settings page. The colour is one of the theme's, which follows light and
  dark themes, or any other from the colour picker. Each custom tag says
  whether writes on its connections are confirmed first, as on PROD. The
  filter button next to the sidebar search shows only the connections with
  one tag

### Fixes

- Formatting JSON in the cell editor and the cell comparison no longer sorts
  object keys or rounds large numbers
- Exporting an empty table as SQL wrote an INSERT without any rows; the file
  is now empty. Copy as SQL escapes strings for the result's database rather
  than always for PostgreSQL
- The sidebar selection no longer jumps to another row or disappears when
  the tree refreshes or a group above it is collapsed and opened again
- Completion offers the columns of a CTE or a subquery in FROM after its name
  or alias, also while the select list is still being written, including
  columns it selects with `*` and names from a CTE column list. A query inside
  a CTE gets the columns of its own tables, and FROM lists the statement's CTEs
- Completion lists columns and tables before functions and keywords where a
  name is expected, and the current schema's tables before other schemas.
  Keywords are offered there too once a word is typed
- Long names in the completion list are shortened in the middle, and the list
  is wider
- Completion offers the columns of tables without an alias, such as in
  `UPDATE users SET` or `DELETE FROM users WHERE`
- Completion no longer opens while a number is typed, and offers only columns
  after `alias.`
- PostgreSQL tables whose names need quoting (mixed case, spaces) listed no
  columns, and a table name shared by two schemas could pick up the other
  schema's keys
- SQLite tables with a composite primary key report every key column, not
  only the first, so editing or deleting a row in the results matches that
  row alone
- Choosing a function in completion inserts `name()` with the caret between
  the parentheses again, instead of a literal `name($0)`
- Clicking a cell in the results table no longer starts a text selection in
  the SQL view that follows the pointer with no button held
- Format SQL replaced the wrong range when text before the statement held
  non-ASCII characters, mangling the query. Lint markers and script syntax
  errors were misplaced the same way
- Show Create Statement on PostgreSQL wrote a stored generated column as a
  default, which PostgreSQL rejects, and dropped the
  `GENERATED ... AS IDENTITY` clause of identity columns
- Exporting a table shows how many rows are written, stops when the dialog is
  closed, removing the partial file, and reports success or failure in a
  notification instead of only in the log. A row that fails to arrive now
  fails the export, where it was skipped and the file looked complete. Tables
  whose names need quoting, such as mixed case, can be exported
- Copy and export as TSV took time quadratic in the number of selected rows,
  so a large selection could take many seconds
- Copy and export as JSON, Markdown, SQL and VALUES write the selected rows
  in grid order, where they could come out shuffled

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
