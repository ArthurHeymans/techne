//! SQLite's OS/database boundary; schema, migrations and search belong in Scheme.
//!
//! Each connection owns a blocking worker, never the editor/VM thread. Requests
//! are serialized and bounded; cancelling a wait sets a flag checked by SQLite's
//! progress handler. Each statement has a native-owned transaction, rolled back
//! on failure/cancellation, and callers cannot issue transaction control SQL.
//! Cancellation racing a completed commit does NOT prove the write did not land.
//! Dropping/closing a handle interrupts its worker and releases the connection.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use rusqlite::{
    Connection, InterruptHandle, OpenFlags,
    hooks::{AuthAction, AuthContext, Authorization},
    limits::Limit,
    params_from_iter,
    types::{Value as SqlValue, ValueRef},
};
use techne_vm::{
    api::{Arg, Args, Bytes, Foreign, IntoValue, Value},
    vm::{Capability, Error, ROOT_MODULE, Vm},
};
use tokio::{
    runtime::Handle,
    sync::{Mutex, OwnedMutexGuard, Semaphore, oneshot},
};

const SQL_BYTES: usize = 64 * 1024;
// Mail MIME sources and attachments routinely exceed a megabyte. These
// are hard upper bounds, not preallocated buffers; callers still page rows.
const INPUT_BYTES: usize = 32 * 1024 * 1024;
const RESULT_BYTES: usize = 40 * 1024 * 1024;
const ROWS: usize = 4096;
const PARAMS: usize = 256;
const COLUMNS: usize = 128;
const TIMEOUT: Duration = Duration::from_secs(5);
// A process-wide bound: persistent workers must not exhaust the blocking pool.
static CONNECTIONS: Semaphore = Semaphore::const_new(32);

/// Shared runtime for database workers (at most 32 connections process-wide).
/// No database work runs on its async executor thread.
pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread().worker_threads(1).thread_name("techne-store").build().expect("SQLite runtime")
    })
}

struct Database {
    sender: RefCell<Option<mpsc::SyncSender<Job>>>,
    gate: Arc<Mutex<()>>,
    interrupt: Arc<InterruptHandle>,
    closed: Arc<AtomicBool>,
}

impl Database {
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.interrupt.interrupt();
        self.sender.borrow_mut().take();
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        self.close();
    }
}

/// Dropped with the VM future, including on task-kill. No interrupt-handle race
/// with the next request: cancellation is specific to this job's progress flag.
struct Cancel(Arc<AtomicBool>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct Job {
    sql: String,
    params: Vec<SqlValue>,
    query: bool,
    cancel: Arc<AtomicBool>,
    reply: oneshot::Sender<Result<Reply, String>>,
    // Kept by the worker, not the future: cancellation cannot enqueue unbounded
    // requests behind a statement which has not stopped yet.
    _gate: OwnedMutexGuard<()>,
}

enum Reply {
    Count(usize),
    Rows(Vec<Vec<Cell>>),
}
struct Cell(SqlValue);

impl IntoValue for Cell {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        match self.0 {
            SqlValue::Null => false.into_value(vm),
            SqlValue::Integer(i) => i.into_value(vm),
            SqlValue::Real(f) => f.into_value(vm),
            SqlValue::Text(s) => s.into_value(vm),
            SqlValue::Blob(b) => Bytes(b).into_value(vm),
        }
    }
}
impl IntoValue for Reply {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        match self {
            Self::Count(n) => n.into_value(vm),
            Self::Rows(rows) => rows.into_value(vm),
        }
    }
}

fn bounded_text(v: Arg<'_>, max: usize, who: &str) -> Result<String, Error> {
    let s = v.as_str().ok_or_else(|| v.type_error_for(who, "string"))?;
    if s.len() > max || s.contains('\0') {
        return Err(Error::new(format!("{who}: text exceeds {max} bytes or contains NUL")));
    }
    Ok(s.to_owned())
}

fn parameters(v: Arg<'_>) -> Result<Vec<SqlValue>, Error> {
    let items = v.items().ok_or_else(|| v.type_error_for("sqlite", "parameter list or vector"))?;
    if items.len() > PARAMS {
        return Err(Error::new("sqlite: at most 256 parameters"));
    }
    let mut bytes = 0;
    items
        .into_iter()
        .map(|v| {
            let value = if v.is_false() {
                SqlValue::Null
            } else if let Some(s) = v.as_str() {
                bytes += s.len();
                if bytes > INPUT_BYTES {
                    return Err(Error::new("sqlite: parameter bytes exceed 32 MiB"));
                }
                SqlValue::Text(s.to_owned())
            } else if let Some(b) = v.as_bytes() {
                bytes += b.len();
                if bytes > INPUT_BYTES {
                    return Err(Error::new("sqlite: parameter bytes exceed 32 MiB"));
                }
                SqlValue::Blob(b.to_vec())
            } else if let Ok(i) = v.integer() {
                SqlValue::Integer(i)
            } else if v.kind().is_none()
                && let Ok(f) = v.real()
                && f.is_finite()
            {
                SqlValue::Real(f)
            } else {
                return Err(v.type_error_for("sqlite", "#f, int64, finite flonum, string or bytevector"));
            };
            Ok(value)
        })
        .collect()
}

fn authorize(ctx: AuthContext<'_>) -> Authorization {
    match ctx.action {
        AuthAction::Transaction { .. }
        | AuthAction::Savepoint { .. }
        | AuthAction::Attach { .. }
        | AuthAction::Detach { .. }
        | AuthAction::Unknown { .. } => Authorization::Deny,
        AuthAction::Pragma { pragma_name, pragma_value } => {
            if pragma_value.is_none() && ["compile_options", "data_version"].iter().any(|s| pragma_name.eq_ignore_ascii_case(s)) {
                Authorization::Allow
            } else {
                Authorization::Deny
            }
        }
        AuthAction::Function { function_name } if function_name.eq_ignore_ascii_case("load_extension") => Authorization::Deny,
        AuthAction::CreateVtable { module_name, .. } if !module_name.eq_ignore_ascii_case("fts5") => Authorization::Deny,
        _ => Authorization::Allow,
    }
}

fn configure(path: &str) -> rusqlite::Result<Connection> {
    let c = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    c.busy_timeout(Duration::from_millis(250))?;
    c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA temp_store=MEMORY;")?;
    for (limit, n) in [
        (Limit::SQLITE_LIMIT_LENGTH, INPUT_BYTES),
        (Limit::SQLITE_LIMIT_SQL_LENGTH, SQL_BYTES),
        (Limit::SQLITE_LIMIT_COLUMN, COLUMNS),
        (Limit::SQLITE_LIMIT_VARIABLE_NUMBER, PARAMS),
        (Limit::SQLITE_LIMIT_EXPR_DEPTH, 100),
        (Limit::SQLITE_LIMIT_COMPOUND_SELECT, 32),
        (Limit::SQLITE_LIMIT_TRIGGER_DEPTH, 32),
        (Limit::SQLITE_LIMIT_ATTACHED, 0),
        (Limit::SQLITE_LIMIT_VDBE_OP, 100_000),
        (Limit::SQLITE_LIMIT_WORKER_THREADS, 0),
    ] {
        c.set_limit(limit, n as i32)?;
    }
    Ok(c)
}

fn statement(c: &Connection, job: &Job) -> Result<Reply, String> {
    let mut stmt = c.prepare(&job.sql).map_err(|e| e.to_string())?;
    if job.query {
        if !stmt.readonly() {
            return Err("sqlite-query: statement must be read-only".into());
        }
        let columns = stmt.column_count();
        let mut cursor = stmt.query(params_from_iter(&job.params)).map_err(|e| e.to_string())?;
        let mut rows = Vec::new();
        let mut bytes = 0;
        while let Some(row) = cursor.next().map_err(|e| e.to_string())? {
            if rows.len() == ROWS {
                return Err("sqlite-query: result exceeds 4096 rows; use LIMIT".into());
            }
            let mut cells = Vec::with_capacity(columns);
            bytes += 32;
            for i in 0..columns {
                let v = row.get_ref(i).map_err(|e| e.to_string())?;
                bytes += 32
                    + match v {
                        ValueRef::Text(b) | ValueRef::Blob(b) => b.len(),
                        _ => 8,
                    };
                if bytes > RESULT_BYTES {
                    return Err("sqlite-query: result exceeds 40 MiB; page the query".into());
                }
                let cell = match v {
                    ValueRef::Null => SqlValue::Null,
                    ValueRef::Integer(n) => SqlValue::Integer(n),
                    ValueRef::Real(n) => SqlValue::Real(n),
                    ValueRef::Text(b) => SqlValue::Text(std::str::from_utf8(b).map_err(|_| "sqlite-query: invalid UTF-8 text")?.to_owned()),
                    ValueRef::Blob(b) => SqlValue::Blob(b.to_vec()),
                };
                cells.push(Cell(cell));
            }
            rows.push(cells);
        }
        Ok(Reply::Rows(rows))
    } else {
        if stmt.column_count() != 0 {
            return Err("sqlite-execute: row-returning statement; use sqlite-query".into());
        }
        stmt.execute(params_from_iter(&job.params)).map(Reply::Count).map_err(|e| e.to_string())
    }
}

fn perform(c: &mut Connection, job: &Job, closed: &Arc<AtomicBool>) -> Result<Reply, String> {
    if closed.load(Ordering::SeqCst) || job.cancel.load(Ordering::SeqCst) {
        return Err("sqlite: closed or cancelled".into());
    }
    let cancel = job.cancel.clone();
    let progress_closed = closed.clone();
    let deadline = Instant::now() + TIMEOUT;
    c.progress_handler(
        1000,
        Some(move || cancel.load(Ordering::SeqCst) || progress_closed.load(Ordering::SeqCst) || Instant::now() >= deadline),
    )
    .map_err(|e| e.to_string())?;
    // The native owns BEGIN/COMMIT/ROLLBACK; the authorizer only sees caller SQL.
    let result = (|| {
        let tx = c.transaction().map_err(|e| e.to_string())?;
        tx.authorizer(Some(authorize)).map_err(|e| e.to_string())?;
        let result = statement(&tx, job);
        tx.authorizer(None::<fn(AuthContext<'_>) -> Authorization>).map_err(|e| e.to_string())?;
        // Always permit cleanup, even after progress cancellation/timeout.
        tx.progress_handler(0, None::<fn() -> bool>).map_err(|e| e.to_string())?;
        let reply = result?;
        if closed.load(Ordering::SeqCst) || job.cancel.load(Ordering::SeqCst) || job.reply.is_closed() || Instant::now() >= deadline {
            return Err("sqlite: cancelled or timed out".into());
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(reply)
    })();
    // Also reset after errors in BEGIN or hook installation.
    let _ = c.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    let _ = c.progress_handler(0, None::<fn() -> bool>);
    result
}

type Opened = (mpsc::SyncSender<Job>, Arc<InterruptHandle>, Arc<AtomicBool>);

fn worker(path: String, ready: oneshot::Sender<Result<Opened, String>>, _permit: tokio::sync::SemaphorePermit<'static>) {
    if ready.is_closed() {
        return;
    }
    let mut c = match configure(&path) {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(format!("sqlite-open: {e}")));
            return;
        }
    };
    let (sender, receiver) = mpsc::sync_channel::<Job>(1);
    let interrupt = Arc::new(c.get_interrupt_handle());
    let closed = Arc::new(AtomicBool::new(false));
    if ready.send(Ok((sender, interrupt, closed.clone()))).is_err() {
        return;
    }
    while let Ok(job) = receiver.recv() {
        let result = perform(&mut c, &job, &closed).map_err(|e| format!("sqlite: {e}"));
        let _ = job.reply.send(result);
        // Never reuse a connection if transaction cleanup unexpectedly failed.
        if closed.load(Ordering::SeqCst) || !c.is_autocommit() {
            break;
        }
    }
    closed.store(true, Ordering::SeqCst);
}

async fn request(db: Rc<Database>, sql: String, params: Vec<SqlValue>, query: bool) -> Result<Reply, String> {
    let gate = db.gate.clone().lock_owned().await;
    if db.closed.load(Ordering::SeqCst) {
        return Err("sqlite: database is closed".into());
    }
    let cancel = Cancel(Arc::new(AtomicBool::new(false)));
    let (reply, receive) = oneshot::channel();
    let job = Job { sql, params, query, cancel: cancel.0.clone(), reply, _gate: gate };
    db.sender.borrow().as_ref().ok_or("sqlite: database is closed")?.try_send(job).map_err(|_| "sqlite: database worker unavailable")?;
    let result = receive.await.map_err(|_| "sqlite: database worker stopped")?;
    if db.closed.load(Ordering::SeqCst) {
        return Err("sqlite: database is closed".into());
    }
    result
}

const PRELUDE: &str = r#"
(define (sqlite-open path)
  "Open or create the SQLite database at PATH; return a scope-owned handle.
PATH is a filename or \":memory:\" (not a URI). Requires the files capability.
All database work waits asynchronously, on a separate worker."
  (scope-own! (%sqlite-open path) sqlite-close sqlite-closed?))

(define (call-with-sqlite path proc)
  "Call PROC with an open database at PATH; return its result.
Close the handle when PROC returns, fails or is cancelled."
  (let ((db (sqlite-open path)))
    (dynamic-wind (lambda () #f) (lambda () (proc db)) (lambda () (sqlite-close db)))))
"#;

/// Install SQLite natives and scope/lifetime wrappers on the shared runtime.
/// Every native requires `Capability::Files`, including in-memory databases.
pub fn install(vm: &mut Vm) -> Result<(), Error> {
    install_on(vm, runtime().handle().clone())
}

/// Install with workers on `io`'s blocking pool. The host must keep this runtime
/// alive and allow enough blocking threads for its long-lived connections (plus
/// other blocking work). No async runtime context is needed while driving VM tasks.
pub fn install_on(vm: &mut Vm, io: Handle) -> Result<(), Error> {
    vm.name_foreign_type::<Database>("sqlite-database");
    vm.requiring(Capability::Files, |vm| natives(vm, io));
    vm.eval_in(ROOT_MODULE, "<techne-store>", PRELUDE).map(|_| ())
}

fn natives(vm: &mut Vm, io: Handle) {
    vm.register_async("%sqlite-open", 1, move |vm: &mut Vm, args: Args<'_>| {
        let path = args.inspect(vm, 0, |v| bounded_text(v, 4096, "sqlite-open")).and_then(|r| r);
        let io = io.clone();
        async move {
            let path = path.map_err(|e| e.msg.clone())?;
            if path.is_empty() || path.starts_with("file:") || (path.starts_with(':') && path != ":memory:") {
                return Err("sqlite-open: expected a filename or :memory:, not a URI".to_string());
            }
            let permit = CONNECTIONS.try_acquire().map_err(|_| "sqlite-open: process limit of 32 connections")?;
            let (ready, receive) = oneshot::channel();
            io.spawn_blocking(move || worker(path, ready, permit));
            let (sender, interrupt, closed) = receive.await.map_err(|_| "sqlite-open: worker stopped")??;
            Ok(Foreign::new(Database { sender: RefCell::new(Some(sender)), interrupt, closed, gate: Arc::new(Mutex::new(())) }))
        }
    });
    for (name, query) in [("sqlite-execute", false), ("sqlite-query", true)] {
        vm.register_async(name, 3, move |vm: &mut Vm, args: Args<'_>| {
            let input = (|| -> Result<_, Error> {
                vm.admit(2 * INPUT_BYTES + if query { 2 * RESULT_BYTES } else { 0 })?;
                let db = args.get::<Foreign<Database>>(vm, 0)?.0;
                let sql = args.inspect(vm, 1, |v| bounded_text(v, SQL_BYTES, "sqlite"))??;
                let params = args.inspect(vm, 2, parameters)??;
                Ok((db, sql, params))
            })();
            async move {
                let (db, sql, params) = input.map_err(|e| e.msg.clone())?;
                request(db, sql, params, query).await
            }
        });
    }
    techne_vm::document! { vm;
        /// Execute one non-row-returning SQL statement in DB with bound PARAMS.
        /// Return the affected row count. PARAMS is a list/vector of int64,
        /// finite flonums, strings, bytevectors and #f (SQL NULL), at most 256
        /// values and 32 MiB. SQL is at most 64 KiB. Each statement is atomic;
        /// explicit transactions, ATTACH, extensions and most PRAGMAs are denied.
        /// SQLite work has a five-second budget, lock waits 250 ms (not a hard
        /// deadline on OS filesystem calls). Cancellation
        /// rolls back unfinished work, but can race an already completed commit.
        "(sqlite-execute db sql params)";
        /// Query DB with one read-only SQL statement and bound PARAMS.
        /// Return a list of row lists: NULL is #f, blobs are bytevectors, and
        /// other cells are strings or numbers. Parameter limits match
        /// sqlite-execute. At most 4096 rows / 40 MiB (including cell overhead),
        /// never silently truncated; use SQL LIMIT and paging for larger data.
        /// Invalid UTF-8 TEXT raises an error. A cell is at most 32 MiB.
        "(sqlite-query db sql params)";
    }
    techne_vm::procedures! { vm;
        /// Close DB idempotently and interrupt pending work. The handle becomes
        /// unusable immediately; its worker rolls back and releases OS resources
        /// asynchronously. No caller transaction can survive cancellation.
        "(sqlite-close db)" => |db: Foreign<Database>| { db.0.close(); };
        /// Return #t if DB has been closed or its worker has stopped.
        "(sqlite-closed? db)" => |db: Foreign<Database>| db.0.closed.load(Ordering::SeqCst);
    }
}
