//! Runs the editor's Lisp suites (`lisp/editor/tests`) with the editor
//! natives installed; each file evaluates to its number of failed checks.

use std::path::Path;

use techne_vm::vm::Vm;

/// Two independent attached editor worlds share only a private on-disk store.
#[test]
fn installed_packages() {
    let dir = tempfile::tempdir().unwrap();
    let suite = techne_editor::runtime::lisp_dir().join("tests/packages/install.scm");
    for phase in ["install-and-use", "restore-offline"] {
        let mut rt = techne_editor::runtime::Runtime::with_document(techne_text::Document::new(""), "emacs").unwrap();
        rt.eval(&format!("(require {:?})", suite.display().to_string())).unwrap();
        assert_eq!(rt.eval(&format!("({phase} {:?})", dir.path().display().to_string())).unwrap(), "0", "{phase}");
    }
}

#[test]
fn editor_suites() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lisp/editor/tests");
    let mut suites: Vec<_> =
        std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "scm")).collect();
    suites.sort();
    assert!(!suites.is_empty());
    for suite in suites {
        let mut vm = Vm::new();
        techne_editor::install(&mut vm);
        techne_node::install(&mut vm).unwrap();
        let failures = vm.eval_file(&suite).unwrap_or_else(|e| panic!("{}: {e}", suite.display()));
        vm.flush();
        assert!(failures.as_int() == Some(0), "{}: {} failed checks", suite.display(), vm.repr(&failures));
    }
}
