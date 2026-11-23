//! Hermetic end-to-end tests for Phase 2 packages:
//! `techne lock`, `techne lock --check`, `techne run --locked`,
//! canonical `tree-sha256`, cross-repo Git dependencies (single & monorepo),
//! diamond-conflict rejection, root `(override ...)`, cycles, and symlinks.
//!
//! Uses only temporary local Git repositories — zero network access.

use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be,
        0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa,
        0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85,
        0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
        0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f,
        0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (dst, add) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *dst = dst.wrapping_add(add);
        }
    }
    h.iter().map(|w| format!("{w:08x}")).collect()
}

fn techne() -> Command {
    Command::new(env!("CARGO_BIN_EXE_techne"))
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(dir).env("GIT_TERMINAL_PROMPT", "0").output().expect("spawn git");
    assert!(
        out.status.success(),
        "git {:?} failed in {}:\n{}\n{}",
        args,
        dir.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git_init_commit(dir: &Path, msg: &str) -> String {
    if !dir.join(".git").exists() {
        git(dir, &["init", "--quiet"]);
    }
    git(dir, &["add", "."]);
    git(dir, &["-c", "user.name=Test", "-c", "user.email=test@example.org", "-c", "commit.gpgSign=false", "commit", "-qm", msg]);
    git(dir, &["rev-parse", "HEAD"])
}

fn write_file(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

/// Independent Rust implementation of the canonical `tree-sha256` spec
/// (`"F <relpath>\0<byte-len>\0<file-bytes>\n"`, sorted UTF-8 POSIX paths,
/// excluding dot-prefixed entries) to assert bit-for-bit equality with Scheme.
fn rust_tree_sha256(dir: &Path) -> String {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    fn walk(cur: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
        for entry in std::fs::read_dir(cur).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            assert!(!meta.file_type().is_symlink(), "unexpected symlink: {}", path.display());
            let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            if meta.is_dir() {
                walk(&path, &rel, out);
            } else if meta.is_file() {
                out.push((rel, path));
            }
        }
    }
    walk(dir, "", &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut buf = Vec::new();
    for (rel, path) in files {
        let bytes = std::fs::read(&path).unwrap();
        buf.extend_from_slice(b"F ");
        buf.extend_from_slice(rel.as_bytes());
        buf.push(0);
        buf.extend_from_slice(bytes.len().to_string().as_bytes());
        buf.push(0);
        buf.extend_from_slice(&bytes);
        buf.push(b'\n');
    }
    sha256_hex(&buf)
}

fn assert_ok(out: &Output, label: &str) {
    assert!(
        out.status.success(),
        "{label} failed ({}):\nstdout:\n{}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn stderr_str(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn lock_and_run_locked_offline_single_and_monorepo() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let store = root.join("store");

    // 1. Single-package Git repo (acme util), including dotfiles that must be ignored in tree-sha256.
    let util_repo = root.join("util-repo");
    write_file(&util_repo, "techne-package.scm", "(package (acme util) (version \"0.2.0\"))");
    write_file(
        &util_repo,
        "lib/acme/util/math.sld",
        "(define-library (acme util math) (export add1) (import (scheme base)) (begin (define (add1 x) (+ x 1))))",
    );
    write_file(&util_repo, ".secrets.scm", "(secret \"must-not-be-hashed\")");
    write_file(&util_repo, ".hidden/ignored.txt", "ignored");
    let util_oid = git_init_commit(&util_repo, "util v1");
    git(&util_repo, &["-c", "tag.gpgSign=false", "tag", "v1"]);
    let expected_util_hash = rust_tree_sha256(&util_repo);

    // 2. Monorepo holding (acme mono-a) -> intra-repo (acme mono-b) AND cross-repo (acme util).
    let mono_repo = root.join("mono-repo");
    write_file(&mono_repo, "packages/mono-b/techne-package.scm", "(package (acme mono-b) (version \"0.1.0\"))");
    write_file(
        &mono_repo,
        "packages/mono-b/lib/acme/mono-b/core.sld",
        "(define-library (acme mono-b core) (export mul2) (import (scheme base)) (begin (define (mul2 x) (* x 2))))",
    );
    write_file(
        &mono_repo,
        "packages/mono-a/techne-package.scm",
        &format!(
            "(package (acme mono-a)
               (version \"0.3.0\")
               (depends
                 ((acme mono-b) (path \"../mono-b\"))
                 ((acme util) (local {:?} (tag \"v1\")))))",
            util_repo.display().to_string()
        ),
    );
    write_file(
        &mono_repo,
        "packages/mono-a/lib/acme/mono-a/api.sld",
        "(define-library (acme mono-a api)
           (export compute)
           (import (scheme base) (acme mono-b core) (acme util math))
           (begin (define (compute x) (mul2 (add1 x)))))",
    );
    let mono_oid = git_init_commit(&mono_repo, "mono initial");
    let expected_mono_a_hash = rust_tree_sha256(&mono_repo.join("packages/mono-a"));
    let expected_mono_b_hash = rust_tree_sha256(&mono_repo.join("packages/mono-b"));

    // 3. Root project (acme app) with a cloud clause (no editor clause) + script.
    let app = root.join("app");
    let app_manifest = format!(
        "(package (acme app)
           (version \"1.0.0\")
           (techne \"0.4\")
           (depends
             ((acme mono-a) (local {:?} (package (acme mono-a))))
             ((acme util) (local {:?} (tag \"v1\"))))
           (cloud \"main.scm\" (bindings (kv-store kv))))",
        mono_repo.display().to_string(),
        util_repo.display().to_string()
    );
    write_file(&app, "techne-package.scm", &app_manifest);
    write_file(
        &app,
        "main.scm",
        "(import (scheme base) (scheme write) (acme mono-a api) (acme util math))
         (write (list (compute 10) (add1 5) (cdr (command-line))))
         (newline)",
    );
    write_file(&app, "bad-transitive.scm", "(import (scheme base) (acme mono-b core)) (mul2 3)");

    // Before locking, `techne run --locked` must refuse to run.
    let missing_lock = techne().args(["run", "--locked", "--store"]).arg(&store).arg(app.join("main.scm")).output().unwrap();
    assert!(!missing_lock.status.success(), "expected failure without techne.lock");
    assert!(stderr_str(&missing_lock).contains("Missing lockfile"), "{}", stderr_str(&missing_lock));

    // Run `techne lock`.
    let locked = techne().args(["lock"]).arg(&app).arg("--store").arg(&store).output().unwrap();
    assert_ok(&locked, "techne lock");

    // Verify techne.lock contents and exact Rust/Scheme tree-sha256 bit-for-bit equality.
    let lock_text = std::fs::read_to_string(app.join("techne.lock")).unwrap();
    assert!(lock_text.contains(&format!("(commit \"{util_oid}\")")), "{lock_text}");
    assert!(lock_text.contains(&format!("(commit \"{mono_oid}\")")), "{lock_text}");
    assert!(lock_text.contains("(subdir \"packages/mono-a\")"), "{lock_text}");
    assert!(lock_text.contains("(subdir \"packages/mono-b\")"), "{lock_text}");
    assert!(
        lock_text.contains(&format!("(tree-sha256 \"{expected_util_hash}\")")),
        "Scheme vs Rust util tree-sha256 mismatch:\n{lock_text}"
    );
    assert!(
        lock_text.contains(&format!("(tree-sha256 \"{expected_mono_a_hash}\")")),
        "Scheme vs Rust mono-a tree-sha256 mismatch:\n{lock_text}"
    );
    assert!(
        lock_text.contains(&format!("(tree-sha256 \"{expected_mono_b_hash}\")")),
        "Scheme vs Rust mono-b tree-sha256 mismatch:\n{lock_text}"
    );
    // Verify hidden files were NOT copied into the content-addressed store.
    assert!(!store.join(&expected_util_hash).join(".secrets.scm").exists());
    assert!(!store.join(&expected_util_hash).join(".git").exists());

    // `techne lock --check` succeeds.
    let check_ok = techne().args(["lock"]).arg(&app).args(["--check", "--store"]).arg(&store).output().unwrap();
    assert_ok(&check_ok, "techne lock --check");

    // Delete BOTH Git repositories completely to guarantee 100% offline execution!
    std::fs::remove_dir_all(&util_repo).unwrap();
    std::fs::remove_dir_all(&mono_repo).unwrap();

    // `techne run --locked` runs strictly offline and passes script arguments.
    let run_ok =
        techne().args(["run", "--locked", "--store"]).arg(&store).arg(app.join("main.scm")).args(["alpha", "beta"]).output().unwrap();
    assert_ok(&run_ok, "techne run --locked offline");
    assert_eq!(String::from_utf8_lossy(&run_ok.stdout).trim(), "(22 6 (\"alpha\" \"beta\"))");

    // `--package DIR` lets an external script run against the locked package world.
    let ext_script = root.join("external.scm");
    std::fs::write(&ext_script, "(import (scheme base) (scheme write) (acme util math)) (write (add1 99)) (newline)").unwrap();
    let run_ext = techne().args(["run", "--locked", "--package"]).arg(&app).arg("--store").arg(&store).arg(&ext_script).output().unwrap();
    assert_ok(&run_ext, "techne run --locked --package");
    assert_eq!(String::from_utf8_lossy(&run_ext.stdout).trim(), "100");

    // Importing an undeclared transitive dependency ((acme mono-b)) is refused.
    let bad_trans = techne().args(["run", "--locked", "--store"]).arg(&store).arg(app.join("bad-transitive.scm")).output().unwrap();
    assert!(!bad_trans.status.success(), "undeclared transitive import should fail");

    // Modifying techne-package.scm so locked entries become unreachable makes techne.lock stale -> `--check` fails without mutating techne.lock.
    let stale_manifest = format!(
        "(package (acme app)
           (version \"1.0.0\")
           (depends
             ((acme util) (local {:?} (tag \"v1\")))))",
        util_repo.display().to_string()
    );
    write_file(&app, "techne-package.scm", &stale_manifest);
    let check_stale = techne().args(["lock"]).arg(&app).args(["--check", "--store"]).arg(&store).output().unwrap();
    assert!(!check_stale.status.success(), "stale lockfile (unused entries) must fail --check");
    assert!(stderr_str(&check_stale).contains("stale"), "{}", stderr_str(&check_stale));
    assert_eq!(std::fs::read_to_string(app.join("techne.lock")).unwrap(), lock_text, "--check must not modify techne.lock");

    // Adding an unlocked dependency also makes techne.lock stale.
    let extra_dep_manifest = format!(
        "(package (acme app)
           (version \"1.0.0\")
           (depends
             ((acme mono-a) (local {:?} (package (acme mono-a))))
             ((acme util) (local {:?} (tag \"v1\")))
             ((acme extra) (path \"../extra\"))))",
        mono_repo.display().to_string(),
        util_repo.display().to_string()
    );
    write_file(&app, "techne-package.scm", &extra_dep_manifest);
    let check_missing_dep = techne().args(["lock"]).arg(&app).args(["--check", "--store"]).arg(&store).output().unwrap();
    assert!(!check_missing_dep.status.success(), "stale lockfile (missing dep) must fail --check");
    assert!(stderr_str(&check_missing_dep).contains("missing dependency (acme extra)"), "{}", stderr_str(&check_missing_dep));

    // Restore manifest, then tamper with a file inside store/<tree-sha256>/ -> both --check and run --locked reject it!
    write_file(&app, "techne-package.scm", &app_manifest);
    let util_store = store.join(&expected_util_hash);
    std::fs::write(
        util_store.join("lib/acme/util/math.sld"),
        "(define-library (acme util math) (export add1) (import (scheme base)) (begin (define (add1 x) 999)))",
    )
    .unwrap();
    let check_tampered = techne().args(["lock"]).arg(&app).args(["--check", "--store"]).arg(&store).output().unwrap();
    assert!(!check_tampered.status.success(), "tampered store must fail --check");
    assert!(stderr_str(&check_tampered).contains("tree-sha256 mismatch"), "{}", stderr_str(&check_tampered));

    let run_tampered = techne().args(["run", "--locked", "--store"]).arg(&store).arg(app.join("main.scm")).output().unwrap();
    assert!(!run_tampered.status.success(), "tampered store must fail run --locked");
    assert!(stderr_str(&run_tampered).contains("tree-sha256 mismatch"), "{}", stderr_str(&run_tampered));
}

#[test]
fn diamond_conflict_names_both_chains_and_override_resolves_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let store = root.join("store");

    // Shared dep (acme util) with v1 and v2 tags.
    let util_repo = root.join("util");
    write_file(&util_repo, "techne-package.scm", "(package (acme util) (version \"1.0.0\"))");
    write_file(
        &util_repo,
        "lib/acme/util/v.sld",
        "(define-library (acme util v) (export util-val) (import (scheme base)) (begin (define (util-val) 'v1)))",
    );
    git_init_commit(&util_repo, "v1");
    git(&util_repo, &["-c", "tag.gpgSign=false", "tag", "v1"]);

    write_file(&util_repo, "techne-package.scm", "(package (acme util) (version \"2.0.0\"))");
    write_file(
        &util_repo,
        "lib/acme/util/v.sld",
        "(define-library (acme util v) (export util-val) (import (scheme base)) (begin (define (util-val) 'v2)))",
    );
    git_init_commit(&util_repo, "v2");
    git(&util_repo, &["-c", "tag.gpgSign=false", "tag", "v2"]);

    // (acme left) -> (acme util) @ v1
    let left_repo = root.join("left");
    write_file(
        &left_repo,
        "techne-package.scm",
        &format!(
            "(package (acme left) (version \"0.1.0\") (depends ((acme util) (local {:?} (tag \"v1\")))))",
            util_repo.display().to_string()
        ),
    );
    write_file(
        &left_repo,
        "lib/acme/left/api.sld",
        "(define-library (acme left api) (export left-v) (import (scheme base) (acme util v)) (begin (define (left-v) (util-val))))",
    );
    git_init_commit(&left_repo, "left");

    // (acme right) -> (acme util) @ v2
    let right_repo = root.join("right");
    write_file(
        &right_repo,
        "techne-package.scm",
        &format!(
            "(package (acme right) (version \"0.1.0\") (depends ((acme util) (local {:?} (tag \"v2\")))))",
            util_repo.display().to_string()
        ),
    );
    write_file(
        &right_repo,
        "lib/acme/right/api.sld",
        "(define-library (acme right api) (export right-v) (import (scheme base) (acme util v)) (begin (define (right-v) (util-val))))",
    );
    git_init_commit(&right_repo, "right");

    // Root (acme app) depends on both (acme left) and (acme right).
    let app = root.join("app");
    write_file(
        &app,
        "techne-package.scm",
        &format!(
            "(package (acme app)
               (version \"0.1.0\")
               (depends
                 ((acme left) (local {:?}))
                 ((acme right) (local {:?}))))",
            left_repo.display().to_string(),
            right_repo.display().to_string()
        ),
    );
    write_file(
        &app,
        "main.scm",
        "(import (scheme base) (scheme write) (acme left api) (acme right api))
         (write (list (left-v) (right-v)))
         (newline)",
    );

    // Without override: diamond conflict MUST fail loudly naming BOTH full chains.
    let conflict = techne().args(["lock"]).arg(&app).arg("--store").arg(&store).output().unwrap();
    assert!(!conflict.status.success(), "diamond conflict must fail");
    let err = stderr_str(&conflict);
    assert!(err.contains("Diamond dependency conflict for (acme util)"), "{err}");
    assert!(err.contains("(acme app) -> (acme left) -> (acme util)"), "missing left chain:\n{err}");
    assert!(err.contains("(acme app) -> (acme right) -> (acme util)"), "missing right chain:\n{err}");
    assert!(!app.join("techne.lock").exists(), "failed lock must not write techne.lock");

    // Add root `(override ...)` selecting v2 -> resolves cleanly!
    write_file(
        &app,
        "techne-package.scm",
        &format!(
            "(package (acme app)
               (version \"0.1.0\")
               (depends
                 ((acme left) (local {:?}))
                 ((acme right) (local {:?})))
               (override
                 ((acme util) (local {:?} (tag \"v2\")))))",
            left_repo.display().to_string(),
            right_repo.display().to_string(),
            util_repo.display().to_string()
        ),
    );
    let resolved = techne().args(["lock"]).arg(&app).arg("--store").arg(&store).output().unwrap();
    assert_ok(&resolved, "techne lock with git override");

    // Also test root `(override ...)` pointing to a local `(path ...)` checkout!
    let local_override = root.join("util-override");
    write_file(&local_override, "techne-package.scm", "(package (acme util) (version \"9.9.9\"))");
    write_file(
        &local_override,
        "lib/acme/util/v.sld",
        "(define-library (acme util v) (export util-val) (import (scheme base)) (begin (define (util-val) 'overridden)))",
    );

    // Delete Git repos first to prove the git-override lock runs offline.
    std::fs::remove_dir_all(&util_repo).unwrap();
    std::fs::remove_dir_all(&left_repo).unwrap();
    std::fs::remove_dir_all(&right_repo).unwrap();

    let run_v2 = techne().args(["run", "--locked", "--store"]).arg(&store).arg(app.join("main.scm")).output().unwrap();
    assert_ok(&run_v2, "run --locked with v2 override offline");
    assert_eq!(String::from_utf8_lossy(&run_v2.stdout).trim(), "(v2 v2)");

    // Switch override to local path and re-check with a fresh local-path pair.
    let p_app = root.join("path-app");
    let p_mid = root.join("path-mid");
    write_file(&p_mid, "techne-package.scm", "(package (acme mid) (version \"0.1.0\") (depends ((acme util) (path \"../missing-util\"))))");
    write_file(
        &p_mid,
        "lib/acme/mid/api.sld",
        "(define-library (acme mid api) (export mid-v) (import (scheme base) (acme util v)) (begin (define (mid-v) (util-val))))",
    );
    write_file(
        &p_app,
        "techne-package.scm",
        "(package (acme papp)
           (version \"0.1.0\")
           (depends ((acme mid) (path \"../path-mid\")))
           (override ((acme util) (path \"../util-override\"))))",
    );
    write_file(&p_app, "main.scm", "(import (scheme base) (scheme write) (acme mid api)) (write (mid-v)) (newline)");
    let locked_path = techne().args(["lock"]).arg(&p_app).arg("--store").arg(&store).output().unwrap();
    assert_ok(&locked_path, "techne lock with local path override");
    let run_path_ov = techne().args(["run", "--locked", "--store"]).arg(&store).arg(p_app.join("main.scm")).output().unwrap();
    assert_ok(&run_path_ov, "run --locked with local path override");
    assert_eq!(String::from_utf8_lossy(&run_path_ov.stdout).trim(), "overridden");
}

#[test]
fn cycle_detection_and_symlink_rejection() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let store = root.join("store");

    // 1. Dependency cycle: (acme ca) -> (acme cb) -> (acme ca)
    let ca = root.join("ca");
    let cb = root.join("cb");
    write_file(&ca, "techne-package.scm", "(package (acme ca) (version \"0.1.0\") (depends ((acme cb) (path \"../cb\"))))");
    write_file(&cb, "techne-package.scm", "(package (acme cb) (version \"0.1.0\") (depends ((acme ca) (path \"../ca\"))))");
    let cyc = techne().args(["lock"]).arg(&ca).arg("--store").arg(&store).output().unwrap();
    assert!(!cyc.status.success(), "cycle must fail");
    let err = stderr_str(&cyc);
    assert!(err.contains("Dependency cycle detected"), "{err}");
    assert!(err.contains("(acme ca) -> (acme cb) -> (acme ca)"), "{err}");

    // 2. Symlink rejection in canonical tree-sha256 & store materialization.
    #[cfg(unix)]
    {
        let sym_dep = root.join("sym-dep");
        let outside = root.join("outside-secret.txt");
        std::fs::write(&outside, "secret").unwrap();
        write_file(&sym_dep, "techne-package.scm", "(package (acme symdep) (version \"0.1.0\"))");
        std::os::unix::fs::symlink(&outside, sym_dep.join("leak.txt")).unwrap();
        let sym_app = root.join("sym-app");
        write_file(
            &sym_app,
            "techne-package.scm",
            "(package (acme symapp) (version \"0.1.0\") (depends ((acme symdep) (path \"../sym-dep\"))))",
        );
        let out = techne().args(["lock"]).arg(&sym_app).arg("--store").arg(&store).output().unwrap();
        assert!(!out.status.success(), "symlink in package tree must fail");
        assert!(stderr_str(&out).contains("Symlink not allowed"), "{}", stderr_str(&out));

        // Also test a symlink inside a Git repository dependency!
        let git_sym = root.join("git-sym");
        write_file(&git_sym, "techne-package.scm", "(package (acme gitsym) (version \"0.1.0\"))");
        write_file(&git_sym, "real.txt", "hello");
        std::os::unix::fs::symlink("real.txt", git_sym.join("link.txt")).unwrap();
        git_init_commit(&git_sym, "with symlink");
        let git_sym_app = root.join("git-sym-app");
        write_file(
            &git_sym_app,
            "techne-package.scm",
            &format!(
                "(package (acme gitsymapp) (version \"0.1.0\") (depends ((acme gitsym) (local {:?}))))",
                git_sym.display().to_string()
            ),
        );
        let out2 = techne().args(["lock"]).arg(&git_sym_app).arg("--store").arg(&store).output().unwrap();
        assert!(!out2.status.success(), "symlink in Git package tree must fail");
        assert!(stderr_str(&out2).contains("Symlink not allowed"), "{}", stderr_str(&out2));
    }
}
