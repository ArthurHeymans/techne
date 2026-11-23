//! Editor integration tests for Slice 4:
//! - Built-in `gpg` codec (real `gpg` with ephemeral GNUPGHOME) end-to-end:
//!   async `file-visit!`, logical mode & tree-sitter language, `Retention::None`,
//!   0o600 new file permissions, async `save-buffer`, external change refusal
//!   with `C-c r` recovery choices (`Overwrite` / `Revert` / private `*diff*`).
//! - Built-in `age` codec end-to-end: armor preservation, private profile,
//!   read-only decrypt without recipients, fail-closed save.
//! - Async `file-visit!` UX guarantees: non-blocking typing while decode runs,
//!   focus-safe background delivery, same-path deduplication, superseding,
//!   and `C-g` cancellation terminating slow helper subprocesses.
//! - `editor-open!` (`--wait`), `server-edit` (`C-x #`), and modal `:wq` with
//!   async codecs.

use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
    sync::{OnceLock, mpsc},
    time::{Duration, Instant},
};

use techne_editor::{
    host::{Event, Host},
    present::{Input, Output, Snapshot},
    runtime::{Runtime, journal_path_ro},
    server,
};

fn state() {
    static STATE: OnceLock<tempfile::TempDir> = OnceLock::new();
    STATE.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_STATE_HOME", dir.path()) };
        dir
    });
}

fn assert_no_journal_or_lock(path: &Path) {
    let j = journal_path_ro(path).unwrap();
    let mut lock = j.as_os_str().to_os_string();
    lock.push(".lock");
    let lock = std::path::PathBuf::from(lock);
    assert!(!j.exists(), "unexpected journal created for private file: {}", j.display());
    assert!(!lock.exists(), "unexpected lock file created for private file: {}", lock.display());
}

fn drain_until(rt: &mut Runtime, timeout: Duration, mut pred: impl FnMut(&mut Runtime) -> bool) {
    let deadline = Instant::now() + timeout;
    while !pred(rt) {
        assert!(Instant::now() < deadline, "timed out waiting for async editor state");
        let _ = rt.run_tasks(Duration::from_millis(20));
    }
}

fn have_gpg() -> bool {
    Command::new("gpg")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn run_gpg(gnupg: &Path, args: &[&str], stdin_bytes: Option<&[u8]>) -> Vec<u8> {
    let mut cmd = Command::new("gpg");
    cmd.arg("--homedir")
        .arg(gnupg)
        .args(["--batch", "--no-tty", "--yes", "--quiet"])
        .args(args)
        .stdin(if stdin_bytes.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn gpg");
    if let Some(bytes) = stdin_bytes {
        child.stdin.take().unwrap().write_all(bytes).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "gpg {:?} failed: {}", args, String::from_utf8_lossy(&out.stderr));
    out.stdout
}

fn write_executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

#[test]
fn gpg_encrypted_scm_end_to_end_async_visit_save_recovery_and_privacy() {
    if !have_gpg() {
        return;
    }
    state();
    let dir = tempfile::tempdir().unwrap();
    let gnupg = dir.path().join("gnupg");
    std::fs::create_dir(&gnupg).unwrap();
    std::fs::set_permissions(&gnupg, std::fs::Permissions::from_mode(0o700)).unwrap();

    run_gpg(&gnupg, &["--passphrase", "", "--quick-generate-key", "Alice Editor <alice@editor.test>", "default", "default", "never"], None);

    let normal = dir.path().join("normal.scm");
    let secret = dir.path().join("secret.scm.gpg");
    let brand_new = dir.path().join("brand-new.scm.gpg");
    std::fs::write(&normal, "(define normal 1)\n").unwrap();

    let initial_ct =
        run_gpg(&gnupg, &["--encrypt", "--trust-model", "always", "--recipient", "alice@editor.test"], Some(b"(define secret 1)\n"));
    std::fs::write(&secret, &initial_ct).unwrap();

    let (mut rt, _) = Runtime::open(&normal, &dir.path().join("normal.journal"), "emacs").unwrap();
    rt.eval(&format!(
        "(begin (set-option! 'gpg-homedir {:?}) (set-option! 'gpg-batch #t) (set-option! 'gpg-recipients '()))",
        gnupg.display().to_string()
    ))
    .unwrap();

    // Visit .scm.gpg asynchronously via file-visit!.
    let start_state = rt
        .eval(&format!(
            "(begin (file-visit! (current-session) {:?})\n\
                    (list (file-visit-pending? (current-session)) (echo-line (current-session))))",
            secret.display().to_string()
        ))
        .unwrap();
    assert!(start_state.starts_with("(#t \"Decrypting "), "{start_state}");

    // Wait for async decryption to land.
    drain_until(&mut rt, Duration::from_secs(15), |rt| {
        rt.eval("(and (not (file-visit-pending? (current-session))) (equal? (buffer-name (current-buffer (current-session))) \"secret.scm.gpg\"))")
            .unwrap()
            == "#t"
    });
    assert!(rt.take_errors().is_empty());

    let opened = rt
        .eval(
            "(let* ((s (current-session))\n\
                    (b (current-buffer s))\n\
                    (d (buffer-document b)))\n\
               (list (document-string d)\n\
                     (buffer-mode b)\n\
                     (buffer-language b)\n\
                     (document-retention d)\n\
                     (document-private? d)\n\
                     (option b 'lsp-auto)\n\
                     (codec-name (document-codec d))))",
        )
        .unwrap();
    assert_eq!(opened, "(\"(define secret 1)\\n\" scheme-mode scheme none #t #f gpg)");
    assert_no_journal_or_lock(&secret);

    // Edit and save with gpg-recipients empty and default gpg-trust-file-recipients=#f:
    // MUST refuse unauthenticated ciphertext ENC_TO recipients and keep buffer dirty.
    rt.eval(
        "(let* ((s (current-session)) (d (doc s)) (v (session-view s)))\n\
           (view-edit! v (list (list 0 (document-length d) \"(define secret 2)\\n\")) \"new\")\n\
           (run-command s 'save-buffer 1))",
    )
    .unwrap();
    drain_until(&mut rt, Duration::from_secs(15), |rt| {
        rt.eval("(and (string? (echo-line (current-session))) (string-contains (echo-line (current-session)) \"unauthenticated ciphertext ENC_TO\") #t)")
            .unwrap()
            == "#t"
    });
    assert_eq!(rt.eval("(document-dirty? (doc (current-session)))").unwrap(), "#t");
    assert!(rt.take_errors().is_empty());

    // Now explicitly opt in to trusting ciphertext ENC_TO recipients and save asynchronously.
    rt.eval("(begin (set-option! 'gpg-trust-file-recipients #t) (run-command (current-session) 'save-buffer 1))").unwrap();
    drain_until(&mut rt, Duration::from_secs(15), |rt| {
        rt.eval("(and (not (document-dirty? (doc (current-session)))) (string-prefix? \"Wrote \" (or (echo-line (current-session)) \"\")))")
            .unwrap()
            == "#t"
    });
    assert!(rt.take_errors().is_empty());

    let disk_plain = run_gpg(&gnupg, &["--decrypt"], Some(&std::fs::read(&secret).unwrap()));
    assert_eq!(String::from_utf8(disk_plain).unwrap(), "(define secret 2)\n");
    assert_no_journal_or_lock(&secret);

    // External modification refusal via async save-buffer offers recovery choices cleanly.
    let ext_ct =
        run_gpg(&gnupg, &["--encrypt", "--trust-model", "always", "--recipient", "alice@editor.test"], Some(b"(define secret 999)\n"));
    std::fs::write(&secret, &ext_ct).unwrap();

    rt.eval(
        "(let* ((s (current-session)) (d (doc s)) (v (session-view s)))\n\
           (view-edit! v (list (list 0 (document-length d) \"(define secret 3)\\n\")) \"new\")\n\
           (run-command s 'save-buffer 1))",
    )
    .unwrap();
    drain_until(&mut rt, Duration::from_secs(15), |rt| rt.eval("(pair? (pending-decisions (current-session)))").unwrap() == "#t");
    assert!(rt.take_errors().is_empty(), "refused async save must not leave unhandled runtime errors");

    // Compare is async for codec documents: choose it and pump until *diff* appears.
    let pre_compare = rt
        .eval(
            "(let* ((s (current-session))\n\
                    (echo (or (echo-line s) \"\"))\n\
                    (r (car (pending-decisions s)))\n\
                    (labels (map choice-label (record-choices r))))\n\
               (choose-recovery! s r (find (lambda (c) (string=? (choice-label c) \"Compare\")) (record-choices r)))\n\
               (list (and (string-contains echo \"changed on disk\") #t) labels))",
        )
        .unwrap();
    assert_eq!(pre_compare, "(#t (\"Overwrite\" \"Revert\" \"Compare\"))");
    drain_until(&mut rt, Duration::from_secs(15), |rt| rt.eval("(and (buffer-named \"*diff*\") #t)").unwrap() == "#t");
    let diff_check = rt
        .eval(
            "(let* ((diff-buf (buffer-named \"*diff*\"))\n\
                    (diff-doc (and diff-buf (buffer-document diff-buf)))\n\
                    (diff-priv (and diff-doc (document-private? diff-doc)))\n\
                    (diff-txt (and diff-doc (document-string diff-doc))))\n\
               (list diff-priv\n\
                     (and diff-txt (string-contains diff-txt \"-(define secret 999)\") #t)\n\
                     (and diff-txt (string-contains diff-txt \"+(define secret 3)\") #t)))",
        )
        .unwrap();
    assert_eq!(diff_check, "(#t #t #t)");

    // Overwrite is also async for codec documents: choose it and pump until clean.
    rt.eval(
        "(let* ((s (current-session))\n\
                (r (car (pending-decisions s))))\n\
           (choose-recovery! s r (find (lambda (c) (string=? (choice-label c) \"Overwrite\")) (record-choices r))))",
    )
    .unwrap();
    drain_until(&mut rt, Duration::from_secs(15), |rt| {
        rt.eval("(and (null? (pending-decisions (current-session))) (not (document-dirty? (buffer-document (buffer-named \"secret.scm.gpg\")))))")
            .unwrap()
            == "#t"
    });
    assert!(rt.take_errors().is_empty());
    let overwritten = run_gpg(&gnupg, &["--decrypt"], Some(&std::fs::read(&secret).unwrap()));
    assert_eq!(String::from_utf8(overwritten).unwrap(), "(define secret 3)\n");

    // Brand-new .scm.gpg created with 0o600 permissions and zero journals/locks.
    rt.eval(&format!(
        "(begin (set-option! 'gpg-recipients '(\"alice@editor.test\"))\n\
                (file-visit! (current-session) {:?}))",
        brand_new.display().to_string()
    ))
    .unwrap();
    drain_until(&mut rt, Duration::from_secs(15), |rt| {
        rt.eval("(and (not (file-visit-pending? (current-session))) (equal? (buffer-name (current-buffer (current-session))) \"brand-new.scm.gpg\"))")
            .unwrap()
            == "#t"
    });
    rt.eval(
        "(let* ((s (current-session)) (v (session-view s)))\n\
           (view-edit! v (list (list 0 0 \"(define fresh 42)\\n\")) \"new\")\n\
           (run-command s 'save-buffer 1))",
    )
    .unwrap();
    drain_until(&mut rt, Duration::from_secs(15), |rt| rt.eval("(not (document-dirty? (doc (current-session))))").unwrap() == "#t");
    let mode = std::fs::metadata(&brand_new).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "new .gpg file must be created with 0o600");
    assert_no_journal_or_lock(&brand_new);
    let fresh_plain = run_gpg(&gnupg, &["--decrypt"], Some(&std::fs::read(&brand_new).unwrap()));
    assert_eq!(String::from_utf8(fresh_plain).unwrap(), "(define fresh 42)\n");
}

#[test]
fn age_encrypted_md_end_to_end_armor_preservation_and_read_only_decrypt() {
    state();
    let dir = tempfile::tempdir().unwrap();
    let mock_age = dir.path().join("mock-age.sh");
    let id_file = dir.path().join("keys.txt");
    let normal = dir.path().join("normal.txt");
    let notes = dir.path().join("notes.md.age");
    std::fs::write(&id_file, "AGE-KEY-OK\n").unwrap();
    std::fs::write(&normal, "plain\n").unwrap();
    std::fs::write(&notes, "-----BEGIN AGE ENCRYPTED FILE-----\nAGE-HDR:,age1initial\n# Private Note\nhello age\n").unwrap();
    write_executable(
        &mock_age,
        "#!/bin/sh\n\
mode=''\n\
armor=0\n\
recips=''\n\
id_ok=0\n\
while [ $# -gt 0 ]; do\n\
  case \"$1\" in\n\
    -d) mode=d; shift ;;\n\
    -e) mode=e; shift ;;\n\
    -a) armor=1; shift ;;\n\
    -i) shift; [ -f \"$1\" ] && grep -q 'AGE-KEY-OK' \"$1\" && id_ok=1; shift ;;\n\
    -r) shift; recips=\"$recips,$1\"; shift ;;\n\
    *) echo \"bad arg: $1\" >&2; exit 2 ;;\n\
  esac\n\
done\n\
if [ \"$mode\" = e ]; then\n\
  [ -n \"$recips\" ] || { echo 'no recipients' >&2; exit 1; }\n\
  if [ \"$armor\" = 1 ]; then\n\
    printf '%s\\nAGE-HDR:%s\\n' '-----BEGIN AGE ENCRYPTED FILE-----' \"$recips\"\n\
  else\n\
    printf 'AGE-BIN:%s\\n' \"$recips\"\n\
  fi\n\
  cat\n\
  exit 0\n\
elif [ \"$mode\" = d ]; then\n\
  [ \"$id_ok\" = 1 ] || { echo 'bad id' >&2; exit 1; }\n\
  IFS= read -r first || exit 1\n\
  if [ \"$first\" = '-----BEGIN AGE ENCRYPTED FILE-----' ]; then\n\
    IFS= read -r hdr || exit 1\n\
    case \"$hdr\" in AGE-HDR:*) cat; exit 0 ;; *) exit 1 ;; esac\n\
  else\n\
    case \"$first\" in AGE-BIN:*) cat; exit 0 ;; *) exit 1 ;; esac\n\
  fi\n\
fi\n\
exit 2\n",
    );

    let (mut rt, _) = Runtime::open(&normal, &dir.path().join("normal.journal"), "emacs").unwrap();
    rt.eval(&format!(
        "(begin (set-option! 'age-program {:?})\n\
                (set-option! 'age-identities (list {:?}))\n\
                (set-option! 'age-recipients '(\"age1alice\"))\n\
                (set-option! 'age-armor #f)\n\
                (file-visit! (current-session) {:?}))",
        mock_age.display().to_string(),
        id_file.display().to_string(),
        notes.display().to_string()
    ))
    .unwrap();
    drain_until(&mut rt, Duration::from_secs(10), |rt| {
        rt.eval("(and (not (file-visit-pending? (current-session))) (equal? (buffer-name (current-buffer (current-session))) \"notes.md.age\"))")
            .unwrap()
            == "#t"
    });

    let info = rt
        .eval(
            "(let* ((b (current-buffer (current-session))) (d (buffer-document b)))\n\
               (list (document-string d) (buffer-mode b) (document-logical-path d) (document-retention d) (document-private? d)))",
        )
        .unwrap();
    assert_eq!(
        info,
        format!("(\"# Private Note\\nhello age\\n\" text-mode {:?} none #t)", dir.path().join("notes.md").display().to_string())
    );
    assert_no_journal_or_lock(&notes);

    // Save with age-armor=#f -> envelope armor=#t is preserved!
    rt.eval(
        "(let* ((s (current-session)) (d (doc s)) (v (session-view s)))\n\
           (view-edit! v (list (list 0 (document-length d) \"# Updated\\nworld\\n\")) \"new\")\n\
           (run-command s 'save-buffer 1))",
    )
    .unwrap();
    drain_until(&mut rt, Duration::from_secs(10), |rt| rt.eval("(not (document-dirty? (doc (current-session))))").unwrap() == "#t");
    let on_disk = std::fs::read_to_string(&notes).unwrap();
    assert!(on_disk.starts_with("-----BEGIN AGE ENCRYPTED FILE-----\nAGE-HDR:,age1alice\n# Updated\nworld\n"), "{on_disk}");

    // Read-only decrypt without recipients works, while save fails closed cleanly.
    let ro = dir.path().join("ro.md.age");
    std::fs::write(&ro, "AGE-BIN:,age1someone\nreadonly body\n").unwrap();
    rt.eval(&format!(
        "(begin (set-option! 'age-recipients '())\n\
                (set-option! 'age-recipients-file #f)\n\
                (set-option! 'age-recipient-roots '())\n\
                (file-visit! (current-session) {:?}))",
        ro.display().to_string()
    ))
    .unwrap();
    drain_until(&mut rt, Duration::from_secs(10), |rt| {
        rt.eval(
            "(and (not (file-visit-pending? (current-session))) (equal? (buffer-name (current-buffer (current-session))) \"ro.md.age\"))",
        )
        .unwrap()
            == "#t"
    });
    assert_eq!(rt.eval("(document-string (doc (current-session)))").unwrap(), "\"readonly body\\n\"");

    rt.eval(
        "(let* ((s (current-session)) (v (session-view s)))\n\
           (view-edit! v (list (list 0 0 \"edit \")) \"new\")\n\
           (run-command s 'save-buffer 1))",
    )
    .unwrap();
    drain_until(&mut rt, Duration::from_secs(10), |rt| {
        rt.eval("(and (string? (echo-line (current-session))) (string-contains (echo-line (current-session)) \"no recipients\") #t)")
            .unwrap()
            == "#t"
    });
    assert!(rt.take_errors().is_empty());
}

#[test]
fn async_file_visit_nonblocking_focus_safety_dedup_supersede_and_cancel() {
    state();
    let dir = tempfile::tempdir().unwrap();
    let normal = dir.path().join("normal.txt");
    let other = dir.path().join("other.txt");
    let slow_one = dir.path().join("one.slow");
    let slow_bg = dir.path().join("bg.slow");
    let slow_dedup = dir.path().join("dedup.slow");
    let slow_first = dir.path().join("first.slow");
    let slow_second = dir.path().join("second.slow");
    let hang_file = dir.path().join("stuck.hang");
    let hang_pid = dir.path().join("hang.pid");
    let hang_script = dir.path().join("hang-codec.sh");

    std::fs::write(&normal, "").unwrap();
    std::fs::write(&other, "other buffer\n").unwrap();
    for f in [&slow_one, &slow_bg, &slow_dedup, &slow_first, &slow_second, &hang_file] {
        std::fs::write(f, "encoded\n").unwrap();
    }
    let hang_pid_tmp = dir.path().join("hang.pid.tmp");
    write_executable(
        &hang_script,
        &format!(
            "#!/bin/sh\necho $$ > {:?} && mv {:?} {:?}\nexec sleep 30\n",
            hang_pid_tmp.display().to_string(),
            hang_pid_tmp.display().to_string(),
            hang_pid.display().to_string()
        ),
    );

    let (mut rt, _) = Runtime::open(&normal, &dir.path().join("normal.journal"), "emacs").unwrap();

    // Register channel-gated .slow codec and subprocess-backed .hang codec.
    rt.eval(&format!(
        "(begin\n\
           (import (techne trusted-process))\n\
           (define %gates (make-hash-table))\n\
           (define %decode-count (make-hash-table))\n\
           (define (gate-for p)\n\
             (or (hash-table-ref/default %gates p #f)\n\
                 (let ((ch (make-channel 1)))\n\
                   (hash-table-set! %gates p ch)\n\
                   ch)))\n\
           (define-codec slow-test\n\
             \"Channel-gated slow codec.\"\n\
             #:match (lambda (p) (string-suffix? \".slow\" p))\n\
             #:logical-path (lambda (p) (substring p 0 (- (string-length p) 5)))\n\
             #:decode (lambda (p _raw)\n\
                        (hash-table-set! %decode-count p (+ 1 (hash-table-ref/default %decode-count p 0)))\n\
                        (make-codec-result (channel-recv (gate-for p))))\n\
             #:encode (lambda (_p text _env) (string->utf8 text)))\n\
           (define-codec hang-test\n\
             \"Subprocess hang codec.\"\n\
             #:match (lambda (p) (string-suffix? \".hang\" p))\n\
             #:decode (lambda (_p raw)\n\
                        (let ((res (run-trusted-tool-bytes {:?} '() #:stdin raw #:timeout 30000 #:label \"hang\")))\n\
                          (make-codec-result (utf8->string (cadr res)))))\n\
             #:encode (lambda (_p text _env) (string->utf8 text))))",
        hang_script.display().to_string()
    ))
    .unwrap();

    // 1. Non-blocking: start async visit, type immediately in current buffer while blocked.
    rt.eval(&format!("(file-visit! (current-session) {:?})", slow_one.display().to_string())).unwrap();
    let _ = rt.run_tasks(Duration::from_millis(30));
    assert_eq!(rt.eval("(file-visit-pending? (current-session))").unwrap(), "#t");
    for k in ["h", "i"] {
        rt.handle(Input::Key { key: k.into(), at: Instant::now() });
    }
    assert_eq!(rt.eval("(document-string (doc (current-session)))").unwrap(), "\"hi\"");

    // Release gate -> lands in focused pane because we didn't switch views/panes.
    rt.eval(&format!("(channel-send (gate-for {:?}) \"decoded one\\n\")", slow_one.display().to_string())).unwrap();
    drain_until(&mut rt, Duration::from_secs(5), |rt| {
        rt.eval("(equal? (buffer-name (current-buffer (current-session))) \"one.slow\")").unwrap() == "#t"
    });
    assert_eq!(rt.eval("(document-string (doc (current-session)))").unwrap(), "\"decoded one\\n\"");

    // 2. Focus safety: switch buffer while bg.slow is in flight -> opens in background without hijacking focus.
    rt.eval(&format!("(file-visit! (current-session) {:?})", slow_bg.display().to_string())).unwrap();
    let _ = rt.run_tasks(Duration::from_millis(30));
    rt.eval(&format!("(show-document! (current-session) (file-document {:?}))", other.display().to_string())).unwrap();
    rt.eval(&format!("(channel-send (gate-for {:?}) \"decoded bg\\n\")", slow_bg.display().to_string())).unwrap();
    drain_until(&mut rt, Duration::from_secs(5), |rt| {
        rt.eval("(and (not (file-visit-pending? (current-session))) (buffer-named \"bg.slow\") #t)").unwrap() == "#t"
    });
    let focus_res = rt
        .eval(
            "(list (buffer-name (current-buffer (current-session)))\n\
                   (document-string (buffer-document (buffer-named \"bg.slow\")))\n\
                   (and (string-contains (or (echo-line (current-session)) \"\") \"in background\") #t))",
        )
        .unwrap();
    assert_eq!(focus_res, "(\"other.txt\" \"decoded bg\\n\" #t)");

    // 3. Same-path deduplication: repeating file-visit! for an in-flight path does not spawn a second decode.
    rt.eval(&format!(
        "(begin (file-visit! (current-session) {:?}) (file-visit! (current-session) {:?}))",
        slow_dedup.display().to_string(),
        slow_dedup.display().to_string()
    ))
    .unwrap();
    let _ = rt.run_tasks(Duration::from_millis(40));
    rt.eval(&format!("(channel-send (gate-for {:?}) \"dedup ok\\n\")", slow_dedup.display().to_string())).unwrap();
    drain_until(&mut rt, Duration::from_secs(5), |rt| {
        rt.eval("(equal? (buffer-name (current-buffer (current-session))) \"dedup.slow\")").unwrap() == "#t"
    });
    let dedup_count = rt.eval(&format!("(hash-table-ref/default %decode-count {:?} 0)", slow_dedup.display().to_string())).unwrap();
    assert_eq!(dedup_count, "1");

    // 4. Superseding: starting second.slow cancels first.slow so first.slow never lands.
    rt.eval(&format!("(file-visit! (current-session) {:?})", slow_first.display().to_string())).unwrap();
    let _ = rt.run_tasks(Duration::from_millis(30));
    rt.eval(&format!("(file-visit! (current-session) {:?})", slow_second.display().to_string())).unwrap();
    let _ = rt.run_tasks(Duration::from_millis(30));
    rt.eval(&format!(
        "(begin (channel-send (gate-for {:?}) \"late first\\n\")\n\
                (channel-send (gate-for {:?}) \"winner second\\n\"))",
        slow_first.display().to_string(),
        slow_second.display().to_string()
    ))
    .unwrap();
    drain_until(&mut rt, Duration::from_secs(5), |rt| {
        rt.eval(
            "(and (not (file-visit-pending? (current-session))) (equal? (buffer-name (current-buffer (current-session))) \"second.slow\"))",
        )
        .unwrap()
            == "#t"
    });
    assert_eq!(rt.eval("(document-string (doc (current-session)))").unwrap(), "\"winner second\\n\"");

    // 5. C-g cancels in-flight async visit AND kills its slow helper subprocess.
    rt.eval(&format!("(file-visit! (current-session) {:?})", hang_file.display().to_string())).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !hang_pid.exists() {
        assert!(Instant::now() < deadline, "hang helper never started");
        let _ = rt.run_tasks(Duration::from_millis(20));
    }
    let pid_str = std::fs::read_to_string(&hang_pid).unwrap();
    let pid: u32 = pid_str.trim().parse().expect("valid pid");

    rt.handle(Input::Key { key: "C-g".into(), at: Instant::now() });
    drain_until(&mut rt, Duration::from_secs(5), |rt| rt.eval("(not (file-visit-pending? (current-session)))").unwrap() == "#t");

    // Confirm the sleep helper process is fully gone (reaped, not zombie or lingering for 30s).
    let kill_deadline = Instant::now() + Duration::from_secs(5);
    let proc_stat = Path::new("/proc").join(pid.to_string()).join("stat");
    loop {
        let _ = rt.run_tasks(Duration::from_millis(20));
        if std::fs::read_to_string(&proc_stat).is_err() {
            break;
        }
        assert!(Instant::now() < kill_deadline, "cancelled helper process {pid} still alive or unreaped");
    }
    assert!(rt.take_errors().is_empty());
}

struct EditorHost {
    host: Host,
    events: mpsc::Receiver<Event>,
    _listener: server::Listener,
}

impl EditorHost {
    fn start(socket: &Path, profile: &str, setup: impl Fn(&mut Runtime) + Send + Sync + 'static) -> Self {
        state();
        let (tx, events) = mpsc::channel();
        let host = Host::start(None, profile.into(), setup, move |e| drop(tx.send(e)));
        let listener = host.listen(socket).unwrap().expect("listener");
        Self { host, events, _listener: listener }
    }

    fn start_file(file: &Path, socket: &Path, profile: &str, setup: impl Fn(&mut Runtime) + Send + Sync + 'static) -> Self {
        state();
        let (tx, events) = mpsc::channel();
        let f = techne_editor::runtime::File { path: file.to_path_buf(), journal: journal_path_ro(file).unwrap() };
        let host = Host::start(Some(f), profile.into(), setup, move |e| drop(tx.send(e)));
        let listener = host.listen(socket).unwrap().expect("listener");
        Self { host, events, _listener: listener }
    }

    fn keys(&mut self, keys: &str) {
        keys.split(' ').for_each(|k| self.host.send(Input::Key { key: k.into(), at: Instant::now() }));
    }

    fn snapshot(&self, done: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let left = deadline.checked_duration_since(Instant::now()).expect("snapshot in time");
            match self.events.recv_timeout(left).expect("event") {
                Event::Output(Output::Snapshot(s)) if done(&s) => return *s,
                Event::Failed(e) => panic!("{e}"),
                _ => {}
            }
        }
    }

    fn wait_quit(&self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let left = deadline.checked_duration_since(Instant::now()).expect("quit in time");
            match self.events.recv_timeout(left).expect("event") {
                Event::Output(Output::Quit) => return,
                Event::Failed(e) => panic!("{e}"),
                _ => {}
            }
        }
    }
}

#[test]
fn editor_open_wait_and_modal_wq_with_async_codec() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("socket");
    let helper = dir.path().join("slow-codec.sh");
    let wait_file = dir.path().join("wait.slowenc");
    let bad_file = dir.path().join("bad.badenc");
    std::fs::write(&wait_file, "ENC:hello\n").unwrap();
    std::fs::write(&bad_file, "broken").unwrap();
    write_executable(
        &helper,
        "#!/bin/sh\n\
sleep 0.15\n\
if [ \"$1\" = decode ]; then\n\
  sed 's/^ENC://'\n\
elif [ \"$1\" = encode ]; then\n\
  printf 'ENC:'\n\
  cat\n\
else\n\
  exit 1\n\
fi\n",
    );

    let helper_s = helper.display().to_string();
    let mut e = EditorHost::start(&socket, "emacs", move |rt| {
        rt.eval(&format!(
            "(begin\n\
               (import (techne trusted-process))\n\
               (define-codec slowenc\n\
                 \"Slow test codec.\"\n\
                 #:match (lambda (p) (string-suffix? \".slowenc\" p))\n\
                 #:decode (lambda (_p raw)\n\
                            (let ((r (run-trusted-tool-bytes {helper:?} '(\"decode\") #:stdin raw #:label \"slowenc\")))\n\
                              (unless (= (car r) 0) (error \"decode failed\"))\n\
                              (make-codec-result (utf8->string (cadr r)))))\n\
                 #:encode (lambda (_p text _env)\n\
                            (let ((r (run-trusted-tool-bytes {helper:?} '(\"encode\") #:stdin (string->utf8 text) #:label \"slowenc\")))\n\
                              (unless (= (car r) 0) (error \"encode failed\"))\n\
                              (cadr r)))\n\
                 #:private #t)\n\
               (define-codec badenc\n\
                 \"Failing test codec.\"\n\
                 #:match (lambda (p) (string-suffix? \".badenc\" p))\n\
                 #:decode (lambda (_p _raw) (error \"corrupt badenc\"))\n\
                 #:encode (lambda (_p _text _env) (error \"cannot encode badenc\"))\n\
                 #:private #t))",
            helper = helper_s
        ))
        .unwrap();
    });

    // 1. server::open(..., wait=true) opens slowenc asynchronously, server-edit saves async, then unblocks.
    let (sock1, path1) = (socket.clone(), wait_file.clone());
    let waiting = std::thread::spawn(move || server::open(&sock1, &path1, true));
    e.snapshot(|s| s.pane().text == "hello\n");
    e.keys("! C-x #");
    waiting.join().unwrap().unwrap();
    assert_eq!(std::fs::read_to_string(&wait_file).unwrap(), "ENC:!hello\n");
    assert_no_journal_or_lock(&wait_file);

    // 2. Failing async codec decode returns OpenError::Failed to --wait caller (never Done!).
    let (sock2, path2) = (socket.clone(), bad_file.clone());
    let failing = std::thread::spawn(move || server::open(&sock2, &path2, true));
    e.snapshot(|s| s.echo.contains("corrupt badenc"));
    match failing.join().unwrap() {
        Err(server::OpenError::Failed(msg)) => {
            assert!(msg.contains("corrupt badenc"), "unexpected error message: {msg}");
        }
        other => panic!("expected OpenError::Failed for corrupt badenc, got {other:?}"),
    }
    e.host.close();

    // 3. Modal :wq waits for async save to finish before quitting!
    let socket2 = dir.path().join("socket2");
    let modal_file = dir.path().join("modal.slowenc");
    std::fs::write(&modal_file, "ENC:init\n").unwrap();
    let helper_s2 = helper.display().to_string();
    let mut m = EditorHost::start(&socket2, "modal", move |rt| {
        rt.eval(&format!(
            "(begin\n\
               (import (techne trusted-process))\n\
               (define-codec slowenc\n\
                 \"Slow test codec.\"\n\
                 #:match (lambda (p) (string-suffix? \".slowenc\" p))\n\
                 #:decode (lambda (_p raw)\n\
                            (let ((r (run-trusted-tool-bytes {helper:?} '(\"decode\") #:stdin raw #:label \"slowenc\")))\n\
                              (make-codec-result (utf8->string (cadr r)))))\n\
                 #:encode (lambda (_p text _env)\n\
                            (let ((r (run-trusted-tool-bytes {helper:?} '(\"encode\") #:stdin (string->utf8 text) #:label \"slowenc\")))\n\
                              (cadr r)))\n\
                 #:private #t))",
            helper = helper_s2
        ))
        .unwrap();
    });
    server::open(&socket2, &modal_file, false).unwrap();
    m.snapshot(|s| s.pane().text == "init\n");
    m.keys("A ! ESC : w q RET");
    m.wait_quit();
    assert_eq!(std::fs::read_to_string(&modal_file).unwrap(), "ENC:init!\n");
    assert_no_journal_or_lock(&modal_file);
}

#[test]
fn symlink_codec_mismatch_fails_closed_cold_and_warm() {
    state();
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("raw.txt");
    let link = dir.path().join("alias.gpg");
    std::fs::write(&raw, "plaintext body\n").unwrap();
    std::os::unix::fs::symlink(&raw, &link).unwrap();

    // 1. Cold open via alias.gpg -> raw.txt must fail closed (never open plaintext or write raw).
    let (mut rt, _) = Runtime::open(&dir.path().join("other.txt"), &dir.path().join("other.journal"), "emacs").unwrap();
    let cold = rt
        .eval(&format!(
            "(guard (e (#t (if (error-object? e) (error-object-message e) \"err\")))\n\
               (file-document {:?})\n\
               \"OK\")",
            link.display().to_string()
        ))
        .unwrap();
    assert!(cold.contains("codec mismatch"), "expected codec mismatch error, got {cold}");

    // 2. Warm open: open raw.txt first, then visit alias.gpg -> must NOT reuse cached raw.txt!
    rt.eval(&format!("(show-document! (current-session) (file-document {:?}))", raw.display().to_string())).unwrap();
    let warm_sync = rt
        .eval(&format!(
            "(guard (e (#t (if (error-object? e) (error-object-message e) \"err\")))\n\
               (file-document {:?})\n\
               \"OK\")",
            link.display().to_string()
        ))
        .unwrap();
    assert!(warm_sync.contains("codec mismatch"), "expected warm codec mismatch error, got {warm_sync}");

    rt.eval(&format!("(file-visit! (current-session) {:?})", link.display().to_string())).unwrap();
    drain_until(&mut rt, Duration::from_secs(5), |rt| {
        rt.eval("(and (not (file-visit-pending? (current-session))) (string-contains (or (echo-line (current-session)) \"\") \"codec mismatch\") #t)")
            .unwrap()
            == "#t"
    });
    assert_eq!(rt.eval("(buffer-name (current-buffer (current-session)))").unwrap(), "\"raw.txt\"");
    assert!(rt.take_errors().is_empty());
}

#[test]
fn private_clipboard_provenance_and_joined_kills() {
    state();
    let dir = tempfile::tempdir().unwrap();
    let normal = dir.path().join("normal.txt");
    let secret = dir.path().join("secret.priv");
    std::fs::write(&normal, "public line\n").unwrap();
    std::fs::write(&secret, "top-secret-token\n").unwrap();

    let (mut rt, _) = Runtime::open(&normal, &dir.path().join("normal.journal"), "emacs").unwrap();
    rt.eval(
        "(define-codec priv-test\n\
           \"Private test codec.\"\n\
           #:match (lambda (p) (string-suffix? \".priv\" p))\n\
           #:decode (lambda (_p raw) (make-codec-result (if raw (utf8->string raw) \"\")))\n\
           #:encode (lambda (_p text _env) (string->utf8 text))\n\
           #:private #t)",
    )
    .unwrap();

    // Open private doc without focusing it (current buffer stays normal.txt).
    // Invoking copy-location-line on the private doc via act-on! MUST NOT leak to clipboard-out!
    rt.eval(&format!(
        "(let* ((s (current-session))\n\
                (priv-doc (file-document {:?}))\n\
                (t (target 'location (location priv-doc 0))))\n\
           (act-on! s t \"line\"))",
        secret.display().to_string()
    ))
    .unwrap();
    for ch in "copy-location-line".chars() {
        rt.handle(Input::Key { key: ch.to_string(), at: Instant::now() });
    }
    rt.handle(Input::Key { key: "RET".into(), at: Instant::now() });

    let res = rt
        .eval(
            "(let* ((s (current-session))\n\
                    (clip1 (editor-clipboard-out s))\n\
                    (ring1 (car (kill-ring s))))\n\
               ;; Kill a public chunk, then join a private kill (simulating consecutive kill):\n\
               ;; joined kill must stay private and MUST clear any pending clipboard-out.\n\
               (sset! s 'last-kill #f)\n\
               (kill-save! s \"pub-\" #f #f #:private #f)\n\
               (sset! s 'last-kill #t)\n\
               (kill-save! s \"sec\" #f #f #:private #t)\n\
               (let ((clip2 (editor-clipboard-out s))\n\
                     (ring2 (car (kill-ring s))))\n\
                 (list clip1 ring1 clip2 ring2)))",
        )
        .unwrap();
    assert_eq!(res, "(#f (\"top-secret-token\" . #f) #f (\"pub-sec\" . #f))");
}

#[test]
fn server_open_isolation_and_client_disconnect_cancellation() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("socket");
    let helper = dir.path().join("gated-codec.sh");
    let gate_file = dir.path().join("gate.go");
    let started_file = dir.path().join("started");
    let wait_file = dir.path().join("server.gated");
    let other_file = dir.path().join("other.txt");
    std::fs::write(&wait_file, "G:from-server\n").unwrap();
    std::fs::write(&other_file, "interactive-other\n").unwrap();
    write_executable(
        &helper,
        &format!(
            "#!/bin/sh\n\
echo $$ > {started:?}\n\
while [ ! -f {gate:?} ]; do sleep 0.03; done\n\
if [ \"$1\" = decode ]; then\n\
  sed 's/^G://'\n\
else\n\
  printf 'G:'\n\
  cat\n\
fi\n",
            started = started_file.display().to_string(),
            gate = gate_file.display().to_string()
        ),
    );

    let helper_s = helper.display().to_string();
    let mut e = EditorHost::start(&socket, "emacs", move |rt| {
        rt.eval(&format!(
            "(begin\n\
               (import (techne trusted-process))\n\
               (define-codec gated\n\
                 \"Gate-controlled test codec.\"\n\
                 #:match (lambda (p) (string-suffix? \".gated\" p))\n\
                 #:decode (lambda (_p raw)\n\
                            (let ((r (run-trusted-tool-bytes {helper:?} '(\"decode\") #:stdin raw #:timeout 15000 #:label \"gated\")))\n\
                              (unless (= (car r) 0) (error \"decode failed\"))\n\
                              (make-codec-result (utf8->string (cadr r)))))\n\
                 #:encode (lambda (_p text _env)\n\
                            (let ((r (run-trusted-tool-bytes {helper:?} '(\"encode\") #:stdin (string->utf8 text) #:timeout 15000 #:label \"gated\")))\n\
                              (unless (= (car r) 0) (error \"encode failed\"))\n\
                              (cadr r)))\n\
                 #:private #t))",
            helper = helper_s
        ))
        .unwrap();
    });

    // 1. Start a --wait server open that blocks on gate_file.
    let (sock1, path1) = (socket.clone(), wait_file.clone());
    let waiting = std::thread::spawn(move || server::open(&sock1, &path1, true));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !started_file.exists() {
        assert!(Instant::now() < deadline, "gated helper never started");
        std::thread::sleep(Duration::from_millis(20));
    }

    // Interactive C-g MUST NOT cancel the in-flight --wait server open!
    e.keys("C-g");
    std::thread::sleep(Duration::from_millis(80));
    std::fs::write(&gate_file, "go").unwrap();
    e.snapshot(|s| s.pane().text == "from-server\n");
    e.keys("C-x #");
    waiting.join().unwrap().unwrap();

    // 2. Client disconnect (Msg::Gone) cancels an in-flight server open so no stray buffer lands.
    std::fs::remove_file(&gate_file).unwrap();
    std::fs::remove_file(&started_file).unwrap();
    let stray = dir.path().join("stray.gated");
    std::fs::write(&stray, "G:should-not-open\n").unwrap();

    // Open a raw socket connection sending Request::Open { wait: true }, then drop it once decode starts.
    {
        use std::os::unix::net::UnixStream;
        let mut stream = UnixStream::connect(&socket).unwrap();
        let req = server::Request::Open { path: stray.clone(), wait: true };
        let body = rmp_serde::to_vec_named(&req).unwrap();
        let len = (body.len() as u32).to_be_bytes();
        stream.write_all(&len).unwrap();
        stream.write_all(&body).unwrap();
        let dl = Instant::now() + Duration::from_secs(5);
        while !started_file.exists() {
            assert!(Instant::now() < dl, "stray gated helper never started");
            std::thread::sleep(Duration::from_millis(20));
        }
        // Drop stream -> server detects gone() and sends Msg::Gone!
    }
    // Wait long enough for server GONE_CHECK (1s) to detect disconnect and cancel the in-flight decode.
    std::thread::sleep(Duration::from_millis(1300));
    std::fs::write(&gate_file, "go").unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let has_stray = server::eval(
        &socket,
        &server::Request::Eval {
            source: "(and (buffer-named \"stray.gated\") #t)".into(),
            module: None,
            detached: false,
            principal: None,
            timeout_ms: None,
        },
    )
    .unwrap();
    assert_eq!(has_stray.result, Ok("#f".into()), "cancelled server open must not create stray buffer");
    e.host.close();
}

#[test]
fn concurrent_edit_during_async_wq_and_server_edit_and_modal_x_and_cancel() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("socket");
    let helper = dir.path().join("slow-save.sh");
    let enc_started = dir.path().join("enc-started");
    let enc_gate = dir.path().join("enc-gate");
    let modal_file = dir.path().join("modal.gatedsave");
    std::fs::write(&modal_file, "S:one\n").unwrap();
    write_executable(
        &helper,
        &format!(
            "#!/bin/sh\n\
if [ \"$1\" = decode ]; then\n\
  sed 's/^S://'\n\
else\n\
  echo $$ > {started:?}\n\
  while [ ! -f {gate:?} ]; do sleep 0.03; done\n\
  printf 'S:'\n\
  cat\n\
fi\n",
            started = enc_started.display().to_string(),
            gate = enc_gate.display().to_string()
        ),
    );

    let helper_s = helper.display().to_string();
    let mut m = EditorHost::start(&socket, "modal", move |rt| {
        rt.eval(&format!(
            "(begin\n\
               (import (techne trusted-process))\n\
               (define-codec gatedsave\n\
                 \"Gate-controlled save codec.\"\n\
                 #:match (lambda (p) (string-suffix? \".gatedsave\" p))\n\
                 #:decode (lambda (_p raw)\n\
                            (let ((r (run-trusted-tool-bytes {helper:?} '(\"decode\") #:stdin raw #:label \"gs\")))\n\
                              (make-codec-result (utf8->string (cadr r)))))\n\
                 #:encode (lambda (_p text _env)\n\
                            (let ((r (run-trusted-tool-bytes {helper:?} '(\"encode\") #:stdin (string->utf8 text) #:timeout 15000 #:label \"gs\")))\n\
                              (cadr r)))\n\
                 #:private #t)\n\
               (define-codec ro-codec\n\
                 \"Read-only codec that errors on encode.\"\n\
                 #:match (lambda (p) (string-suffix? \".rocodec\" p))\n\
                 #:decode (lambda (_p raw) (make-codec-result (if raw (utf8->string raw) \"\")))\n\
                 #:encode (lambda (_p _text _env) (error \"should not encode clean file\"))\n\
                 #:private #t))",
            helper = helper_s
        ))
        .unwrap();
    });

    // 1. Editing while async :wq is in flight MUST NOT quit and MUST report newer edits remain!
    server::open(&socket, &modal_file, false).unwrap();
    m.snapshot(|s| s.pane().text == "one\n");
    m.keys("A ! ESC : w q RET");
    let dl = Instant::now() + Duration::from_secs(5);
    while !enc_started.exists() {
        assert!(Instant::now() < dl, "encode never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Type another edit while encode is still blocked!
    m.keys("A ? ESC");
    m.snapshot(|s| s.pane().text == "one!?\n");
    std::fs::write(&enc_gate, "go").unwrap();
    m.snapshot(|s| s.echo.contains("newer edits remain"));
    assert_eq!(std::fs::read_to_string(&modal_file).unwrap(), "S:one!\n");

    // 2. Modal :x on a clean read-only file quits cleanly without calling encode!
    let ro_file = dir.path().join("clean.rocodec");
    std::fs::write(&ro_file, "readonly\n").unwrap();
    server::open(&socket, &ro_file, false).unwrap();
    m.snapshot(|s| s.pane().text == "readonly\n");
    m.keys(": x RET");
    m.wait_quit();
    assert_eq!(std::fs::read_to_string(&ro_file).unwrap(), "readonly\n");
}

#[test]
fn live_host_attach_with_slow_codec_does_not_block() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("socket");
    let slow_file = dir.path().join("boot.slowattach");
    let gate_file = dir.path().join("attach.go");
    let helper = dir.path().join("slow-attach.sh");
    std::fs::write(&slow_file, "A:attached-ok\n").unwrap();
    // Gate the helper until AFTER we receive the live attach snapshot — proves attach is
    // strictly non-blocking and cannot hit OPEN_DEADLINE regardless of decode duration!
    write_executable(
        &helper,
        &format!("#!/bin/sh\nwhile [ ! -f {:?} ]; do sleep 0.03; done\nsed 's/^A://'\n", gate_file.display().to_string()),
    );

    let helper_s = helper.display().to_string();
    let e = EditorHost::start_file(&slow_file, &socket, "emacs", move |rt| {
        rt.eval(&format!(
            "(begin\n\
               (import (techne trusted-process))\n\
               (define-codec slowattach\n\
                 \"Slow attach codec.\"\n\
                 #:match (lambda (p) (string-suffix? \".slowattach\" p))\n\
                 #:decode (lambda (_p raw)\n\
                            (let ((r (run-trusted-tool-bytes {helper:?} '() #:stdin raw #:timeout 15000 #:label \"sa\")))\n\
                              (make-codec-result (utf8->string (cadr r)))))\n\
                 #:encode (lambda (_p text _env) (string->utf8 (string-append \"A:\" text)))\n\
                 #:private #t))",
            helper = helper_s
        ))
        .unwrap();
    });
    // First snapshot MUST arrive while the helper is still gated (not yet finished)!
    let first = e.snapshot(|s| s.echo.contains("Decrypting "));
    assert!(first.echo.contains("boot.slowattach"), "unexpected echo: {}", first.echo);

    // Release gate -> decoded buffer lands asynchronously without error!
    std::fs::write(&gate_file, "go").unwrap();
    e.snapshot(|s| s.pane().text == "attached-ok\n");
    e.host.close();
}

#[test]
fn user_only_codec_options_parameterize_precedence_and_unchanged_reload() {
    state();
    let dir = tempfile::tempdir().unwrap();
    let normal = dir.path().join("normal.txt");
    let tracked = dir.path().join("tracked.cnt");
    std::fs::write(&normal, "plain\n").unwrap();
    std::fs::write(&tracked, "C:hello\n").unwrap();

    let (mut rt, _) = Runtime::open(&normal, &dir.path().join("normal.journal"), "emacs").unwrap();

    // 1. Codec options are user-only (refuse buffer/mode scopes) and enforce schemas.
    let scope_err = rt
        .eval(
            "(guard (e (#t (if (error-object? e) (error-object-message e) \"err\")))\n\
               (set-option! 'gpg-program \"/tmp/evil\" #:buffer (current-buffer (current-session)))\n\
               \"OK\")",
        )
        .unwrap();
    assert!(scope_err.contains("user-only"), "expected user-only rejection for #:buffer, got {scope_err}");
    let mode_err = rt
        .eval(
            "(guard (e (#t (if (error-object? e) (error-object-message e) \"err\")))\n\
               (set-option! 'gpg-program \"/tmp/evil\" #:mode 'text-mode)\n\
               \"OK\")",
        )
        .unwrap();
    assert!(mode_err.contains("user-only"), "expected user-only rejection for #:mode, got {mode_err}");

    let schema_err = rt
        .eval(
            "(guard (e (#t (if (error-object? e) (error-object-message e) \"err\")))\n\
               (set-option! 'gpg-recipients \"not-a-list\")\n\
               \"OK\")",
        )
        .unwrap();
    assert_ne!(schema_err, "\"OK\"", "expected schema validation error for gpg-recipients");

    // 2. Explicit (parameterize ((default-gpg-armor #f)) ...) overrides (set-option! 'gpg-armor #t)!
    let p1 = dir.path().join("opt1.arm");
    let p2 = dir.path().join("opt2.arm");
    std::fs::write(&p1, "x").unwrap();
    std::fs::write(&p2, "x").unwrap();
    let param_prec = rt
        .eval(&format!(
            "(begin\n\
               (define-codec arm-probe\n\
                 \"Probes default-gpg-armor inside codec decode.\"\n\
                 #:match (lambda (p) (string-suffix? \".arm\" p))\n\
                 #:decode (lambda (_p _raw) (make-codec-result (if (default-gpg-armor) \"armored\" \"binary\")))\n\
                 #:encode (lambda (_p text _env) (string->utf8 text))\n\
                 #:private #t)\n\
               (set-option! 'gpg-armor #t)\n\
               (let ((from-opt (document-string (file-document {:?})))\n\
                     (from-param (parameterize ((default-gpg-armor #f))\n\
                                   (document-string (file-document {:?})))))\n\
                 (list from-opt from-param)))",
            p1.display().to_string(),
            p2.display().to_string()
        ))
        .unwrap();
    assert_eq!(param_prec, "(\"armored\" \"binary\")");

    // 3. Reloading a clean codec document whose on-disk bytes haven't changed skips redundant decode!
    let reload_count = rt
        .eval(&format!(
            "(begin\n\
               (define %cnt 0)\n\
               (define-codec cnt-codec\n\
                 \"Counting codec.\"\n\
                 #:match (lambda (p) (string-suffix? \".cnt\" p))\n\
                 #:decode (lambda (_p raw)\n\
                            (set! %cnt (+ %cnt 1))\n\
                            (make-codec-result (if raw (substring (utf8->string raw) 2) \"\")))\n\
                 #:encode (lambda (_p text _env) (string->utf8 (string-append \"C:\" text)))\n\
                 #:private #t)\n\
               (let ((d (file-document {:?})))\n\
                 (reload-document! d)\n\
                 (reload-document! d)\n\
                 (list %cnt (document-string d))))",
            tracked.display().to_string()
        ))
        .unwrap();
    assert_eq!(reload_count, "(1 \"hello\\n\")");
}

#[cfg(unix)]
#[test]
fn live_attach_and_open_reject_non_utf8_canonical_target_without_opening_fffd_sibling() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let socket = root.join("socket");

    let bad_dir = root.join(OsString::from_vec(b"bad-\xff-dir".to_vec()));
    let fffd_dir = root.join("bad-\u{FFFD}-dir");
    std::fs::create_dir(&bad_dir).unwrap();
    std::fs::create_dir(&fffd_dir).unwrap();

    let mut gz = Command::new("gzip").args(["-c", "-n"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().expect("spawn gzip");
    gz.stdin.take().unwrap().write_all(b"NON-UTF8-LIVE-GZ\n").unwrap();
    let gz_bad = gz.wait_with_output().unwrap().stdout;
    let mut gz2 = Command::new("gzip").args(["-c", "-n"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().expect("spawn gzip");
    gz2.stdin.take().unwrap().write_all(b"SIBLING-FFFD-LIVE-GZ\n").unwrap();
    let gz_fffd = gz2.wait_with_output().unwrap().stdout;

    let bad_gz = bad_dir.join("secret.txt.gz");
    let fffd_gz = fffd_dir.join("secret.txt.gz");
    std::fs::write(&bad_gz, &gz_bad).unwrap();
    std::fs::write(&fffd_gz, &gz_fffd).unwrap();

    let utf8_gz_link = root.join("utf8-gz-link.txt.gz");
    std::os::unix::fs::symlink(&bad_gz, &utf8_gz_link).unwrap();

    let e = EditorHost::start(&socket, "emacs", |_| {});
    let _ = e.snapshot(|_| true);

    // 1. Live frontend attach (Msg::Attach -> attach_live) must reject the non-UTF-8 canonical target.
    let (tx2, rx2) = mpsc::channel();
    let _second = e.host.attach(
        Some(techne_editor::runtime::File { path: utf8_gz_link.clone(), journal: std::path::PathBuf::new() }),
        "emacs".into(),
        move |ev| drop(tx2.send(ev)),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let attach_err = loop {
        let left = deadline.checked_duration_since(Instant::now()).expect("attach failure in time");
        match rx2.recv_timeout(left).expect("second frontend event") {
            Event::Failed(msg) => break msg,
            Event::Output(Output::Snapshot(_)) => panic!("live attach must not succeed for non-UTF-8 canonical target"),
            _ => {}
        }
    };
    assert!(attach_err.contains("not valid UTF-8"), "expected UTF-8 rejection on live attach, got: {attach_err}");

    // 2. External server::open (Msg::Open -> open_for -> editor-open!) must also reject it.
    match server::open(&socket, &utf8_gz_link, true) {
        Err(server::OpenError::Failed(msg)) => {
            assert!(msg.contains("not valid UTF-8"), "expected UTF-8 rejection on server::open, got: {msg}");
        }
        other => panic!("expected OpenError::Failed for non-UTF-8 canonical target, got {other:?}"),
    }

    // 3. The literal U+FFFD sibling must never have been opened or registered.
    let files_scm = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lisp/editor/files.scm");
    let check = server::eval(
        &socket,
        &server::Request::Eval {
            source: format!(
                "(begin (require {:?}) (list (open-document-at {:?}) (filter (lambda (f) f) (map (lambda (b) (document-path (buffer-document b))) (buffer-list)))))",
                files_scm.to_str().unwrap(),
                fffd_gz.to_str().unwrap(),
            ),
            module: None,
            detached: false,
            principal: None,
            timeout_ms: None,
        },
    )
    .unwrap();
    assert_eq!(check.result, Ok("(#f ())".into()), "U+FFFD sibling must not be opened or registered");

    e.host.close();
}
