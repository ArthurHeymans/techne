//! Runs the Scheme suites under `lisp/tests/net` with `techne run`, which
//! has sockets and processes, as `techne-vm`'s suites under `lisp/tests`
//! have not.
use std::{path::Path, process::Command};

#[test]
fn lisp_net_suites() {
    let lisp = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lisp");
    let mut suites: Vec<_> = std::fs::read_dir(lisp.join("tests/net"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "scm"))
        .collect();
    suites.sort();
    assert!(!suites.is_empty());
    for suite in suites {
        let out = Command::new(env!("CARGO_BIN_EXE_techne")).arg("run").arg(&suite).current_dir(&lisp).output().unwrap();
        assert!(
            out.status.success(),
            "{}:\n{}{}",
            suite.display(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
