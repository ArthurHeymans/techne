//! Starting worlds from a template: a world booted once (builtins and the
//! prelude), whose state each new world copies instead of rebuilding.
//!
//! Booting reads and compiles the prelude and installs every builtin, about
//! 2 ms; what that leaves behind is small (a few hundred heap objects, a
//! few hundred codes, the tables naming them), so copying it is far
//! cheaper. `Vm::new` and `Vm::with_grants` start worlds this way from a
//! template per thread and grants, booted when a second world is asked
//! for (the first is booted itself); `TECHNE_TEMPLATE=0` boots every world
//! instead.
//!
//! A started world shares nothing mutable with its template or its
//! siblings: heap objects are copied (keeping their identity hashes),
//! tables cloned, runtime state (scheduler, registers, JIT, account) made
//! fresh. What it shares is immutable: codes' instructions (until compiled
//! code patches them, `Code::ops`), strings behind `Rc`s (names, source
//! text, docstrings), macros, and the builtins' implementations. It keeps
//! its template alive (`Vm::origin`), because symbols are owned by the
//! worlds that interned them and the template interned the prelude's.

use std::{cell::RefCell, rc::Rc};

use super::{Code, Grants, Vm};
use crate::{heap::Copier, value::Value};

thread_local! {
    /// By grants: `None` once a world was booted on this thread, the
    /// template once a second was asked for.
    static TEMPLATES: RefCell<Vec<(Grants, Option<Rc<Vm>>)>> = const { RefCell::new(Vec::new()) };
}

/// Whether `Vm::with_grants` starts worlds from templates.
pub(super) fn enabled() -> bool {
    std::env::var_os("TECHNE_TEMPLATE").is_none_or(|v| v != "0")
}

/// `TECHNE_TEMPLATE=1`: copy every world, the first too (for testing).
fn always() -> bool {
    std::env::var_os("TECHNE_TEMPLATE").is_some_and(|v| v == "1")
}

/// A world granted `grants`. The first on a thread is booted: a program
/// running one world pays nothing for templates. The second boots this
/// thread's template for them, which it and every later one copies.
pub(super) fn start(grants: Grants) -> Vm {
    let seen = TEMPLATES.with(|t| t.borrow().iter().find(|(g, _)| *g == grants).map(|(_, vm)| vm.clone()));
    let template = match seen {
        None if !always() => {
            TEMPLATES.with(|t| t.borrow_mut().push((grants, None)));
            return Vm::boot(grants);
        }
        Some(Some(template)) => template,
        _ => {
            let template = Rc::new(Vm::freeze(Vm::boot(grants)));
            TEMPLATES.with(|t| {
                let mut t = t.borrow_mut();
                t.retain(|(g, _)| *g != grants);
                t.push((grants, Some(template.clone())));
            });
            template
        }
    };
    Vm::instantiate(&template)
}

impl Vm {
    /// Make `vm` a template: it must be at rest (nothing running, nothing
    /// rooted from Rust, no foreign objects but the standard ports), since
    /// none of that state is carried over.
    pub(crate) fn freeze(mut vm: Vm) -> Vm {
        vm.collect();
        // It runs nothing more: its compiler thread would only idle for
        // the life of the thread holding it.
        for (_, _, cancelled) in vm.jit_jobs.drain(..) {
            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        vm.jit = None;
        // Its codes' instructions and names are every world's.
        vm.codes.iter_mut().flatten().for_each(|code| code.share());
        assert!(vm.frames.is_empty() && vm.handlers.is_empty() && vm.stack_top == 0, "a template runs nothing");
        assert!(vm.live == 0 && vm.run_queue.is_empty() && vm.executions.is_empty(), "a template has no tasks");
        assert!(vm.roots.borrow().iter().all(|r| r.strong_count() == 0) && vm.locals.is_empty(), "a template has no roots");
        assert!(
            vm.foreign.iter().flatten().all(|(f, _)| crate::ports::standard_port_for_world(f).is_some()),
            "a template holds no foreign objects but its standard ports"
        );
        assert!(vm.staging.is_none() && vm.staged.is_none() && vm.compiling.is_empty(), "a template loads nothing");
        // What a world starts afresh must be empty in the template too:
        // otherwise it would be state the world silently loses.
        assert!(
            vm.channels.is_empty()
                && vm.free_channels.is_empty()
                && vm.children.is_empty()
                && vm.killed.is_empty()
                && vm.timers.is_empty()
                && vm.woken.is_empty()
                && vm.masks.is_empty()
                && vm.active.is_empty()
                && vm.scratch.is_empty()
                && vm.jit_unwind.is_empty()
                && vm.current_task.is_none(),
            "a template is at rest"
        );
        vm
    }

    /// A new world with `template`'s state.
    pub(crate) fn instantiate(template: &Rc<Vm>) -> Vm {
        let account = crate::alloc::Account::new().expect("an account for the world");
        let _charged = account.enter();
        let Vm {
            // Copied below, object by object, from the values that reach them.
            heap: _,
            // Runtime state, fresh in every world.
            account: _,
            entered: _,
            entered_gen: _,
            memory_limit: _,
            pressure: _,
            regs: _,
            frames: _,
            handlers: _,
            moves: _,
            staging: _,
            staged: _,
            requiring: _,
            code_ptrs: _,
            code_table_base: _,
            compiling: _,
            dead_codes: _,
            jit: _,
            jit_threshold: _,
            jit_jobs: _,
            jit_job_ids: _,
            jit_callers: _,
            young_consts: _,
            jit_error: _,
            poisoned: _,
            jit_unwind: _,
            raise_trace: _,
            interrupt: _,
            stops: _,
            executions: _,
            active: _,
            thread: _,
            wake_notifier: _,
            stack_top: _,
            scratch: _,
            roots: _,
            world: _,
            // Foreign objects: the standard ports, fresh for each world.
            foreign: _,
            foreign_free: _,
            tasks: _,
            free_tasks: _,
            live: _,
            run_queue: _,
            wakes: _,
            woken: _,
            timers: _,
            spare_suspend: _,
            channels: _,
            children: _,
            killed: _,
            // The scopes: the root scope the prelude makes.
            scopes,
            free_scopes,
            free_channels: _,
            offers: _,
            current_task: _,
            masks: _,
            locals: _,
            out: _,
            origin: _,
            _symbols: _,
            // The template's state.
            globals,
            global_rooted,
            free_globals,
            global_names,
            global_module,
            user_defined,
            bindings,
            record_types,
            modules,
            free_modules,
            retired,
            dead_modules,
            pinned_modules,
            generations,
            module_paths,
            current_module,
            grants,
            files,
            file_codes,
            free_files,
            pinned_files,
            variable_docs,
            codes,
            free_codes,
            natives,
            apply_native,
            jit_inline,
            jit_fixnum,
            specials,
            foreign_type_names,
            next_id,
        } = &**template;

        let mut vm = Vm::empty(*grants, account);
        let mut copier = Copier::new();
        let mut copy = |vm: &mut Vm, v: Value| copier.value(&mut vm.heap, v);
        for &v in globals.iter() {
            let v = copy(&mut vm, v);
            vm.globals.push(v, &mut vm.moves);
        }
        vm.specials = specials.map(|v| copy(&mut vm, v));
        vm.apply_native = copy(&mut vm, *apply_native);
        vm.jit_inline = jit_inline.iter().map(|&(v, inline)| (copy(&mut vm, v), inline)).collect();
        vm.jit_fixnum = jit_fixnum.iter().map(|&v| copy(&mut vm, v)).collect();
        vm.codes = codes
            .iter()
            .map(|c| {
                c.as_deref().map(|c| {
                    let mut code = c.instance_copy();
                    code.handle.set(copy(&mut vm, code.handle.get()));
                    code.consts.iter_mut().for_each(|v| *v = copy(&mut vm, *v));
                    Box::new(code)
                })
            })
            .collect();
        vm.scopes = scopes.iter().map(|s| s.as_ref().map(|s| s.copy_for_world(&mut |v| copy(&mut vm, v)))).collect();
        copier.finish(&mut vm.heap);
        for (obj, index) in copier.foreign() {
            let (port, type_name) = template.foreign[index].as_ref().expect("a live foreign object");
            let port = crate::ports::standard_port_for_world(port).expect("a standard port");
            vm.foreign.push(Some((port, type_name)));
            // SAFETY: a foreign object just copied: header, then its index.
            unsafe { *obj.add(1) = (vm.foreign.len() - 1) as u64 };
            vm.heap.register_foreign(obj);
        }
        vm.code_ptrs = vm.codes.iter().map(|c| c.as_deref().map_or(std::ptr::null(), |c| c as *const Code)).collect();
        vm.code_table_base = vm.code_ptrs.as_ptr();
        vm.heap.code_handles = vm.codes.iter().map(|c| c.as_ref().map_or(Value::VOID, |c| c.handle.get())).collect();

        vm.global_rooted.clone_from(global_rooted);
        vm.free_globals.clone_from(free_globals);
        vm.global_names.clone_from(global_names);
        vm.global_module.clone_from(global_module);
        vm.user_defined.clone_from(user_defined);
        vm.bindings.clone_from(bindings);
        vm.record_types.clone_from(record_types);
        vm.modules.clone_from(modules);
        vm.free_modules.clone_from(free_modules);
        vm.retired.clone_from(retired);
        vm.dead_modules.clone_from(dead_modules);
        vm.pinned_modules.clone_from(pinned_modules);
        vm.generations = *generations;
        vm.module_paths.clone_from(module_paths);
        vm.current_module = *current_module;
        vm.files.clone_from(files);
        vm.file_codes.clone_from(file_codes);
        vm.free_files.clone_from(free_files);
        vm.pinned_files.clone_from(pinned_files);
        vm.variable_docs.clone_from(variable_docs);
        vm.free_codes.clone_from(free_codes);
        vm.natives.clone_from(natives);
        vm.foreign_type_names.clone_from(foreign_type_names);
        vm.next_id = *next_id;
        vm.free_scopes.clone_from(free_scopes);
        vm.origin = Some(template.clone());
        vm.set_jit(super::jit_threshold_from_env());
        vm
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::ROOT_MODULE;

    fn booted() -> Rc<Vm> {
        Rc::new(Vm::freeze(Vm::boot(Grants::ALL)))
    }

    fn eval(vm: &mut Vm, src: &str) -> String {
        crate::builtins::repr(vm.eval_source_raw(src).unwrap())
    }

    /// A world refers to nothing in its template's heap: with the
    /// template's objects overwritten, it still runs (a missed reference
    /// would read garbage).
    #[test]
    fn a_world_refers_to_nothing_in_its_template_heap() {
        let t = booted();
        let mut a = Vm::instantiate(&t);
        // SAFETY (test only): nothing else uses the template meanwhile; it
        // is never run again.
        unsafe { (*Rc::as_ptr(&t).cast_mut()).heap.scribble() };
        let programs = [
            ("(map (lambda (x) (* x x)) '(1 2 3))", "(1 4 9)"),
            ("(let-values (((q r) (floor/ 7 2))) (list q r))", "(3 1)"),
            ("(guard (e (#t (condition/report-string e))) (error \"boom\" 1 2))", "\"boom 1 2\""),
            ("(guard (e ((error-object? e) (error-object-message e))) (raise (make-condition-type 'x '() '())))", ""),
            ("(call-with-output-string (lambda (p) (write '(a \"b\" #\\c) p)))", "\"(a \\\"b\\\" #\\\\c)\""),
            ("(let ((p (make-parameter 1))) (parameterize ((p 2)) (p)))", "2"),
            ("(with-output-to-string (lambda () (display \"hi\")))", "\"hi\""),
            ("(vector-map + #(1 2) #(10 20))", "#(11 22)"),
            ("(string-append \"a\" (number->string 42))", "\"a42\""),
            ("(let ((h (make-hash-table))) (hash-table-set! h car 2) (hash-table-ref/default h car 0))", "2"),
            ("(apply + 1 2 '(3 4))", "10"),
            ("(call/cc (lambda (k) (dynamic-wind (lambda () #f) (lambda () (k 'out)) (lambda () #f))))", "out"),
        ];
        for (src, want) in programs {
            match a.eval_source(src) {
                Ok(v) => {
                    let got = a.repr(&v);
                    assert!(want.is_empty() || got == want, "{src}: {got}, not {want}");
                }
                Err(e) => assert!(want.is_empty(), "{src}: {}", e.msg),
            }
            a.full_collect();
        }
    }

    #[test]
    fn worlds_from_a_template_diverge() {
        let t = booted();
        let mut a = Vm::instantiate(&t);
        let mut b = Vm::instantiate(&t);
        eval(&mut a, "(define (car x) 'mine)");
        eval(&mut a, "(define xs (list 1 2 3))");
        assert_eq!(eval(&mut a, "(car xs)"), "mine");
        assert_eq!(eval(&mut b, "(car (list 1 2 3))"), "1");
        assert!(b.eval_source("xs").is_err());
    }

    #[test]
    fn a_world_outlives_its_template_handle_and_siblings() {
        let mut a = {
            let t = booted();
            let mut sibling = Vm::instantiate(&t);
            eval(&mut sibling, "(define v (make-vector 3 'x))");
            Vm::instantiate(&t)
        };
        a.full_collect();
        assert_eq!(eval(&mut a, "(map (lambda (x) (* x x)) '(1 2 3))"), "(1 4 9)");
        assert_eq!(eval(&mut a, "(symbol->string 'lambda)"), "\"lambda\"");
    }

    /// The copied heap is the world's own: collections and the template's
    /// literals (immutable) behave as in a booted world.
    #[test]
    fn copied_objects_collect_and_keep_identity() {
        let t = booted();
        let mut a = Vm::instantiate(&t);
        // A record type the prelude made, used across collections.
        let src = "(define-record-type point (make-point x y) point? (x point-x)) \
                   (define p (make-point 1 2)) \
                   (define h (make-eq-hashtable)) (hashtable-set! h car 'car) (hashtable-set! h p 'p)";
        if a.eval_source(src).is_ok() {
            a.full_collect();
            assert_eq!(eval(&mut a, "(list (hashtable-ref h car #f) (hashtable-ref h p #f))"), "(car p)");
        }
        a.full_collect();
        a.full_collect();
        assert_eq!(eval(&mut a, "(let loop ((i 0) (acc '())) (if (= i 1000) (length acc) (loop (+ i 1) (cons i acc))))"), "1000");
        assert!(a.get_global_in(ROOT_MODULE, "map").is_some());
    }
}
