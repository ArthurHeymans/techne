//! Headless editor integration: a real PTY drives the Scheme terminal.
#![cfg(unix)]

use std::time::{Duration, Instant};
use techne_editor::{
    present::{Input, Snapshot},
    runtime::Runtime,
};

fn key(rt: &mut Runtime, key: &str) {
    rt.handle(Input::Key { key: key.into(), at: Instant::now() });
}

fn typing(rt: &mut Runtime, text: &str) {
    for c in text.chars() {
        let key_name = if c == ' ' { "SPC".into() } else { c.to_string() };
        key(rt, &key_name);
    }
}

fn until(rt: &mut Runtime, ready: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        rt.run_tasks(Duration::from_millis(5)).unwrap();
        let s = rt.snapshot();
        if ready(&s) {
            return s;
        }
        assert!(
            Instant::now() < deadline,
            "{}: {}\n{}\n{:?}",
            s.pane().status,
            s.echo,
            s.pane().text.to_string().trim_end(),
            rt.take_errors()
        );
    }
}

fn command(rt: &mut Runtime, script: &str) {
    key(rt, "M-x");
    typing(rt, "terminal-command");
    key(rt, "RET");
    enter_command(rt, script);
}

fn enter_command(rt: &mut Runtime, script: &str) {
    assert!(rt.snapshot().minibuffer.is_some_and(|m| m.prompt == "Terminal command: "));
    typing(rt, script);
    key(rt, "RET");
}

#[test]
fn terminal_runs_a_pty_projects_styles_and_routes_keys() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes");
    std::fs::write(&file, "notes\n").unwrap();
    let mut rt = Runtime::open(&file, &dir.path().join("journal"), "emacs").unwrap().0;
    rt.handle(Input::Viewport { columns: 44, rows: 10, split_gap: 1 });
    command(
        &mut rt,
        r#"stty -echo; printf '\033]2;PTY ready\007\033[38;2;10;20;30m界ready\033[0m\n'; read -r line; stty size; printf 'got:%s\n' "$line""#,
    );
    let first = until(&mut rt, |s| s.pane().text.to_string().contains("界ready"));
    assert!(first.pane().status.contains("PTY ready"));
    assert!(first.pane().layers.iter().any(|h| first.style(h.style).fg.map(|c| c.rgb) == Some([10, 20, 30])));
    key(&mut rt, "<f12>");
    assert!(rt.snapshot().pane().status.contains("COPY"));
    key(&mut rt, "M-<");
    key(&mut rt, "<f12>");
    rt.handle(Input::Viewport { columns: 51, rows: 12, split_gap: 1 });
    typing(&mut rt, "hello");
    key(&mut rt, "RET");
    let last = until(&mut rt, |s| s.pane().status.contains("Exit 0"));
    let text = last.pane().text.to_string();
    assert!(text.contains("10 51") && text.contains("got:hello"), "{text}");
    assert!(rt.take_errors().is_empty());
}

#[test]
fn alignment_pattern_is_projected_on_screen_and_in_scrollback() {
    let mut rt = Runtime::with_document(techne_text::Document::new(""), "emacs").unwrap();
    rt.handle(Input::Viewport { columns: 5, rows: 4, split_gap: 1 });
    command(&mut rt, r#"stty -echo; printf '\033#8'; read -r go; printf '\033[2;1H\n'"#);
    let live = until(&mut rt, |s| s.pane().text.to_string() == "EEEEE\nEEEEE\n");
    assert_eq!(live.pane().scroll, 0);
    key(&mut rt, "RET");
    let done = until(&mut rt, |s| s.pane().status.contains("Exit 0"));
    assert_eq!(done.pane().text.to_string(), "EEEEE\nEEEEE\n     \n");
    assert!(rt.take_errors().is_empty());
}

#[test]
fn eof_retries_output_after_a_timed_draw_fails() {
    let mut rt = Runtime::with_document(techne_text::Document::new(""), "emacs").unwrap();
    rt.eval(
        r#"(eval '(begin
                    (define original-render! render!)
                    (define live-draws 0)
                    (set! render!
                      (lambda (s b state)
                        (when (process state)
                          (set! live-draws (+ 1 live-draws))
                          (when (= live-draws 1) (error "injected draw failure")))
                        (original-render! s b state))))
                 (package-module (find-package 'terminal)))"#,
    )
    .unwrap();
    // No more output after the injected failure. Keep the PTY open until
    // we see that failure, then let the child exit without printing again.
    command(&mut rt, "stty -echo; printf 'unshown tail'; read -r finish");
    let attempts = "(eval 'live-draws (package-module (find-package 'terminal)))";
    let deadline = Instant::now() + Duration::from_secs(10);
    while rt.eval(attempts).unwrap() == "0" {
        assert!(Instant::now() < deadline, "timed draw was not attempted");
        rt.run_tasks(Duration::from_millis(5)).unwrap();
    }
    assert!(!rt.snapshot().pane().text.to_string().contains("unshown tail"));
    key(&mut rt, "RET");
    let done = until(&mut rt, |s| s.pane().status.contains("Exit 0"));
    assert!(done.pane().text.to_string().contains("unshown tail"));
    // The retry must happen before process state is cleared, not as a
    // side effect of rendering the retained buffer in copy mode.
    assert_eq!(rt.eval(attempts).unwrap(), "2");
}

#[test]
fn redraws_making_progress_during_a_burst_and_flush_the_end() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes");
    std::fs::write(&file, "notes\n").unwrap();
    let mut rt = Runtime::open(&file, &dir.path().join("journal"), "emacs").unwrap().0;
    command(
        &mut rt,
        r#"stty -echo; printf 'ready\n'; read -r go; i=0; while [ "$i" -lt 120 ]; do printf 'burst %03d: synthetic kernel log line with some payload\n' "$i"; i=$((i + 1)); sleep .003; done; read -r done; printf '\033[31mfinal tail\033[0m'"#,
    );
    let ready = until(&mut rt, |s| s.pane().text.to_string().contains("ready"));
    key(&mut rt, "RET");
    until(&mut rt, |s| {
        let text = s.pane().text.to_string();
        text.contains("burst 000") && !text.contains("burst 119")
    });
    let burst = until(&mut rt, |s| s.pane().text.to_string().contains("burst 119"));
    // On a quiet host the timer coalesces many individual PTY writes.
    // We intentionally don't assert a wall-clock frame rate.
    assert!(burst.pane().revision - ready.pane().revision < 120 * 2);
    key(&mut rt, "RET");
    let ended = until(&mut rt, |s| s.pane().status.contains("Exit 0"));
    assert!(ended.pane().text.to_string().contains("final tail"));
    assert!(rt.take_errors().is_empty());
}

#[test]
fn the_first_child_command_sees_the_pane_size_and_a_tty() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes");
    std::fs::write(&file, "notes\n").unwrap();
    let mut rt = Runtime::open(&file, &dir.path().join("journal"), "emacs").unwrap().0;
    rt.handle(Input::Viewport { columns: 37, rows: 10, split_gap: 1 });
    command(&mut rt, r#"test -t 1 && test -z "${COLUMNS+x}${LINES+x}" && stty size"#);
    let done = until(&mut rt, |s| s.pane().status.contains("Exit 0"));
    assert!(done.pane().text.to_string().starts_with("8 37"));
    assert!(rt.take_errors().is_empty());
}

#[test]
fn a_left_hand_terminal_uses_the_frontends_split_gap() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes");
    std::fs::write(&file, "notes\n").unwrap();
    let mut rt = Runtime::open(&file, &dir.path().join("journal"), "emacs").unwrap().0;
    rt.handle(Input::Viewport { columns: 100, rows: 24, split_gap: 4 });
    key(&mut rt, "C-x");
    key(&mut rt, "3");
    assert_eq!(rt.snapshot().panes.len(), 2);
    if rt.snapshot().focus != 0 {
        key(&mut rt, "C-x");
        key(&mut rt, "o");
    }
    command(&mut rt, "stty size");
    let done = until(&mut rt, |s| s.pane().status.contains("Exit 0"));
    assert!(done.pane().display.truncate_lines);
    assert!(done.pane().text.to_string().starts_with("22 46"));
}

#[test]
fn successful_shell_exit_closes_the_terminal_unless_it_is_the_last_buffer() {
    for only in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes");
        std::fs::write(&file, "notes\n").unwrap();
        let mut rt = Runtime::open(&file, &dir.path().join("journal"), "emacs").unwrap().0;
        rt.eval("(run-command (current-session) 'terminal 1)").unwrap();
        until(&mut rt, |s| s.pane().status.contains("SEMI"));
        if only {
            rt.eval("(let* ((s (current-session)) (b (current-buffer s))) (for-each (lambda (other) (unless (eq? b other) (drop-buffer! s other))) (buffer-list)))").unwrap();
        }
        typing(&mut rt, "exit 0");
        key(&mut rt, "RET");
        let result = until(&mut rt, |s| s.pane().status.contains("Exit 0") || !s.pane().status.contains("*terminal*"));
        assert_eq!(result.pane().status.contains("*terminal*"), only);
        assert!(rt.take_errors().is_empty());
    }
}

#[test]
fn child_resize_reaches_the_pty_and_survives_editor_keys() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes");
    std::fs::write(&file, "notes\n").unwrap();
    let mut rt = Runtime::open(&file, &dir.path().join("journal"), "emacs").unwrap().0;
    rt.handle(Input::Viewport { columns: 80, rows: 27, split_gap: 1 });
    command(&mut rt, r#"stty -echo; printf '\033[8;12;34tready\n'; read -r line; stty size"#);
    until(&mut rt, |s| s.pane().text.to_string().contains("ready"));
    key(&mut rt, "RET");
    let done = until(&mut rt, |s| s.pane().status.contains("Exit 0"));
    assert!(done.pane().text.to_string().contains("12 34"));
    assert!(rt.take_errors().is_empty());
}

#[test]
fn alternate_screen_hides_scrollback_and_restores_it_on_exit() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes");
    std::fs::write(&file, "notes\n").unwrap();
    let mut rt = Runtime::open(&file, &dir.path().join("journal"), "emacs").unwrap().0;
    rt.handle(Input::Viewport { columns: 40, rows: 6, split_gap: 1 });
    command(
        &mut rt,
        r#"stty -echo; printf 'old\na\nb\nc\nd\nlive\n\033[?1049h\033[?2004h\033[?25lALT'; read -r line; printf '\033[?1049l\033[?25h\n'; test "$line" = "$(printf '\033[200~pasted\033[201~')" || exit 3; printf 'received:pasted\n'"#,
    );
    let alt = until(&mut rt, |s| s.pane().text.to_string().contains("ALT"));
    assert!(!alt.pane().text.to_string().contains("old"));
    rt.handle(Input::Clipboard { text: "pasted".into() });
    key(&mut rt, "C-S-v");
    key(&mut rt, "RET");
    let result = until(&mut rt, |s| s.pane().status.contains("Exit 0"));
    let text = result.pane().text.to_string();
    assert!(text.starts_with("old\na\nb\n"), "{text}");

    assert!(text.contains("received:pasted"), "{text}");
    assert!(!text.contains("ALT"));
    assert!(rt.take_errors().is_empty());
}

#[test]
fn ebb_keys_leave_editor_prefixes_and_allow_literal_terminal_input() {
    for profile in ["emacs", "modal"] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes");
        std::fs::write(&file, "notes\n").unwrap();
        let mut rt = Runtime::open(&file, &dir.path().join("journal"), profile).unwrap().0;
        let script = r#"stty -echo -icanon -isig -ixon; printf 'ready\n'; od -An -t u1 -N 7; printf 'done\n'; read -r line"#;
        if profile == "emacs" {
            command(&mut rt, script);
        } else {
            rt.eval("(run-command (current-session) 'terminal-command 1)").unwrap();
            enter_command(&mut rt, script);
        }
        let ready = until(&mut rt, |s| s.pane().text.to_string().starts_with("ready"));
        assert!(ready.pane().status.contains("SEMI"));
        if profile == "emacs" {
            key(&mut rt, "C-x");
            key(&mut rt, "b");
            let snap = rt.snapshot();
            assert!(
                snap.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Switch to buffer:")),
                "{:?}: {} {:?}",
                snap.minibuffer,
                snap.echo,
                rt.take_errors()
            );
            key(&mut rt, "C-g");
        }
        key(&mut rt, "M-x");
        assert!(rt.snapshot().minibuffer.is_some());
        key(&mut rt, "C-g");
        key(&mut rt, "C-c");
        key(&mut rt, "C-e");
        assert!(rt.snapshot().pane().status.contains("COPY"));
        key(&mut rt, "C-c");
        key(&mut rt, "C-j");
        assert!(rt.snapshot().pane().status.contains("SEMI"));
        key(&mut rt, "C-c");
        key(&mut rt, "M-d");
        assert!(rt.snapshot().pane().status.contains("CHAR"));
        for k in ["C-x", "C-y", "C-c", "C-q"] {
            key(&mut rt, k);
        }
        key(&mut rt, if profile == "emacs" { "C-M-m" } else { "M-RET" });
        assert!(rt.snapshot().pane().status.contains("SEMI"));
        key(&mut rt, "C-q");
        key(&mut rt, "C-x");
        rt.handle(Input::Clipboard { text: "B".into() });
        rt.handle(Input::Clipboard { text: "A".into() });
        key(&mut rt, "C-y");
        key(&mut rt, "M-y");
        let done = until(&mut rt, |s| s.pane().text.to_string().contains("done"));
        let text = done.pane().text.to_string();
        assert!(text.lines().any(|line| line.split_whitespace().eq(["24", "25", "3", "17", "24", "65", "66"])), "{text}");
        key(&mut rt, "C-c");
        key(&mut rt, "C-k");
        let stopped = until(&mut rt, |s| s.pane().status.contains("Child exited"));
        assert!(stopped.pane().text.to_string().contains("done"));
        assert!(rt.take_errors().is_empty());
    }
}

#[test]
fn terminal_interrupts_the_child_and_killing_the_buffer_reaps_it() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes");
    std::fs::write(&file, "notes\n").unwrap();
    let mut rt = Runtime::open(&file, &dir.path().join("journal"), "emacs").unwrap().0;
    command(&mut rt, r#"trap 'printf "interrupted\n"' INT; printf 'pid:%s\n' "$$"; while :; do read -r line; done"#);
    let ready = until(&mut rt, |s| s.pane().text.to_string().starts_with("pid:"));
    let text = ready.pane().text.to_string();
    let pid = text.lines().next().unwrap().trim().strip_prefix("pid:").unwrap().parse::<u32>().unwrap();
    key(&mut rt, "C-c");
    key(&mut rt, "C-c");
    until(&mut rt, |s| s.pane().text.to_string().contains("interrupted"));
    key(&mut rt, "<f12>");
    key(&mut rt, "M-x");
    typing(&mut rt, "kill-buffer");
    key(&mut rt, "RET");
    assert!(!rt.snapshot().pane().status.contains("*terminal*"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::process::Command::new("kill").args(["-0", &pid.to_string()]).stderr(std::process::Stdio::null()).status().unwrap().success()
    {
        assert!(Instant::now() < deadline, "terminal child {pid} survived killing the buffer");
        rt.run_tasks(Duration::from_millis(5)).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(rt.take_errors().is_empty());
}
