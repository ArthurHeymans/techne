//! Language step 7: code, globals, source text and JIT code of what is no
//! longer reachable are freed; what is retained stays safe.

use techne_vm::vm::Vm;

fn eval(vm: &mut Vm, src: &str) -> String {
    let v = vm.eval_source(src).unwrap_or_else(|e| panic!("{src}: {e}"));
    techne_vm::builtins::repr(v)
}

#[test]
fn redefinitions_free_the_code_they_replace() {
    let mut vm = Vm::new();
    let mut live = Vec::new();
    for i in 0..1000 {
        eval(&mut vm, &format!("(define (f x) (let ((g (lambda (y) (+ x y {i})))) (g \"a string {i}\")))"));
        if i % 100 == 99 {
            vm.full_collect();
            live.push((vm.live_codes(), vm.live_files()));
        }
    }
    assert!(live.windows(2).all(|w| w[1] == w[0]), "codes and files: {live:?}");
}

/// What a VM holds that reclamation must bound: codes, globals, modules,
/// source files, JIT arenas and old-generation words.
fn sizes(vm: &mut Vm) -> [usize; 6] {
    vm.full_collect();
    // Arenas are freed on the compiler thread, after the releases sent now.
    std::thread::sleep(std::time::Duration::from_millis(20));
    [vm.live_codes(), vm.live_globals(), vm.live_modules(), vm.live_files(), vm.live_jit_arenas(), vm.heap.old_words()]
}

const PACKAGE: &str = r#"
(define greeting "hello from the package")
(define (helper x) (* 2 x))
(define (work n)
  (let loop ((i 0) (acc 0))
    (if (= i n) acc (loop (+ i 1) (+ acc (helper i))))))
(define (make-counter)
  (let ((n 0))
    (lambda () (set! n (+ n 1)) (list n (helper n) greeting))))
"#;

#[test]
fn load_use_unload_cycles_plateau_and_retained_closures_stay_safe() {
    cycles(Vm::new());
}

#[test]
fn cycles_with_everything_compiled() {
    let mut vm = Vm::new();
    vm.set_jit(Some(1));
    cycles(vm);
}

fn cycles(mut vm: Vm) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pkg.scm");
    std::fs::write(&path, PACKAGE).unwrap();
    let load = format!("(load-package 'pkg {:?})", path.display().to_string());
    let module = "(package-module (find-package 'pkg))";
    eval(&mut vm, &load);
    // A closure of the first generation, kept past every unload.
    eval(&mut vm, &format!("(define kept (eval '(make-counter) {module}))"));
    assert_eq!(eval(&mut vm, "(kept)"), r#"(1 2 "hello from the package")"#);
    eval(&mut vm, "(unload-package 'pkg)");
    let mut samples = Vec::new();
    for i in 1..=2000 {
        eval(&mut vm, &load);
        assert_eq!(eval(&mut vm, &format!("(eval '(work 2000) {module})")), "3998000");
        eval(&mut vm, "(unload-package 'pkg)");
        if i % 250 == 0 {
            samples.push(sizes(&mut vm));
        }
    }
    assert_eq!(eval(&mut vm, "(kept)"), r#"(2 4 "hello from the package")"#);
    let first = samples[0];
    assert!(samples.iter().all(|s| s[..4] == first[..4]), "codes, globals, modules, files: {samples:?}");
    // JIT arenas and the heap level off: no more in the second half than in
    // the first, give or take the arenas of compilations in flight (leaking
    // them would add one per 64 functions, some 30 here). The heap may grow
    // a table early on.
    let level = |i: usize, slack: usize| {
        let (early, late) = samples.split_at(samples.len() / 2);
        late.iter().map(|s| s[i]).max().unwrap() <= early.iter().map(|s| s[i]).max().unwrap() + slack
    };
    assert!(level(4, 2) && level(5, 0), "JIT arenas and old words: {samples:?}");
    // Once the closure goes, so does the first generation.
    eval(&mut vm, "(set! kept #f)");
    let after = sizes(&mut vm);
    assert!(after[0] < first[0] && after[1] < first[1], "{after:?} after dropping the closure, {first:?} before");
}

#[test]
fn why_a_retired_generation_is_retained() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pkg.scm");
    std::fs::write(&path, PACKAGE).unwrap();
    let mut vm = Vm::new();
    eval(&mut vm, &format!("(load-package 'pkg {:?})", path.display().to_string()));
    // A weak table holds the package's helper without keeping it.
    eval(
        &mut vm,
        "(define m (package-module (find-package 'pkg)))
         (define kept (eval '(make-counter) m))
         (define weak (make-weak-hash-table))
         (hash-table-set! weak (eval 'helper m) #t)
         (unload-package 'pkg)
         (collect-garbage 'full)
         (define (helper) (car (hash-table-keys weak)))",
    );
    let why = eval(&mut vm, "(why-retained (helper))");
    for step in ["the global kept of user", "the code of lambda", "global helper of", "(retired), which lambda uses"] {
        assert!(why.contains(step), "{step} in {why}");
    }
    eval(&mut vm, "(set! kept #f)");
    let why = eval(&mut vm, "(why-retained (helper))");
    assert!(!why.contains("kept"), "only what asking used is left: {why}");
    assert_eq!(eval(&mut vm, "(collect-garbage 'full) (hash-table-count weak)"), "0");
}

/// A constant in the nursery when its code is compiled (here the closure
/// an inlined call is guarded by) is kept current as the collector moves
/// it: the guard does not match whatever comes to live at its old place.
#[test]
fn constants_move_with_their_objects() {
    for jit in [None, Some(1)] {
        let mut vm = Vm::new();
        vm.set_jit(jit);
        vm.eval_in(techne_vm::vm::ROOT_MODULE, "root.scm", "(define (rootmap f x) (f x))").unwrap();
        eval(&mut vm, "(define (caller) (rootmap (lambda (y) 42) 0))");
        assert_eq!(eval(&mut vm, "(caller)"), "42");
        vm.collect();
        vm.eval_in(techne_vm::vm::ROOT_MODULE, "root.scm", "(define (rootmap f x) 999)").unwrap();
        assert_eq!(eval(&mut vm, "(caller)"), "999", "jit {jit:?}");
    }
}

/// A macro imported from a retired package keeps that package's bindings,
/// which its expansions refer to.
#[test]
fn an_imported_macro_keeps_its_module() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("macros.scm");
    std::fs::write(&path, "(define secret 42) (define-syntax reveal (syntax-rules () ((_) secret))) (provide reveal)").unwrap();
    let mut vm = Vm::new();
    eval(&mut vm, &format!("(load-package 'macros {:?})", path.display().to_string()));
    eval(&mut vm, &format!("(require {:?})", path.display().to_string()));
    eval(&mut vm, "(unload-package 'macros)");
    vm.full_collect();
    // Other modules may take the freed slots.
    eval(&mut vm, "(define secret 0)");
    assert_eq!(eval(&mut vm, "(reveal)"), "42");
}

/// Where a macro is defined is kept by value: its file's slot may go to
/// another file.
#[test]
fn a_macro_remembers_its_file() {
    let mut vm = Vm::new();
    vm.eval_in(techne_vm::vm::USER_MODULE, "macro-definition.scm", "\n(define-syntax twice (syntax-rules () ((_ e) (begin e e))))")
        .unwrap();
    vm.full_collect();
    vm.eval_in(techne_vm::vm::USER_MODULE, "unrelated.scm", "(define x 1)").unwrap();
    let d = vm.describe_name(techne_vm::vm::USER_MODULE, techne_vm::reader::intern("twice"));
    assert_eq!((d.file.as_deref(), d.line), (Some("macro-definition.scm"), 2));
}

/// A macro that a package's macro defined elsewhere keeps the package's
/// bindings its rules refer to, also once the package is reloaded.
#[test]
fn a_macro_made_by_a_macro_keeps_its_modules() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("maker.scm");
    std::fs::write(
        &path,
        "(define secret 42)
         (define-syntax define-revealer
           (syntax-rules () ((_ name) (define-syntax name (syntax-rules () ((_) (list secret)))))))
         (provide define-revealer)",
    )
    .unwrap();
    let mut vm = Vm::new();
    let (load, require) =
        (format!("(load-package 'maker {:?})", path.display().to_string()), format!("(require {:?})", path.display().to_string()));
    eval(&mut vm, &load);
    eval(&mut vm, &require);
    eval(&mut vm, "(define-revealer reveal)");
    assert_eq!(eval(&mut vm, "(reveal)"), "(42)");
    eval(&mut vm, &load);
    eval(&mut vm, &require);
    vm.full_collect();
    eval(&mut vm, "(define secret 0)");
    assert_eq!(eval(&mut vm, "(reveal)"), "(42)");
}
