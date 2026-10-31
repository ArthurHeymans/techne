//! The language server client (`lisp/editor/lsp.scm`) against a fake
//! server in Python that keeps its own copy of each document from the
//! changes it is sent — so a change sent wrong shows as a copy that is
//! not the text — and publishes an error over every `bad` in it; and
//! against rust-analyzer where it is installed.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use techne_editor::{
    present::{Input, Snapshot, Underline},
    runtime::Runtime,
};

/// ENCODING is the position encoding it takes ("utf-8" or "utf-16");
/// it mirrors each document's text to `<file>.mirror` and logs the
/// methods it is sent to LOG.
const FAKE: &str = r#"
import sys, json
ENCODING, LOG = sys.argv[1], sys.argv[2]
docs = {}

def log(line):
    with open(LOG, 'a') as f: f.write(line + '\n')

def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line: return None
        if line.strip() == b'': break
        if line.lower().startswith(b'content-length:'):
            length = int(line.split(b':')[1])
    return json.loads(sys.stdin.buffer.read(length))

def send(value):
    body = json.dumps(value).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(body) + body)
    sys.stdout.buffer.flush()

def params_of(msg):
    return msg.get('params') or {}

def units(s):
    return len(s.encode('utf-8')) if ENCODING == 'utf-8' else len(s.encode('utf-16-le')) // 2

def offset(text, pos):
    lines = text.split('\n')
    line = pos['line']
    if line >= len(lines): return len(text)
    start = sum(len(l) + 1 for l in lines[:line])
    col, n = 0, 0
    for ch in lines[line]:
        if n >= pos['character']: break
        n += units(ch)
        col += 1
    return start + col

def position(text, i):
    before = text[:i]
    line = before.count('\n')
    return {'line': line, 'character': units(before[before.rfind('\n') + 1:])}

def publish(uri, version):
    text = docs[uri]
    found, i = [], text.find('bad')
    while i >= 0:
        found.append({'range': {'start': position(text, i), 'end': position(text, i + 3)},
                      'severity': 1, 'message': 'bad word', 'source': 'fake'})
        i = text.find('bad', i + 1)
    send({'jsonrpc': '2.0', 'method': 'textDocument/publishDiagnostics',
          'params': {'uri': uri, 'version': version, 'diagnostics': found}})

def mirror(uri):
    with open(uri[len('file://'):] + '.mirror', 'w') as f: f.write(docs[uri])

while True:
    msg = read()
    if msg is None: break
    method = msg.get('method')
    if method == 'textDocument/didChange':
        log(method + (':incremental' if all('range' in c for c in params_of(msg)['contentChanges']) else ':full'))
    elif method == 'textDocument/didOpen' or method == 'textDocument/didClose':
        log(method + ' ' + params_of(msg)['textDocument']['uri'].rsplit('/', 1)[-1])
    elif method: log(method)
    params = msg.get('params') or {}
    if method == 'initialize':
        assert ENCODING in params['capabilities']['general']['positionEncodings']
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': {'capabilities': {
            'positionEncoding': ENCODING,
            'textDocumentSync': {'openClose': True, 'change': 2, 'save': {'includeText': False}}}}})
    elif method == 'textDocument/didOpen':
        d = params['textDocument']
        docs[d['uri']] = d['text']
        mirror(d['uri']); publish(d['uri'], d['version'])
    elif method == 'textDocument/didChange':
        uri = params['textDocument']['uri']
        for c in params['contentChanges']:
            if 'range' in c:
                t = docs[uri]
                a, b = offset(t, c['range']['start']), offset(t, c['range']['end'])
                docs[uri] = t[:a] + c['text'] + t[b:]
            else:
                docs[uri] = c['text']
        mirror(uri); publish(uri, params['textDocument']['version'])
    elif method == 'shutdown':
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': None})
    elif method == 'exit':
        break
    elif 'id' in msg:
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': None})
"#;

struct Fixture {
    _dir: tempfile::TempDir,
    dir: PathBuf,
    rt: Runtime,
    path: PathBuf,
    log: PathBuf,
}

impl Fixture {
    /// A runtime on DIR/NAME holding TEXT, the fake declared for Rust
    /// files where a `fake.toml` is, taking ENCODING.
    fn new(name: &str, text: &str, encoding: &str) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let fake = root.join("fake.py");
        std::fs::write(&fake, FAKE).unwrap();
        let path = root.join(name);
        std::fs::write(&path, text).unwrap();
        let log = root.join("log");
        let (mut rt, _) = Runtime::open(&path, &root.join("x.journal"), "emacs").unwrap();
        // The fake alone: rust-analyzer, where it is installed, serves
        // no test of it.
        rt.eval("(define-lsp-server rust-analyzer \"Not in these tests.\")").unwrap();
        rt.eval(&format!(
            "(define-lsp-server fake \"A fake, for tests.\" #:command '(\"python3\" {:?} {encoding:?} {:?}) #:languages '(rust) #:roots '(\"fake.toml\"))",
            fake.display().to_string(),
            log.display().to_string()
        ))
        .unwrap();
        Fixture { _dir: dir, dir: root, rt, path, log }
    }

    fn command(&mut self, name: &str) {
        self.rt.eval(&format!("(run-command (current-session) '{name} 1)")).unwrap();
    }

    fn keys(&mut self, keys: &str) {
        for k in keys.split(' ') {
            self.rt.handle(Input::Key { key: k.into(), at: Instant::now() });
        }
    }

    /// Snapshots until DONE, running the editor's tasks between them.
    fn until(&mut self, what: &str, done: impl Fn(&Snapshot, &Fixture) -> bool) -> Snapshot {
        let start = Instant::now();
        loop {
            let s = self.rt.snapshot();
            if done(&s, self) {
                return s;
            }
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "timed out waiting for {what}: echo {:?}, layers {:?}, text {:?}, mirror {:?}, log {:?}, errors {:?}",
                s.echo,
                s.pane().layers,
                text(&s),
                self.mirror(),
                self.log(),
                self.rt.take_errors()
            );
            self.rt.run_tasks(Duration::from_millis(20)).unwrap();
        }
    }

    fn mirror(&self) -> String {
        std::fs::read_to_string(self.path.with_extension("rs.mirror")).unwrap_or_default()
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

fn underlined(s: &Snapshot) -> Vec<(usize, usize)> {
    let mut found: Vec<_> = s
        .pane()
        .layers
        .iter()
        .filter(|h| matches!(s.style(h.style).underline, Some((Underline::Curl, _))))
        .map(|h| (h.from, h.to))
        .collect();
    found.dedup();
    found
}

fn text(s: &Snapshot) -> String {
    s.pane().text.to_string()
}

#[test]
fn changes_are_sent_as_they_are_made_and_diagnostics_follow() {
    for encoding in ["utf-16", "utf-8"] {
        let mut f = Fixture::new("m.rs", "fn é😀() { bad }\n", encoding);
        f.command("lsp");
        let s = f.until("the first diagnostic", |s, _| !underlined(s).is_empty());
        let at = "fn é😀() { ".len();
        assert_eq!(underlined(&s), [(at, at + 3)], "{encoding}");
        assert!(s.pane().status.contains("lsp:1E 0W"), "{}", s.pane().status);
        // Typed before it and after it, on another line, killed: the
        // server's copy is the text after each, and the diagnostics are
        // where the words are, past the line's wide characters.
        let synced = |f: &mut Fixture, what: &str, count: usize| {
            let s = f.until(what, |s, f| f.mirror() == text(s) && underlined(s).len() == count);
            let t = text(&s);
            let want: Vec<_> = t.match_indices("bad").map(|(i, _)| (i, i + 3)).collect();
            assert_eq!(underlined(&s), want, "{encoding}: {t:?}");
            s
        };
        f.keys("M-< x y RET");
        synced(&mut f, "a line before", 1);
        f.keys("C-e SPC b a d");
        let s = synced(&mut f, "a word after the wide ones", 2);
        assert!(s.pane().status.contains("lsp:2E"), "{}", s.pane().status);
        f.keys("C-a C-k");
        synced(&mut f, "the line killed", 0);
        f.keys("M-> b a d");
        synced(&mut f, "a word at the end", 1);
        // Edits sent incrementally, never the whole text again.
        assert_eq!(f.log().matches("textDocument/didOpen").count(), 1, "{}", f.log());
        assert!(f.log().contains("didChange:incremental") && !f.log().contains("didChange:full"), "{}", f.log());
    }
}

#[test]
fn diagnostics_are_gone_to_and_listed() {
    let mut f = Fixture::new("m.rs", "a bad\nb\nc bad\n", "utf-16");
    f.command("lsp");
    f.until("diagnostics", |s, _| underlined(s).len() == 2);
    f.keys("M-g n");
    let s = f.until("the first", |s, _| s.pane().head() == 2);
    assert!(s.echo.contains("bad word"), "{}", s.echo);
    f.keys("M-g n");
    f.until("the second", |s, _| s.pane().head() == 10);
    f.keys("M-g p");
    f.until("back to the first", |s, _| s.pane().head() == 2);
    f.keys("M-g f");
    let s = f.until("the list", |s, _| s.minibuffer.is_some());
    let rows = &s.minibuffer.as_ref().unwrap().rows;
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows[0].text(0).contains("error: bad word"), "{:?}", rows[0]);
}

#[test]
fn files_attach_as_they_open_where_their_project_is() {
    let mut f = Fixture::new("m.rs", "bad\n", "utf-16");
    // Without the project's file, opening a Rust file starts nothing.
    std::fs::write(f.dir.join("other.rs"), "bad\n").unwrap();
    let other = f.dir.join("other.rs").display().to_string();
    f.rt.eval(&format!("(show-document! (current-session) (file-document {other:?}))")).unwrap();
    f.rt.run_tasks(Duration::from_millis(300)).unwrap();
    assert!(underlined(&f.rt.snapshot()).is_empty());
    assert_eq!(f.log(), "");
    // With it, the next file opened attaches by itself.
    std::fs::write(f.dir.join("fake.toml"), "").unwrap();
    std::fs::write(f.dir.join("third.rs"), "x bad\n").unwrap();
    let third = f.dir.join("third.rs").display().to_string();
    f.rt.eval(&format!("(show-document! (current-session) (file-document {third:?}))")).unwrap();
    let s = f.until("the opened file's diagnostic", |s, _| !underlined(s).is_empty());
    assert_eq!(underlined(&s), [(2, 5)]);
    // Saved: the server is told; killed: the document is closed.
    f.keys("a C-x C-s");
    f.until("didSave", |_, f| f.log().contains("textDocument/didSave"));
    // Moved: the server closes the old name and opens the new.
    let moved = f.dir.join("moved.rs").display().to_string();
    f.rt.eval(&format!("(move-path! {third:?} {moved:?})")).unwrap();
    f.until("reopened", |_, f| f.log().contains("didClose third.rs") && f.log().contains("didOpen moved.rs"));
    let s = f.until("its diagnostic again", |s, _| !underlined(s).is_empty());
    assert!(s.pane().status.contains("lsp:1E"), "{}", s.pane().status);
    f.command("kill-buffer");
    f.until("didClose", |_, f| f.log().contains("didClose moved.rs"));
}

#[test]
fn a_server_ending_is_said_and_shut_down_on_request() {
    let mut f = Fixture::new("m.rs", "bad\n", "utf-16");
    f.command("lsp");
    f.until("ready", |s, _| !underlined(s).is_empty());
    f.command("lsp-shutdown");
    let s = f.until("shut down", |s, f| f.log().contains("exit") && s.echo.contains("fake shut down"));
    assert!(underlined(&s).is_empty(), "its diagnostics go with it");
    assert!(!s.pane().status.contains("lsp:"), "{}", s.pane().status);
    // And M-x lsp starts it again.
    f.command("lsp");
    f.until("ready again", |s, _| !underlined(s).is_empty());
    f.command("lsp-restart");
    f.until("restarted", |s, f| f.log().matches("initialize\n").count() == 3 && !underlined(s).is_empty());
}

#[test]
fn asking_for_a_server_that_is_not_installed_says_so() {
    let mut f = Fixture::new("m.rs", "x\n", "utf-16");
    f.rt.eval("(define-lsp-server fake \"Not here.\" #:command '(\"no-such-language-server\") #:languages '(rust))").unwrap();
    f.command("lsp");
    let s = f.rt.snapshot();
    assert!(s.echo.contains("Not installed: no-such-language-server"), "{}", s.echo);
}

/// The real thing, where it is installed: rust-analyzer over a type
/// error. Skipped where it is not.
#[test]
fn rust_analyzer_when_installed() {
    if std::process::Command::new("rust-analyzer").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("rust-analyzer not installed; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::create_dir(root.join("src")).unwrap();
    let path = root.join("src/main.rs");
    std::fs::write(&path, "fn main() { let n: u32 = \"three\"; }\n").unwrap();
    let (mut rt, _) = Runtime::open(&path, &root.join("x.journal"), "emacs").unwrap();
    // Attached as it opened: Cargo.toml is there.
    let start = Instant::now();
    loop {
        let s = rt.snapshot();
        if !underlined(&s).is_empty() {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(120), "no diagnostic from rust-analyzer: {:?} {:?}", s.echo, s.pane().status);
        rt.run_tasks(Duration::from_millis(50)).unwrap();
    }
    let _ = Path::new("");
}
