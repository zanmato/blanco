# Blanco SQL Editor - Development Notes

## Project Overview

Blanco is a cross-platform SQL editor built with GPUI and gpui-component. It provides a modern, native UI for executing SQL queries against various databases.

## Getting Started

The SQL grammar lives in the `crates/tree-sitter-sequel` git submodule, tracking the `main` branch of upstream `derekstride/tree-sitter-sql` (the crates.io release is far behind and missing grammar we rely on). Upstream gitignores the generated `src/parser.c` and `src/tree_sitter/*.h`, so they are produced locally and never committed. After cloning, run:

```bash
./scripts/setup.sh   # inits the submodule and runs `tree-sitter generate`
```

This requires the tree-sitter CLI (`cargo install tree-sitter-cli`). The blanco workspace patches `tree-sitter-sequel` to this submodule via `[patch.crates-io]` in the root `Cargo.toml`. To pull newer grammar later: `git submodule update --remote crates/tree-sitter-sequel && ./scripts/setup.sh`.

`scripts/setup.sh` also generates the in-tree `crates/tree-sitter-redis` grammar, which highlights the editor when the active connection is a Redis (non-SQL) backend. Unlike sequel it is not a submodule (we author it), but its generated `src/parser.c` + `src/tree_sitter/*.h` are likewise gitignored and produced by `tree-sitter generate`. The command/subcommand catalog is codegenned from a live server (`COMMAND LIST` / `COMMAND DOCS`) into two committed artifacts: `grammar/keywords.js` (consumed by the grammar) and `bindings/rust/commands.rs` (exposed as `tree_sitter_redis::commands` + `tree_sitter_redis::subcommands_for`, consumed by the editor's Redis completion provider so highlighting and completion share one command set). Regenerate both after a Redis upgrade with `python3 crates/tree-sitter-redis/scripts/extract-redis-commands.py` (defaults to the docker-compose dev server on `127.0.0.1:6400`). The editor language is chosen per connection by `DatabaseType::editor_language()` (`"sql"` vs `"redis"`); both grammars are registered at startup in `main.rs` (`sql::register_languages` + `redis_syntax::register_language`). Redis tabs get `redis_completion::RedisCompletionProvider` (command/subcommand completion) instead of the SQL completion/selection-range/linting machinery, which is gated behind `supports_sql()`.

## Architecture

### File Structure

- `main.rs` - GPUI application initialization, window setup
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
