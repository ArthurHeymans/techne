//! Scheme test suites, each run by the `techne-vm` binary in several modes.
//!
//! - `lang/...`: our own suites, `tests/suites/lang/*.scm`.
//! - `r7rs/...`: chibi-scheme's R7RS conformance suite, one trial per
//!   section, from the file named by `TECHNE_R7RS_TESTS`.
//! - `bench/...`: the programs of ecraven's r7rs-benchmarks, run once each
//!   and checked by their own result predicate, from the checkout named by
//!   `TECHNE_R7RS_BENCHMARKS`.
//!
//! The Nix dev shell sets both variables to pinned upstream sources; without
//! them those suites are skipped. Suites use the test API in
//! `tests/suites/test.scm`; every top-level form is read and evaluated on its
//! own, so a form the reader rejects fails alone.
//!
//! Modes: `default`, `interp` (`TECHNE_JIT=0`), `jit` (everything compiled
//! before it runs) and `gc` (a collection on every allocation); select with
//! `TECHNE_TEST_MODES=default,jit`. Every mode must give the same results.
//!
//! `tests/suites/expected-failures.txt` lists what fails today. A suite
//! fails when a test fails that is not listed, or a listed one passes, so the
//! list only shrinks deliberately. `TECHNE_BLESS=1` rewrites it from the
//! results of this run.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

use libtest_mimic::{Arguments, Failed, Trial};
use wait_timeout::ChildExt;

const MODES: &[(&str, &[(&str, &str)])] =
    &[("default", &[]), ("interp", &[("TECHNE_JIT", "0")]), ("jit", &[("TECHNE_JIT", "1")]), ("gc", &[("TECHNE_GC_STRESS", "1")])];

/// A program to run and how to judge its output.
#[derive(Clone)]
struct Suite {
    /// `lang/printer`, `r7rs/6-2-numbers`, `bench/fib`.
    name: String,
    program: String,
    /// Working directory of the run.
    dir: PathBuf,
    kind: Kind,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// Reports tests through `test.scm`.
    Tests,
    /// An r7rs-benchmarks program: passes when it prints its result line.
    Benchmark,
}

/// What a run produced: failed test ids (`F.N`, or `abort`/`fail` for the
/// whole suite) with a description, and the ids that passed.
#[derive(Default)]
struct Outcome {
    failed: BTreeMap<String, String>,
    passed: BTreeSet<String>,
}

type Expected = BTreeMap<String, BTreeMap<String, String>>;

fn main() {
    let args = Arguments::from_args();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/suites");
    let expected_path = root.join("expected-failures.txt");
    let expected = Arc::new(parse_expected(&fs::read_to_string(&expected_path).unwrap_or_default()));
    let bless = std::env::var_os("TECHNE_BLESS").is_some();
    let results: Arc<Mutex<Expected>> = Arc::default();

    let selected: Vec<_> = match std::env::var("TECHNE_TEST_MODES") {
        Ok(list) => MODES.iter().filter(|(m, _)| list.split(',').any(|s| s == *m)).collect(),
        Err(_) => MODES.iter().collect(),
    };
    let shim = fs::read_to_string(root.join("test.scm")).unwrap();

    let mut suites = lang_suites(&root.join("lang"), &shim);
    let mut skipped = Vec::new();
    match std::env::var_os("TECHNE_R7RS_TESTS") {
        Some(file) => suites.extend(r7rs_suites(Path::new(&file), &shim)),
        None => skipped.push("r7rs (TECHNE_R7RS_TESTS is not set)"),
    }
    match std::env::var_os("TECHNE_R7RS_BENCHMARKS") {
        Some(dir) => suites.extend(benchmark_suites(Path::new(&dir))),
        None => skipped.push("bench (TECHNE_R7RS_BENCHMARKS is not set)"),
    }

    let trials = suites
        .into_iter()
        .flat_map(|suite| selected.iter().map(move |mode| (suite.clone(), mode)))
        // A collection per allocation makes the benchmarks too slow.
        .filter(|(suite, (mode, _))| !(suite.kind == Kind::Benchmark && *mode == "gc"))
        .map(|(suite, (mode, env))| {
            let (expected, results) = (expected.clone(), results.clone());
            let record = bless && *mode == selected[0].0;
            Trial::test(suite.name.clone(), move || run_trial(&suite, env, &expected, record.then_some(&*results))).with_kind(*mode)
        })
        .chain(skipped.into_iter().map(|name| Trial::test(name, || Ok(())).with_ignored_flag(true)))
        .collect();

    let conclusion = libtest_mimic::run(&args, trials);
    let _ = fs::remove_dir_all(temp_dir());
    if bless {
        let ran = results.lock().unwrap();
        // Keep the entries of suites that did not run.
        let mut merged: Expected = expected.iter().filter(|(s, _)| !ran.contains_key(*s)).map(|(s, f)| (s.clone(), f.clone())).collect();
        merged.extend(ran.iter().filter(|(_, f)| !f.is_empty()).map(|(s, f)| (s.clone(), f.clone())));
        fs::write(&expected_path, render_expected(&merged)).unwrap();
        eprintln!("wrote {}", expected_path.display());
        return;
    }
    conclusion.exit();
}

fn run_trial(suite: &Suite, env: &[(&str, &str)], expected: &Expected, record: Option<&Mutex<Expected>>) -> Result<(), Failed> {
    let outcome = run(suite, env)?;
    if let Some(results) = record {
        results.lock().unwrap().insert(suite.name.clone(), outcome.failed.clone());
    }
    let none = BTreeMap::new();
    let known = expected.get(&suite.name).unwrap_or(&none);
    let aborted = outcome.failed.contains_key("abort");
    let new: Vec<_> = outcome.failed.iter().filter(|(id, _)| !known.contains_key(*id)).map(|(id, why)| format!("  {id} {why}")).collect();
    let fixed: Vec<_> = known
        .keys()
        .filter(|id| !outcome.failed.contains_key(*id) && (outcome.passed.contains(*id) || !aborted))
        .map(|id| format!("  {id}"))
        .collect();
    if new.is_empty() && fixed.is_empty() {
        return Ok(());
    }
    let mut msg = String::new();
    if !new.is_empty() {
        msg += &format!("failures not in expected-failures.txt:\n{}\n", new.join("\n"));
    }
    if !fixed.is_empty() {
        msg += &format!("expected to fail but did not (remove from expected-failures.txt):\n{}\n", fixed.join("\n"));
    }
    Err(msg.into())
}

/// Runs the suite's program and collects its results.
fn run(suite: &Suite, env: &[(&str, &str)]) -> Result<Outcome, Failed> {
    let tmp = temp_dir();
    fs::create_dir_all(&tmp).unwrap();
    let base = tmp.join(format!("{}-{}", suite.name.replace('/', "-"), env.iter().map(|(k, v)| format!("{k}{v}")).collect::<String>()));
    let (file, out, err) = (base.with_extension("scm"), base.with_extension("out"), base.with_extension("err"));
    fs::write(&file, &suite.program).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_techne-vm"))
        .arg(&file)
        .current_dir(&suite.dir)
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .stdout(fs::File::create(&out).unwrap())
        .stderr(fs::File::create(&err).unwrap())
        .spawn()
        .unwrap();
    let default_timeout = if suite.kind == Kind::Benchmark { 120 } else { 30 };
    let timeout: u64 = std::env::var("TECHNE_SUITE_TIMEOUT").ok().and_then(|s| s.parse().ok()).unwrap_or(default_timeout);
    let finished = child.wait_timeout(Duration::from_secs(timeout)).unwrap().is_some();
    if !finished {
        let _ = child.kill();
        let _ = child.wait();
    }
    let stdout = String::from_utf8_lossy(&fs::read(&out).unwrap()).into_owned();
    // Error messages name the generated file; keep descriptions stable.
    let stderr = String::from_utf8_lossy(&fs::read(&err).unwrap()).replace(&format!("{}/", tmp.display()), "");
    let why = || {
        if finished { first_line(&stderr) } else { format!("timed out after {timeout} s") }
    };
    Ok(match suite.kind {
        Kind::Benchmark if stdout.contains("+!CSVLINE!+") => Outcome::default(),
        Kind::Benchmark => Outcome { failed: [("fail".to_owned(), why())].into(), passed: BTreeSet::new() },
        Kind::Tests => {
            let mut outcome = Outcome::default();
            for line in stdout.lines() {
                if let Some(rest) = line.strip_prefix(";;;test pass ") {
                    outcome.passed.insert(rest.trim().to_owned());
                } else if let Some(rest) = line.strip_prefix(";;;test fail ") {
                    let (id, what) = rest.split_once(' ').unwrap_or((rest, ""));
                    outcome.failed.insert(id.to_owned(), truncate(what, 100));
                }
            }
            if !stdout.lines().any(|l| l == ";;;test end") {
                let last = stdout.lines().rev().find_map(|l| l.strip_prefix(";;;test ")?.split(' ').nth(1));
                let after = last.map(|id| format!(" (after {id})")).unwrap_or_default();
                outcome.failed.insert("abort".to_owned(), truncate(&format!("{}{after}", why()), 100));
            }
            outcome
        }
    })
}

/// Where the generated programs and their output go during this run.
fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!("techne-suites-{}", std::process::id()))
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").to_owned()
}

/// At most `n` characters of `s` on one line, control characters escaped.
fn truncate(s: &str, n: usize) -> String {
    let s: String = s.chars().flat_map(|c| if c.is_control() { c.escape_default().collect() } else { vec![c] }).collect();
    if s.chars().count() <= n { s } else { s.chars().take(n).collect::<String>() + "..." }
}

// ----- suites -----

fn lang_suites(dir: &Path, shim: &str) -> Vec<Suite> {
    let mut files: Vec<_> =
        fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "scm")).collect();
    files.sort();
    files
        .iter()
        .map(|path| Suite {
            name: format!("lang/{}", path.file_stem().unwrap().to_string_lossy()),
            program: forms_program(shim, &top_level_forms(&fs::read_to_string(path).unwrap())),
            dir: dir.to_owned(),
            kind: Kind::Tests,
        })
        .collect()
}

/// One suite per `(test-begin "...")` section; `import` forms are dropped
/// (they import the standard libraries, which are always visible) and
/// sections without tests skipped.
fn r7rs_suites(file: &Path, shim: &str) -> Vec<Suite> {
    let src = fs::read_to_string(file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    let mut sections: Vec<(String, Vec<&str>)> = Vec::new();
    for form in top_level_forms(&src) {
        if let Some(rest) = form.strip_prefix("(test-begin \"") {
            sections.push((rest.split('"').next().unwrap().to_owned(), Vec::new()));
        } else if let Some((_, forms)) = sections.last_mut()
            && !form.starts_with("(import")
        {
            forms.push(form);
        }
    }
    sections
        .into_iter()
        .filter(|(_, forms)| forms.iter().any(|f| f.starts_with("(test")))
        .map(|(title, forms)| Suite {
            name: format!("r7rs/{}", slug(&title)),
            program: forms_program(shim, &forms),
            dir: file.parent().unwrap().to_owned(),
            kind: Kind::Tests,
        })
        .collect()
}

/// The programs with an input file, each run once with the harness's
/// common code, as its `bench` script assembles them.
fn benchmark_suites(dir: &Path) -> Vec<Suite> {
    let mut names: Vec<String> = fs::read_dir(dir.join("inputs"))
        .unwrap()
        .filter_map(|e| e.unwrap().file_name().to_string_lossy().strip_suffix(".input").map(str::to_owned))
        .filter(|n| dir.join("src").join(format!("{n}.scm")).exists())
        .collect();
    names.sort();
    let read = |p: &str| fs::read_to_string(dir.join(p)).unwrap();
    let common = read("src/common.scm") + &read("src/common-postlude.scm");
    let work = bench_workdir(dir);
    names
        .into_iter()
        .map(|name| {
            // The input is: iteration count, parameters, expected result,
            // with comments. Run one iteration.
            let input = read(&format!("inputs/{name}.input"));
            let first = input.lines().position(|l| !l.trim().is_empty() && !l.trim_start().starts_with(';')).unwrap_or(0);
            let input: String = input
                .lines()
                .enumerate()
                .map(|(i, l)| {
                    if i == first { format!("1 {}", l.trim_start().trim_start_matches(|c: char| c.is_ascii_digit())) } else { l.to_owned() }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let program = format!(
                "{BENCH_PRELUDE}(define %bench-input (open-input-string {}))\n{}\n{common}",
                scheme_string(&input),
                read(&format!("src/{name}.scm")).replace("(read)", "(read %bench-input)"),
            );
            Suite { name: format!("bench/{name}"), program, dir: work.clone(), kind: Kind::Benchmark }
        })
        .collect()
}

/// Benchmarks write to `outputs/` next to their inputs, and the source tree
/// may be read-only (the Nix store): they run in a temporary directory that
/// links to everything else.
fn bench_workdir(dir: &Path) -> PathBuf {
    let work = temp_dir().join("bench-work");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(work.join("outputs")).unwrap();
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() != "outputs" {
            std::os::unix::fs::symlink(entry.path(), work.join(entry.file_name())).unwrap();
        }
    }
    work
}

/// What r7rs-benchmarks expects from each implementation's prelude.
const BENCH_PRELUDE: &str = r#"(define (this-scheme-implementation-name) "techne")
(define (flush-output-port . port) (flush-output))
(define (current-jiffy) (current-milliseconds))
(define (jiffies-per-second) 1000)
(define (current-second) (/ (current-milliseconds) 1000.))
"#;

fn forms_program(shim: &str, forms: &[&str]) -> String {
    let forms: Vec<String> = forms.iter().map(|f| scheme_string(f)).collect();
    format!("{shim}\n(%test-forms (list\n{}))\n", forms.join("\n"))
}

fn scheme_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn slug(s: &str) -> String {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join("-")
}

// ----- splitting source into top-level forms -----

/// The source text of each top-level datum, comments between them dropped.
/// Only lexical structure matters: strings, `|symbols|`, characters,
/// comments and brackets, so that unsupported syntax inside a form still
/// splits correctly.
fn top_level_forms(src: &str) -> Vec<&str> {
    let b = src.as_bytes();
    let mut forms = Vec::new();
    let mut i = skip_atmosphere(b, 0);
    while i < b.len() {
        let end = datum_end(b, i);
        forms.push(&src[i..end]);
        i = skip_atmosphere(b, end);
    }
    forms
}

/// Skips whitespace and comments other than `#;`.
fn skip_atmosphere(b: &[u8], mut i: usize) -> usize {
    loop {
        match b.get(i) {
            Some(c) if c.is_ascii_whitespace() => i += 1,
            Some(b';') => i = line_end(b, i),
            Some(b'#') if b.get(i + 1) == Some(&b'|') => i = block_comment_end(b, i),
            _ => return i,
        }
    }
}

fn line_end(b: &[u8], i: usize) -> usize {
    b[i..].iter().position(|&c| c == b'\n').map_or(b.len(), |p| i + p + 1)
}

/// The end of a (nesting) `#| ... |#` comment starting at `i`.
fn block_comment_end(b: &[u8], mut i: usize) -> usize {
    let mut depth = 0;
    while i < b.len() {
        if b[i..].starts_with(b"#|") {
            depth += 1;
            i += 2;
        } else if b[i..].starts_with(b"|#") {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else {
            i += 1;
        }
    }
    b.len()
}

/// The end of the datum starting at `i` (prefixes such as `'`, `#;` and
/// `#u8` included).
fn datum_end(b: &[u8], i: usize) -> usize {
    match b[i] {
        b'(' | b'[' => {
            let mut j = i + 1;
            loop {
                j = skip_atmosphere(b, j);
                match b.get(j) {
                    None => return b.len(),
                    Some(b')' | b']') => return j + 1,
                    Some(_) => j = datum_end(b, j),
                }
            }
        }
        b'"' | b'|' => delimited_end(b, i),
        b'\'' | b'`' => next_datum_end(b, i + 1),
        b',' => next_datum_end(b, if b.get(i + 1) == Some(&b'@') { i + 2 } else { i + 1 }),
        b'#' => match b.get(i + 1) {
            Some(b';') => {
                let end = next_datum_end(b, i + 2);
                next_datum_end(b, end)
            }
            Some(b'\\') => atom_end(b, (i + 3).min(b.len())),
            Some(b'(') => datum_end(b, i + 1),
            _ => {
                // #u8( ... ), #0=( ... ), #true, #x1F and the like.
                let end = atom_end(b, i + 1);
                if b.get(end) == Some(&b'(') { datum_end(b, end) } else { end }
            }
        },
        _ => atom_end(b, i),
    }
}

fn next_datum_end(b: &[u8], i: usize) -> usize {
    let j = skip_atmosphere(b, i);
    if j < b.len() { datum_end(b, j) } else { j }
}

/// The end of a string or `|symbol|` starting at `i`.
fn delimited_end(b: &[u8], i: usize) -> usize {
    let close = b[i];
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            c if c == close => return j + 1,
            _ => j += 1,
        }
    }
    b.len()
}

fn atom_end(b: &[u8], mut i: usize) -> usize {
    while i < b.len() {
        match b[i] {
            b'|' => i = delimited_end(b, i),
            c if c.is_ascii_whitespace() || b"()[]\";'`,".contains(&c) => return i,
            _ => i += 1,
        }
    }
    i
}

// ----- expected failures -----

fn parse_expected(text: &str) -> Expected {
    let mut expected = Expected::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let mut parts = line.splitn(3, ' ');
        let (suite, id) = (parts.next().unwrap(), parts.next().unwrap_or("abort"));
        expected.entry(suite.to_owned()).or_default().insert(id.to_owned(), parts.next().unwrap_or("").to_owned());
    }
    expected
}

fn render_expected(expected: &Expected) -> String {
    let header = "\
# Tests that fail today, as `suite id description` (see tests/suites.rs).
# An id is F.N for the Nth test of the Fth top-level form (F.0: the form
# failed outside a test), `abort` when the suite did not run to its end and
# `fail` for a benchmark. Regenerate with TECHNE_BLESS=1 cargo test --test suites.
";
    let lines = expected
        .iter()
        .flat_map(|(suite, failures)| failures.iter().map(move |(id, why)| format!("{suite} {id} {why}").trim_end().to_owned()));
    let mut lines: Vec<_> = lines.collect();
    lines.sort_by_key(|l| natural_key(l));
    format!("{header}\n{}\n", lines.join("\n"))
}

/// Sorts `r7rs/x 10.1` after `r7rs/x 9.2`.
fn natural_key(line: &str) -> (String, Vec<u64>, String) {
    let mut parts = line.splitn(3, ' ');
    let suite = parts.next().unwrap_or("").to_owned();
    let id = parts.next().unwrap_or("");
    (suite, id.split('.').filter_map(|n| n.parse().ok()).collect(), id.to_owned())
}
