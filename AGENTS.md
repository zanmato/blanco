# Blanco SQL Editor - Development Notes

## Project Overview

Blanco is a cross-platform SQL editor built with GPUI and gpui-component. It provides a modern, native UI for executing SQL queries against various databases.

## Getting Started

The SQL grammars come from `tree-sitter-sql-polyglot`, a git dependency on the `blanco-dialect-coverage` branch of our fork `zanmato/Tree-sitter-sql-polyglot` (upstream `relativelyunknown/Tree-sitter-sql-polyglot`). It holds an ANSI base grammar and one grammar per dialect built on it. Blanco enables the `postgres`, `mariadb`, `sqlite`, `tsql`, `clickhouse`, `duckdb` and `cockroachdb` features. `Dialect::sql_grammar()` picks a backend's `SqlGrammar`: MySQL connections use MariaDB's grammar, a superset of MySQL's, and SQL that belongs to no backend (snippets, the import preview) uses the ANSI base. The fork commits its parsers compressed (`parser.c.br`) and its build script inflates them, so building Blanco never runs `tree-sitter generate`.

Grammar fixes go to the fork with a corpus test in each dialect they touch, then Blanco's lockfile is bumped (`cargo update -p tree-sitter-sql-polyglot`). In the fork, generate one grammar at a time and compress the parsers before pushing. Never run `npm run generate:all`, which generates every dialect at once: ClickHouse alone peaks at about 31 GB of memory.

`sql_parser::statement_parser` keeps one tree-sitter parser per grammar and thread, created the first time that grammar is parsed, and every function that parses takes the `SqlGrammar`. `sql_parser::language` and `sql_parser::highlights_query` give a grammar's language and highlight query (the base captures plus the dialect's own). For highlighting, `sql::register_languages` registers only the ANSI grammar at startup. A backend's grammar is registered by `sql::highlight_language(db_type)` when the first editor or log view for it opens, so code that names a highlighter language for a connection must go through it. Registered names all start with `sql` (`sql`, `sql_postgres`, `sql_duckdb`, ...), which `SqlView` relies on to terminate logged statements with `;`.

The in-tree `crates/tree-sitter-redis` grammar highlights the editor when the active connection is a Redis (non-SQL) backend. We author it and commit its parser the same way the fork does: `crates/tree-sitter-redis/scripts/generate-parser.sh` runs `tree-sitter generate` and writes `src/parser.c.br` and `src/node-types.json.br`, which its build script inflates. Regenerate it after changing `grammar.js`, with the CLI release the fork uses (0.26.3) so both share a parser ABI. The command/subcommand catalog is codegenned from a live server (`COMMAND LIST` / `COMMAND DOCS`) into two committed artifacts: `grammar/keywords.js` (consumed by the grammar) and `bindings/rust/commands.rs` (exposed as `tree_sitter_redis::commands` + `tree_sitter_redis::subcommands_for`, consumed by the editor's Redis completion provider so highlighting and completion share one command set). Regenerate both after a Redis upgrade with `python3 crates/tree-sitter-redis/scripts/extract-redis-commands.py` (defaults to the docker-compose dev server on `127.0.0.1:6400`). The editor language is chosen per connection by `DatabaseType::editor_language()` (a SQL grammar's name, or `"redis"`). The Redis grammar is registered at startup in `main.rs` (`redis_syntax::register_language`). Redis tabs get `redis_completion::RedisCompletionProvider` (command/subcommand completion) instead of the SQL completion/selection-range/linting machinery, which is gated behind `supports_sql()`.

## SQL function catalogs and signature help

Built-in function completion and the signature-help popover read per-dialect
catalogs in `crates/blanco/src/sql/functions/`. `postgres.rs` (pg_catalog.pg_proc),
`mysql.rs` (MariaDB `mysql.help_topic`) and `clickhouse.rs` (`system.functions`)
are generated from the docker-compose dev servers by
`python3 scripts/extract-sql-functions.py all` (stdlib only: uses the host `psql`,
`docker exec` into the MariaDB container, and ClickHouse's HTTP port).
`sqlite.rs` and `mssql.rs` are curated by hand because those engines expose no
signature catalog; the `sqlite` subcommand only lints the curated file against
`pragma_function_list`. Every catalog is sorted by lowercased name so
`sql::functions::lookup` can binary-search and overloads stay adjacent; a test
asserts the order. User-defined functions come from
`Connection::list_function_signatures` (Postgres, MySQL, MSSQL) and are cached in
`MetadataCache::functions_by_schema`.

The popover itself lives in the gpui-component fork (`SignatureHelpProvider`,
`SignatureHelpPopover`; branch `blanco`). `sql::signature_help` implements the
provider using `CompletionContext::enclosing_call`, which the parser fills from
the tree-sitter `invocation` node with a textual fallback for unclosed calls.
Function completion items insert `name($0)` as an LSP snippet; the fork's
`insert_completion` places the caret at `$0`, so help opens right after accepting.

## Completion metadata

`SqlCompletionProvider` owns its metadata fetches (`in_flight`, keyed by `MetadataKey`)
instead of letting a completion request own them. The editor drops a request on the next
keystroke, so a fetch tied to the request restarts on every key and never finishes on a
slow connection. Requests join the running fetch, expired cache entries are served while
a refresh runs behind them, a fetch is bounded by the query timeout setting, and a failed
fetch caches nothing (`fetch.rs` returns errors, never an empty listing).

## Keyword completion

Keywords are ranked by `sql/completion/keywords.rs`, which reads the tokens
before the cursor in the innermost parenthesized group rather than the
tree-sitter tree (mostly ERROR nodes while typing): the last clause keyword,
the statement verb, and whether an item was just finished. The keywords that
position expects come first (`ItemGroup::ExpectedKeyword`, ahead of names),
the rest follow, and clause keywords the statement already holds sink to the
end. Keywords only some backends understand are listed in
`Dialect::BACKEND_KEYWORDS` and offered where `Dialect::backend_keywords` has
them. `is_sql_keyword` reads the separate `RESERVED_WORDS`, which leaves those
backend keywords out so `settings.` still completes a table named `settings`.

## Binary and deferred values

Result rows carry text, so the bytes of a binary column travel as `0x` plus
lowercase hex (`blanco_core::binary`), which every driver produces for
`ColumnType::Binary`. Anything that writes a value back goes through
`Dialect::value_literal`, which spells hex notation of a binary column as the
backend's byte literal: change operations (`ColumnValue::Binary`, binary primary
keys included), the SQL and VALUES transformers, filters
(`FilterCondition::binary`). Text without the `0x` in a binary cell is sent as a
string. `results_panel/binary_viewer.rs` is what a binary cell expands into. It
decodes its image itself and renders it as a `RenderImage`, so nothing lands in
GPUI's asset cache, and drops the texture from every window when it is replaced
or the viewer goes away.

PostgreSQL `hstore` columns are `ColumnType::KeyValue`. hstore is an
extension type without a fixed OID, and on the simple query protocol (scripts,
and queries without parameters) sqlx gives such a column's type only as an
OID, named `?`. The driver collects those OIDs while reading and names them
from `pg_type` once the statement's rows are read, since the pool's single
connection is busy until then; values are text either way. Cells show the
pairs as `key: value` (`blanco_core::hstore` parses and formats the text
form), copy and export keep the server's text, and expanding a cell opens
`results_panel/key_value_editor.rs`, which writes its pairs back as an hstore
literal through the cell's pending edit. It sits in the same overlay as the
binary viewer (`CellOverlayEditor`), and `finish_overlay_editor` commits
either.

With `results.defer_values_over_kb` set, a plain `SELECT *` on one table with a
primary key is rewritten by `Dialect::deferred_read` (`blanco_core::deferred`)
so large text and binary columns come back as a preview plus a size helper
column, which `DeferredPlan::apply` strips again. The three places that run a
grid query do this: the editor's run, the filter bar and the refresh after a
commit. The grid knows deferred cells by primary key and column name
(`results_panel/deferred_values.rs`), so sorting, filtering and moving columns
do not disturb them. Any handler that needs a value starts with
`defer_action` or `load_deferred_values_then`, which fetch by key and run the
handler again. A new handler that reads `delegate.rows` to write or copy must
do the same, or it will use previews.

## DuckDB

`crates/duckdb` (package `blanco-duckdb`) wraps duckdb-rs, built with
`bundled` and pinned with a tilde (`~1.10506.0` is DuckDB 1.5.6, the crate's
second component encodes the DuckDB release). It is bundled because neither
Windows nor Debian has a system libduckdb and the self updater replaces only
the executable. The first clean build compiles DuckDB's C++ for several
minutes. Each compiler process takes gigabytes and cc starts one per Cargo
job, so on Linux `.cargo/config.toml` points `CXX` at `scripts/cxx-limited`,
which runs at most `BLANCO_CXX_JOBS` (default 4) at once. A package built
on its own (`cargo test -p database`) would turn on fewer features of the
crates libduckdb-sys's build script is compiled from, and Cargo would build
DuckDB again for it. The `[build-dependencies]` of `crates/duckdb/Cargo.toml`
pin those features to the workspace's. Cargo would also give the build
script's dependencies debug info only when the same crates are normal
dependencies of the command's packages, so `[profile.dev.build-override]`
sets `debug = false` outright. `scripts/duckdb-build-inputs`
(nightly, for `--unit-graph`) reports any command that drifts after a
dependency change. duckdb-rs is synchronous, so every call runs in `spawn_blocking`.

A process may open a DuckDB file once, so connections to one file share an
instance from `OPEN_DATABASES` and take their own DuckDB connection from it.
Each `DuckdbConnection` keeps one session for the user's statements (`SET`,
`USE`, temp tables and transactions carry over) and takes a fresh connection
per catalog lookup. Dropping a run's future interrupts the session through
DuckDB's interrupt handle. Results go through Arrow and its display
formatter, so every type (lists, structs, maps, decimals, timestamps with zone)
gets text without per-type code; blobs become `0x` hex. DuckDB answers
`INSERT`/`UPDATE`/`DELETE` with a one-column `Count` result, which becomes
`rows_affected`. Scripts are split with `statement_stream::split_script`,
because duckdb-rs's own multi-statement prepare runs all but the last
statement silently.

The sidebar treats DuckDB like PostgreSQL: databases (the file's own and any
`ATTACH`ed ones, from `duckdb_databases()`) above schemas. The connection
string (`blanco_duckdb::connection_string`) carries `database=` so a
connection works in one of them, `read_only=1` to open the file read only, and
`memory_id=` to keep in-memory databases of different connections apart.
DuckDB's text functions take no `BLOB`, so deferred values and the binary
filters compare blobs through `HEX()`. `ADD COLUMN` takes no constraint and
`ALTER TABLE` one action, so the DDL builders split those into steps. The
built-in function catalog `sql/functions/duckdb.rs` is generated with
`python3 scripts/extract-sql-functions.py duckdb`, which reads
`duckdb_functions()` through `cargo run -p blanco-duckdb --example
function_catalog`. A table function's named options (`read_csv`'s `delim`,
`header` and the rest) fold into one trailing `[option = value, ...]`
parameter and are listed under Options in its documentation, since shown as
positional parameters they made a 46 argument signature.

Database files open from File, Open, a drop on the window, launch arguments
and the platform's open requests. `database_file::database_file_type` tells
SQLite from DuckDB by header (`SQLite format 3`, or `DUCK` at offset 8), so
the extension does not matter. File, New creates either kind and opens it the
same way; `Dialect::new_file_statement` makes SQLite write its header at once.
The `.deb` ships `blanco-duckdb.xml`, since shared-mime-info has no DuckDB
type for the desktop entry to claim.

## CockroachDB

CockroachDB speaks PostgreSQL's protocol, so it is its own `DatabaseType` on
the PostgreSQL driver: `PostgresConnectionFactory::cockroachdb()` makes a
`PostgresConnection` `with_database_type(CockroachDb)`, which names the
backend in messages and switches the few catalog reads that differ (views
leave out materialized views, which CockroachDB's `information_schema.views`
lists too, and databases report no size, since `pg_database_size` answers 0
and the real figures sit behind the closed `crdb_internal`). Everything else
is a `Dialect` arm, mostly shared with PostgreSQL. The differences:
`EXPLAIN` has no JSON format, so `explain_plan::parse_cockroachdb` reads the
text tree and adds each operator's own time up to inclusive times. The
process list reads `SHOW CLUSTER SESSIONS` and cancels with `CANCEL QUERIES`
and `CANCEL SESSION`, taking only 32 hex digit session ids.
`autocommit_before_ddl` commits the open transaction before each DDL
statement, so `rolls_back_ddl` is false. A table declared without a primary
key gets a hidden `rowid` key (`Dialect::hidden_row_key`), which the
catalog reports as the primary key but `SELECT *` leaves out, so the grid's
read rewrite (`Dialect::deferred_read`) names every column whenever the key
is that hidden one, with value deferral off too. `pg_dump` fails on its
catalog, so there is no backup. The function catalog is generated with
`python3 scripts/extract-sql-functions.py cockroachdb`, which folds the
per-type overloads CockroachDB lists into polymorphic signatures. The compose
`cockroachtestdb` is a secure single node (user and password `blanco`, port
26258) whose `docker/cockroach/init.sql` grants the user `admin`, as the
process list test needs. `test_cockroachdb_catalog_differences` covers the
driver's differences.

## Rich clipboard

GPUI's `ClipboardItem` holds only text and images, so Copy as HTML writes
through `crates/clipboard` (`blanco-clipboard`), which offers one copy under
several MIME types (`text/html` plus a `text/plain` alternative). Its platform
modules are adapted from GPUI's own clipboards (Apache-2.0) and cut down to
writing: NSPasteboard on macOS, Win32 with "HTML Format" on Windows, a
selection server thread on X11 (from GPUI's arboard-derived code), and on
Wayland a seat bound on GPUI's own `wl_display`, from the window's raw
display handle, because only input serials from that connection may take the
selection. Its data device exists only for the `set_selection` request:
Mutter sends selection and drag events to the newest data device of a
client, so a lasting one would starve GPUI's and break pasting and drops. GPUI
copies the plain text first, so its own clipboard state, which misses the
rich copy, still pastes the same text. Mutter ignores a selection whose serial
is not newer than the current one, so the rich copy then waits on a thread
for a later input event (the release of the same click or key) and takes the
selection with that serial; with none within a second the plain text stays.
`blanco_clipboard::init` runs when a window opens so
the Wayland seat has seen the click before the copy. A window without a
display (tests) makes `write` fail on every platform, and the caller falls back
to GPUI's plain text copy. Reading stays with GPUI. Remove the crate once GPUI
can write other MIME types. `cargo run -p blanco-clipboard --example x11_write`
exercises the X11 path against a real server.

## Process list

`editor/process_list.rs` lists a server's sessions and cancels a query or
ends a session (connection menu, Process List; palette, a database's
Process list). The listing query and the cancel and kill statements are
`Dialect::process_list_query`, `cancel_query_statement` and
`kill_session_statement` (`dialect/process_list.rs`): PostgreSQL, MySQL and
MariaDB, SQL Server (kill only) and ClickHouse (cancel only, as it has no
sessions). The statements accept only ids of the backend's form. The tab
works through `DatabaseService::open_dedicated_connection`, a connection of
its own outside the shared cache: PostgreSQL's pool holds one connection and
SQL Server's client serves one request at a time, so on the shared
connection the listing would wait behind the query it is meant to cancel.
Actions are confirmed and refused on read-only connections, which the
connection wrapper would not catch for PostgreSQL's `SELECT
pg_terminate_backend`. `test_process_list_finds_cancels_and_kills_sessions`
exercises every backend against the compose servers.

## Invisible characters

One rule decides which characters look like nothing or a plain space:
`invisible_character` in the gpui-component fork. The editor boxes them
when `editor.highlight_invisible_characters` is on (the default, via
`InputState::highlight_invisible_characters`), and result cells mark them
with `results.show_invisible_characters` (`display_text` in
`results_panel/cell_edit_state.rs`). Both only look at what is on screen: a
cell formats its first `DISPLAY_CHARACTERS`, and the editor scans the visible
rows and, within a long unwrapped row, only the visible columns. Measure
changes to either with `scripts/measure-frames large-values` or
`large-query`, which time frames under Xvfb with GPUI's `ZED_MEASUREMENTS`.

## Query timing

`QueryResult::timing` is measured by each driver around its own row loop with
`blanco_core::StatementTimer`: execution runs until the first row (or the end of
a statement without rows), fetch from the first row to the last. sqlx drivers
acquire their connection before starting the timer, so connecting is not
counted, and a script restarts the timer at every statement boundary. The
Postgres and MySQL pools set `test_before_acquire(false)`: sqlx's ping on
checkout cost a round trip per query that showed up as a gap between the total
and the split. A connection the server dropped fails one query instead, which
`note_possible_disconnect` evicts (`test_dropped_pool_connection_reconnects_on_next_run`). SQL Server
walks tiberius' item stream (`timed_results`) instead of `into_results`, and
ClickHouse counts the first HTTP body chunk as the first row. The run's SQL log
comment shows the split after the total, and query history stores it in
`execution_us`/`fetch_us` next to the whole run's `duration_ms`. A new driver
path that builds a `QueryResult` from a server query should fill `timing`.

## Types and sequences

The sidebar lists PostgreSQL's (and CockroachDB's) enums, composite types,
domains and sequences from `Connection::list_types` and `list_sequences`, in
category folders after Triggers, and opens their DDL like a routine's:
`ObjectKind` (once `RoutineKind`) names every object `object_ddl` returns, and
PostgreSQL builds the type and sequence DDL from the catalog. A new
`Connection` method needs forwarding in `database::TokioConnection`, which
every driver is wrapped in, or the app sees the trait default.
`ColumnInfo::enum_labels` carries an enum column's labels, so the grid's
table columns and completion's column cache have them without another query:
the cell editor's bubble offers them under Pick Value, and
`sql/completion/enum_values.rs` finds a value typed for a column from the text
before the cursor (a half typed string leaves the syntax tree in error), which
completion resolves like a column completed where its name ends.

## SQL log selection

`blanco_ui::SqlView` entries are `SelectableLogEntry` elements that register with
gpui-base's window text selection (`TextSelectionHandle`, the `TextSelectionLayer` that
`Root` mounts). Drag selects within and across entries and `Root`'s copy binding copies
it. gpui-base's own `SelectableText` cannot carry syntax highlights, hence the element.

## Dialect, ConnectionContext and tabs

Every per-backend SQL fact lives on `blanco_core::Dialect` (`crates/blanco-core/src/dialect.rs`),
obtained with `db_type.dialect()`: identifier quoting (`quote_identifier`, `quote_qualified`,
`quote_path`), string literal escaping, bind placeholders, row limiting (`apply_row_limit`
emits `TOP n` for SQL Server), EXPLAIN wrapping and plan parsing, upsert spelling, default
schema/port and capability flags (`supports_sql`, `rows_editable`, `supports_create_table_ddl`,
`supports_column_ddl`). Do not match on `DatabaseType` in UI, transformer or import code; add a
method here instead so a new backend is one arm per method.

The table a result grid edits is a path (`QueryResult::table_name`, `sales.orders`), schema
included whenever the query names one, built by `TableExtractor` with unquoted parts folded as
the backend folds them (`Dialect::unquoted_identifier`) and quoted only where needed
(`Dialect::path_segment`). Writes quote it with `quote_path`; lookups split it with
`split_table_path` and pass the schema on. An unqualified name means what the session's search
path finds, so PostgreSQL's `get_columns_for_table` resolves it there rather than assuming
`public`. `results_panel/multi_schema_test.rs` edits, defers and follows foreign keys across
schemas against the compose PostgreSQL and CockroachDB, with a namesake in `public` each time. `DatabaseType::ALL` / `index()`
give the connection dialog order.

The identity a tab or action works against is one `blanco_core::ConnectionContext`
(connection id, name, type, database, schema, environment). Actions such as
`CreateNewQueryTab`, the `*Params` structs and every tab carry `context` rather than the six
fields. `EnvironmentType` lives in blanco-core (re-exported by app-database).

Query and script tabs embed a shared `ConnectionBackedTab` (`editor/tabs.rs`) and deref to
it, so `tab.title`, `tab.editor`, `tab.context`, `tab.db_id` work on both. Prefer the
`TabType` accessors (`title(cx)`, `context(cx)`, `connection_tab()`, `query()`, `db_id()`)
over matching on the enum; the tab strip groups every connection-bound tab under its
connection using `context(cx)`.

Column, index and foreign key DDL is built by `blanco_core::ddl::{column_operation_sql,
index_operation_sql, foreign_key_operation_sql, table_change_sql}` and driven from the
structure tab (`editor/table_structure.rs`). The tab shows one section at a time (columns,
indexes, foreign keys, table, create statement) picked from a list on its left, and a
selected row opens in a side panel (`table_structure/forms.rs`: `ColumnForm`, `IndexForm`,
`ForeignKeyForm`), whose grids live in `table_structure/lists.rs`. Nothing the panel saves
runs: it stages a `StructureChange`, whose statements are built at once so an unsupported
change is refused when it is added. `table_structure/staging.rs` replays the pending list
onto the loaded structure, and the lists show that replay with each row marked (added,
modified, dropped), so a staged column can be indexed and every row action addresses a
staged row. Editing a column that is still to be added replaces its addition. Undoing a
change replays the rest and drops the ones that no longer fit, as a reload does. The bar at
the bottom shows the pending statements and runs them as one
`execute_operations_transactional` batch (`Dialect::rolls_back_ddl` says whether a failure
rolls back). On a connection whose environment confirms writes, Apply opens a confirmation
with the statements first. A structure tab with pending changes counts as unsaved for
closing tabs and windows (`TabType::unsaved_name`).

`ColumnOperation::Alter` carries the column before and after (`ColumnSpec`, comment
included), and each backend emits only what changed, so a rename alone stays a rename
where nothing else could be changed in place. MySQL changes a column only by restating it
with `CHANGE COLUMN`, and its catalog's `DATA_TYPE` drops `unsigned`, enum labels and
`AUTO_INCREMENT`, so its `table_info` reports each column's `COLUMN_TYPE` and attributes
(`TableInfo::column_definitions`), the panel starts from those (`staging::column_spec`), and
the builder repeats them. Generated MySQL columns are refused rather than restated without
their expression. Comments are set apart from the definition elsewhere: `COMMENT ON` on
PostgreSQL, CockroachDB and DuckDB, `COMMENT COLUMN` and `MODIFY COMMENT` on ClickHouse,
the `MS_Description` extended property on SQL Server (one `IF EXISTS` statement picks
add, update or drop). SQLite has no comments (`Dialect::supports_comments`).

`IndexSpec` adds a method (`Dialect::index_methods`, ClickHouse's skipping index types), a
partial index condition (`supports_partial_indexes`) and NULLS NOT DISTINCT, which needs
PostgreSQL 15: the tab reads `Connection::server_version_num` and asks
`Dialect::supports_nulls_not_distinct`. The Postgres index listing reads
`indnullsnotdistinct` through `to_jsonb(pg_index)` so older servers answer NULL instead of
failing. The type field suggests `Dialect::data_types()` (`dialect/data_types.rs`, plus the
database's own types from `list_types`) through an `EditorState` completion provider,
inserting a type that takes arguments as a `name($0)` snippet.

The Table section reads `Connection::table_info` (comment, row estimate, sizes, creation
and last modified time, backend options). A failure leaves it empty rather than failing the
tab. The last modified time says what it is worth (`LastModified::note`): MySQL's InnoDB
`UPDATE_TIME` and SQL Server's index usage statistics reset when the server restarts, and
ClickHouse's is its newest data part. SQLite cannot change a column's type, nullability or
default in place, nor add or drop a foreign key, and reports `DdlError::Unsupported` instead
of a lossy table rebuild. The foreign keys list reads `Connection::get_foreign_keys_for_table`.
Drivers return one catalog row per key column and fold them with
`ForeignKeyConstraint::from_column_rows`. `test_column_and_index_ddl_round_trip` exercises
the builders, comments, table information and NULLS NOT DISTINCT against every compose
backend.

The create statement section shows `Connection::table_ddl`, which also serves the MCP
`get_table_ddl` tool. PostgreSQL builds it from the catalog: views as views, `INHERITS`
with only the table's own columns (`attislocal`, `conislocal`), `PARTITION BY` and
`PARTITION OF`, `WITH (...)`, then indexes, comments, triggers (`pg_get_triggerdef`) and
grants (`aclexplode`, the owner and CockroachDB's `admin` and `root` left out). Routines get
their `proacl` grants. MySQL and ClickHouse add grants from `TABLE_PRIVILEGES` and
`system.grants`. A failing grants read is logged and leaves the grants out, so a user who
may not read the privilege catalog still gets the statement.
`test_table_ddl_includes_comments_partitions_triggers_and_grants` recreates the PostgreSQL
objects from their DDL and compares.

A new table is the same tab in its creating mode (`TableStructureTab::new_table`, opened by
the `NewTable` action from the sidebar's connection, database, schema and Tables folder
menus, which know a folder by `TreeItemMetadata::category`). Its pending changes are only
additions, and editing an added column replaces its addition, so they read as one
`ddl::NewTable` that `create_table_sql` turns into a single `CREATE TABLE` with the primary
key (`ColumnSpec::primary_key`) and foreign keys inline, followed by indexes and column
comments. Staging checks that statement instead of building alterations, which is how SQLite
and DuckDB get the foreign keys they cannot add later. It starts from
`Dialect::new_table_key`. Once created, the tab takes the table's name and shows it from the
server.

## Virtual foreign keys

Foreign keys the user declares for views and tables without constraints live in the app
database's `virtual_foreign_keys` table (`app_database::VirtualForeignKey`), keyed by
connection, database, schema (empty when there is none) and table, and deleted with their
connection by its foreign key. A temporary connection has no row there, so the structure
tab offers no virtual keys on one. They are never sent to the server. The structure tab's
foreign keys section lists them after the declared keys and edits them in
`ForeignKeyForm`'s virtual mode, which saves at once. `virtual_keys::merge_into_columns`
gives a table's columns the keys as `ColumnInfo::foreign_key` wherever the server declares
none, which the query run does for `QueryResult::table_columns`, so the grid's foreign key
popover and open referenced row follow them unchanged. The schema graph draws them as
dashed edges (`GraphEdge::dashed`).

## Connection secrets

Passwords, SSH passwords and key passphrases live in the OS keyring through GPUI's
credential API, one entry per connection and `database::SecretKind`. They are never
loaded in bulk: the sidebar's `ConnectionData` and `DatabaseService`'s stored
`ConnectionConfig`s carry no secrets (callers pass `config.without_secrets()`), and
`DatabaseService::get_or_create_connection` fills the missing ones through its
`ConnectionSecretStore` only when a new physical connection is opened. The store is
`connection_credentials::KeyringSecretStore`, a channel bridge onto one foreground
task that awaits each `read_credentials` before taking the next request. On Linux
every GPUI credential read opens a fresh Secret Service session, and a burst of
concurrent sessions crashes gnome-keyring-daemon, so keep reads serialized and lazy.
The edit dialog hydrates the single connection it shows with
`connection_credentials::hydrate_connection`.

Secrets in `ConnectionConfig::ask_secrets` are never stored. `get_or_create_connection`
asks for them through its `SecretPrompt` instead, and asks for a missing secret the
server or SSH host refuses (`secret_prompt::rejected_secret`), retrying a few times.
Answers are kept in `DatabaseService`'s session map, reused for reconnects, and
dropped by `forget_session_secrets` on Disconnect or when the config is replaced.
The prompt is `secret_prompt::DialogSecretPrompt`, the same kind of foreground bridge
as the keyring store, showing one dialog at a time. A connection without a password,
or with a typed one, is pinged on open so a refusal surfaces while it can be asked
for; PostgreSQL's pools otherwise only fail at the first query.

A PostgreSQL connection without a password gets one from the password file
(`database::pgpass`, libpq's rules: `PGPASSFILE`, else `~/.pgpass`) in
`DatabaseService::apply_password_file`. That is the only lookup: the Postgres
driver's own is switched off in `PostgresConnection::connect_options`, because it
matches an SSH tunnel's local end and reads `~/.pgpass` even when `PGPASSFILE`
is set. Tests that connect without a password call
`DatabaseService::set_password_file` with a temporary path, so they do not
depend on the home directory of whoever runs them.

## Windows

`windows.rs` owns Blanco's windows. Each window is a `BlancoApp` with its own tabs,
sidebar and command palette over the shared `DatabaseService`, and a row in the app
database's `windows` table (geometry, display UUID, active tab, last activation);
`query_tabs.window_id` says which window shows a tab. `windows::restore` reopens every
row at startup, the most recently active last, on the display whose UUID was saved.
Closing one of several windows deletes its row and, by cascade, its tabs (unsaved file
changes are asked about first); the last window closing quits and keeps its row.
Anything that is not one window's runs once in `windows::init`: database service
messages, the idle disconnect sweep, files the platform asks to open (frontmost
window), MCP requests (the window holding the tab named by id, else the frontmost) and
the status bar (`StatusBarState::shared`). Moving a tab rebuilds it in the new window
from `MovedTab`, keeping its row, because tab entities belong to the window that made
them. Code that must reach every window (sidebar reloads after a connection changes,
tabs of a saved temporary connection) goes through `windows::reload_connections` and
`windows::move_tabs_to_connection`, which defer so they can be called from inside any
panel's update. Call `update_window`, not `WindowHandle::update`, to run app code in a
window: the latter holds the `Root` view that dialogs and notifications read.

## Updates

`update_manager::UpdateManager` (a global installed in `main.rs`) asks GitHub for the
latest release hourly while `general.check_for_updates` is on and tracks an
`UpdateState` entity: `Available` renders an Update button next to the sidebar toggle,
clicking it downloads the platform asset (`Downloading`), and `Ready` turns the button
into Restart, which swaps the running executable with `self_replace` and calls
`cx.restart()`. An install whose executable directory is not writable (the `.deb` in
`/usr/bin`) gets `Manual` instead, where Update opens the release page. Releases are staged under `<data_local_dir>/blanco/updates/<version>/`.
A download is checked against the release's `SHA256SUMS` and, with `UPDATE_PUBLIC_KEY`
embedded, against the minisign signature `SHA256SUMS.minisig` that the release
workflow makes with the `MINISIGN_SECRET_KEY` secret (`scripts/generate-update-key.sh`).
`release_asset()` must name the assets exactly as `build-and-package.yml` uploads them,
so change both together. Tests build `BlancoApp` without the global, hence `try_global`.

## Nix

`flake.nix` builds Blanco from source with crane (`nix/package.nix`) for
x86_64 and aarch64 Linux, and offers it as an overlay and a dev shell. The
package's source is a fileset of `Cargo.toml`, `Cargo.lock`, `.cargo`,
`crates` and `scripts/cxx-limited`, so a build input outside those must be
added there. `.cargo/config.toml`'s relative `CXX` does not survive crane's
dependency build, so the package sets `CXX_<target>` to a wrapper that runs
`scripts/cxx-limited` from the store. GPUI loads Vulkan, Wayland, xkbcommon
and the X11 libraries with dlopen, so they reach the binary as an rpath
(`runtimeLibraries`) rather than a wrapper's `LD_LIBRARY_PATH`, which the
shell in Blanco's terminal would inherit. A new library loaded that way belongs in that list.
`dontPatchELF` keeps the fixup's `--shrink-rpath` from dropping them again.
`scripts/nix-docker` runs any Nix command against the checkout in the
`nixos/nix` container, with the store in a volume, for testing on a machine
without Nix (`scripts/nix-docker nix build -L .#blanco`), and
`scripts/nix-docker bash scripts/nix-smoke` starts the package under Xvfb with
Mesa's software Vulkan driver and saves a screenshot. The flake sees only
files git tracks. A prebuilt package fed from the release assets is planned.

## Architecture

### File Structure

- `main.rs` - GPUI application initialization
- `windows.rs` - Opening, restoring and closing windows, and the services they share
- `app.rs` - Main application layout with title bar, sidebar, editor, results
- `editor_panel.rs` - Tabbed SQL editor with run/format buttons
- `results_panel.rs` - Table view for query results
- `sidebar.rs` - Database connection tree sidebar
- `connection.rs` - Mock database connection data structures

### Key Components

- **Root Component**: All gpui-component windows MUST wrap the main app in `gpui_component::Root::new()`
- **Entity Pattern**: Used for state management (`Entity<T>`)
- **Focusable**: Components implement this trait for focus handling
- **TableDelegate**: Custom pattern for rendering table data

## Important Learnings

### GPUI Trait Requirements

Always import these traits for full API access:

```rust
use gpui::{
    prelude::FluentBuilder,  // .when(), .when_some()
    AppContext,              // .new() method for entities
    InteractiveElement,      // .on_click(), .on_mouse_down(), make sure to use .id()
    ParentElement,           // .child(), .children()
    Render,                  // Render trait
    Styled,                  // Styling methods
};
```

### gpui-component Specifics

- Button variants require: `ButtonVariants` trait
- Sizing methods require: `Sizable` trait
- Must use `gpui_component::Root` as top-level view

### Window Configuration

WindowOptions requires these fields:

- `window_decorations: Some(gpui::WindowDecorations::Client)` - For custom decorations
- `is_minimizable`, `is_resizable`, `tabbing_identifier` - All required in GPUI 0.2.0
- Bounds must be wrapped: `WindowBounds::Windowed(bounds)`

### Icons

Check the IconName enum before using an icon. New icons are copied from [Lucide](https://github.com/lucide-icons/lucide) (`icons/*.svg`).

## Known Issues (Fixed)

1. ✅ **Runtime panic**: "line should exists in text_wrapper" - Fixed by removing `.default_value()`
2. ✅ **Runtime panic**: Root component unwrap - Fixed by wrapping in `gpui_component::Root`
3. ✅ **Compilation**: Missing trait imports - Fixed by adding FluentBuilder, AppContext, etc.

## Theme System

gpui-component provides a comprehensive theme system:

- **Light/Dark Mode**: Automatic system appearance sync or manual toggle
- **Customizable**: `radius` (px(6.)), `radius_lg` (px(8.)), fonts, colors
- **Access Theme**: `cx.theme()` provides access to all theme colors and settings
- **Change Theme**: `Theme::change(ThemeMode::Dark, window, cx)` to switch modes
- **Global Access**: `Theme::global(cx)` or `Theme::global_mut(cx)` for mutations

Themes live in `crates/blanco/assets/themes/` and are embedded in the binary. The
loader ignores keys it does not recognise, so a token spelled the way Zed spells
it parses cleanly and then silently falls back to gpui-component's default theme.
The authority on key names is `ThemeConfigColors` in gpui-component's
`theme/schema.rs`; `embedded_themes_are_registered_and_parse` in `main.rs`
deserializes each file into `ThemeSet` to catch drift. `catppuccin.json` is
generated, not hand-edited: change the role mapping in
`scripts/generate-catppuccin-theme.py` and re-run it.

`tokyo-night.json` is converted from the VS Code theme by
`cargo run -p vsx-themes --example convert -- --open-vsx enkia.tokyo-night --sql`
(or a clone of its repository in place of `--open-vsx`). Each third-party theme
keeps its license beside it as `<theme>.LICENSE.txt`, which is not embedded.
`embedded_themes_are_registered_and_parse` also fails on keys `custom_themes::unknown_keys`
reports: gpui-component flattens the status colors (`error`, `error.background`, ...)
directly into `highlight`, and a nested `status` object, `chart_bullish` or
`tiles.background` were all silently ignored in the past.

Users add theme files to `<config dir>/blanco/themes` (`custom_themes::themes_dir`).
`custom_themes::load_custom_themes` reads them once at startup, before the
embedded themes, since `load_themes_from_str` keeps the first theme of a name
and a custom file should win. Edits apply after a restart. Blanco does not use
`ThemeRegistry::watch_dir`: its watcher rebuilds the registry from the folder
alone without calling `on_load` again, which drops the embedded themes. Keys a
file has that the `ThemeSet` schema lacks are reported in a notification, as
the loader ignores them. `test-data/themes/aurora.json` is a theme from
gpui-component kept for the tests.

### VS Code themes

`crates/vsx-themes` converts VS Code color themes and holds the settings
panel that installs them from Open VSX (Appearance, VS Code Themes). It
is written to be copied into the other gpui-component apps (broquest,
shouting-robin): nothing in it names Blanco, and only its `gpui` and
`gpui-component` dependencies change with the app (`gpui-component = { package
= "gpui-kit", ... }` there). An app creates a `ThemeBrowser` with the folder
its custom themes load from at startup, `ConvertOptions` (`SQL` here), and what
Use means (`settings::use_theme`), and adds `browser::setting_group` to a
settings page. Its HTTP is blocking reqwest on GPUI's background executor, so
it needs no tokio bridge. `gallery.rs` speaks VS Code's gallery API to Open
VSX's VS Code adapter (`/vscode/gallery`, see its swagger "VSCode Adapter"),
not the VS Code Marketplace, whose terms limit it to Microsoft's products. The
adapter rate limits with 429 and `Retry-After`, which the client waits out once. The
Themes category also holds icon themes. Tags tell them apart (`vsce` adds
`color-theme`, `icon-theme`, `product-icon-theme` from what a package
contributes), so only untagged packages, made by older tools, get their
manifest fetched. Open VSX's tag filter (filter type 1) does not narrow the
results, so the tags are read on the client.

`package.rs` reads a `.vsix` or an extension folder: JSONC, `%key%` labels from
`package.nls.json`, `include` chains and `.tmTheme` token colors.
`convert.rs` maps workbench colors by role with fallbacks and contrast floors,
and finds syntax colors by asking which `tokenColors` rule VS Code would apply
to a representative scope per capture (`CAPTURE_SCOPES`, with `SQL`'s
preferred scopes first). Converting the Catppuccin VS Code extension and
diffing it against `catppuccin.json` is a good check after changing the
mapping: surfaces should match, and the remaining differences are the VS Code
port's own choices (surface2 borders, mauve `storage.type`). The registry has
no way to replace or drop a theme, so reinstalling or removing one takes
effect after a restart. For the same reason Try never registers: it applies the
converted `ThemeConfig` with `Theme::update` and keeps a clone of the whole
`Theme` from before, which Revert, or releasing the panel, writes back only
while the tried config is still the one on screen (`browser::is_showing`), so
a theme chosen meanwhile stays. Converted themes name no fonts, so an app's
own font settings survive a Try. Installed files are `vscode.<publisher>.<name>.json`.

### Using Themes

```rust
// Access theme in render
.bg(cx.theme().background)
.text_color(cx.theme().foreground)
.border_color(cx.theme().border)

// Custom radius
.rounded(cx.theme().radius)
```

## Database Integration (SQLX)

Added SQLX for SQLite database queries:

- **Module**: `database.rs` - DatabaseManager with async query execution
- **Features**: Connection management, query execution, result parsing
- **Dependencies**: sqlx 0.8 with sqlite, runtime-async-std runtime

### Future Integration

- Wire DatabaseManager into EditorPanel for query execution
- Update ResultsPanel to display QueryResult
- Add connection management UI

## Settings System

User-configurable settings stored in JSON:

- **Location**: `~/.config/blanco/settings.json` (or platform equivalent)
- **Modules**: `settings/mod.rs` - Settings struct and persistence
- **UI**: Settings tab in EditorPanel with live validation
- **Scope**: Theme, Editor (fonts, tabs), Database (connections, timeout)

## Visual GPUI Tests

Tests that exercise UI components (editor, results panel, etc.) use `#[gpui::test]` which provides a `TestAppContext`. These tests open real windows and render components, catching runtime panics that unit tests miss.

### Test harness

`src/test_harness.rs` contains a shared `TestHarness` and helpers for setting up the full Blanco stack (in-memory AppDatabase, DatabaseService with temp SQLite file, AppSettings, EditorPanel with a query tab).

```rust
#[gpui::test]
async fn test_something(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(&harness, "SELECT 1", &mut cx);
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    let rows = result_row_count(&harness, &cx).expect("should have rows");
    assert_eq!(rows, 1);
}
```

### Key patterns

- `cx.executor().allow_parking()` must be called before any database IO, since tokio runs on separate threads from smol
- `wait_for_query()` polls `run_until_parked()` in a loop because tokio tasks are parked from smol's perspective while in flight
- Use `InputState::set_value()` to replace editor text, not `replace()` (which only replaces the selected range)
- Inside `open_window`, `cx` is `&mut App`. Import `gpui::AppContext` to get `.new()` for creating entities
- `VisualTestContext::from_window(handle, cx)` converts a `TestAppContext` into one that provides `window` to `update_in` calls
- Add `#[cfg(test)]` accessors on structs rather than making fields `pub(crate)` (e.g. `EditorPanel::active_query_tab()`, `ResultsPanel::table_state()`)

### Running tests

```bash
cargo test -p blanco -- test_execute         # visual tests only
cargo test -p blanco                          # all tests
cargo llvm-cov -p blanco --summary-only       # coverage report
```

## Rust coding guidelines

- Never revert to a simpler approach, such as hardcoding or stubbing things.
- Prioritize code correctness and clarity. Speed and efficiency are secondary priorities unless otherwise specified.
- Do not write organizational or comments that summarize the code. Comments should only be written in order to explain "why" the code is written in some way in the case there is a reason that is tricky / non-obvious.
- Prefer implementing functionality in existing files unless it is a new logical component. Avoid creating many small files.
- Avoid using functions that panic like `unwrap()`, instead use mechanisms like `?` to propagate errors.
- Be careful with operations like indexing which may panic if the indexes are out of bounds.
- Never silently discard errors with `let _ =` on fallible operations. Always handle errors appropriately:
  - Propagate errors with `?` when the calling function should handle them
  - Use `.log_err()` or similar when you need to ignore errors but want visibility
  - Use explicit error handling with `match` or `if let Err(...)` when you need custom logic
  - Example: avoid `let _ = client.request(...).await?;` - use `client.request(...).await?;` instead
- When implementing async operations that may fail, ensure errors propagate to the UI layer so users get meaningful feedback.
- Never create files with `mod.rs` paths - prefer `src/some_module.rs` instead of `src/some_module/mod.rs`.
- When creating new crates, prefer specifying the library root path in `Cargo.toml` using `[lib] path = "...rs"` instead of the default `lib.rs`, to maintain consistent and descriptive naming (e.g., `gpui.rs` or `main.rs`).
- Avoid creative additions unless explicitly requested
- Use full words for variable names (no abbreviations like "q" for "queue")
- Use variable shadowing to scope clones in async contexts for clarity, minimizing the lifetime of borrowed references.
  Example:
  ```rust
  executor.spawn({
      let task_ran = task_ran.clone();
      async move {
          *task_ran.borrow_mut() = true;
      }
  });
  ```
- Run clippy and fmt after completing a feature to make sure no issues were introduced

## GPUI

GPUI is a UI framework which also provides primitives for state and concurrency management. This app is using gpui-component, which provides a set of prebuilt components.

### Context

Context types allow interaction with global state, windows, entities, and system services. They are typically passed to functions as the argument named `cx`. When a function takes callbacks they come after the `cx` parameter.

- `App` is the root context type, providing access to global state and read and update of entities.
- `Context<T>` is provided when updating an `Entity<T>`. This context dereferences into `App`, so functions which take `&App` can also take `&Context<T>`.
- `AsyncApp` and `AsyncWindowContext` are provided by `cx.spawn` and `cx.spawn_in`. These can be held across await points.

### `Window`

`Window` provides access to the state of an application window. It is passed to functions as an argument named `window` and comes before `cx` when present. It is used for managing focus, dispatching actions, directly drawing, getting user input state, etc.

#### Entities

An `Entity<T>` is a handle to state of type `T`. With `thing: Entity<T>`:

- `thing.entity_id()` returns `EntityId`
- `thing.downgrade()` returns `WeakEntity<T>`
- `thing.read(cx: &App)` returns `&T`.
- `thing.read_with(cx, |thing: &T, cx: &App| ...)` returns the closure's return value.
- `thing.update(cx, |thing: &mut T, cx: &mut Context<T>| ...)` allows the closure to mutate the state, and provides a `Context<T>` for interacting with the entity. It returns the closure's return value.
- `thing.update_in(cx, |thing: &mut T, window: &mut Window, cx: &mut Context<T>| ...)` takes a `AsyncWindowContext` or `VisualTestContext`. It's the same as `update` while also providing the `Window`.

Within the closures, the inner `cx` provided to the closure must be used instead of the outer `cx` to avoid issues with multiple borrows.

Trying to update an entity while it's already being updated must be avoided as this will cause a panic.

When `read_with`, `update`, or `update_in` are used with an async context, the closure's return value is wrapped in an `anyhow::Result`.

`WeakEntity<T>` is a weak handle. It has `read_with`, `update`, and `update_in` methods that work the same, but always return an `anyhow::Result` so that they can fail if the entity no longer exists. This can be useful to avoid memory leaks - if entities have mutually recursive handles to each other they will never be dropped.

### Concurrency

All use of entities and UI rendering occurs on a single foreground thread.

`cx.spawn(async move |cx| ...)` runs an async closure on the foreground thread. Within the closure, `cx` is an async context like `AsyncApp` or `AsyncWindowContext`.

When the outer cx is a `Context<T>`, the use of `spawn` instead looks like `cx.spawn(async move |handle, cx| ...)`, where `handle: WeakEntity<T>`.

To do work on other threads, `cx.background_spawn(async move { ... })` is used. Often this background task is awaited on by a foreground task which uses the results to update state.

Both `cx.spawn` and `cx.background_spawn` return a `Task<R>`, which is a future that can be awaited upon. If this task is dropped, then its work is cancelled. To prevent this one of the following must be done:

- Awaiting the task in some other async context.
- Detaching the task via `task.detach()` or `task.detach_and_log_err(cx)`, allowing it to run indefinitely.
- Storing the task in a field, if the work should be halted when the struct is dropped.

A task which doesn't do anything but provide a value can be created with `Task::ready(value)`.

### Elements

The `Render` trait is used to render some state into an element tree that is laid out using flexbox layout. An `Entity<T>` where `T` implements `Render` is sometimes called a "view".

Example:

```
struct TextWithBorder(SharedString);

impl Render for TextWithBorder {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().border_1().child(self.0.clone())
    }
}
```

Since `impl IntoElement for SharedString` exists, it can be used as an argument to `child`. `SharedString` is used to avoid copying strings, and is either an `&'static str` or `Arc<str>`.

UI components that are constructed just to be turned into elements can instead implement the `RenderOnce` trait, which is similar to `Render`, but its `render` method takes ownership of `self`. Types that implement this trait can use `#[derive(IntoElement)]` to use them directly as children.

The style methods on elements are similar to those used by Tailwind CSS.

If some attributes or children of an element tree are conditional, `.when(condition, |this| ...)` can be used to run the closure only when `condition` is true. Similarly, `.when_some(option, |this, value| ...)` runs the closure when the `Option` has a value.

### Input events

Input event handlers can be registered on an element via methods like `.on_click(|event, window, cx: &mut App| ...)`.

Often event handlers will want to update the entity that's in the current `Context<T>`. The `cx.listener` method provides this - its use looks like `.on_click(cx.listener(|this: &mut T, event, window, cx: &mut Context<T>| ...)`.

### Actions

Actions are dispatched via user keyboard interaction or in code via `window.dispatch_action(SomeAction.boxed_clone(), cx)` or `focus_handle.dispatch_action(&SomeAction, window, cx)`.

Actions with no data defined with the `actions!(some_namespace, [SomeAction, AnotherAction])` macro call. Otherwise the `Action` derive macro is used. Doc comments on actions are displayed to the user.

Action handlers can be registered on an element via the event handler `.on_action(|action, window, cx| ...)`. Like other event handlers, this is often used with `cx.listener`.

### Notify

When a view's state has changed in a way that may affect its rendering, it should call `cx.notify()`. This will cause the view to be rerendered. It will also cause any observe callbacks registered for the entity with `cx.observe` to be called.

### Entity events

While updating an entity (`cx: Context<T>`), it can emit an event using `cx.emit(event)`. Entities register which events they can emit by declaring `impl EventEmittor<EventType> for EntityType {}`.

Other entities can then register a callback to handle these events by doing `cx.subscribe(other_entity, |this, other_entity, event, cx| ...)`. This will return a `Subscription` which deregisters the callback when dropped. Typically `cx.subscribe` happens when creating a new entity and the subscriptions are stored in a `_subscriptions: Vec<Subscription>` field.
