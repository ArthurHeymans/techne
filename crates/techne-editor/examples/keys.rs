//! Keystroke latency in the runtime, headless: named workloads type keys
//! and each key is timed from its input to the snapshot that shows its
//! effect (the runtime's part of the keystroke budget, REQUIREMENTS.md,
//! "Responsiveness budgets"; drawing the frame is the frontend's).
//!
//! `keys` runs every workload and prints p50, p99 and the slowest key,
//! marking those over the 4 ms budget. `keys --list` names them; `keys
//! NAME` runs one and prints how many keys it timed, and `keys NAME
//! --setup` runs only what precedes them: the difference in instructions
//! between the two, per key, is what `runtime/bench/icount.sh` reports,
//! which does not depend on the machine's load.

use std::time::{Duration, Instant};

use techne_editor::{present::Input, runtime::Runtime};
use techne_text::Document;

/// The runtime's share of the keystroke budget.
const BUDGET: Duration = Duration::from_millis(4);

struct Workload {
    name: &'static str,
    profile: &'static str,
    text: fn() -> String,
    /// Keys before the timed ones.
    setup: &'static str,
    timed: &'static str,
}

const WORKLOADS: &[Workload] = &[
    Workload { name: "type", profile: "emacs", text: lines, setup: "", timed: TYPING },
    Workload { name: "type-long-line", profile: "emacs", text: long_line, setup: "C-n", timed: TYPING },
    Workload { name: "move", profile: "emacs", text: lines, setup: "", timed: MOVING },
    Workload { name: "move-modal", profile: "modal", text: lines, setup: "", timed: MOVING_MODAL },
    Workload { name: "search-first-key", profile: "emacs", text: lines, setup: "C-c s s", timed: "a" },
    Workload { name: "search-narrow", profile: "emacs", text: lines, setup: "C-c s s a", timed: "l p h" },
    Workload { name: "search-delete", profile: "emacs", text: lines, setup: "C-c s s a l p", timed: "DEL" },
    Workload { name: "command-first-key", profile: "emacs", text: lines, setup: "M-x", timed: "s" },
];

const TYPING: &str = "t h e SPC q u i c k SPC b r o w n SPC f o x SPC j u m p s SPC o v e r SPC t h e SPC l a z y SPC d o g RET";
const MOVING: &str = "C-n C-n C-n C-f C-f M-f M-f C-e C-a C-v C-v C-v M-v C-n C-p M-> M-v M-v M-<";
const MOVING_MODAL: &str = "j j j l l w w $ 0 C-f C-f C-f C-b j k G C-b C-b g g";

/// 100,000 lines of words, as `crates/techne-window/bench.sh` makes them.
fn lines() -> String {
    let mut words = Words(1);
    (0..100_000).map(|_| (0..words.below(15)).map(|_| words.next()).collect::<Vec<_>>().join(" ") + "\n").collect()
}

/// One line of 1 MB between two short ones.
fn long_line() -> String {
    let mut words = Words(2);
    let mut line = String::new();
    while line.len() < 1 << 20 {
        line.push_str(words.next());
        line.push(' ');
    }
    format!("start\n{line}\nend\n")
}

/// Words drawn by a fixed linear congruential generator.
struct Words(u64);

impl Words {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }

    fn next(&mut self) -> &'static str {
        const WORDS: [&str; 10] = ["alpha", "beta", "gamma", "delta", "println!", "fn", "let", "mut", "struct", "impl"];
        WORDS[self.below(10) as usize]
    }
}

fn press(rt: &mut Runtime, key: &str) -> Duration {
    let start = Instant::now();
    rt.handle(Input::Key { key: key.into(), at: start });
    std::hint::black_box(rt.snapshot());
    start.elapsed()
}

/// The time each timed key took, or none with `setup_only`.
fn run(w: &Workload, setup_only: bool) -> Vec<Duration> {
    let mut rt = Runtime::with_document(Document::new(&(w.text)()), w.profile).expect("the editor starts");
    for k in w.setup.split_whitespace() {
        press(&mut rt, k);
    }
    if setup_only {
        return Vec::new();
    }
    w.timed.split_whitespace().map(|k| press(&mut rt, k)).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--list") {
        WORKLOADS.iter().for_each(|w| println!("{}", w.name));
        return;
    }
    if let Some(name) = args.first() {
        let w = WORKLOADS.iter().find(|w| w.name == name).unwrap_or_else(|| panic!("no workload {name}"));
        let timed = run(w, args.get(1).is_some_and(|a| a == "--setup"));
        // The number of keys timed, for dividing instructions by.
        println!("{}", timed.len());
        return;
    }
    let ms = |d: Duration| format!("{:.2}", d.as_secs_f64() * 1000.0);
    println!("{:<18} {:>5} {:>8} {:>8} {:>8}  (ms, key to snapshot)", "workload", "keys", "p50", "p99", "max");
    for w in WORKLOADS {
        let mut times = run(w, false);
        times.sort();
        let at = |q: f64| times[((q * times.len() as f64).ceil() as usize).clamp(1, times.len()) - 1];
        let over = if at(1.0) > BUDGET { "  over budget" } else { "" };
        println!("{:<18} {:>5} {:>8} {:>8} {:>8}{over}", w.name, times.len(), ms(at(0.5)), ms(at(0.99)), ms(at(1.0)));
    }
}
