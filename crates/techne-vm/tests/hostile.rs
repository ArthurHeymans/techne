//! Worlds as a boundary between tenants (REQUIREMENTS.md): a world given
//! what a VM bug would give it (`techne_vm::hostile`: values forged from
//! any bits, any word of its own cage read or written) may break its own
//! heap at will, and still read nothing of a neighbour's, change nothing
//! of it, nor end the process. What it does ends in errors, or in its
//! world retired.
//!
//! Each case runs in a child process, under a deadline, so that one ending
//! the process fails only itself. In it, an attack runs twice, against
//! neighbours whose secrets differ only in a cookie the hostile world is
//! never told: what the hostile world sees must be the same both times,
//! and hold no cookie. While the hostile world runs, the pages of the
//! neighbour's heap are inaccessible, so that touching them ends the
//! process; after, the neighbour's secrets, and its heap, are checked
//! intact.
//!
//! The fuzzer (`corruption_fuzz`) breaks a hostile world's heap at random,
//! then runs programs that walk it: `TECHNE_HOSTILE_SEEDS` (default 20) and
//! `TECHNE_HOSTILE_START` (default 0) pick the seeds.

use std::{
    process::Command,
    time::{Duration, Instant},
};

use techne_vm::{hostile, vm::Vm};

/// The tag of a heap pointer, of a native, and of a symbol (`value.rs`).
const POINTER: u64 = 0xFFFA << 48;
const NATIVE: u64 = 0xFFFE << 48;
const SYMBOL: u64 = 0xFFFC << 48;
/// Where spans start in a cage (`alloc::cage::START`).
const START: u64 = 64 << 10;

/// Run `attack` in a child process (the test `name` of this binary), once
/// against each of two neighbours whose secrets differ: what it returns,
/// all the hostile world saw, must not tell them apart. Each run is a
/// process of its own, without address randomisation, so that both start
/// alike.
fn isolated(name: &str, attack: impl Fn(&mut Neighbour) -> Vec<String>) {
    if std::env::var("TECHNE_HOSTILE_CHILD").as_deref() == Ok(name) {
        techne_vm::alloc::contain_out_of_memory();
        let run = std::env::var("TECHNE_HOSTILE_RUN").unwrap().parse().unwrap();
        let mut neighbour = Neighbour::new(run);
        let seen = attack(&mut neighbour);
        seen.iter().for_each(|out| neighbour.assert_unread(out));
        neighbour.assert_intact();
        // Each run's cookie made the other's, so that one is not mistaken
        // for a difference.
        let seen: Vec<String> = seen.into_iter().map(|out| out.replace(&neighbour.marker, "SECRET")).collect();
        std::fs::write(std::env::var("TECHNE_HOSTILE_SEEN").unwrap(), seen.join("\n\u{1e}\n")).unwrap();
        return;
    }
    let runs: Vec<(Option<std::process::ExitStatus>, String, String)> = (0..2).map(|run| child(name, run)).collect();
    let tail = |log: &str| log.lines().rev().take(30).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    let failed = runs.iter().find(|(status, ..)| !status.is_some_and(|s| s.success()));
    let differ = failed.is_none() && runs[0].2 != runs[1].2;
    let outcome = match failed {
        Some((None, log, _)) => format!("past its deadline\n{}", tail(log)),
        Some((Some(status), log, _)) => format!("{status}\n{}", tail(log)),
        None if differ => {
            let (a, b) = (runs[0].2.split("\n\u{1e}\n"), runs[1].2.split("\n\u{1e}\n"));
            let (a, b) = a.zip(b).find(|(a, b)| a != b).unwrap_or_default();
            format!("what the hostile world saw depends on the neighbour's secrets:\n  {a}\n  {b}")
        }
        None => "contained".into(),
    };
    assert!(failed.is_none() && !differ, "{name}: {outcome}");
}

/// Run `name` as run `run` in a child process: how it ended (`None` past
/// its deadline), its output, and what the hostile world saw.
fn child(name: &str, run: u64) -> (Option<std::process::ExitStatus>, String, String) {
    use std::os::unix::process::CommandExt as _;
    let path = |what: &str| std::env::temp_dir().join(format!("techne-hostile-{}-{name}-{run}.{what}", std::process::id()));
    let (out, seen) = (path("log"), path("seen"));
    let log = std::fs::File::create(&out).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([name, "--exact", "--nocapture", "--include-ignored", "--test-threads=1"])
        .env("TECHNE_HOSTILE_CHILD", name)
        .env("TECHNE_HOSTILE_RUN", run.to_string())
        .env("TECHNE_HOSTILE_SEEN", &seen)
        // Checking the hostile world's heap, broken on purpose, would abort.
        .env_remove("TECHNE_GC_VERIFY")
        .stdout(log.try_clone().unwrap())
        .stderr(log);
    // SAFETY: only a system call between fork and exec.
    unsafe {
        command.pre_exec(|| {
            libc::personality(libc::ADDR_NO_RANDOMIZE as libc::c_ulong);
            Ok(())
        })
    };
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(600);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let read = |p: &std::path::Path| {
        let text = std::fs::read_to_string(p).unwrap_or_default();
        let _ = std::fs::remove_file(p);
        text
    };
    (status, read(&out), read(&seen))
}

/// A world holding secrets marked with a cookie of its own.
struct Neighbour {
    vm: Vm,
    marker: String,
    fingerprint: String,
}

/// The secrets, by name, that the hostile world is given the bits of; the
/// symbol's are its id, which `symbols_forged_name_nothing_of_a_neighbour`
/// tries.
const SECRETS: [&str; 7] = ["secret-string", "secret-bytes", "secret-vector", "secret-pair", "secret-table", "secret-proc", "secret-box"];

impl Neighbour {
    /// A neighbour of cookie `n`th: as long as any other's, and laid out
    /// alike.
    fn new(n: u64) -> Neighbour {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64;
        let cookie = (seed.wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(n as u32 * 13) & ((1 << 44) - 1)) | (1 << 44);
        let marker = format!("SECRET{cookie:x}");
        let mut vm = Vm::new();
        vm.set_jit(None);
        hostile::install(&mut vm);
        vm.eval_source(&format!(
            r#"(define secret-string (string-copy "{marker}"))
               (define secret-bytes (let ((b (make-bytevector 64 0)))
                 (do ((i 0 (+ i 1))) ((= i 64) b)
                   (bytevector-u8-set! b i (modulo (quotient {cookie} (expt 256 (modulo i 8))) 256)))))
               (define secret-symbol (string->symbol "{marker}-symbol"))
               (define secret-vector (make-vector 16 {cookie}))
               (define secret-pair (cons {cookie} "{marker}-pair"))
               (define secret-table (let ((t (make-hash-table))) (hash-table-set! t 'k "{marker}-table") t))
               (define (secret-proc) "{marker}-proc")
               (define secret-box (box (list secret-string {cookie})))
               (define secret-regexp (%make-regexp "{marker}" "{marker}" '()))
               (define (fingerprint)
                 (with-output-to-string
                   (lambda ()
                     (write (list secret-string secret-bytes secret-symbol secret-vector secret-pair
                                  (hash-table-ref/default secret-table 'k #f) (secret-proc) (unbox secret-box))))))
               (collect-garbage)"#
        ))
        .unwrap();
        let fingerprint = vm.eval_source("(fingerprint)").unwrap();
        let fingerprint = vm.repr(&fingerprint);
        Neighbour { vm, marker, fingerprint }
    }

    /// The bits of the secret `name`.
    fn bits(&mut self, name: &str) -> i64 {
        let v = self.vm.eval_source(&format!("(%bits {name})")).unwrap();
        self.vm.get::<i64>(&v).unwrap()
    }

    /// Run `f` with the neighbour's heap inaccessible.
    fn hidden<T>(&mut self, f: impl FnOnce() -> T) -> T {
        hostile::protect(&self.vm, true);
        let t = f();
        hostile::protect(&self.vm, false);
        t
    }

    /// Its secrets as they were, and its heap whole.
    fn assert_intact(&mut self) {
        assert!(self.vm.poisoned().is_none(), "the neighbour was retired");
        let now = self.vm.eval_source("(fingerprint)").unwrap();
        assert_eq!(self.vm.repr(&now), self.fingerprint, "the neighbour's secrets changed");
        hostile::verify(&mut self.vm);
    }

    /// Whether `out`, what the hostile world returned, holds a secret.
    fn assert_unread(&self, out: &str) {
        assert!(!out.contains(&self.marker), "the hostile world read a secret: {out}");
    }
}

fn hostile_world(jit: Option<u32>) -> Vm {
    let mut vm = Vm::new();
    vm.set_jit(jit);
    hostile::install(&mut vm);
    vm.eval_source(PROBE).unwrap();
    vm
}

/// `(probe x)`: every way of looking into `x`, each on its own, as a list
/// of what each found or the error it raised.
const PROBE: &str = r#"
(define (try f x)
  (guard (e ((error-object? e) (cons (error-object-message e) (error-object-irritants e)))
            (#t (list 'raised e)))
    (f x)))
(define probes
  (list (lambda (x) x) car cdr (lambda (x) (vector-ref x 0)) vector-length (lambda (x) (vector->list x))
        string-length (lambda (x) (string-copy x)) (lambda (x) (string-ref x 0)) (lambda (x) (string->list x))
        bytevector-length (lambda (x) (bytevector-copy x)) (lambda (x) (bytevector-u8-ref x 0))
        symbol->string unbox (lambda (x) (x)) (lambda (x) (apply x '(1 2)))
        (lambda (x) (hash-table-ref/default x 'k #f)) (lambda (x) (hash-table->alist x))
        (lambda (x) (with-output-to-string (lambda () (write x))))
        (lambda (x) (equal? x (list 1 2))) (lambda (x) (equal-hash x))))
(define (probe x) (map (lambda (f) (try f x)) probes))
"#;

/// What `src` returned in `vm`, or the error it raised, as text.
fn run(vm: &mut Vm, src: &str) -> String {
    match vm.eval_source(src) {
        Ok(v) => vm.repr(&v),
        Err(e) => format!("error: {}", e.msg),
    }
}

const JITS: [Option<u32>; 2] = [None, Some(1)];

/// The offset in its cage of the object `expr` evaluates to, in Scheme.
fn offset_of(expr: &str) -> String {
    format!("(modulo (%bits {expr}) 4294967296)")
}

#[test]
fn values_of_a_neighbour_read_nothing_of_it() {
    isolated("values_of_a_neighbour_read_nothing_of_it", |neighbour| {
        let bits: Vec<i64> = SECRETS.iter().map(|name| neighbour.bits(name)).collect();
        neighbour.hidden(|| {
            let mut seen = vec![];
            for jit in JITS {
                let mut vm = hostile_world(jit);
                for &bits in &bits {
                    seen.push(run(&mut vm, &format!("(probe (%forge {bits}))")));
                    // Its own pair, made to point at the neighbour's offset.
                    let offset = bits as u64 & 0xFFFF_FFFF;
                    seen.push(run(&mut vm, &format!("(probe (%forge (+ (%bits '(x)) (- {offset} {}))))", offset_of("'(x)"))));
                }
            }
            seen
        })
    });
}

#[test]
fn a_world_reading_all_of_its_cage_finds_nothing_of_a_neighbour() {
    isolated("a_world_reading_all_of_its_cage_finds_nothing_of_a_neighbour", |neighbour| {
        let cookie = u64::from_str_radix(&neighbour.marker["SECRET".len()..], 16).unwrap();
        neighbour.hidden(|| {
            let mut vm = hostile_world(Some(1));
            let src = format!(
                "(let loop ((at {START}) (n 0)) (if (>= at (%cage-top)) n (loop (+ at 8) (if (= (%cage-ref at) {cookie}) (+ n 1) n))))"
            );
            let found = run(&mut vm, &src);
            assert_eq!(found, "0", "the hostile world read a secret: the neighbour's cookie is in its cage");
            vec![found]
        })
    });
}

#[test]
fn values_forged_into_its_own_cage_end_in_errors() {
    isolated("values_forged_into_its_own_cage_end_in_errors", |neighbour| {
        neighbour.hidden(|| {
            let mut seen = vec![];
            for jit in JITS {
                let mut vm = hostile_world(jit);
                let top = run(&mut vm, "(%cage-top)").parse::<u64>().unwrap();
                let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
                for _ in 0..300 {
                    // Any word of what the heap has used, aligned or not.
                    let offset = START + rng.next() % (top - START);
                    seen.push(run(&mut vm, &format!("(probe (%forge {}))", (POINTER | offset) as i64)));
                }
            }
            seen
        })
    });
}

#[test]
fn a_header_claiming_the_whole_cage_reads_only_the_cage() {
    isolated("a_header_claiming_the_whole_cage_reads_only_the_cage", |neighbour| {
        neighbour.hidden(|| {
            let mut seen = vec![];
            for jit in JITS {
                for (make, read) in [
                    ("(make-vector 4 1)", "(vector-ref x (- (vector-length x) 1))"),
                    ("(make-vector 4 1)", "(vector->list x)"),
                    ("(make-string 4 #\\a)", "(string-copy x (- (string-length x) 8))"),
                    ("(make-bytevector 4 1)", "(bytevector-u8-ref x (- (bytevector-length x) 1))"),
                ] {
                    let mut vm = hostile_world(jit);
                    // The header's length (from bit 16) made all ones.
                    seen.push(run(
                        &mut vm,
                        &format!(
                            "(define x {make})
                                 (define at {})
                                 (%cage-set! at (+ (modulo (%cage-ref at) 65536) (* 65535 65536 65536)))
                                 (try (lambda (x) {read}) x)",
                            offset_of("x")
                        ),
                    ));
                }
            }
            seen
        })
    });
}

#[test]
fn a_collection_over_a_corrupt_header_ends_only_the_world() {
    isolated("a_collection_over_a_corrupt_header_ends_only_the_world", |neighbour| {
        neighbour.hidden(|| {
            let mut vm = hostile_world(None);
            let src = format!(
                "(define x (make-vector 4 1))
                     (define at {})
                     (%cage-set! at (+ (modulo (%cage-ref at) 65536) (* 65535 65536 65536)))
                     (collect-garbage)",
                offset_of("x")
            );
            vec![run(&mut vm, &src)]
        })
    });
}

#[test]
fn symbols_forged_name_nothing_of_a_neighbour() {
    isolated("symbols_forged_name_nothing_of_a_neighbour", |neighbour| {
        neighbour.hidden(|| {
            let mut vm = hostile_world(None);
            // Every id a symbol of the thread may have.
            let src = format!(
                "(let loop ((id 0) (found '()))
                       (if (= id 100000) (length found)
                           (loop (+ id 1) (let ((s (try symbol->string (%forge (+ {} id))))) (if (string? s) (cons s found) found)))))",
                SYMBOL as i64
            );
            vec![run(&mut vm, &src), run(&mut vm, &src.replace("(length found)", "found"))]
        })
    });
}

#[test]
fn natives_forged_are_this_worlds_or_none() {
    isolated("natives_forged_are_this_worlds_or_none", |neighbour| {
        neighbour.hidden(|| {
            let mut seen = vec![];
            for jit in JITS {
                let mut vm = hostile_world(jit);
                for index in [0u64, 1, 100, 1000, 5000, 1 << 20, (1 << 32) - 1, (1 << 48) - 1] {
                    seen.push(run(&mut vm, &format!("(try (lambda (f) (f)) (%forge {}))", (NATIVE | index) as i64)));
                }
            }
            // A world granted nothing calls nothing ungranted, whatever
            // it forges.
            let mut vm = Vm::with_grants(techne_vm::vm::Grants::NONE);
            hostile::install(&mut vm);
            let open = run(&mut vm, "(%bits open-input-file)").parse::<i64>().unwrap();
            let out = run(&mut vm, &format!("((%forge {open}) \"/etc/passwd\")"));
            assert!(out.contains("not granted"), "{out}");
            seen.push(out);
            seen
        })
    });
}

#[test]
fn foreign_values_forged_are_this_worlds_or_none() {
    isolated("foreign_values_forged_are_this_worlds_or_none", |neighbour| {
        neighbour.hidden(|| {
            let mut seen = vec![];
            for jit in JITS {
                let mut vm = hostile_world(jit);
                // Its own regexp, its index in the world's table of
                // foreign values (the object's only field) made another.
                for index in [0u64, 1, 2, 7, 100, 1 << 31, u32::MAX as u64, (1 << 47) - 1, u64::MAX] {
                    let src = format!(
                        "(define r (%make-regexp \"x\" \"x\" '()))
                             (%cage-set! (+ {} 8) {})
                             (list (probe r) (try (lambda (r) (%regexp-search r \"SECRET\" 0 6 0 #f)) r))",
                        offset_of("r"),
                        index as i64
                    );
                    seen.push(run(&mut vm, &src));
                }
            }
            seen
        })
    });
}

#[test]
fn numbers_and_dicts_that_hold_themselves_end_in_errors() {
    isolated("numbers_and_dicts_that_hold_themselves_end_in_errors", |neighbour| {
        neighbour.hidden(|| {
            let mut seen = vec![];
            for jit in JITS {
                let mut vm = hostile_world(jit);
                // A ratio's numerator and a complex number's real part made
                // the number itself; a dict's root made a node of children
                // all itself.
                let src = format!(
                    "(define (itself! x at) (%cage-set! (+ {} 8 (* 8 at)) (%bits x)) x)
                     (define r1 (itself! (/ 1 3) 0))
                     (define r2 (itself! (/ 1 7) 0))
                     (define c (itself! (make-rectangular 1.5 2.5) 0))
                     (define d (dict 'a 1 'b 2))
                     (define n (%forge (%cage-ref (+ {} 16))))
                     (define at {})
                     (%cage-set! (+ at 8) (%bits 0))
                     (%cage-set! (+ at 16) (%bits 4294967295))
                     (do ((k 2 (+ k 1))) ((= k 34)) (%cage-set! (+ at 8 (* 8 k)) (%bits n)))
                     (list (try (lambda (x) (eqv? x r2)) r1) (try (lambda (x) (equal? x r2)) r1)
                           (try (lambda (x) (+ x 1)) c) (try (lambda (x) (number->string x)) c)
                           (try (lambda (x) (eqv? x (itself! (make-rectangular 1.5 2.5) 0))) c)
                           (try (lambda (d) (dict-ref d 'zzz #f)) d) (try (lambda (d) (dict-set d 'zzz 3)) d)
                           (try (lambda (d) (dict-delete d 'zzz)) d)
                           (try (lambda (x) (let ((t (make-hash-table equal?))) (hash-table-set! t x 1) (hash-table-ref/default t x #f))) c))",
                    offset_of("x"),
                    offset_of("d"),
                    offset_of("n")
                );
                seen.push(run(&mut vm, &src));
            }
            seen
        })
    });
}

#[test]
fn strings_of_bytes_that_are_no_utf8_end_in_errors() {
    isolated("strings_of_bytes_that_are_no_utf8_end_in_errors", |neighbour| {
        neighbour.hidden(|| {
            let mut seen = vec![];
            for jit in JITS {
                let mut vm = hostile_world(jit);
                // Its bytes after the header word made 0xff, then a lead
                // byte last, its continuation past the end.
                let src = format!(
                    "(define s (make-string 16 #\\a))
                         (define at {})
                         (%cage-set! (+ at 8) -1)
                         (%cage-set! (+ at 16) -2305843009213693952)
                         (probe s)",
                    offset_of("s")
                );
                seen.push(run(&mut vm, &src));
            }
            seen
        })
    });
}

#[test]
fn a_world_past_its_memory_ceiling_ends_alone() {
    isolated("a_world_past_its_memory_ceiling_ends_alone", |neighbour| {
        neighbour.hidden(|| {
            let mut vm = hostile_world(Some(1));
            run(&mut vm, "(define kept (make-vector 100000 'kept))");
            // No VM bug: the host lowers its limit below a third of what it
            // holds, and it runs on.
            vm.set_memory_limit(vm.held().total() / 8);
            let out = run(&mut vm, "(vector-length kept)");
            assert!(out.starts_with("error"), "{out}");
            vec![out]
        })
    });
}

#[test]
fn snapshots_of_a_corrupt_world_end_in_errors() {
    isolated("snapshots_of_a_corrupt_world_end_in_errors", |neighbour| {
        neighbour.hidden(|| {
            let mut vm = hostile_world(None);
            let src = format!(
                "(define kept (list (make-vector 4 1) (make-string 3 #\\a)))
                     (%cage-set! {} {})",
                offset_of("(car kept)"),
                (POINTER | START) as i64
            );
            let out = run(&mut vm, &src);
            let copied = vm.snapshot().map(|s| run(&mut s.world(), "kept"));
            vec![out, format!("{:?}", copied.map_err(|e| e.msg.clone()))]
        })
    });
}

#[test]
fn corruption_fuzz() {
    isolated("corruption_fuzz", |neighbour| {
        let seeds = std::env::var("TECHNE_HOSTILE_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(20u64);
        let start = std::env::var("TECHNE_HOSTILE_START").ok().and_then(|s| s.parse().ok()).unwrap_or(0u64);
        (start..start + seeds)
            .flat_map(|seed| {
                eprintln!("hostile seed {seed} (TECHNE_HOSTILE_START={seed} TECHNE_HOSTILE_SEEDS=1)");
                neighbour.hidden(|| fuzz_one(seed))
            })
            .collect()
    });
}

/// A world of seed `seed` whose heap, walked a while, is broken at random,
/// then walked again: what it saw.
fn fuzz_one(seed: u64) -> Vec<String> {
    let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let mut vm = hostile_world(*rng.pick(&JITS));
    let mut seen = vec![run(&mut vm, WORKLOAD_SETUP)];
    // Warm: compiled, promoted, a collection perhaps under way.
    for _ in 0..rng.below(4) {
        seen.push(run(&mut vm, rng.pick(&WORKLOADS)));
    }
    let top = run(&mut vm, "(%cage-top)").parse::<u64>().unwrap();
    for _ in 0..1 + rng.below(8) {
        // Mostly a word of an object the workloads walk, its header or a
        // field; else any word of what the heap has used.
        let offset = if rng.chance(75) {
            let at = run(&mut vm, &offset_of(&format!("(list-ref data {})", rng.below(DATA))));
            at.parse::<u64>().unwrap_or(START) + 8 * rng.below(4) as u64
        } else {
            START + rng.next() % (top - START)
        };
        let word = match rng.below(5) {
            0 => rng.next(),
            // A header of any kind and length.
            1 => (rng.next() & 0xFFFF_FFFF) << 16 | (rng.next() & 0xFF),
            // A header of a small length.
            2 => (rng.next() & 0xFF) << 16 | (rng.next() & 0xFF),
            3 => POINTER | (rng.next() & 0xFFFF_FFFF),
            _ => POINTER | (START + (rng.next() % (top - START))),
        };
        seen.push(run(&mut vm, &format!("(%cage-set! {offset} {})", word as i64)));
    }
    for _ in 0..4 {
        seen.push(run(&mut vm, rng.pick(&WORKLOADS)));
    }
    seen
}

const DATA: usize = 8;

/// `data`: objects of every kind the workloads walk.
const WORKLOAD_SETUP: &str = r#"
(define data
  (list (iota 50) (make-vector 20 'v) (make-string 30 #\s) (make-bytevector 40 7)
        (let ((t (make-hash-table))) (do ((i 0 (+ i 1))) ((= i 30) t) (hash-table-set! t i (list i))))
        (lambda (x) (+ x 1)) (box (vector 1 "two" 'three)) (list->string (map integer->char (iota 40 65)))))
"#;

const WORKLOADS: [&str; 9] = [
    "(collect-garbage)",
    "(probe data)",
    "(map probe data)",
    "(with-output-to-string (lambda () (write data)))",
    "(equal? data (list-copy data))",
    "(begin (define more (map (lambda (i) (make-vector i data)) (iota 200))) (collect-garbage) (length more))",
    "(map (lambda (x) (try equal-hash x)) data)",
    "(map (lambda (x) (try (lambda (f) (f 1)) x)) data)",
    "(begin (define kept (vector-map (lambda (x) (list x)) (list->vector data))) (collect-garbage) (vector-length kept))",
];

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
