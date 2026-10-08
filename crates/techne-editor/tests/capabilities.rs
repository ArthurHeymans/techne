//! Editor file effects obey the embedding world's grants and share file ownership.
use techne_vm::vm::{Grants, Vm};

#[cfg(unix)]
#[test]
fn file_aliases_share_a_document_and_journal_owner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    let link = dir.path().join("link");
    let journal = dir.path().join("journal");
    let other = dir.path().join("other-journal");
    std::fs::write(&path, "text").unwrap();
    std::os::unix::fs::symlink(&path, &link).unwrap();
    let mut vm = Vm::new();
    techne_editor::install(&mut vm);
    let source = format!(
        "(define d (open-document {:?} {:?}))
         (= (document-id d) (document-id (open-document {:?} {:?})))",
        path.to_str().unwrap(),
        journal.to_str().unwrap(),
        link.to_str().unwrap(),
        other.to_str().unwrap()
    );
    assert_eq!(techne_vm::builtins::repr(vm.eval_source(&source).unwrap()), "#t");
    assert!(!other.exists(), "visiting an alias must not start another journal");
}

#[test]
fn file_operations_require_the_files_capability() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    let journal = dir.path().join("journal");
    std::fs::write(&path, "unchanged").unwrap();
    std::fs::write(&journal, "not a journal").unwrap();
    let mut vm = Vm::with_grants(Grants::NONE);
    techne_editor::install(&mut vm);
    for source in [
        format!("(open-file {:?})", path.to_str().unwrap()),
        format!("(open-document {:?} {:?})", path.to_str().unwrap(), journal.to_str().unwrap()),
        "(document-save! (make-document \"text\"))".into(),
        "(document-save-overwriting! (make-document \"text\"))".into(),
    ] {
        let error = vm.eval_source(&source).unwrap_err();
        assert!(error.msg.contains("not granted"), "{source}: {error}");
    }
    assert_eq!(std::fs::read_to_string(path).unwrap(), "unchanged");
    assert_eq!(std::fs::read_to_string(journal).unwrap(), "not a journal");
}
