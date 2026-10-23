//! Cleanup calls are VM frames: their waits and transfers must not get lost.
use std::{
    fs,
    process::{Command, Stdio},
    time::Duration,
};
use wait_timeout::ChildExt;

fn check(source: &str, expected: &str) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("unwind.scm");
    fs::write(&file, source).unwrap();
    // Stress is on whenever TECHNE_GC_STRESS is set, whatever its value.
    for (jit, gc) in [("0", None), ("1", None), ("0", Some("full"))] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_techne-vm"));
        command.arg(&file).env("TECHNE_JIT", jit).env("TECHNE_JIT_SYNC", "1").env("TECHNE_GC_VERIFY", "1").env_remove("TECHNE_GC_STRESS");
        if let Some(gc) = gc {
            command.env("TECHNE_GC_STRESS", gc);
        }
        let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let done = child.wait_timeout(Duration::from_secs(30)).unwrap().is_some();
        if !done {
            child.kill().unwrap();
        }
        let out = child.wait_with_output().unwrap();
        assert!(done && out.status.success(), "jit={jit}, gc={gc:?}: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout), expected, "jit={jit}, gc={gc:?}");
    }
}

#[test]
fn escape_waits_for_its_after_thunk_and_keeps_its_value() {
    check(
        r#"
        (define entered (make-channel 1)) (define gate (make-channel 1))
        (define log '())
        (define t (spawn (lambda ()
          (call/ec (lambda (out)
            (dynamic-wind (lambda () #f)
              (lambda () (out (list (vector 'original))))
              (lambda () (channel-send entered #t)
                         (set! log (cons (channel-recv gate) log)))))))))
        (channel-recv entered) (collect-garbage 'full)
        (channel-send gate 'finished)
        (displayln (list (task-join t) log))
    "#,
        "((#(original)) (finished))\n",
    );
}

#[test]
fn error_survives_a_local_handler_wait_in_its_after_thunk() {
    check(
        r#"
        (define entered (make-channel 1)) (define gate (make-channel 1))
        (define log '())
        (define t (spawn (lambda ()
          (guard (original (#t original))
            (dynamic-wind (lambda () #f)
              (lambda () (raise (list (vector 'original))))
              (lambda ()
                (guard (cleanup (#t (channel-send entered #t)
                                    (set! log (cons (channel-recv gate) log))))
                  (raise 'cleanup))))))))
        (channel-recv entered) (collect-garbage 'full)
        (channel-send gate 'handled)
        (displayln (list (task-join t) log))
    "#,
        "((#(original)) (handled))\n",
    );
}

#[test]
fn uncaught_task_error_waits_for_cleanup_before_publication() {
    check(
        r#"
        (define entered (make-channel 1)) (define gate (make-channel 1))
        (define finished #f)
        (define bad (spawn (lambda () (error "join failure"))))
        (define t (spawn (lambda ()
          (dynamic-wind (lambda () #f)
            (lambda () (task-join bad))
            (lambda () (channel-send entered #t)
                       (channel-recv gate) (set! finished #t))))))
        (channel-recv entered) (collect-garbage 'full)
        (displayln (task-done? t))
        (channel-send gate #t)
        (displayln (list (guard (e (#t (condition/report-string e))) (task-join t)) finished))
    "#,
        "#f\n(join failure #t)\n",
    );
}

#[test]
fn kill_while_waiting_in_cleanup_drops_pending_transfer_and_outer_winds() {
    check(
        r#"
        (define entered (make-channel 1)) (define gate (make-channel 1))
        (define log '())
        (define t (spawn (lambda ()
          (call/ec (lambda (out)
            (dynamic-wind (lambda () #f)
              (lambda () (dynamic-wind (lambda () #f) (lambda () (out 'body))
                (lambda () (set! log (cons 'inner log))
                  (channel-send entered #t) (channel-recv gate)
                  (set! log (cons 'resumed log)))))
              (lambda () (set! log (cons 'outer log)))))))))
        (channel-recv entered) (collect-garbage 'full)
        (task-kill t)
        (displayln (list (guard (e (#t (condition/report-string e))) (task-join t)) log))
    "#,
        "(task killed (inner))\n",
    );
}

#[test]
fn cleanup_error_replaces_the_original_at_the_host_boundary() {
    check(
        r#"
        (define log '())
        (displayln
          (list
            (guard (e (#t (condition/report-string e)))
              (dynamic-wind (lambda () #f)
                (lambda () (dynamic-wind (lambda () #f)
                  (lambda () (error "body"))
                  (lambda () (set! log (cons 'inner log)) (error "after"))))
                (lambda () (set! log (cons 'outer log)))))
            (reverse log)))
    "#,
        "(after (inner outer))\n",
    );
}

#[test]
fn cleanup_escape_replaces_pending_escape_without_running_a_wind_twice() {
    check(
        r#"
        (define log '()) (define saved #f)
        (define t (spawn (lambda ()
          (call/ec (lambda (outer)
            (set! saved outer)
            (dynamic-wind (lambda () (set! log (cons 'outer-before log)))
              (lambda () (call/ec (lambda (inner)
                (dynamic-wind (lambda () (set! log (cons 'inner-before log)))
                  (lambda () (inner 'body))
                  (lambda () (set! log (cons 'inner-after log))
                    (yield) (outer 'replacement))))))
              (lambda () (set! log (cons 'outer-after log)) (yield))))))))
        (displayln (list (task-join t) (reverse log)))
        (displayln (guard (e (#t 'expired)) (saved 'late)))
    "#,
        "(replacement (outer-before inner-before inner-after outer-after))\nexpired\n",
    );
}

#[test]
fn cleanup_raise_keeps_outer_handlers_and_can_replace_the_same_live_escape() {
    check(
        r#"
        (define after-count 0)
        (define t (spawn (lambda ()
          (call/ec (lambda (out)
            (with-exception-handler
              (lambda (condition) (yield) (out condition))
              (lambda () (dynamic-wind (lambda () #f)
                (lambda () (out 'body))
                (lambda () (set! after-count (+ after-count 1)) (raise 'after))))))))))
        (displayln (list (task-join t) after-count))
    "#,
        "(after 1)\n",
    );
}
