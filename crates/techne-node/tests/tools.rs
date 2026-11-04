//! The public Scheme API, including bounded output, stale edits and live jobs.
use std::{fs, path::Path};
use techne_vm::vm::{Grants, Vm};

fn world(root: &Path) -> Vm {
    let mut vm = Vm::new();
    techne_node::install(&mut vm).unwrap();
    let library = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lisp/techne/tools.sld");
    vm.eval_source(&format!(
        "(include {:?}) (import (techne tools)) (define p (tool-project {:?}))
         (define (check expected actual) (unless (equal? expected actual) (error \"check\" expected actual)))
         (define (fails thunk) (guard (e (#t #t)) (thunk) #f))",
        library.to_str().unwrap(),
        root.to_str().unwrap()
    ))
    .unwrap();
    vm
}
fn check(vm: &mut Vm, source: &str) {
    let value = vm.eval_source(&format!("{source}\n'done")).unwrap();
    assert_eq!(vm.repr(&value), "done");
}

#[test]
fn discovery_uses_live_metadata_and_pages_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (define (newly-defined x) "Return X unchanged, an agent discovery fixture." x)
        (define d (tool-discover "newly-defined" #:limit 1))
        (check 'newly-defined (tool-ref (car (tool-ref d 'items)) 'name))
        (check 1 (length (tool-ref d 'items)))
        (check #t (string? (tool-ref (car (tool-ref d 'items)) 'signature)))
        (define desc (tool-describe 'newly-defined #:chars 8))
        (check "Return X" (tool-ref desc 'doc))
        (check #t (tool-ref desc 'truncated))
        (check 8 (tool-ref desc 'next))
        (check #t (fails (lambda () (tool-discover "read" #:limit 0))))
        (check #t (fails (lambda () (tool-discover "read" #:offset -1))))
        (check #t (fails (lambda () (tool-describe 'no-such-binding))))
    "#,
    );
}

#[test]
fn files_and_search_are_bounded_and_honest_about_skips() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(".ignore"), "ignored\n").unwrap();
    fs::write(dir.path().join("ignored"), "needle").unwrap();
    fs::write(dir.path().join("one.scm"), "needle needle\nneedle\n").unwrap();
    fs::write(dir.path().join("two.txt"), format!("{}needle{}\n", "é".repeat(1000), "x".repeat(1000))).unwrap();
    fs::write(dir.path().join("binary"), [0xff]).unwrap();
    // Version control metadata, at the root and nested, a directory or (a
    // worktree's, a submodule's) a file, is never listed; other hidden
    // files are, under the ignore rules.
    for meta in [".git", ".jj", ".hg", ".svn"] {
        fs::create_dir(dir.path().join(meta)).unwrap();
        fs::write(dir.path().join(meta).join("config"), "needle").unwrap();
    }
    fs::write(dir.path().join(".jj").join(".gitignore"), "/*\n").unwrap();
    fs::create_dir_all(dir.path().join(".github/workflows")).unwrap();
    fs::write(dir.path().join(".github/workflows/ci.yml"), "on: push\n").unwrap();
    fs::create_dir_all(dir.path().join("ignored-dir/.git")).unwrap();
    fs::create_dir_all(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join("sub/.git"), "gitdir: elsewhere\n").unwrap();
    fs::write(dir.path().join(".ignore"), "ignored\nignored-dir\n").unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (define files (tool-files p #:limit 1))
        (check 5 (tool-ref files 'total))
        (check '(".github/workflows/ci.yml" ".ignore" "binary" "one.scm" "two.txt") (tool-ref (tool-files p #:limit 10) 'items))
        (check 1 (length (tool-ref files 'items)))
        (check 1 (tool-ref files 'next))
        (check #t (tool-ref files 'truncated))
        (define found (tool-search p "needle" #:limit 1))
        (check 3 (tool-ref found 'total))
        (check 1 (length (tool-ref found 'items)))
        (check #t (tool-ref found (quote complete)))
        (check 0 (tool-ref found (quote skipped)))
        (check 1 (tool-ref found (quote binary)))
        (check 0 (length (tool-ref found (quote problems))))
        (define line (tool-search p "needle" #:prefix "two"))
        (check #t (tool-ref line 'complete))
        (define item (car (tool-ref line 'items)))
        (check #t (and (string-contains (tool-ref item 'text) "needle") #t))
        (check #t (<= (string-length (tool-ref item 'text)) 240))
        (check 1 (tool-ref item 'line))
        (check #t (tool-ref (tool-expand p (tool-ref item 'handle) #:chars 10) 'truncated))
        (check #t (fails (lambda () (tool-search p ""))))
        (check #t (fails (lambda () (tool-files p #:limit 51))))
    "#,
    );
}

#[test]
fn search_handles_survive_counting_more_files_than_the_cache_holds() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..150 {
        fs::write(dir.path().join(format!("{i:03}.txt")), if i == 0 { "needle" } else { "other" }).unwrap();
    }
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (define found (tool-search p "needle" #:limit 1))
        (check 1 (tool-ref found (quote total)))
        (check "needle" (tool-ref (tool-expand p (tool-ref (car (tool-ref found (quote items))) (quote handle))) (quote text)))
    "#,
    );
}

#[test]
fn a_search_keeps_the_handles_of_earlier_replies() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("read.txt"), "kept").unwrap();
    for i in 0..150 {
        fs::write(dir.path().join(format!("{i:03}.txt")), "other").unwrap();
    }
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (define read (tool-read p "read.txt"))
        (check 0 (tool-ref (tool-search p "absent") (quote total)))
        (check "kept" (tool-ref (tool-expand p (tool-ref read (quote handle))) (quote text)))
    "#,
    );
}

#[test]
fn snapshots_page_unicode_and_expire_explicitly() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("source"), "aé€😀\nnext\n").unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (define r (tool-read p "source" #:lines 1 #:chars 2))
        (define h (tool-ref r 'handle))
        (check "aé" (tool-ref r 'text))
        (check 2 (tool-ref r 'next))
        (check "€😀\n" (tool-ref (tool-expand p h #:offset 2) 'text))
        (check #f (tool-ref (tool-expand p h #:offset 2) 'truncated))
        (check "next\n" (tool-ref (tool-read p "source" #:from 2 #:lines 1) 'text))
        (check #t (fails (lambda () (tool-read p "source" #:from 99))))
        (check #t (fails (lambda () (tool-expand p h #:offset 99))))
        (check #t (fails (lambda () (tool-expand (tool-project (tool-root p)) h))))
    "#,
    );
    fs::write(dir.path().join("source"), "changed").unwrap();
    check(
        &mut vm,
        r#"
        (check "aé€😀\n" (tool-ref (tool-expand p h) 'text))
        (check "changed" (tool-ref (tool-read p "source") 'text))
        (check #t (tool-refresh! p))
        (check #t (fails (lambda () (tool-expand p h))))
    "#,
    );
}

#[test]
fn definitions_come_from_syntax_not_matching_comments() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("source.scm"), "; (define fake 1)\n(define (real x) (+ x 1))\n'(define quoted 1)\n").unwrap();
    fs::write(
        dir.path().join("source.rs"),
        "// fn fake() {}\nstruct Foo {}\nimpl Foo { fn real(&self) { let s = \"fn fake() {}\"; } }\nfn real() {}\n",
    )
    .unwrap();
    fs::write(dir.path().join("other.txt"), "fn fake() {}").unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (check 1 (tool-ref (tool-symbols p "source.scm") 'total))
        (check "(define (real x) (+ x 1))" (tool-ref (tool-read-symbol p "source.scm" "real") 'text))
        (check 3 (tool-ref (tool-symbols p "source.rs") 'total))
        (check 2 (tool-ref (tool-symbols p "source.rs" #:name "real") 'total))
        (check #t (fails (lambda () (tool-read-symbol p "source.rs" "real"))))
        (check #t (fails (lambda () (tool-read-symbol p "source.rs" "fake"))))
        (check #t (fails (lambda () (tool-symbols p "other.txt"))))
        (check "struct Foo {}" (tool-ref (tool-read-symbol p "source.rs" "Foo") 'text))
    "#,
    );
}

#[test]
fn replacements_validate_the_original_and_do_not_partially_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source");
    fs::write(&path, "one é\ntwo\nthree\n").unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (define before (tool-read p "source"))
        (define rev (tool-ref before 'revision))
        (check #t (fails (lambda () (tool-edit! p "source" rev '(("one" "ONE") ("absent" "no"))))))
        (check "one é\ntwo\nthree\n" (tool-ref (tool-read p "source") 'text))
        (check #t (fails (lambda () (tool-edit! p "source" rev '(("one é" "x") ("é" "y"))))))
        (define after (tool-edit! p "source" rev '(("one é" "ONE 😀") ("two" "TWO"))))
        (check 2 (length (tool-ref after 'changes)))
        (check "ONE 😀\nTWO\nthree\n" (tool-ref (tool-read p "source") 'text))
        (check "one é\ntwo\nthree\n" (tool-ref (tool-expand p (tool-ref before 'handle)) 'text))
        (check #t (fails (lambda () (tool-edit! p "source" rev '(("three" "THREE"))))))
    "#,
    );
    fs::write(&path, "aaaa").unwrap();
    check(
        &mut vm,
        r#"
        (define current (tool-read p "source"))
        (check #t (fails (lambda () (tool-edit! p "source" (tool-ref current 'revision) '(("aaa" "x"))))))
    "#,
    );
    assert_eq!(fs::read_to_string(path).unwrap(), "aaaa");
}

#[test]
fn external_edits_paths_and_cache_limits_are_checked() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    fs::write(dir.path().join("source"), "first").unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (define old (tool-read p "source"))
        (check #t (fails (lambda () (tool-read p "escape"))))
    "#,
    );
    fs::write(dir.path().join("source"), "external").unwrap();
    check(
        &mut vm,
        r#"
        (check #t (fails (lambda () (tool-edit! p "source" (tool-ref old 'revision) '(("first" "lost"))))))
        (check "external" (tool-ref (tool-read p "source") 'text))
    "#,
    );
    for i in 0..129 {
        fs::write(dir.path().join("source"), format!("revision {i}")).unwrap();
        check(&mut vm, "(tool-read p \"source\" #:chars 1)");
    }
    check(&mut vm, "(check #t (fails (lambda () (tool-expand p (tool-ref old 'handle)))))");
    fs::write(dir.path().join("large"), vec![b'x'; (1 << 20) + 1]).unwrap();
    check(&mut vm, "(check #t (fails (lambda () (tool-read p \"large\"))))");
}

#[test]
fn search_replace_blocks_check_every_file_before_writing_any() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("one"), "alpha\nbeta\nalpha beta\n").unwrap();
    fs::write(dir.path().join("two"), "gamma\n").unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("target"), dir.path().join("dangling")).unwrap();
    let mut vm = world(dir.path());
    vm.eval_source(
        r#"(define (block file old new)
             (string-append file "\n<<<<<<< SEARCH\n" old (if (string=? old "") "" "\n")
                            "=======\n" new (if (string=? new "") "" "\n") ">>>>>>> REPLACE\n"))"#,
    )
    .unwrap();
    check(
        &mut vm,
        r#"
        ;; One old text missing in the second file: neither file changes.
        (check #t (fails (lambda () (tool-apply! p (string-append (block "two" "gamma" "delta") (block "one" "nowhere" "x"))))))
        (check "gamma\n" (tool-ref (tool-read p "two") 'text))
        ;; Ambiguous old text is refused.
        (check #t (fails (lambda () (tool-apply! p (block "one" "alpha" "x")))))
        ;; Several blocks, the later ones without a file line, and fences.
        (check #t (fails (lambda () (tool-apply! p (string-append (block "one" "alpha\nbeta" "x") (block "one" "beta\nalpha" "y"))))))
        (define done (tool-apply! p (string-append "```\n" (block "one" "alpha beta" "ALPHA BETA")
                                                   "<<<<<<< SEARCH\nalpha\nbeta\n=======\nfirst\n>>>>>>> REPLACE\n```\n"
                                                   (block "two" "gamma" "delta"))))
        (check 2 (length done))
        (check 2 (tool-ref (car done) 'replacements))
        (check "first\nALPHA BETA\n" (tool-ref (tool-read p "one") 'text))
        (check "delta\n" (tool-ref (tool-read p "two") 'text))
        ;; A revision given must be the file's.
        (check #t (fails (lambda () (tool-apply! p (block "two @00000000" "delta" "x")))))
        (define rev (tool-ref (tool-read p "two") 'revision))
        (tool-apply! p (block (string-append "two @" (substring rev 0 12)) "delta" "epsilon"))
        (check "epsilon\n" (tool-ref (tool-read p "two") 'text))
        ;; An empty SEARCH creates, in new directories, but never replaces.
        (check #t (tool-ref (car (tool-apply! p (block "new/dir/three" "" "made"))) 'created))
        (check "made\n" (tool-ref (tool-read p "new/dir/three") 'text))
        (check #t (fails (lambda () (tool-apply! p (block "two" "" "clobbered")))))
        (check #t (fails (lambda () (tool-apply! p (block "../outside" "" "x")))))
        (check #t (fails (lambda () (tool-apply! p (block "/tmp/absolute" "" "x")))))
        (check #t (fails (lambda () (tool-apply! p "no blocks at all"))))
        ;; A bad creation is found before an edit beside it is written.
        (check #t (fails (lambda () (tool-apply! p (string-append (block "two" "epsilon" "zeta") (block "../bad" "" "x"))))))
        (check #t (fails (lambda () (tool-apply! p (string-append (block "two" "epsilon" "zeta") (block "big" "" (make-string 1048577 #\x)))))))
        (check "epsilon\n" (tool-ref (tool-read p "two") 'text))
        ;; Neither a link among the directories nor a dangling link is followed.
        (check #t (fails (lambda () (tool-apply! p (block "escape/newdir/file" "" "x")))))
        (check #t (fails (lambda () (tool-apply! p (block "dangling" "" "x")))))
        ;; Find within a snapshot: lines of the file the handle names.
        (define found (tool-find p (tool-ref (tool-read p "one") 'handle) "ALPHA" #:limit 1))
        (check 1 (tool-ref found 'total))
        (check 2 (tool-ref (car (tool-ref found 'items)) 'line))
        (check "one:2: ALPHA BETA" (tool-show found))
        (check #t (fails (lambda () (tool-find p "stale:handle:0:1" "x"))))
    "#,
    );
    assert!(!dir.path().parent().unwrap().join("outside").exists());
    assert!(!outside.path().join("newdir").exists(), "nothing is made through a link");
    assert!(!outside.path().join("target").exists(), "a dangling link is not written through");
}

#[test]
fn kept_output_is_a_snapshot_to_find_in_and_show() {
    let dir = tempfile::tempdir().unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (define job (tool-run p "sh" '("-c" "seq 1 3000; echo oops >&2") #:retain 5 #:keep 100000))
        (define r (tool-process-result job))
        (define out (tool-ref r 'stdout))
        (check (tool-ref out 'characters) (tool-ref out 'kept))
        (define hits (tool-find p (tool-ref out 'handle) "2999"))
        (check 2999 (tool-ref (car (tool-ref hits 'items)) 'line))
        (check "2998\n2999\n3000\n" (tool-ref (tool-lines p (tool-ref out 'handle) #:from 2998 #:lines 5) 'text))
        (check #t (fails (lambda () (tool-lines p (tool-ref out 'handle) #:from 3002))))
        (define hit (tool-ref (car (tool-ref hits 'items)) 'handle))
        (check "2999\n" (tool-ref (tool-lines p hit #:from 2990 #:lines 20) 'text))
        (check "3000" (substring (tool-ref (tool-expand p (tool-ref out 'handle) #:offset (- (tool-ref out 'kept) 5)) 'text) 0 4))
        ;; The report is made once: the same handles each time.
        (check (tool-ref out 'handle) (tool-ref (tool-ref (tool-process-result job) 'stdout) 'handle))
        (check "oops\n" (tool-ref (tool-expand p (tool-ref (tool-ref r 'stderr) 'handle)) 'text))
    "#,
    );
    check(
        &mut vm,
        r#"
        (define again (tool-process-result (tool-run p "sh" '("-c" "printf abcdef") #:keep 3)))
        (check 3 (tool-ref (tool-ref again 'stdout) 'kept))
        (check "abc" (tool-ref (tool-expand p (tool-ref (tool-ref again 'stdout) 'handle)) 'text))
        (check #f (tool-ref (tool-ref (tool-process-result (tool-run p "true" '())) 'stdout) 'handle))
        (check #t (fails (lambda () (tool-run p "true" '() #:keep 0))))
        (check #t (fails (lambda () (tool-run p "true" '() #:keep 1048577))))
        ;; Both streams at their limit of four-byte characters keep both handles.
        (define wide (tool-process-result
                       (tool-run p "sh" '("-c" "s=😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀; s=$s$s$s$s$s$s$s$s; s=$s$s$s$s$s$s$s$s; s=$s$s$s$s$s$s$s$s; s=$s$s$s$s$s$s$s$s; s=$s$s$s$s$s$s$s$s; s=$s$s; printf %s $s; printf %s $s >&2")
                                 #:keep 1048576)))
        (check 1048576 (tool-ref (tool-ref wide 'stdout) 'kept))
        (check "😀" (tool-ref (tool-expand p (tool-ref (tool-ref wide 'stdout) 'handle) #:chars 1) 'text))
        (check "😀" (tool-ref (tool-expand p (tool-ref (tool-ref wide 'stderr) 'handle) #:chars 1) 'text))
        (check #t (string-prefix? "status 0\n--- stdout (6 characters, last 3 shown; all 3 kept: \""
                                  (tool-show (tool-process-result (tool-run p "printf" '("abcdef") #:retain 3 #:keep 3)))))
        (check "a\nb" (tool-show (list "a" 'b)))
        (check "x" (tool-show (tool-batch (list (lambda () "x")))))
    "#,
    );
}

#[test]
fn a_timeout_kills_a_wait_does_not_and_a_check_finds_what_matters() {
    let dir = tempfile::tempdir().unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (define slow (tool-process-result (tool-run p "sh" '("-c" "echo start; sleep 30; echo never") #:timeout 0.5)))
        (check #t (tool-ref slow 'timed-out))
        (check #f (tool-ref slow 'running))
        (check "start\n" (tool-ref (tool-ref slow 'stdout) 'text))
        (check #t (string-prefix? "timed out, killed" (tool-show slow)))
        (define job (tool-run p "sh" '("-c" "echo one; sleep 2; echo two")))
        (define early (tool-process-result job #:wait 0.5))
        (check #t (tool-ref early 'running))
        (check #f (tool-ref early 'status))
        (check "one\n" (tool-ref (tool-ref early 'stdout) 'text))
        (check #t (string-prefix? "running" (tool-show early)))
        (define late (tool-process-result job))
        (check '(#f 0 #f) (list (tool-ref late 'running) (tool-ref late 'status) (tool-ref late 'timed-out)))
        (check "one\ntwo\n" (tool-ref (tool-ref late 'stdout) 'text))
        (check #t (fails (lambda () (tool-run p "true" '() #:timeout 0))))
        (check #t (fails (lambda () (tool-run p "true" '() #:timeout 86401))))
        (check #t (fails (lambda () (tool-process-result job #:wait -1))))

        (define c (tool-check p "sh" '("-c" "echo ok; echo error: bad; echo after; echo error: bad >&2; exit 2") #:context 1))
        (check 2 (tool-ref c 'status))
        (check 2 (tool-ref (tool-ref c 'hits) 'total))
        (define first (car (tool-ref (tool-ref c 'hits) 'items)))
        (check '(2 "error: bad\nafter\n") (list (tool-ref first 'line) (tool-ref first 'text)))
        (check "<stderr of sh>" (tool-ref (cadr (tool-ref (tool-ref c 'hits) 'items)) 'path))
        (check "after\n" (tool-ref (tool-lines p (tool-ref (tool-ref c 'stdout) 'handle) #:from 3) 'text))
        (define quiet (tool-check p "sh" '("-c" "echo nothing; exit 1")))
        (check '(1 0 "nothing\n") (list (tool-ref quiet 'status) (tool-ref (tool-ref quiet 'hits) 'total) (tool-ref (tool-ref quiet 'stdout) 'tail)))
        (check #t (and (string-contains (tool-show quiet) "nothing matched") #t))
        (define one (tool-ref (tool-check p "sh" '("-c" "echo a; echo ab; echo b") #:find '("a" "b") #:limit 2) 'hits))
        (check '(3 2 #t #f) (list (tool-ref one 'total) (length (tool-ref one 'items)) (tool-ref one 'truncated) (tool-ref one 'next)))
        (check #t (tool-ref (tool-check p "sleep" '("30") #:timeout 0.3) 'timed-out))
        (check #t (fails (lambda () (tool-check p "true" '() #:context 41))))
        (check #t (fails (lambda () (tool-check p "true" '() #:find '("")))))
        (check #t (string? (tool-stamp)))
        (check (tool-stamp) (tool-stamp #:now #t))
    "#,
    );
    check(
        &mut vm,
        r#"
        ;; Its leader gone, what it started still holds its streams: killed at the deadline.
        (define start (current-milliseconds))
        (define orphaned (tool-process-result (tool-run p "sh" '("-c" "sleep 20 & exit 0") #:timeout 0.3)))
        (check #t (tool-ref orphaned 'timed-out))
        (check #t (< (- (current-milliseconds) start) 10000))
        ;; Every matching line counted, past a page of fifty.
        (define many (tool-check p "sh" '("-c" "i=0; while [ $i -lt 60 ]; do echo error: $i; i=$((i+1)); done") #:limit 5))
        (check '(60 5 #t #t) (list (tool-ref (tool-ref many 'hits) 'total) (length (tool-ref (tool-ref many 'hits) 'items))
                                   (tool-ref (tool-ref many 'hits) 'truncated) (tool-ref (tool-ref many 'hits) 'complete)))
        ;; Output past what is kept: searched no further, and said.
        (define long (tool-check p "sh" '("-c" "head -c 1100000 /dev/zero | tr '\\0' x; echo; echo error: late")))
        (check '(#t #f 0) (list (tool-ref long 'lost) (tool-ref (tool-ref long 'hits) 'complete) (tool-ref (tool-ref long 'hits) 'total)))
        (check #t (and (string-contains (tool-ref (tool-ref long 'stdout) 'tail) "error: late") #t))
        ;; A hit past context clipped short is an item of its own.
        (define wide (tool-check p "sh" '("-c" "echo error: one; head -c 17000 /dev/zero | tr '\\0' x; echo; echo error: two") #:context 3))
        (check 2 (length (tool-ref (tool-ref wide 'hits) 'items)))
        (check #t (tool-ref (car (tool-ref (tool-ref wide 'hits) 'items)) 'context-truncated))
        (check "error: two\n" (tool-ref (cadr (tool-ref (tool-ref wide 'hits) 'items)) 'text))
        ;; The second error in no item: truncated, though context was asked.
        (define first-only (tool-check p "sh" '("-c" "echo error: one; head -c 17000 /dev/zero | tr '\\0' x; echo; echo error: two") #:context 3 #:limit 1))
        (check '(1 2 #t) (list (length (tool-ref (tool-ref first-only 'hits) 'items)) (tool-ref (tool-ref first-only 'hits) 'total)
                               (tool-ref (tool-ref first-only 'hits) 'truncated)))
        ;; Many matching lines, many patterns: one pass, each line once.
        (define dense (tool-check p "sh" '("-c" "i=0; while [ $i -lt 3000 ]; do echo error: warning: FAILED $i; i=$((i+1)); done") #:limit 3))
        (check '(3000 3 #t) (list (tool-ref (tool-ref dense 'hits) 'total) (length (tool-ref (tool-ref dense 'hits) 'items))
                                  (tool-ref (tool-ref dense 'hits) 'truncated)))
    "#,
    );
}

#[test]
fn process_streams_drain_concurrently_and_report_loss() {
    let dir = tempfile::tempdir().unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (define job (tool-run p "sh" '("-c" "i=0; while [ $i -lt 12000 ]; do printf abcdefghijklmnopqrstuvwxyz; printf ABCDEFGHIJKLMNOPQRSTUVWXYZ >&2; i=$((i+1)); done; exit 7") #:retain 100))
        (define result (tool-process-result job #:chars 20))
        (check 7 (tool-ref result 'status))
        (check #t (tool-process-done? job))
        (check #t (tool-ref (tool-ref result 'stdout) 'truncated))
        (check #t (tool-ref (tool-ref result 'stderr) 'truncated))
        (check 312000 (tool-ref (tool-ref result 'stdout) 'characters))
        (check 312000 (tool-ref (tool-ref result 'stderr) 'characters))
        (check 20 (string-length (tool-ref (tool-ref result 'stdout) 'text)))
        (check result (tool-process-result job #:chars 20))
        (check #t (fails (lambda () (tool-run p "sh" '() #:retain 0))))
        (define small (tool-process-result (tool-run p "sh" (quote ("-c" "printf abcdef")) #:retain 3) #:chars 20))
        (check "def" (tool-ref (tool-ref small (quote stdout)) (quote text)))
        (check #t (tool-ref (tool-ref small (quote stdout)) (quote truncated)))
        (define sleeper (tool-run p "sh" '("-c" "sleep 300")))
        (define cancelled (tool-process-cancel! sleeper))
        (check #t (tool-process-done? sleeper))
        (check #t (pair? (tool-ref cancelled 'status)))
    "#,
    );
}

#[test]
fn file_operations_require_the_files_capability() {
    let mut vm = Vm::with_grants(Grants::NONE);
    techne_node::install(&mut vm).unwrap();
    let error = vm.eval_source("(%repository-canonical-root \"/\")").unwrap_err();
    assert!(error.msg.contains("not granted"), "{error}");
}
