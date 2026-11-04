//! The benchmarks' Scheme scripts (`runtime/bench`), run by `techne run`
//! on the inputs in `tests/bench/`: each must print what the Python it
//! replaced printed for them, in `tests/bench/expected/`. The inputs
//! hold the cases that differed while porting: a command measured twice
//! in one file, a file without results, a zero or missing deviation, an
//! empty table, integers past a double's precision, rounding ties, wrk
//! latencies in each unit and none.

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn input(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/bench").join(name).display().to_string()
}

fn expected(name: &str) -> String {
    std::fs::read_to_string(input(&format!("expected/{name}"))).unwrap()
}

/// What `techne run runtime/bench/SCRIPT ARGS` prints, given STDIN.
fn run(script: &str, args: &[&str], stdin: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_techne"))
        .arg("run")
        .arg(root().join("runtime/bench").join(script))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{script}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn summarize() {
    let out = run("summarize.scm", &[&input("earlier"), &input("later")], "");
    assert_eq!(out, expected("summarize.md"));
}

#[test]
fn http_summarize() {
    assert_eq!(run("http/summarize.scm", &[&input("results.tsv")], ""), expected("http-summarize.md"));
    assert_eq!(run("http/summarize.scm", &[&input("empty.tsv")], ""), expected("http-summarize-empty.md"));
}

#[test]
fn wrk_log() {
    let out = run("http/wrk.scm", &["techne", "1", "/", "16", "0", "9", &input("wrk.txt")], "");
    assert_eq!(out, expected("wrk.tsv"));
    let out = run("http/wrk.scm", &["techne", "1", "/work", "16", "10000", "9", &input("wrk-bare.txt")], "");
    assert_eq!(out, expected("wrk-bare.tsv"));
}

#[test]
fn steady_median() {
    for times in ["odd", "even"] {
        let line = std::fs::read_to_string(input(&format!("times-{times}.txt"))).unwrap();
        assert_eq!(run("steady/median.scm", &[], &line), expected(&format!("median-{times}.txt")), "{times}");
    }
}
