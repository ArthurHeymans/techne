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
    runtime::{Client, Runtime},
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

def rng(text, a, b):
    return {'start': position(text, a), 'end': position(text, b)}

def word_at(text, i):
    isword = lambda c: c.isalnum() or c == '_'
    a = i
    while a > 0 and isword(text[a - 1]): a -= 1
    b = i
    while b < len(text) and isword(text[b]): b += 1
    word = text[a:b]
    if not word: return '', []
    spans, j = [], 0
    while True:
        j = text.find(word, j)
        if j < 0: break
        if (j == 0 or not isword(text[j - 1])) and (j + len(word) >= len(text) or not isword(text[j + len(word)])):
            spans.append((j, j + len(word)))
        j += 1
    return word, spans

versions = {}

def mirror(uri):
    with open(uri[len('file://'):] + '.mirror', 'w') as f: f.write(docs[uri])

sys.stderr.write('fake: starting\n'); sys.stderr.flush()
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
            'textDocumentSync': {'openClose': True, 'change': 2, 'save': {'includeText': False}},
            'definitionProvider': True, 'referencesProvider': True, 'hoverProvider': True,
            'renameProvider': {'prepareProvider': True}, 'documentFormattingProvider': True,
            # Not where its root says so.
            'documentRangeFormattingProvider': not __import__('os').path.exists(params['rootPath'] + '/norange'), 'codeActionProvider': True,
            'inlayHintProvider': True}}})
    elif method == 'textDocument/didOpen':
        d = params['textDocument']
        docs[d['uri']] = d['text']
        versions[d['uri']] = d['version']
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
        versions[uri] = params['textDocument']['version']
        if 'crash' in docs[uri]:
            sys.stderr.write('thread main panicked at boom\n'); sys.stderr.flush()
            sys.exit(101)
        mirror(uri); publish(uri, params['textDocument']['version'])
    elif method == 'textDocument/prepareRename':
        # Not a keyword; and where the text says so, not done at all.
        uri = params['textDocument']['uri']
        text = docs[uri]
        word, spans = word_at(text, offset(text, params['position']))
        a = next((a for a, b in spans if a <= offset(text, params['position']) <= b), None)
        if 'noprepare' in text:
            send({'jsonrpc': '2.0', 'id': msg['id'], 'error': {'code': -32601, 'message': 'no such method'}})
            continue
        result = None if word in ('', 'fn') else {'range': rng(text, a, a + len(word)), 'placeholder': word}
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': result})
    elif method in ('textDocument/definition', 'textDocument/references', 'textDocument/hover', 'textDocument/rename'):
        # The word at the position: its first occurrence is its
        # definition, every one a reference.
        uri = params['textDocument']['uri']
        text = docs[uri]
        word, spans = word_at(text, offset(text, params['position']))
        if method == 'textDocument/definition':
            result = {'uri': uri, 'range': rng(text, *spans[0])} if spans else None
        elif method == 'textDocument/references':
            result = [{'uri': uri, 'range': rng(text, a, b)} for a, b in spans]
        elif method == 'textDocument/hover':
            result = {'contents': {'kind': 'markdown', 'value': '```rust\n%s: Thing\n```\n\n---\n\nA thing called %s.' % (word, word)}} if word else None
        else:
            changes = {uri: [{'range': rng(text, a, b), 'newText': params['newName']} for a, b in spans]}
            # A file beside it that is not open: from its text on disk.
            other = uri.rsplit('/', 1)[0] + '/other.rs'
            import os
            if os.path.exists(other[len('file://'):]) and other not in docs:
                otext = open(other[len('file://'):]).read()
                _, ospans = word_at(otext + ' ' + word, len(otext) + 1)
                changes[other] = [{'range': rng(otext, a, b), 'newText': params['newName']} for a, b in ospans if b <= len(otext)]
            result = {'changes': changes}
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': result})
    elif method == 'textDocument/inlayHint':
        # After each `let NAME`, its type.
        uri = params['textDocument']['uri']
        text = docs[uri]
        hints, i = [], text.find('let ')
        while i >= 0:
            j = i + 4
            while j < len(text) and (text[j].isalnum() or text[j] == '_'): j += 1
            hints.append({'position': position(text, j), 'label': ': i32', 'kind': 1})
            i = text.find('let ', j)
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': hints})
    elif method in ('textDocument/formatting', 'textDocument/rangeFormatting'):
        # Tabs to the options' spaces: in the range only, where one is given.
        uri = params['textDocument']['uri']
        text = docs[uri]
        # A text saying `fail` cannot be formatted; one saying `slow` is
        # formatted after a while.
        if 'fail' in text:
            send({'jsonrpc': '2.0', 'id': msg['id'], 'error': {'code': -32603, 'message': 'cannot format'}})
            continue
        if 'slow' in text:
            import time
            time.sleep(0.5)
        a, b = 0, len(text)
        if 'range' in params:
            a, b = offset(text, params['range']['start']), offset(text, params['range']['end'])
        width = ' ' * params['options']['tabSize']
        tabs = [i for i, c in enumerate(text) if c == '\t' and a <= i < b]
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': [{'range': rng(text, i, i + 1), 'newText': width} for i in reversed(tabs)]})
    elif method == 'textDocument/codeAction':
        d = params['textDocument']
        text = docs[d['uri']]
        spans, i = [], text.find('bad')
        while i >= 0:
            spans.append((i, i + 3)); i = text.find('bad', i + 1)
        def fix(title, word):
            return {'title': title, 'kind': 'quickfix',
                    'edit': {'documentChanges': [{'textDocument': {'uri': d['uri'], 'version': versions[d['uri']]},
                                                  'edits': [{'range': rng(text, a, b), 'newText': word} for a, b in spans]}]}}
        # Two of one title, and two insertions at one place.
        actions = [fix('Make it good', 'good'), fix('Make it good', 'fine'),
                   {'title': 'Insert AB', 'edit': {'changes': {d['uri']: [
                       {'range': rng(text, 0, 0), 'newText': 'A'}, {'range': rng(text, 0, 0), 'newText': 'B'}]}}}] if spans else []
        if 'twofiles' in text:
            # An edit of this file and of one that is not there; and a
            # command whose edit the server asks to be applied, which
            # would create a file too.
            good = fix('', 'good')['edit']['documentChanges'][0]
            missing = d['uri'].rsplit('/', 1)[0] + '/missing.rs'
            actions = [{'title': 'Break two files', 'edit': {'documentChanges': [
                           good, {'textDocument': {'uri': missing, 'version': None},
                                  'edits': [{'range': rng('', 0, 0), 'newText': 'x'}]}]}},
                       {'title': 'Ask to apply', 'command': {'title': 'Ask', 'command': 'apply', 'arguments': [
                           {'documentChanges': [good, {'kind': 'create', 'uri': missing}]}]}}]
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': actions})
    elif method == 'workspace/executeCommand':
        send({'jsonrpc': '2.0', 'id': 'srv-1', 'method': 'workspace/applyEdit',
              'params': {'edit': params['arguments'][0]}})
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': None})
    elif method is None and msg.get('id') == 'srv-1':
        log('applyEdit ' + json.dumps(msg.get('result'), sort_keys=True))
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
fn what_goes_to_and_from_a_server_is_logged() {
    let mut f = Fixture::new("m.rs", "bad\n", "utf-16");
    f.command("lsp");
    f.until("ready", |s, _| !underlined(s).is_empty());
    f.command("lsp-events");
    // The log's buffer's text, once it is as WANT has it.
    let events = |f: &mut Fixture, what: &str, want: &dyn Fn(&str) -> bool| {
        let start = Instant::now();
        loop {
            let found = f
                .rt
                .eval("(let ((b (find (lambda (b) (string-prefix? \"*lsp-events fake \" (buffer-name b))) (buffer-list)))) (if b (document-string (buffer-document b)) \"\"))")
                .unwrap();
            let t = found.replace("\\\"", "\"");
            if want(&t) {
                return t;
            }
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "timed out waiting for {what}: {t}; buffers {:?}, echo {:?}, errors {:?}",
                f.rt.eval("(map buffer-name (buffer-list))"),
                f.rt.snapshot().echo,
                f.rt.take_errors()
            );
            f.rt.run_tasks(Duration::from_millis(20)).unwrap();
        }
    };
    events(&mut f, "the log shown", &|t| {
        t.contains("---- ")
            && t.contains(" --> {")
            && t.contains("\"method\":\"initialize\"")
            && t.contains(" <-- {")
            && t.contains("stderr fake: starting")
    });
    // Followed as more comes.
    f.keys("M-> x");
    events(&mut f, "an edit logged", &|t| t.contains("textDocument/didChange"));
    // A server that dies says why: its stderr's last line.
    f.keys("c r a s h");
    let s = f.until("ended", |s, _| s.echo.contains("fake ended"));
    assert!(s.echo.contains("panicked at boom"), "{}", s.echo);
    events(&mut f, "its end logged", &|t| t.contains("stderr thread main panicked at boom") && t.contains(" ended ----"));
    // With no server left running, the log of the one that ran.
    f.command("lsp-events");
    assert!(f.rt.take_errors().is_empty() && !f.rt.snapshot().echo.contains("No language server"), "{}", f.rt.snapshot().echo);
    // Why a run ended is its own stderr's last line, however long what
    // it wrote last: not an earlier run's, not the middle of a panic.
    let lsp_log = techne_editor::runtime::lisp_dir().join("lsp-log.scm");
    f.rt.eval(&format!("(require {:?})", lsp_log.display().to_string())).unwrap();
    let tails =
        f.rt.eval(
            "(let ((log (lsp-log 'tails \"tails\")))
               (lsp-log-event! log 'err \"earlier run\\n\")
               (let ((since (lsp-log-stderr-count log)))
                 (list (lsp-log-stderr-tail log since)
                       (begin (lsp-log-event! log 'err (string-append (make-string 10000 #\\x) \"\\nlast words\\n\"))
                              (lsp-log-stderr-tail log since)))))",
        )
        .unwrap();
    assert_eq!(tails, "(#f \"last words\")");
}

#[test]
fn a_server_that_crashes_is_started_again() {
    let mut f = Fixture::new("m.rs", "bad\n", "utf-16");
    f.command("lsp");
    f.until("ready", |s, _| s.pane().status.contains("lsp:1E"));
    f.keys("M-> c r a s h");
    let s = f.until("ended", |s, _| s.echo.contains("fake ended"));
    assert!(s.echo.contains("panicked at boom") && s.echo.contains("reconnecting in 1s"), "{}", s.echo);
    let s = f.until("reconnecting", |s, _| s.pane().status.contains("lsp:reconnecting") || s.pane().status.contains("lsp:starting"));
    assert!(underlined(&s).is_empty(), "the dead server's diagnostics go");
    // Started again with the buffer as it is now, unsaved.
    f.until("again", |s, f| f.log().matches("initialize\n").count() == 2 && s.pane().status.contains("lsp:1E"));
    assert!(f.mirror().ends_with("crash"), "{:?}", f.mirror());
    // Shut down by hand: not started again.
    f.command("lsp-shutdown");
    f.until("shut down", |s, _| s.echo.contains("fake shut down"));
    f.rt.run_tasks(Duration::from_millis(1500)).unwrap();
    assert_eq!(f.log().matches("initialize\n").count(), 2, "{}", f.log());
}

#[test]
fn asking_for_a_server_that_is_not_installed_says_so() {
    let mut f = Fixture::new("m.rs", "x\n", "utf-16");
    f.rt.eval("(define-lsp-server fake \"Not here.\" #:command '(\"no-such-language-server\") #:languages '(rust))").unwrap();
    f.command("lsp");
    let s = f.rt.snapshot();
    assert!(s.echo.contains("Not installed: no-such-language-server"), "{}", s.echo);
}

#[test]
fn definitions_references_and_back() {
    let mut f = Fixture::new("m.rs", "fn foo() {}\nfn main() { foo(); foo(); }\n", "utf-16");
    f.command("lsp");
    f.until("ready", |s, _| s.pane().status.contains("lsp:0E"));
    // On the second line's first foo.
    let call = "fn foo() {}\nfn main() { ".len();
    f.rt.eval(&format!("(view-set-ranges! (session-view (current-session)) (list (list {call} {call})) 0)")).unwrap();
    f.keys("M-.");
    f.until("the definition", |s, _| s.pane().head() == 3);
    f.keys("M-,");
    f.until("back", |s, _| s.pane().head() == call);
    // The references, listed; RET on the last goes there.
    f.keys("M-?");
    let s = f.until("the references", |s, _| s.minibuffer.as_ref().is_some_and(|m| m.rows.len() == 3));
    assert!(s.minibuffer.as_ref().unwrap().rows[0].text(1).contains("m.rs:1"), "{:?}", s.minibuffer);
    f.keys("C-n C-n RET");
    let last = "fn foo() {}\nfn main() { foo(); ".len();
    f.until("the last reference", |s, _| s.minibuffer.is_none() && s.pane().head() == last);
    f.keys("M-,");
    f.until("back again", |s, _| s.pane().head() == call);
}

#[test]
fn what_is_at_point_is_said_as_it_rests() {
    let mut f = Fixture::new("m.rs", "fn foo() {}\nlet x = bad;\n", "utf-16");
    f.command("lsp");
    f.until("ready", |s, _| s.pane().status.contains("lsp:1E"));
    // On foo: its hover's signature, not its documentation.
    f.keys("C-f C-f C-f C-f");
    let s = f.until("the hover", |s, _| s.echo.contains("foo: Thing"));
    assert!(!s.echo.contains("A thing"), "{}", s.echo);
    // On the diagnostic: that first.
    f.keys("C-n C-e C-b C-b");
    f.until("the diagnostic", |s, _| s.echo.contains("bad word [fake]"));
    // C-h . shows all of it.
    f.keys("C-p C-a C-f C-f C-f C-h .");
    let s = f.until("*eldoc*", |s, _| s.panes.iter().any(|p| p.text.to_string().contains("A thing called foo.")));
    assert_eq!(s.panes.len(), 2, "beside the buffer, which keeps the focus");
    // Rendered: the code fenced in rust as a code block, the rule drawn.
    let eldoc = s.panes.iter().map(|p| p.text.to_string()).find(|t| t.contains("A thing")).unwrap();
    assert_eq!(eldoc, format!(" rust\n foo: Thing\n\n{}\n\nA thing called foo.", "─".repeat(40)));
}

#[test]
fn renaming_formatting_and_acting() {
    let mut f = Fixture::new("m.rs", "fn foo() {}\n\tfoo(bad);\n", "utf-8");
    eval(&mut f, WATCH);
    f.command("lsp");
    f.until("ready", |s, _| s.pane().status.contains("lsp:1E"));
    std::fs::write(f.dir.join("other.rs"), "fn other() { foo(); }\n").unwrap();
    f.keys("C-f C-f C-f C-c c r");
    f.until("the prompt", |s, _| s.minibuffer.as_ref().is_some_and(|m| m.input == "foo"));
    f.keys("DEL DEL DEL b a r RET");
    let s = f.until("renamed", |s, f| text(s) == "fn bar() {}\n\tbar(bad);\n" && f.mirror() == text(s));
    assert!(s.echo.contains("Renamed foo to bar in 2 files"), "{}", s.echo);
    // The file that was not open is now, in a buffer, renamed in.
    assert_eq!(f.rt.eval("(document-string (buffer-document (buffer-named \"other.rs\")))").unwrap(), "\"fn other() { bar(); }\\n\"");
    // Both files' changes name the rename.
    let renames = eval(&mut f, "(map car (ended-of 'lsp-rename))");
    let rename = renames.trim_matches(['(', ')']).to_string();
    for name in ["m.rs", "other.rs"] {
        let ops = eval(&mut f, &format!("(operations-of (buffer-document (buffer-named {name:?})))"));
        assert!(ops.split([' ', '(', ')']).any(|o| o == rename), "{name}: {ops} vs {renames}");
    }
    f.keys("C-c c f");
    f.until("formatted", |s, f| text(s) == "fn bar() {}\n    bar(bad);\n" && f.mirror() == text(s));
    f.keys("C-c c a");
    f.until("the actions", |s, _| s.minibuffer.as_ref().is_some_and(|m| m.rows.len() == 3));
    // The second of two of one title is the second.
    f.keys("C-n RET");
    f.until("acted", |s, f| text(s) == "fn bar() {}\n    bar(fine);\n" && f.mirror() == text(s) && underlined(s).is_empty());
    // Each is one change, undone at once.
    f.keys("C-/");
    f.until("undone", |s, _| text(s) == "fn bar() {}\n    bar(bad);\n");
    // Two insertions at one place go in as the server gave them.
    f.keys("C-c c a");
    f.until("the actions again", |s, _| s.minibuffer.as_ref().is_some_and(|m| m.rows.len() == 3));
    f.keys("C-n C-n RET");
    f.until("inserted", |s, _| text(s).starts_with("ABfn bar()"));
}

#[test]
fn renaming_asks_what_is_renamed_first() {
    let mut f = Fixture::new("m.rs", "fn foo() {}\nfoo();\n", "utf-16");
    f.command("lsp");
    f.until("ready", |s, _| s.pane().status.contains("lsp:"));
    // A keyword: nothing to rename, nothing asked.
    f.keys("C-c c r");
    let s = f.until("refused", |s, _| s.echo.contains("Nothing here can be renamed"));
    assert!(s.minibuffer.is_none());
    // The name, as the server spans it; changed while the new one is
    // read: refused, nothing renamed.
    f.keys("C-f C-f C-f C-f C-c c r");
    f.until("the prompt", |s, _| s.minibuffer.as_ref().is_some_and(|m| m.input == "foo"));
    f.rt.eval("(view-edit! (make-view (buffer-document (buffer-named \"m.rs\")) \"other\") '((4 5 \"x\")) \"new\")").unwrap();
    f.keys("C-a C-k b a r RET");
    let s = f.until("refused again", |s, _| s.echo.contains("The name changed"));
    assert_eq!(text(&s), "fn fxo() {}\nfoo();\n");
    // Changed elsewhere meanwhile: renamed where the name is now.
    f.keys("C-c c r");
    f.until("the prompt again", |s, _| s.minibuffer.as_ref().is_some_and(|m| m.input == "fxo"));
    f.rt.eval("(view-edit! (make-view (buffer-document (buffer-named \"m.rs\")) \"other\") '((0 0 \"// \")) \"new\")").unwrap();
    f.keys("C-a C-k f o o RET");
    f.until("renamed", |s, f| text(s) == "// fn foo() {}\nfoo();\n" && f.mirror() == text(s));
    // A server without prepareRename after all: the name at point.
    f.rt.eval("(view-edit! (make-view (buffer-document (buffer-named \"m.rs\")) \"other\") '((0 0 \"noprepare \")) \"new\")").unwrap();
    f.keys("C-c c r");
    f.until("the prompt, without", |s, _| s.minibuffer.as_ref().is_some_and(|m| m.input == "foo"));
    f.keys("C-a C-k b a r RET");
    f.until("renamed without", |s, f| text(s) == "noprepare // fn bar() {}\nbar();\n" && f.mirror() == text(s));
}

#[test]
fn formatting_the_region_or_the_buffer() {
    let mut f = Fixture::new("m.rs", "\ta\n\tb\n\tc\n", "utf-16");
    f.command("lsp");
    f.until("ready", |s, _| s.pane().status.contains("lsp:"));
    f.until("ready", |s, _| s.pane().status.contains("lsp:0E"));
    f.rt.eval("(set-option! 'lsp-format-tab-size 2)").unwrap();
    // The second line's region: only it.
    f.keys("C-n C-SPC C-n C-c c f");
    f.until("the region formatted", |s, f| text(s) == "\ta\n  b\n\tc\n" && f.mirror() == text(s));
    // No region: the buffer.
    f.keys("C-c c f");
    f.until("the buffer formatted", |s, f| text(s) == "  a\n  b\n  c\n" && f.mirror() == text(s));
}

#[test]
fn formatting_a_region_where_only_buffers_are() {
    let mut f = Fixture::new("m.rs", "\ta\n\tb\n", "utf-16");
    std::fs::write(f.dir.join("norange"), "").unwrap();
    f.command("lsp");
    f.until("ready", |s, _| s.pane().status.contains("lsp:"));
    f.keys("C-n C-SPC C-n C-c c f");
    f.until("asked", |s, _| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Format the whole buffer?")));
    f.keys("y e s RET");
    f.until("the buffer formatted", |s, f| text(s) == "    a\n    b\n" && f.mirror() == text(s));
}

#[test]
fn workspace_edits_are_all_or_none() {
    let mut f = Fixture::new("m.rs", "fn twofiles() {}\nbad\n", "utf-16");
    f.command("lsp");
    f.until("ready", |s, _| s.pane().status.contains("lsp:1E"));
    // One of its files is not there: none of it is made.
    f.keys("C-c c a");
    f.until("the actions", |s, _| s.minibuffer.as_ref().is_some_and(|m| m.rows.len() == 2));
    f.keys("RET");
    let s = f.until("refused", |s, _| s.echo.contains("no such file"));
    assert_eq!(text(&s), "fn twofiles() {}\nbad\n");
    // Asked by the server, the same: and it is told which change failed.
    f.keys("C-c c a");
    f.until("the actions again", |s, _| s.minibuffer.as_ref().is_some_and(|m| m.rows.len() == 2));
    f.keys("C-n RET");
    f.until("told", |_, f| f.log().contains("applyEdit "));
    let log = f.log();
    let told = log.lines().find(|l| l.starts_with("applyEdit ")).unwrap();
    assert!(
        told.contains("\"applied\": false") && told.contains("\"failedChange\": 1") && told.contains("creating files is not supported"),
        "{told}"
    );
    let s = f.rt.snapshot();
    assert_eq!(text(&s), "fn twofiles() {}\nbad\n");
    assert!(!f.rt.eval("(map buffer-name (buffer-list))").unwrap().contains("missing.rs"), "no buffer for what was refused");
}

#[test]
fn types_are_shown_after_names() {
    let mut f = Fixture::new("m.rs", "fn main() {\n    let x = 1;\n    let yz = x;\n}\n", "utf-16");
    f.command("lsp");
    f.until("ready", |s, _| s.pane().status.contains("lsp:0E"));
    let s = f.until("the hints", |s, _| s.pane().inlays.len() == 2);
    let text = text(&s);
    let x = text.find("let x").unwrap() + 5;
    let yz = text.find("let yz").unwrap() + 6;
    assert_eq!(s.pane().inlays.iter().map(|i| (i.at, i.text.as_str())).collect::<Vec<_>>(), [(x, ": i32"), (yz, ": i32")]);
    assert!(s.style(s.pane().inlays[0].style).slant.is_some(), "drawn in the hint's face");
    // Typed before them, they move with the text; asked again, the
    // server's say where they are.
    f.keys("M-< C-n C-e SPC");
    let s = f.until("moved", |s, _| s.pane().inlays.first().is_some_and(|i| i.at == x));
    assert_eq!(s.pane().inlays.len(), 2);
    f.rt.eval("(set-option! 'lsp-inlay-hints #f)").unwrap();
    let s = f.until("gone", |s, _| s.pane().inlays.is_empty());
    assert!(s.pane().inlays.is_empty());
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

/// Record each operation that ends, newest first, in `ended`: (serial
/// command outcome parent committed?); `(operations-of D)` lists the
/// serial of the operation each change of D was made in, oldest first.
const WATCH: &str = "
  (define ended '())
  (add-hook! 'operation-ended 'test
    (lambda (s op)
      (set! ended (cons (list (operation-serial op) (operation-command op) (operation-outcome op)
                              (operation-parent-serial op) (operation-committed? op))
                        ended))))
  (define (ended-of name) (filter (lambda (e) (eq? (cadr e) name)) ended))
  (define (operations-of d)
    (let loop ((r (document-revision d)) (acc '()))
      (if (= r 0) acc (loop (- r 1) (cons (let ((o (document-operation d r))) (and o (cadr o))) acc)))))";

fn eval(f: &mut Fixture, source: &str) -> String {
    f.rt.eval(source).unwrap_or_else(|e| panic!("{source}: {e}"))
}

#[test]
fn formatting_is_an_operation_its_changes_name() {
    let mut f = Fixture::new("m.rs", "fn a() {\n\tx();\n}\n", "utf-8");
    let other = f.dir.join("n.rs");
    std::fs::write(&other, "fn b() {\n\ty();\n}\n").unwrap();
    eval(&mut f, WATCH);
    f.command("lsp");
    f.until("ready", |s, _| s.pane().status.contains("lsp:0E"));
    // A second editor session, on another file.
    f.rt.attach(Client(1), None, "emacs").unwrap();
    eval(&mut f, &format!("(show-document! (current-session) (file-document {:?}))", other.display().to_string()));
    f.command("lsp");
    f.until("attached", |s, _| s.pane().status.contains("lsp:0E"));
    // Each asks its server to format, and types before the answer.
    for client in [Client::FIRST, Client(1)] {
        f.rt.select(client).unwrap();
        f.keys("C-c c f C-e z");
    }
    let formatted = |f: &mut Fixture, client| {
        f.rt.select(client).unwrap();
        let s = f.rt.snapshot();
        text(&s).contains("    ")
    };
    let start = Instant::now();
    while !(formatted(&mut f, Client::FIRST) && formatted(&mut f, Client(1))) {
        if start.elapsed() > Duration::from_secs(30) {
            let mut seen = Vec::new();
            for c in [Client::FIRST, Client(1)] {
                f.rt.select(c).unwrap();
                let s = f.rt.snapshot();
                seen.push((text(&s), s.echo.clone(), s.pane().status.clone()));
            }
            panic!("not formatted: {seen:?} {:?} {}", f.rt.take_errors(), f.log());
        }
        f.rt.run_tasks(Duration::from_millis(20)).unwrap();
    }
    let formats = eval(&mut f, "(reverse (ended-of 'lsp-format))");
    let serials: Vec<String> = formats.trim_matches(['(', ')']).split(") (").map(|e| e.split(' ').next().unwrap().to_string()).collect();
    assert_eq!(serials.len(), 2, "{formats}");
    assert!(formats.matches("lsp-format done #f #t").count() == 2, "{formats}");
    // Each file's changes name its own: the format, and the key typed.
    let changes =
        |f: &mut Fixture, path: &std::path::Path| eval(f, &format!("(operations-of (file-document {:?}))", path.display().to_string()));
    let path = f.path.clone();
    let (m, n) = (changes(&mut f, &path), changes(&mut f, &other));
    assert!(m.contains(&serials[0]) && !m.contains(&serials[1]), "{m} {serials:?}");
    assert!(n.contains(&serials[1]) && !n.contains(&serials[0]), "{n} {serials:?}");
    assert!(!m.contains("#f") && !n.contains("#f"), "every change made in an operation: {m} {n}");
    // After a restart, the journal says so too.
    let Fixture { _dir, rt, path, .. } = f;
    drop(rt);
    let (mut rt, _) = Runtime::open(&path, &path.parent().unwrap().join("x.journal"), "emacs").unwrap();
    rt.eval(WATCH).unwrap();
    assert_eq!(rt.eval(&format!("(operations-of (file-document {:?}))", path.display().to_string())).unwrap(), m);
}

#[test]
fn a_superseded_format_is_cancelled_and_a_refused_one_failed() {
    let mut f = Fixture::new("m.rs", "fn a() {\n\tslow();\n}\n", "utf-8");
    eval(&mut f, WATCH);
    f.command("lsp");
    f.until("ready", |s, _| s.pane().status.contains("lsp:0E"));
    // Asked again before the first answer: the first is superseded.
    f.keys("C-c c f C-c c f");
    f.until("formatted", |s, _| text(s).contains("    slow"));
    let formats = eval(&mut f, "(reverse (ended-of 'lsp-format))");
    assert!(formats.contains("lsp-format cancelled #f #f") && formats.contains("lsp-format done #f #t"), "{formats}");
    // The server refuses: the error is said, and the operation failed.
    f.keys("C-e SPC f a i l");
    f.keys("C-c c f");
    f.until("refused", |s, _| s.echo.contains("cannot format"));
    let last = eval(&mut f, "(car (ended-of 'lsp-format))");
    assert!(last.contains("lsp-format failed #f #f"), "{last}");
}
