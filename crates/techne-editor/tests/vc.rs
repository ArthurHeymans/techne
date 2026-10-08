//! The vertical slice of the architecture review: a version control status
//! view (`lisp/editor/packages/vc.scm`) whose rows come from a process in
//! the background, keep the caret on its row as they change, reload with
//! their package while work is in flight, and whose targets' actions work
//! wherever the targets come from.

use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use techne_editor::{
    present::{Input, Snapshot},
    runtime::{Runtime, lisp_dir},
};

fn keys(rt: &mut Runtime, keys: &str) {
    for k in keys.split(' ') {
        rt.handle(Input::Key { key: k.to_string(), at: Instant::now() });
    }
}

fn type_text(rt: &mut Runtime, text: &str) {
    for c in text.chars() {
        let key = if c == ' ' { "SPC".to_string() } else { c.to_string() };
        rt.handle(Input::Key { key, at: Instant::now() });
    }
}

/// Run background tasks until `done` holds of a snapshot, as the host's
/// loop does between inputs.
fn until(rt: &mut Runtime, what: &str, done: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let start = Instant::now();
    loop {
        rt.run_tasks(Duration::from_millis(5));
        let s = rt.snapshot();
        if done(&s) {
            return s;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "timed out waiting for {what}: {:?} / {}", s.pane().text.to_string(), s.echo);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "init.defaultBranch=main"])
        .args(args)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?}");
}

/// A Git repository with a.txt and b.txt committed and a.txt changed, and
/// a runtime on a.txt (its journal outside the repository).
fn repository() -> (tempfile::TempDir, tempfile::TempDir, PathBuf, Runtime) {
    let (dir, state) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = dir.path().canonicalize().unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    std::fs::write(root.join("b.txt"), "two\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "start"]);
    std::fs::write(root.join("a.txt"), "ONE\n").unwrap();
    let rt = Runtime::open(&root.join("a.txt"), &state.path().join("a.journal"), "emacs").unwrap().0;
    (dir, state, root, rt)
}

fn text(s: &Snapshot) -> String {
    s.pane().text.to_string()
}

#[test]
fn a_status_view_from_a_process() {
    let (_dir, _state, root, mut rt) = repository();
    rt.eval("(set-option! 'vc-refresh-interval #f)").unwrap();
    let head = format!("git {}", root.display());
    keys(&mut rt, "C-x v d");
    let s = until(&mut rt, "the status", |s| text(s).contains("a.txt"));
    assert_eq!(text(&s), format!("{head}\nM  a.txt"));
    assert!(s.pane().status.contains("vc-status"), "{}", s.pane().status);
    // The caret inside a.txt's row; then more changes, one wider.
    keys(&mut rt, "C-n C-f C-f C-f C-f");
    std::fs::write(root.join("b.txt"), "TWO\n").unwrap();
    std::fs::write(root.join("c.txt"), "new\n").unwrap();
    keys(&mut rt, "g");
    let s = until(&mut rt, "the refresh", |s| text(s).contains("c.txt"));
    assert_eq!(text(&s), format!("{head}\nM   a.txt\nM   b.txt\n??  c.txt"));
    let (t, head_at) = (&s.pane().text, s.pane().head());
    assert_eq!(t.byte_to_line(head_at), 1, "still on a.txt's row");
    assert_eq!(t.byte_slice(head_at - 1..head_at).to_string(), "a", "after the same character");
    // RET opens the file of the row.
    keys(&mut rt, "C-n RET");
    assert!(rt.snapshot().pane().status.contains("b.txt"));
}

/// Reloading the package while a refresh is in flight cancels it with the
/// old generation; the new one takes over the status buffer and refreshes
/// it by itself. Nothing is left running.
#[test]
fn reloading_while_a_refresh_is_in_flight() {
    let (_dir, _state, root, mut rt) = repository();
    let path = lisp_dir().join("packages/vc.scm").display().to_string();
    keys(&mut rt, "C-x v d");
    until(&mut rt, "the status", |s| text(s).contains("a.txt"));
    std::fs::write(root.join("c.txt"), "new\n").unwrap();
    keys(&mut rt, "g");
    assert_eq!(rt.eval("(request-pending? (hash-table-ref (buffer-state (session-buffer (current-session))) 'slot))").unwrap(), "#t");
    rt.eval(&format!("(load-package 'vc {path:?})")).unwrap();
    assert_eq!(rt.eval("(package-generation (find-package 'vc))").unwrap(), "2");
    let s = until(&mut rt, "the new generation's refresh", |s| text(s).contains("c.txt"));
    assert!(text(&s).ends_with("M   a.txt\n??  c.txt"), "{}", text(&s));
    // It refreshes by itself: a file written later shows up.
    rt.eval("(set-option! 'vc-refresh-interval 20)").unwrap();
    std::fs::write(root.join("d.txt"), "later\n").unwrap();
    until(&mut rt, "the timer's refresh", |s| text(s).contains("d.txt"));
    rt.eval("(set-option! 'vc-refresh-interval 2000)").unwrap();
    // A last refresh may have started before the interval grew.
    let start = Instant::now();
    while rt.eval("(request-pending? (hash-table-ref (buffer-state (session-buffer (current-session))) 'slot))").unwrap() == "#t" {
        assert!(start.elapsed() < Duration::from_secs(20), "the last refresh never finished");
        rt.run_tasks(Duration::from_millis(5));
    }
    let live =
        "(length (filter (lambda (r) (guard (e (#t #f)) (not (task-done? r)))) (scope-resources (package-scope (find-package 'vc)))))";
    assert_eq!(rt.eval(live).unwrap(), "1", "the package runs its timer, nothing more");
    assert_eq!(rt.eval("(map scope-name (scope-children %root-scope))").unwrap(), "(vc)", "the old generation is gone");
    // Unloaded, its keys and commands go; the buffer stays, inert.
    rt.eval("(unload-package 'vc)").unwrap();
    assert_eq!(rt.eval("(memq 'vc-status (command-names))").unwrap(), "#f");
}

/// A refresh slower than the interval is left to finish: the timer does
/// not replace it, which would keep the view from ever refreshing.
#[test]
fn a_slow_refresh_is_not_replaced_by_the_timer() {
    let (_dir, _state, _root, mut rt) = repository();
    keys(&mut rt, "C-x v d");
    until(&mut rt, "the status", |s| text(s).contains("a.txt"));
    let slot = "(hash-table-ref (buffer-state (session-buffer (current-session))) 'slot)";
    rt.eval("(define slow-done #f)").unwrap();
    // Slower than the timer's sleep at the default interval, so it ticks.
    rt.eval(&format!("(request! {slot} (lambda () (sleep 2500)) (lambda (_) (set! slow-done #t)))")).unwrap();
    rt.eval("(set-option! 'vc-refresh-interval 1)").unwrap();
    let start = Instant::now();
    while rt.eval("slow-done").unwrap() != "#t" {
        assert!(start.elapsed() < Duration::from_secs(10), "the slow refresh was replaced");
        rt.run_tasks(Duration::from_millis(5));
    }
    rt.eval("(set-option! 'vc-refresh-interval #f)").unwrap();
}

/// A changed file's actions need nothing of the view: from the minibuffer,
/// with the view not shown, C-; offers them; reverting refreshes the views
/// of the repository.
#[test]
fn acting_on_changes_from_anywhere() {
    let (_dir, _state, root, mut rt) = repository();
    rt.eval("(set-option! 'vc-refresh-interval #f)").unwrap();
    keys(&mut rt, "C-x v d");
    until(&mut rt, "the status", |s| text(s).contains("a.txt"));
    keys(&mut rt, "C-x b RET");
    assert!(rt.snapshot().pane().status.contains("a.txt"));
    keys(&mut rt, "C-x v f");
    until(&mut rt, "the changes", |s| s.minibuffer.as_ref().is_some_and(|m| !m.rows.is_empty()));
    keys(&mut rt, "C-;");
    let s = rt.snapshot();
    let actions: Vec<String> = s.minibuffer.unwrap().rows.iter().map(|r| r.text(0)).collect();
    assert_eq!(actions, ["vc-visit", "vc-diff", "vc-revert", "vc-copy-path"]);
    type_text(&mut rt, "diff");
    keys(&mut rt, "RET");
    let s = until(&mut rt, "the diff", |s| s.panes.len() == 2);
    let diff = s.panes[1].text.to_string();
    assert!(diff.contains("-one") && diff.contains("+ONE"), "{diff}");
    // Revert, after asking: the file is back and the view no longer lists it.
    keys(&mut rt, "C-x 1 C-x v f");
    until(&mut rt, "the changes", |s| s.minibuffer.is_some());
    keys(&mut rt, "C-;");
    type_text(&mut rt, "revert");
    keys(&mut rt, "RET");
    assert_eq!(rt.snapshot().minibuffer.unwrap().prompt, "1/2 Revert a.txt? ");
    keys(&mut rt, "RET");
    until(&mut rt, "the revert", |s| s.echo == "Reverted a.txt");
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "one\n");
    let name = format!("*vc {}*", root.display());
    let shown = |rt: &mut Runtime| rt.eval(&format!("(document-string (buffer-document (buffer-named {name:?})))")).unwrap();
    let start = Instant::now();
    while shown(&mut rt).contains("a.txt") {
        assert!(start.elapsed() < Duration::from_secs(20), "{}", shown(&mut rt));
        rt.run_tasks(Duration::from_millis(5));
    }
    assert!(shown(&mut rt).contains("No changes"), "{}", shown(&mut rt));
}
