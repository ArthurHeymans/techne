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

/// Snapshotting a world that has `Capability::Files` and `confine_reading(root)`
/// after revoking `Capability::Loading` must preserve `ReadingAccess::Confined(root)`
/// and never widen to unrestricted OS reads.
#[test]
fn snapshot_preserves_reading_confinement_when_loading_revoked() {
    let dir = tempfile::tempdir().unwrap();
    let inside = dir.path().join("inside");
    std::fs::create_dir(&inside).unwrap();
    std::fs::write(inside.join("ok.txt"), "inside-ok").unwrap();
    std::fs::write(dir.path().join("secret.txt"), "outside-secret").unwrap();

    let mut vm = Vm::with_grants(Grants::NONE.with(Capability::Loading).with(Capability::Files));
    vm.confine_reading(&inside).unwrap();
    vm.set_grant(Capability::Loading, false);

    let snap = vm.snapshot().expect("snapshot with Files + Confined must succeed");
    let mut world = snap.world();

    let ok_path = techne_vm::reader::string_repr(&inside.join("ok.txt").display().to_string());
    let secret_path = techne_vm::reader::string_repr(&dir.path().join("secret.txt").display().to_string());
    assert_eq!(says(&mut world, &format!("(file->string {ok_path})")), "\"inside-ok\"");
    for op in ["file->string", "open-input-file", "open-binary-input-file"] {
        let said = says(&mut world, &format!("({op} {secret_path})"));
        assert!(said.contains("not found"), "snapshot must preserve confinement for {op}: {said}");
    }

    // Without Files or Loading, snapshot sets DenyAll (never Unrestricted).
    let denied_vm = Vm::with_grants(Grants::NONE);
    let denied_snap = denied_vm.snapshot().expect("snapshot with Grants::NONE");
    let mut denied_world = denied_snap.world();
    let said = says(&mut denied_world, &format!("(open-input-file {ok_path})"));
    assert!(said.contains("not granted") || said.contains("denied") || said.contains("not found"), "expected refusal, got: {said}");
}

/// Installing a `BundleVfs` revokes OS file access so file ports cannot bypass
/// the virtual bundle to read or write real host paths.
#[test]
fn bundle_vfs_refuses_os_file_ports() {
    use std::{collections::BTreeMap, sync::Arc};
    use techne_vm::bundle::BundleVfs;

    let dir = tempfile::tempdir().unwrap();
    let host_file = dir.path().join("host-secret.txt");
    let write_target = dir.path().join("should-not-exist.txt");
    std::fs::write(&host_file, "top-secret").unwrap();

    let mut root_files = BTreeMap::new();
    root_files.insert("app.scm".into(), Arc::<[u8]>::from(&b"(define ok #t)"[..]));
    let bundle = Arc::new(BundleVfs::new(root_files, None, BTreeMap::new()).expect("valid legacy bundle"));
    let mut vm = Vm::with_grants(Grants::NONE.with(Capability::Loading).with(Capability::Files));
    vm.install_bundle(bundle);

    let host_str = techne_vm::reader::string_repr(&host_file.display().to_string());
    let write_str = techne_vm::reader::string_repr(&write_target.display().to_string());
    for expr in [
        format!("(open-input-file {host_str})"),
        format!("(open-binary-input-file {host_str})"),
        format!("(file->string {host_str})"),
        format!("(open-output-file {write_str})"),
        format!("(open-binary-output-file {write_str})"),
        format!("(file-exists? {host_str})"),
        format!("(directory-list {})", techne_vm::reader::string_repr(&dir.path().display().to_string())),
    ] {
        let err = vm.eval_source(&expr).expect_err(&format!("OS file op must be refused under BundleVfs: {expr}"));
        assert!(
            err.msg.contains("not permitted") || err.msg.contains("not granted") || err.msg.contains("bundle"),
            "{expr}: unexpected error: {}",
            err.msg
        );
    }
    assert!(!write_target.exists(), "open-output-file must not create host file under BundleVfs");
}
