//! The native Scheme mail app: interaction and the frontend-neutral rich view.
use std::{
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};

use techne_editor::{
    present::{CursorShape, Input, LineNumbers},
    runtime::Runtime,
};

fn editor() -> (tempfile::TempDir, Runtime) {
    let dir = tempfile::Builder::new().permissions(std::fs::Permissions::from_mode(0o700)).tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, "My previous workspace\n").unwrap();
    let rt = Runtime::open(&path, &dir.path().join("notes.journal"), "emacs").unwrap().0;
    (dir, rt)
}
fn command(rt: &mut Runtime, name: &str) {
    rt.eval(&format!("(run-command (current-session) '{name} 1)")).unwrap();
    let errors = rt.take_errors();
    assert!(errors.is_empty(), "command {name}: {errors:?}");
}
fn key(rt: &mut Runtime, keys: &str) {
    // A sequence is its keys, as "C-c C-k"; " " alone is the space.
    let sequence: Vec<&str> = if keys.len() > 1 { keys.split(' ').collect() } else { vec![keys] };
    for key in sequence {
        rt.handle(Input::Key { key: key.into(), at: Instant::now() });
        let errors = rt.take_errors();
        assert!(errors.is_empty(), "key {key}: {errors:?}");
    }
}
fn pane(rt: &mut Runtime, index: usize) -> String {
    rt.snapshot().panes[index].text.to_string()
}
fn summary_subjects(rt: &mut Runtime) -> String {
    rt.eval("(map (lambda (m) (cdr (assq 'subject m))) (hash-table-ref (sget (current-session) 'mail) 'messages))").unwrap()
}
fn enter(rt: &mut Runtime, text: &str) {
    for c in text.chars() {
        key(rt, &c.to_string());
    }
    key(rt, "RET");
}
fn pump_until(rt: &mut Runtime, mut done: impl FnMut(&mut Runtime) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        rt.run_tasks(Duration::from_millis(20)).unwrap();
        if done(rt) {
            return;
        }
        assert!(Instant::now() < deadline, "mail task did not reach its expected state");
        std::thread::sleep(Duration::from_millis(5));
    }
}
/// The sample messages in a cache of their own, listed in the Inbox.
fn samples(rt: &mut Runtime, dir: &std::path::Path) {
    rt.eval(&format!(
        "(require {:?}) (mail-test-seed-samples! {:?})",
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../lisp/tests/fixtures/mail/mail-sample-fixture.scm"),
        dir.join("samples.sqlite").display().to_string()
    ))
    .unwrap();
    command(rt, "mail");
    key(rt, "i");
}
fn real_accounts(rt: &mut Runtime, dir: &std::path::Path) {
    let database = dir.join("mail.sqlite").display().to_string();
    rt.eval(&format!(
        "(import (techne mail store)) (set-option! 'mail-database {database:?})
      (set-option! 'mail-accounts '(((id . \"test\") (kind . maildir) (path . \"/missing-techne-mail-fixture\")
        (from . \"ada@example.org\") (smtp-host . \"smtp.invalid\") (smtp-user . \"ada@example.org\"))))"
    ))
    .unwrap();
}

#[test]
fn root_inbox_filters_use_the_displayed_name_and_delete_requires_trash() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    let root = dir.path().join("Maildir");
    std::fs::create_dir_all(root.join("cur")).unwrap();
    std::fs::create_dir_all(root.join("Provider/All Mail/cur")).unwrap();
    let message = "Subject: Root message\r\nMessage-ID: <root@example.org>\r\n\r\nbody";
    let inbox_file = root.join("cur/one:2,S");
    let all_file = root.join("Provider/All Mail/cur/one:2,S");
    std::fs::write(&inbox_file, message).unwrap();
    std::fs::write(&all_file, message).unwrap();
    let configure = |rt: &mut Runtime, filter: &str| {
        rt.eval(&format!(
            "(set-option! 'mail-accounts '(((id . \"test\") (kind . maildir) (path . {:?}) {filter})))",
            root.display().to_string()
        ))
        .unwrap();
    };
    configure(&mut rt, "(folders . (\"INBOX\" \"Provider/All Mail\"))");
    command(&mut rt, "mail");
    command(&mut rt, "mail-sync");
    pump_until(&mut rt, |rt| pane(rt, 0).contains("Up to date"));
    key(&mut rt, "i");
    assert_eq!(summary_subjects(&mut rt), "(\"Root message\")");
    key(&mut rt, "d");
    let result = rt.eval("(run-command (current-session) 'mail-execute 1)");
    let echo = rt.snapshot().echo;
    let outcome = format!("{result:?} {:?} {echo}", rt.take_errors());
    assert!(outcome.contains("Set trash"), "{outcome}");
    assert!(inbox_file.is_file() && all_file.is_file(), "neither copy can be marked for expunge");
    configure(&mut rt, "(skip . (\"INBOX\"))");
    command(&mut rt, "mail-sync");
    pump_until(&mut rt, |rt| summary_subjects(rt) == "()");
    assert!(inbox_file.is_file(), "excluding a folder only affects the cache");
}

#[test]
fn a_failed_bulk_archive_keeps_undo_for_completed_moves() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    let root = dir.path().join("Maildir");
    let inbox = root.join("cur");
    let archive = root.join("Archive/cur");
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::create_dir_all(&archive).unwrap();
    for (name, hour) in [("first", "13"), ("second", "12")] {
        std::fs::write(inbox.join(format!("{name}:2,S")), format!("Subject: {name}\nDate: Thu, 5 Nov 2026 {hour}:00:00 +0000\n\nbody"))
            .unwrap();
    }
    rt.eval(&format!(
        "(set-option! 'mail-accounts '(((id . \"test\") (kind . maildir) (path . {:?}) (archive . \"Archive\"))))",
        root.display().to_string(),
    ))
    .unwrap();
    command(&mut rt, "mail");
    command(&mut rt, "mail-sync");
    pump_until(&mut rt, |rt| pane(rt, 0).contains("Up to date"));
    key(&mut rt, "i");
    assert_eq!(summary_subjects(&mut rt), "(\"first\" \"second\")");
    key(&mut rt, "m m");
    std::fs::write(archive.join("second:2,S"), "existing destination").unwrap();
    rt.eval("(run-command (current-session) 'mail-archive 1)").unwrap();
    assert!(archive.join("first:2,S").is_file());
    assert!(rt.snapshot().echo.contains("already exists"));
    rt.take_errors();
    command(&mut rt, "mail-undo");
    assert!(inbox.join("first:2,S").is_file());
    assert!(!archive.join("first:2,S").exists());
    assert_eq!(std::fs::read_to_string(archive.join("second:2,S")).unwrap(), "existing destination");
}

#[test]
fn attached_sessions_keep_distinct_listings_and_marks() {
    let (dir, mut rt) = editor();
    samples(&mut rt, dir.path());
    key(&mut rt, "d");
    assert_eq!(
        rt.eval(
            r#"(let* ((one (current-session)) (one-mail (sget one 'mail))
                       (a (hash-table-ref one-mail 'summary)) (two (start-session #f "emacs")))
                  (let ((independent-start? (not (eq? a (current-buffer two)))))
                    (run-command two 'mail-all 1)
                    (let* ((two-mail (sget two 'mail)) (b (hash-table-ref two-mail 'summary)))
                      (list independent-start? (not (eq? a b)) (eq? (listing-get a 'mail-state) one-mail)
                            (eq? (listing-get b 'mail-state) two-mail)
                            (length (listing-marked a #\D)) (length (listing-marked b #\D))))))"#,
        )
        .unwrap(),
        "(#t #t #t #t 1 0)"
    );
}

#[test]
fn a_named_maildir_accepts_symlinked_cur_but_does_not_prune_an_unavailable_source() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    let root = dir.path().join("Maildir");
    let actual_cur = dir.path().join("actual-cur");
    std::fs::create_dir(&actual_cur).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::write(actual_cur.join("one:2,S"), "Subject: Symlinked Inbox\n\nbody").unwrap();
    std::os::unix::fs::symlink(&actual_cur, root.join("cur")).unwrap();
    rt.eval(&format!(
        "(set-option! 'mail-accounts '(((id . \"test\") (kind . maildir) (mailbox . \"INBOX\") (path . {:?}))))",
        root.display().to_string()
    ))
    .unwrap();
    command(&mut rt, "mail");
    command(&mut rt, "mail-sync");
    pump_until(&mut rt, |rt| pane(rt, 0).contains("Up to date"));
    std::fs::remove_dir_all(actual_cur).unwrap();
    command(&mut rt, "mail-sync");
    pump_until(&mut rt, |rt| pane(rt, 0).contains("Sync incomplete"));
    assert_eq!(
        rt.eval(
            "(let* ((db (mail-store-open (option #f 'mail-database)))
                        (count (length (mail-store-query db \"\")))) (mail-store-close db) count)"
        )
        .unwrap(),
        "1"
    );
}

#[test]
fn a_mark_cannot_delete_the_message_that_replaces_a_cached_row() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    let root = dir.path().join("Maildir");
    let inbox = root.join("cur");
    let trash = root.join("Trash/cur");
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::create_dir_all(&trash).unwrap();
    std::fs::write(inbox.join("old:2,S"), "Subject: Old message\n\nold").unwrap();
    rt.eval(&format!("(set-option! 'mail-accounts '(((id . \"test\") (kind . maildir) (path . {:?}))))", root.display().to_string()))
        .unwrap();
    command(&mut rt, "mail");
    command(&mut rt, "mail-sync");
    pump_until(&mut rt, |rt| pane(rt, 0).contains("Up to date"));
    key(&mut rt, "i");
    assert_eq!(summary_subjects(&mut rt), "(\"Old message\")");
    key(&mut rt, "d");
    let replacement = inbox.join("replacement:2,S");
    std::fs::remove_file(inbox.join("old:2,S")).unwrap();
    std::fs::write(&replacement, "Subject: Replacement\n\nnew").unwrap();
    // Import through a second connection, without refreshing the first view.
    rt.eval(&format!(
        "(let ((db (mail-store-open (option #f 'mail-database))))
           (mail-store-forget! db \"test\")
           (mail-store-import-maildir! db \"test\" \"INBOX\" {:?})
           (mail-store-close db))",
        root.display().to_string()
    ))
    .unwrap();
    rt.eval("(run-command (current-session) 'mail-execute 1)").unwrap();
    assert!(rt.snapshot().echo.contains("No such message"));
    assert!(replacement.is_file());
    assert_eq!(std::fs::read_dir(&trash).unwrap().count(), 0);
}

#[test]
fn maildir_sync_skips_oversized_sources_and_loads_binary_mime_on_demand() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    let source = dir.path().join("Maildir");
    std::fs::create_dir_all(source.join("cur")).unwrap();
    std::fs::create_dir_all(source.join("new")).unwrap();
    let mut wire = b"From: Ada <ada@example.org>\nSubject: Lazy binary fixture\nContent-Type: application/octet-stream; name=x.bin\nContent-Transfer-Encoding: binary\n\n".to_vec();
    wire.push(255);
    std::fs::write(source.join("new/1000"), &wire).unwrap();
    std::fs::write(source.join("new/999999-big"), vec![65; 16_777_217]).unwrap();
    let path = source.display().to_string();
    rt.eval(&format!(
        "(set-option! 'mail-accounts '(((id . \"test\") (kind . maildir) (path . {path:?}))))
        (set-option! 'mail-credential-provider (lambda (a p) (error \"MUST-NOT-LOOK-UP-SECRETS\")))"
    ))
    .unwrap();
    rt.handle(Input::Viewport { columns: 160, rows: 50 });
    command(&mut rt, "mail");
    command(&mut rt, "mail-sync");
    // Too large is left out on purpose, and said so: not a failure.
    pump_until(&mut rt, |rt| pane(rt, 0).contains("Up to date"));
    assert!(pane(&mut rt, 0).contains("1 over 16 MiB not cached"), "{}", pane(&mut rt, 0));
    assert!(pane(&mut rt, 0).lines().any(|l| l.contains("test") && l.contains("1 too large")), "{}", pane(&mut rt, 0));
    key(&mut rt, "a");
    assert!(pane(&mut rt, 0).contains("Lazy binary fixture"));
    key(&mut rt, "RET");
    assert!(pane(&mut rt, 1).contains("Attachments · 1"));
    assert_eq!(
        rt.eval(
            "(let* ((db (mail-store-open (option #f 'mail-database)))
        (n (caar (sqlite-query db \"SELECT sum(length(raw)) FROM mail_messages\" '()))))
        (mail-store-close db) n)"
        )
        .unwrap(),
        "0"
    );
    // Read, it is marked so at the source, as mu4e marks it: the file is
    // renamed into cur with S; its content is the same.
    assert!(!source.join("new/1000").exists());
    command(&mut rt, "mail-source");
    let focus = rt.snapshot().focus;
    assert!(pane(&mut rt, focus).contains('�'));
    assert_eq!(std::fs::read(source.join("cur/1000:2,S")).unwrap(), wire);
    command(&mut rt, "mail-sync-status");
    let focus = rt.snapshot().focus;
    let status = pane(&mut rt, focus);
    assert!(status.contains("Not cached: larger than 16 MiB") && status.contains("test / INBOX: 1 message"), "{status}");
    assert!(!status.contains("failed"), "{status}");
}

#[test]
fn cancelling_maildir_sync_does_not_turn_into_a_cached_partial_message() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    let source = dir.path().join("Maildir");
    std::fs::create_dir_all(source.join("cur")).unwrap();
    std::fs::create_dir_all(source.join("new")).unwrap();
    let wire =
        format!("Subject: Cancel me\nContent-Type: text/plain\nContent-Transfer-Encoding: quoted-printable\n\n{}", "x=41".repeat(50_000));
    std::fs::write(source.join("new/1000"), wire).unwrap();
    let path = source.display().to_string();
    rt.eval(&format!("(set-option! 'mail-accounts '(((id . \"test\") (kind . maildir) (path . {path:?}))))")).unwrap();
    command(&mut rt, "mail");
    key(&mut rt, "a");
    command(&mut rt, "mail-sync");
    rt.run_tasks(Duration::from_millis(5)).unwrap();
    command(&mut rt, "mail-cancel-sync");
    rt.run_tasks(Duration::from_millis(50)).unwrap();
    assert!(pane(&mut rt, 0).contains("Sync cancelled"));
    assert_eq!(
        rt.eval(
            "(let* ((db (mail-store-open (option #f 'mail-database))) (n (mail-ref (mail-store-counts db) 'total)))
        (mail-store-close db) n)"
        )
        .unwrap(),
        "0"
    );
    command(&mut rt, "mail-sync");
    pump_until(&mut rt, |rt| pane(rt, 0).contains("Up to date"));
    assert_eq!(
        rt.eval(
            "(let* ((db (mail-store-open (option #f 'mail-database)))
        (n (caar (sqlite-query db \"SELECT length(body) FROM mail_messages\" '()))))
        (mail-store-close db) n)"
        )
        .unwrap(),
        "100000"
    );
}

#[test]
fn autosave_drains_an_edit_arriving_during_a_delayed_sqlite_write() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    command(&mut rt, "mail");
    command(&mut rt, "mail-compose");
    rt.eval("(define draft-state (buffer-state (current-buffer (current-session))))
      (define draft-db (hash-table-ref draft-state 'db))
      (define blocker (spawn (lambda ()
        (sqlite-query draft-db \"WITH RECURSIVE t(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM t WHERE x<100000000) SELECT sum(x) FROM t\" '()))))").unwrap();
    rt.run_tasks(Duration::from_millis(20)).unwrap();
    rt.eval(
        "(move! (current-session) (lambda (p) (document-length (doc (current-session)))))
      (insert-text! (current-session) \"first edit\" 'new)",
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        rt.run_tasks(Duration::from_millis(20)).unwrap();
        if rt.eval("(hash-table-ref/default draft-state 'saving-revision #f)").unwrap() != "#f" {
            break;
        }
        assert!(Instant::now() < deadline, "autosave did not start while the connection was occupied");
        std::thread::sleep(Duration::from_millis(5));
    }
    rt.eval("(insert-text! (current-session) \" second edit\" 'new) (task-cancel blocker)").unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        rt.run_tasks(Duration::from_millis(20)).unwrap();
        if rt.eval("(equal? (hash-table-ref/default draft-state 'saved-revision #f) (document-revision (doc (current-session))))").unwrap()
            == "#t"
        {
            break;
        }
        assert!(Instant::now() < deadline, "newer edit was not drained after the pending save");
        std::thread::sleep(Duration::from_millis(5));
    }
    let stored = rt.eval("(caddr (car (mail-store-drafts draft-db)))").unwrap();
    assert!(stored.contains("first edit second edit"), "acknowledged revision must really be durable: {stored}");
    assert!(rt.take_errors().is_empty());
}

#[test]
fn reader_navigation_tracks_the_unfocused_summary_without_moving_focus() {
    for position in ["bottom", "side"] {
        let (dir, mut rt) = editor();
        rt.handle(Input::Viewport { columns: 160, rows: 50 });
        rt.eval(&format!(
            "(require {:?}) (mail-test-seed-navigation! {:?} 100)
             (set-option! 'mail-reader-position '{position})",
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../lisp/tests/fixtures/mail/mail-navigation-fixture.scm"),
            dir.path().join("navigation.sqlite").display().to_string()
        ))
        .unwrap();
        command(&mut rt, "mail");
        key(&mut rt, "a");
        let text = pane(&mut rt, 0);
        assert_eq!(text.lines().position(|l| l.contains("Nav item 000")), Some(2), "only two header rows: {text}");
        assert!(!text.lines().any(|l| l == "MAIL"));
        key(&mut rt, "RET");
        for _ in 0..40 {
            key(&mut rt, "n");
        }
        for name in ["mail-next", "mail-previous"] {
            command(&mut rt, name);
            assert_eq!(
                rt.eval(
                    "(let* ((s (current-session)) (st (sget s 'mail))
                        (v (car (session-panes s))) (d (view-document v))
                        (p (cadr (list-ref (view-ranges v) (view-primary v))))
                        (t (target-at d p)))
                       (and t (= (target-value t) (hash-table-ref st 'reader-id))
                         (= (target-value t) (hash-table-ref st 'selected)) (= (point s) 0)))"
                )
                .unwrap(),
                "#t"
            );
            let snap = rt.snapshot();
            assert_eq!(snap.focus, 1);
            assert!(matches!(snap.panes[0].request, Some(techne_editor::present::ViewRequest::Recenter(_))));
            assert!(snap.panes[1].request.is_none(), "recenter must target the summary, not the reader");
        }
        assert!(pane(&mut rt, 1).contains("Nav item 040"));
        key(&mut rt, "b");
        assert_eq!(rt.snapshot().focus, 0);
    }
}

#[test]
fn compact_reader_details_disclose_identity_and_recipients_without_source() {
    let (dir, mut rt) = editor();
    rt.handle(Input::Viewport { columns: 80, rows: 30 });
    samples(&mut rt, dir.path());
    key(&mut rt, "RET");
    assert!(pane(&mut rt, 0).contains("▸ to Arthur <arthur@example.org>"));
    assert!(pane(&mut rt, 0).lines().position(|l| l.contains("Hi Arthur,")).unwrap() < 8, "the message starts near the top");
    key(&mut rt, "H");
    let expanded = pane(&mut rt, 0);
    assert!(expanded.contains("Account  Personal"), "{expanded}");
    assert!(expanded.contains("Mailbox  INBOX"));
    assert!(expanded.contains("To  Arthur <arthur@example.org>"));
    assert!(expanded.contains("Cc  —"));
    assert!(expanded.contains("Reply-To  From address (no override)"));
    key(&mut rt, "H");
    assert!(!pane(&mut rt, 0).contains("Account  Personal"));
}

#[test]
fn marking_and_inherited_motions_keep_the_hidden_caret_selection_consistent() {
    let (dir, mut rt) = editor();
    samples(&mut rt, dir.path());
    let at_selection = "(let* ((s (current-session)) (t (target-at (doc s) (point s))))
      (and t (eq? (target-kind t) 'mail-message)
        (equal? (target-value t) (hash-table-ref (sget s 'mail) 'selected))))";
    let first = rt.eval("(hash-table-ref (sget (current-session) 'mail) 'selected)").unwrap();
    command(&mut rt, "listing-mark-row");
    assert_eq!(rt.eval(at_selection).unwrap(), "#t");
    assert_ne!(rt.eval("(hash-table-ref (sget (current-session) 'mail) 'selected)").unwrap(), first);
    key(&mut rt, "u");
    assert_eq!(rt.eval(at_selection).unwrap(), "#t");
    key(&mut rt, "DEL");
    assert_eq!(rt.eval(at_selection).unwrap(), "#t");
    key(&mut rt, "RET");
    assert!(pane(&mut rt, 1).contains("Sketches for the reading room"));
    key(&mut rt, "b");
    key(&mut rt, "M->");
    assert_eq!(rt.eval(at_selection).unwrap(), "#t");
    key(&mut rt, "m"); // The last row must not advance into the invisible footer.
    assert_eq!(rt.eval(at_selection).unwrap(), "#t");
    key(&mut rt, "M-<");
    assert_eq!(rt.eval(at_selection).unwrap(), "#t");
    assert_eq!(rt.eval("(hash-table-ref (sget (current-session) 'mail) 'selected)").unwrap(), first);
}

#[test]
fn binary_mime_source_is_replacement_decoded_and_original_octets_stay_cached() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    rt.eval("(define source-wire (bytevector-append
      (string->utf8 \"Subject: Binary source fixture\\r\\nContent-Type: application/octet-stream; name=x.bin\\r\\nContent-Transfer-Encoding: binary\\r\\n\\r\\n\") #u8(255 27 91 51 49 109 0 10)))
      (define source-db (mail-store-open (option #f 'mail-database)))
      (mail-store-import! source-db \"test\" \"INBOX\" \"binary-source\" source-wire '())").unwrap();
    command(&mut rt, "mail");
    key(&mut rt, "a");
    key(&mut rt, "RET");
    command(&mut rt, "mail-source");
    let focus = rt.snapshot().focus;
    let source = pane(&mut rt, focus);
    assert!(source.contains("Subject: Binary source fixture"));
    assert!(source.contains('�'));
    assert!(!source.contains('\u{1b}') && !source.contains('\0'), "wire controls must be inert in Source");
    assert_eq!(
        rt.eval(
            "(let ((m (car (mail-store-query source-db \"\" #:group 'all))))
      (equal? source-wire (mail-ref (mail-store-message source-db (mail-ref m 'id)) 'raw)))"
        )
        .unwrap(),
        "#t"
    );
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'read-only)").unwrap(), "#t");
    rt.eval("(mail-store-close source-db)").unwrap();
}

#[test]
fn a_message_read_stays_on_its_row_until_the_list_is_made_again() {
    let (dir, mut rt) = editor();
    samples(&mut rt, dir.path());
    command(&mut rt, "mail-groups");
    enter(&mut rt, "Unread");
    assert!(pane(&mut rt, 0).contains("Unread · 4 messages"));
    key(&mut rt, "RET");
    assert!(pane(&mut rt, 1).contains("A place for the longer conversations"));
    assert!(pane(&mut rt, 0).contains("A place for the longer conversations"), "read, and still listed");
    assert_eq!(rt.eval("(cdr (assq 'unread (car (hash-table-ref (sget (current-session) 'mail) 'messages))))").unwrap(), "0");
    key(&mut rt, "n");
    assert!(pane(&mut rt, 1).contains("Sketches for the reading room"));
    key(&mut rt, "p");
    assert!(pane(&mut rt, 1).contains("A place for the longer conversations"));
    assert!(pane(&mut rt, 0).contains("Unread · 4 messages"), "both read, neither gone");
    // The last actual read change was Lea; read commands participate in undo.
    key(&mut rt, "a");
    assert!(summary_subjects(&mut rt).contains("Sketches for the reading room"));
    // Choosing the group again makes the list again: what was read goes.
    command(&mut rt, "mail-groups");
    enter(&mut rt, "Unread");
    assert!(pane(&mut rt, 0).contains("Unread · 3 messages"));
    assert!(!summary_subjects(&mut rt).contains("A place for the longer conversations"));
    assert!(summary_subjects(&mut rt).contains("Sketches for the reading room"), "the undone read is unread again");
}

#[test]
fn catching_up_reads_all_of_a_group_and_one_undo_takes_it_back() {
    let (dir, mut rt) = editor();
    samples(&mut rt, dir.path());
    assert!(pane(&mut rt, 0).contains("4 unread"));
    let inbox = |rt: &mut Runtime| pane(rt, 0).lines().find(|l| l.contains("Inbox") && l.contains(" i ")).unwrap().to_string();
    // As Gnus's c: all read, and back to the groups.
    key(&mut rt, "c");
    assert!(pane(&mut rt, 0).contains("Accounts"), "back on the dashboard");
    assert!(!inbox(&mut rt).contains('4'), "{}", inbox(&mut rt));
    assert!(rt.snapshot().echo.contains("4 messages marked read in Inbox"), "{}", rt.snapshot().echo);
    key(&mut rt, "C-/");
    assert!(inbox(&mut rt).contains('4'), "one undo for all: {}", inbox(&mut rt));
    // Within a search, only what it finds.
    key(&mut rt, "i");
    command(&mut rt, "mail-search");
    enter(&mut rt, "from:nora");
    key(&mut rt, "c");
    key(&mut rt, "i");
    assert!(pane(&mut rt, 0).contains("3 unread"));
}

#[test]
fn q_in_the_reader_closes_it_and_ret_opens_it_again() {
    let (dir, mut rt) = editor();
    rt.handle(Input::Viewport { columns: 160, rows: 50 });
    samples(&mut rt, dir.path());
    key(&mut rt, "RET");
    assert_eq!(rt.snapshot().panes.len(), 2);
    assert_eq!(rt.snapshot().focus, 1);
    key(&mut rt, "q");
    let s = rt.snapshot();
    assert_eq!(s.panes.len(), 1, "the reader's pane is closed");
    assert!(s.panes[0].text.to_string().contains("Inbox · 7 messages"), "the list has the frame");
    key(&mut rt, "n");
    key(&mut rt, "RET");
    assert_eq!(rt.snapshot().panes.len(), 2, "RET opens it again");
    assert!(pane(&mut rt, 1).contains("Sketches for the reading room"));
    assert_eq!(rt.snapshot().focus, 1);
    // A list given the frame with f stays too when a message opens.
    key(&mut rt, "b");
    key(&mut rt, "f");
    assert_eq!(rt.snapshot().panes.len(), 1);
    key(&mut rt, "RET");
    assert_eq!(rt.snapshot().panes.len(), 2, "the list stays open beside the message");
    assert!(pane(&mut rt, 0).contains("Inbox · 7 messages"));
    // In one pane, as a short frame has, q is back to the list.
    rt.handle(Input::Viewport { columns: 80, rows: 30 });
    key(&mut rt, "q");
    assert!(pane(&mut rt, 0).contains("Inbox · 7 messages"));
}

#[test]
#[ignore = "a click is not yet a command: operation-ended hooks do not see it"]
fn a_click_selects_its_row_at_once() {
    let (dir, mut rt) = editor();
    samples(&mut rt, dir.path());
    let selected_sender = |rt: &mut Runtime| {
        rt.eval(
            "(let ((st (sget (current-session) 'mail)))
                   (cdr (assq 'sender (find (lambda (m) (equal? (cdr (assq 'id m)) (hash-table-ref st 'selected)))
                                            (hash-table-ref st 'messages)))))",
        )
        .unwrap()
    };
    let click = |rt: &mut Runtime, pane: usize, needle: &str| {
        let s = rt.snapshot();
        let text = s.panes[pane].text.to_string();
        let at = text.find(needle).unwrap();
        rt.handle(Input::Click { view: s.panes[pane].view, revision: s.panes[pane].revision, pos: at, extend: false, at: Instant::now() });
        assert!(rt.take_errors().is_empty());
    };
    click(&mut rt, 0, "Ada Moreno");
    // No key after it: the highlight is on the row clicked already.
    assert!(selected_sender(&mut rt).contains("Ada Moreno"));
    let s = rt.snapshot();
    let text = s.panes[0].text.to_string();
    let ada = text.find("Ada Moreno").unwrap();
    assert!(s.panes[0].layers.iter().any(|h| h.from <= ada && ada < h.to && s.style(h.style).bg.is_some()), "the row is highlighted");
    // On the dashboard too.
    key(&mut rt, "q");
    click(&mut rt, 0, "Starred");
    assert_eq!(rt.eval("(hash-table-ref (sget (current-session) 'mail) 'dash-key)").unwrap(), "\"starred\"");
}

#[test]
fn rows_a_search_finds_or_a_conversation_are_marked_together_as_in_mu4e() {
    let (dir, mut rt) = editor();
    samples(&mut rt, dir.path());
    let row = |rt: &mut Runtime, who: &str| pane(rt, 0).lines().find(|l| l.contains(who)).unwrap().to_string();
    command(&mut rt, "mail-mark-pattern");
    enter(&mut rt, "from:elias");
    assert!(row(&mut rt, "Elias Hart").starts_with('*') && row(&mut rt, "Lea Wagner").starts_with(' '));
    key(&mut rt, "!");
    assert!(row(&mut rt, "Elias Hart").starts_with('!'));
    key(&mut rt, "x");
    assert!(!pane(&mut rt, 0).contains("4 unread"), "Elias read");
    // A conversation: Mika's message and Jonas's reply.
    while !row(&mut rt, "Mika Chen").contains("Re: Notes") || rt.eval("(hash-table-ref (sget (current-session) 'mail) 'selected)").unwrap()
        != rt.eval("(cdr (assq 'id (find (lambda (m) (string-prefix? \"Mika\" (cdr (assq 'sender m)))) (hash-table-ref (sget (current-session) 'mail) 'messages))))").unwrap()
    {
        key(&mut rt, "n");
    }
    key(&mut rt, "T");
    assert!(row(&mut rt, "Mika Chen").starts_with('*') && row(&mut rt, "Jonas Berg").starts_with('*'));
    assert!(row(&mut rt, "Ada Moreno").starts_with(' '));
}

#[test]
fn a_maildir_account_reads_the_folders_under_its_path() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    let root = dir.path().join("account");
    for (folder, subject, id, hour) in [
        ("Inbox", "Question", "<q@example.org>", 10),
        ("[Gmail]/Sent Mail", "Re: Question", "<a@example.org>", 11),
        ("[Gmail]/All Mail", "Question", "<q@example.org>", 10),
        ("[Gmail]/Spam", "Unwanted", "<s@example.org>", 12),
    ] {
        std::fs::create_dir_all(root.join(folder).join("cur")).unwrap();
        std::fs::write(
            root.join(folder).join("cur/one:2,S"),
            format!("From: ada@example.org\r\nSubject: {subject}\r\nMessage-ID: {id}\r\nIn-Reply-To: <q@example.org>\r\nDate: Thu, 5 Nov 2026 {hour}:00:00 +0000\r\n\r\nbody\r\n"),
        )
        .unwrap();
    }
    let path = root.display().to_string();
    rt.eval(&format!("(set-option! 'mail-accounts '(((id . \"test\") (kind . maildir) (path . {path:?}) (skip . (\"[Gmail]/Spam\")))))"))
        .unwrap();
    command(&mut rt, "mail");
    command(&mut rt, "mail-sync");
    pump_until(&mut rt, |rt| pane(rt, 0).contains("Up to date"));
    let dashboard = pane(&mut rt, 0);
    let rows: Vec<&str> = dashboard.lines().collect();
    let account = rows.iter().position(|l| l.contains("▾") && l.contains("test")).unwrap_or_else(|| panic!("{dashboard}"));
    assert!(rows[account + 1].contains("Inbox") && rows[account + 2].contains("[Gmail]/Sent Mail"), "{dashboard}");
    assert!(rows[account + 3].contains("[Gmail]/All Mail"), "Inbox first, All Mail last: {dashboard}");
    assert!(!dashboard.contains("Spam"), "{dashboard}");
    command(&mut rt, "mail-groups");
    enter(&mut rt, "All mail");
    assert_eq!(
        rt.eval(
            "(import (techne mail store)) (map (lambda (m) (list (cdr (assq 'mailbox m)) (cdr (assq 'subject m)) (cdr (assq 'tree m))))
                   (hash-table-ref (sget (current-session) 'mail) 'messages))"
        )
        .unwrap(),
        "((\"Inbox\" \"Question\" \"\") (\"[Gmail]/Sent Mail\" \"Re: Question\" \"╰─► \"))",
        "two folders, one conversation; the copy in All Mail is the same message, and the skipped folder is not read"
    );
    assert!(pane(&mut rt, 0).contains("All mail · 2 messages"));
    key(&mut rt, "RET");
    key(&mut rt, "H");
    assert!(pane(&mut rt, 1).contains("Mailbox  Inbox, [Gmail]/All Mail"), "the details name both folders");
    key(&mut rt, "b");
    command(&mut rt, "mail-search");
    enter(&mut rt, "in:All");
    assert_eq!(summary_subjects(&mut rt), "(\"Question\")", "and it is found by either");
    command(&mut rt, "mail-clear-search");
    command(&mut rt, "mail-search");
    enter(&mut rt, "in:Sent");
    assert_eq!(summary_subjects(&mut rt), "(\"Re: Question\")");
}

#[test]
fn marks_are_shown_on_rows_then_done_and_undone_together() {
    let (dir, mut rt) = editor();
    samples(&mut rt, dir.path());
    let row = |rt: &mut Runtime, who: &str| pane(rt, 0).lines().find(|l| l.contains(who)).unwrap().to_string();
    let state = "(map (lambda (m) (list (cdr (assq 'unread m)) (cdr (assq 'starred m))))
                   (hash-table-ref (sget (current-session) 'mail) 'messages))";
    let before = rt.eval(state).unwrap();
    key(&mut rt, "r"); // Elias: to archive. Each mark moves on to the next row.
    key(&mut rt, "-"); // Lea: to lose her star.
    key(&mut rt, "?"); // Ada: to be unread.
    assert!(row(&mut rt, "Elias Hart").starts_with('r'));
    assert!(row(&mut rt, "Lea Wagner").starts_with('-'));
    assert!(row(&mut rt, "Ada Moreno").starts_with('?'));
    assert_eq!(rt.eval(state).unwrap(), before, "nothing is done until x");
    key(&mut rt, "u"); // Mika: nothing to unmark, and on to Jonas.
    key(&mut rt, "x");
    assert!(!summary_subjects(&mut rt).contains("A place for the longer conversations"), "archived");
    assert!(rt.eval(state).unwrap().starts_with("((1 0) (1 0) "), "Lea unstarred, Ada unread");
    assert!(!row(&mut rt, "Lea Wagner").starts_with('-'), "the marks are gone");
    key(&mut rt, "a");
    assert_eq!(rt.eval(state).unwrap(), before, "one undo for all three");
    // Several rows marked, then one mark for them all; U takes them off.
    key(&mut rt, "M-<");
    key(&mut rt, "m");
    key(&mut rt, "m");
    key(&mut rt, "!");
    assert!(row(&mut rt, "Elias Hart").starts_with('!') && row(&mut rt, "Lea Wagner").starts_with('!'));
    key(&mut rt, "U");
    assert!(row(&mut rt, "Elias Hart").starts_with(' '));
    key(&mut rt, "x");
    assert_eq!(rt.eval(state).unwrap(), before, "nothing marked, nothing done");
    // In the reader a mark is on the open message, and the next is read.
    key(&mut rt, "M-<");
    key(&mut rt, "RET");
    key(&mut rt, "+");
    assert!(row(&mut rt, "Elias Hart").starts_with('+'));
    assert!(pane(&mut rt, 1).contains("Sketches for the reading room"));
    assert_eq!(rt.snapshot().focus, 1);
}

#[test]
fn narrow_viewport_has_one_pane_and_working_read_back_and_paging() {
    let (dir, mut rt) = editor();
    rt.handle(Input::Viewport { columns: 80, rows: 30 });
    samples(&mut rt, dir.path());
    assert_eq!(rt.snapshot().panes.len(), 1);
    key(&mut rt, "RET");
    assert_eq!(rt.snapshot().panes.len(), 1);
    assert!(pane(&mut rt, 0).contains("A place for the longer conversations"));
    key(&mut rt, "f");
    assert_eq!(rt.snapshot().focus, 0);
    key(&mut rt, "b");
    assert!(pane(&mut rt, 0).contains("Inbox · 7 messages"));
    command(&mut rt, "mail-quit");
    assert!(pane(&mut rt, 0).contains("My previous workspace"));
}

#[test]
fn opening_a_stale_draft_row_reads_the_newer_saved_text_without_writing() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    command(&mut rt, "mail");
    rt.eval(
        "(define inspect-db (mail-store-open (option #f 'mail-database)))
         (mail-store-draft! inspect-db \"draft-fixture\" \"test\" \"From: ada@example.org\\nSubject: Before\\n\\nOld body\")",
    )
    .unwrap();
    command(&mut rt, "mail-drafts");
    assert!(pane(&mut rt, 0).contains("Before"));
    rt.eval("(mail-store-draft! inspect-db \"draft-fixture\" \"test\" \"From: ada@example.org\\nSubject: After\\n\\nNew body\")").unwrap();
    key(&mut rt, "RET");
    assert!(rt.snapshot().pane().text.to_string().contains("New body"));
    rt.eval(
        "(mail-store-draft! inspect-db \"draft-fixture\" \"test\" \"Subject: Newer still\\n\\nNewer body\")
             (define stale-buffer (current-buffer (current-session)))",
    )
    .unwrap();
    command(&mut rt, "mail-postpone");
    let saved = |rt: &mut Runtime| rt.eval("(caddr (mail-store-draft inspect-db \"draft-fixture\"))").unwrap();
    assert!(saved(&mut rt).contains("Newer body"), "postponing must not rewrite an unchanged draft");
    rt.eval("(drop-buffer! (current-session) stale-buffer)").unwrap();
    assert!(saved(&mut rt).contains("Newer body"), "closing must not rewrite an unchanged draft");
}

#[test]
fn discarding_a_sent_buffer_keeps_the_delivery_record() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    command(&mut rt, "mail");
    command(&mut rt, "mail-compose");
    rt.eval(
        "(define inspect-db (mail-store-open (option #f 'mail-database)))
         (define saved-draft (car (mail-store-drafts inspect-db)))
         (define draft-id (car saved-draft))
         (call-with-mail-store-lease inspect-db
           (lambda ()
             (unless (mail-store-claim-draft! inspect-db draft-id (caddr saved-draft) \"token\")
               (error \"draft claim failed\"))
             (mail-store-finish-draft! inspect-db draft-id \"token\" \"sent\")))",
    )
    .unwrap();
    command(&mut rt, "mail-discard-draft");
    assert!(rt.snapshot().echo.contains("delivery record retained"));
    assert_eq!(rt.eval("(mail-store-draft-state inspect-db draft-id)").unwrap(), "\"sent\"");
    assert!(rt.snapshot().minibuffer.is_none());
}

#[test]
fn ambiguous_draft_is_inspectable_and_ordinary_save_reopen_cannot_reset_it() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    command(&mut rt, "mail");
    command(&mut rt, "mail-compose");
    rt.eval("(define inspect-db (mail-store-open (option #f 'mail-database)))
      (define draft-id (caar (mail-store-drafts inspect-db)))
      (mail-store-draft! inspect-db draft-id \"test\" \"From: ada@example.org\\nTo: ben@example.org\\nSubject: ambiguous\\n\\nBody\" #:state \"uncertain\")").unwrap();
    command(&mut rt, "mail-save-draft");
    rt.eval("(drop-buffer! (current-session) (current-buffer (current-session)))").unwrap();
    command(&mut rt, "mail-drafts");
    assert!(pane(&mut rt, 0).contains("ambiguous") && pane(&mut rt, 0).contains("uncertain"), "{}", pane(&mut rt, 0));
    key(&mut rt, "RET");
    let focus = rt.snapshot().focus;
    assert!(pane(&mut rt, focus).contains("Subject: ambiguous"));
    command(&mut rt, "mail-save-draft");
    assert_eq!(rt.eval("(mail-store-draft-state inspect-db draft-id)").unwrap(), "\"uncertain\"");
    let before = pane(&mut rt, focus);
    rt.handle(Input::Key { key: "X".into(), at: Instant::now() });
    rt.take_errors();
    assert_eq!(pane(&mut rt, focus), before);
    command(&mut rt, "mail-resolve-delivery");
    enter(&mut rt, "Confirmed NOT delivered");
    assert_eq!(rt.eval("(mail-store-draft-state inspect-db draft-id)").unwrap(), "\"draft\"");
}

#[test]
fn credential_failure_before_submission_remains_retryable_and_secret_free() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    command(&mut rt, "mail");
    command(&mut rt, "mail-compose");
    rt.eval(
        "(view-edit! (session-view (current-session))
      (list (list 0 (document-length (doc (current-session)))
        \"From: ada@example.org\\nTo: ben@example.org\\nSubject: Test\\n\\nHello\")) \"new\")
      (set-option! 'mail-credential-provider (lambda (a p) (error \"SECRET-MUST-NOT-APPEAR\")))",
    )
    .unwrap();
    command(&mut rt, "mail-send");
    enter(&mut rt, "yes");
    pump_until(&mut rt, |rt| {
        rt.eval("(document-string (buffer-document (buffer-named \"*Messages*\")))").unwrap().contains("Not submitted")
    });
    assert_eq!(
        rt.eval(
            "(mail-store-draft-state (hash-table-ref (buffer-state (current-buffer (current-session))) 'db)
      (hash-table-ref (buffer-state (current-buffer (current-session))) 'draft-id))"
        )
        .unwrap(),
        "\"draft\""
    );
    assert!(rt.take_errors().is_empty());
    let messages = rt.eval("(document-string (buffer-document (buffer-named \"*Messages*\")))").unwrap();
    assert!(!messages.contains("SECRET-MUST-NOT-APPEAR"), "{messages}");
    assert!(messages.contains("Not submitted"), "{messages}");
}

#[test]
fn pasted_encoded_from_to_cc_bcc_preserve_mailbox_boundaries() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    command(&mut rt, "mail");
    command(&mut rt, "mail-compose");
    rt.eval("(view-edit! (session-view (current-session))
      (list (list 0 (document-length (doc (current-session)))
        \"From: =?UTF-8?Q?D=C3=B6e=2C_Ada?= <ada@example.org>\\nTo: =?UTF-8?Q?D=C3=B6e=2C_Jane?= <jane@example.org>\\nCc: =?UTF-8?Q?Rivers=2C_Ben?= <ben@example.org>\\nBcc: =?UTF-8?Q?Hidden=2C_Recipient?= <hidden@example.org>\\nSubject: Pasted headers\\n\\nBody\")) \"new\")").unwrap();
    command(&mut rt, "mail-send");
    let snapshot = rt.snapshot();
    let prompt = &snapshot.minibuffer.as_ref().expect("explicit confirmation").prompt;
    assert!(prompt.contains("Send to jane@example.org, ben@example.org, hidden@example.org?"), "{prompt}");
    key(&mut rt, "C-g"); // Never submit to the configured invalid host.
}

#[test]
fn real_mail_opens_unread_not_old_read_messages() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    let path = dir.path().join("mail.sqlite").display().to_string();
    rt.eval(&format!("(let ((db (mail-store-open {path:?})))
      (mail-store-import! db \"test\" \"INBOX\" \"old\" \"Date: Sat, 13 Feb 2021 12:00:00 +0000\\nSubject: Old read fixture\\n\\nbody\" '(\"\\\\Seen\"))
      (mail-store-import! db \"test\" \"INBOX\" \"new\" \"Date: Thu, 5 Nov 2026 12:00:00 +0000\\nSubject: New unread fixture\\n\\nbody\" '())
      (mail-store-import! db \"test\" \"INBOX\" \"newer\" \"Date: Fri, 6 Nov 2026 12:00:00 +0000\\nSubject: Newer read fixture\\n\\nbody\" '(\"\\\\Seen\"))
      (mail-store-close db))")).unwrap();
    command(&mut rt, "mail");
    key(&mut rt, "u");
    assert!(pane(&mut rt, 0).contains("Unread · 1 message"));
    assert_eq!(summary_subjects(&mut rt), "(\"New unread fixture\")");
    key(&mut rt, "RET");
    assert!(pane(&mut rt, 1).contains("New unread fixture"));
    assert!(pane(&mut rt, 0).contains("New unread fixture"), "read, it stays listed in this session");
    let mut reopened = Runtime::open(&dir.path().join("notes.txt"), &dir.path().join("reopened.journal"), "emacs").unwrap().0;
    real_accounts(&mut reopened, dir.path());
    command(&mut reopened, "mail");
    key(&mut reopened, "u");
    assert!(pane(&mut reopened, 0).contains("You're caught up."));
    assert!(pane(&mut reopened, 0).contains("Browse all mail"));
    assert!(!pane(&mut reopened, 0).contains("Connect an IMAP"));
}

#[test]
fn new_composition_starts_in_the_recipient_field() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    command(&mut rt, "mail");
    command(&mut rt, "mail-compose");
    assert_eq!(
        rt.eval("(let* ((s (current-session)) (d (doc s)) (p (point s))) (document-substring d (line-start d p) p))").unwrap(),
        "\"To: \""
    );
    for c in "ben@example.org".chars() {
        key(&mut rt, &c.to_string());
    }
    assert!(pane(&mut rt, 0).contains("To: ben@example.org\n"));
}

#[test]
fn ordinary_quoted_and_encoded_reply_names_reach_envelope_confirmation() {
    for (reply_header, expected_to, envelope) in [
        ("", "\"Doe, Jane\" <jane@example.org>", "jane@example.org"),
        ("Reply-To: =?UTF-8?Q?D=C3=B6e=2C_Jane?= <reply@example.org>\r\n", "\"Döe, Jane\" <reply@example.org>", "reply@example.org"),
    ] {
        let (dir, mut rt) = editor();
        real_accounts(&mut rt, dir.path());
        let source = dir.path().join("Maildir");
        std::fs::create_dir_all(source.join("new")).unwrap();
        std::fs::write(source.join("new/quoted"), format!("From: \"Doe, Jane\" <jane@example.org>\r\n{reply_header}To: ada@example.org\r\nSubject: Quoted reply fixture\r\nMessage-ID: <quoted@example.org>\r\n\r\nHello Ada\r\n")).unwrap();
        let path = source.display().to_string();
        rt.eval(&format!(
            "(set-option! 'mail-accounts '(((id . \"test\") (kind . maildir) (path . {path:?})
          (from . \"ada@example.org\") (smtp-host . \"must-not-connect.invalid\"))))"
        ))
        .unwrap();
        command(&mut rt, "mail");
        command(&mut rt, "mail-sync");
        pump_until(&mut rt, |rt| pane(rt, 0).contains("Up to date"));
        key(&mut rt, "i");
        key(&mut rt, "RET");
        command(&mut rt, "mail-reply");
        let draft = pane(&mut rt, 1);
        assert!(draft.contains(&format!("To: {expected_to}")), "{draft}");
        for c in "Thanks, Jane.".chars() {
            key(&mut rt, &c.to_string());
        }
        let edited = pane(&mut rt, 1);
        assert!(edited.starts_with("From: ada@example.org\n"), "typing a reply must not corrupt headers: {edited}");
        assert!(edited.contains("\n\nThanks, Jane."), "reply caret begins in the body: {edited}");
        command(&mut rt, "mail-send");
        let snapshot = rt.snapshot();
        let prompt = &snapshot.minibuffer.as_ref().expect("send confirmation, not network submission").prompt;
        assert!(prompt.contains(&format!("Send to {envelope}?")), "{prompt}");
        key(&mut rt, "C-g"); // No credential lookup, connection or sending.
    }
}

#[test]
fn mail_accounts_can_be_configured_with_eval_buffer() {
    let (dir, mut rt) = editor();
    let config = dir.path().join("mail-config.scm");
    std::fs::write(
        &config,
        "(import (techne editor)) (set-option! 'mail-accounts '(((id . \"loaded-fixture\") (kind . maildir) (path . \"/not-used\"))))",
    )
    .unwrap();
    command(&mut rt, "find-file");
    key(&mut rt, "C-a");
    key(&mut rt, "C-k");
    enter(&mut rt, &config.display().to_string());
    command(&mut rt, "eval-buffer");
    pump_until(&mut rt, |rt| rt.eval("(option #f 'mail-accounts)").unwrap().contains("loaded-fixture"));
}

#[test]
fn unconfigured_mail_stays_on_the_dashboard_and_uses_normal_mode_help() {
    let (_dir, mut rt) = editor();
    assert_eq!(rt.eval("(not (find-package 'mail-runtime))").unwrap(), "#t");
    rt.eval("(set-option! 'mail-database \":memory:\")").unwrap();
    command(&mut rt, "mail");
    assert_eq!(rt.eval("(not (find-package 'mail-runtime))").unwrap(), "#f");
    assert!(pane(&mut rt, 0).contains("No accounts yet"));
    key(&mut rt, "g");
    assert!(rt.snapshot().echo.contains("Set mail-accounts"));
    assert!(pane(&mut rt, 0).contains("No accounts yet"));
    key(&mut rt, "C-h m");
    let snapshot = rt.snapshot();
    let help = snapshot.pane().text.to_string();
    assert!(help.contains("mail-dashboard-mode"), "{}\n{help}", snapshot.echo);
    assert!(help.contains("mail-sync"), "{help}");
}

#[test]
fn sync_reports_partial_success_safely_and_preserves_maildir_octets() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    let maildir = dir.path().join("Maildir");
    std::fs::create_dir_all(maildir.join("cur")).unwrap();
    std::fs::create_dir_all(maildir.join("new")).unwrap();
    let mut raw = b"From: Ada <ada@example.org>\r\nTo: Ben <ben@example.org>\r\nSubject: Real local fixture\r\nContent-Type: text/plain; charset=iso-8859-1\r\n\r\nPlain *literal* caf".to_vec();
    raw.extend_from_slice(b"\xe9 \x1b]52;c;not-a-clipboard-command\x07\r\n");
    std::fs::write(maildir.join("new/message"), &raw).unwrap();
    let path = maildir.display().to_string();
    rt.eval(&format!(
        "(set-option! 'mail-accounts
       '(((id . \"local\") (kind . maildir) (path . {path:?}))
         ((id . \"needs-secret\") (kind . imap) (host . \"must-not-connect.invalid\") (user . \"test\"))))
       (set-option! 'mail-credential-provider (lambda (a p) (error \"PRIVATE-PROVIDER-ERROR\")))"
    ))
    .unwrap();
    command(&mut rt, "mail");
    command(&mut rt, "mail-sync");
    pump_until(&mut rt, |rt| pane(rt, 0).contains("Sync incomplete"));
    let dashboard = pane(&mut rt, 0);
    assert!(dashboard.lines().any(|l| l.contains("local") && l.contains("synced")), "{dashboard}");
    assert!(dashboard.lines().any(|l| l.contains("needs-secret") && l.contains("incomplete")), "{dashboard}");
    key(&mut rt, "i");
    assert!(pane(&mut rt, 0).contains("Real local fixture"));
    assert!(pane(&mut rt, 0).contains("Sync incomplete"));
    key(&mut rt, "RET");
    let body = pane(&mut rt, 1);
    assert!(body.contains("Plain *literal* café"), "{body}");
    assert!(!body.contains('\x1b'));
    command(&mut rt, "mail-sync-status");
    let status = pane(&mut rt, 1);
    assert!(status.contains("needs-secret"));
    assert!(status.contains("Credentials"));
    assert!(status.contains("secret manager"));
    assert!(!status.contains("PRIVATE-PROVIDER-ERROR"));
    assert_eq!(std::fs::read(maildir.join("cur/message:2,S")).unwrap(), raw, "read: renamed with S, its content untouched");
}

#[test]
fn the_reader_has_real_rich_structure() {
    let (dir, mut rt) = editor();
    samples(&mut rt, dir.path());
    key(&mut rt, "RET");
    let s = rt.snapshot();
    assert_eq!(s.panes.len(), 2);
    let summaries = s.panes[0].text.to_string();
    let reader = s.panes[1].text.to_string();
    assert!(summaries.contains("Inbox · 7 messages"), "{summaries}");
    assert!(summaries.contains("3 unread"), "{summaries}");
    assert!(reader.contains("A place for the longer conversations"), "{reader}");
    assert!(reader.contains("Make room for the message"), "{reader}");
    assert!(reader.contains("• Search when you know"), "{reader}");
    assert!(reader.contains("┃ Tools should feel"), "{reader}");
    assert!(!reader.contains("<html>"));
    assert!(reader.contains("remote images blocked"));
    assert!(!s.panes[1].layers.is_empty(), "rich faces are present");
    assert_eq!(s.panes[1].display.line_numbers, LineNumbers::Off, "reader has no code gutter");
}

#[test]
fn summary_selection_is_row_based_without_rendering_or_marking_read() {
    let (dir, mut rt) = editor();
    samples(&mut rt, dir.path());
    assert_eq!(rt.snapshot().panes[0].cursor, CursorShape::Hidden);
    let summary = pane(&mut rt, 0);
    let elias = summary.lines().find(|line| line.contains("Elias Hart")).unwrap();
    assert!(elias.contains("A place"), "sender and subject share one row: {elias}");
    // The list opens alone; moving through it opens no message.
    assert_eq!(rt.snapshot().panes.len(), 1);
    for movement in ["<down>", "<up>", "C-n", "C-p", "n"] {
        key(&mut rt, movement);
        assert_eq!(rt.snapshot().panes.len(), 1, "selection must not open or decode a message");
    }
    assert!(pane(&mut rt, 0).contains("4 unread"));
    key(&mut rt, "RET");
    assert!(pane(&mut rt, 0).contains("3 unread"));
    assert_eq!(rt.snapshot().focus, 1);
    assert!(pane(&mut rt, 1).contains("reading-notes.txt"));
    command(&mut rt, "mail-back");
    assert_eq!(rt.snapshot().focus, 0);
}

#[test]
fn local_archive_is_undoable_and_does_not_destroy_the_cached_message() {
    let (dir, mut rt) = editor();
    samples(&mut rt, dir.path());
    key(&mut rt, "A");
    assert!(!summary_subjects(&mut rt).contains("A place for the longer conversations"));
    key(&mut rt, "a");
    assert!(summary_subjects(&mut rt).contains("A place for the longer conversations"));
}

#[test]
fn search_empty_state_and_reset_work_with_keys() {
    let (dir, mut rt) = editor();
    samples(&mut rt, dir.path());
    command(&mut rt, "mail-search");
    for c in "zzzznomatch".chars() {
        key(&mut rt, &c.to_string());
    }
    key(&mut rt, "RET");
    assert!(pane(&mut rt, 0).contains("No messages match"));
    command(&mut rt, "mail-clear-search");
    assert!(summary_subjects(&mut rt).contains("A place for the longer conversations"));
}

#[test]
fn bottom_reader_is_default_and_live_position_changes_restore_title_space() {
    let (dir, mut rt) = editor();
    rt.handle(Input::Viewport { columns: 160, rows: 50 });
    samples(&mut rt, dir.path());
    key(&mut rt, "RET");
    key(&mut rt, "b");
    assert_eq!(rt.eval("(option #f 'mail-reader-position)").unwrap(), "bottom");
    assert!(rt.eval("(session-tree (current-session))").unwrap().contains("split below"));
    assert!(pane(&mut rt, 0).contains("A place for the longer conversations"));
    let selected = rt.eval("(hash-table-ref (sget (current-session) 'mail) 'selected)").unwrap();
    let reader = pane(&mut rt, 1);
    command(&mut rt, "mail-toggle-reader-position");
    assert_eq!(rt.eval("(option #f 'mail-reader-position)").unwrap(), "side");
    assert!(rt.eval("(session-tree (current-session))").unwrap().contains("split right"));
    assert!(!pane(&mut rt, 0).contains("A place for the longer conversations"));
    assert_eq!(pane(&mut rt, 1), reader);
    assert_eq!(rt.eval("(hash-table-ref (sget (current-session) 'mail) 'selected)").unwrap(), selected);
    rt.eval("(invoke (current-session) 'set-option (dict 'name 'mail-reader-position 'value 'bottom 'global #t))").unwrap();
    assert!(rt.eval("(session-tree (current-session))").unwrap().contains("split below"));
    assert!(pane(&mut rt, 0).contains("A place for the longer conversations"));
    assert_eq!(rt.eval("(guard (e (#t 'rejected)) (set-option! 'mail-reader-position 'left))").unwrap(), "rejected");
}

#[test]
fn reader_position_changes_preserve_focus_point_scroll_and_maximized_view() {
    let (dir, mut rt) = editor();
    samples(&mut rt, dir.path());
    key(&mut rt, "RET");
    rt.eval(
        "(define kept-reader (session-view (current-session)))
        (view-set-ranges! kept-reader '((12 12)) 0) (view-set-scroll! kept-reader 12)",
    )
    .unwrap();
    let messages = summary_subjects(&mut rt);
    for _ in 0..2 {
        command(&mut rt, "mail-toggle-reader-position");
        assert_eq!(rt.snapshot().focus, 1);
        assert_eq!(rt.eval("(eq? kept-reader (session-view (current-session)))").unwrap(), "#t");
        assert_eq!(rt.eval("(point (current-session))").unwrap(), "12");
        assert_eq!(rt.eval("(view-scroll kept-reader)").unwrap(), "12");
        assert_eq!(summary_subjects(&mut rt), messages);
    }
    key(&mut rt, "f");
    assert_eq!(rt.snapshot().panes.len(), 1);
    command(&mut rt, "mail-toggle-reader-position");
    assert_eq!(rt.snapshot().panes.len(), 1);
    key(&mut rt, "f");
    assert_eq!(rt.snapshot().panes.len(), 2);
    assert_eq!(rt.snapshot().focus, 1);
    assert!(rt.eval("(session-tree (current-session))").unwrap().contains("split right"));
    assert_eq!(rt.eval("(eq? kept-reader (session-view (current-session)))").unwrap(), "#t");
}

#[test]
fn bottom_and_side_responsiveness_use_height_and_width_respectively() {
    let (dir, mut rt) = editor();
    rt.handle(Input::Viewport { columns: 80, rows: 50 });
    samples(&mut rt, dir.path());
    key(&mut rt, "RET");
    key(&mut rt, "b");
    assert_eq!(rt.snapshot().panes.len(), 2, "a tall narrow viewport can stack panes");
    rt.handle(Input::Viewport { columns: 80, rows: 30 });
    assert_eq!(rt.snapshot().panes.len(), 1);
    rt.handle(Input::Viewport { columns: 80, rows: 50 });
    assert_eq!(rt.snapshot().panes.len(), 2, "automatically collapsed panes restore when they fit");
    command(&mut rt, "mail-toggle-reader-position");
    assert_eq!(rt.snapshot().panes.len(), 1, "side layout must not cram two panes into 80 columns");
    rt.handle(Input::Viewport { columns: 160, rows: 50 });
    assert_eq!(rt.snapshot().panes.len(), 2);
    key(&mut rt, "f");
    rt.handle(Input::Viewport { columns: 120, rows: 50 });
    rt.handle(Input::Viewport { columns: 170, rows: 50 });
    assert_eq!(rt.snapshot().panes.len(), 1, "manual maximization survives resizing");
    key(&mut rt, "f");
    assert_eq!(rt.snapshot().panes.len(), 2);
}

#[test]
fn side_reader_can_be_configured_before_opening_mail() {
    let (dir, mut rt) = editor();
    rt.eval("(set-option! 'mail-reader-position 'side)").unwrap();
    samples(&mut rt, dir.path());
    key(&mut rt, "RET");
    assert!(rt.eval("(session-tree (current-session))").unwrap().contains("split right"));
}

#[test]
fn a_reply_is_a_row_under_its_parent_and_the_selection_follows_the_reader() {
    let (dir, mut rt) = editor();
    samples(&mut rt, dir.path());
    let list = pane(&mut rt, 0);
    let rows: Vec<&str> = list.lines().collect();
    let mika = rows.iter().position(|l| l.contains("Mika Chen")).unwrap();
    assert!(rows[mika].contains("Re: Notes from the"), "the root has the subject: {list}");
    assert!(rows[mika + 1].contains("Jonas Berg") && rows[mika + 1].contains("╰─►"), "the reply is under it: {list}");
    assert!(!rows[mika + 1].contains("Notes"), "without repeating the subject: {list}");
    assert!(rows[mika - 1].contains("Ada Moreno"), "the conversation sorts by its newest message: {list}");
    assert!(list.contains("7 messages"), "{list}");
    let selected = "(hash-table-ref (sget (current-session) 'mail) 'selected)";
    for _ in 0..3 {
        key(&mut rt, "n");
    }
    key(&mut rt, "RET");
    assert!(pane(&mut rt, 1).contains("Thanks! I added the two points"));
    let at_mika = rt.eval(selected).unwrap();
    key(&mut rt, "n");
    assert!(pane(&mut rt, 1).contains("Agreed. I will write up"));
    assert_ne!(rt.eval(selected).unwrap(), at_mika, "the highlighted row moves with the reader");
    assert_eq!(rt.eval(selected).unwrap(), rt.eval("(hash-table-ref (sget (current-session) 'mail) 'reader-id)").unwrap());
    key(&mut rt, "n");
    assert!(pane(&mut rt, 1).contains("All checks passed."), "then the next conversation");
    key(&mut rt, "b");
    key(&mut rt, "p");
    key(&mut rt, "A");
    let subjects = summary_subjects(&mut rt);
    assert_eq!(subjects.matches("Notes from the workshop").count(), 1, "archiving a row archives that message: {subjects}");
    key(&mut rt, "a");
    assert_eq!(summary_subjects(&mut rt).matches("Notes from the workshop").count(), 2);
}

#[test]
fn the_selected_row_still_shows_whether_it_is_unread() {
    let (dir, mut rt) = editor();
    samples(&mut rt, dir.path());
    let s = rt.snapshot();
    let text = s.panes[0].text.to_string();
    // The colour of the text at NEEDLE: the selection adds a background only.
    let fg = |needle: &str| {
        let at = text.find(needle).unwrap();
        s.panes[0].layers.iter().filter(|h| h.from <= at && at < h.to).find_map(|h| s.style(h.style).fg.map(|c| c.rgb))
    };
    assert!(fg("A place for").is_some());
    assert_eq!(fg("A place for"), fg("Sketches for"), "selected and unselected unread subjects");
    assert_ne!(fg("A place for"), fg("Coffee next"), "unread and read subjects");
}

#[test]
fn maximizing_summaries_uses_the_full_width_for_subjects() {
    let (dir, mut rt) = editor();
    samples(&mut rt, dir.path());
    key(&mut rt, "RET");
    key(&mut rt, "b");
    key(&mut rt, "f");
    assert_eq!(rt.snapshot().panes.len(), 1);
    assert!(pane(&mut rt, 0).contains("A place for the longer conversations"));
    key(&mut rt, "f");
    assert_eq!(rt.snapshot().panes.len(), 2);
}

#[test]
fn plain_text_quotes_fold_and_no_markdown_is_interpreted() {
    let (dir, mut rt) = editor();
    samples(&mut rt, dir.path());
    for _ in 0..3 {
        key(&mut rt, "n");
    }
    key(&mut rt, "RET");
    let folded = pane(&mut rt, 1);
    assert!(folded.contains("Notes from the workshop"), "{folded}");
    assert!(folded.contains("Show quoted text"), "{folded}");
    command(&mut rt, "mail-toggle-quotes");
    assert!(pane(&mut rt, 1).contains("Could we make this extensible"));
}

#[test]
fn quit_restores_previous_panes_and_reader_is_read_only() {
    let (dir, mut rt) = editor();
    samples(&mut rt, dir.path());
    key(&mut rt, "RET");
    let before = pane(&mut rt, 1);
    // An ordinary self-insertion must not mutate a received message.
    rt.handle(Input::Key { key: "X".into(), at: Instant::now() });
    rt.take_errors();
    assert_eq!(pane(&mut rt, 1), before);
    command(&mut rt, "mail-back");
    key(&mut rt, "q"); // The list's q goes back to the dashboard,
    assert_eq!(rt.snapshot().panes.len(), 1);
    assert!(pane(&mut rt, 0).contains("Accounts"));
    key(&mut rt, "q"); // and the dashboard's leaves mail.
    assert_eq!(rt.snapshot().panes.len(), 1);
    assert!(pane(&mut rt, 0).contains("My previous workspace"));
}

#[test]
fn the_dashboard_lists_every_account_and_its_folders_and_opens_them_as_lists() {
    let (dir, mut rt) = editor();
    rt.handle(Input::Viewport { columns: 120, rows: 50 });
    let database = dir.path().join("mail.sqlite").display().to_string();
    rt.eval(&format!(
        "(import (techne mail store)) (set-option! 'mail-database {database:?})
         (set-option! 'mail-accounts '(((id . \"work\") (kind . imap) (host . \"imap.invalid\"))
                                       ((id . \"home\") (kind . maildir) (path . \"/missing-home\"))
                                       ((id . \"lists\") (kind . maildir) (path . \"/missing-lists\"))))
         (let ((db (mail-store-open {database:?})))
           (mail-store-import! db \"work\" \"INBOX\" \"1:1\" \"Subject: Work one\\nMessage-ID: <w1@x>\\nDate: Thu, 5 Nov 2026 10:00:00 +0000\\n\\nbody\" '())
           (mail-store-import! db \"work\" \"Sent\" \"1:1\" \"Subject: Work sent\\nMessage-ID: <w2@x>\\nDate: Thu, 5 Nov 2026 11:00:00 +0000\\n\\nbody\" '(\"\\\\Seen\"))
           (mail-store-import! db \"home\" \"Inbox\" \"a\" \"Subject: Home one\\nMessage-ID: <h1@x>\\nDate: Thu, 5 Nov 2026 12:00:00 +0000\\n\\nbody\" '())
           (mail-store-close db))"
    ))
    .unwrap();
    // Not from the mail workspace: M-x mail from any buffer.
    command(&mut rt, "mail");
    let dashboard = pane(&mut rt, 0);
    assert_eq!(rt.snapshot().panes.len(), 1, "the dashboard has the frame: {dashboard}");
    for account in ["work", "home", "lists"] {
        assert!(dashboard.lines().any(|l| l.contains(account)), "every configured account is listed: {dashboard}");
    }
    assert!(dashboard.lines().any(|l| l.contains("lists") && l.contains("not fetched yet")), "{dashboard}");
    let inbox = dashboard.lines().find(|l| l.contains("Inbox") && l.contains(" i ")).unwrap();
    assert!(inbox.trim_end().ends_with('2') && inbox.matches('2').count() == 2, "the Inbox folders: two, both unread: {inbox}");
    // A folder: RET opens it as a list above the reader, as a group does.
    // Inbox, Unread, Starred, All mail, Drafts, work, its INBOX, its Sent:
    // n passes over headings and blank rows.
    for _ in 0..7 {
        key(&mut rt, "n");
    }
    key(&mut rt, "RET");
    assert_eq!(rt.snapshot().panes.len(), 1, "the list, alone until RET opens a message");
    assert!(pane(&mut rt, 0).contains("work · Sent · 1 message"), "{}", pane(&mut rt, 0));
    assert_eq!(summary_subjects(&mut rt), "(\"Work sent\")");
    key(&mut rt, "q");
    // G knows every account's folders too, and opens them the same way.
    command(&mut rt, "mail-groups");
    enter(&mut rt, "home · Inbox");
    assert_eq!(rt.snapshot().panes.len(), 1);
    assert_eq!(summary_subjects(&mut rt), "(\"Home one\")");
    // A catch-up on the dashboard is of the selected row.
    // On the dashboard, TAB folds the selected row's account, and c makes
    // its mail read, and nothing else.
    key(&mut rt, "q");
    key(&mut rt, "TAB");
    assert!(!pane(&mut rt, 0).contains("Sent"), "{}", pane(&mut rt, 0));
    key(&mut rt, "c");
    assert_eq!(
        rt.eval("(let ((db (mail-store-open (option #f 'mail-database)))) (let ((n (mail-ref (mail-store-counts db) 'unread))) (mail-store-close db) n))").unwrap(),
        "1",
        "home's stays unread"
    );
    assert!(pane(&mut rt, 0).lines().any(|l| l.contains("Inbox") && l.contains(" i ") && l.contains('1')));
}

#[test]
fn composing_is_from_the_account_shown_and_completes_whom_you_write_to() {
    let (dir, mut rt) = editor();
    rt.handle(Input::Viewport { columns: 160, rows: 50 });
    let database = dir.path().join("mail.sqlite").display().to_string();
    rt.eval(&format!(
        "(import (techne mail store)) (set-option! 'mail-database {database:?})
         (set-option! 'mail-accounts '(((id . \"work\") (kind . maildir) (path . \"/missing-work\"))
                                       ((id . \"home\") (kind . maildir) (path . \"/missing-home\"))))
         (let ((db (mail-store-open {database:?})))
           (define (put account box key from to subject)
             (mail-store-import! db account box key
               (string-append \"From: \" from \"\\nTo: \" to \"\\nSubject: \" subject \"\\nMessage-ID: <\" key \"@x>\\nDate: Thu, 5 Nov 2026 10:00:00 +0000\\n\\nbody\") '(\"\\\\Seen\")))
           (put \"work\" \"Sent\" \"s1\" \"Ada Worker <ada@work.example>\" \"Ben Byrne <ben@x.org>\" \"To Ben\")
           (put \"work\" \"Sent\" \"s2\" \"Ada Worker <ada@work.example>\" \"Ben Byrne <ben@x.org>, Dan <dan@x.org>\" \"To both\")
           (put \"work\" \"INBOX\" \"i1\" \"Carol Cole <carol@x.org>\" \"ada@work.example\" \"From Carol\")
           (put \"work\" \"INBOX\" \"i2\" \"A List <list@lists.org>\" \"dev@lists.org\" \"On the list\")
           (put \"work\" \"INBOX\" \"i3\" \"GitHub <notifications@github.com>\" \"ada@work.example\" \"A notification\")
           (put \"home\" \"[Gmail]/Sent Mail\" \"h1\" \"Ada Home <ada@home.example>\" \"Erin <erin@x.org>\" \"From home\")
           (mail-store-close db))"
    ))
    .unwrap();
    // From a folder of an account: that account, and the From it sends with.
    command(&mut rt, "mail");
    command(&mut rt, "mail-groups");
    enter(&mut rt, "home · [Gmail]/Sent Mail");
    command(&mut rt, "mail-compose");
    // Asked, with the account of the folder first, each with its From.
    let prompt = rt.snapshot().minibuffer.expect("the account is asked for");
    assert_eq!(prompt.rows[prompt.selected.unwrap()].text(0).split(' ').next().unwrap(), "home");
    key(&mut rt, "RET");
    // The draft is where the list was: it was alone in the frame.
    let draft = |rt: &mut Runtime| {
        let f = rt.snapshot().focus;
        pane(rt, f)
    };
    assert!(draft(&mut rt).starts_with("From: Ada Home <ada@home.example>\nTo: \n"), "{}", draft(&mut rt));
    // To completes whom you wrote to, then who wrote to you; not lists or
    // notifications.
    for c in "by".chars() {
        key(&mut rt, &c.to_string());
    }
    key(&mut rt, "C-M-i");
    key(&mut rt, "TAB");
    key(&mut rt, "RET");
    assert!(draft(&mut rt).contains("To: Ben Byrne <ben@x.org>\n"), "{}", draft(&mut rt));
    for c in ", co".chars() {
        key(&mut rt, &c.to_string());
    }
    key(&mut rt, "C-M-i");
    key(&mut rt, "TAB");
    key(&mut rt, "RET");
    assert!(draft(&mut rt).contains("To: Ben Byrne <ben@x.org>, Carol Cole <carol@x.org>\n"), "{}", draft(&mut rt));
    // Two parts, as Orderless takes them: M-SPC separates them.
    for c in ", co".chars() {
        key(&mut rt, &c.to_string());
    }
    key(&mut rt, "C-M-i");
    key(&mut rt, "M-SPC");
    for c in "carol".chars() {
        key(&mut rt, &c.to_string());
    }
    key(&mut rt, "TAB");
    key(&mut rt, "RET");
    assert!(draft(&mut rt).contains("Carol Cole <carol@x.org>, Carol Cole <carol@x.org>\n"), "{}", draft(&mut rt));
    for c in ", list".chars() {
        key(&mut rt, &c.to_string());
    }
    key(&mut rt, "C-M-i");
    assert!(rt.snapshot().echo.contains("No match"), "a list is not a contact: {}", rt.snapshot().echo);
    // In the body, nothing completes.
    key(&mut rt, "M->");
    for c in "ben".chars() {
        key(&mut rt, &c.to_string());
    }
    key(&mut rt, "C-M-i");
    assert!(rt.snapshot().echo.contains("No completion here"), "{}", rt.snapshot().echo);
    // A reply is from the address the message was written to.
    key(&mut rt, "C-c C-d");
    command(&mut rt, "mail-groups");
    enter(&mut rt, "work · INBOX");
    command(&mut rt, "mail-search");
    enter(&mut rt, "from:carol");
    key(&mut rt, "RET");
    command(&mut rt, "mail-reply");
    assert!(draft(&mut rt).starts_with("From: Ada Worker <ada@work.example>\nTo: \"Carol Cole\" <carol@x.org>\n"), "{}", draft(&mut rt));
}

#[test]
fn drafts_are_kept_for_later_discarded_or_deleted_as_in_message_mode() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    let drafts = |rt: &mut Runtime| {
        rt.eval("(let* ((db (mail-store-open (option #f 'mail-database))) (n (length (mail-store-drafts db)))) (mail-store-close db) n)")
            .unwrap()
    };
    // Nothing written: C-c C-k discards it at once.
    command(&mut rt, "mail");
    command(&mut rt, "mail-compose");
    assert_eq!(drafts(&mut rt), "1");
    key(&mut rt, "C-c C-k");
    assert_eq!(drafts(&mut rt), "0");
    assert!(rt.eval("(buffer-named \"*mail draft\")").unwrap() == "#f");
    assert!(rt.eval("(any (lambda (b) (eq? (buffer-mode b) 'mail-compose-mode)) (buffer-list))").unwrap() == "#f", "its buffer is gone");
    // Something written: asked first; C-c C-d keeps it for later.
    command(&mut rt, "mail-compose");
    for c in "ben@example.org".chars() {
        key(&mut rt, &c.to_string());
    }
    key(&mut rt, "C-c C-k");
    assert!(rt.snapshot().minibuffer.is_some(), "asked first");
    enter(&mut rt, "no");
    assert_eq!(drafts(&mut rt), "1");
    key(&mut rt, "C-c C-d");
    assert_eq!(drafts(&mut rt), "1", "kept for later");
    // The drafts are a mailbox: listed, RET opens one below the list, d
    // flags it and x deletes it.
    command(&mut rt, "mail-drafts");
    let list = pane(&mut rt, 0);
    assert!(list.contains("Drafts · 1 draft") && list.contains("ben@example.org") && list.contains("(no subject)"), "{list}");
    key(&mut rt, "RET");
    assert_eq!(rt.snapshot().panes.len(), 2, "the draft opens below its list");
    assert!(pane(&mut rt, 1).contains("To: ben@example.org"));
    key(&mut rt, "C-c C-d");
    assert!(pane(&mut rt, 0).contains("Drafts · 1 draft"), "back to the drafts: {}", pane(&mut rt, 0));
    key(&mut rt, "d");
    key(&mut rt, "x");
    enter(&mut rt, "yes");
    assert_eq!(drafts(&mut rt), "0");
    assert!(pane(&mut rt, 0).contains("No drafts."));
    assert!(rt.eval("(any (lambda (b) (eq? (buffer-mode b) 'mail-compose-mode)) (buffer-list))").unwrap() == "#f", "and its buffer");
    // A draft of an account no longer configured opens, sent from one
    // that is, and goes as the others do.
    rt.eval(
        "(let ((db (mail-store-open (option #f 'mail-database))))
               (mail-store-draft! db \"orphan-draft-0001\" \"maildir:/gone\" \"From: \\nTo: \\nSubject: \\n\\n\") (mail-store-close db))",
    )
    .unwrap();
    command(&mut rt, "mail-drafts");
    assert!(pane(&mut rt, 0).contains("maildir:/gone (not configured)"), "{}", pane(&mut rt, 0));
    key(&mut rt, "RET");
    assert!(pane(&mut rt, 1).starts_with("From: ada@example.org\n"), "it opens, From the account it is sent from: {}", pane(&mut rt, 1));
    key(&mut rt, "C-c C-k");
    assert_eq!(drafts(&mut rt), "0", "and goes");
}

/// The files of a Maildir folder, by name.
fn files(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = ["cur", "new"]
        .iter()
        .flat_map(|sub| {
            std::fs::read_dir(dir.join(sub))
                .into_iter()
                .flatten()
                .map(move |e| format!("{sub}/{}", e.unwrap().file_name().to_string_lossy()))
        })
        .collect();
    names.sort();
    names
}

#[test]
fn archive_and_delete_change_the_maildir_as_mu4e_does() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    let gmail = dir.path().join("gmail");
    let plain = dir.path().join("plain");
    for (root, folder, file, id, hour) in [
        (&gmail, "Inbox", "1", "<one@x>", 12),
        (&gmail, "[Gmail]/All Mail", "1", "<one@x>", 12),
        (&gmail, "Inbox", "2", "<two@x>", 11),
        (&gmail, "[Gmail]/All Mail", "2", "<two@x>", 11),
        (&gmail, "[Gmail]/Trash", "", "", 0),
        (&plain, "Inbox", "3", "<three@x>", 10),
        (&plain, "Inbox", "4", "<four@x>", 9),
        (&plain, "Archive", "", "", 0),
        (&plain, "Trash", "", "", 0),
    ] {
        std::fs::create_dir_all(root.join(folder).join("cur")).unwrap();
        if !file.is_empty() {
            std::fs::write(
                root.join(folder).join("cur").join(format!("{file}:2,S")),
                format!("From: a@x\r\nSubject: m{file}\r\nMessage-ID: {id}\r\nDate: Thu, 5 Nov 2026 {hour}:00:00 +0000\r\n\r\nbody\r\n"),
            )
            .unwrap();
        }
    }
    rt.eval(&format!(
        "(set-option! 'mail-accounts '(((id . \"g\") (kind . maildir) (path . {:?}) (archive-inbox-label? . #t))
                                       ((id . \"p\") (kind . maildir) (path . {:?}) (archive . \"Archive\"))))",
        gmail.display().to_string(),
        plain.display().to_string()
    ))
    .unwrap();
    command(&mut rt, "mail");
    command(&mut rt, "mail-sync");
    pump_until(&mut rt, |rt| pane(rt, 0).contains("Up to date"));
    key(&mut rt, "i");
    assert_eq!(summary_subjects(&mut rt), "(\"m1\" \"m2\" \"m3\" \"m4\")");
    // Gmail: archiving is leaving the Inbox; the Inbox copy gets T, which
    // mbsync expunges, and All Mail keeps it.
    key(&mut rt, "A");
    assert_eq!(files(&gmail.join("Inbox")), ["cur/1:2,ST", "cur/2:2,S"]);
    assert_eq!(files(&gmail.join("[Gmail]/All Mail")), ["cur/1:2,S", "cur/2:2,S"]);
    assert_eq!(summary_subjects(&mut rt), "(\"m2\" \"m3\" \"m4\")", "out of the Inbox");
    // a undoes it, until mbsync runs.
    key(&mut rt, "a");
    assert_eq!(files(&gmail.join("Inbox")), ["cur/1:2,S", "cur/2:2,S"]);
    assert_eq!(summary_subjects(&mut rt), "(\"m1\" \"m2\" \"m3\" \"m4\")");
    // Delete moves to the real Trash, even when there is an All Mail folder.
    key(&mut rt, "M-<");
    key(&mut rt, "n");
    key(&mut rt, "d");
    // An account with an archive folder: archiving moves it there.
    key(&mut rt, "r");
    key(&mut rt, "x");
    assert_eq!(files(&gmail.join("Inbox")), ["cur/1:2,S"]);
    assert_eq!(files(&gmail.join("[Gmail]/All Mail")), ["cur/1:2,S", "cur/2:2,S"]);
    assert_eq!(files(&gmail.join("[Gmail]/Trash")), ["cur/2:2,S"]);
    assert_eq!(files(&plain.join("Inbox")), ["cur/4:2,S"]);
    assert_eq!(files(&plain.join("Archive")), ["cur/3:2,S"]);
    assert_eq!(summary_subjects(&mut rt), "(\"m1\" \"m4\")");
    // And deleting moves it to the trash folder.
    key(&mut rt, "n");
    key(&mut rt, "d");
    key(&mut rt, "x");
    assert_eq!(files(&plain.join("Inbox")), Vec::<String>::new());
    assert_eq!(files(&plain.join("Trash")), ["cur/4:2,S"]);
    // One undo for what x did.
    key(&mut rt, "a");
    assert_eq!(files(&plain.join("Inbox")), ["cur/4:2,S"]);
    assert_eq!(files(&plain.join("Trash")), Vec::<String>::new());
    // The next sync reads the files; the moved copy stays out of the Inbox.
    command(&mut rt, "mail-sync");
    pump_until(&mut rt, |rt| pane(rt, 0).contains("Up to date"));
    command(&mut rt, "mail-inbox");
    assert_eq!(summary_subjects(&mut rt), "(\"m1\" \"m4\")");
}

#[test]
fn a_sync_leaves_the_cache_as_the_configuration_says() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    let root = dir.path().join("account");
    for (folder, file) in [("Inbox", "a:2,S"), ("Inbox", "b:2,S"), ("lists", "c:2,S")] {
        std::fs::create_dir_all(root.join(folder).join("cur")).unwrap();
        std::fs::write(
            root.join(folder).join("cur").join(file),
            format!("Subject: {folder} {file}\r\nMessage-ID: <{file}@x>\r\n\r\nbody\r\n"),
        )
        .unwrap();
    }
    let path = root.display().to_string();
    let missing = dir.path().join("unmounted").display().to_string();
    let configure = |rt: &mut Runtime, skip: &str| {
        rt.eval(&format!(
            "(set-option! 'mail-accounts '(((id . \"here\") (kind . maildir) (path . {path:?}) (skip . ({skip})))
                                           ((id . \"away\") (kind . maildir) (path . {missing:?}))))"
        ))
        .unwrap();
    };
    let cached = |rt: &mut Runtime| {
        rt.eval(
            "(let* ((db (mail-store-open (option #f 'mail-database)))
                    (rows (sqlite-query db \"SELECT account||' '||mailbox||' '||source_key FROM mail_messages ORDER BY 1\" '())))
               (mail-store-close db) (map car rows))",
        )
        .unwrap()
    };
    // What was cached before: an account opened as a Maildir once, and one
    // whose disk is not mounted now; and a draft.
    rt.eval(
        "(let ((db (mail-store-open (option #f 'mail-database))))
           (mail-store-import! db \"maildir:/old\" \"INBOX\" \"o\" \"Subject: old\\n\\nbody\" '())
           (mail-store-import! db \"away\" \"Inbox\" \"w\" \"Subject: away\\n\\nbody\" '())
           (mail-store-draft! db \"d1\" \"maildir:/old\" \"To: x\\n\\ndraft\")
           (mail-store-close db))",
    )
    .unwrap();
    configure(&mut rt, "");
    command(&mut rt, "mail");
    assert!(pane(&mut rt, 0).lines().any(|l| l.contains("maildir:/old") && l.contains("g drops it")), "{}", pane(&mut rt, 0));
    command(&mut rt, "mail-sync");
    pump_until(&mut rt, |rt| pane(rt, 0).contains("Sync incomplete"));
    assert_eq!(
        cached(&mut rt),
        "(\"away Inbox w\" \"here Inbox a\" \"here Inbox b\" \"here lists c\")",
        "the account no longer configured is gone; the one that cannot be read now keeps its mail"
    );
    assert!(pane(&mut rt, 0).contains("1 gone from the cache"), "{}", pane(&mut rt, 0));
    assert!(!pane(&mut rt, 0).contains("maildir:/old"));
    assert_eq!(
        rt.eval("(let* ((db (mail-store-open (option #f 'mail-database))) (n (length (mail-store-drafts db)))) (mail-store-close db) n)")
            .unwrap(),
        "1",
        "drafts stay"
    );
    // A folder skipped now, and a file deleted at the source.
    std::fs::remove_file(root.join("Inbox/cur/b:2,S")).unwrap();
    configure(&mut rt, "\"lists\"");
    command(&mut rt, "mail-sync");
    pump_until(&mut rt, |rt| pane(rt, 0).contains("2 gone from the cache"));
    assert_eq!(cached(&mut rt), "(\"away Inbox w\" \"here Inbox a\")");
}

#[test]
fn a_sync_reads_every_accounts_inbox_before_the_other_folders() {
    let (dir, mut rt) = editor();
    real_accounts(&mut rt, dir.path());
    let mut accounts = String::new();
    for account in ["one", "two"] {
        let root = dir.path().join(account);
        for folder in ["Inbox", "lists", "[Gmail]/All Mail"] {
            std::fs::create_dir_all(root.join(folder).join("cur")).unwrap();
            std::fs::write(
                root.join(folder).join("cur/m:2,S"),
                format!("Subject: {account} {folder}\r\nMessage-ID: <{account}-{folder}@x>\r\n\r\nbody\r\n"),
            )
            .unwrap();
        }
        accounts.push_str(&format!("((id . {account:?}) (kind . maildir) (path . {:?}))", root.display().to_string()));
    }
    rt.eval(&format!("(set-option! 'mail-accounts '({accounts}))")).unwrap();
    command(&mut rt, "mail");
    command(&mut rt, "mail-sync");
    pump_until(&mut rt, |rt| pane(rt, 0).contains("Up to date"));
    assert_eq!(
        rt.eval(
            "(let* ((db (mail-store-open (option #f 'mail-database)))
                    (rows (sqlite-query db \"SELECT account||' '||mailbox FROM mail_messages ORDER BY id\" '())))
               (mail-store-close db) (map car rows))"
        )
        .unwrap(),
        "(\"one Inbox\" \"two Inbox\" \"one lists\" \"two lists\" \"one [Gmail]/All Mail\" \"two [Gmail]/All Mail\")"
    );
    let dashboard = pane(&mut rt, 0);
    for account in ["one", "two"] {
        assert!(dashboard.lines().any(|l| l.contains(account) && l.contains("synced") && l.contains("3 new")), "{dashboard}");
    }
}

#[test]
fn a_group_chosen_from_outside_mail_opens_as_from_the_dashboard() {
    let (dir, mut rt) = editor();
    rt.handle(Input::Viewport { columns: 160, rows: 50 });
    rt.eval(&format!(
        "(require {:?}) (mail-test-seed-samples! {:?})",
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../lisp/tests/fixtures/mail/mail-sample-fixture.scm"),
        dir.path().join("samples.sqlite").display().to_string()
    ))
    .unwrap();
    command(&mut rt, "mail");
    key(&mut rt, "q");
    command(&mut rt, "mail-groups");
    enter(&mut rt, "Starred");
    assert_eq!(rt.snapshot().panes.len(), 1, "the list, as from the dashboard");
    assert!(pane(&mut rt, 0).contains("Starred · 1 message"));
    key(&mut rt, "RET");
    assert!(rt.eval("(session-tree (current-session))").unwrap().contains("split below"));
    assert!(pane(&mut rt, 1).contains("Sketches for the reading room"));
    key(&mut rt, "b");
    key(&mut rt, "q");
    assert!(pane(&mut rt, 0).contains("Accounts"));
    key(&mut rt, "q");
    assert!(pane(&mut rt, 0).contains("My previous workspace"));
}
