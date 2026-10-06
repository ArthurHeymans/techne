//! Runs the editor's Lisp suites (`lisp/editor/tests`) with the editor
//! natives installed; each file evaluates to its number of failed checks.

use std::path::Path;

use techne_vm::vm::Vm;

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
        let failures = vm.eval_file(&suite).unwrap_or_else(|e| panic!("{}: {e}", suite.display()));
        vm.flush();
        assert!(failures.is_int() && failures.as_int() == 0, "{}: {} failed checks", suite.display(), techne_vm::builtins::repr(failures));
    }
}
