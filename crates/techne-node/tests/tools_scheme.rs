//! Cargo bridge only. Fixtures and assertions live in lisp/tests/tools/.
use std::path::Path;
use techne_vm::vm::{Capability, Grants, Vm};

fn run_suite(file: &str, grants: Grants) {
    let mut vm = Vm::with_grants(grants);
    techne_node::install(&mut vm).unwrap();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lisp/tests/tools").join(file);
    match vm.eval_file(&path) {
        Err(error) if error.exit_code() == Some(0) => {}
        Err(error) => panic!("{}: {error}", path.display()),
        Ok(_) => panic!("{} did not finish with test-summary", path.display()),
    }
}

#[test]
fn scheme_tools() {
    run_suite("run.scm", Grants::ALL);
}

#[test]
fn scheme_help_without_file_or_process_grants() {
    // Only the host can choose this world's grants; Scheme cannot grant itself access.
    run_suite("help.scm", Grants::ALL.without(Capability::Files).without(Capability::Processes));
}
