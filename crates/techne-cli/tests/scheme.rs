//! Unified Cargo bridge for all workspace Scheme test suites (`techne test`).
//!
//! Discovers every Scheme test file via `techne test --all --list --format tsv`
//! (receiving exact canonical paths and metadata) and runs each applicable
//! `(file, mode)` as its own `libtest-mimic` trial, preserving `#:serial`
//! scheduling with a process-wide mutex.
use std::{path::Path, process::Command, sync::Mutex};

use libtest_mimic::{Arguments, Failed, Trial};

static SERIAL_LOCK: Mutex<()> = Mutex::new(());

struct Entry {
    id: String,
    path: String,
    env: String,
    gc_stress: bool,
    serial: bool,
}

fn main() {
    let args = Arguments::from_args();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bin = env!("CARGO_BIN_EXE_techne");

    let out = Command::new(bin)
        .current_dir(&root)
        .args(["test", "--all", "--list", "--format", "tsv"])
        .output()
        .expect("failed to invoke `techne test --all --list --format tsv`");
    assert!(
        out.status.success(),
        "techne test --list failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let entries: Vec<Entry> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let mut cols = line.split('\t');
            let id = cols.next().expect("tsv id").to_owned();
            let path = cols.next().expect("tsv path").to_owned();
            let env = cols.next().expect("tsv env").to_owned();
            let gc_stress = cols.next().expect("tsv gc-stress") == "true";
            let serial = cols.next().expect("tsv serial") == "true";
            Entry { id, path, env, gc_stress, serial }
        })
        .collect();
    assert!(!entries.is_empty(), "techne test --list returned no test files");

    let modes: Vec<String> = match std::env::var("TECHNE_TEST_MODES") {
        Ok(list) if !list.trim().is_empty() => list.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect(),
        _ => vec!["default".to_owned(), "gc".to_owned()],
    };

    let mut trials = Vec::new();
    for entry in entries {
        for mode in &modes {
            if mode == "gc" && !entry.gc_stress {
                continue;
            }
            if mode != "default" && mode != "gc" && entry.env != "vm" {
                continue;
            }
            let root = root.clone();
            let id_run = entry.id.clone();
            let path_run = entry.path.clone();
            let mode_run = mode.clone();
            let serial = entry.serial;
            trials.push(
                Trial::test(entry.id.clone(), move || run_one(&root, bin, &id_run, &path_run, &mode_run, serial)).with_kind(mode.clone()),
            );
        }
    }

    libtest_mimic::run(&args, trials).exit();
}

fn run_one(root: &Path, bin: &str, id: &str, path: &str, mode: &str, serial: bool) -> Result<(), Failed> {
    let _guard = if serial { Some(SERIAL_LOCK.lock().unwrap_or_else(|e| e.into_inner())) } else { None };
    let out = Command::new(bin)
        .current_dir(root)
        .args(["test", "--mode", mode, path])
        .output()
        .map_err(|e| Failed::from(format!("spawn techne test: {e}")))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(Failed::from(format!("{id} [{mode}]:\n{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))))
    }
}
