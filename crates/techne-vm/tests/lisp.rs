//! Runs the Scheme test suites under `lisp/tests` and under each package's
//! `tests` (`packages/*/tests`; each exits non-zero on a failed check),
//! normally and with a minor GC on every allocation. A package's suites
//! import its libraries as any file in its directory does: the program
//! finds the manifest above them (`techne_vm::cli::use_package_of`).
use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn suites_in(tests: &Path) -> Vec<PathBuf> {
    let mut suites: Vec<_> =
        std::fs::read_dir(tests).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "scm")).collect();
    suites.sort();
    suites
}

#[test]
fn lisp_suites() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut packages: Vec<_> =
        std::fs::read_dir(root.join("packages")).unwrap().map(|e| e.unwrap().path().join("tests")).filter(|p| p.is_dir()).collect();
    packages.sort();
    assert!(!packages.is_empty());
    for tests in std::iter::once(root.join("lisp/tests")).chain(packages) {
        let suites = suites_in(&tests);
        assert!(!suites.is_empty(), "{}", tests.display());
        for (suite, stress) in suites.iter().flat_map(|suite| [None, Some("1")].map(|stress| (suite, stress))) {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_techne-vm"));
            cmd.arg(suite).current_dir(tests.parent().unwrap());
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
