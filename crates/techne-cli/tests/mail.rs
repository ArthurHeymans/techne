//! Mail protocol, parsing and persistence policy is exercised as Scheme.
use std::{path::Path, process::Command};

#[test]
fn mail_scheme_suites() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for suite in [
        "mail-mime-tests.scm",
        "mail-backend-tests.scm",
        "mail-smtp-tests.scm",
        "host/mail-store-tests.scm",
        "host/mail-smtp-timeout-tests.scm",
        "mail-date-tests.scm",
        "authinfo-tests.scm",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_techne"))
            .current_dir(&root)
            .arg("run")
            .arg(root.join("lisp/tests").join(suite))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{suite}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
