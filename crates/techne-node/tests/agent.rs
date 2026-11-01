//! The public Scheme API, including bounded output, stale edits and live jobs.
use std::{fs, path::Path};
use techne_vm::vm::{Grants, Vm};

fn world(root: &Path) -> Vm {
    let mut vm = Vm::new();
    techne_node::install(&mut vm).unwrap();
    let library = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lisp/techne/agent.sld");
    vm.eval_source(&format!(
        "(include {:?}) (import (techne agent)) (define p (agent-project {:?}))
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
        (define d (agent-discover "newly-defined" #:limit 1))
        (check 'newly-defined (agent-ref (car (agent-ref d 'items)) 'name))
        (check 1 (length (agent-ref d 'items)))
        (check #t (string? (agent-ref (car (agent-ref d 'items)) 'signature)))
        (define desc (agent-describe 'newly-defined #:chars 8))
        (check "Return X" (agent-ref desc 'doc))
        (check #t (agent-ref desc 'truncated))
        (check 8 (agent-ref desc 'next))
        (check #t (fails (lambda () (agent-discover "read" #:limit 0))))
        (check #t (fails (lambda () (agent-discover "read" #:offset -1))))
        (check #t (fails (lambda () (agent-describe 'no-such-binding))))
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
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (define files (agent-files p #:limit 1))
        (check 3 (agent-ref files 'total))
        (check 1 (length (agent-ref files 'items)))
        (check 1 (agent-ref files 'next))
        (check #t (agent-ref files 'truncated))
        (define found (agent-search p "needle" #:limit 1))
        (check 3 (agent-ref found 'total))
        (check 1 (length (agent-ref found 'items)))
        (check #t (agent-ref found (quote complete)))
        (check 0 (agent-ref found (quote skipped)))
        (check 1 (agent-ref found (quote binary)))
        (check 0 (length (agent-ref found (quote problems))))
        (define line (agent-search p "needle" #:prefix "two"))
        (check #t (agent-ref line 'complete))
        (define item (car (agent-ref line 'items)))
        (check #t (and (string-contains (agent-ref item 'text) "needle") #t))
        (check #t (<= (string-length (agent-ref item 'text)) 240))
        (check 1 (agent-ref item 'line))
        (check #t (agent-ref (agent-expand p (agent-ref item 'handle) #:chars 10) 'truncated))
        (check #t (fails (lambda () (agent-search p ""))))
        (check #t (fails (lambda () (agent-files p #:limit 51))))
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
        (define found (agent-search p "needle" #:limit 1))
        (check 1 (agent-ref found (quote total)))
        (check "needle" (agent-ref (agent-expand p (agent-ref (car (agent-ref found (quote items))) (quote handle))) (quote text)))
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
        (define r (agent-read p "source" #:lines 1 #:chars 2))
        (define h (agent-ref r 'handle))
        (check "aé" (agent-ref r 'text))
        (check 2 (agent-ref r 'next))
        (check "€😀\n" (agent-ref (agent-expand p h #:offset 2) 'text))
        (check #f (agent-ref (agent-expand p h #:offset 2) 'truncated))
        (check "next\n" (agent-ref (agent-read p "source" #:from 2 #:lines 1) 'text))
        (check #t (fails (lambda () (agent-read p "source" #:from 99))))
        (check #t (fails (lambda () (agent-expand p h #:offset 99))))
        (check #t (fails (lambda () (agent-expand (agent-project (agent-root p)) h))))
    "#,
    );
    fs::write(dir.path().join("source"), "changed").unwrap();
    check(
        &mut vm,
        r#"
        (check "aé€😀\n" (agent-ref (agent-expand p h) 'text))
        (check "changed" (agent-ref (agent-read p "source") 'text))
        (check #t (agent-refresh! p))
        (check #t (fails (lambda () (agent-expand p h))))
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
        (check 1 (agent-ref (agent-symbols p "source.scm") 'total))
        (check "(define (real x) (+ x 1))" (agent-ref (agent-read-symbol p "source.scm" "real") 'text))
        (check 3 (agent-ref (agent-symbols p "source.rs") 'total))
        (check 2 (agent-ref (agent-symbols p "source.rs" #:name "real") 'total))
        (check #t (fails (lambda () (agent-read-symbol p "source.rs" "real"))))
        (check #t (fails (lambda () (agent-read-symbol p "source.rs" "fake"))))
        (check #t (fails (lambda () (agent-symbols p "other.txt"))))
        (check "struct Foo {}" (agent-ref (agent-read-symbol p "source.rs" "Foo") 'text))
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
        (define before (agent-read p "source"))
        (define rev (agent-ref before 'revision))
        (check #t (fails (lambda () (agent-edit! p "source" rev '(("one" "ONE") ("absent" "no"))))))
        (check "one é\ntwo\nthree\n" (agent-ref (agent-read p "source") 'text))
        (check #t (fails (lambda () (agent-edit! p "source" rev '(("one é" "x") ("é" "y"))))))
        (define after (agent-edit! p "source" rev '(("one é" "ONE 😀") ("two" "TWO"))))
        (check 2 (length (agent-ref after 'changes)))
        (check "ONE 😀\nTWO\nthree\n" (agent-ref (agent-read p "source") 'text))
        (check "one é\ntwo\nthree\n" (agent-ref (agent-expand p (agent-ref before 'handle)) 'text))
        (check #t (fails (lambda () (agent-edit! p "source" rev '(("three" "THREE"))))))
    "#,
    );
    fs::write(&path, "aaaa").unwrap();
    check(
        &mut vm,
        r#"
        (define current (agent-read p "source"))
        (check #t (fails (lambda () (agent-edit! p "source" (agent-ref current 'revision) '(("aaa" "x"))))))
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
        (define old (agent-read p "source"))
        (check #t (fails (lambda () (agent-read p "escape"))))
    "#,
    );
    fs::write(dir.path().join("source"), "external").unwrap();
    check(
        &mut vm,
        r#"
        (check #t (fails (lambda () (agent-edit! p "source" (agent-ref old 'revision) '(("first" "lost"))))))
        (check "external" (agent-ref (agent-read p "source") 'text))
    "#,
    );
    for i in 0..129 {
        fs::write(dir.path().join("source"), format!("revision {i}")).unwrap();
        check(&mut vm, "(agent-read p \"source\" #:chars 1)");
    }
    check(&mut vm, "(check #t (fails (lambda () (agent-expand p (agent-ref old 'handle)))))");
    fs::write(dir.path().join("large"), vec![b'x'; (1 << 20) + 1]).unwrap();
    check(&mut vm, "(check #t (fails (lambda () (agent-read p \"large\"))))");
}

#[test]
fn process_streams_drain_concurrently_and_report_loss() {
    let dir = tempfile::tempdir().unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (define job (agent-run p "sh" '("-c" "i=0; while [ $i -lt 12000 ]; do printf abcdefghijklmnopqrstuvwxyz; printf ABCDEFGHIJKLMNOPQRSTUVWXYZ >&2; i=$((i+1)); done; exit 7") #:retain 100))
        (define result (agent-process-result job #:chars 20))
        (check 7 (agent-ref result 'status))
        (check #t (agent-process-done? job))
        (check #t (agent-ref (agent-ref result 'stdout) 'truncated))
        (check #t (agent-ref (agent-ref result 'stderr) 'truncated))
        (check 312000 (agent-ref (agent-ref result 'stdout) 'characters))
        (check 312000 (agent-ref (agent-ref result 'stderr) 'characters))
        (check 20 (string-length (agent-ref (agent-ref result 'stdout) 'text)))
        (check result (agent-process-result job #:chars 20))
        (check #t (fails (lambda () (agent-run p "sh" '() #:retain 0))))
        (define small (agent-process-result (agent-run p "sh" (quote ("-c" "printf abcdef")) #:retain 3) #:chars 20))
        (check "def" (agent-ref (agent-ref small (quote stdout)) (quote text)))
        (check #t (agent-ref (agent-ref small (quote stdout)) (quote truncated)))
        (define sleeper (agent-run p "sh" '("-c" "sleep 300")))
        (define cancelled (agent-process-cancel! sleeper))
        (check #t (agent-process-done? sleeper))
        (check #t (pair? (agent-ref cancelled 'status)))
    "#,
    );
}

#[test]
fn file_operations_require_the_files_capability() {
    let mut vm = Vm::with_grants(Grants::NONE);
    techne_node::install(&mut vm).unwrap();
    let error = vm.eval_source("(%agent-open \"/\")").unwrap_err();
    assert!(error.msg.contains("not granted"), "{error}");
}
