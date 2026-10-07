//! The documentation of everything loaded with the editor (the root
//! module's natives and prelude, the editor's Lisp, Org's), checked by
//! `documentation-problems` (lisp/editor/checkdoc.scm) as the `checkdoc`
//! command checks it: docstrings that are missing or break the
//! convention, keys bound to no command, prefixes without a name.
//!
//! `documentation-problems.txt` lists what is wrong today, as
//! tests/suites/expected-failures.txt does for the Scheme suites: any
//! change to that set fails, so it only shrinks. `TECHNE_BLESS=1`
//! rewrites it from the run.

use std::path::Path;

use techne_vm::vm::Vm;

#[test]
fn documentation_problems() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let mut vm = Vm::new();
    techne_editor::install(&mut vm);
    techne_process::install(&mut vm).unwrap();
    for file in [
        "lisp/editor/main.scm",
        "lisp/editor/api.scm",
        "lisp/editor/checkdoc.scm",
        "lisp/org/org.scm",
        "lisp/org/agenda.scm",
        "lisp/test.scm",
    ] {
        vm.eval_source(&format!("(require {:?})", root.join(file).display().to_string())).unwrap_or_else(|e| panic!("{file}: {e}"));
    }
    let problems: Vec<String> = vm
        .eval_source(
            "(map (lambda (p) (string-append (repr (problem-module p)) \" \" (symbol->string (problem-subject p))
                                            \" \" (symbol->string (problem-name p)) \": \" (problem-text p)))
                  (documentation-problems))",
        )
        .and_then(|v| vm.get(v))
        .unwrap();
    let prefix = format!("\"{}/", root.display());
    let mut found: Vec<String> = problems.iter().map(|p| p.replace(&prefix, "\"")).collect();
    found.sort();
    found.dedup();
    let expected_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/documentation-problems.txt");
    if std::env::var_os("TECHNE_BLESS").is_some() {
        std::fs::write(&expected_file, found.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        return;
    }
    let expected: Vec<String> = std::fs::read_to_string(&expected_file).unwrap_or_default().lines().map(str::to_string).collect();
    let new: Vec<&String> = found.iter().filter(|p| !expected.contains(p)).collect();
    let fixed: Vec<&String> = expected.iter().filter(|p| !found.contains(p)).collect();
    assert!(
        new.is_empty() && fixed.is_empty(),
        "documentation problems changed (TECHNE_BLESS=1 rewrites {}):\nnew:\n  {}\nfixed:\n  {}",
        expected_file.display(),
        new.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n  "),
        fixed.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n  "),
    );
}
