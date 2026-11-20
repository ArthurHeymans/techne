//! Mail protocol, parsing and persistence policy is exercised as Scheme.
use std::{path::Path, process::Command};

#[test]
fn mail_scheme_suites() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for suite in [
        "packages/mail/tests/mail-mime-tests.scm",
        "packages/mail/tests/mail-backend-tests.scm",
        "packages/mail/tests/mail-smtp-tests.scm",
        "packages/mail/tests/host/mail-store-tests.scm",
        "packages/mail/tests/host/mail-smtp-timeout-tests.scm",
        "packages/mail/tests/mail-date-tests.scm",
        "lisp/tests/authinfo-tests.scm",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_techne")).current_dir(&root).arg("run").arg(root.join(suite)).output().unwrap();
        assert!(
            output.status.success(),
            "{suite}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
