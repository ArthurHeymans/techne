//! Attaching to an editor of another process, as through ssh: `techne
//! attach --stdio` relays to the editor running on its machine, and with
//! `--start` starts `techne daemon` there when none runs. The machine is
//! this one, the command a plain one instead of ssh.

#![cfg(any(feature = "window", feature = "term", feature = "web"))]

use std::{
    path::Path,
    process::Command,
    sync::mpsc,
    time::{Duration, Instant},
};

use techne_editor::{
    attach::Remote,
    host::Event,
    present::{Input, Output, Snapshot},
};

const TECHNE: &str = env!("CARGO_BIN_EXE_techne");

/// `techne attach --stdio` with `args`, in the runtime directory `run`.
fn relay(run: &Path, state: &Path, ssh: bool, args: &[&str]) -> Vec<String> {
    let mut command = if ssh {
        ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "-T", "-e", "none", "--", "localhost"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    } else {
        Vec::new()
    };
    command.extend([
        "env".to_owned(),
        format!("XDG_RUNTIME_DIR={}", run.display()),
        format!("XDG_STATE_HOME={}", state.display()),
        format!("XDG_CONFIG_HOME={}", state.display()),
        TECHNE.into(),
        "attach".into(),
        "--stdio".into(),
    ]);
    command.extend(args.iter().map(|a| a.to_string()));
    command
}

fn attach(command: &[String], file: Option<&Path>) -> (Remote, mpsc::Receiver<Event>) {
    let (tx, events) = mpsc::channel();
    (Remote::attach_through(command, "emacs", file.map(Path::to_owned), move |e| drop(tx.send(e))).unwrap(), events)
}

fn event<T>(events: &mpsc::Receiver<Event>, mut take: impl FnMut(Event) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let left = deadline.checked_duration_since(Instant::now()).expect("the event in time");
        if let Some(t) = take(events.recv_timeout(left).expect("an event")) {
            return t;
        }
    }
}

fn snapshot(events: &mpsc::Receiver<Event>, done: impl Fn(&Snapshot) -> bool) -> Snapshot {
    event(events, |e| match e {
        Event::Output(Output::Snapshot(s)) if done(&s) => Some(*s),
        Event::Failed(e) => panic!("{e}"),
        _ => None,
    })
}

/// The pid of the daemon of a runtime directory, as its log says it.
fn daemon_pid(run: &Path) -> Option<String> {
    let log = std::fs::read_to_string(run.join("techne/daemon.log")).ok()?;
    let at = log.find("(pid ")? + 5;
    Some(log[at..].split(')').next()?.to_string())
}

/// Stops the daemon of a runtime directory, however the test ends.
struct Daemon(std::path::PathBuf);

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(pid) = daemon_pid(&self.0) {
            let _ = Command::new("kill").args(["-TERM", &pid]).output();
        }
    }
}

#[test]
fn attached_through_a_relay_to_a_daemon_it_started() {
    relay_scenario(false);
}

#[test]
#[ignore = "needs passwordless ssh to localhost"]
fn attached_through_real_ssh_to_a_daemon_it_started() {
    relay_scenario(true);
}

fn relay_scenario(ssh: bool) {
    let run = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let file = state.path().join("draft.txt");
    std::fs::write(&file, "").unwrap();
    let relay = |args: &[&str]| relay(run.path(), state.path(), ssh, args);
    // None runs: the frontend is told so, as an editor would tell it.
    let (_r, events) = attach(&relay(&[]), None);
    let why = event(&events, |e| match e {
        Event::Failed(why) => Some(why),
        _ => None,
    });
    assert!(why.starts_with("no editor is running there"), "{why}");
    // Started.
    let _daemon = Daemon(run.path().to_owned());
    let (mut first, events) = attach(&relay(&["--start"]), Some(&file));
    snapshot(&events, |_| true);
    for k in ["h", "i"] {
        first.send(Input::Key { key: k.into(), at: Instant::now() });
    }
    snapshot(&events, |s| s.pane().text == "hi");
    first.close();
    // The daemon runs on: the next one finds what the first typed.
    let (mut second, events) = attach(&relay(&[]), Some(&file));
    snapshot(&events, |s| s.pane().text == "hi");
    for k in ["C-x", "C-s"] {
        second.send(Input::Key { key: k.into(), at: Instant::now() });
    }
    snapshot(&events, |s| !s.pane().status.contains("[+]"));
    assert_eq!(std::fs::read_to_string(&file).unwrap().trim_end(), "hi");
    // It is stopped by SIGTERM, telling those attached.
    let pid = daemon_pid(run.path()).expect("the daemon's pid in its log");
    assert!(Command::new("kill").args(["-TERM", &pid]).status().unwrap().success());
    let why = event(&events, |e| match e {
        Event::Failed(why) => Some(why),
        _ => None,
    });
    assert_eq!(why, "the editor was stopped");
    second.close();
}
