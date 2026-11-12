//! The documentation of everything loaded with the editor (the root
//! module's natives and prelude, the network, process and node libraries,
//! the editor's Lisp and its packages, Org's), checked by `documentation-problems`
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
    techne_net::install(&mut vm).unwrap();
    // The node library, with the process library.
    techne_node::install(&mut vm).unwrap();
    techne_editor::runtime::load_lisp(&mut vm).unwrap();
    for file in [
        "lisp/editor/checkdoc.scm",
        "lisp/org/org.scm",
        "lisp/org/agenda.scm",
        "lisp/test.scm",
        "lisp/schema.scm",
        "lisp/http/http.scm",
        "lisp/http/server.scm",
        "lisp/http/debug-route.scm",
        "lisp/cloud/kv.scm",
        "lisp/cloud/manifest.scm",
        "lisp/cloud/tokens.scm",
        "lisp/cloud/auth.scm",
        "lisp/cloud/control.scm",
        "lisp/cloud/hosts.scm",
        "lisp/cloud/serve.scm",
    ] {
        vm.eval_source(&format!("(require {:?})", root.join(file).display().to_string())).unwrap_or_else(|e| panic!("{file}: {e}"));
    }
    vm.eval_source("(import (techne tools) (techne source) (techne path) (techne time) (techne format))").unwrap();
    let problems: Vec<String> = vm
        .eval_source(
            "(map (lambda (p) (string-append (repr (problem-module p)) \" \" (symbol->string (problem-subject p))
                                            \" \" (symbol->string (problem-name p)) \": \" (problem-text p)))
                  (documentation-problems))",
        )
        .and_then(|v| vm.get(&v))
        .unwrap();
    let prefix = format!("\"{}/", root.display());
    let found: Vec<String> = problems.iter().map(|p| p.replace(&prefix, "\"")).collect();
    assert!(found.is_empty(), "documentation problems (M-x checkdoc shows them):\n  {}", found.join("\n  "));
}
