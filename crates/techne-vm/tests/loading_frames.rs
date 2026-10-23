//! Loading from the Scheme forms loop must not nest a Rust dispatcher.
use std::{
    fs,
    process::{Command, Stdio},
    time::Duration,
};
use wait_timeout::ChildExt;

fn check(files: &[(&str, &str)], source: &str, expected: &str) {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
        fs::write(dir.path().join(name), text).unwrap();
    }
    // eval-source uses this file's directory for library and include lookup.
    let file = dir.path().join("input.scm");
    let program = format!("(displayln (task-join (spawn (lambda () (eval-source {source:?} \"user\" {:?})))))", file.to_str().unwrap());
    fs::write(&file, program).unwrap();
    // Stress is on whenever TECHNE_GC_STRESS is set, whatever its value.
    for (jit, stress) in [("0", None), ("1", None), ("0", Some("full"))] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_techne-vm"));
        command.arg(&file).current_dir(dir.path()).env("TECHNE_JIT", jit).env("TECHNE_JIT_SYNC", "1").env_remove("TECHNE_GC_STRESS");
        if let Some(stress) = stress {
            command.env("TECHNE_GC_STRESS", stress);
        }
        let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let finished = child.wait_timeout(Duration::from_secs(30)).unwrap().is_some();
        if !finished {
            child.kill().unwrap();
        }
        let out = child.wait_with_output().unwrap();
        assert!(finished && out.status.success(), "jit={jit}, gc={stress:?}: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout), expected, "jit={jit}, gc={stress:?}");
    }
}

#[test]
fn imported_library_dependencies_can_wait() {
    check(
        &[
            (
                "dependency.sld",
                r#"
                (define-library (dependency)
                  (export answer)
                  (import (techne))
                  (begin
                    (define channel (make-channel))
                    (spawn (lambda () (channel-send channel 42)))
                    (define answer (channel-recv channel))))
            "#,
            ),
            (
                "waiting.sld",
                r#"
                (define-library (waiting)
                  (export result)
                  (include-library-declarations "imports.scm")
                  (include "body.scm"))
            "#,
            ),
            ("imports.scm", "(import (scheme base) (rename (only (dependency) answer) (answer result)))"),
            ("body.scm", "(define-syntax twice (syntax-rules () ((_ x) (+ x x)))) (define doubled (twice result))"),
        ],
        "(import (prefix (waiting) w:)) w:result",
        "42\n",
    );
}

#[test]
fn included_forms_compile_after_the_previous_form_runs() {
    check(
        &[(
            "waiting.scm",
            r#"
            (define channel (make-channel))
            (spawn (lambda () (channel-send channel 42)))
            (define answer (channel-recv channel))
            ;; This definition takes effect while the first form runs,
            ;; not while the entire include is compiled.
            (eval-source "(define-syntax result (syntax-rules () ((_) answer)))" "user" "macro.scm")
            (result)
        "#,
        )],
        "(include \"waiting.scm\")",
        "42\n",
    );
}

const BLOCKED_LIBRARY: &str = r#"
    (define-library (waiting)
      (export answer)
      (import (techne))
      (begin
        (define answer 10)
        (channel-send library-entered #t)
        (channel-recv library-go)
        (set! answer 42)))
"#;

fn concurrent_import(expression: &str) {
    let source = format!(
        r#"
        (eval '(begin (define library-entered (make-channel))
                      (define library-go (make-channel))) "root")
        (define-syntax hidden-import (syntax-rules () ((_)(import (waiting)))))
        (define loader (spawn (lambda () (eval-source "(import (waiting)) answer" "user" "input.scm"))))
        (channel-recv library-entered)
        (define seen
          (guard (e (#t (if (string-contains (condition/report-string e) "still loading") 'loading 'wrong-error)))
            {expression}))
        (channel-send library-go #t)
        (list seen (task-join loader))
    "#
    );
    check(&[("waiting.sld", BLOCKED_LIBRARY)], &source, "(loading 42)\n");
}

#[test]
fn macro_import_cannot_observe_a_loading_library() {
    concurrent_import(r#"(eval-source "(hidden-import) answer" "user" "input.scm")"#);
}

#[test]
fn environment_cannot_observe_a_loading_library() {
    concurrent_import("(eval 'answer (environment '(waiting)))");
}

#[test]
fn failed_library_body_is_reloaded() {
    // Retry through each entry path, not only the literal forms queue.
    for expression in ["(import (retry)) answer", "(hidden-import) answer", "(eval 'answer (environment '(retry)))"] {
        let source = format!(
            r#"
            (eval '(define library-attempts 0) "root")
            (define-syntax hidden-import (syntax-rules () ((_)(import (retry)))))
            (define first (guard (e (#t 'failed)) (eval-source {expression:?} "user" "input.scm")))
            (define second (eval-source {expression:?} "user" "input.scm"))
            (list first second library-attempts)
        "#
        );
        check(
            &[(
                "retry.sld",
                r#"
                (define-library (retry)
                  (export answer)
                  (import (techne))
                  (begin
                    (define answer 10)
                    (set! library-attempts (+ library-attempts 1))
                    (if (= library-attempts 1) (error "first load fails"))
                    (set! answer 42)))
            "#,
            )],
            &source,
            "(failed 42 2)\n",
        );
    }
}

fn stopped_library_is_reloaded(stop: &str) {
    let source = format!(
        r#"
        (eval '(begin (define library-entered (make-channel))
                      (define library-go (make-channel))
                      (define library-attempts 0)) "root")
        (define loader (spawn (lambda () (eval-source "(import (retry)) answer" "user" "input.scm"))))
        (channel-recv library-entered)
        ({stop} loader)
        (define first (guard (e (#t 'stopped)) (task-join loader)))
        (define second (eval-source "(import (retry)) answer" "user" "input.scm"))
        (list first second library-attempts)
    "#
    );
    check(
        &[(
            "retry.sld",
            r#"
            (define-library (retry)
              (export answer)
              (import (techne))
              (begin
                (define answer 10)
                (set! library-attempts (+ library-attempts 1))
                (when (= library-attempts 1)
                  (channel-send library-entered #t)
                  (channel-recv library-go))
                (set! answer 42)))
        "#,
        )],
        &source,
        "(stopped 42 2)\n",
    );
}

#[test]
fn killed_library_loader_is_reloaded() {
    stopped_library_is_reloaded("task-kill");
}

#[test]
fn cancelled_library_loader_is_reloaded() {
    stopped_library_is_reloaded("task-cancel");
}

#[test]
fn failed_library_declarations_are_reloaded() {
    for expression in ["(import (broken)) answer", "(hidden-import) answer", "(eval 'answer (environment '(broken)))"] {
        let source = format!(
            r#"
            (define-syntax hidden-import (syntax-rules () ((_)(import (broken)))))
            (define (attempt)
              (guard (e (#t (condition/report-string e)))
                (eval-source {expression:?} "user" "input.scm")))
            (define first (attempt))
            (define second (attempt))
            (list (string=? first second)
                  (if (string-contains second "unknown declaration") 'retried 'wrong-error))
        "#
        );
        check(&[("broken.sld", "(define-library (broken) (export answer) (unknown-declaration))")], &source, "(#t retried)\n");
    }
}

#[test]
fn failed_library_read_is_retried() {
    check(
        &[("broken.sld", "(define-library (broken)")],
        r#"
            (define (attempt)
              (guard (e (#t (condition/report-string e)))
                (eval-source "(import (broken))" "user" "input.scm")))
            (define first (attempt))
            (define second (attempt))
            (list (string=? first second)
                  (if (string-contains second "unexpected end of input") 'retried 'wrong-error))
        "#,
        "(#t retried)\n",
    );
}

#[test]
fn escaped_closures_keep_aborted_library_and_file_globals() {
    check(
        &[(
            "escaped.sld",
            r#"
            (define source-private (vector 'source))
            (set! escaped-source (lambda () source-private))
            (define-library (escaped)
              (export answer)
              (import (techne))
              (begin
                (define private-value (vector 'library))
                (define answer 10)
                (set! escaped-library (lambda () private-value))
                (error "abort the library")))
        "#,
        )],
        r#"
            (eval '(begin (define escaped-library #f) (define escaped-source #f)) "root")
            (guard (e (#t #f)) (eval-source "(import (escaped))" "user" "input.scm"))
            (collect-garbage 'full)
            ;; Reusing slots must not change either escaped procedure's globals.
            (define churn (vector 'new-slot))
            (list (escaped-library) (escaped-source))
        "#,
        "(#(library) #(source))\n",
    );
}

#[test]
fn cond_expand_ignores_a_killed_inline_library() {
    check(
        &[],
        r#"
            (eval '(define library-entered (make-channel)) "root")
            (define loader (spawn (lambda ()
              (eval-source "(define-library (inline-only) (export answer) (import (techne))
                             (begin (define answer 10) (channel-send library-entered #t) (sleep 100000)))"
                           "user" "inline.scm"))))
            (channel-recv library-entered)
            (task-kill loader)
            (guard (e (#t #f)) (task-join loader))
            (define first (cond-expand ((library (inline-only)) 'available) (else 'missing)))
            (guard (e (#t #f)) (eval-source "(import (inline-only))" "user" "input.scm"))
            (list first (cond-expand ((library (inline-only)) 'available) (else 'missing)))
        "#,
        "(missing missing)\n",
    );
}

fn pending_library_vm(jit: Option<u32>, full_gc: bool) -> (techne_vm::vm::Vm, techne_vm::tasks::TaskId) {
    use techne_vm::{tasks::Progress, vm::Vm};
    let mut vm = Vm::new();
    vm.set_jit(jit);
    vm.set_gc_stress(full_gc, full_gc);
    let loader = vm
        .eval_source(
            r#"(lambda ()
        (load-module-from "/dev/null"
          "(define-library (snapshot-pending) (export answer) (import (techne))
             (begin (define answer 10) (sleep 100000) (set! answer 42)))"))"#,
        )
        .unwrap();
    let task = vm.spawn(&loader);
    assert_eq!(vm.run_tasks_for(Duration::from_secs(1)), Progress::Blocked);
    (vm, task)
}

fn snapshot_reloads_dead_library(colliding: bool) {
    use techne_vm::tasks::Progress;
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("snapshot-pending.sld"),
        "(define-library (snapshot-pending) (export answer) (import (scheme base)) (begin (define answer 42)))",
    )
    .unwrap();
    let input = dir.path().join("input.scm");
    let source = format!("(eval-source \"(import (snapshot-pending)) answer\" \"user\" {:?})", input.to_str().unwrap());
    for (jit, full_gc) in [(None, false), (Some(1), false), (None, true)] {
        let (mut vm, task) = pending_library_vm(jit, full_gc);
        let old_id = vm.task_execution(&task).unwrap();
        vm.kill_task(&task).unwrap();
        assert_eq!(vm.run_tasks_for(Duration::from_secs(1)), Progress::Finished);
        drop(task);
        let snapshot = vm.snapshot().unwrap();
        let mut copied = snapshot.world();
        copied.set_jit(jit);
        copied.set_gc_stress(full_gc, full_gc);
        // Both layouts exposed stale loader state: an unrelated live id
        // looked like the old owner, otherwise abort used absent file holds.
        loop {
            let id = copied.new_execution();
            assert!(id.0 <= old_id.0, "fresh world passed the old execution id");
            if !colliding || id != old_id {
                copied.discard_execution(id);
            }
            if id == old_id {
                break;
            }
        }
        let value = copied.eval_source(&source).unwrap();
        assert_eq!(value.as_int(), Some(42), "colliding={colliding}, jit={jit:?}, gc={full_gc}");
    }
}

#[test]
fn snapshot_retires_dead_libraries_without_an_execution_id_collision() {
    snapshot_reloads_dead_library(false);
}

#[test]
fn snapshot_retires_dead_libraries_with_an_execution_id_collision() {
    snapshot_reloads_dead_library(true);
}

#[test]
fn snapshot_rejects_a_library_whose_loader_is_still_live() {
    let (vm, task) = pending_library_vm(None, false);
    drop(task);
    let error = vm.snapshot().err().expect("a library is still loading");
    assert!(error.msg.contains("library is loading"), "{error}");
}

#[test]
fn included_library_failure_evicts_the_enclosing_file() {
    for first in [r#"(load-module-from "retry.sld" "(include \"declaration.scm\")")"#, "(environment '(retry))"] {
        let source = format!(
            r#"
            (eval '(define library-attempts 0) "root")
            (define first (guard (e (#t 'failed)) {first}))
            (define result (eval-source "(import (retry)) answer" "user" "input.scm"))
            (list first result library-attempts)
        "#
        );
        check(
            &[
                ("retry.sld", "(include \"declaration.scm\")"),
                (
                    "declaration.scm",
                    r#"
                (define-library (retry)
                  (export answer)
                  (import (techne))
                  (begin
                    (set! library-attempts (+ library-attempts 1))
                    (define answer 10)
                    (if (= library-attempts 1) (error "first load fails"))
                    (set! answer 42)))
              "#,
                ),
            ],
            &source,
            "(failed 42 2)\n",
        );
    }
}
