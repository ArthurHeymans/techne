//! Host-managed multi-world editor Lisp tests.
//! Single-world editor suites under `lisp/editor/tests/*.scm` run via `techne test`.

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
