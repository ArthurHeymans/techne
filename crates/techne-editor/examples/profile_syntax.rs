//! Headless startup, first snapshot, first edit and first definition jump.
use std::time::Instant;

use techne_editor::{present::Input, runtime::Runtime};

const SOURCE: &str = "fn first() {}\n\nfn second() {\n    let x = 1;\n}\n";

fn key(rt: &mut Runtime, key: &str) {
    rt.handle(Input::Key { key: key.into(), at: Instant::now() });
}

fn main() {
    let dir = tempfile::tempdir().expect("temporary fixture");
    let file = dir.path().join("startup.rs");
    std::fs::write(&file, SOURCE).expect("write fixture");
    let start = Instant::now();
    let (mut rt, _) = Runtime::open(&file, &dir.path().join("startup.journal"), "emacs").expect("open fixture");
    let open = start.elapsed().as_micros();
    eprintln!("profile-syntax: opened");
    let start = Instant::now();
    let before = rt.snapshot();
    let snapshot = start.elapsed().as_micros();
    eprintln!("profile-syntax: initial snapshot");
    assert_eq!(before.pane().text.to_string(), SOURCE);
    key(&mut rt, "M->");
    let start = Instant::now();
    key(&mut rt, "SPC");
    let edited = rt.snapshot();
    let edit = start.elapsed().as_micros();
    eprintln!("profile-syntax: first edit");
    assert_eq!(edited.pane().text.to_string(), format!("{SOURCE} "));
    let start = Instant::now();
    key(&mut rt, "C-M-a");
    let jump = start.elapsed().as_micros();
    assert_eq!(rt.eval("(point (current-session))").expect("point"), SOURCE.find("fn second").unwrap().to_string());
    println!("(syntax {open} {snapshot} {edit} {jump})");
}
