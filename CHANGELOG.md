# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Features

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
- More actions can be rebound in the Keybindings settings, now listed by
  group with search: cancel the running query, new query tab, close tab,
  next and previous tab (Ctrl+Tab and Ctrl+Shift+Tab), toggle the sidebar,
  toggle the results pane, import into or export the selected table, and
  refresh the connection. A shortcut that is also used by another action is
  flagged. Wrap Tab Navigation in the Editor settings decides whether next
  and previous tab go around or stop at the ends
- Toggle Results Pane hides the results under the editor and keeps the
  toolbar with Run
- Run All, next to Run Current and on Ctrl+Shift+Enter (Cmd+Shift+Enter on
  macOS, rebindable or unbindable), runs every statement in the tab whatever
  the cursor or selection. A statement that is only a comment is skipped and
  makes no result tab. On a PROD connection it asks first and says how many
  statements will run

### Fixes

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
