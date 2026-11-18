use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use techne_vm::vm::{Capability, Grants, Vm};

struct Store(Vm);
impl Store {
    fn new() -> Self {
        let mut vm = Vm::new();
        techne_store::install(&mut vm).unwrap();
        vm.eval_source("(define (report thunk) (guard (e (#t (condition/report-string e))) (thunk)))").unwrap();
        Self(vm)
    }
    fn run(&mut self, source: &str) -> Result<String, String> {
        let interrupt = self.0.interrupt_handle();
        let (done, watch) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            if matches!(watch.recv_timeout(Duration::from_secs(20)), Err(mpsc::RecvTimeoutError::Timeout)) {
                interrupt.interrupt();
            }
        });
        let result = self.0.eval_source(source);
        drop(done);
        result.map(|v| self.0.repr(&v)).map_err(|e| e.to_string())
    }
    fn eval(&mut self, source: &str) -> String {
        self.run(source).unwrap_or_else(|e| panic!("{source}\n{e}"))
    }
    fn fails(&mut self, source: &str, message: &str) {
        let error = self.run(source).expect_err(source);
        assert!(error.contains(message), "{source}: {error}, expected {message}");
    }
}

#[test]
fn binding_roundtrip_and_documentation() {
    let mut s = Store::new();
    assert_eq!(
        s.eval(
            r#"
        (import (techne sqlite))
        (define db (sqlite-open ":memory:"))
        (sqlite-execute db "create table t (s text, i integer, f real, b blob, n)" '())
        (define text "it's λ 雪; DROP TABLE t; --")
        (define count (sqlite-execute db "insert into t values (?1,?2,?3,?4,?5)"
                         (list text -9223372036854775808 1.25 (bytevector 0 255 39) #f)))
        (list count (equal? (sqlite-query db "select * from t where s=?" (list text))
                           (list (list text -9223372036854775808 1.25 (bytevector 0 255 39) #f)))
              (sqlite-query db "select ?1, ?2" (vector 9223372036854775807 "a\x0;b")))
    "#
        ),
        r#"(1 #t ((9223372036854775807 "a\x0;b")))"#
    );
    s.fails(r#"(sqlite-query db "select ?" (list #t))"#, "finite flonum");
    s.fails(r#"(sqlite-query db "select ?" (list 9223372036854775808))"#, "int64");
    s.fails(r#"(sqlite-query db "select ?" '())"#, "parameter");
    // Wrapper and native documentation are both installed.
    assert_eq!(s.eval("(every (lambda (name) (string? (documentation (eval name)))) '(sqlite-open sqlite-execute sqlite-query sqlite-close sqlite-closed? call-with-sqlite))"), "#t");
}

#[test]
fn persistence_constraints_and_multiple_statements() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mail.sqlite");
    let mut s = Store::new();
    s.eval(&format!(r#"(define path {:?}) (define db (sqlite-open path))"#, path.to_str().unwrap()));
    s.eval(
        r#"(sqlite-execute db "create table t (id integer primary key, s text not null unique)" '())
              (sqlite-execute db "insert into t values (?,?)" '(1 "saved"))"#,
    );
    s.fails(r#"(sqlite-execute db "insert into t values (?,?)" '(2 "saved"))"#, "UNIQUE constraint");
    s.fails(r#"(sqlite-execute db "insert into t values (?,?)" '(3 #f))"#, "NOT NULL constraint");
    // OR FAIL retains earlier changes in SQLite's statement; our transaction
    // must roll those back too.
    s.fails(r#"(sqlite-execute db "insert or fail into t values (6,'partial'), (7,'saved')" '())"#, "UNIQUE constraint");
    s.fails(r#"(sqlite-execute db "insert into t values (4,'no'); insert into t values (5,'no')" '())"#, "Multiple statements");
    s.fails(r#"(sqlite-execute db "select * from t" '())"#, "row-returning");
    s.fails(r#"(sqlite-query db "delete from t returning id" '())"#, "read-only");
    assert_eq!(
        s.eval(
            r#"(sqlite-close db) (sqlite-close db)
                         (define reopened (sqlite-open path))
                         (sqlite-query reopened "select * from t" '())"#
        ),
        r#"((1 "saved"))"#
    );
    s.fails(r#"(sqlite-query db "select 1" '())"#, "closed");
    assert_eq!(s.eval("(sqlite-closed? db)"), "#t");
    assert_eq!(
        s.eval(
            r#"
        (define connections (list reopened (sqlite-open path)))
        (define writes (map (lambda (i) (spawn (lambda ()
          (sqlite-execute (list-ref connections (modulo i 2)) "insert into t values (?,?)"
                          (list (+ i 10) (number->string i)))))) (iota 20)))
        (for-each task-join writes)
        (sqlite-query reopened "select count(*) from t" '())
    "#
        ),
        "((21))"
    );
    // A separately created VM sees the on-disk state too.
    let mut other = Store::new();
    assert_eq!(
        other.eval(&format!(
            r#"(call-with-sqlite {:?} (lambda (db) (sqlite-query db "select s from t where id=1" '())))"#,
            path.to_str().unwrap()
        )),
        r#"(("saved"))"#
    );
}

#[test]
fn fts5_and_foreign_keys() {
    let mut s = Store::new();
    assert_eq!(
        s.eval(
            r#"
        (define db (sqlite-open ":memory:"))
        (sqlite-execute db "create virtual table docs using fts5(body)" '())
        (sqlite-execute db "insert into docs(body) values (?)" '("hello Unicode λ"))
        (sqlite-query db "select body from docs where docs match ?" '("hello"))
    "#
        ),
        r#"(("hello Unicode λ"))"#
    );
    s.eval(
        r#"(sqlite-execute db "create table parent (id primary key)" '())
              (sqlite-execute db "create table child (id references parent(id))" '())"#,
    );
    s.fails(r#"(sqlite-execute db "insert into child values (?)" '(1))"#, "FOREIGN KEY constraint");
}

#[test]
fn transactions_and_escape_hatches_are_denied() {
    let mut s = Store::new();
    s.eval(r#"(define db (sqlite-open ":memory:"))"#);
    for sql in [
        "BEGIN",
        "COMMIT",
        "ROLLBACK",
        "SAVEPOINT x",
        "RELEASE x",
        "ROLLBACK TO x",
        "ATTACH DATABASE ':memory:' AS other",
        "DETACH DATABASE other",
        "PRAGMA writable_schema=ON",
        "PRAGMA journal_mode=OFF",
        "PRAGMA foreign_keys=OFF",
        "select load_extension('bad')",
    ] {
        s.fails(&format!("(sqlite-execute db {sql:?} '())"), "not authorized");
    }
    s.fails(r#"(sqlite-execute db "VACUUM" '())"#, "within a transaction");
    assert_eq!(s.eval(r#"(sqlite-query db "select 1" '())"#), "((1))");
}

#[test]
fn denied_files_capability() {
    let mut vm = Vm::with_grants(Grants::ALL.without(Capability::Files));
    techne_store::install(&mut vm).unwrap();
    for source in [
        r#"(sqlite-open ":memory:")"#,
        r#"(sqlite-open "/tmp/must-not-create.sqlite")"#,
        r#"(sqlite-query #f "select 1" '())"#,
        r#"(sqlite-execute #f "select 1" '())"#,
        "(sqlite-close #f)",
        "(sqlite-closed? #f)",
    ] {
        let error = vm.eval_source(source).unwrap_err();
        assert!(error.msg.contains("not granted"), "{source}: {error}");
    }
}

#[test]
fn bounds_do_not_silently_truncate() {
    let mut s = Store::new();
    s.eval(r#"(define db (sqlite-open ":memory:"))"#);
    s.fails(
        r#"(sqlite-query db "with recursive t(x) as (values(1) union all select x+1 from t where x<4097) select x from t" '())"#,
        "4096 rows",
    );
    s.fails(r#"(sqlite-query db "with recursive t(x) as (values(1) union all select x+1 from t where x<6) select zeroblob(8388608) from t" '())"#, "40 MiB");
    s.fails(r#"(sqlite-query db "select zeroblob(33554433)" '())"#, "too big");
    s.fails(r#"(sqlite-query db (make-string 65537 #\x) '())"#, "65536 bytes");
    s.fails(r#"(sqlite-query db "select ?" (make-list 257 1))"#, "256 parameters");
    s.fails(r#"(sqlite-query db "select ?" (list (make-string 33554433 #\x)))"#, "32 MiB");
    s.fails(r#"(sqlite-query db "select cast(x'ff' as text)" '())"#, "invalid UTF-8");
    s.fails(r#"(sqlite-open "file:foo?mode=ro")"#, "not a URI");
    assert_eq!(s.eval(r#"(sqlite-query db "select 42" '())"#), "((42))");
}

const LONG_WRITE: &str = "with recursive t(x) as (values(1) union all select x+1 from t where x<100000000) insert into t select x from t";

#[test]
fn cancellation_kill_and_independent_workers() {
    let mut s = Store::new();
    s.eval(
        r#"(define db (sqlite-open ":memory:")) (define other (sqlite-open ":memory:"))
              (sqlite-execute db "create table t(x)" '())"#,
    );
    for stop in ["task-cancel", "task-kill"] {
        let start = Instant::now();
        let out = s.eval(&format!(
            r#"
            (define pending (spawn (lambda () (sqlite-execute db {LONG_WRITE:?} '()))))
            (sleep 20)
            ;; Finishes before the long operation on the first connection:
            ;; both the VM and a second database worker remain responsive.
            (define independent (sqlite-query other "select 99" '()))
            (define was-running (not (task-done? pending)))
            ({stop} pending)
            (report (lambda () (task-join pending)))
            (list was-running independent (sqlite-query db "select count(*) from t" '()))
        "#
        ));
        assert_eq!(out, "(#t ((99)) ((0)))");
        assert!(start.elapsed() < Duration::from_secs(3));
    }
    // Many tasks on one connection serialize without keeping caller transactions.
    assert_eq!(
        s.eval(
            r#"
        (define tasks (map (lambda (i) (spawn (lambda () (sqlite-execute db "insert into t values (?)" (list i))))) (iota 20)))
        (for-each task-join tasks)
        (sqlite-query db "select count(*) from t" '())
    "#
        ),
        "((20))"
    );
}

#[test]
fn timeout_rolls_back_and_connection_remains_usable() {
    let mut s = Store::new();
    s.eval(r#"(define db (sqlite-open ":memory:")) (sqlite-execute db "create table t(x)" '())"#);
    let start = Instant::now();
    let sql = "with recursive r(x) as (values(1) union all select x+1 from r where x<100000000) insert into t select sum(x) from r";
    s.fails(&format!("(sqlite-execute db {sql:?} '())"), "interrupt");
    assert!(start.elapsed() < Duration::from_secs(10));
    assert_eq!(s.eval(r#"(sqlite-query db "select count(*) from t" '())"#), "((0))");
}

#[test]
fn lifetime_scope_and_close_pending_work() {
    let mut s = Store::new();
    assert_eq!(
        s.eval(
            r#"
        (define held #f)
        (report (lambda () (call-with-sqlite ":memory:"
          (lambda (db) (set! held db) (error "body fails")))))
        (sqlite-closed? held)
    "#
        ),
        "#t"
    );
    assert_eq!(
        s.eval(
            r#"
        (define scope (make-scope 'database))
        (define owned (with-scope scope (sqlite-open ":memory:")))
        (scope-shutdown! scope)
        (sqlite-closed? owned)
    "#
        ),
        "#t"
    );
    s.eval(r#"(define db (sqlite-open ":memory:")) (sqlite-execute db "create table t(x)" '())"#);
    assert_eq!(
        s.eval(
            r#"
        (define cancelled-db #f)
        (define using (spawn (lambda () (call-with-sqlite ":memory:"
          (lambda (db) (set! cancelled-db db) (sleep 100000))))))
        (sleep 20) (task-cancel using)
        (report (lambda () (task-join using)))
        (sqlite-closed? cancelled-db)
    "#
        ),
        "#t"
    );
    let out = s.eval(&format!(
        r#"
        (define pending (spawn (lambda () (sqlite-execute db {LONG_WRITE:?} '()))))
        (sleep 20) (sqlite-close db)
        (list (sqlite-closed? db) (string? (report (lambda () (task-join pending)))))
    "#
    ));
    assert_eq!(out, "(#t #t)");
}

#[test]
fn dropping_vm_releases_worker_and_rolls_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("drop.sqlite");
    let mut s = Store::new();
    s.eval(&format!(r#"(define db (sqlite-open {:?})) (sqlite-execute db "create table t(x)" '())"#, path.to_str().unwrap()));
    assert_eq!(
        s.eval(&format!(
            r#"
        (define pending (spawn (lambda () (sqlite-execute db {LONG_WRITE:?} '()))))
        (sleep 20) (not (task-done? pending))
    "#
        )),
        "#t"
    );
    drop(s);
    let external = rusqlite::Connection::open(&path).unwrap();
    external.busy_timeout(Duration::from_secs(2)).unwrap();
    external.execute("insert into t values (42)", []).unwrap();
    assert_eq!(external.query_row("select count(*) from t", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
}

#[test]
fn install_on_host_runtime() {
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let mut vm = Vm::new();
    techne_store::install_on(&mut vm, runtime.handle().clone()).unwrap();
    let v = vm
        .eval_source(
            r#"(task-join (spawn (lambda () (call-with-sqlite ":memory:" (lambda (db) (sqlite-query db "select ?" '("host")))))))"#,
        )
        .unwrap();
    assert_eq!(vm.repr(&v), r#"(("host"))"#);
    // Drop VM first, allowing its blocking workers to stop before runtime drop.
    drop(vm);
}
