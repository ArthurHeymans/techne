//! Differential fuzzing: random programs must behave the same under the
//! interpreter and the JIT (compiling synchronously, in the background, and
//! under GC stress).
//!
//! `TECHNE_FUZZ_SEEDS` (default 40) and `TECHNE_FUZZ_START` (default 0) pick
//! the seeds. A mismatch leaves the program in the temp directory and names
//! the seed, so `TECHNE_FUZZ_START=<seed> TECHNE_FUZZ_SEEDS=1` reproduces it.
//!
//! Programs are well-typed by construction, mostly, and always terminate:
//! loops count to small constants, recursion goes through a depth argument
//! that decreases, and functions only call earlier ones. Some expressions are
//! deliberately ill-typed or overflow, to exercise error paths; every test
//! call is wrapped in a `guard` that prints the condition.

use std::{
    fmt::Write as _,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Ty {
    Int,
    Num,
    Bool,
    List,
    Vec,
    Str,
}

const TYPES: [Ty; 6] = [Ty::Int, Ty::Num, Ty::Bool, Ty::List, Ty::Vec, Ty::Str];

struct Fun {
    name: String,
    params: Vec<Ty>,
    ret: Ty,
}

/// Variables in scope with their types; `mutable` ones may be `set!`.
#[derive(Clone, Default)]
struct Env {
    vars: Vec<(String, Ty)>,
}

impl Env {
    fn with(&self, name: &str, ty: Ty) -> Env {
        let mut e = self.clone();
        e.vars.push((name.to_string(), ty));
        e
    }
    fn of(&self, ty: Ty) -> Vec<&str> {
        self.vars.iter().filter(|(_, t)| *t == ty).map(|(n, _)| n.as_str()).collect()
    }
}

/// Loop counters (`i…`) and the recursion depth are never assigned, so
/// loops and recursion still terminate.
fn assignable(var: &str) -> bool {
    var != "depth" && !var.starts_with('i')
}

struct Gen {
    rng: Rng,
    funs: Vec<Fun>,
    /// Functions up to this index may be called.
    current: Option<usize>,
    /// The function being generated, which calls itself with a smaller depth.
    this: Option<usize>,
    fresh: usize,
}

impl Gen {
    fn name(&mut self, prefix: &str) -> String {
        self.fresh += 1;
        format!("{prefix}{}", self.fresh)
    }

    fn expr(&mut self, ty: Ty, env: &Env, depth: usize) -> String {
        // Occasionally a variable of another type: type errors.
        if self.rng.chance(2) && !env.vars.is_empty() {
            return self.rng.pick(&env.vars).0.clone();
        }
        let vars = env.of(ty);
        if depth == 0 || self.rng.chance(20) {
            if !vars.is_empty() && self.rng.chance(70) {
                return self.rng.pick(&vars).to_string();
            }
            return self.literal(ty);
        }
        let d = depth - 1;
        // Forms that work for any type.
        match self.rng.below(14) {
            0 => {
                return format!("(if {} {} {})", self.expr(Ty::Bool, env, d), self.expr(ty, env, d), self.expr(ty, env, d));
            }
            1 => {
                let x = self.name("x");
                let t = *self.rng.pick(&TYPES);
                let init = self.expr(t, env, d);
                return format!("(let (({x} {init})) {})", self.expr(ty, &env.with(&x, t), d));
            }
            2 => {
                // A counted loop accumulating a value of this type.
                let (i, acc) = (self.name("i"), self.name("acc"));
                let k = self.rng.below(4) + 1;
                let init = self.expr(ty, env, d);
                let inner = env.with(&i, Ty::Int).with(&acc, ty);
                let step = self.expr(ty, &inner, d);
                return format!(
                    "(let {} (({i} 0) ({acc} {init})) (if (< {i} {k}) ({} (+ {i} 1) {step}) {acc}))",
                    self.loop_name(),
                    self.last_loop()
                );
            }
            3 => {
                // A closure, applied: parameters, captures.
                let x = self.name("x");
                let t = *self.rng.pick(&TYPES);
                let body = self.expr(ty, &env.with(&x, t), d);
                return format!("((lambda ({x}) {body}) {})", self.expr(t, env, d));
            }
            4 => {
                // Mutation of a captured variable through a closure.
                let (c, f) = (self.name("c"), self.name("f"));
                let init = self.expr(ty, env, d);
                let inner = env.with(&c, ty);
                let update = self.expr(ty, &inner, d);
                return format!("(let (({c} {init})) (let (({f} (lambda () (set! {c} {update})))) ({f}) ({f}) {c}))");
            }
            5 => {
                let body = self.expr(ty, env, d);
                let handler = self.expr(ty, env, d);
                let e = self.name("e");
                return format!("(guard ({e} (#t {handler})) {body})");
            }
            6 if self.rng.chance(30) => {
                return format!("(begin (raise {}) {})", self.expr(Ty::Int, env, d), self.literal(ty));
            }
            7 | 8 => {
                if let Some(call) = self.call(ty, env, d) {
                    return call;
                }
            }
            _ => {}
        }
        self.typed(ty, env, d, &vars)
    }

    fn loop_name(&mut self) -> String {
        self.fresh += 1;
        format!("loop{}", self.fresh)
    }
    fn last_loop(&self) -> String {
        format!("loop{}", self.fresh)
    }

    fn literal(&mut self, ty: Ty) -> String {
        match ty {
            Ty::Int => match self.rng.below(10) {
                0 => self.rng.pick(&["140737488355327", "-140737488355328", "2147483648", "-1"]).to_string(),
                _ => (self.rng.below(40) as i64 - 10).to_string(),
            },
            Ty::Num => self.rng.pick(&["0.5", "-2.25", "3.0", "1e10", "0.0", "7"]).to_string(),
            Ty::Bool => self.rng.pick(&["#t", "#f"]).to_string(),
            Ty::List => self.rng.pick(&["'()", "'(1 2 3)", "'(5)", "(list 4 -2 9 0)"]).to_string(),
            Ty::Vec => self.rng.pick(&["(vector 1 2 3)", "(make-vector 4 0)", "(vector)"]).to_string(),
            Ty::Str => self.rng.pick(&["\"ab\"", "\"\"", "\"xyz\""]).to_string(),
        }
    }

    fn typed(&mut self, ty: Ty, env: &Env, d: usize, vars: &[&str]) -> String {
        let e = |g: &mut Gen, t: Ty| g.expr(t, env, d);
        match ty {
            Ty::Int => match self.rng.below(15) {
                0 => format!("(+ {} {})", e(self, Ty::Int), e(self, Ty::Int)),
                1 => format!("(- {} {})", e(self, Ty::Int), e(self, Ty::Int)),
                2 => format!("(* {} {})", e(self, Ty::Int), e(self, Ty::Int)),
                3 => {
                    let op = *self.rng.pick(&["quotient", "remainder", "modulo"]);
                    let divisor = if self.rng.chance(80) { self.rng.pick(&["3", "-7", "2", "1"]).to_string() } else { e(self, Ty::Int) };
                    format!("({op} {} {divisor})", e(self, Ty::Int))
                }
                4 => format!("(length {})", e(self, Ty::List)),
                5 => format!("(vector-length {})", e(self, Ty::Vec)),
                6 => format!("(string-length {})", e(self, Ty::Str)),
                7 => format!("(car {})", e(self, Ty::List)),
                8 => {
                    let v = self.name("v");
                    let vec = e(self, Ty::Vec);
                    let idx = e(self, Ty::Int);
                    if self.rng.chance(70) {
                        format!("(let (({v} {vec})) (vector-ref {v} (modulo {idx} (max 1 (vector-length {v})))))")
                    } else {
                        format!("(vector-ref {vec} {idx})")
                    }
                }
                9 => format!("(apply + {})", e(self, Ty::List)),
                10 => format!("(+ {} {})", e(self, Ty::Int), self.rng.below(5)),
                11 if vars.iter().any(|v| assignable(v)) => {
                    // Assignment to a local in scope.
                    let candidates: Vec<&str> = vars.iter().copied().filter(|v| assignable(v)).collect();
                    let v = self.rng.pick(&candidates).to_string();
                    format!("(begin (set! {v} {}) {v})", e(self, Ty::Int))
                }
                12 => format!("(abs {})", e(self, Ty::Int)),
                13 if self.rng.chance(50) => {
                    // Products past 48 bits (boxed integers) and past 64 (errors).
                    let big = *self.rng.pick(&["12345678", "-98765432", "140737488355327", "3037000500", "65536"]);
                    format!("(* {big} {})", e(self, Ty::Int))
                }
                13 => {
                    // A variable assigned in a loop and read only by the
                    // handler of the error that ends the loop.
                    let (v, i, l, x) = (self.name("s"), self.name("i"), self.loop_name(), self.name("e"));
                    let init = e(self, Ty::Int);
                    let k = self.rng.below(4) + 1;
                    let inner = env.with(&i, Ty::Int);
                    let value = self.expr(Ty::Int, &inner, d);
                    format!(
                        "(let (({v} {init})) (guard ({x} (#t {v})) (let {l} (({i} 0)) (if (< {i} {k}) (begin (set! {v} {value}) ({l} (+ {i} 1))) (car '())))))"
                    )
                }
                _ => format!("(- {})", e(self, Ty::Int)),
            },
            Ty::Num => match self.rng.below(6) {
                0 => format!("(+ {} {})", e(self, Ty::Num), e(self, Ty::Num)),
                1 => format!("(* {} {})", e(self, Ty::Num), e(self, Ty::Int)),
                2 => format!("(- {} {})", e(self, Ty::Int), e(self, Ty::Num)),
                3 => format!("(exact->inexact {})", e(self, Ty::Int)),
                4 => format!("(/ {} 4.0)", e(self, Ty::Num)),
                _ => e(self, Ty::Int),
            },
            Ty::Bool => match self.rng.below(9) {
                0 => format!("(< {} {})", e(self, Ty::Int), e(self, Ty::Int)),
                1 => format!("(<= {} {})", e(self, Ty::Num), e(self, Ty::Int)),
                2 => format!("(= {} {})", e(self, Ty::Num), e(self, Ty::Num)),
                3 => format!("(null? {})", e(self, Ty::List)),
                4 => format!("(pair? {})", e(self, Ty::List)),
                5 => format!("(not {})", e(self, Ty::Bool)),
                6 => format!("(and {} {})", e(self, Ty::Bool), e(self, Ty::Bool)),
                7 => format!("(eq? {} {})", e(self, Ty::Int), e(self, Ty::Int)),
                _ => format!("(> {} {})", e(self, Ty::Int), self.rng.below(10)),
            },
            Ty::List => match self.rng.below(9) {
                0 => format!("(cons {} {})", e(self, Ty::Int), e(self, Ty::List)),
                1 => format!("(cdr {})", e(self, Ty::List)),
                2 => format!("(reverse {})", e(self, Ty::List)),
                3 => {
                    let x = self.name("x");
                    format!("(map (lambda ({x}) {}) {})", self.expr(Ty::Int, &env.with(&x, Ty::Int), d), e(self, Ty::List))
                }
                4 => {
                    let x = self.name("x");
                    format!("(filter (lambda ({x}) {}) {})", self.expr(Ty::Bool, &env.with(&x, Ty::Int), d), e(self, Ty::List))
                }
                5 => format!("(list {} {})", e(self, Ty::Int), e(self, Ty::Int)),
                6 => format!("(append {} {})", e(self, Ty::List), e(self, Ty::List)),
                7 => format!("(iota {})", self.rng.below(6)),
                _ => format!("(vector->list {})", e(self, Ty::Vec)),
            },
            Ty::Vec => match self.rng.below(3) {
                0 => format!("(make-vector {} {})", self.rng.below(5), e(self, Ty::Int)),
                1 => {
                    let v = self.name("v");
                    let x = e(self, Ty::Int);
                    format!("(let (({v} (make-vector 3 0))) (vector-set! {v} 1 {x}) {v})")
                }
                _ => format!("(list->vector {})", e(self, Ty::List)),
            },
            Ty::Str => match self.rng.below(3) {
                0 => format!("(number->string {})", e(self, Ty::Int)),
                1 => format!("(string-append {} {})", e(self, Ty::Str), e(self, Ty::Str)),
                _ => format!("(substring \"abcdef\" 0 {})", self.rng.below(7)),
            },
        }
    }

    /// A call returning `ty`: an earlier function, or this one with a
    /// smaller depth.
    fn call(&mut self, ty: Ty, env: &Env, d: usize) -> Option<String> {
        let current = self.current?;
        let candidates: Vec<usize> = (0..=current).filter(|&i| self.funs[i].ret == ty).collect();
        if candidates.is_empty() {
            return None;
        }
        let f = *self.rng.pick(&candidates);
        let params = self.funs[f].params.clone();
        let args: Vec<String> = params.iter().map(|&t| self.expr(t, env, d)).collect();
        let name = self.funs[f].name.clone();
        let depth = if Some(f) == self.this { "(- depth 1)".to_string() } else { self.rng.below(3).to_string() };
        Some(format!("({name} {depth} {})", args.join(" ")))
    }

    fn function(&mut self, index: usize) -> String {
        self.current = Some(index);
        self.this = Some(index);
        let fun = &self.funs[index];
        let (name, params, ret) = (fun.name.clone(), fun.params.clone(), fun.ret);
        let mut env = Env::default().with("depth", Ty::Int);
        let names: Vec<String> = params
            .iter()
            .enumerate()
            .map(|(i, &t)| {
                let n = format!("p{i}");
                env = env.with(&n, t);
                n
            })
            .collect();
        // The base case may only call earlier functions.
        self.this = None;
        self.current = index.checked_sub(1);
        let base = self.expr(ret, &env, 2);
        self.current = Some(index);
        self.this = Some(index);
        let body = self.expr(ret, &env, 3);
        self.this = None;
        format!("(define ({name} depth {})\n  (if (<= depth 0) {base}\n      {body}))\n", names.join(" "))
    }

    fn program(seed: u64) -> String {
        let mut g = Gen { rng: Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1), funs: Vec::new(), current: None, this: None, fresh: 0 };
        let n = g.rng.below(4) + 2;
        for i in 0..n {
            let params = (0..g.rng.below(3) + 1).map(|_| *g.rng.pick(&TYPES)).collect();
            let ret = *g.rng.pick(&TYPES);
            g.funs.push(Fun { name: format!("f{i}"), params, ret });
        }
        let mut out = String::from(
            "(define results '())\n\
             (define (record x) (set! results (cons x results)))\n\
             (define (try thunk) (guard (e (#t (list 'err (if (error-object? e) (condition/report-string e) e)))) (thunk)))\n",
        );
        for i in 0..n {
            out.push_str(&g.function(i));
        }
        g.current = Some(n - 1);
        let rounds = 2 + g.rng.below(2);
        for round in 0..rounds {
            // Each function enough times to be compiled in every JIT mode.
            let env = Env::default().with("i", Ty::Int);
            let mut calls = String::new();
            for f in 0..n {
                let params = g.funs[f].params.clone();
                let args: Vec<String> = params.iter().map(|&t| g.expr(t, &env, 2)).collect();
                let depth = g.rng.below(4);
                let _ = write!(calls, " (record (try (lambda () ({} {depth} {}))))", g.funs[f].name, args.join(" "));
            }
            if g.rng.chance(30) {
                // In tasks, to be preempted inside compiled code.
                let _ = writeln!(
                    out,
                    "(let ((t (spawn (lambda () (let loop ((i 0)) (when (< i 25) {calls} (loop (+ i 1)))) 'done))))\n  (record (task-join t)))"
                );
            } else {
                let _ = writeln!(out, "(let loop ((i 0)) (when (< i 25) {calls} (loop (+ i 1))))");
            }
            if round + 1 < rounds && g.rng.chance(60) {
                // Redefine a function: call sites specialised for it.
                let f = g.rng.below(n);
                let text = g.function(f);
                let text = text.replacen(&format!("(define ({} ", g.funs[f].name), "(set! FN (lambda (", 1).replace("FN", &g.funs[f].name);
                let _ = writeln!(out, "{})", text.trim_end());
                g.current = Some(n - 1);
            }
        }
        out.push_str("(for-each (lambda (r) (write r) (newline)) (reverse results))\n");
        out
    }
}

struct Outcome {
    stdout: String,
    stderr: String,
    status: Option<i32>,
}

/// Run `file` with `env`; `None` on timeout.
fn run(file: &std::path::Path, env: &[(&str, &str)], timeout: Duration) -> Option<Outcome> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_techne-vm"))
        .arg(file)
        .env_remove("TECHNE_JIT")
        .env_remove("TECHNE_GC_STRESS")
        .envs(env.iter().copied())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let out = child.wait_with_output().unwrap();
    Some(Outcome {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        status: out.status.code(),
    })
}

fn env_num(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[test]
fn jit_matches_interpreter() {
    let seeds = env_num("TECHNE_FUZZ_SEEDS", 40);
    let start = env_num("TECHNE_FUZZ_START", 0);
    let dir = std::env::temp_dir().join(format!("techne-fuzz-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let modes: [&[(&str, &str)]; 4] = [
        &[("TECHNE_JIT", "1")],
        &[("TECHNE_JIT", "20")],
        &[("TECHNE_JIT", "1"), ("TECHNE_GC_STRESS", "1")],
        &[("TECHNE_JIT", "5"), ("TECHNE_GC_STRESS", "1")],
    ];
    let (mut ran, mut errors, mut skipped) = (0, 0, 0);
    for seed in start..start + seeds {
        let program = Gen::program(seed);
        let file = dir.join(format!("seed-{seed}.scm"));
        std::fs::write(&file, &program).unwrap();
        let Some(reference) = run(&file, &[("TECHNE_JIT", "0")], Duration::from_secs(10)) else {
            skipped += 1;
            continue;
        };
        ran += 1;
        errors += reference.stdout.matches("(err ").count().min(1);
        for mode in modes {
            let Some(got) = run(&file, mode, Duration::from_secs(120)) else {
                panic!("seed {seed} timed out with {mode:?} (interpreter finished): {}", file.display());
            };
            assert!(
                got.stdout == reference.stdout && got.stderr == reference.stderr && got.status == reference.status,
                "seed {seed} differs with {mode:?}: {}\n--- interpreter (status {:?})\n{}{}\n--- JIT (status {:?})\n{}{}",
                file.display(),
                reference.status,
                reference.stdout,
                reference.stderr,
                got.status,
                got.stdout,
                got.stderr
            );
        }
        std::fs::remove_file(&file).unwrap();
    }
    eprintln!("fuzz: {ran} programs ({errors} with caught errors), {skipped} skipped (interpreter timeout)");
    assert!(ran > seeds / 2, "most generated programs should run");
}

#[test]
fn programs_are_mostly_valid() {
    // Guards the generator itself: most programs run to the end.
    let dir = std::env::temp_dir().join(format!("techne-fuzz-valid-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let finished = (0..20)
        .filter(|&seed| {
            let file = dir.join(format!("seed-{seed}.scm"));
            std::fs::write(&file, Gen::program(seed)).unwrap();
            run(&file, &[("TECHNE_JIT", "0")], Duration::from_secs(10)).is_some_and(|o| o.status == Some(0))
        })
        .count();
    assert!(finished >= 15, "only {finished}/20 generated programs ran to the end");
}
