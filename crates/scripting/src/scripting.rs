//! Embedded JavaScript engine backing "script" tabs.
//!
//! A script tab runs a plain JavaScript program in QuickJS with two globals
//! injected: `console` (log/warn/error) and `db`, a driver-agnostic database
//! handle that executes through the app's [`DatabaseServiceTrait`]. The `db`
//! API is deliberately *synchronous*: workflows that can't be a single SQL
//! script (drop N template databases, fetch rows and write them back one at a
//! time) are inherently sequential, and a synchronous API makes accidentally
//! launching a fan-out of parallel queries impossible.
//!
//! Synchronous host functions on top of async drivers mean blocking, so each
//! run gets a dedicated `std::thread`. QuickJS's `Runtime`/`Context` are
//! `!Send`, and blocking a tokio worker or GPUI's foreground thread would
//! deadlock, so this thread must stay a plain OS thread: it is created here,
//! never joined by the UI, and talks to the app only over a channel.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use blanco_core::{ColumnType, QueryResult};
use database::DatabaseServiceTrait;
use rquickjs::function::{Opt, Rest};
use rquickjs::object::Property;
use rquickjs::{CatchResultExt, Context, Ctx, Exception, Function, Object, Runtime, Value};
use serde_json::{Map, Value as Json};
use smol::channel::{Receiver, Sender, unbounded};

/// Memory ceiling for a script's QuickJS heap. Allocating past it raises a
/// catchable JS error instead of aborting the process, which matters because
/// `db.query` materializes whole result sets into JS values.
const SCRIPT_MEMORY_LIMIT_BYTES: usize = 256 * 1024 * 1024;
const SCRIPT_STACK_SIZE_BYTES: usize = 1024 * 1024;

/// How often a blocked host call re-checks the cancel flag while waiting on the
/// database. Also bounds how long "Stop" takes to abandon an in-flight query.
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Ties a JS result object back to the raw [`QueryResult`] it came from, so
/// `db.display(result)` can render real column types instead of re-deriving
/// them from JSON. Defined non-enumerable so it stays out of `JSON.stringify`,
/// `Object.keys` and `for...in`.
const RESULT_ID_PROPERTY: &str = "__blancoResultId";

/// Reported when the user presses Stop. Not something a script can catch:
/// cancellation unwinds the whole run via the interrupt handler.
pub const CANCELLED_MESSAGE: &str = "cancelled by user";

/// The file name QuickJS attributes evaluated source to, and therefore the
/// prefix its stack frames carry. Used to recover line numbers for editor
/// diagnostics.
const EVAL_FILE_NAME: &str = "eval_script";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleLevel {
    Log,
    Warn,
    Error,
}

#[derive(Debug)]
pub enum ScriptEvent {
    Console {
        level: ConsoleLevel,
        text: String,
    },
    /// An explicit `db.display(value)` call: render this in the results grid.
    Display(Box<QueryResult>),
    /// Terminal event. `Err` carries a user-facing message (JS exception with
    /// stack, engine setup failure, or [`CANCELLED_MESSAGE`]).
    Finished(Result<(), String>),
}

/// Handle to a running script. Dropping the receiver does not stop the script;
/// raise `cancel` for that.
pub struct ScriptJob {
    pub cancel: Arc<AtomicBool>,
    pub events: Receiver<ScriptEvent>,
}

/// Start `source` on a fresh script thread. The returned receiver yields
/// console/display events as the script runs and always ends with exactly one
/// [`ScriptEvent::Finished`], including when engine setup itself fails.
pub fn spawn_script(
    source: String,
    db: Arc<dyn DatabaseServiceTrait>,
    connection_id: i64,
    database_name: String,
    tokio: tokio::runtime::Handle,
) -> ScriptJob {
    let cancel = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = unbounded();

    let spawned = std::thread::Builder::new()
        .name("blanco-script".into())
        .spawn({
            let cancel = cancel.clone();
            let sender = sender.clone();
            move || {
                let host = ScriptHost {
                    db,
                    connection_id,
                    database_name,
                    tokio,
                    cancel,
                    events: sender.clone(),
                    results: RefCell::new(Vec::new()),
                };
                let outcome = run_script(&source, Rc::new(host));
                // A gone receiver means the tab stopped listening; the script
                // has already finished, so dropping the terminal event is
                // exactly right.
                sender.send_blocking(ScriptEvent::Finished(outcome)).ok();
            }
        });

    if let Err(error) = spawned {
        sender
            .send_blocking(ScriptEvent::Finished(Err(format!(
                "failed to start script thread: {error}"
            ))))
            .ok();
    }

    ScriptJob {
        cancel,
        events: receiver,
    }
}

/// Extract the 1-based source line a QuickJS error message/stack points at, so
/// the editor can put a squiggle on it. Returns `None` when the error carries
/// no frame from the evaluated script (e.g. a host-thrown message).
pub fn error_source_line(message: &str) -> Option<u32> {
    message.split(EVAL_FILE_NAME).skip(1).find_map(|rest| {
        let digits: String = rest
            .strip_prefix(':')?
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse().ok()
    })
}

fn run_script(source: &str, host: Rc<ScriptHost>) -> Result<(), String> {
    let runtime = Runtime::new().map_err(|error| format!("failed to start JS runtime: {error}"))?;
    runtime.set_memory_limit(SCRIPT_MEMORY_LIMIT_BYTES);
    runtime.set_max_stack_size(SCRIPT_STACK_SIZE_BYTES);
    // Aborts even a `while (true) {}` that never reaches a host call.
    runtime.set_interrupt_handler(Some(Box::new({
        let cancel = host.cancel.clone();
        move || cancel.load(Ordering::Relaxed)
    })));

    let context =
        Context::full(&runtime).map_err(|error| format!("failed to create JS context: {error}"))?;

    let result = context.with(|ctx| {
        install_globals(&ctx, &host)
            .catch(&ctx)
            .map_err(|error| error.to_string())?;
        ctx.eval::<(), _>(source)
            .catch(&ctx)
            .map_err(|error| error.to_string())
    });

    // The interrupt handler surfaces as an ordinary evaluation failure; report
    // it as a cancel rather than as a script error.
    if host.cancel.load(Ordering::Relaxed) {
        return Err(CANCELLED_MESSAGE.to_string());
    }
    result
}

/// Everything the host functions need, shared by `Rc` within the script thread.
struct ScriptHost {
    db: Arc<dyn DatabaseServiceTrait>,
    connection_id: i64,
    database_name: String,
    tokio: tokio::runtime::Handle,
    cancel: Arc<AtomicBool>,
    events: Sender<ScriptEvent>,
    /// Raw results handed out to JS, indexed by [`RESULT_ID_PROPERTY`].
    results: RefCell<Vec<QueryResult>>,
}

/// Marker for "the user pressed Stop while this call was waiting".
struct Cancelled;

impl ScriptHost {
    fn emit(&self, event: ScriptEvent) {
        self.events.send_blocking(event).ok();
    }

    /// Run `future` on the app's tokio runtime, abandoning it if the cancel
    /// flag is raised while it is in flight.
    ///
    /// Sound only because the caller is a plain OS thread: `block_on` from a
    /// runtime worker or from GPUI's foreground thread would deadlock.
    fn block_on<T>(&self, future: impl Future<Output = T>) -> Result<T, Cancelled> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(Cancelled);
        }
        self.tokio.block_on(async {
            tokio::pin!(future);
            loop {
                tokio::select! {
                    biased;
                    output = &mut future => return Ok(output),
                    _ = tokio::time::sleep(CANCEL_POLL_INTERVAL) => {
                        if self.cancel.load(Ordering::Relaxed) {
                            return Err(Cancelled);
                        }
                    }
                }
            }
        })
    }

    fn run_query(&self, sql: &str, parameters: &[String]) -> Result<QueryResult, HostError> {
        let database = Some(self.database_name.as_str());
        let result = if parameters.is_empty() {
            self.block_on(self.db.execute_query(self.connection_id, database, sql))
        } else {
            self.block_on(self.db.execute_query_with_params(
                self.connection_id,
                database,
                sql,
                parameters,
            ))
        }?;
        result.map_err(|error| HostError::Failed(format!("{error:#}")))
    }
}

/// A host call that could not complete. `Cancelled` unwinds the run; `Failed`
/// becomes a JS exception the script may catch.
enum HostError {
    Cancelled,
    Failed(String),
}

impl From<Cancelled> for HostError {
    fn from(_: Cancelled) -> Self {
        HostError::Cancelled
    }
}

impl HostError {
    fn into_js(self, ctx: &Ctx<'_>) -> rquickjs::Error {
        match self {
            HostError::Cancelled => Exception::throw_internal(ctx, CANCELLED_MESSAGE),
            HostError::Failed(message) => Exception::throw_message(ctx, &message),
        }
    }
}

fn install_globals<'js>(ctx: &Ctx<'js>, host: &Rc<ScriptHost>) -> rquickjs::Result<()> {
    let globals = ctx.globals();

    let console = Object::new(ctx.clone())?;
    for (name, level) in [
        ("log", ConsoleLevel::Log),
        ("warn", ConsoleLevel::Warn),
        ("error", ConsoleLevel::Error),
    ] {
        let host = host.clone();
        console.set(
            name,
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, args: Rest<Value<'js>>| {
                let text = args
                    .0
                    .iter()
                    .map(|value| format_console_argument(&ctx, value))
                    .collect::<Vec<_>>()
                    .join(" ");
                host.emit(ScriptEvent::Console { level, text });
            })?,
        )?;
    }
    globals.set("console", console)?;

    let db = Object::new(ctx.clone())?;

    db.set("query", {
        let host = host.clone();
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>,
                  sql: String,
                  params: Opt<Vec<Value<'js>>>|
                  -> rquickjs::Result<Value<'js>> {
                let parameters = bind_parameters(&ctx, params.0)?;
                let result = host
                    .run_query(&sql, &parameters)
                    .map_err(|error| error.into_js(&ctx))?;
                result_to_js(&ctx, &host, result)
            },
        )?
    })?;

    db.set("execute", {
        let host = host.clone();
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>,
                  sql: String,
                  params: Opt<Vec<Value<'js>>>|
                  -> rquickjs::Result<f64> {
                let parameters = bind_parameters(&ctx, params.0)?;
                let result = host
                    .run_query(&sql, &parameters)
                    .map_err(|error| error.into_js(&ctx))?;
                Ok(result.rows_affected as f64)
            },
        )?
    })?;

    db.set("transaction", {
        let host = host.clone();
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, statements: Vec<String>| -> rquickjs::Result<Object<'js>> {
                let outcome = host
                    .block_on(host.db.execute_operations_transactional(
                        host.connection_id,
                        Some(host.database_name.as_str()),
                        &statements,
                    ))
                    .map_err(|_| HostError::Cancelled.into_js(&ctx))?;

                match outcome {
                    Ok(outcome) => {
                        let object = Object::new(ctx.clone())?;
                        object.set("rowsAffected", outcome.rows_affected as f64)?;
                        object.set("operationsExecuted", outcome.operations_executed as f64)?;
                        Ok(object)
                    }
                    Err(failure) => {
                        // `applied` matters to the script author: on backends
                        // without transactions a mid-batch failure leaves the
                        // leading statements committed.
                        let message = format!(
                            "{:#} ({} of {} statements applied)",
                            failure.error,
                            failure.applied,
                            statements.len()
                        );
                        Err(HostError::Failed(message).into_js(&ctx))
                    }
                }
            },
        )?
    })?;

    db.set("display", {
        let host = host.clone();
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, value: Value<'js>| -> rquickjs::Result<()> {
                let result = match stored_result_id(&value) {
                    Some(id) => host.results.borrow().get(id).cloned(),
                    None => None,
                };
                let result = match result {
                    Some(result) => result,
                    None => synthesize_result(&ctx, &value)?,
                };
                host.emit(ScriptEvent::Display(Box::new(result)));
                Ok(())
            },
        )?
    })?;

    globals.set("db", db)?;

    Ok(())
}

/// Format one `console.*` argument. Strings print bare (so log lines read like
/// log lines); everything else prints as JSON.
fn format_console_argument<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> String {
    if let Some(text) = value.as_string() {
        return text.to_string().unwrap_or_else(|_| "<string>".to_string());
    }
    if value.is_undefined() {
        return "undefined".to_string();
    }
    match ctx.json_stringify(value.clone()) {
        Ok(Some(json)) => json.to_string().unwrap_or_else(|_| "<value>".to_string()),
        // `undefined`, functions and symbols stringify to `None`.
        Ok(None) => "undefined".to_string(),
        Err(_) => "<unserializable>".to_string(),
    }
}

/// Coerce JS bind parameters to the `String` form the drivers accept.
///
/// The driver-level API has no typed parameter representation, so this is a
/// deliberate lossy narrowing: numbers and booleans print, objects/arrays go
/// through JSON, and `null`/`undefined` are rejected because there is no way to
/// express a SQL NULL bind through it.
fn bind_parameters<'js>(
    ctx: &Ctx<'js>,
    params: Option<Vec<Value<'js>>>,
) -> rquickjs::Result<Vec<String>> {
    let Some(params) = params else {
        return Ok(Vec::new());
    };

    params
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            if value.is_null() || value.is_undefined() {
                return Err(Exception::throw_type(
                    ctx,
                    &format!(
                        "parameter {} is null/undefined; bind parameters cannot express SQL NULL, \
                         inline it in the statement instead",
                        index + 1
                    ),
                ));
            }
            if let Some(text) = value.as_string() {
                return text.to_string();
            }
            if let Some(number) = value.as_int() {
                return Ok(number.to_string());
            }
            if let Some(number) = value.as_float() {
                return Ok(number.to_string());
            }
            if let Some(flag) = value.as_bool() {
                return Ok(flag.to_string());
            }
            match ctx.json_stringify(value)? {
                Some(json) => json.to_string(),
                None => Err(Exception::throw_type(
                    ctx,
                    &format!("parameter {} cannot be used as a bind value", index + 1),
                )),
            }
        })
        .collect()
}

/// Convert a [`QueryResult`] into the JS shape scripts see, keeping the raw
/// result on the side so `db.display` can render it with its real column types.
fn result_to_js<'js>(
    ctx: &Ctx<'js>,
    host: &Rc<ScriptHost>,
    result: QueryResult,
) -> rquickjs::Result<Value<'js>> {
    let rows: Vec<Json> = result
        .rows
        .iter()
        .map(|row| {
            // Duplicate column names collapse here (last wins); the `columns`
            // array preserves the full, ordered list.
            let mut object = Map::new();
            for (column, cell) in result.columns.iter().zip(row) {
                object.insert(
                    column.clone(),
                    cell.clone().map(Json::String).unwrap_or(Json::Null),
                );
            }
            Json::Object(object)
        })
        .collect();

    let payload = serde_json::json!({
        "columns": result.columns,
        "rows": rows,
        "rowsAffected": result.rows_affected,
        "executionTimeMs": result.execution_time_ms,
    });

    let value = ctx.json_parse(payload.to_string())?;

    if let Some(object) = value.as_object() {
        let id = {
            let mut results = host.results.borrow_mut();
            results.push(result);
            results.len() - 1
        };
        object.prop(RESULT_ID_PROPERTY, Property::from(id as f64))?;
    }

    Ok(value)
}

/// Read back the side-table index stamped by [`result_to_js`].
fn stored_result_id(value: &Value<'_>) -> Option<usize> {
    let object = value.as_object()?;
    let id: f64 = object.get(RESULT_ID_PROPERTY).ok()?;
    (id >= 0.0).then_some(id as usize)
}

/// Build a displayable result from an arbitrary JS value (anything that didn't
/// come straight out of `db.query`).
fn synthesize_result<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> rquickjs::Result<QueryResult> {
    let json: Json = match ctx.json_stringify(value.clone())? {
        Some(text) => serde_json::from_str(&text.to_string()?).map_err(|error| {
            Exception::throw_type(ctx, &format!("cannot display value: {error}"))
        })?,
        None => Json::Null,
    };
    Ok(json_to_result(json))
}

fn json_to_result(json: Json) -> QueryResult {
    let (columns, rows) = match json {
        Json::Array(items) => {
            // Union of keys across all rows, in first-seen order, so rows with
            // missing keys still line up under the right columns.
            let mut columns: Vec<String> = Vec::new();
            for item in &items {
                if let Json::Object(object) = item {
                    for key in object.keys() {
                        if !columns.iter().any(|existing| existing == key) {
                            columns.push(key.clone());
                        }
                    }
                }
            }

            if columns.is_empty() {
                let rows = items
                    .into_iter()
                    .map(|item| vec![json_to_cell(item)])
                    .collect();
                (vec!["value".to_string()], rows)
            } else {
                let rows = items
                    .into_iter()
                    .map(|item| match item {
                        Json::Object(mut object) => columns
                            .iter()
                            .map(|column| json_to_cell(object.remove(column).unwrap_or(Json::Null)))
                            .collect(),
                        // A scalar mixed into an array of objects lands under
                        // the first column rather than being dropped.
                        other => {
                            let mut row = vec![None; columns.len()];
                            if let Some(first) = row.first_mut() {
                                *first = json_to_cell(other);
                            }
                            row
                        }
                    })
                    .collect();
                (columns, rows)
            }
        }
        Json::Object(object) => {
            let columns: Vec<String> = object.keys().cloned().collect();
            let row: Vec<Option<String>> =
                object.into_iter().map(|(_, v)| json_to_cell(v)).collect();
            (columns, vec![row])
        }
        Json::Null => (Vec::new(), Vec::new()),
        scalar => (vec!["value".to_string()], vec![vec![json_to_cell(scalar)]]),
    };

    let row_count = rows.len();
    QueryResult {
        column_types: vec![ColumnType::Text; columns.len()],
        columns,
        rows,
        rows_affected: row_count as u64,
        query_text: None,
        execution_time_ms: None,
        is_error: false,
        table_name: None,
        connection_id: None,
        table_columns: None,
    }
}

fn json_to_cell(value: Json) -> Option<String> {
    match value {
        Json::Null => None,
        Json::String(text) => Some(text),
        other => Some(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use database::{ConnectionConfig, DatabaseService};

    struct TestEnvironment {
        _runtime: tokio::runtime::Runtime,
        handle: tokio::runtime::Handle,
        db: Arc<dyn DatabaseServiceTrait>,
        connection_id: i64,
        _temp_dir: tempfile::TempDir,
    }

    impl TestEnvironment {
        fn new() -> Self {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("failed to build tokio runtime");
            let handle = runtime.handle().clone();

            let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
            let db_path = temp_dir.path().join("scripting.db");
            let connection_id = 1;
            let config = ConnectionConfig::new_sqlite(
                connection_id,
                "test".into(),
                db_path.to_string_lossy().to_string(),
            );

            let service = DatabaseService::new(handle.clone());
            handle.block_on(service.add_connection_config(config));

            Self {
                _runtime: runtime,
                handle,
                db: Arc::new(service),
                connection_id,
                _temp_dir: temp_dir,
            }
        }

        fn run(&self, source: &str) -> (Vec<ScriptEvent>, Arc<AtomicBool>) {
            let job = spawn_script(
                source.to_string(),
                self.db.clone(),
                self.connection_id,
                "main".to_string(),
                self.handle.clone(),
            );
            let cancel = job.cancel.clone();
            let mut events = Vec::new();
            while let Ok(event) = job.events.recv_blocking() {
                let terminal = matches!(event, ScriptEvent::Finished(_));
                events.push(event);
                if terminal {
                    break;
                }
            }
            (events, cancel)
        }
    }

    fn console_lines(events: &[ScriptEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                ScriptEvent::Console { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn finished(events: &[ScriptEvent]) -> Result<(), String> {
        events
            .iter()
            .find_map(|event| match event {
                ScriptEvent::Finished(outcome) => Some(outcome.clone()),
                _ => None,
            })
            .expect("script should always report a terminal event")
    }

    fn displays(events: &[ScriptEvent]) -> Vec<&QueryResult> {
        events
            .iter()
            .filter_map(|event| match event {
                ScriptEvent::Display(result) => Some(result.as_ref()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn runs_a_query_loop_and_displays_results() {
        let environment = TestEnvironment::new();
        let (events, _) = environment.run(
            r#"
            db.execute("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT)");
            for (let i = 1; i <= 5; i++) {
                db.execute("INSERT INTO items (id, name) VALUES (?, ?)", [i, "item-" + i]);
            }
            const counted = db.query("SELECT COUNT(*) AS total FROM items");
            console.log("inserted", counted.rows[0].total, "rows");
            db.display(db.query("SELECT id, name FROM items ORDER BY id"));
            "#,
        );

        assert_eq!(finished(&events), Ok(()));
        assert_eq!(console_lines(&events), vec!["inserted 5 rows"]);

        let displayed = displays(&events);
        assert_eq!(displayed.len(), 1);
        assert_eq!(displayed[0].columns, vec!["id", "name"]);
        assert_eq!(displayed[0].rows.len(), 5);
        assert_eq!(displayed[0].rows[0][1], Some("item-1".to_string()));
    }

    #[test]
    fn database_errors_become_catchable_exceptions() {
        let environment = TestEnvironment::new();
        let (events, _) = environment.run(
            r#"
            try {
                db.query("SELECT * FROM table_that_does_not_exist");
                console.log("unreachable");
            } catch (error) {
                console.warn("caught:", String(error.message).length > 0);
            }
            console.log("still running");
            "#,
        );

        assert_eq!(finished(&events), Ok(()));
        assert_eq!(
            console_lines(&events),
            vec!["caught: true", "still running"]
        );
    }

    #[test]
    fn uncaught_exceptions_finish_with_an_error() {
        let environment = TestEnvironment::new();
        let (events, _) = environment.run("throw new Error('boom');");

        let error = finished(&events).expect_err("script should fail");
        assert!(error.contains("boom"), "unexpected error: {error}");
    }

    #[test]
    fn syntax_errors_report_their_source_line() {
        let environment = TestEnvironment::new();
        let (events, _) = environment.run("const a = 1;\nconst b = ;\n");

        let error = finished(&events).expect_err("script should fail to compile");
        assert_eq!(error_source_line(&error), Some(2), "error was: {error}");
    }

    #[test]
    fn display_synthesizes_a_result_from_plain_values() {
        let environment = TestEnvironment::new();
        let (events, _) =
            environment.run(r#"db.display([{ name: "a", size: 1 }, { name: "b", size: null }]);"#);

        assert_eq!(finished(&events), Ok(()));
        let displayed = displays(&events);
        assert_eq!(displayed.len(), 1);
        assert_eq!(displayed[0].columns, vec!["name", "size"]);
        assert_eq!(
            displayed[0].rows,
            vec![
                vec![Some("a".to_string()), Some("1".to_string())],
                vec![Some("b".to_string()), None],
            ]
        );
    }

    #[test]
    fn cancellation_interrupts_a_runaway_loop() {
        let environment = TestEnvironment::new();
        let job = spawn_script(
            "while (true) {}".to_string(),
            environment.db.clone(),
            environment.connection_id,
            "main".to_string(),
            environment.handle.clone(),
        );

        std::thread::sleep(Duration::from_millis(50));
        job.cancel.store(true, Ordering::Relaxed);

        let event = job
            .events
            .recv_blocking()
            .expect("script should report a terminal event");
        match event {
            ScriptEvent::Finished(Err(message)) => assert_eq!(message, CANCELLED_MESSAGE),
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
