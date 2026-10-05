//! Runs the Scheme test suites under `lisp/tests` (each exits non-zero on a
//! failed check), normally and with a minor GC on every allocation.
use std::{path::Path, process::Command};

#[test]
fn lisp_suites() {
    let lisp = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lisp");
    let mut suites: Vec<_> = std::fs::read_dir(lisp.join("tests"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "scm"))
        .collect();
    suites.sort();
    assert!(!suites.is_empty());
    for suite in suites {
        for stress in [None, Some("1")] {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_techne-vm"));
            cmd.arg(&suite).current_dir(&lisp);
            if let Some(mode) = stress {
                cmd.env("TECHNE_GC_STRESS", mode);
            }
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "{} (stress={stress:?}):\n{}{}",
                suite.display(),
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}
