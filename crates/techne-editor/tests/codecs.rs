//! Editor integration tests for Slice 3:
//! - `private` retention profile (`Retention::None`, zero journal/lock files,
//!   0o600 on new files, LSP refused, excluded from session persistence,
//!   fail-closed on non-private -> private reopen, relocation without journal)
//! - `(techne codec)` + built-in `gzip` codec (mode + tree-sitter language via
//!   logical path, checked save, external change refusal with Overwrite /
//!   Revert / Compare plaintext diff, reload, revert, new `.md.gz` creation)
//! - Custom `#:private #t` codec with envelope preservation and concurrent edit
//!   during async encode.

use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

use techne_editor::runtime::{Client, File, Runtime, journal_for, journal_path_ro};

fn gzip_bytes(text: &str) -> Vec<u8> {
    let mut child = Command::new("gzip").args(["-c", "-n"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().expect("spawn gzip");
    child.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "gzip failed: {}", String::from_utf8_lossy(&out.stderr));
    out.stdout
}

fn gunzip_string(bytes: &[u8]) -> String {
    let mut child = Command::new("gzip").args(["-dc"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().expect("spawn gzip -dc");
    child.stdin.take().unwrap().write_all(bytes).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "gzip -dc failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

fn assert_no_journal_or_lock(path: &Path) {
    let j = journal_path_ro(path).unwrap();
    let mut lock = j.as_os_str().to_os_string();
    lock.push(".lock");
    let lock = std::path::PathBuf::from(lock);
    assert!(!j.exists(), "unexpected journal created for private file: {}", j.display());
    assert!(!lock.exists(), "unexpected lock file created for private file: {}", lock.display());
}

#[test]
fn private_profile_no_journal_no_lsp_no_session_and_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let scratch_file = dir.path().join("normal.scm");
    let secret_file = dir.path().join("secret.scm");
    let moved_secret = dir.path().join("moved-secret.scm");
    std::fs::write(&scratch_file, "(define normal 1)\n").unwrap();

    let (mut rt, _) = Runtime::open(&scratch_file, &dir.path().join("normal.journal"), "emacs").unwrap();

    // Open a brand-new file under the private profile.
    let res = rt
        .eval(&format!(
            "(let* ((d (file-document {:?} #:private #t))\n\
                    (b (add-buffer! d))\n\
                    (v (make-view d \"user\")))\n\
               (show-document! (current-session) d)\n\
               (view-edit! v (list (list 0 0 \"(define api-key \\\"TOP-SECRET\\\")\\n\")) \"new\")\n\
               (view-undo! v)\n\
               (view-redo! v)\n\
               (save-document! d)\n\
               (let ((quiet-lsp (lsp-attach! b #:quiet #t))\n\
                     (loud-lsp (guard (e (#t (error-object-message e))) (lsp-attach! b) \"UNEXPECTED\"))\n\
                     (state (editor-session-state (current-session))))\n\
                 (list (document-retention d)\n\
                       (document-private? d)\n\
                       (option b 'lsp-auto)\n\
                       (null? quiet-lsp)\n\
                       (and (string-contains loud-lsp \"private\") #t)\n\
                       (and (string-contains state \"secret.scm\") #t)\n\
                       (and (string-contains state \"normal.scm\") #t))))",
            secret_file.display().to_string()
        ))
        .unwrap();
    assert_eq!(res, "(none #t #f #t #t #f #t)");

    // Newly created private file has mode 0o600 and zero journal/lock files.
    let mode = std::fs::metadata(&secret_file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "new private file should be created with 0o600");
    assert_no_journal_or_lock(&secret_file);

    // Relocating a private file via move-path! creates zero journal/lock files.
    let moved_res = rt
        .eval(&format!(
            "(begin (move-path! {:?} {:?})\n\
                    (let ((d (file-document {:?} #:private #t)))\n\
                      (list (document-retention d) (document-private? d))))",
            secret_file.display().to_string(),
            moved_secret.display().to_string(),
            moved_secret.display().to_string()
        ))
        .unwrap();
    assert_eq!(moved_res, "(none #t)");
    assert_no_journal_or_lock(&secret_file);
    assert_no_journal_or_lock(&moved_secret);

    // Reopening an already-open non-private file with #:private #t MUST fail closed.
    let fail_closed = rt
        .eval(&format!(
            "(guard (e (#t (and (error-object? e)\n\
                               (string-contains (error-object-message e) \"private\")\n\
                               #t)))\n\
               (file-document {:?} #:private #t)\n\
               #f)",
            scratch_file.display().to_string()
        ))
        .unwrap();
    assert_eq!(fail_closed, "#t");

    assert!(rt.take_errors().is_empty());
}

#[test]
fn gzip_codec_end_to_end_mode_syntax_checked_save_recovery_and_reload() {
    let dir = tempfile::tempdir().unwrap();
    let gz_scm = dir.path().join("sample.scm.gz");
    let gz_md = dir.path().join("notes.md.gz");
    std::fs::write(&gz_scm, gzip_bytes("(define x 1)\n")).unwrap();

    // Attach directly to the .scm.gz file (tests Runtime::attach codec routing).
    let mut rt = Runtime::new().unwrap();
    rt.attach(Client::FIRST, Some(&File { path: gz_scm.clone(), journal: dir.path().join("unused.journal") }), "emacs").unwrap();

    // Verify decompressed text, logical mode, tree-sitter language, codec & retention.
    let info = rt
        .eval(
            "(let* ((b (current-buffer (current-session)))\n\
                    (d (buffer-document b)))\n\
               (list (document-string d)\n\
                     (buffer-mode b)\n\
                     (buffer-language b)\n\
                     (codec-name (document-codec d))\n\
                     (document-retention d)\n\
                     (document-private? d)))",
        )
        .unwrap();
    assert_eq!(info, "(\"(define x 1)\\n\" scheme-mode scheme gzip journal #f)");

    // Edit and save; verify compressed file on disk decompresses to edited text.
    rt.eval(
        "(let* ((d (buffer-document (current-buffer (current-session))))\n\
                (v (make-view d \"user\")))\n\
           (view-edit! v (list (list 0 (document-length d) \"(define x 42)\\n\")) \"new\")\n\
           (save-document! d))",
    )
    .unwrap();
    assert_eq!(gunzip_string(&std::fs::read(&gz_scm).unwrap()), "(define x 42)\n");

    // External modification on disk: edit buffer and try checked save -> refused with choices.
    std::fs::write(&gz_scm, gzip_bytes("(define x 99)\n")).unwrap();
    let refusal = rt
        .eval(
            "(let* ((d (buffer-document (current-buffer (current-session))))\n\
                    (v (make-view d \"user\")))\n\
               (view-edit! v (list (list 0 (document-length d) \"(define x 77)\\n\")) \"new\")\n\
               (guard (e (#t (list (and (changed-on-disk? e) #t)\n\
                                   (map choice-label (or (condition-choices e) '())))))\n\
                 (save-document! d)\n\
                 'unexpected))",
        )
        .unwrap();
    assert_eq!(refusal, "(#t (\"Overwrite\" \"Revert\" \"Compare\"))");

    // Compare (diff-buffer-with-file) uses file-text which decodes gzip to plaintext asynchronously!
    rt.eval(
        "(let* ((s (current-session))\n\
                (d (buffer-document (current-buffer s))))\n\
           (invoke s 'diff-buffer-with-file (dict \"document\" d)))",
    )
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let _ = rt.run_tasks(std::time::Duration::from_millis(20));
        if rt.eval("(and (buffer-named \"*diff*\") #t)").unwrap() == "#t" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let diff = rt.eval("(document-string (buffer-document (buffer-named \"*diff*\")))").unwrap();
    assert!(diff.contains("-(define x 99)"), "diff should show external plaintext: {diff}");
    assert!(diff.contains("+(define x 77)"), "diff should show buffer plaintext: {diff}");

    // Revert brings external gzip text into buffer, and undo restores our unsaved edit.
    let rev_res = rt
        .eval(
            "(let* ((d (buffer-document (current-buffer (current-session))))\n\
                    (v (make-view d \"user\")))\n\
               (let* ((changed (revert-document! d))\n\
                      (after-revert (document-string d)))\n\
                 (view-undo! v)\n\
                 (list changed after-revert (document-string d))))",
        )
        .unwrap();
    assert_eq!(rev_res, "(#t \"(define x 99)\\n\" \"(define x 77)\\n\")");

    // Explicit overwrite replaces disk file with buffer text.
    rt.eval("(save-document! (buffer-document (current-buffer (current-session))) #:overwrite #t)").unwrap();
    assert_eq!(gunzip_string(&std::fs::read(&gz_scm).unwrap()), "(define x 77)\n");

    // Clean reload picks up another external gzip update.
    std::fs::write(&gz_scm, gzip_bytes("(define x 123)\n")).unwrap();
    let reloaded = rt
        .eval(
            "(let ((d (buffer-document (current-buffer (current-session)))))\n\
               (list (reload-document! d) (document-string d)))",
        )
        .unwrap();
    assert_eq!(reloaded, "(#t \"(define x 123)\\n\")");

    // Opening a brand-new nonexistent .md.gz picks text-mode + markdown language and creates valid gzip on save.
    let new_md = rt
        .eval(&format!(
            "(let* ((d (file-document {:?}))\n\
                    (b (add-buffer! d))\n\
                    (v (make-view d \"user\")))\n\
               (view-edit! v (list (list 0 0 \"# Hello\\n\")) \"new\")\n\
               (save-document! d)\n\
               (list (buffer-mode b) (buffer-language b)))",
            gz_md.display().to_string()
        ))
        .unwrap();
    assert_eq!(new_md, "(text-mode markdown)");
    assert_eq!(gunzip_string(&std::fs::read(&gz_md).unwrap()), "# Hello\n");

    assert!(rt.take_errors().is_empty());
}

#[test]
fn custom_private_codec_preserves_envelope_and_handles_concurrent_edit_during_async_save() {
    let dir = tempfile::tempdir().unwrap();
    let scratch_file = dir.path().join("base.txt");
    let box_file = dir.path().join("vault.scm.sbox");
    std::fs::write(&scratch_file, "base\n").unwrap();
    std::fs::write(&box_file, "RECIPIENT:alice@example.org\n(define secret 1)\n").unwrap();

    let (mut rt, _) = Runtime::open(&scratch_file, &dir.path().join("base.journal"), "emacs").unwrap();

    // Register a custom private codec whose encode waits asynchronously on a channel
    // so the test deterministically injects a concurrent edit mid-save.
    let opened = rt
        .eval(&format!(
            "(begin\n\
               (define %encode-entered (make-channel 1))\n\
               (define %encode-release (make-channel 1))\n\
               (define-codec sbox\n\
                 \"Private envelope test codec.\"\n\
                 #:match (lambda (p) (string-suffix? \".sbox\" p))\n\
                 #:logical-path (lambda (p) (substring p 0 (- (string-length p) 5)))\n\
                 #:init-envelope (lambda (_p) '((recipient . \"default\")))\n\
                 #:decode (lambda (_p raw)\n\
                            (if (not raw)\n\
                                (make-codec-result \"\" #:envelope '((recipient . \"default\")))\n\
                                (let* ((s (utf8->string raw))\n\
                                       (nl (string-index s #\\newline))\n\
                                       (hdr (substring s 0 nl))\n\
                                       (body (substring s (+ nl 1))))\n\
                                  (make-codec-result body #:envelope (list (cons 'recipient hdr))))))\n\
                 #:encode (lambda (_p text env)\n\
                            (channel-send %encode-entered #t)\n\
                            (channel-recv %encode-release)\n\
                            (let ((hdr (cdr (assq 'recipient env))))\n\
                              (string->utf8 (string-append hdr \"\\n\" text))))\n\
                 #:private #t\n\
                 #:priority 20)\n\
               (define %box-doc (file-document {:?}))\n\
               (define %box-buf (add-buffer! %box-doc))\n\
               (show-document! (current-session) %box-doc)\n\
               (list (document-string %box-doc)\n\
                     (document-codec-envelope %box-doc)\n\
                     (document-retention %box-doc)\n\
                     (document-private? %box-doc)\n\
                     (buffer-mode %box-buf)\n\
                     (buffer-language %box-buf)\n\
                     (and (string-contains (editor-session-state (current-session)) \"vault.scm.sbox\") #t)))",
            box_file.display().to_string()
        ))
        .unwrap();
    assert_eq!(opened, "(\"(define secret 1)\\n\" ((recipient . \"RECIPIENT:alice@example.org\")) none #t scheme-mode scheme #f)");
    assert_no_journal_or_lock(&box_file);

    // Edit -> start async save -> wait until encode is running -> make a concurrent edit ->
    // release encode -> verify disk has rev 1 with preserved envelope, buffer keeps rev 2, and dirty? is #t!
    let concurrent = rt
        .eval(
            "(let ((v (make-view %box-doc \"user\")))\n\
               (view-edit! v (list (list 0 (document-length %box-doc) \"(define secret 2)\\n\")) \"new\")\n\
               (let ((save-task (spawn (lambda () (save-document! %box-doc)))))\n\
                 (channel-recv %encode-entered)\n\
                 (let ((second-save-err (guard (e (#t (error-object-message e))) (save-document! %box-doc) #f)))\n\
                   (view-edit! v (list (list (document-length %box-doc) (document-length %box-doc) \";; concurrent\\n\")) \"new\")\n\
                   (channel-send %encode-release #t)\n\
                   (task-join save-task)\n\
                   (list (and (string? second-save-err) (string-contains second-save-err \"under way\") #t)\n\
                         (document-dirty? %box-doc)\n\
                         (document-string %box-doc)))))",
        )
        .unwrap();
    rt.run_tasks(Duration::from_millis(10)).unwrap();
    assert_eq!(concurrent, "(#t #t \"(define secret 2)\\n;; concurrent\\n\")");
    assert_eq!(std::fs::read_to_string(&box_file).unwrap(), "RECIPIENT:alice@example.org\n(define secret 2)\n");
    assert_no_journal_or_lock(&box_file);

    // External modification while encoding -> checked save refuses!
    let ext_race = rt
        .eval(&format!(
            "(let ((save-task (spawn (lambda ()\n\
                                      (guard (e (#t (and (changed-on-disk? e) #t)))\n\
                                        (save-document! %box-doc)\n\
                                        #f)))))\n\
               (channel-recv %encode-entered)\n\
               (call-with-output-file {:?}\n\
                 (lambda (p) (display \"RECIPIENT:bob@example.org\\n(define secret 999)\\n\" p)))\n\
               (channel-send %encode-release #t)\n\
               (task-join save-task))",
            box_file.display().to_string()
        ))
        .unwrap();
    assert_eq!(ext_race, "#t");

    // Reverting picks up the new external envelope (bob@example.org) and text!
    let reverted = rt
        .eval(
            "(begin\n\
               (revert-document! %box-doc)\n\
               (list (document-string %box-doc) (document-codec-envelope %box-doc)))",
        )
        .unwrap();
    assert_eq!(reverted, "(\"(define secret 999)\\n\" ((recipient . \"RECIPIENT:bob@example.org\")))");
    assert_no_journal_or_lock(&box_file);

    assert!(rt.take_errors().is_empty());
}

#[test]
fn attach_fails_closed_when_codec_matcher_throws() {
    let dir = tempfile::tempdir().unwrap();
    let boom_file = dir.path().join("vault.boom");
    std::fs::write(&boom_file, "CIPHERTEXT\n").unwrap();

    let mut rt = Runtime::new().unwrap();
    rt.eval(
        "(define-codec exploding\n\
           \"Throws during match.\"\n\
           #:match (lambda (p) (if (string-suffix? \".boom\" p) (error \"matcher boom\") #f))\n\
           #:decode (lambda (_p _raw) \"\")\n\
           #:encode (lambda (_p _text _env) #u8())\n\
           #:private #t)",
    )
    .unwrap();

    let err = rt.attach(Client::FIRST, Some(&File { path: boom_file.clone(), journal: std::path::PathBuf::new() }), "emacs").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("matcher boom"), "attach must surface matcher error instead of falling open: {msg}");
    assert_no_journal_or_lock(&boom_file);
}

#[test]
fn decoded_raw_io_and_open_file_reuse_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let gz_file = dir.path().join("data.scm.gz");
    let priv_file = dir.path().join("secret.txt");
    std::fs::write(&gz_file, gzip_bytes("(define a 1)\n")).unwrap();
    std::fs::write(&priv_file, "top-secret\n").unwrap();

    let (mut rt, _) = Runtime::open(&dir.path().join("base.txt"), &dir.path().join("base.journal"), "emacs").unwrap();

    let res = rt
        .eval(&format!(
            "(let ((d (file-document {:?}))\n\
                   (p (file-document {:?} #:private #t)))\n\
               (list (document-decoded? d)\n\
                     (document-decoded? p)\n\
                     (guard (e (#t (and (string-contains (error-object-message e) \"codec\") #t))) (document-save! d) #f)\n\
                     (guard (e (#t (and (string-contains (error-object-message e) \"codec\") #t))) (document-reload! d) #f)\n\
                     (guard (e (#t (and (string-contains (error-object-message e) \"codec\") #t))) (document-revert! d \"user\") #f)\n\
                     (guard (e (#t (and (string-contains (error-object-message e) \"codec\") #t))) (open-file {:?}) #f)\n\
                     (guard (e (#t (and (string-contains (error-object-message e) \"without a journal\") #t))) (open-file {:?}) #f)))",
            gz_file.display().to_string(),
            priv_file.display().to_string(),
            gz_file.display().to_string(),
            priv_file.display().to_string(),
        ))
        .unwrap();
    assert_eq!(res, "(#t #f #t #t #t #t #t)");
    assert_no_journal_or_lock(&priv_file);
    assert!(rt.take_errors().is_empty());
}

#[test]
fn symlink_to_private_codec_opens_privately_and_reuses_canonically() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("vault.scm.sbox");
    let link = dir.path().join("notes.txt");
    std::fs::write(&target, "HDR\n(define secret 7)\n").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let reg_sbox = "(define-codec sbox\n\
                      \"Private test codec.\"\n\
                      #:match (lambda (p) (string-suffix? \".sbox\" p))\n\
                      #:logical-path (lambda (p) (substring p 0 (- (string-length p) 5)))\n\
                      #:decode (lambda (_p raw)\n\
                                 (if (not raw)\n\
                                     (make-codec-result \"\" #:envelope \"HDR\")\n\
                                     (let* ((s (utf8->string raw))\n\
                                            (nl (string-index s #\\newline)))\n\
                                       (make-codec-result (substring s (+ nl 1)) #:envelope (substring s 0 nl)))))\n\
                      #:encode (lambda (_p text env) (string->utf8 (string-append env \"\\n\" text)))\n\
                      #:private #t)";

    // 1. Via file-document
    let (mut rt, _) = Runtime::open(&dir.path().join("base.txt"), &dir.path().join("base.journal"), "emacs").unwrap();
    rt.eval(reg_sbox).unwrap();
    let res = rt
        .eval(&format!(
            "(let* ((d1 (file-document {:?}))\n\
                    (d2 (file-document {:?})))\n\
               (list (document-string d1)\n\
                     (codec-name (document-codec d1))\n\
                     (document-retention d1)\n\
                     (document-private? d1)\n\
                     (document=? d1 d2)))",
            link.display().to_string(),
            target.display().to_string(),
        ))
        .unwrap();
    assert_eq!(res, "(\"(define secret 7)\\n\" sbox none #t #t)");
    assert_no_journal_or_lock(&link);
    assert_no_journal_or_lock(&target);

    // 2. Via Runtime::attach directly to the symlink
    let mut rt2 = Runtime::new().unwrap();
    rt2.eval(reg_sbox).unwrap();
    rt2.attach(Client::FIRST, Some(&File { path: link.clone(), journal: std::path::PathBuf::new() }), "emacs").unwrap();
    let attached = rt2
        .eval(
            "(let ((d (buffer-document (current-buffer (current-session)))))\n\
               (list (document-string d) (codec-name (document-codec d)) (document-retention d) (document-private? d)))",
        )
        .unwrap();
    assert_eq!(attached, "(\"(define secret 7)\\n\" sbox none #t)");
    assert_no_journal_or_lock(&link);
    assert_no_journal_or_lock(&target);
}

#[test]
fn move_path_refuses_codec_and_private_boundary_crossings() {
    let dir = tempfile::tempdir().unwrap();
    let normal = dir.path().join("normal.txt");
    let dest_sbox = dir.path().join("dest.sbox");
    let gz_a = dir.path().join("a.txt.gz");
    let gz_b = dir.path().join("b.txt.gz");
    let raw_a = dir.path().join("a.txt");
    let priv_a = dir.path().join("p1.sbox");
    let priv_b = dir.path().join("p2.sbox");

    std::fs::write(&normal, "hello\n").unwrap();
    std::fs::write(&gz_a, gzip_bytes("compressed\n")).unwrap();
    std::fs::write(&priv_a, "secret\n").unwrap();

    let (mut rt, _) = Runtime::open(&normal, &dir.path().join("normal.journal"), "emacs").unwrap();
    let res = rt
        .eval(&format!(
            "(begin\n\
               (define-codec sbox \"Private.\"\n\
                 #:match (lambda (p) (string-suffix? \".sbox\" p))\n\
                 #:decode (lambda (_p raw) (if raw (utf8->string raw) \"\"))\n\
                 #:encode (lambda (_p text _e) (string->utf8 text))\n\
                 #:private #t)\n\
               (define d-gz (file-document {:?}))\n\
               (define d-priv (file-document {:?}))\n\
               (let ((r1 (guard (e (#t (and (error-object? e) #t))) (move-path! {:?} {:?}) #f))\n\
                     (r2 (guard (e (#t (and (error-object? e) #t))) (move-path! {:?} {:?}) #f)))\n\
                 (move-path! {:?} {:?})\n\
                 (move-path! {:?} {:?})\n\
                 (list r1 r2 (document-path d-gz) (document-path d-priv))))",
            gz_a.display().to_string(),
            priv_a.display().to_string(),
            normal.display().to_string(),
            dest_sbox.display().to_string(),
            gz_a.display().to_string(),
            raw_a.display().to_string(),
            gz_a.display().to_string(),
            gz_b.display().to_string(),
            priv_a.display().to_string(),
            priv_b.display().to_string(),
        ))
        .unwrap();
    assert!(res.starts_with("(#t #t "), "expected both invalid moves refused and valid moves ok: {res}");
    assert!(normal.exists() && !dest_sbox.exists());
    assert!(!gz_a.exists() && gz_b.exists() && !raw_a.exists());
    assert!(!priv_a.exists() && priv_b.exists());
    assert_no_journal_or_lock(&priv_a);
    assert_no_journal_or_lock(&priv_b);
}

#[test]
fn preexisting_journal_fails_closed_for_private_open_without_creating_lock() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("had-journal.txt");
    std::fs::write(&target, "initial\n").unwrap();

    // Open normally (journaled), make an unsaved edit, then drop runtime.
    {
        let mut rt = Runtime::new().unwrap();
        rt.attach(Client::FIRST, Some(&File { path: target.clone(), journal: std::path::PathBuf::new() }), "emacs").unwrap();
        rt.eval(
            "(let* ((d (buffer-document (current-buffer (current-session))))\n\
                    (v (make-view d \"user\")))\n\
               (view-edit! v (list (list 0 0 \"UNSAVED:\")) \"new\"))",
        )
        .unwrap();
    }

    // Remove the lock file while keeping the journal on disk.
    let j = journal_for(&target).unwrap();
    assert!(j.exists(), "expected journal on disk after normal open");
    let mut lock = j.as_os_str().to_os_string();
    lock.push(".lock");
    let lock = std::path::PathBuf::from(lock);
    let _ = std::fs::remove_file(&lock);
    assert!(!lock.exists());

    // Now opening with #:private #t MUST fail closed and MUST NOT create a lock file!
    let (mut rt2, _) = Runtime::open(&dir.path().join("other.txt"), &dir.path().join("other.journal"), "emacs").unwrap();
    let refused = rt2
        .eval(&format!(
            "(guard (e (#t (and (error-object? e)\n\
                               (string-contains (error-object-message e) \"unsaved edits\")\n\
                               #t)))\n\
               (file-document {:?} #:private #t)\n\
               #f)",
            target.display().to_string(),
        ))
        .unwrap();
    assert_eq!(refused, "#t");
    assert!(!lock.exists(), "failed private open must not create a lock file");

    // Clean up journal so test dir doesn't leave anything in state dir.
    let _ = std::fs::remove_file(&j);
}

#[test]
fn private_buffer_suppresses_history_and_read_file_name_excludes_open_private() {
    use techne_editor::present::Input;

    let dir = tempfile::tempdir().unwrap();
    let normal = dir.path().join("normal.txt");
    let priv_file = dir.path().join("my-secret.txt");
    std::fs::write(&normal, "hello world\n").unwrap();
    std::fs::write(&priv_file, "top secret token\n").unwrap();

    let (mut rt, _) = Runtime::open(&normal, &dir.path().join("normal.journal"), "emacs").unwrap();
    // Open and focus the private buffer.
    rt.eval(&format!(
        "(let ((dp (file-document {:?} #:private #t)))\n\
           (add-buffer! dp)\n\
           (show-document! (current-session) dp))",
        priv_file.display().to_string(),
    ))
    .unwrap();

    // 1. Minibuffer history while private buffer is focused is suppressed.
    rt.eval(
        "(let ((s (current-session)))\n\
           (completing-read s \"Eval: \" '() #:initial \"secret-eval-input\" #:history 'eval #:require-match #f #:accept (lambda (_s _c) #t))\n\
           ((command 'minibuffer-accept-input) s 1))",
    )
    .unwrap();

    // 2. Isearch while private buffer is focused does not record 'isearch history.
    for key in ["C-s", "t", "o", "k", "e", "n", "RET"] {
        rt.handle(Input::Key { key: key.into(), at: std::time::Instant::now() });
    }

    // 3. Switch back to normal buffer: minibuffer history works again, and read-file-name
    // excludes the already-open private file from 'file-name history.
    let res = rt
        .eval(&format!(
            "(let* ((s (current-session))\n\
                    (dn (file-document {:?}))\n\
                    (eval-priv (minibuffer-history 'eval))\n\
                    (isearch-priv (minibuffer-history 'isearch)))\n\
               (show-document! s dn)\n\
               (completing-read s \"Eval: \" '() #:initial \"public-eval-input\" #:history 'eval #:require-match #f #:accept (lambda (_s _c) #t))\n\
               ((command 'minibuffer-accept-input) s 1)\n\
               (read-file-name s \"Find: \" {:?} (lambda (_s _p) #t))\n\
               ((command 'minibuffer-accept-input) s 1)\n\
               (list (not (member \"secret-eval-input\" eval-priv))\n\
                     (not (member \"token\" isearch-priv))\n\
                     (and (member \"public-eval-input\" (minibuffer-history 'eval)) #t)\n\
                     (not (member {:?} (minibuffer-history 'file-name)))))",
            normal.display().to_string(),
            priv_file.display().to_string(),
            priv_file.display().to_string(),
        ))
        .unwrap();
    assert_eq!(res, "(#t #t #t #t)");
    assert_no_journal_or_lock(&priv_file);
}

#[test]
fn higher_priority_non_private_codec_cannot_strip_privacy_and_attach_recovers_codec_journal() {
    let dir = tempfile::tempdir().unwrap();
    let dual = dir.path().join("item.dual");
    std::fs::write(&dual, "hello\n").unwrap();

    let (mut rt, _) = Runtime::open(&dir.path().join("base.txt"), &dir.path().join("base.journal"), "emacs").unwrap();
    let res = rt
        .eval(&format!(
            "(begin\n\
               (define-codec priv-low \"Private lower.\"\n\
                 #:match (lambda (p) (string-suffix? \".dual\" p))\n\
                 #:decode (lambda (_p r) (if r (utf8->string r) \"\"))\n\
                 #:encode (lambda (_p t _e) (string->utf8 t))\n\
                 #:private #t #:priority 5)\n\
               (define-codec pub-high \"Public higher.\"\n\
                 #:match (lambda (p) (string-suffix? \".dual\" p))\n\
                 #:decode (lambda (_p r) (if r (utf8->string r) \"\"))\n\
                 #:encode (lambda (_p t _e) (string->utf8 t))\n\
                 #:private #f #:priority 20)\n\
               (let ((d (file-document {:?})))\n\
                 (list (codec-name (document-codec d))\n\
                       (document-retention d)\n\
                       (document-private? d))))",
            dual.display().to_string(),
        ))
        .unwrap();
    assert_eq!(res, "(pub-high none #t)");
    assert_no_journal_or_lock(&dual);

    // Codec-backed journaled attach preserves and reports recovered unsaved edits across restart.
    let gz = dir.path().join("recover.scm.gz");
    std::fs::write(&gz, gzip_bytes("(define v 1)\n")).unwrap();
    {
        let mut rt1 = Runtime::new().unwrap();
        let rec1 = rt1.attach(Client::FIRST, Some(&File { path: gz.clone(), journal: std::path::PathBuf::new() }), "emacs").unwrap();
        assert_eq!(rec1, Some(techne_text::Recovery::Clean));
        rt1.eval(
            "(let* ((d (buffer-document (current-buffer (current-session))))\n\
                    (v (make-view d \"user\")))\n\
               (view-edit! v (list (list 0 (document-length d) \"(define v 999)\\n\")) \"new\"))",
        )
        .unwrap();
    }
    {
        let mut rt2 = Runtime::new().unwrap();
        let rec2 = rt2.attach(Client::FIRST, Some(&File { path: gz.clone(), journal: std::path::PathBuf::new() }), "emacs").unwrap();
        assert_eq!(rec2, Some(techne_text::Recovery::Replayed { transactions: 1, torn: false }));
        let rec = rt2
            .eval(
                "(let ((d (buffer-document (current-buffer (current-session)))))\n\
                   (list (document-string d) (document-dirty? d) (codec-name (document-codec d))))",
            )
            .unwrap();
        assert_eq!(rec, "(\"(define v 999)\\n\" #t gzip)");
    }
    let j = journal_for(&gz).unwrap();
    let _ = std::fs::remove_file(&j);
    let mut lock = j.as_os_str().to_os_string();
    lock.push(".lock");
    let _ = std::fs::remove_file(std::path::PathBuf::from(lock));
}

#[test]
fn register_codec_requires_files_capability_and_cold_import_survives_scope_shutdown() {
    use techne_vm::vm::{Capability, Grants, Vm};

    // 1. register-codec! fails closed without Capability::Files.
    let mut restricted = Runtime::granted(Grants::NONE.with(Capability::Loading)).unwrap();
    let denied = restricted
        .eval(
            "(guard (e (#t (and (error-object? e) #t)))\n\
               (define-codec sneak \"Denied.\"\n\
                 #:match (lambda (_p) #t)\n\
                 #:decode (lambda (_p _r) \"\")\n\
                 #:encode (lambda (_p _t _e) #u8()))\n\
               #f)",
        )
        .unwrap();
    assert_eq!(denied, "#t");

    // 2. Cold import of (techne codec) inside a temporary scope preserves built-in gzip after scope-shutdown!.
    let mut vm = Vm::new();
    techne_process::install(&mut vm).unwrap();
    vm.eval_source(
        "(begin\n\
           (define sc (make-scope 'cold-import))\n\
           (with-scope sc\n\
             (eval '(begin (import (techne codec)) (codec-names)) (interaction-environment)))\n\
           (scope-shutdown! sc)\n\
           (unless (eval '(and (memq 'gzip (codec-names)) #t) (interaction-environment))\n\
             (error \"built-in gzip codec disappeared after cold-import scope shutdown\")))",
    )
    .unwrap();
}

#[test]
fn alias_symlink_legacy_journal_fails_closed_for_private_open_and_mid_decode_change_retries() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.txt");
    let alias = dir.path().join("alias.txt");
    std::fs::write(&target, "hello\n").unwrap();
    symlink(&target, &alias).unwrap();

    // Create a legacy journal under the symlink alias path (not the canonical target path).
    let alias_journal = journal_path_ro(&alias).unwrap();
    std::fs::create_dir_all(alias_journal.parent().unwrap()).unwrap();
    std::fs::write(&alias_journal, b"legacy-journal-bytes").unwrap();

    let (mut rt, _) = Runtime::open(&dir.path().join("base.txt"), &dir.path().join("base.journal"), "emacs").unwrap();
    let refused = rt
        .eval(&format!(
            "(guard (e (#t (and (error-object? e) (string-contains (error-object-message e) \"journal\") #t)))\n\
               (file-document {:?} #:private #t)\n\
               #f)",
            alias.display().to_string()
        ))
        .unwrap();
    assert_eq!(refused, "#t");
    let _ = std::fs::remove_file(&alias_journal);

    // M5: If disk file changes mid-decode, open_decoded rejects the stale decode and file-document retries once.
    let race_file = dir.path().join("race.m5");
    std::fs::write(&race_file, "v1\n").unwrap();
    let retried = rt
        .eval(&format!(
            "(begin\n\
               (define decode-calls 0)\n\
               (define-codec m5-race \"Mutates target on first decode.\"\n\
                 #:match (lambda (p) (string-suffix? \".m5\" p))\n\
                 #:decode (lambda (p raw)\n\
                            (set! decode-calls (+ decode-calls 1))\n\
                            (when (= decode-calls 1)\n\
                              (call-with-output-file p (lambda (port) (display \"v2\\n\" port))))\n\
                            (make-codec-result (if raw (utf8->string raw) \"\")))\n\
                 #:encode (lambda (_p t _e) (string->utf8 t))\n\
                 #:private #t)\n\
               (let ((d (file-document {:?})))\n\
                 (list decode-calls (document-string d))))",
            race_file.display().to_string()
        ))
        .unwrap();
    assert_eq!(retried, "(2 \"v2\\n\")");
    assert_no_journal_or_lock(&race_file);
}

#[test]
fn large_shell_input_and_latched_private_history_and_relocation_guard() {
    let dir = tempfile::tempdir().unwrap();
    let pub_file = dir.path().join("pub.txt");
    let priv_file = dir.path().join("priv.txt");
    let moved_file = dir.path().join("moved.rel");
    let rel_file = dir.path().join("orig.rel");
    std::fs::write(&pub_file, "public\n").unwrap();
    std::fs::write(&priv_file, "secret-needle\n").unwrap();
    std::fs::write(&rel_file, "initial\n").unwrap();

    let (mut rt, _) = Runtime::open(&pub_file, &dir.path().join("pub.journal"), "emacs").unwrap();

    // M8: shell-command-on-region (run-shell!) with >256KiB stdin piped to `cat` completes without deadlocking.
    let big_res = rt
        .eval(
            "(let* ((s (current-session))\n\
                    (d (session-document s))\n\
                    (v (make-view d \"user\"))\n\
                    (big (let loop ((i 0) (acc '()))\n\
                           (if (= i 16000) (apply string-append acc) (loop (+ i 1) (cons \"0123456789abcdef\\n\" acc))))))\n\
               (view-edit! v (list (list 0 (document-length d) big)) \"new\")\n\
               (invoke s 'shell-command-on-region\n\
                 (dict \"command\" \"cat\" \"region\" (text-range d 0 (document-length d) (document-revision d)) \"replace\" #f))\n\
               (let wait ((tries 0))\n\
                 (let ((b (buffer-named \"*Shell Command Output*\")))\n\
                   (if (and b (= (document-length (buffer-document b)) (string-length big)))\n\
                       #t\n\
                       (if (> tries 200) #f (begin (sleep 10) (wait (+ tries 1))))))))",
        )
        .unwrap();
    assert_eq!(big_res, "#t");

    // M9: switching focus from private -> public while completing-read or isearch is open still suppresses history.
    rt.eval(&format!(
        "(let* ((s (current-session))\n\
                (dp (file-document {:?} #:private #t))\n\
                (dn (file-document {:?})))\n\
           (add-buffer! dp)\n\
           (show-document! s dp)\n\
           (completing-read s \"Eval: \" '() #:initial \"latched-secret-eval\" #:history 'eval #:require-match #f #:accept (lambda (_s _c) #t))\n\
           (show-document! s dn)\n\
           ((command 'minibuffer-accept-input) s 1)\n\
           (show-document! s dp))",
        priv_file.display().to_string(),
        pub_file.display().to_string()
    ))
    .unwrap();
    for key in ["C-s", "n", "e", "e", "d", "l", "e"] {
        rt.handle(techne_editor::present::Input::Key { key: key.into(), at: std::time::Instant::now() });
    }
    rt.eval(&format!("(show-document! (current-session) (file-document {:?}))", pub_file.display().to_string())).unwrap();
    rt.handle(techne_editor::present::Input::Key { key: "RET".into(), at: std::time::Instant::now() });
    let m9 = rt
        .eval(
            "(list (not (member \"latched-secret-eval\" (minibuffer-history 'eval)))\n\
                   (not (member \"needle\" (minibuffer-history 'isearch))))",
        )
        .unwrap();
    assert_eq!(m9, "(#t #t)");

    // SF1: relocating a codec document mid-encode refuses commit with "document relocated while encoding".
    let outcome = rt
        .eval(&format!(
            "(begin\n\
               (define rel-started (make-channel 1))\n\
               (define rel-go (make-channel 1))\n\
               (define-codec slow-rel \"Pauses during encode.\"\n\
                 #:match (lambda (p) (string-suffix? \".rel\" p))\n\
                 #:decode (lambda (_p r) (make-codec-result (if r (utf8->string r) \"\")))\n\
                 #:encode (lambda (_p t _e)\n\
                            (channel-send rel-started #t)\n\
                            (channel-recv rel-go)\n\
                            (string->utf8 t))\n\
                 #:private #t)\n\
               (define rel-doc (file-document {:?}))\n\
               (define rel-view (make-view rel-doc \"user\"))\n\
               (view-edit! rel-view (list (list 0 (document-length rel-doc) \"updated\\n\")) \"new\")\n\
               (let ((t (spawn (lambda ()\n\
                                 (guard (e (#t (error-object-message e)))\n\
                                   (save-document! rel-doc)\n\
                                   \"UNEXPECTED\")))))\n\
                 (channel-recv rel-started)\n\
                 (move-path! {:?} {:?})\n\
                 (channel-send rel-go #t)\n\
                 (task-join t)))",
            rel_file.display().to_string(),
            rel_file.display().to_string(),
            moved_file.display().to_string()
        ))
        .unwrap();
    assert!(outcome.contains("relocated while encoding"), "unexpected outcome: {outcome}");
    assert_eq!(std::fs::read_to_string(&moved_file).unwrap(), "initial\n");
}

#[test]
fn private_clipboard_diff_trace_redaction_and_explicit_codec_and_journal_preservation() {
    let dir = tempfile::tempdir().unwrap();
    let pub_file = dir.path().join("pub.txt");
    let priv_file = dir.path().join("priv.txt");
    let raw_gz = dir.path().join("raw.scm.gz");
    let explicit_gz = dir.path().join("explicit-journal.scm.gz");
    let custom_journal = dir.path().join("custom.journal");
    std::fs::write(&pub_file, "public line\n").unwrap();
    std::fs::write(&priv_file, "top-secret-line\n").unwrap();
    std::fs::write(&raw_gz, "plain-not-gzip\n").unwrap();
    std::fs::write(&explicit_gz, gzip_bytes("(define a 1)\n")).unwrap();

    let (mut rt, _) = Runtime::open(&pub_file, &dir.path().join("pub.journal"), "emacs").unwrap();

    // SF4: private kill-save! leaves clipboard-out #f; private diff marks *diff* private and isolates from public diff;
    // invoke on private buffer redacts string arguments in operation trace.
    let sf4 = rt
        .eval(&format!(
            "(let* ((s (current-session))\n\
                    (dp (file-document {:?} #:private #t))\n\
                    (dn (file-document {:?})))\n\
               (define-command (test-priv-note s note)\n\
                 \"Test command with a string argument.\"\n\
                 #:arguments '((note string #:doc \"A note.\"))\n\
                 note)\n\
               (add-buffer! dp)\n\
               (show-document! s dp)\n\
               (sset! s 'clipboard-out #f)\n\
               (kill-save! s \"MY-SECRET-CLIP\" #f #f)\n\
               (let ((clip-after-priv (sget s 'clipboard-out)))\n\
                 (invoke s 'test-priv-note (dict \"note\" \"SECRET-ARG\"))\n\
                 (let* ((last-op (car (reverse (traced-operation-records))))\n\
                        (args-str (call-with-output-string (lambda (p) (write (record-arguments last-op) p)))))\n\
                   (invoke s 'diff-buffer-with-file (dict \"document\" dp))\n\
                   (let* ((priv-diff-doc (buffer-document (buffer-named \"*diff*\")))\n\
                          (priv-diff-is-priv (document-private? priv-diff-doc)))\n\
                     (show-document! s dn)\n\
                     (invoke s 'diff-buffer-with-file (dict \"document\" dn))\n\
                     (let* ((pub-diff-doc (buffer-document (buffer-named \"*diff*\")))\n\
                            (reused? (document=? priv-diff-doc pub-diff-doc))\n\
                            (pub-diff-priv? (document-private? pub-diff-doc)))\n\
                       (list (not clip-after-priv)\n\
                             priv-diff-is-priv\n\
                             (not (string-contains args-str \"SECRET-ARG\"))\n\
                             (and (string-contains args-str \"<redacted>\") #t)\n\
                             (not reused?)\n\
                             (not pub-diff-priv?)))))))",
            priv_file.display().to_string(),
            pub_file.display().to_string()
        ))
        .unwrap();
    assert_eq!(sf4, "(#t #t #t #t #t #t)");

    // SF5a: explicit #:codec #f survives add-buffer! without attaching gzip codec.
    let sf5a = rt
        .eval(&format!(
            "(let* ((d (file-document {:?} #:codec #f))\n\
                    (b (add-buffer! d)))\n\
               (list (not (document-codec d)) (document-string d)))",
            raw_gz.display().to_string()
        ))
        .unwrap();
    assert_eq!(sf5a, "(#t \"plain-not-gzip\\n\")");

    // SF5b: Runtime::attach honors explicit File.journal for non-private codec files.
    let mut rt2 = Runtime::new().unwrap();
    rt2.attach(Client::FIRST, Some(&File { path: explicit_gz.clone(), journal: custom_journal.clone() }), "emacs").unwrap();
    assert!(custom_journal.exists(), "explicit journal path should be used for non-private codec attach");
}

#[test]
fn attach_rejects_non_utf8_canonical_target_without_opening_fffd_sibling() {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let bad_dir = root.join(OsStr::from_bytes(b"bad-\xff-dir"));
    let fffd_dir = root.join("bad-\u{FFFD}-dir");
    std::fs::create_dir(&bad_dir).unwrap();
    std::fs::create_dir(&fffd_dir).unwrap();

    let bad_plain = bad_dir.join("note.txt");
    let bad_gz = bad_dir.join("secret.txt.gz");
    let fffd_plain = fffd_dir.join("note.txt");
    let fffd_gz = fffd_dir.join("secret.txt.gz");
    std::fs::write(&bad_plain, "NON-UTF8-PLAIN\n").unwrap();
    std::fs::write(&bad_gz, gzip_bytes("NON-UTF8-GZ\n")).unwrap();
    std::fs::write(&fffd_plain, "SIBLING-FFFD-PLAIN\n").unwrap();
    std::fs::write(&fffd_gz, gzip_bytes("SIBLING-FFFD-GZ\n")).unwrap();

    let utf8_plain_link = root.join("utf8-plain-link.txt");
    let utf8_gz_link = root.join("utf8-gz-link.txt.gz");
    std::os::unix::fs::symlink(&bad_plain, &utf8_plain_link).unwrap();
    std::os::unix::fs::symlink(&bad_gz, &utf8_gz_link).unwrap();

    let mut rt = Runtime::new().unwrap();
    let err_gz =
        rt.attach(Client::FIRST, Some(&File { path: utf8_gz_link.clone(), journal: std::path::PathBuf::new() }), "emacs").unwrap_err();
    assert!(err_gz.contains("not valid UTF-8"), "expected UTF-8 rejection for codec attach, got: {err_gz}");

    let err_plain =
        rt.attach(Client::FIRST, Some(&File { path: utf8_plain_link.clone(), journal: std::path::PathBuf::new() }), "emacs").unwrap_err();
    assert!(err_plain.contains("not valid UTF-8"), "expected UTF-8 rejection for plain attach, got: {err_plain}");

    // Neither literal U+FFFD sibling file may have been opened or registered.
    let files_scm = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lisp/editor/files.scm");
    let registered = rt
        .eval(&format!(
            "(begin (require {:?}) (list (open-document-at {:?}) (open-document-at {:?}) (filter (lambda (f) f) (map (lambda (b) (document-path (buffer-document b))) (buffer-list)))))",
            files_scm.to_str().unwrap(),
            fffd_gz.to_str().unwrap(),
            fffd_plain.to_str().unwrap()
        ))
        .unwrap();
    assert_eq!(registered, "(#f #f ())");
}
