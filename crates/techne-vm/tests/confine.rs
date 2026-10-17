//! A world confined as a host confines a deployment's: files read from
//! its directory only, loading stopped, no console.

use techne_vm::vm::{Capability, Grants, Vm};

fn says(vm: &mut Vm, src: &str) -> String {
    match vm.eval_source(src) {
        Ok(v) => vm.repr(&v),
        Err(e) => e.msg.clone(),
    }
}

#[test]
fn reading_within_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let inside = dir.path().join("inside");
    std::fs::create_dir(&inside).unwrap();
    std::fs::write(inside.join("lib.scm"), "(provide x) (define x 1)").unwrap();
    std::fs::write(inside.join("text"), "hello").unwrap();
    std::fs::write(dir.path().join("secret"), "secret").unwrap();
    std::os::unix::fs::symlink(dir.path().join("secret"), inside.join("link")).unwrap();
    let mut vm = Vm::with_grants(Grants::NONE.with(Capability::Loading).with(Capability::Files));
    vm.confine_reading(&inside).unwrap();
    let path = |name: &str| format!("{:?}", inside.join(name).display().to_string());
    assert_eq!(says(&mut vm, &format!("(file->string {})", path("text"))), "\"hello\"");
    assert_eq!(says(&mut vm, &format!("(read-line (open-input-file {}))", path("text"))), "\"hello\"");
    // Outside, through `..` or a link: as if missing.
    for outside in [path("../secret"), path("link"), format!("{:?}", dir.path().join("secret").display().to_string())] {
        for reader in ["file->string", "open-input-file", "open-binary-input-file", "read-lines"] {
            let said = says(&mut vm, &format!("({reader} {outside})"));
            assert!(said.contains("not found"), "{reader} {outside}: {said}");
        }
        let said = says(&mut vm, &format!("(include {outside})"));
        assert!(said.contains("not found"), "include {outside}: {said}");
    }
    // Loading stopped: a module loaded already is still imported; any
    // other fails alike, whether its file exists or not.
    assert_eq!(says(&mut vm, &format!("(require {}) x", path("lib.scm"))), "1");
    vm.set_grant(Capability::Loading, false);
    assert_eq!(says(&mut vm, &format!("(require {}) (+ x 1)", path("lib.scm"))), "2");
    std::fs::write(inside.join("other.scm"), "(define y 2)").unwrap();
    let there = says(&mut vm, &format!("(require {})", path("other.scm")));
    let missing = says(&mut vm, &format!("(require {})", path("missing.scm")));
    assert!(there.contains("not granted"), "{there}");
    assert_eq!(there.replace("other.scm", "missing.scm"), missing);
}

/// Without the console a world's standard input is empty, not the
/// process's: reading it would block the thread.
#[test]
fn no_console() {
    let mut vm = Vm::with_grants(Grants::NONE);
    assert_eq!(says(&mut vm, "(eof-object? (read-line))"), "#t");
}
