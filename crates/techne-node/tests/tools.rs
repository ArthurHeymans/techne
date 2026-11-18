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
        ;; Every word, where some binding has them all: no partial.
        (check #f (tool-ref (tool-discover "newly-defined fixture") 'partial))
        ;; Else the bindings with the most words, saying how many.
        (define p (tool-discover "newly-defined fixture zzz-in-nothing"))
        (check 'newly-defined (tool-ref (car (tool-ref p 'items)) 'name))
        (check '((matched 2) (of 3)) (tool-ref p 'partial))
        (check 0 (tool-ref (tool-discover "zzz-in-nothing") 'total))
        ;; Short of every word, a shipped library's export with more of
        ;; them, and the library to import.
        (define l (tool-ref (tool-discover "string index right") 'libraries))
        (check '((name string-index-right) (library (srfi 130))) (car l))
        (check #f (tool-ref (tool-discover "newly-defined fixture") 'libraries))
        ;; The library a file declares for its name, the one importing it
        ;; finds: (srfi 132), not the (srfi 132 sorting) it imports.
        (check '(srfi 132) (tool-ref (car (tool-ref (tool-discover "vector median zzz-in-nothing") 'libraries)) 'library))
        ;; Only exports with more words than the live bindings have.
        (check #f (tool-ref (tool-discover "newly-defined zzz-in-nothing") 'libraries))
        ;; A word given twice counts once.
        (check '((matched 2) (of 3)) (tool-ref (tool-discover "newly-defined newly-defined fixture zzz-in-nothing") 'partial))
        ;; At most 8 exports, first by name, and how many there are.
        (define c (tool-discover "comparator zzz-in-nothing"))
        (check 8 (length (tool-ref c 'libraries)))
        (check #t (> (tool-ref c 'libraries-total) 8))
        (check 'bag-comparator (tool-ref (car (tool-ref c 'libraries)) 'name))
        ;; Shown as text, both say so.
        (define shown (tool-show (tool-discover "string index right")))
        (check #t (and (string-contains shown "[partial: 2 of 3 words matched]") #t))
        (check #t (and (string-contains shown "string-index-right (srfi 130)") #t))
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
        ;; A page wider than the bound is the bound, as a window is: the
        ;; reply says where the rest is. Below 1 is still an error.
        (check 5 (length (tool-ref (tool-files p #:limit 51) 'items)))
        (check 3 (length (tool-ref (tool-search p "needle" #:limit 99) 'items)))
        (check #t (fails (lambda () (tool-files p #:limit 0))))
        (check #t (fails (lambda () (tool-search p "needle" #:limit 1.5))))
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
        ;; Two definitions: the error says where each is, and how to read it.
        (define (message thunk) (guard (e (#t (condition/report-string e))) (thunk) #f))
        (define (at text part) (let ((c (string-contains text part))) (and c (string-cursor->index text c))))
        (define twice (message (lambda () (tool-read-symbol p "source.rs" "real"))))
        (check #t (integer? (at twice "2 definitions of real in source.rs")))
        (check #t (integer? (at twice "line 3, function_item, member: \"")))
        (check #t (integer? (at twice "line 4, function_item, module: \"")))
        (define handle (let ((rest (substring twice (+ (at twice "module: \"") 9) (string-length twice))))
                         (substring rest 0 (at rest "\""))))
        (check "fn real() {}" (tool-ref (tool-expand p handle) 'text))
        (check #t (integer? (at (message (lambda () (tool-read-symbol p "source.rs" "fake")))
                                "nothing named fake is defined in source.rs")))
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
fn an_edit_that_leaves_scheme_unreadable_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.scm"), "(define (f x)\n  (g x))\n(define y 1)\n").unwrap();
    fs::write(dir.path().join("broken.scm"), "(define (f x)\n").unwrap();
    fs::write(dir.path().join("notes.txt"), "(not scheme\n").unwrap();
    let mut vm = world(dir.path());
    vm.eval_source(
        r#"(define (block file old new)
             (string-append file "\n<<<<<<< SEARCH\n" old (if (string=? old "") "" "\n")
                            "=======\n" new (if (string=? new "") "" "\n") ">>>>>>> REPLACE\n"))
           (define (message thunk) (guard (e (#t (condition/report-string e))) (thunk) #f))
           (define (has? text part) (and (string-contains text part) #t))"#,
    )
    .unwrap();
    check(
        &mut vm,
        r#"
        ;; One parenthesis too many: refused, with where, and nothing written.
        (define extra (message (lambda () (tool-apply! p (block "a.scm" "  (g x))" "  (g x)))")))))
        (check #t (has? extra "a.scm would not read as Scheme"))
        (check #t (has? extra "at line 2"))
        (check "(define (f x)\n  (g x))\n(define y 1)\n" (tool-ref (tool-read p "a.scm") 'text))
        ;; One too few, with tool-edit! too.
        (define rev (tool-ref (tool-read p "a.scm") 'revision))
        (check #t (has? (message (lambda () (tool-edit! p "a.scm" rev '(("(g x))" "(g x)")))))
                        "a.scm would not read as Scheme"))
        ;; A new file must read; other files are not Lisp, and one already
        ;; unreadable may be edited as it is.
        (check #t (has? (message (lambda () (tool-apply! p (block "new.sld" "" "(define-library (x)")))) "new.sld"))
        (tool-apply! p (block "notes.txt" "(not scheme" "(still not"))
        (tool-apply! p (block "broken.scm" "(define (f x)" "(define (g x)"))
        (check "(define (g x)\n" (tool-ref (tool-read p "broken.scm") 'text))
        ;; A balanced edit goes through.
        (tool-apply! p (block "a.scm" "  (g x))" "  (h (g x)))"))
        (check "(define (f x)\n  (h (g x)))\n(define y 1)\n" (tool-ref (tool-read p "a.scm") 'text))
    "#,
    );
    assert!(!dir.path().join("new.sld").exists());
}

#[test]
fn file_operations_require_the_files_capability() {
    let mut vm = Vm::with_grants(Grants::NONE);
    techne_node::install(&mut vm).unwrap();
    let error = vm.eval_source("(%repository-canonical-root \"/\")").unwrap_err();
    assert!(error.msg.contains("not granted"), "{error}");
}
