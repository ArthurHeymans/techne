//! Random use of the host API across the worlds of one thread: values made,
//! held across collections, passed to their own and other worlds, kept in
//! globals and natives, their worlds dropped and others made in their
//! place. A value of a live world means what it meant; one of another world
//! or a dropped one is refused, never read (immediates and symbols belong to
//! no world). Under GC stress too.
//!
//! `TECHNE_HOST_API_SEEDS` runs (100), from `TECHNE_HOST_API_START` (0), of
//! `TECHNE_HOST_API_STEPS` steps (300); a failure names its seed.

use std::{cell::RefCell, rc::Rc};

use techne_vm::{api::Value, vm::Vm};

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
}

/// Sources of values: heap objects of several kinds, immediates, a symbol.
const SOURCES: &[&str] = &[
    "(list 1 \"two\" (vector 3 4.5) 'five)",
    "(make-string 40 #\\x)",
    "(let loop ((i 0) (l '())) (if (= i 200) l (loop (+ i 1) (cons (* i i) l))))",
    "(vector (cons 1 2) (bytevector 1 2 3) #\\a)",
    "(lambda (x) (+ x 1))",
    "(expt 3 100)",
    "42",
    "'a-symbol",
    "#\\z",
    "(string->symbol \"made-up\")",
];

/// A world of a slot: its instance (worlds made in a slot count up).
struct World {
    vm: Vm,
    instance: u64,
}

/// A value the host holds: the world instance it came from, and how it
/// printed then.
struct Held {
    value: Value,
    instance: u64,
    printed: String,
}

impl Held {
    /// A heap object, or a native: of its world only.
    fn bound(&self) -> bool {
        self.value.is_object()
    }
}

struct Run {
    rng: Rng,
    worlds: Vec<Option<World>>,
    next_instance: u64,
    held: Vec<Held>,
    /// What the natives registered in worlds return, set by the host.
    handed: Rc<RefCell<Option<Value>>>,
    log: Vec<String>,
    /// Held values taken by a world, and refused (of another or a dropped
    /// world): the runs should see both.
    taken: usize,
    refused: usize,
}

impl Run {
    fn new(seed: u64) -> Run {
        let mut run = Run {
            rng: Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1),
            worlds: (0..3).map(|_| None).collect(),
            next_instance: 1,
            held: Vec::new(),
            handed: Rc::default(),
            log: Vec::new(),
            taken: 0,
            refused: 0,
        };
        for slot in 0..run.worlds.len() {
            run.make_world(slot);
        }
        run
    }

    fn make_world(&mut self, slot: usize) {
        let mut vm = Vm::new();
        if self.rng.below(2) == 0 {
            vm.set_jit(None);
        }
        if self.rng.below(2) == 0 {
            vm.set_gc_stress(true, self.rng.below(2) == 0);
        }
        let handed = self.handed.clone();
        vm.register_fn("handed", move || handed.borrow().clone().unwrap_or(Value::FALSE));
        let instance = self.next_instance;
        self.next_instance += 1;
        self.worlds[slot] = Some(World { vm, instance });
        self.log.push(format!("world {instance} in slot {slot}"));
    }

    fn live(&mut self) -> usize {
        loop {
            let slot = self.rng.below(self.worlds.len());
            if self.worlds[slot].is_some() {
                return slot;
            }
        }
    }

    /// Whether `h` is of the world in `slot`, alive.
    fn of(&self, h: &Held, slot: usize) -> bool {
        self.worlds[slot].as_ref().is_some_and(|w| w.instance == h.instance)
    }

    fn world_alive(&self, instance: u64) -> bool {
        self.worlds.iter().flatten().any(|w| w.instance == instance)
    }

    /// What `vm.repr` gives for `h` in the world of `slot`.
    fn expected(&self, h: &Held, slot: usize) -> String {
        if !h.bound() || self.of(h, slot) {
            h.printed.clone()
        } else if self.world_alive(h.instance) {
            "#<a value of another world>".into()
        } else {
            "#<a value of a dropped world>".into()
        }
    }

    fn step(&mut self) {
        match self.rng.below(9) {
            0 | 1 => {
                let slot = self.live();
                let src = SOURCES[self.rng.below(SOURCES.len())];
                let w = self.worlds[slot].as_mut().unwrap();
                let value = w.vm.eval_source(src).unwrap_or_else(|e| panic!("{src}: {e}"));
                let printed = w.vm.repr(&value);
                self.log.push(format!("make {src} in world {}", w.instance));
                self.held.push(Held { value, instance: w.instance, printed });
            }
            2 => {
                // Print a held value in some world.
                let Some(i) = self.pick() else { return };
                let slot = self.live();
                let want = self.expected(&self.held[i], slot);
                let got = self.worlds[slot].as_ref().unwrap().vm.repr(&self.held[i].value);
                self.log.push(format!("print value {i} in slot {slot}"));
                assert_eq!(got, want, "value {i} printed in slot {slot}");
            }
            3 => {
                // Pass it to a procedure of some world.
                let Some(i) = self.pick() else { return };
                let slot = self.live();
                let ok = !self.held[i].bound() || self.of(&self.held[i], slot);
                let h = &self.held[i];
                let w = self.worlds[slot].as_mut().unwrap();
                self.log.push(format!("list value {i} in world {}", w.instance));
                match w.vm.call_global("list", std::slice::from_ref(&h.value)) {
                    Ok(l) => {
                        assert!(ok, "value {i} of world {} taken by world {}", h.instance, w.instance);
                        self.taken += 1;
                        assert_eq!(w.vm.repr(&l), format!("({})", h.printed));
                    }
                    Err(e) => {
                        assert!(!ok && e.msg.contains("world"), "value {i}: {e}");
                        self.refused += 1;
                    }
                }
            }
            4 => {
                // Keep it in a global, then read it back.
                let Some(i) = self.pick() else { return };
                let slot = self.live();
                let ok = !self.held[i].bound() || self.of(&self.held[i], slot);
                let h = &self.held[i];
                let w = self.worlds[slot].as_mut().unwrap();
                self.log.push(format!("global value {i} in world {}", w.instance));
                match w.vm.set_global("kept", &h.value) {
                    Ok(()) => {
                        assert!(ok, "value {i} of world {} kept by world {}", h.instance, w.instance);
                        let v = w.vm.eval_source("kept").unwrap();
                        assert_eq!(w.vm.repr(&v), h.printed);
                    }
                    Err(e) => {
                        assert!(!ok && e.msg.contains("world"), "value {i}: {e}");
                        self.refused += 1;
                    }
                }
            }
            5 => {
                // A native returns it.
                let Some(i) = self.pick() else { return };
                let slot = self.live();
                let ok = !self.held[i].bound() || self.of(&self.held[i], slot);
                *self.handed.borrow_mut() = Some(self.held[i].value.clone());
                let h = &self.held[i];
                let w = self.worlds[slot].as_mut().unwrap();
                self.log.push(format!("native hands value {i} in world {}", w.instance));
                match w.vm.eval_source("(handed)") {
                    Ok(v) => {
                        assert!(ok, "value {i} of world {} returned in world {}", h.instance, w.instance);
                        assert_eq!(w.vm.repr(&v), h.printed);
                    }
                    Err(e) => {
                        assert!(!ok && e.msg.contains("world"), "value {i}: {e}");
                        self.refused += 1;
                    }
                }
                *self.handed.borrow_mut() = None;
            }
            6 => {
                let slot = self.live();
                let w = self.worlds[slot].as_mut().unwrap();
                self.log.push(format!("collect world {}", w.instance));
                if self.rng.below(2) == 0 {
                    w.vm.full_collect()
                } else {
                    w.vm.collect()
                }
                // Garbage meanwhile, so that objects move.
                w.vm.eval_source("(let loop ((i 0)) (when (< i 300) (make-vector 20 i) (loop (+ i 1))))").unwrap();
            }
            7 => {
                // Drop a world, perhaps make another in its place.
                let slot = self.rng.below(self.worlds.len());
                if let Some(w) = self.worlds[slot].take() {
                    self.log.push(format!("drop world {}", w.instance));
                }
                if self.worlds.iter().all(Option::is_none) || self.rng.below(2) == 0 {
                    self.make_world(slot);
                }
            }
            _ => {
                // Forget a held value.
                if let Some(i) = self.pick() {
                    self.log.push(format!("forget value {i}"));
                    self.held.swap_remove(i);
                }
            }
        }
    }

    fn pick(&mut self) -> Option<usize> {
        (!self.held.is_empty()).then(|| self.rng.below(self.held.len()))
    }
}

fn var(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[test]
fn host_values_across_worlds() {
    let (start, seeds, steps) = (var("TECHNE_HOST_API_START", 0), var("TECHNE_HOST_API_SEEDS", 100), var("TECHNE_HOST_API_STEPS", 300));
    let (mut taken, mut refused) = (0, 0);
    for seed in start..start + seeds {
        let mut run = Run::new(seed);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            for _ in 0..steps {
                run.step();
            }
        }));
        if let Err(panic) = result {
            let tail = run.log.iter().rev().take(12).rev().cloned().collect::<Vec<_>>().join("\n  ");
            eprintln!("seed {seed} failed; its last steps:\n  {tail}");
            std::panic::resume_unwind(panic);
        }
        (taken, refused) = (taken + run.taken, refused + run.refused);
    }
    assert!(taken > 0 && refused > 0, "{taken} taken, {refused} refused");
}
