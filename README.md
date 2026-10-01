# Blanco

Blanco is a cross platform SQL editor and database client built with
[GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) and
[gpui-kit](https://github.com/longbridge/gpui-kit).

Supported backends: PostgreSQL, MySQL/MariaDB, SQLite, SQL Server, ClickHouse
and Redis.

## Highlights

- Schema aware completion, formatting and linting for SQL, plus command
  completion and highlighting for Redis.
- Statement at cursor and selection execution, bind parameter prompts, query
  history and snippets.
- Editable results grid with atomic commits, row filter, copy as
  CSV/TSV/JSON/SQL/Markdown, export to CSV/TSV/JSON/SQL/Markdown, charts and
  foreign key peeking.
- EXPLAIN results rendered as a plan tree (PostgreSQL, MySQL, SQLite) with
  time bars for hot nodes.
- Sidebar schema tree with filter, rename/truncate/drop from the tree, table
  structure view, object DDL and an ER diagram.
- CSV and JSON (array or NDJSON) import with column mapping and conflict
  strategies, applied as one transaction.
- Read-only connections, and a confirmation before writes on connections
  tagged PROD.
- JavaScript scripting (`db.query`, `db.execute`, `db.transaction`,
  `db.display`) running in a sandboxed QuickJS runtime.
- SSH tunnels with `known_hosts` verification, TLS, credentials in the OS
  keychain.

## Installing

Prebuilt binaries for Linux (x86_64, plus a `.deb`), macOS (Apple silicon) and
Windows (x86_64) are attached to each
[release](https://github.com/zanmato/blanco/releases).

## Building

Requirements: a recent stable Rust toolchain and the tree-sitter CLI
(`cargo install tree-sitter-cli`). On Linux you also need the usual GPUI build
dependencies (X11/Wayland, Vulkan, fontconfig).

```bash
./scripts/setup.sh        # init the grammar submodule and generate parsers
cargo run --release -p blanco
```

## Layout

- `crates/blanco`: the GPUI application.
- `crates/blanco-core`: the `Connection` trait, `DatabaseType`, shared value
  types, the read/write statement classifier, DDL builders and plan parsing.
- `crates/database`: `DatabaseService`, connection configs, SSH tunnels and the
  tokio bridge every connection is wrapped in.
- `crates/{postgres,mysql,sqlite,mssql,clickhouse,redis}`: drivers.
- `crates/app-database`: the SQLite app store (connections, tabs, history,
  snippets, settings) with versioned migrations.
- `crates/sql-parser`: tree-sitter statement parsing and EXPLAIN wrapping.
- `crates/transformers`: CSV/TSV/JSON/SQL/VALUES/Markdown formatters.
- `crates/scripting`: the QuickJS script host.
- `crates/ui`: generic GPUI widgets (tree, tabs, graph view).

## Testing

```bash
./scripts/check.sh        # fmt, clippy (deny warnings) and the test suite
./scripts/check.sh --db   # also require the docker databases
```

Database backed tests skip when their server is unreachable unless
`BLANCO_RUN_DB_TESTS=1` is set. Start the test servers with `docker compose up -d`
(MariaDB, PostgreSQL with TLS, ClickHouse, SQL Server, Redis and an SSH bastion).
The SSH cases need the bastion host key trusted once:

```bash
ssh-keyscan -p 2222 127.0.0.1 >> ~/.ssh/known_hosts
```

Install the check script as a pre push hook with
`ln -sf ../../scripts/check.sh .git/hooks/pre-push`.

## Website

The site at <https://zanmato.github.io/blanco> is built from `site/` with
`scripts/build-site` and published by the release workflow.

Its screenshots in `docs/images/screenshots` come from
`scripts/screenshots/capture`. The script builds Blanco with the `screenshots`
feature and runs it on a virtual X display against a throwaway data directory
and a generated demo database, so nothing from your own profile or keychain
shows up. It needs Xvfb, ImageMagick, sqlite3, python3 and Mesa's lavapipe
Vulkan driver.

## Logs and data

- Settings, connections, history and snippets: `$XDG_DATA_HOME/blanco/blanco.db`
- Logs (also written to stderr): `$XDG_STATE_HOME/blanco/blanco.log`
- Secrets (passwords, SSH passphrases, API keys): the OS keychain

## License

Apache 2.0, see [LICENSE](LICENSE).
