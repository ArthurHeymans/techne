//! Feature-only calibration of census snapshots, including actual JIT bump
//! allocation and the original native fallback under nursery exhaustion.
#![cfg(all(feature = "counting-allocator", feature = "allocation-census"))]
use std::{fs, process::Command};

fn count(line: &str, field: &str) -> u64 {
    let key = format!("{field}: ");
    let value = line.split_once(&key).unwrap_or_else(|| panic!("missing {field}: {line}")).1;
    value.split(|c: char| !c.is_ascii_digit()).next().unwrap().parse().unwrap()
}

#[test]
fn census_counts_inline_and_native_integer_allocations_without_counting_promotion() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("allocations.scm");
    fs::write(
        &file,
        r#"
(define (drive f n)
  (let loop ((i 0) (sum 140737488355328) (held '()))
    (if (= i n) (list sum (length held))
        (loop (+ i 1) (f sum i) (cons i held)))))
(define (probe)
  (drive + 200)
  (gc-stats)
  (let ((answer (drive + 50000))) (gc-stats) answer))
(write (probe))
"#,
    )
    .unwrap();
    for wide in ["0", "1"] {
        let out = Command::new(env!("CARGO_BIN_EXE_techne-vm"))
            .arg(&file)
            .env("TECHNE_ALLOC_CENSUS", "1")
            .env("TECHNE_JIT", "20")
            .env("TECHNE_JIT_SYNC", "1")
            .env("TECHNE_JIT_WIDE_ADD", wide)
            .env("TECHNE_JIT_LOG", "1")
            .env("TECHNE_NURSERY_KB", "64")
            .env("TECHNE_GC_VERIFY", "1")
            .env_remove("TECHNE_GC_STRESS")
            .env_remove("TECHNE_GC_STATS")
            .env_remove("TECHNE_JIT_IR")
            .env_remove("TECHNE_JIT_ASM")
            .env_remove("TECHNE_JIT_STATS")
            .env_remove("TECHNE_JIT_MEMORY")
            .env_remove("TECHNE_JIT_FEEDBACK")
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{log}");
        assert_eq!(out.stdout, b"(140738738330328 50000)");
        assert_eq!(log.contains("jit-wide-add:"), wide == "1", "{log}");
        let snapshots: Vec<_> = log.lines().filter(|l| l.starts_with("allocation-census: ")).collect();
        assert_eq!(snapshots.len(), 3, "{log}");
        assert!(snapshots.iter().all(|l| l.contains("truncated: false")), "{log}");
        let delta = |field| count(snapshots[1], field) - count(snapshots[0], field);
        assert_eq!(delta("pairs"), 50002, "{log}");
        assert_eq!(delta("pair_words"), 150006, "{log}");
        let integers = if wide == "1" { 50000 } else { 100000 };
        assert_eq!(delta("bigints"), integers, "{log}");
        assert_eq!(delta("bigint_words"), integers * 2, "{log}");
        assert_eq!(delta("direct_old_objects"), 0, "promotion must not count again: {log}");
        let gc: Vec<_> = log.lines().filter(|l| l.starts_with("GcStats { ")).collect();
        assert_eq!(gc.len(), 2, "{log}");
        assert!(count(gc[1], "minor") > count(gc[0], "minor"), "exercise nursery exhaustion: {log}");
    }
}

#[test]
fn census_is_absent_until_explicitly_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("no-census.scm");
    fs::write(&file, "(gc-stats) (write (+ 20 22))").unwrap();
    for setting in [None, Some("0"), Some("1")] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_techne-vm"));
        command.arg(&file).env_remove("TECHNE_ALLOC_CENSUS").env_remove("TECHNE_GC_STATS");
        if let Some(setting) = setting {
            command.env("TECHNE_ALLOC_CENSUS", setting);
        }
        let out = command.output().unwrap();
        let log = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{log}");
        assert_eq!(out.stdout, b"42");
        assert_eq!(log.contains("allocation-census:"), setting == Some("1"), "{log}");
    }
}
