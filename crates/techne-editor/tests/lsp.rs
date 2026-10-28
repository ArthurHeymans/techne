//! The language server client against two servers: a fake that says
//! one diagnostic and one definition, and techne's own (techne-lsp on
//! a Scheme file with an unbound name).

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use techne_editor::{
    present::{Input, Snapshot, Underline},
    runtime::Runtime,
};

/// A language server in Python: answers initialize, publishes an
/// error over the first word when a file opens, and definitions at it.
const FAKE: &str = r#"
import sys, json

def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line: return None
        if line.strip() == b'':
            break
        if line.lower().startswith(b'content-length:'):
            length = int(line.split(b':')[1])
    return json.loads(sys.stdin.buffer.read(length))

def send(value):
    body = json.dumps(value).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(body) + body)
    sys.stdout.buffer.flush()

while True:
    msg = read()
    if msg is None: break
    method = msg.get('method')
    if method == 'initialize':
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': {'capabilities': {}}})
    elif method == 'textDocument/didOpen':
        uri = msg['params']['textDocument']['uri']
        send({'jsonrpc': '2.0', 'method': 'textDocument/publishDiagnostics',
              'params': {'uri': uri, 'diagnostics': [
                  {'range': {'start': {'line': 0, 'character': 0}, 'end': {'line': 0, 'character': 2}},
                   'severity': 1, 'message': 'bad fn'}]}})
    elif method == 'textDocument/definition':
        uri = msg['params']['textDocument']['uri']
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result':
              {'uri': uri, 'range': {'start': {'line': 0, 'character': 3}, 'end': {'line': 0, 'character': 7}}}})
    elif method == 'shutdown':
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': None})
    elif 'id' in msg:
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': None})
"#;

fn open(dir: &Path, name: &str, text: &str) -> (Runtime, PathBuf) {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    let (rt, _) = Runtime::open(&path, &dir.join("x.journal"), "emacs").unwrap();
    (rt, path)
}

/// Snapshots until `done`, running the editor's tasks between them;
/// the last one either way.
fn until(rt: &mut Runtime, done: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let start = Instant::now();
    loop {
        let s = rt.snapshot();
        if done(&s) || start.elapsed() > Duration::from_secs(30) {
            return s;
        }
        rt.run_tasks(Duration::from_millis(20)).unwrap();
    }
}

fn diagnostics_shown(s: &Snapshot) -> bool {
    s.pane().layers.iter().any(|h| matches!(s.style(h.style).underline, Some((Underline::Curl, _))))
}

fn diagnostic_ranges(s: &Snapshot) -> Vec<(usize, usize)> {
    let mut ranges: Vec<_> = s
        .pane()
        .layers
        .iter()
        .filter(|h| matches!(s.style(h.style).underline, Some((Underline::Curl, _))))
        .map(|h| (h.from, h.to))
        .collect();
    ranges.sort_unstable();
    ranges.dedup();
    ranges
}

/// A fake like `FAKE`, tuned per test: its diagnostic covers
/// characters `start..end` of the first line, it can stamp the
/// publication with the version it was sent, speak in lower-case
/// headers, flood stderr before reading anything, refuse definitions
/// for documents it has not been sent, and count its starts.
struct FakeSpec {
    start: usize,
    end: usize,
    versioned: bool,
    lower: bool,
    noisy: bool,
    opened_only: bool,
    count_in: Option<PathBuf>,
    /// Copies of the one diagnostic per publication.
    many: usize,
    /// A file written on didOpen, so a test can act while the
    /// publication that follows it is being read.
    mark_open_in: Option<PathBuf>,
    /// Write frames a byte at a time, flushed.
    fragment: bool,
    /// Pad the diagnostic's message by this many bytes.
    pad: usize,
    /// Refuse the first initialize (marking it in this file);
    /// later starts answer normally.
    fail_first_in: Option<PathBuf>,
    /// Publish under the URI as a URL library would respell it
    /// (`+` for the `%2B` it was sent).
    normalize_uri: bool,
}

impl Default for FakeSpec {
    fn default() -> Self {
        Self {
            start: 0,
            end: 2,
            versioned: false,
            lower: false,
            noisy: false,
            opened_only: false,
            count_in: None,
            many: 1,
            mark_open_in: None,
            fragment: false,
            pad: 0,
            fail_first_in: None,
            normalize_uri: false,
        }
    }
}

fn write_fake(dir: &Path, name: &str, spec: &FakeSpec) -> PathBuf {
    let path = dir.join(name);
    let script = r#"
import sys, json
NOISY
opened = set()
def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line: return None
        if line.strip() == b'': break
        if line.lower().startswith(b'content-length:'):
            length = int(line.split(b':')[1])
    return json.loads(sys.stdin.buffer.read(length))
def emit(frame):
EMITBODY
def send(value):
    body = json.dumps(value).encode()
    emit(b'HEADER: %d\r\n\r\n' % len(body) + body)
while True:
    msg = read()
    if msg is None: break
    method = msg.get('method')
    if method == 'initialize':
        COUNT
        FAILFIRST
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': {'capabilities': {}}})
    elif method == 'textDocument/didOpen':
        uri = msg['params']['textDocument']['uri']
        opened.add(uri)
        MARK
        send({'jsonrpc': '2.0', 'method': 'textDocument/publishDiagnostics',
              'params': {'uri': PUBURI, VERSION 'diagnostics': [
                  {'range': {'start': {'line': 0, 'character': STARTAT}, 'end': {'line': 0, 'character': ENDAT}},
                   'severity': 1, 'message': 'bad' + 'x' * PAD}] * MANY}})
    elif method == 'textDocument/definition':
        uri = msg['params']['textDocument']['uri']
        result = {'uri': uri, 'range': {'start': {'line': 0, 'character': 3}, 'end': {'line': 0, 'character': 7}}} if (uri in opened or not OPENED_ONLY) else None
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': result})
    elif 'id' in msg:
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': None})
"#
    .replace("NOISY", if spec.noisy { "sys.stderr.buffer.write(b'x' * (2**20)); sys.stderr.buffer.flush()" } else { "" })
    .replace("HEADER", if spec.lower { "content-length" } else { "Content-Length" })
    .replace("COUNT", &spec.count_in.as_ref().map(|p| format!("open({:?}, 'a').write('x')", p.display().to_string())).unwrap_or_default())
    .replace("VERSION", if spec.versioned { "'version': msg['params']['textDocument']['version'], " } else { "" })
    .replace("STARTAT", &spec.start.to_string())
    .replace("ENDAT", &spec.end.to_string())
    .replace(
        "EMITBODY",
        if spec.fragment {
            "    import time\n    for i in range(len(frame)):\n        sys.stdout.buffer.write(frame[i:i+1]); sys.stdout.buffer.flush(); time.sleep(0.0002)"
        } else {
            "    sys.stdout.buffer.write(frame); sys.stdout.buffer.flush()"
        },
    )
    .replace("PAD", &spec.pad.to_string())
    .replace("MANY", &spec.many.to_string())
    .replace("MARK", &spec.mark_open_in.as_ref().map(|p| format!("open({:?}, 'w').write('x')", p.display().to_string())).unwrap_or_default())
    .replace(
        "FAILFIRST",
        if spec.fail_first_in.is_some() {
            "import os\n        if not os.path.exists(FAILMARK):\n            open(FAILMARK, 'w').write('x')\n            send({'jsonrpc': '2.0', 'id': msg['id'], 'error': {'code': -32000, 'message': 'not yet'}})\n            continue"
        } else {
            ""
        },
    )
    .replace("FAILMARK", &spec.fail_first_in.as_ref().map(|p| format!("{:?}", p.display().to_string())).unwrap_or_default())
    .replace("PUBURI", if spec.normalize_uri { "uri.replace('%2B', '+')" } else { "uri" })
    .replace("OPENED_ONLY", if spec.opened_only { "True" } else { "False" });
    std::fs::write(&path, script).unwrap();
    path
}

fn declare(rt: &mut Runtime, name: &str, fake: &Path) {
    rt.eval(&format!(
        "(define-lsp-server {name} \"A fake, for tests.\" #:command '(\"python3\" {:?}) #:modes '(prog-mode) #:languages '(rust))",
        fake.display().to_string()
    ))
    .unwrap();
}

#[test]
fn a_server_s_diagnostics_and_definition() {
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("fake.py");
    std::fs::write(&fake, FAKE).unwrap();
    let (mut rt, _) = open(dir.path(), "m.rs", "fn main() {}\n");
    rt.eval(&format!(
        "(define-lsp-server fake \"A fake, for tests.\" #:command '(\"python3\" {:?}) #:modes '(prog-mode) #:languages '(rust))",
        fake.display().to_string()
    ))
    .unwrap();
    rt.eval("(run-command (current-session) 'lsp 1)").unwrap();
    let s = until(&mut rt, diagnostics_shown);
    assert!(diagnostics_shown(&s), "the fake's error is drawn");
    let h = s.pane().layers.iter().find(|h| matches!(s.style(h.style).underline, Some((Underline::Curl, _)))).unwrap();
    assert_eq!((h.from, h.to), (0, 2));
    // An edit goes by the watcher as a didChange; nothing breaks.
    rt.handle(Input::Key { key: "x".into(), at: Instant::now() });
    until(&mut rt, |s| s.pane().text.to_string().starts_with('x'));
    rt.eval("(run-command (current-session) 'lsp-find-definition 1)").unwrap();
    // The fake answers character 3 whatever the text: there the caret goes.
    let s = until(&mut rt, |s| s.pane().head() == 3);
    assert_eq!(s.pane().head(), 3, "at the definition the server named");
    assert!(rt.take_errors().is_empty(), "{:?}", rt.take_errors());
}

/// The real thing, where it is installed: rust-analyzer over a type
/// error. Skipped where it is not (CI has the fake above for that).
#[test]
fn rust_analyzer_when_installed() {
    if std::process::Command::new("rust-analyzer").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    let (mut rt, _) = open(dir.path(), "src/main.rs", "fn main() { let n: u32 = \"three\"; }\n");
    rt.eval("(run-command (current-session) 'lsp 1)").unwrap();
    let s = until(&mut rt, diagnostics_shown);
    assert!(diagnostics_shown(&s), "rust-analyzer's error is drawn");
    assert!(rt.take_errors().is_empty(), "{:?}", rt.take_errors());
}

#[test]
fn techne_s_own_language_server() {
    let dir = tempfile::tempdir().unwrap();
    let (mut rt, _) = open(dir.path(), "m.scm", "(define (f x) y)\n");
    // The server is another package's binary: a workspace test run
    // builds it first, a lone package run may not have.
    let server = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/techne-lsp");
    if !server.exists() {
        eprintln!("techne-lsp not built; skipping (build the workspace to run this)");
        return;
    }
    rt.eval(&format!(
        "(define-lsp-server techne \"Techne Lisp, by techne-lsp.\" #:command '({:?}) #:modes '(scheme-mode) #:languages '(scheme))",
        server.display().to_string()
    ))
    .unwrap();
    rt.eval("(run-command (current-session) 'lsp 1)").unwrap();
    let s = until(&mut rt, diagnostics_shown);
    assert!(diagnostics_shown(&s), "echo={:?} errors={:?} layers={:?}", s.echo, rt.take_errors(), s.pane().layers);
    assert!(rt.take_errors().is_empty(), "{:?}", rt.take_errors());
}

#[test]
fn diagnostics_move_with_the_text_they_were_computed_from() {
    // The publication is stamped with the revision it was sent; an
    // edit before it moves it by that edit, not into the void.
    // (Lower-case headers throughout: also a legal frame.)
    let dir = tempfile::tempdir().unwrap();
    let fake = write_fake(dir.path(), "fake.py", &FakeSpec { start: 3, end: 7, versioned: true, lower: true, ..Default::default() });
    let (mut rt, _) = open(dir.path(), "m.rs", "fn main() {}\n");
    declare(&mut rt, "versioned", &fake);
    rt.eval("(run-command (current-session) 'lsp 1)").unwrap();
    let s = until(&mut rt, diagnostics_shown);
    assert_eq!(diagnostic_ranges(&s), vec![(3, 7)]);
    // A newline goes in before it: its line and its bytes both move.
    rt.handle(Input::Key { key: "RET".into(), at: Instant::now() });
    let s = until(&mut rt, |s| diagnostic_ranges(s) == vec![(4, 8)]);
    assert_eq!(diagnostic_ranges(&s), vec![(4, 8)], "the inserted line carries the diagnostic with it");
}

#[test]
fn two_servers_keep_their_own_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let one = write_fake(dir.path(), "one.py", &FakeSpec { start: 0, end: 2, ..Default::default() });
    let two = write_fake(dir.path(), "two.py", &FakeSpec { start: 3, end: 7, ..Default::default() });
    let (mut rt, _) = open(dir.path(), "m.rs", "fn main() {}\n");
    declare(&mut rt, "one", &one);
    declare(&mut rt, "two", &two);
    rt.eval("(run-command (current-session) 'lsp 1)").unwrap();
    let s = until(&mut rt, |s| diagnostic_ranges(s) == vec![(0, 2), (3, 7)]);
    assert_eq!(diagnostic_ranges(&s), vec![(0, 2), (3, 7)], "neither publication replaces the other");
}

#[test]
fn a_noisy_server_still_starts() {
    // A megabyte of stderr before a byte is read: only a drained
    // stderr lets this server ever answer `initialize`.
    let dir = tempfile::tempdir().unwrap();
    let fake = write_fake(dir.path(), "fake.py", &FakeSpec { noisy: true, ..Default::default() });
    let (mut rt, _) = open(dir.path(), "m.rs", "fn main() {}\n");
    declare(&mut rt, "noisy", &fake);
    rt.eval("(run-command (current-session) 'lsp 1)").unwrap();
    let s = until(&mut rt, diagnostics_shown);
    assert!(diagnostics_shown(&s), "echo={:?}", s.echo);
}

#[test]
fn definition_from_cold_waits_for_the_server() {
    // No `lsp` first, in a file whose name needs percent-coding:
    // the request waits for a ready server holding the document
    // (this fake answers nothing for one it was not sent).
    let dir = tempfile::tempdir().unwrap();
    let fake = write_fake(dir.path(), "fake.py", &FakeSpec { opened_only: true, ..Default::default() });
    let (mut rt, _) = open(dir.path(), "a b.rs", "fn main() {}\n");
    declare(&mut rt, "cold", &fake);
    rt.eval("(run-command (current-session) 'lsp-find-definition 1)").unwrap();
    let s = until(&mut rt, |s| s.pane().head() == 3);
    assert_eq!(s.pane().head(), 3, "echo={:?}", s.echo);
}

#[test]
fn a_restarted_server_is_started_once() {
    // The old session's death notice must not evict its successor:
    // a later attach finds the restarted server in its slot rather
    // than starting a third process.
    let dir = tempfile::tempdir().unwrap();
    let count = dir.path().join("starts");
    let fake = write_fake(dir.path(), "fake.py", &FakeSpec { count_in: Some(count.clone()), ..Default::default() });
    let (mut rt, _) = open(dir.path(), "m.rs", "fn main() {}\n");
    declare(&mut rt, "counted", &fake);
    rt.eval("(run-command (current-session) 'lsp 1)").unwrap();
    until(&mut rt, diagnostics_shown);
    rt.eval("(run-command (current-session) 'lsp-restart 1)").unwrap();
    let starts = |p: &Path| std::fs::read_to_string(p).map(|s| s.len()).unwrap_or(0);
    let s = until(&mut rt, |s| starts(&count) == 2 && diagnostics_shown(s));
    assert_eq!(starts(&count), 2, "echo={:?}", s.echo);
    // Let the old reader's end land, then ask something of the slot.
    for _ in 0..25 {
        rt.run_tasks(Duration::from_millis(20)).unwrap();
    }
    rt.eval("(run-command (current-session) 'lsp-find-definition 1)").unwrap();
    let s = until(&mut rt, |s| s.pane().head() == 3);
    assert_eq!(s.pane().head(), 3, "echo={:?}", s.echo);
    assert_eq!(starts(&count), 2, "no third server was started");
}

#[test]
fn an_edit_during_receipt_does_not_mix_revisions() {
    // A thousand findings take a while to read; an edit landing
    // while they are must not give bytes of two revisions under one
    // stamp. However the edit times against the reading, the
    // finding (against the opened "fn main() {}") is carried by
    // the two inserted bytes to (5, 9).
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("opened");
    let spec = FakeSpec { start: 3, end: 7, versioned: true, many: 1000, mark_open_in: Some(marker.clone()), ..Default::default() };
    let fake = write_fake(dir.path(), "fake.py", &spec);
    let (mut rt, _) = open(dir.path(), "m.rs", "fn main() {}\n");
    declare(&mut rt, "receipt", &fake);
    rt.eval("(run-command (current-session) 'lsp 1)").unwrap();
    until(&mut rt, |_| marker.exists());
    // In among the reading, not after it.
    let began = Instant::now();
    while began.elapsed() < Duration::from_millis(50) {
        rt.run_tasks(Duration::from_millis(1)).unwrap();
    }
    rt.handle(Input::Key { key: "é".into(), at: Instant::now() });
    let s = until(&mut rt, |s| diagnostic_ranges(s) == vec![(5, 9)]);
    assert_eq!(diagnostic_ranges(&s), vec![(5, 9)], "text={:?}", s.pane().text.to_string());
}

#[test]
fn a_frame_arriving_a_byte_at_a_time() {
    // Some four thousand fragments for the diagnostics frame:
    // assembled as they come, not re-walked per fragment (that way
    // lay ~20 s of reader time for eight thousand of them).
    let dir = tempfile::tempdir().unwrap();
    let fake = write_fake(dir.path(), "fake.py", &FakeSpec { fragment: true, pad: 4000, ..Default::default() });
    let (mut rt, _) = open(dir.path(), "m.rs", "fn main() {}\n");
    declare(&mut rt, "fragmented", &fake);
    rt.eval("(run-command (current-session) 'lsp 1)").unwrap();
    let began = Instant::now();
    let s = until(&mut rt, diagnostics_shown);
    assert!(diagnostics_shown(&s), "echo={:?}", s.echo);
    assert!(began.elapsed() < Duration::from_secs(20), "took {:?}", began.elapsed());
}

#[test]
fn a_refused_start_can_be_tried_again() {
    // The server answers its first initialize with an error: that
    // session is finished and out of the way, and attaching again
    // starts the server again — no corpse in the slot.
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("refused");
    let fake = write_fake(dir.path(), "fake.py", &FakeSpec { fail_first_in: Some(marker.clone()), ..Default::default() });
    let (mut rt, _) = open(dir.path(), "m.rs", "fn main() {}\n");
    declare(&mut rt, "once", &fake);
    rt.eval("(run-command (current-session) 'lsp 1)").unwrap();
    until(&mut rt, |_| marker.exists());
    // Attaching again, as a user would: whenever the refused
    // session has been cleared away, the next attach starts the
    // server again and it answers. (A dead session left in its
    // slot answers every attach with itself, forever.)
    let began = Instant::now();
    let s = loop {
        let s = rt.snapshot();
        if diagnostics_shown(&s) || began.elapsed() > Duration::from_secs(30) {
            break s;
        }
        rt.eval("(run-command (current-session) 'lsp 1)").unwrap();
        rt.run_tasks(Duration::from_millis(20)).unwrap();
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(diagnostics_shown(&s), "echo={:?}", s.echo);
}

#[test]
fn diagnostics_under_a_respelled_uri() {
    // The client sends the file as c%2B%2B.rs; the server publishes
    // as c++.rs, as its URL library spells it. The findings are the
    // file's all the same.
    let dir = tempfile::tempdir().unwrap();
    let fake = write_fake(dir.path(), "fake.py", &FakeSpec { normalize_uri: true, ..Default::default() });
    let (mut rt, _) = open(dir.path(), "c++.rs", "fn main() {}\n");
    declare(&mut rt, "respelling", &fake);
    rt.eval("(run-command (current-session) 'lsp 1)").unwrap();
    let s = until(&mut rt, diagnostics_shown);
    assert!(diagnostic_ranges(&s).contains(&(0, 2)), "ranges={:?} echo={:?}", diagnostic_ranges(&s), s.echo);
}

#[test]
fn no_server_serves_this_buffer() {
    // No declared server matches a Scheme buffer (rust-analyzer is
    // for Rust): asking for a definition says so, rather than
    // looking and finding none.
    let dir = tempfile::tempdir().unwrap();
    let (mut rt, _) = open(dir.path(), "m.scm", "(define (f x) x)\n");
    rt.eval("(run-command (current-session) 'lsp-find-definition 1)").unwrap();
    let s = until(&mut rt, |s| format!("{:?}", s.echo).contains("serves this buffer"));
    assert!(format!("{:?}", s.echo).contains("serves this buffer"), "echo={:?}", s.echo);
}
