//! Run the Scheme report suite against the actual CLI and its recorded inputs.
use std::{path::Path, process::Command};

#[test]
fn scheme_benchmark_reports() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let binary = env!("CARGO_BIN_EXE_techne");
    let status = Command::new(binary).current_dir(root).args(["run", "lisp/tests/host/bench-reports.scm", binary]).status().unwrap();
    assert!(status.success(), "benchmark report suite: {status}");
}
