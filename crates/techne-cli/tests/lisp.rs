//! Runs the Scheme suites under `lisp/tests/net` and `lisp/tests/process`
//! with `techne run`, which has sockets and processes, as `techne-vm`'s
//! suites under `lisp/tests` have not.
use std::{path::Path, process::Command};

fn run_suites_in(subdir: &str) {
    let lisp = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lisp");
    let mut suites: Vec<_> = std::fs::read_dir(lisp.join(subdir))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "scm"))
        .collect();
    suites.sort();
    assert!(!suites.is_empty());
    let bin = env!("CARGO_BIN_EXE_techne");
    for suite in suites {
        let out = Command::new(bin).env("TECHNE_BIN", bin).arg("run").arg(&suite).current_dir(&lisp).output().unwrap();
        assert!(
            out.status.success(),
            "{}:\n{}{}",
            suite.display(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn lisp_net_suites() {
    run_suites_in("tests/net");
}

#[test]
fn lisp_process_suites() {
    run_suites_in("tests/process");
}
