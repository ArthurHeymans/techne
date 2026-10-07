//! The documentation of everything loaded with the editor (the root
//! module's natives and prelude, the process and node libraries, the
//! editor's Lisp and its packages, Org's), checked by `documentation-problems`
//! (lisp/editor/checkdoc.scm) as the `checkdoc` command checks it:
//! docstrings that are missing or break the convention, keys bound to no
//! command, prefixes without a name. There must be none.

use std::path::Path;

use techne_vm::vm::Vm;

#[test]
fn documentation_problems() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let mut vm = Vm::new();
    techne_editor::install(&mut vm);
    // The node library, with the process library.
    techne_node::install(&mut vm).unwrap();
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
    // The editor's features that are packages, as the runtime loads them.
    for path in techne_editor::runtime::builtin_packages() {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        vm.eval_source(&format!("(load-package '{name} {:?})", path.display().to_string())).unwrap_or_else(|e| panic!("{name}: {e}"));
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
    let found: Vec<String> = problems.iter().map(|p| p.replace(&prefix, "\"")).collect();
    assert!(found.is_empty(), "documentation problems (M-x checkdoc shows them):\n  {}", found.join("\n  "));
}
