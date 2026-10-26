//! The editor's state kept across runs, in techne in $XDG_STATE_HOME:
//! a test binary of its own, as it sets the variable, which the
//! journals of the files opened follow too.

use std::time::Instant;

use techne_editor::{
    present::Input,
    runtime::{Client, Runtime},
};

/// The frontends keep the minibuffer histories there, in a directory only
/// its user can read.
#[test]
fn histories_are_kept_in_the_state_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    // The only test of this binary.
    unsafe { std::env::set_var("XDG_STATE_HOME", dir.path().join("state")) };
    let mut rt = Runtime::open(&dir.path().join("a.txt"), &dir.path().join("a.journal"), "emacs").unwrap().0;
    rt.keep_histories(None).unwrap();
    for key in ["M-x", "k", "i", "l", "l", "-", "w", "o", "r", "d", "M-RET"] {
        rt.handle(Input::Key { key: key.to_string(), at: Instant::now() });
    }
    rt.detach(Client::FIRST);
    use std::os::unix::fs::PermissionsExt;
    let state = dir.path().join("state/techne");
    assert_eq!(std::fs::metadata(&state).unwrap().permissions().mode() & 0o777, 0o700);
    assert!(std::fs::read_to_string(state.join("history")).unwrap().contains("\"kill-word\""));
}
