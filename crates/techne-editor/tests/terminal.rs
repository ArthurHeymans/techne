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
fn scheme_terminal_scenarios() {
    let suite = techne_editor::runtime::lisp_dir().join("tests/pty/scenarios.scm");
    for (scenario, profile) in [
        "(osc-prompts)",
        "(alignment)",
        "(initial-size)",
        "(split-size)",
        "(resize)",
        "(shell-exit #f)",
        "(shell-exit #t)",
        "(redraw-retry)",
        "(burst)",
        "(paste)",
        "(private-prompt)",
        "(profile-keys #t)",
    ]
    .into_iter()
    .map(|s| (s, "emacs"))
    .chain([("(profile-keys #f)", "modal")])
    {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes");
        std::fs::write(&file, "notes\n").unwrap();
        let mut rt = Runtime::open(&file, &dir.path().join("journal"), profile).unwrap().0;
        rt.eval(&format!("(require {:?})", suite.display().to_string())).unwrap();
        let result = rt.eval(&format!("(begin {scenario} (test-failures))")).unwrap_or_else(|e| panic!("{scenario}: {e}"));
        assert_eq!(result, "0", "{scenario}");
        assert!(rt.take_errors().is_empty(), "{scenario}");
    }
}

#[test]
fn kitty_inline_chunks_produce_a_grid_image() {
    let mut rt = Runtime::with_document(techne_text::Document::new(""), "emacs").unwrap();
    // A one-pixel red RGBA image from two chunks, then a forbidden file transfer.
    command(
        &mut rt,
        r#"printf '\033_Ga=T,i=7,f=32,s=1,v=1,c=5,r=2,C=1,m=1;/wAA\033\\\033_Gm=0;/w==\033\\\033_Ga=T,t=f,i=8;/wAA/w==\033\\'; read -r go"#,
    );
    let ready = until(&mut rt, |s| !s.pane().blocks.is_empty());
    assert_eq!(ready.pane().blocks.len(), 1);
    let image = &ready.pane().blocks[0];
    assert_eq!(image.grid, Some((0, 5, 2)));
    assert_eq!(image.image.0.rgba, [255, 0, 0, 255]);
    assert!(rt.take_errors().is_empty());
}

#[test]
fn kitty_virtual_rectangle_uses_the_origin_placeholder() {
    let mut rt = Runtime::with_document(techne_text::Document::new(""), "emacs").unwrap();
    command(
        &mut rt,
        r#"printf '\033_Ga=T,q=2,i=7,f=32,s=1,v=1,c=3,r=2,U=1;/wAA/w==\033\\\033[2;4H\033[38;2;0;0;7m\364\216\273\256\314\205\314\205\033[0m'; read -r go"#,
    );
    let ready = until(&mut rt, |s| !s.pane().blocks.is_empty());
    let image = &ready.pane().blocks[0];
    assert_eq!(image.grid, Some((3, 3, 2)));
    assert_eq!(image.at, ready.pane().text.line_to_byte(1));
    assert!(!ready.pane().text.to_string().contains('\u{10eeee}'));
    assert!(rt.take_errors().is_empty());
}

#[test]
fn mouse_and_focus_reports_reach_the_child() {
    let mut rt = Runtime::with_document(techne_text::Document::new(""), "emacs").unwrap();
    command(
        &mut rt,
        r#"stty -icanon -echo min 1 time 0; printf '\033[?1000;1004;1006hready'; head -c 12 | od -An -tu1; printf '\033[?1000;1004;1006l'"#,
    );
    let ready = until(&mut rt, |s| s.pane().display.report_pointer);
    rt.handle(Input::Focus { focused: true });
    rt.handle(Input::Pointer { view: ready.pane().view, column: 3, row: 4, code: 0, release: false });
    let ended = until(&mut rt, |s| s.pane().status.contains("Exit 0"));
    let text = ended.pane().text.to_string();
    assert!(
        text.split_whitespace()
            .collect::<Vec<_>>()
            .windows(12)
            .any(|w| w == ["27", "91", "73", "27", "91", "60", "48", "59", "52", "59", "53", "77"]),
        "{text}"
    );
    assert!(!ended.pane().display.report_pointer);
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
