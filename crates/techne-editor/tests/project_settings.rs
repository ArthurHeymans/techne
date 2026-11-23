//! Per-project settings (`.techne.scm` + `.editorconfig`) and live LSP
//! configuration integration tests:
//! - Option precedence, selectors, `(root #t)`, zero-I/O cached reads, `explain-option`
//! - Undoable before-save formatting (`trim-trailing-whitespace`, `insert-final-newline`)
//! - `#:user-only` unconditional rejection, value-based trust, `0600` ledger persistence,
//!   non-user principal refusal, fragment-composition defense, `settings:pending` / `settings:err`
//! - Interactive commands (`project-settings-status`, `project-settings-trust`,
//!   `project-settings-revoke`, `project-settings-reload`) and save auto-reload
//! - `M-x compile` respecting trusted `compile-command`
//! - Live LSP `workspace/didChangeConfiguration` push without restart, `(unset)` tombstone
//!   removal, per-`scopeUri` `workspace/configuration`, and EditorConfig-inherited
//!   `formattingOptions`

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use techne_editor::{
    present::{Input, Pane, Snapshot},
    runtime::Runtime,
};

fn keys(rt: &mut Runtime, seq: &str) {
    for k in seq.split(' ') {
        rt.handle(Input::Key { key: k.to_string(), at: Instant::now() });
    }
}

fn type_text(rt: &mut Runtime, text: &str) {
    for c in text.chars() {
        let key = if c == ' ' { "SPC".to_string() } else { c.to_string() };
        rt.handle(Input::Key { key, at: Instant::now() });
    }
}

fn mx(rt: &mut Runtime, cmd: &str) {
    keys(rt, "M-x");
    type_text(rt, cmd);
    keys(rt, "M-RET");
}

fn until(rt: &mut Runtime, what: &str, done: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let start = Instant::now();
    loop {
        rt.run_tasks(Duration::from_millis(15)).unwrap();
        let s = rt.snapshot();
        if done(&s) {
            return s;
        }
        assert!(
            start.elapsed() < Duration::from_secs(25),
            "timed out waiting for {what}: echo={:?} status={:?} minibuffer={:?} errors={:?}",
            s.echo,
            s.pane().status,
            s.minibuffer.as_ref().map(|m| &m.prompt),
            rt.take_errors()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn isolate_trust_store(rt: &mut Runtime, trust_file: &Path) {
    rt.eval(&format!(
        "(begin (import (techne project-settings)) (set-trust-store-path-hook! (lambda () {:?})) (project-trust-clear-memory!))",
        trust_file.to_string_lossy()
    ))
    .unwrap();
}

fn compilation_pane(s: &Snapshot) -> Option<&Pane> {
    s.panes.iter().find(|p| p.status.contains("*compilation*"))
}

#[test]
fn editorconfig_and_techne_scm_precedence_root_zero_io_and_explain() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let proj = base.join("proj");
    let sub = proj.join("sub");
    let isolated = sub.join("isolated");
    std::fs::create_dir_all(&isolated).unwrap();
    std::fs::create_dir_all(proj.join(".git")).unwrap();

    // Parent .editorconfig & .techne.scm
    std::fs::write(
        proj.join(".editorconfig"),
        "root = true\n\
         [*]\n\
         indent_style = space\n\
         indent_size = 4\n\
         tab_width = 8\n\
         max_line_length = 100\n\
         [*.rs]\n\
         indent_size = 2\n",
    )
    .unwrap();
    std::fs::write(
        proj.join(".techne.scm"),
        "(techne-settings 1\n\
           (all (tab-width 6))\n\
           (mode prog-mode (fill-column 88))\n\
           (files \"*.rs\" (indent-width 3)))\n",
    )
    .unwrap();

    // Child .techne.scm overrides indent-width for *.rs; child .editorconfig sets max_line_length
    std::fs::write(
        sub.join(".editorconfig"),
        "[*]\n\
         max_line_length = 72\n",
    )
    .unwrap();
    std::fs::write(
        sub.join(".techne.scm"),
        "(techne-settings 1\n\
           (files \"*.rs\" (indent-width 5)))\n",
    )
    .unwrap();

    // Isolated subdir with (root #t) stops .techne.scm traversal (but .editorconfig continues unless root=true)
    std::fs::write(
        isolated.join(".techne.scm"),
        "(techne-settings 1\n\
           (root #t)\n\
           (all (tab-width 2)))\n",
    )
    .unwrap();

    let rs_file = sub.join("main.rs");
    let txt_file = sub.join("notes.txt");
    let iso_file = isolated.join("lib.rs");
    std::fs::write(&rs_file, "fn main() {}\n").unwrap();
    std::fs::write(&txt_file, "hello\n").unwrap();
    std::fs::write(&iso_file, "pub fn x() {}\n").unwrap();

    let trust_file = base.join("trust.scm");
    let (mut rt, _) = Runtime::open(&rs_file, &base.join("j1"), "emacs").unwrap();
    isolate_trust_store(&mut rt, &trust_file);

    // 1. Subdir .techne.scm (indent-width 5) beats parent .techne.scm (3) and .editorconfig (2)
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'indent-width)").unwrap(), "5");
    // 2. Parent .techne.scm (tab-width 6) beats parent .editorconfig (tab_width 8)
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'tab-width)").unwrap(), "6");
    // 3. Subdir .editorconfig (max_line_length 72) beats parent .techne.scm (fill-column 88) because subdir is nearer!
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'fill-column)").unwrap(), "72");
    // 4. Parent .editorconfig indent_style = space -> indent-tabs = #f
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'indent-tabs)").unwrap(), "#f");

    // 5. User #:directory override beats .techne.scm; buffer-local beats user #:directory
    rt.eval(&format!("(set-option! 'indent-width 7 #:directory {:?})", sub.to_string_lossy())).unwrap();
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'indent-width)").unwrap(), "7");
    rt.eval("(set-option! 'indent-width 9 #:buffer (current-buffer (current-session)))").unwrap();
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'indent-width)").unwrap(), "9");
    rt.eval("(unset-option! 'indent-width #:buffer (current-buffer (current-session)))").unwrap();
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'indent-width)").unwrap(), "7");
    rt.eval(&format!("(unset-option! 'indent-width #:directory {:?})", sub.to_string_lossy())).unwrap();
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'indent-width)").unwrap(), "5");

    // 6. explain-option reports .techne.scm and .editorconfig sources
    let explained = rt.eval("(explain-option (current-buffer (current-session)) 'indent-width)").unwrap();
    assert!(explained.contains(".techne.scm"), "expected .techne.scm in explain-option: {explained}");
    assert!(explained.contains(".editorconfig"), "expected .editorconfig in explain-option: {explained}");

    // 7. Zero-I/O read after load: deleting sub/.techne.scm on disk does NOT change cached option
    std::fs::remove_file(sub.join(".techne.scm")).unwrap();
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'indent-width)").unwrap(), "5");
    // Reload picks up deletion -> falls back to parent .techne.scm (3)
    mx(&mut rt, "project-settings-reload");
    let _ = rt.snapshot();
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'indent-width)").unwrap(), "3");

    // 8. Isolated subdir with (root #t) does not inherit parent .techne.scm fill-column 88 or tab-width 6
    rt.eval(&format!("(show-document! (current-session) (file-document {:?}))", iso_file.to_string_lossy())).unwrap();
    let _ = rt.snapshot();
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'tab-width)").unwrap(), "2");
    // Inherited .editorconfig still gives max_line_length = 72 from sub/.editorconfig
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'fill-column)").unwrap(), "72");

    // 9. notes.txt is in text-mode, so parent (mode prog-mode (fill-column 88)) does not match
    rt.eval(&format!("(show-document! (current-session) (file-document {:?}))", txt_file.to_string_lossy())).unwrap();
    let _ = rt.snapshot();
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'indent-width)").unwrap(), "4");
    assert!(rt.take_errors().is_empty());
}

#[test]
fn before_save_formatting_is_one_undoable_edit_and_skips_read_only() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().canonicalize().unwrap().join("proj");
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    std::fs::write(
        proj.join(".editorconfig"),
        "root = true\n\
         [*]\n\
         trim_trailing_whitespace = true\n\
         insert_final_newline = true\n",
    )
    .unwrap();
    let file = proj.join("sample.txt");
    let original = "alpha   \nbeta\t  \ngamma  ";
    std::fs::write(&file, original).unwrap();

    let (mut rt, _) = Runtime::open(&file, &tmp.path().join("j"), "emacs").unwrap();
    isolate_trust_store(&mut rt, &tmp.path().join("trust.scm"));

    // Save with C-x C-s: trailing whitespace trimmed + final newline appended
    keys(&mut rt, "C-x C-s");
    let saved = std::fs::read_to_string(&file).unwrap();
    assert_eq!(saved, "alpha\nbeta\ngamma\n");
    assert_eq!(rt.snapshot().pane().text.to_string(), "alpha\nbeta\ngamma\n");

    // A single undo reverts ALL before-save formatting edits cleanly
    keys(&mut rt, "C-/");
    assert_eq!(rt.snapshot().pane().text.to_string(), original);

    // Read-only buffer must not be modified by before-save
    rt.eval("(set-option! 'read-only #t #:buffer (current-buffer (current-session)))").unwrap();
    keys(&mut rt, "C-x C-s");
    assert_eq!(rt.snapshot().pane().text.to_string(), original);
    assert!(rt.take_errors().is_empty());
}

#[test]
fn user_only_rejection_value_trust_principal_guard_fragment_defense_and_compile() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let proj = base.join("proj");
    let sub = proj.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::create_dir_all(proj.join(".git")).unwrap();

    let techne_file = proj.join(".techne.scm");
    std::fs::write(
        &techne_file,
        "(techne-settings 1\n\
           (all (direnv-enabled #f)\n\
                (server-eval #f)\n\
                (compile-command \"echo TRUSTED_BUILD_OK\")\n\
                (lsp-settings (dict \"a\" 1))))\n",
    )
    .unwrap();
    std::fs::write(
        sub.join(".techne.scm"),
        "(techne-settings 1\n\
           (all (lsp-settings (dict \"b\" 2))))\n",
    )
    .unwrap();
    let file = proj.join("main.rs");
    let sub_file = sub.join("lib.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();
    std::fs::write(&sub_file, "pub fn f() {}\n").unwrap();

    let trust_file = base.join("trust.scm");
    let (mut rt, _) = Runtime::open(&file, &base.join("j"), "emacs").unwrap();
    isolate_trust_store(&mut rt, &trust_file);

    // 1. user-only options are unconditionally rejected from .techne.scm and #:directory
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'direnv-enabled)").unwrap(), "#t");
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'server-eval)").unwrap(), "#t");
    let dir_err = rt.eval(&format!("(set-option! 'direnv-enabled #f #:directory {:?})", proj.to_string_lossy()));
    assert!(dir_err.is_err(), "setting user-only option with #:directory must fail");

    // 2. Risky compile-command is inert until trusted, and mode-line shows settings:pending
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'compile-command)").unwrap(), "#f");
    let snap = rt.snapshot();
    assert!(snap.pane().status.contains("settings:pending"), "expected settings:pending in status: {}", snap.pane().status);

    // user-only options must NOT appear in pending trust prompts
    let pending = rt.eval("(map cadr (buffer-project-settings-pending (current-buffer (current-session))))").unwrap();
    assert!(pending.contains("compile-command"), "pending={pending}");
    assert!(!pending.contains("direnv-enabled"), "user-only option leaked into pending: {pending}");
    assert!(!pending.contains("server-eval"), "user-only option leaked into pending: {pending}");

    // 3. Non-user principal ("agent") cannot approve, deny, or revoke trust
    let agent_err = rt.eval(&format!(
        "(with-operation 'agent \"agent\" 'settings (lambda () (project-settings-approve! {:?} 'compile-command \"echo TRUSTED_BUILD_OK\")))",
        proj.to_string_lossy()
    ));
    assert!(agent_err.is_err(), "agent principal must be refused from approving project trust");

    // 4. Interactive M-x project-settings-status shows *project-settings* view with pending entry
    mx(&mut rt, "project-settings-status");
    let status_snap = rt.snapshot();
    let status_pane = status_snap.panes.iter().find(|p| p.status.contains("*project-settings*")).expect("expected *project-settings* pane");
    let status_text = status_pane.text.to_string();
    assert!(status_text.contains("compile-command"), "expected compile-command in status view: {status_text}");
    assert!(status_text.contains("pending"), "expected pending in status view: {status_text}");
    keys(&mut rt, "q");

    // 5. Interactive M-x project-settings-trust approves compile-command
    mx(&mut rt, "project-settings-trust");
    until(&mut rt, "trust option picker", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Review project setting")));
    type_text(&mut rt, "compile-command");
    keys(&mut rt, "RET");
    until(&mut rt, "trust confirmation prompt", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("compile-command")));
    type_text(&mut rt, "approve");
    keys(&mut rt, "RET");
    let approved_snap = until(&mut rt, "trust approval message", |s| s.echo.contains("Trusted compile-command"));
    assert!(approved_snap.echo.contains("Trusted compile-command"));

    // Trust ledger persisted with 0600 permissions
    let mode = std::fs::metadata(&trust_file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "trust store must have 0600 permissions, got {:o}", mode);

    // compile-command is now active
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'compile-command)").unwrap(), "\"echo TRUSTED_BUILD_OK\"");

    // M-x compile uses trusted compile-command by default!
    mx(&mut rt, "compile");
    let comp_prompt = until(&mut rt, "compile command prompt", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Compile in ")));
    assert!(
        comp_prompt.minibuffer.as_ref().unwrap().input.contains("echo TRUSTED_BUILD_OK"),
        "expected compile prompt to default to trusted compile-command, got: {:?}",
        comp_prompt.minibuffer.as_ref().unwrap().input
    );
    keys(&mut rt, "RET");
    let done = until(&mut rt, "compile output", |s| compilation_pane(s).is_some_and(|p| p.text.to_string().contains("TRUSTED_BUILD_OK")));
    assert!(compilation_pane(&done).unwrap().text.to_string().contains("TRUSTED_BUILD_OK"));

    // 6. Fragment-composition defense across parent + child .techne.scm for lsp-settings
    rt.eval(&format!("(show-document! (current-session) (file-document {:?}))", sub_file.to_string_lossy())).unwrap();
    let _ = rt.snapshot();
    // Approving only child's partial fragment (dict "b" 2) must NOT activate merged (dict "a" 1 "b" 2)
    rt.eval(&format!("(project-settings-approve! {:?} 'lsp-settings (dict \"b\" 2))", sub.to_string_lossy())).unwrap();
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'lsp-settings)").unwrap(), "{}");
    // Approving the full effective merged dict activates both keys
    rt.eval(&format!("(project-settings-approve! {:?} 'lsp-settings (dict \"a\" 1 \"b\" 2))", sub.to_string_lossy())).unwrap();
    assert_eq!(rt.eval("(dict-ref (option (current-buffer (current-session)) 'lsp-settings) \"a\")").unwrap(), "1");
    assert_eq!(rt.eval("(dict-ref (option (current-buffer (current-session)) 'lsp-settings) \"b\")").unwrap(), "2");

    // 7. Modifying .techne.scm and saving auto-reloads and invalidates value trust
    rt.eval(&format!("(show-document! (current-session) (file-document {:?}))", techne_file.to_string_lossy())).unwrap();
    rt.eval(
        "(let* ((s (current-session)) (d (session-document s))) \
           (view-edit! (session-view s) \
             (list (list 0 (document-length d) \
               \"(techne-settings 1 (all (compile-command \\\"echo CHANGED_BUILD\\\")))\")) \
             \"new\"))",
    )
    .unwrap();
    keys(&mut rt, "C-x C-s");
    // Switch back to main.rs: changed value is untrusted again and mode-line shows settings:pending
    rt.eval(&format!("(show-document! (current-session) (file-document {:?}))", file.to_string_lossy())).unwrap();
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'compile-command)").unwrap(), "#f");
    assert!(rt.snapshot().pane().status.contains("settings:pending"));

    // 8. Malformed .techne.scm shows settings:err on mode-line without evaluating or crashing
    rt.eval(&format!("(show-document! (current-session) (file-document {:?}))", techne_file.to_string_lossy())).unwrap();
    rt.eval(
        "(let* ((s (current-session)) (d (session-document s))) \
           (view-edit! (session-view s) \
             (list (list 0 (document-length d) \
               \"(techne-settings 99 (all (indent-width 4)))\")) \
             \"new\"))",
    )
    .unwrap();
    keys(&mut rt, "C-x C-s");
    rt.eval(&format!("(show-document! (current-session) (file-document {:?}))", file.to_string_lossy())).unwrap();
    let err_snap = rt.snapshot();
    assert!(err_snap.pane().status.contains("settings:err"), "expected settings:err in status: {}", err_snap.pane().status);
    mx(&mut rt, "project-settings-status");
    let diag_snap = rt.snapshot();
    let diag_text = diag_snap
        .panes
        .iter()
        .find(|p| p.status.contains("*project-settings*"))
        .expect("expected *project-settings* pane on error")
        .text
        .to_string();
    assert!(diag_text.contains("Unsupported `.techne.scm` version"), "expected version error in status view: {diag_text}");
    keys(&mut rt, "q");

    // 9. Programmatic revoke clears trust immediately
    rt.eval(&format!("(project-settings-revoke! {:?} 'compile-command)", proj.to_string_lossy())).unwrap();
    assert_eq!(
        rt.eval(
            "(import (techne project-settings)) (length (filter (lambda (e) (eq? (cadr e) 'compile-command)) (project-trust-entries)))"
        )
        .unwrap(),
        "0"
    );
    assert!(rt.take_errors().is_empty());
}

/// Fake LSP server that logs its PID, every `workspace/didChangeConfiguration`,
/// pulls per-`scopeUri` `workspace/configuration` after every config change,
/// and logs `formattingOptions` on `textDocument/formatting`.
const FAKE_CONFIG_LSP: &str = r#"
import sys, os, json

LOG = sys.argv[1]
URI_ROOT = sys.argv[2]
URI_SUB = sys.argv[3]
req_seq = 0

def log(obj):
    with open(LOG, 'a') as f:
        f.write(json.dumps(obj, sort_keys=True) + '\n')

def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line: return None
        if line.strip() == b'': break
        if line.lower().startswith(b'content-length:'):
            length = int(line.split(b':')[1])
    return json.loads(sys.stdin.buffer.read(length))

def send(val):
    body = json.dumps(val).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(body) + body)
    sys.stdout.buffer.flush()

log({'event': 'start', 'pid': os.getpid()})

while True:
    msg = read()
    if msg is None:
        break
    method = msg.get('method')
    params = msg.get('params') or {}
    if method == 'initialize':
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': {'capabilities': {
            'positionEncoding': 'utf-8',
            'textDocumentSync': {'openClose': True, 'change': 2, 'save': {'includeText': False}},
            'documentFormattingProvider': True,
        }}})
    elif method == 'workspace/didChangeConfiguration':
        log({'event': 'didChangeConfiguration', 'settings': params.get('settings')})
        req_seq += 1
        send({
            'jsonrpc': '2.0',
            'id': f'cfg-{req_seq}',
            'method': 'workspace/configuration',
            'params': {'items': [
                {'scopeUri': URI_ROOT, 'section': 'server'},
                {'scopeUri': URI_SUB, 'section': 'server'},
            ]},
        })
    elif method is None and isinstance(msg.get('id'), str) and msg['id'].startswith('cfg-'):
        log({'event': 'configurationResponse', 'id': msg['id'], 'result': msg.get('result')})
    elif method == 'textDocument/formatting':
        log({'event': 'formatting', 'options': params.get('options')})
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': []})
    elif method == 'shutdown':
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': None})
    elif method == 'exit':
        break
    elif 'id' in msg:
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': None})
"#;

fn read_json_log(path: &Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn wait_log(rt: &mut Runtime, log_path: &Path, what: &str, pred: impl Fn(&[serde_json::Value]) -> bool) -> Vec<serde_json::Value> {
    let start = Instant::now();
    loop {
        rt.run_tasks(Duration::from_millis(15)).unwrap();
        let _ = rt.snapshot();
        let entries = read_json_log(log_path);
        if pred(&entries) {
            return entries;
        }
        assert!(
            start.elapsed() < Duration::from_secs(25),
            "timed out waiting for {what}; log={:?}; errors={:?}",
            entries,
            rt.take_errors()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn live_lsp_configuration_push_unset_scope_uri_formatting_and_no_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let proj = base.join("proj");
    let sub = proj.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    std::fs::write(proj.join("fake.toml"), "").unwrap();

    // EditorConfig sets tab indentation of width 2 for formattingOptions inheritance
    std::fs::write(
        proj.join(".editorconfig"),
        "root = true\n\
         [*.rs]\n\
         indent_style = tab\n\
         indent_size = 2\n",
    )
    .unwrap();

    // Root .techne.scm overrides a key and unsets a server default key
    std::fs::write(
        proj.join(".techne.scm"),
        "(techne-settings 1\n\
           (all (lsp-settings (dict \"server\" (dict \"overrideMe\" \"from-root\" \"dropMe\" (unset))))))\n",
    )
    .unwrap();
    // Subdir .techne.scm adds a nested key
    std::fs::write(
        sub.join(".techne.scm"),
        "(techne-settings 1\n\
           (all (lsp-settings (dict \"server\" (dict \"subOnly\" #t)))))\n",
    )
    .unwrap();

    let main_rs = proj.join("main.rs");
    let sub_rs = sub.join("nested.rs");
    std::fs::write(&main_rs, "fn main() {}\n").unwrap();
    std::fs::write(&sub_rs, "pub fn n() {}\n").unwrap();

    let fake_py = base.join("fake_cfg_lsp.py");
    std::fs::write(&fake_py, FAKE_CONFIG_LSP).unwrap();
    let log_path = base.join("lsp.jsonl");
    let uri_root = format!("file://{}", main_rs.display());
    let uri_sub = format!("file://{}", sub_rs.display());

    let (mut rt, _) = Runtime::open(&main_rs, &base.join("j"), "emacs").unwrap();
    isolate_trust_store(&mut rt, &base.join("trust.scm"));

    // Disable rust-analyzer and register our fake server with declared default settings
    rt.eval("(define-lsp-server rust-analyzer \"Not in these tests.\")").unwrap();
    rt.eval(&format!(
        "(define-lsp-server fake-cfg \"Fake config LSP.\" \
           #:command '(\"python3\" {:?} {:?} {:?} {:?}) \
           #:languages '(rust) \
           #:roots '(\"fake.toml\") \
           #:settings (dict \"server\" (dict \"defaultKey\" \"keep\" \"dropMe\" \"gone\" \"overrideMe\" \"base\")))",
        fake_py.to_string_lossy(),
        log_path.to_string_lossy(),
        uri_root,
        uri_sub
    ))
    .unwrap();
    // Trigger LSP attachment on main.rs
    rt.eval("(run-command (current-session) 'lsp 1)").unwrap();

    // 1. Initial configuration before trust: only server defaults (untrusted .techne.scm is inert)
    let entries = wait_log(&mut rt, &log_path, "initial configurationResponse", |es| {
        es.iter().any(|e| e["event"] == "configurationResponse" && e["id"] == "cfg-1")
    });
    let starts: Vec<_> = entries.iter().filter(|e| e["event"] == "start").collect();
    assert_eq!(starts.len(), 1, "server should start exactly once");
    let initial_pid = starts[0]["pid"].as_i64().unwrap();

    let cfg1 = entries.iter().find(|e| e["event"] == "didChangeConfiguration").unwrap();
    assert_eq!(cfg1["settings"]["server"]["defaultKey"], "keep");
    assert_eq!(cfg1["settings"]["server"]["dropMe"], "gone");
    assert_eq!(cfg1["settings"]["server"]["overrideMe"], "base");

    // 2. Approve root .techne.scm lsp-settings -> live didChangeConfiguration WITHOUT restart!
    rt.eval(&format!(
        "(begin (import (techne project-settings)) \
           (project-settings-approve! {:?} 'lsp-settings \
             (dict \"server\" (dict \"overrideMe\" \"from-root\" \"dropMe\" json-unset))))",
        proj.to_string_lossy()
    ))
    .unwrap();
    let entries = wait_log(&mut rt, &log_path, "cfg-2 after root approval", |es| {
        es.iter().any(|e| e["event"] == "configurationResponse" && e["id"] == "cfg-2")
    });
    let pushes: Vec<_> = entries.iter().filter(|e| e["event"] == "didChangeConfiguration").collect();
    assert_eq!(pushes.len(), 2);
    let srv2 = &pushes[1]["settings"]["server"];
    assert_eq!(srv2["defaultKey"], "keep");
    assert_eq!(srv2["overrideMe"], "from-root");
    assert!(srv2.get("dropMe").is_none(), "(unset) must remove server default key dropMe, got: {srv2}");

    // 3. Approve subdir merged lsp-settings and trigger a live user option change to check per-scopeUri
    rt.eval(&format!(
        "(begin (import (techne project-settings)) \
           (project-settings-approve! {:?} 'lsp-settings \
             (dict \"server\" (dict \"overrideMe\" \"from-root\" \"dropMe\" json-unset \"subOnly\" #t))))",
        sub.to_string_lossy()
    ))
    .unwrap();
    rt.eval("(set-option! 'lsp-settings (dict \"server\" (dict \"userKey\" 42)))").unwrap();
    let entries = wait_log(&mut rt, &log_path, "configurationResponse with userKey=42", |es| {
        es.iter().any(|e| e["event"] == "configurationResponse" && e["result"][0]["userKey"] == 42 && e["result"][1]["subOnly"] == true)
    });
    let resp = entries.iter().rev().find(|e| e["event"] == "configurationResponse" && e["result"][0]["userKey"] == 42).unwrap();
    // Root scopeUri has userKey=42, overrideMe="from-root", no dropMe, no subOnly
    assert_eq!(resp["result"][0]["defaultKey"], "keep");
    assert_eq!(resp["result"][0]["overrideMe"], "from-root");
    assert_eq!(resp["result"][0]["userKey"], 42);
    assert!(resp["result"][0].get("dropMe").is_none());
    assert!(resp["result"][0].get("subOnly").is_none());
    // Subdir scopeUri ALSO has subOnly=true!
    assert_eq!(resp["result"][1]["subOnly"], true);
    assert_eq!(resp["result"][1]["overrideMe"], "from-root");
    assert_eq!(resp["result"][1]["userKey"], 42);

    // 4. Revoke root trust -> live push reverts overrideMe to "base" and restores dropMe="gone"
    rt.eval(&format!("(project-settings-revoke! {:?} 'lsp-settings)", proj.to_string_lossy())).unwrap();
    let entries = wait_log(&mut rt, &log_path, "didChangeConfiguration after revoke", |es| {
        es.iter().rev().find(|e| e["event"] == "didChangeConfiguration").is_some_and(|e| {
            e["settings"]["server"]["overrideMe"] == "base"
                && e["settings"]["server"]["dropMe"] == "gone"
                && e["settings"]["server"]["userKey"] == 42
        })
    });

    // Server PID MUST still be the original single process (ZERO restarts!)
    let starts: Vec<_> = entries.iter().filter(|e| e["event"] == "start").collect();
    assert_eq!(starts.len(), 1, "LSP server must NOT restart on live configuration changes");
    assert_eq!(starts[0]["pid"].as_i64().unwrap(), initial_pid);

    // 5. Formatting inherits EditorConfig indent_size=2 and indent_style=tab (insertSpaces=false)
    rt.eval("(run-command (current-session) 'lsp-format-buffer 1)").unwrap();
    let entries = wait_log(&mut rt, &log_path, "formatting options", |es| es.iter().any(|e| e["event"] == "formatting"));
    let fmt = entries.iter().find(|e| e["event"] == "formatting").unwrap();
    assert_eq!(fmt["options"]["tabSize"], 2);
    assert_eq!(fmt["options"]["insertSpaces"], false);
    assert!(rt.take_errors().is_empty());
}

fn direnv_bin() -> Option<PathBuf> {
    let out = Command::new("which").arg("direnv").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() { None } else { Some(PathBuf::from(path)) }
}

fn make_executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

#[test]
fn lsp_session_identity_tracks_direnv_env_id() {
    let Some(real_direnv) = direnv_bin() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let xdg = base.join("xdg");
    std::fs::create_dir_all(&xdg).unwrap();
    let wrapper = base.join("isolated-direnv.sh");
    make_executable(
        &wrapper,
        &format!(
            "#!/bin/sh\n\
             export XDG_DATA_HOME={0:?}\n\
             export XDG_CONFIG_HOME={0:?}\n\
             export XDG_CACHE_HOME={0:?}\n\
             unset DIRENV_DIR DIRENV_FILE DIRENV_WATCHES DIRENV_DIFF\n\
             exec {1:?} \"$@\"\n",
            xdg, real_direnv
        ),
    );

    let proj = base.join("proj");
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    std::fs::write(proj.join("fake.toml"), "").unwrap();
    std::fs::write(proj.join(".envrc"), "export TECHNE_LSP_ENV_MARKER=v1\n").unwrap();
    let main_rs = proj.join("main.rs");
    std::fs::write(&main_rs, "fn main() {}\n").unwrap();

    let fake_py = base.join("fake_env_lsp.py");
    std::fs::write(&fake_py, FAKE_CONFIG_LSP).unwrap();
    let log_path = base.join("env_lsp.jsonl");
    let uri = format!("file://{}", main_rs.display());

    let (mut rt, _) = Runtime::open(&main_rs, &base.join("j"), "emacs").unwrap();
    isolate_trust_store(&mut rt, &base.join("trust.scm"));
    rt.eval(&format!("(set-option! 'direnv-program {:?})", wrapper.to_string_lossy())).unwrap();
    rt.eval(&format!("(set-option! 'direnv-data-dir {:?})", xdg.join("direnv").to_string_lossy())).unwrap();
    rt.eval("(execution-site-invalidate!)").unwrap();
    rt.eval("(define-lsp-server rust-analyzer \"Not in these tests.\")").unwrap();
    rt.eval(&format!(
        "(define-lsp-server fake-env \"Fake env LSP.\" \
           #:command '(\"python3\" {:?} {:?} {:?} {:?}) \
           #:languages '(rust) \
           #:roots '(\"fake.toml\") \
           #:settings (dict \"env\" \"test\"))",
        fake_py.to_string_lossy(),
        log_path.to_string_lossy(),
        uri,
        uri
    ))
    .unwrap();

    // Start LSP while .envrc is still blocked -> env-id is "base"
    rt.eval("(run-command (current-session) 'lsp 1)").unwrap();
    wait_log(&mut rt, &log_path, "first server start", |es| es.iter().any(|e| e["event"] == "didChangeConfiguration"));
    let env_id_before = rt.eval("(lsp-session-env-id (car (lsp-buffer-sessions (current-buffer (current-session)))))").unwrap();
    assert_eq!(env_id_before, "\"base\"");

    // Allow .envrc interactively -> resolved environment gets a non-"base" env-id
    mx(&mut rt, "direnv-allow");
    until(&mut rt, "direnv allow prompt", |s| {
        s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Allow ") && m.prompt.contains(".envrc"))
    });
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "direnv active", |s| s.echo.contains("direnv active"));

    // Starting LSP now sees a distinct env-id in session identity
    rt.eval("(run-command (current-session) 'lsp 1)").unwrap();
    let entries =
        wait_log(&mut rt, &log_path, "second server start for new env-id", |es| es.iter().filter(|e| e["event"] == "start").count() >= 2);
    assert!(entries.iter().filter(|e| e["event"] == "start").count() >= 2);
    let env_ids = rt.eval("(map lsp-session-env-id (lsp-buffer-sessions (current-buffer (current-session))))").unwrap();
    assert!(env_ids.contains("\"base\""), "expected original \"base\" env-id session: {env_ids}");
    let distinct_count = rt
        .eval("(length (filter (lambda (id) (not (equal? id \"base\"))) (map lsp-session-env-id (lsp-buffer-sessions (current-buffer (current-session))))))")
        .unwrap();
    assert_eq!(distinct_count, "1", "expected a distinct non-base env-id session after direnv allow: {env_ids}");
    assert!(rt.take_errors().is_empty());
}

#[test]
fn security_hardening_agent_user_only_scope_shadowing_and_reload_ancestors() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let proj = base.join("proj");
    let sub = proj.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::create_dir_all(proj.join(".git")).unwrap();

    std::fs::write(proj.join(".techne.scm"), "(techne-settings 1 (all (tab-width 6)))\n").unwrap();
    let sub_file = sub.join("main.rs");
    std::fs::write(&sub_file, "fn main() {}\n").unwrap();

    let (mut rt, _) = Runtime::open(&sub_file, &base.join("j"), "emacs").unwrap();
    isolate_trust_store(&mut rt, &base.join("trust.scm"));

    // 1. Agent principal cannot write #:user-only option via set-option! or via set-option command
    let set_opt_err = rt.eval("(with-operation 'agent \"agent\" 'settings (lambda () (set-option! 'direnv-enabled #f)))");
    assert!(set_opt_err.is_err(), "agent principal must be rejected from set-option! on user-only option");

    let invoke_err =
        rt.eval("(invoke (current-session) 'set-option (dict 'name 'direnv-enabled 'value #f 'global #t) #:principal \"agent\")");
    assert!(invoke_err.is_err(), "agent principal must be rejected from set-option command on user-only option");
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'direnv-enabled)").unwrap(), "#t");

    // 2. Non-root package scope cannot shadow a root #:user-only option
    let shadow_err = rt.eval(
        "(let ((pkg (make-scope 'untrusted-pkg))) \
           (with-scope pkg \
             (register-option! 'direnv-enabled #f \"shadowed\" #:type 'boolean)))",
    );
    assert!(shadow_err.is_err(), "non-root scope must not shadow root #:user-only option");

    // 3. Even root scope cannot weaken #:user-only #t to #f on an existing option
    let weaken_err = rt.eval("(register-option! 'direnv-enabled #f \"weakened\" #:type 'boolean #:user-only #f)");
    assert!(weaken_err.is_err(), "re-registering a #:user-only option without #:user-only #t must fail");

    // 4. Out-of-band edit to parent/root .techne.scm is picked up by M-x project-settings-reload from a subdir
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'tab-width)").unwrap(), "6");
    std::fs::write(proj.join(".techne.scm"), "(techne-settings 1 (all (tab-width 3)))\n").unwrap();
    // Cached read stays 6 before reload
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'tab-width)").unwrap(), "6");
    mx(&mut rt, "project-settings-reload");
    let _ = rt.snapshot();
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'tab-width)").unwrap(), "3");
    assert!(rt.take_errors().is_empty());
}

#[test]
fn before_save_cr_and_preserve_crlf_line_endings() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let proj = base.join("proj");
    std::fs::create_dir_all(proj.join(".git")).unwrap();

    std::fs::write(
        proj.join(".editorconfig"),
        "root = true\n\
         [*.crlf]\n\
         trim_trailing_whitespace = true\n\
         insert_final_newline = true\n\
         [*.cr]\n\
         end_of_line = cr\n\
         trim_trailing_whitespace = true\n\
         insert_final_newline = true\n",
    )
    .unwrap();

    let crlf_file = proj.join("win.crlf");
    let crlf_orig = "one   \r\ntwo\t \r\nthree  ";
    std::fs::write(&crlf_file, crlf_orig).unwrap();

    let cr_file = proj.join("mac.cr");
    let cr_orig = "a  \rb \rc ";
    std::fs::write(&cr_file, cr_orig).unwrap();

    let (mut rt, _) = Runtime::open(&crlf_file, &base.join("j"), "emacs").unwrap();
    isolate_trust_store(&mut rt, &base.join("trust.scm"));

    // 1. Preserve CRLF: trims trailing whitespace before \r\n and appends \r\n final newline
    keys(&mut rt, "C-x C-s");
    assert_eq!(std::fs::read_to_string(&crlf_file).unwrap(), "one\r\ntwo\r\nthree\r\n");
    // Saving again does not double-append final newline
    keys(&mut rt, "C-x C-s");
    assert_eq!(std::fs::read_to_string(&crlf_file).unwrap(), "one\r\ntwo\r\nthree\r\n");
    // Single undo reverts cleanly
    keys(&mut rt, "C-/");
    assert_eq!(rt.snapshot().pane().text.to_string(), crlf_orig);

    // 2. Explicit end_of_line = cr trims before \r, appends \r final newline, and is idempotent
    rt.eval(&format!("(show-document! (current-session) (file-document {:?}))", cr_file.to_string_lossy())).unwrap();
    let _ = rt.snapshot();
    keys(&mut rt, "C-x C-s");
    assert_eq!(std::fs::read_to_string(&cr_file).unwrap(), "a\rb\rc\r");
    keys(&mut rt, "C-x C-s");
    assert_eq!(std::fs::read_to_string(&cr_file).unwrap(), "a\rb\rc\r");
    keys(&mut rt, "C-/");
    assert_eq!(rt.snapshot().pane().text.to_string(), cr_orig);
    assert!(rt.take_errors().is_empty());
}

#[test]
fn lsp_auto_false_honored_on_first_file_open_via_trusted_project_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let scratch = base.join("scratch.txt");
    std::fs::write(&scratch, "hello\n").unwrap();

    let proj_off = base.join("proj_off");
    std::fs::create_dir_all(proj_off.join(".git")).unwrap();
    std::fs::write(proj_off.join("fake.toml"), "").unwrap();
    std::fs::write(proj_off.join(".techne.scm"), "(techne-settings 1 (all (lsp-auto #f)))\n").unwrap();
    let off_rs = proj_off.join("main.rs");
    std::fs::write(&off_rs, "fn off() {}\n").unwrap();

    let proj_on = base.join("proj_on");
    std::fs::create_dir_all(proj_on.join(".git")).unwrap();
    std::fs::write(proj_on.join("fake.toml"), "").unwrap();
    let on_rs = proj_on.join("main.rs");
    std::fs::write(&on_rs, "fn on() {}\n").unwrap();

    let fake_py = base.join("fake_auto_lsp.py");
    std::fs::write(&fake_py, FAKE_CONFIG_LSP).unwrap();
    let log_path = base.join("auto_lsp.jsonl");
    let uri_off = format!("file://{}", off_rs.display());
    let uri_on = format!("file://{}", on_rs.display());

    let (mut rt, _) = Runtime::open(&scratch, &base.join("j"), "emacs").unwrap();
    isolate_trust_store(&mut rt, &base.join("trust.scm"));

    // Register fake server and pre-approve (lsp-auto #f) for proj_off BEFORE opening proj_off/main.rs
    rt.eval("(define-lsp-server rust-analyzer \"Not in these tests.\")").unwrap();
    rt.eval(&format!(
        "(define-lsp-server fake-auto \"Fake auto LSP.\" \
           #:command '(\"python3\" {:?} {:?} {:?} {:?}) \
           #:languages '(rust) \
           #:roots '(\"fake.toml\"))",
        fake_py.to_string_lossy(),
        log_path.to_string_lossy(),
        uri_off,
        uri_on
    ))
    .unwrap();
    rt.eval(&format!(
        "(begin (import (techne project-settings)) \
           (project-settings-approve! {:?} 'lsp-auto #f))",
        proj_off.to_string_lossy()
    ))
    .unwrap();

    // Open proj_off/main.rs for the VERY FIRST time: project-settings must load before lsp checks lsp-auto
    rt.eval(&format!("(show-document! (current-session) (file-document {:?}))", off_rs.to_string_lossy())).unwrap();
    for _ in 0..15 {
        rt.run_tasks(Duration::from_millis(20)).unwrap();
        let _ = rt.snapshot();
    }
    assert_eq!(rt.eval("(option (current-buffer (current-session)) 'lsp-auto)").unwrap(), "#f");
    assert_eq!(
        rt.eval("(length (lsp-buffer-sessions (current-buffer (current-session))))").unwrap(),
        "0",
        "LSP must NOT auto-attach when trusted .techne.scm sets (lsp-auto #f)"
    );
    assert!(read_json_log(&log_path).is_empty(), "LSP server process must not have started for proj_off");

    // Control: opening proj_on/main.rs DOES auto-start the fake LSP server
    rt.eval(&format!("(show-document! (current-session) (file-document {:?}))", on_rs.to_string_lossy())).unwrap();
    let entries = wait_log(&mut rt, &log_path, "control auto-start in proj_on", |es| es.iter().any(|e| e["event"] == "start"));
    assert_eq!(entries.iter().filter(|e| e["event"] == "start").count(), 1);
    assert!(rt.take_errors().is_empty());
}
