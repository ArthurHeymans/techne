//! Keystroke latency in the runtime, headless: named workloads type keys
//! and each key is timed from its input to the snapshot that shows its
//! effect (the runtime's part of the keystroke budget, REQUIREMENTS.md,
//! "Responsiveness budgets"; drawing the frame is the frontend's).
//!
//! `keys` runs every workload `ROUNDS` times and prints p50, p99 and the
//! slowest key, marking those over the 4 ms budget. `keys --list` names
//! them; `keys NAME` runs one and prints how many keys it timed. Under
//! callgrind, collecting only in `timed_keys`, that run counts the
//! instructions of its timed keys, which `runtime/bench/icount.sh` reports
//! per key: they do not depend on the machine's load.

use std::time::{Duration, Instant};

use techne_editor::{present::Input, runtime::Runtime};
use techne_text::Document;

/// The runtime's share of the keystroke budget.
const BUDGET: Duration = Duration::from_millis(4);

/// Runs of each workload timed, each in a new runtime.
const ROUNDS: usize = 5;

struct Workload {
    name: &'static str,
    profile: &'static str,
    text: fn() -> String,
    /// Lisp evaluated before the timed keys: what a workload's buffer
    /// needs set on it (its syntax language: a benchmark's document
    /// has no file to ask).
    eval: &'static str,
    /// Keys before the timed ones.
    setup: &'static str,
    timed: &'static str,
    /// Wait until a snapshot shows highlights before the timed keys:
    /// a syntax workload timed over plain text (its worker's tree
    /// not landed yet) measures a keystroke it meant to exclude.
    coloured: bool,
}

const WORKLOADS: &[Workload] = &[
    Workload { name: "type", profile: "emacs", text: lines, eval: "", setup: "", timed: TYPING, coloured: false },
    Workload { name: "type-long-line", profile: "emacs", text: long_line, eval: "", setup: "C-n", timed: TYPING, coloured: false },
    // Typing and moving in Rust the syntax layer colours: what parsing
    // and collecting cost a keystroke, against documents with no file
    // (which is why these set the language themselves).
    Workload { name: "type-rust", profile: "emacs", text: rust_file, eval: RUST, setup: "", timed: TYPING, coloured: true },
    Workload { name: "move-rust", profile: "emacs", text: rust_file, eval: RUST, setup: "", timed: MOVING, coloured: true },
    // The same past the boundary: the worker parses, a key is timed
    // only once its tree has landed and coloured the text.
    Workload { name: "type-rust-big", profile: "emacs", text: rust_file_big, eval: RUST, setup: "", timed: TYPING, coloured: true },
    Workload { name: "move-rust-big", profile: "emacs", text: rust_file_big, eval: RUST, setup: "", timed: MOVING, coloured: true },
    // A bracket left open before the long line: the line ending the list
    // is indented by a scan of the whole megabyte (keyfuzz found it).
    Workload {
        name: "indent-after-long-line",
        profile: "emacs",
        text: long_line,
        eval: "",
        setup: "C-n [ <down>",
        timed: "TAB",
        coloured: false,
    },
    Workload { name: "move", profile: "emacs", text: lines, eval: "", setup: "", timed: MOVING, coloured: false },
    Workload { name: "move-modal", profile: "modal", text: lines, eval: "", setup: "", timed: MOVING_MODAL, coloured: false },
    Workload { name: "search-open", profile: "emacs", text: lines, eval: "", setup: "C-c s", timed: "s", coloured: false },
    Workload { name: "search-first-key", profile: "emacs", text: lines, eval: "", setup: "C-c s s", timed: "a", coloured: false },
    Workload { name: "search-narrow", profile: "emacs", text: lines, eval: "", setup: "C-c s s a", timed: "l p h", coloured: false },
    Workload { name: "search-delete", profile: "emacs", text: lines, eval: "", setup: "C-c s s a l p", timed: "DEL", coloured: false },
    // A line yanked into the input: several common words at once, from
    // no narrower input.
    Workload { name: "search-yank-line", profile: "emacs", text: lines, eval: "", setup: "C-k C-c s s", timed: "C-y", coloured: false },
    Workload { name: "command-first-key", profile: "emacs", text: lines, eval: "", setup: "M-x", timed: "s", coloured: false },
];

const RUST: &str = "(set-option! 'syntax-language 'rust)";

/// Rust filled to `limit` bytes: functions with the usual furniture.
fn rust_sized(limit: usize) -> String {
    let mut out = String::new();
    for i in 0.. {
        let block = format!(
            "/// Worker {i}, documented as it goes.\npub fn work_{i}(input: &str, limit: usize) -> Option<String> {{\n    let trimmed = input.trim();\n    if trimmed.len() > limit {{ return None; }}\n    Some(format!(\"{{}}: {{}}\", {i}, trimmed))\n}}\n\n"
        );
        if out.len() + block.len() > limit {
            break;
        }
        out.push_str(&block);
    }
    out
}

/// Just under the syntax layer's 64 KiB synchronous band
/// (`syntax::SYNC_MAX`): the snapshot that asks parses it.
fn rust_file() -> String {
    rust_sized(60 << 10)
}

/// Just over it: the worker parses, and the text colours when its
/// tree lands (the barrier in `run` waits for exactly that).
fn rust_file_big() -> String {
    rust_sized(96 << 10)
}

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

/// The time each timed key took.
fn run(w: &Workload) -> Vec<Duration> {
    let mut rt = Runtime::with_document(Document::new(&(w.text)()), w.profile).expect("the editor starts");
    // No deadline for the snapshot's parts: under valgrind they run far
    // slower than it, and a part past it is not called again, so counts
    // depended on the time it took. A slow part shows as a slow key.
    rt.set_part_deadline(Duration::from_secs(3600));
    // The syntax layer's per-snapshot budget is a keystroke's, not a
    // profiling run's: the same exemption — and with it in place the
    // coloured workloads below can in fact colour under Callgrind,
    // instead of aborting their one collection and staying plain.
    rt.set_syntax_budget(Duration::from_secs(3600));
    if !w.eval.is_empty() {
        rt.eval(w.eval).expect("the workload's Lisp");
    }
    for k in w.setup.split_whitespace() {
        press(&mut rt, k);
    }
    if w.coloured {
        let mut landed = false;
        for _ in 0..2000 {
            if !rt.snapshot().pane().layers.is_empty() {
                landed = true;
                break;
            }
            let _ = rt.run_tasks(Duration::from_millis(5));
        }
        assert!(landed, "{}: highlights never landed", w.name);
    }
    timed_keys(&mut rt, w.timed)
}

/// Type `keys`, timing each: what the benchmark counts.
#[inline(never)]
fn timed_keys(rt: &mut Runtime, keys: &str) -> Vec<Duration> {
    keys.split_whitespace().map(|k| press(rt, k)).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--list") {
        WORKLOADS.iter().for_each(|w| println!("{}", w.name));
        return;
    }
    if let Some(name) = args.first() {
        let w = WORKLOADS.iter().find(|w| w.name == name).unwrap_or_else(|| panic!("no workload {name}"));
        // The number of keys timed, for dividing instructions by.
        println!("{}", run(w).len());
        return;
    }
    let ms = |d: Duration| format!("{:.2}", d.as_secs_f64() * 1000.0);
    println!("{:<18} {:>5} {:>8} {:>8} {:>8}  (ms, key to snapshot)", "workload", "keys", "p50", "p99", "max");
    for w in WORKLOADS {
        let mut times: Vec<Duration> = (0..ROUNDS).flat_map(|_| run(w)).collect();
        times.sort();
        let at = |q: f64| times[((q * times.len() as f64).ceil() as usize).clamp(1, times.len()) - 1];
        let over = if at(1.0) > BUDGET { "  over budget" } else { "" };
        println!("{:<18} {:>5} {:>8} {:>8} {:>8}{over}", w.name, times.len(), ms(at(0.5)), ms(at(0.99)), ms(at(1.0)));
    }
}
