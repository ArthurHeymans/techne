//! Random keys for the editor (PLAN.md, Stage 1, slice 7.3): seeded key
//! sequences drawn from the keys the session binds, and characters, over a
//! corpus of documents and states, both key profiles.
//!
//! A trace fails when the runtime records an error no command raised (of an
//! input outside its command, of a part of a snapshot) or a command or hook
//! raises a violation, a mistake in the editor's code (`Runtime::take_errors`);
//! when it crashes; or when it does not end in time. What the user's code
//! raises, a command evaluating it shows, and it fails nothing. Each trace runs
//! in a process of its own, its keys logged as they go, so a crash or a hang
//! still says which key it was; a failing trace is shrunk to a short sequence
//! failing the same way, printed as a `replay` command.
//!
//! - `keyfuzz fuzz [--seeds A-B] [--keys N] [--corpus NAME]` runs traces and
//!   fails on the first failure; keys slower than ten times the keystroke
//!   budget in wall time are listed as candidates for `count`.
//! - `keyfuzz count [--seeds A-B] [--keys N] [--corpus NAME] [--shrink]` runs
//!   each trace under callgrind, counting the instructions of each key (as
//!   `runtime/bench/icount.sh` does for `keys`), a collection made before
//!   each, and fails listing the keys over the budget of 28 M, but those
//!   running the user's code;
//!   with `--shrink`, the first one's prefix is shrunk while it stays over:
//!   what remains is a workload to add to `keys`.
//! - `keyfuzz replay CORPUS KEY...` presses the keys, the last one counted
//!   under callgrind (`last_key`).
//! - `keyfuzz --list` names the corpus.
//!
//! The runtime is driven synchronously: background tasks are not run between
//! keys and snapshot parts have no deadline, so that a trace replays the same
//! way, under callgrind too, where the JIT compiles synchronously
//! (`TECHNE_JIT_SYNC`), on its thread, which callgrind does not count. Its
//! world is not granted files, processes, the network, the environment or the
//! console, loading is taken back once the editor has loaded, and its HOME is
//! a directory of its own: random keys reach nothing outside it.

use std::{
    fmt::Write as _,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use techne_editor::{present::Input, runtime::Runtime};
use techne_text::Document;
use techne_vm::vm::{Capability, Grants};
use wait_timeout::ChildExt;

/// Instructions a key may take: some 4 ms on the daily hardware, as in
/// `runtime/bench/icount.sh`.
const KEYS_BUDGET: u64 = 28_000_000;

/// Wall time past which a key is a candidate for counting: ten times the
/// keystroke budget.
const SLOW: Duration = Duration::from_millis(40);

/// How long a trace may run before it counts as hung, and under callgrind.
const HANG: Duration = Duration::from_secs(120);
const HANG_COUNTED: Duration = Duration::from_secs(3600);

/// Keys over the budget that are known, each with its plan (PLAN.md,
/// Stage 1, slice 7): (corpus, `*` for any, command, why). Listed, not
/// failed on.
const KNOWN_SLOW: &[(&str, &str, &str)] = &[
    // As on main: the minibuffer's input matched against a 1 MB line.
    ("long-line", "search-all-buffers", "matching a line of a megabyte"),
    // 0.3% over, in one random 62-key state of the corpus.
    ("minibuffer", "completion-at-point", "at the budget's edge"),
    // As on main: indenting after an open bracket scans the lines above
    // (the `keys` workload indent-after-long-line).
    ("lines", "indent-for-tab-command", "indenting scans back over the text"),
    // In any corpus: help listing every name, more of them with each
    // feature (some 39 M on CI's runner).
    ("*", "describe-function", "listing every name"),
    // In any corpus (`*`): the first definition motion of a session
    // compiles its language's definitions query (some 23 M), kept after.
    ("*", "beginning-of-defun", "the definitions query compiled on first use"),
    ("*", "end-of-defun", "the definitions query compiled on first use"),
    ("*", "mark-defun", "the definitions query compiled on first use"),
];

/// Probes a shrink may make, at most.
const PROBES: usize = 200;
const PROBES_COUNTED: usize = 30;

struct Corpus {
    name: &'static str,
    profile: &'static str,
    text: fn() -> String,
    /// Lisp evaluated before the keys, with `(current-session)` the session.
    setup_lisp: &'static str,
    /// Keys pressed before the random ones.
    setup_keys: &'static str,
}

const MANY_BUFFERS: &str = "(do ((i 0 (+ i 1))) ((= i 40))
  (make-generated-buffer! (string-append \"b\" (number->string i)) (make-document (make-string (* i 50) #\\x)) 'scheme-mode))";

const CORPUS: &[Corpus] = &[
    Corpus { name: "lines", profile: "emacs", text: lines, setup_lisp: "", setup_keys: "" },
    Corpus { name: "lines-modal", profile: "modal", text: lines, setup_lisp: "", setup_keys: "" },
    Corpus { name: "long-line", profile: "emacs", text: long_line, setup_lisp: "", setup_keys: "C-n" },
    Corpus { name: "long-line-modal", profile: "modal", text: long_line, setup_lisp: "", setup_keys: "j" },
    Corpus { name: "unicode", profile: "emacs", text: unicode, setup_lisp: "", setup_keys: "" },
    Corpus { name: "unicode-modal", profile: "modal", text: unicode, setup_lisp: "", setup_keys: "" },
    Corpus { name: "scheme", profile: "emacs", text: scheme, setup_lisp: "", setup_keys: "" },
    Corpus { name: "scheme-modal", profile: "modal", text: scheme, setup_lisp: "", setup_keys: "" },
    Corpus { name: "buffers", profile: "emacs", text: scheme, setup_lisp: MANY_BUFFERS, setup_keys: "" },
    Corpus { name: "buffers-modal", profile: "modal", text: scheme, setup_lisp: MANY_BUFFERS, setup_keys: "" },
    Corpus { name: "minibuffer", profile: "emacs", text: lines, setup_lisp: "", setup_keys: "M-x" },
    Corpus { name: "minibuffer-modal", profile: "modal", text: lines, setup_lisp: "", setup_keys: "SPC :" },
    Corpus { name: "search", profile: "emacs", text: lines, setup_lisp: "", setup_keys: "C-c s s a" },
    Corpus { name: "search-modal", profile: "modal", text: lines, setup_lisp: "", setup_keys: "SPC s s a" },
    Corpus { name: "search-line-modal", profile: "modal", text: lines, setup_lisp: "", setup_keys: "/ a" },
];

/// 100,000 lines of words, as `keys` makes them.
fn lines() -> String {
    let mut words = Rng(1);
    (0..100_000).map(|_| (0..words.below(15)).map(|_| words.word()).collect::<Vec<_>>().join(" ") + "\n").collect()
}

/// One line of 1 MB between two short ones.
fn long_line() -> String {
    let mut words = Rng(2);
    let mut line = String::new();
    while line.len() < 1 << 20 {
        line.push_str(words.word());
        line.push(' ');
    }
    format!("start\n{line}\nend\n")
}

/// Lines mixing scripts, combining marks, emoji sequences, right-to-left
/// text, tabs and control characters.
fn unicode() -> String {
    const PIECES: [&str; 12] =
        ["héllo", "e\u{301}\u{302}", "日本語のテキスト", "👩‍👩‍👧‍👦", "🇧🇪", "שלום עולם", "مرحبا", "\t", "Ǆ ǅ ǆ", "a\u{200d}b", "\u{feff}", "ﬁ"];
    let mut rng = Rng(3);
    (0..2000)
        .map(|_| (0..rng.below(8) + 1).map(|_| PIECES[rng.below(PIECES.len() as u64) as usize]).collect::<Vec<_>>().join(" ") + "\n")
        .collect()
}

/// Definitions, then one expression nested 500 deep.
fn scheme() -> String {
    let defs = "(define (f x)\n  (let ((y (* x 2)))\n    \"a \\\"string\\\" here\" ; a comment\n    (+ y 1)))\n#| block |#\n(define v #(1 2 #\\a))\n".repeat(200);
    format!("{defs}{}x{}\n", "(".repeat(500), ")".repeat(500))
}

/// A fixed linear congruential generator.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }

    fn word(&mut self) -> &'static str {
        const WORDS: [&str; 10] = ["alpha", "beta", "gamma", "delta", "println!", "fn", "let", "mut", "struct", "impl"];
        WORDS[self.below(10) as usize]
    }

    fn pick<'a>(&mut self, items: &'a [&'a str]) -> &'a str {
        items[self.below(items.len() as u64) as usize]
    }
}

/// Characters typed, besides the bound keys.
const CHARS: &[&str] =
    &["a", "e", "x", "z", "0", "7", "(", ")", "[", "]", "\"", "'", ";", "#", "\\", "|", ".", "-", "SPC", "é", "日", "🙂", "\u{301}", "ש"];
/// Keys pressed often whatever binds them.
const COMMON: &[&str] = &["RET", "DEL", "TAB", "C-g", "ESC", "<left>", "<right>", "<up>", "<down>"];

/// The keys of one random step: a bound sequence, a character or a common
/// key. Quitting is left out.
fn step(rng: &mut Rng, bound: Vec<String>) -> Vec<String> {
    match rng.below(10) {
        0..=2 => vec![rng.pick(CHARS).to_string()],
        3 => vec![rng.pick(COMMON).to_string()],
        _ => {
            let bound: Vec<String> = bound.into_iter().filter(|k| k != "C-x C-c").collect();
            if bound.is_empty() {
                return vec![rng.pick(CHARS).to_string()];
            }
            bound[rng.below(bound.len() as u64) as usize].split(' ').map(String::from).collect()
        }
    }
}

fn corpus(name: &str) -> &'static Corpus {
    CORPUS.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("no corpus {name}"))
}

/// A runtime over the corpus's document, set up, loading taken back.
fn start(c: &Corpus) -> Runtime {
    let grants = [
        Capability::Files,
        Capability::Processes,
        Capability::Network,
        Capability::HostControl,
        Capability::Environment,
        Capability::Console,
    ]
    .into_iter()
    .fold(Grants::ALL, Grants::without);
    let mut rt = Runtime::with_document_granted(Document::new(&(c.text)()), c.profile, grants).expect("the editor starts");
    // No deadline: under callgrind parts run far slower, and a part past it
    // is not called again, so that runs would differ. A hang is the
    // trace's timeout's.
    rt.set_part_deadline(Duration::from_secs(1 << 20));
    // Compile the probes before user code can shadow names such as `quote`.
    // Recompiling their bodies after an evaluation would use those bindings.
    rt.eval(
        "(define (keyfuzz-before-key)
           (sset! (current-session) 'ran-user-code #f) (collect-garbage))
         (define (keyfuzz-this-command) (sget (current-session) 'this-command))
         (define (keyfuzz-ran-user-code) (sget (current-session) 'ran-user-code))",
    )
    .expect("the session probes");
    if !c.setup_lisp.is_empty() {
        rt.eval(c.setup_lisp).unwrap_or_else(|e| panic!("{}: setup: {e}", c.name));
    }
    rt.set_grant(Capability::Loading, false);
    for k in c.setup_keys.split_whitespace() {
        press(&mut rt, k);
    }
    let errors = rt.take_errors();
    assert!(errors.is_empty(), "{}: setup failed: {errors:?}", c.name);
    rt
}

/// Press `key`, then take the snapshot that shows it; true if the session quit.
fn press(rt: &mut Runtime, key: &str) -> bool {
    let quit = rt.handle(Input::Key { key: key.into(), at: Instant::now() }).is_some();
    std::hint::black_box(rt.snapshot());
    quit
}

/// A key of a trace: what `count` counts, each key a dump of its own.
#[inline(never)]
fn timed_key(rt: &mut Runtime, key: &str) -> bool {
    press(rt, key)
}

/// The last key of a replay: what `count` counts when shrinking.
#[inline(never)]
fn last_key(rt: &mut Runtime, key: &str) -> bool {
    press(rt, key)
}

/// Where a child writes what it did: not its standard output, which the
/// editor's Lisp may write to.
struct Log(std::fs::File);

impl Log {
    fn line(&mut self, fields: &[&str]) {
        let line: Vec<String> = fields.iter().map(|f| f.replace(['\t', '\n'], " ")).collect();
        writeln!(self.0, "{}", line.join("\t")).expect("the log is written");
        self.0.flush().expect("the log is written");
    }

    /// After a key: fail on what went wrong, else go on.
    fn check(&mut self, rt: &mut Runtime) {
        let errors = rt.take_errors();
        if let Some(first) = errors.first() {
            self.line(&["fail", &signature(first), &errors.join(" | ")]);
            std::process::exit(1);
        }
    }
}

/// What a failure is, for shrinking to one failing the same way: its first
/// two parts ("pane-status: car"), not the values it names.
fn signature(error: &str) -> String {
    error.splitn(3, ": ").take(2).collect::<Vec<_>>().join(": ")
}

/// A child: a trace generated from a seed (`trace CORPUS SEED KEYS`) or keys
/// given (`replay CORPUS KEY...`), logged to `$KEYFUZZ_LOG`.
fn child(mode: &str, args: &[String]) {
    let mut log = Log(std::fs::File::create(std::env::var("KEYFUZZ_LOG").expect("KEYFUZZ_LOG")).expect("the log is created"));
    let c = corpus(&args[0]);
    let mut rt = start(c);
    log.line(&["started"]);
    if mode == "trace" {
        let seed: u64 = args[1].parse().expect("a seed");
        let count: usize = args[2].parse().expect("a number of keys");
        // Each corpus its own keys for a seed.
        let salt = c.name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ salt);
        let mut pressed = 0;
        while pressed < count {
            let mut keys = step(&mut rng, before_key(&mut rt)).into_iter().peekable();
            while let Some(k) = keys.next() {
                log.line(&["key", &k]);
                let start = Instant::now();
                let quit = timed_key(&mut rt, &k);
                log.line(&["took", &start.elapsed().as_micros().to_string()]);
                after_key(&mut rt, &mut log);
                pressed += 1;
                if quit {
                    return;
                }
                if keys.peek().is_some() {
                    before_key(&mut rt);
                }
            }
        }
    } else {
        let (last, prefix) = args[1..].split_last().expect("keys to replay");
        for k in prefix {
            before_key(&mut rt);
            log.line(&["key", k]);
            let quit = press(&mut rt, k);
            after_key(&mut rt, &mut log);
            if quit {
                return;
            }
        }
        before_key(&mut rt);
        log.line(&["key", last]);
        last_key(&mut rt, last);
        after_key(&mut rt, &mut log);
    }
}

/// What a trace asks of the runtime before each key, and a replay too, so
/// that both allocate the same. Returns the bindings, from which a trace
/// draws its keys. A collection then, so that one comes in a key only if
/// the key itself allocates for it: else a pause, which has a budget of its
/// own (REQUIREMENTS.md, judged by `runtime/bench/icount.sh`), comes at
/// whichever key the allocations of those before add up at.
fn before_key(rt: &mut Runtime) -> Vec<String> {
    let bound = rt.bindings();
    rt.eval("(keyfuzz-before-key)").expect("the session");
    bound
}

/// What a trace asks after each key, and a replay too: the command it ran,
/// whether it ran the user's code, whose time is not the budget's, and what
/// went wrong, which ends the run.
fn after_key(rt: &mut Runtime, log: &mut Log) {
    log.line(&["command", &rt.eval("(keyfuzz-this-command)").expect("the command probe")]);
    log.line(&["user", &rt.eval("(keyfuzz-ran-user-code)").expect("the user-code probe")]);
    log.check(rt);
}

/// What a child run came to.
#[derive(Debug, Default)]
struct Run {
    keys: Vec<String>,
    /// Microseconds each key took, for traces.
    took: Vec<u64>,
    /// The command each key ran, for traces: the last one run if it ran
    /// none.
    commands: Vec<String>,
    /// Whether each key ran the user's code, for traces.
    user: Vec<bool>,
    /// The failure's signature and what it says.
    failure: Option<(String, String)>,
}

/// Run a child (`trace` or `replay`), under callgrind with `counted`, in a
/// directory of its own as HOME. Callgrind's dumps go to `dumps`.
fn run(args: &[String], counted: Option<&Path>) -> Run {
    let dir = tempfile::tempdir().expect("a directory");
    let log = dir.path().join("log");
    let stderr = dir.path().join("stderr");
    let exe = std::env::current_exe().expect("this program");
    let mut cmd = match counted {
        Some(dumps) => {
            let mut cmd = Command::new("valgrind");
            cmd.args([
                "--tool=callgrind",
                "--toggle-collect=*timed_key*",
                "--toggle-collect=*last_key*",
                "--dump-after=keyfuzz::timed_key",
            ])
            .arg(format!("--callgrind-out-file={}", dumps.join("cg.out").display()))
            .arg(&exe);
            cmd
        }
        None => Command::new(&exe),
    };
    if counted.is_some() {
        // Compiling where it is asked for, on the compiler's thread, which
        // callgrind does not count: counts do not depend on when it ends.
        cmd.env("TECHNE_JIT_SYNC", "1");
    }
    cmd.args(args)
        .env("KEYFUZZ_LOG", &log)
        .env("HOME", dir.path())
        .env("XDG_STATE_HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .env("XDG_DATA_HOME", dir.path())
        .env("XDG_CACHE_HOME", dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&stderr).expect("stderr is created"));
    let mut child = cmd.spawn().expect("the child starts");
    let status = child.wait_timeout(if counted.is_some() { HANG_COUNTED } else { HANG }).expect("the child is waited for");
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let mut out = Run::default();
    for line in std::fs::read_to_string(&log).unwrap_or_default().lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields[..] {
            ["key", k] => out.keys.push(k.to_string()),
            ["took", us] => out.took.push(us.parse().unwrap_or(0)),
            ["command", name] => out.commands.push(name.to_string()),
            ["user", ran] => out.user.push(ran == "#t"),
            ["fail", sig, detail] => out.failure = Some((sig.to_string(), detail.to_string())),
            _ => {}
        }
    }
    if out.failure.is_none() {
        out.failure = match status {
            None => Some(("hang".into(), format!("no end after {:?}", if counted.is_some() { HANG_COUNTED } else { HANG }))),
            Some(s) if !s.success() => {
                let err = std::fs::read(&stderr).unwrap_or_default();
                let tail = String::from_utf8_lossy(&err[err.len().saturating_sub(2000)..]).into_owned();
                Some(("crash".into(), format!("{s}: {tail}")))
            }
            Some(_) => None,
        };
    }
    out
}

/// The shortest subsequence of `keys` found, within `probes` tries, for which
/// `fails` holds, by delta debugging (ddmin). `keys` fails.
fn shrink(mut keys: Vec<String>, probes: usize, mut fails: impl FnMut(&[String]) -> bool) -> Vec<String> {
    let (mut tried, mut chunks) = (0, 2);
    while keys.len() >= 2 && tried < probes {
        let size = keys.len().div_ceil(chunks);
        let smaller =
            (0..chunks).map(|i| (i * size, ((i + 1) * size).min(keys.len()))).filter(|(from, to)| from < to).find_map(|(from, to)| {
                if tried >= probes {
                    return None;
                }
                tried += 1;
                let rest: Vec<String> = keys[..from].iter().chain(&keys[to..]).cloned().collect();
                fails(&rest).then_some(rest)
            });
        match smaller {
            Some(rest) => (keys, chunks) = (rest, (chunks - 1).max(2)),
            None if chunks >= keys.len() => break,
            None => chunks = (chunks * 2).min(keys.len()),
        }
    }
    keys
}

fn replay_args(corpus: &str, keys: &[String]) -> Vec<String> {
    ["replay".to_string(), corpus.to_string()].into_iter().chain(keys.iter().cloned()).collect()
}

/// A key as a shell word.
fn quoted(k: &str) -> String {
    if k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') { k.to_string() } else { format!("'{}'", k.replace('\'', "'\\''")) }
}

fn command_line(corpus: &str, keys: &[String]) -> String {
    let mut line = format!("keyfuzz replay {corpus}");
    for k in keys {
        let _ = write!(line, " {}", quoted(k));
    }
    line
}

struct Options {
    seeds: std::ops::Range<u64>,
    keys: usize,
    corpus: Vec<&'static Corpus>,
    shrink: bool,
}

fn options(args: &[String]) -> Options {
    let mut o = Options { seeds: 0..4, keys: 200, corpus: CORPUS.iter().collect(), shrink: false };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--shrink" {
            o.shrink = true;
            continue;
        }
        let value = it.next().unwrap_or_else(|| panic!("{a} wants a value"));
        match a.as_str() {
            "--seeds" => {
                let (from, to) = value.split_once('-').unwrap_or((value, value));
                o.seeds = from.parse().expect("a seed")..to.parse::<u64>().expect("a seed") + 1;
            }
            "--keys" => o.keys = value.parse().expect("a number of keys"),
            "--corpus" => o.corpus = value.split(',').map(corpus).collect(),
            _ => panic!("unknown option {a}"),
        }
    }
    o
}

/// Run traces; shrink and report the first failure.
fn fuzz(o: &Options) -> bool {
    let mut candidates = Vec::new();
    for c in &o.corpus {
        for seed in o.seeds.clone() {
            let args = ["trace".to_string(), c.name.to_string(), seed.to_string(), o.keys.to_string()];
            let trace = run(&args, None);
            for (i, us) in trace.took.iter().enumerate() {
                if Duration::from_micros(*us) > SLOW {
                    candidates.push(format!("{} seed {seed} key {} ({}): {} ms", c.name, i + 1, trace.keys[i], us / 1000));
                }
            }
            let Some((sig, detail)) = trace.failure else { continue };
            println!("FAIL {} seed {seed} after {} keys: {detail}", c.name, trace.keys.len());
            let keys = shrink(trace.keys, PROBES, |keys| run(&replay_args(c.name, keys), None).failure.is_some_and(|(s, _)| s == sig));
            let again = run(&replay_args(c.name, &keys), None);
            println!("shrunk to {} keys: {}", keys.len(), again.failure.map(|f| f.1).unwrap_or_else(|| "(flaky)".into()));
            println!("  {}", command_line(c.name, &keys));
            return false;
        }
    }
    if !candidates.is_empty() {
        println!("keys slower than {} ms in wall time, for `keyfuzz count`:", SLOW.as_millis());
        candidates.iter().for_each(|c| println!("  {c}"));
    }
    true
}

/// The instructions of each key callgrind dumped in `dir`, in order.
fn counts(dir: &Path) -> Vec<u64> {
    let mut parts: Vec<(u64, u64)> = std::fs::read_dir(dir)
        .expect("the dumps")
        .filter_map(|e| {
            let path: PathBuf = e.ok()?.path();
            let n: u64 = path.extension()?.to_str()?.parse().ok()?;
            let text = std::fs::read_to_string(&path).ok()?;
            let total = text.lines().find_map(|l| l.strip_prefix("totals: ").or_else(|| l.strip_prefix("summary: ")))?;
            Some((n, total.trim().parse().ok()?))
        })
        .collect();
    parts.sort();
    parts.into_iter().map(|(_, total)| total).collect()
}

/// Instructions of the last key of `keys`, replayed under callgrind.
fn last_count(corpus: &str, keys: &[String]) -> Option<u64> {
    let dumps = tempfile::tempdir().expect("a directory");
    let replay = run(&replay_args(corpus, keys), Some(dumps.path()));
    if replay.failure.is_some() {
        return None;
    }
    // One file, at exit: what `last_key` ran.
    let text = std::fs::read_to_string(dumps.path().join("cg.out")).ok()?;
    text.lines().find_map(|l| l.strip_prefix("totals: ").or_else(|| l.strip_prefix("summary: ")))?.trim().parse().ok()
}

/// Count every key of the traces and report those over budget; with
/// `--shrink`, the first one's prefix is shrunk while it stays over.
fn count(o: &Options) -> bool {
    let mut over = Vec::new();
    for c in &o.corpus {
        for seed in o.seeds.clone() {
            let dumps = tempfile::tempdir().expect("a directory");
            let args = ["trace".to_string(), c.name.to_string(), seed.to_string(), o.keys.to_string()];
            let trace = run(&args, Some(dumps.path()));
            if let Some((_, detail)) = &trace.failure {
                println!("FAIL {} seed {seed}, counted: {detail}", c.name);
                return false;
            }
            let counts = counts(dumps.path());
            assert_eq!(counts.len(), trace.keys.len(), "{} seed {seed}: a dump for each key", c.name);
            let max = counts.iter().max().copied().unwrap_or(0);
            println!("{:<18} seed {seed}: {} keys, at most {max} instructions per key", c.name, counts.len());
            for (i, n) in counts.into_iter().enumerate().filter(|&(i, n)| n > KEYS_BUDGET && !trace.user[i]) {
                let known =
                    KNOWN_SLOW.iter().any(|&(corpus, command, _)| (corpus == "*" || corpus == c.name) && command == trace.commands[i]);
                println!(
                    "  key {} ({}, {}): {n} instructions{}",
                    i + 1,
                    trace.keys[i],
                    trace.commands[i],
                    if known { " (known)" } else { "" }
                );
                if !known {
                    over.push((c.name, trace.keys[..=i].to_vec()));
                }
            }
        }
    }
    if over.is_empty() {
        return true;
    }
    println!("{} keys over the budget of {KEYS_BUDGET} instructions", over.len());
    if o.shrink {
        let (corpus, keys) = &over[0];
        let (last, prefix) = keys.split_last().expect("a key");
        let prefix = shrink(prefix.to_vec(), PROBES_COUNTED, |prefix| {
            let keys: Vec<String> = prefix.iter().chain([last]).cloned().collect();
            last_count(corpus, &keys).is_some_and(|n| n > KEYS_BUDGET)
        });
        let keys: Vec<String> = prefix.into_iter().chain([last.clone()]).collect();
        println!("the first, shrunk to {} keys:", keys.len());
        println!("  {}", command_line(corpus, &keys));
    }
    false
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ok = match args.first().map(String::as_str) {
        Some("--list") => {
            CORPUS.iter().for_each(|c| println!("{}", c.name));
            true
        }
        // A child, which its parent gives a log.
        Some(mode @ ("trace" | "replay")) if std::env::var_os("KEYFUZZ_LOG").is_some() => {
            child(mode, &args[1..]);
            true
        }
        Some("replay") => {
            let r = run(&args, None);
            println!("{} keys: {}", r.keys.len(), r.failure.as_ref().map_or("no failure", |f| &f.1));
            r.failure.is_none()
        }
        Some("fuzz") => fuzz(&options(&args[1..])),
        Some("count") => count(&options(&args[1..])),
        _ => {
            eprintln!("usage: keyfuzz fuzz|count [--seeds A-B] [--keys N] [--corpus NAME,...] [--shrink] | replay CORPUS KEY... | --list");
            false
        }
    };
    std::process::exit(if ok { 0 } else { 1 });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_survive_user_code_shadowing_quote() {
        let mut rt = start(corpus("scheme-modal"));
        rt.eval("(define (quote x) x)").unwrap();
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut log = Log(file.reopen().unwrap());
        assert!(!before_key(&mut rt).is_empty());
        after_key(&mut rt, &mut log);
        let text = std::fs::read_to_string(file.path()).unwrap();
        assert!(text.lines().any(|l| l == "user\t#f"), "{text}");
    }
}
