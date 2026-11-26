//! Execution-site and local `direnv` editor integration (`lisp/editor/direnv.scm`,
//! `lisp/techne/execution-site.sld`): non-blocking status/allow/deny/reload
//! commands, confirmation on allow, secret value redaction (stdout and stderr),
//! watched-file save invalidation, compile, and async LSP auto-discovery from a
//! direnv-provided `PATH`.

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

/// Wrapper around the real `direnv` binary using isolated XDG directories so
/// tests never touch the user's allowlist or race on process environment.
/// Returns `(wrapper_path, direnv_data_dir)`.
fn write_isolated_direnv(root: &Path, real_direnv: &Path) -> (PathBuf, PathBuf) {
    let root = root.canonicalize().unwrap();
    let xdg_data = root.join("xdg-data");
    let xdg_config = root.join("xdg-config");
    let xdg_cache = root.join("xdg-cache");
    std::fs::create_dir_all(&xdg_data).unwrap();
    std::fs::create_dir_all(&xdg_config).unwrap();
    std::fs::create_dir_all(&xdg_cache).unwrap();
    let wrapper = root.join("isolated-direnv.sh");
    make_executable(
        &wrapper,
        &format!(
            "#!/bin/sh\n\
             export XDG_DATA_HOME={:?}\n\
             export XDG_CONFIG_HOME={:?}\n\
             export XDG_CACHE_HOME={:?}\n\
             unset DIRENV_DIR DIRENV_FILE DIRENV_WATCHES DIRENV_DIFF\n\
             exec {:?} \"$@\"\n",
            xdg_data, xdg_config, xdg_cache, real_direnv
        ),
    );
    (wrapper, xdg_data.join("direnv"))
}

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
        rt.run_tasks(Duration::from_millis(10)).unwrap();
        let s = rt.snapshot();
        if done(&s) {
            return s;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "timed out waiting for {what}: echo={:?} status={:?} minibuffer={:?}",
            s.echo,
            s.pane().status,
            s.minibuffer.as_ref().map(|m| &m.prompt)
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn compilation(s: &Snapshot) -> Option<&Pane> {
    s.panes.iter().find(|p| p.status.contains("*compilation*"))
}

fn messages_text(rt: &mut Runtime) -> String {
    rt.eval("(let ((b (buffer-named \"*Messages*\"))) (if b (let ((d (buffer-document b))) (document-substring d 0 (document-length d))) \"\"))")
        .unwrap()
}

fn approve_direnv_interactively(rt: &mut Runtime) -> Snapshot {
    mx(rt, "direnv-allow");
    until(rt, "direnv allow confirmation prompt", |s| {
        s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Allow ") && m.prompt.contains(".envrc"))
    });
    type_text(rt, "yes");
    keys(rt, "RET");
    until(rt, "direnv allow completion", |s| s.echo.contains("direnv active") || s.echo.contains("direnv error"))
}

#[test]
fn direnv_status_allow_deny_save_invalidation_and_redaction() {
    let Some(real_direnv) = direnv_bin() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let (wrapper, direnv_data) = write_isolated_direnv(tmp.path(), &real_direnv);
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    std::fs::write(
        proj.join(".envrc"),
        "export TECHNE_TEST_SECRET=\"super-secret-value-999\"\n\
         export TECHNE_TEST_FLAG=\"enabled\"\n",
    )
    .unwrap();
    let file = proj.join("notes.txt");
    std::fs::write(&file, "hello\n").unwrap();

    let mut rt = Runtime::open(&file, &tmp.path().join("notes.journal"), "emacs").unwrap().0;
    rt.eval(&format!("(set-option! 'direnv-program {:?})", wrapper.to_string_lossy())).unwrap();
    rt.eval(&format!("(set-option! 'direnv-data-dir {:?})", direnv_data.to_string_lossy())).unwrap();
    rt.eval("(execution-site-invalidate!)").unwrap();

    // Blocked before allow (non-blocking async status).
    mx(&mut rt, "direnv-status");
    let s = until(&mut rt, "direnv blocked status", |s| s.echo.contains("direnv blocked"));
    assert!(s.echo.contains("direnv-allow"), "expected allow hint, got: {}", s.echo);
    assert!(s.pane().status.contains("direnv:blocked"), "expected blocked mode-line, got: {}", s.pane().status);

    // M-x direnv-allow confirms first; answering "no" cancels without approving.
    mx(&mut rt, "direnv-allow");
    let prompt_snap = until(&mut rt, "direnv allow prompt", |s| {
        s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Allow ") && m.prompt.contains(".envrc"))
    });
    assert_eq!(prompt_snap.minibuffer.as_ref().unwrap().rows[0].text(0), "no");
    keys(&mut rt, "RET");
    assert!(rt.snapshot().echo.contains("cancelled"), "expected cancellation echo: {}", rt.snapshot().echo);

    // Confirming "yes" approves: names appear, secret values NEVER appear.
    let s = approve_direnv_interactively(&mut rt);
    assert!(s.echo.contains("direnv active"), "expected active status, got: {}", s.echo);
    assert!(s.echo.contains("+TECHNE_TEST_FLAG"), "expected flag name in summary: {}", s.echo);
    assert!(s.echo.contains("+TECHNE_TEST_SECRET"), "expected secret var name in summary: {}", s.echo);
    assert!(!s.echo.contains("super-secret-value-999"), "secret value leaked into echo: {}", s.echo);
    assert!(s.pane().status.contains("direnv"), "expected active mode-line: {}", s.pane().status);
    assert!(!s.pane().status.contains("direnv:blocked"), "unexpected blocked mode-line: {}", s.pane().status);
    assert!(!messages_text(&mut rt).contains("super-secret-value-999"), "secret value leaked into *Messages*");

    // Open and save .envrc inside Techne: document-saved hook invalidates cache and refreshes asynchronously.
    rt.eval(&format!("(show-document! (current-session) (file-document {:?}))", proj.join(".envrc").to_string_lossy())).unwrap();
    keys(&mut rt, "M-> RET");
    type_text(&mut rt, "echo 'STDERR_SECRET_LEAK_777' >&2; exit 1");
    keys(&mut rt, "C-x C-s");

    let cached = rt.eval(&format!("(execution-site-cached-snapshot {:?})", proj.to_string_lossy())).unwrap();
    assert_eq!(cached, "#f", "saving .envrc should immediately invalidate cached snapshot");

    // Async refresh from document-saved transitions mode-line to direnv:blocked.
    let s = until(&mut rt, "async blocked refresh after save", |s| s.pane().status.contains("direnv:blocked"));
    assert!(s.pane().status.contains("direnv:blocked"));

    // Approving the failing .envrc reports sanitized error and NEVER leaks stderr secret.
    let s = approve_direnv_interactively(&mut rt);
    assert!(s.echo.contains("direnv error"), "expected error status, got: {}", s.echo);
    assert!(!s.echo.contains("STDERR_SECRET_LEAK_777"), "stderr secret leaked into echo: {}", s.echo);
    assert!(!messages_text(&mut rt).contains("STDERR_SECRET_LEAK_777"), "stderr secret leaked into *Messages*");

    // Deny clears and reports denied.
    mx(&mut rt, "direnv-deny");
    let s = until(&mut rt, "direnv denied status", |s| s.echo.contains("direnv denied"));
    assert!(s.pane().status.contains("direnv:denied"), "expected denied mode-line: {}", s.pane().status);

    // Reload refreshes cleanly.
    mx(&mut rt, "direnv-reload");
    let s = until(&mut rt, "direnv reload status", |s| s.echo.contains("direnv denied"));
    assert!(s.echo.contains("direnv denied"));
    assert!(rt.take_errors().is_empty());
}

#[test]
fn compile_uses_project_direnv_environment_and_path() {
    let Some(real_direnv) = direnv_bin() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let (wrapper, direnv_data) = write_isolated_direnv(tmp.path(), &real_direnv);
    let proj = tmp.path().join("proj");
    let bin_dir = proj.join("devbin");
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    std::fs::create_dir_all(&bin_dir).unwrap();

    make_executable(&bin_dir.join("techne-direnv-builder"), "#!/bin/sh\necho \"BUILT_WITH=$PROJ_BUILD_TOKEN\"\n");
    std::fs::write(proj.join(".envrc"), "PATH_add devbin\nexport PROJ_BUILD_TOKEN=\"from-direnv-env\"\n").unwrap();
    let file = proj.join("main.txt");
    std::fs::write(&file, "hello\n").unwrap();

    let mut rt = Runtime::open(&file, &tmp.path().join("compile.journal"), "emacs").unwrap().0;
    rt.eval(&format!("(set-option! 'direnv-program {:?})", wrapper.to_string_lossy())).unwrap();
    rt.eval(&format!("(set-option! 'direnv-data-dir {:?})", direnv_data.to_string_lossy())).unwrap();
    rt.eval("(execution-site-invalidate!)").unwrap();

    let s = approve_direnv_interactively(&mut rt);
    assert!(s.echo.contains("direnv active"));

    keys(&mut rt, "M-x");
    type_text(&mut rt, "compile");
    keys(&mut rt, "M-RET");
    keys(&mut rt, "C-a C-k");
    type_text(&mut rt, "techne-direnv-builder");
    keys(&mut rt, "RET");

    let s = until(&mut rt, "direnv compilation", |s| compilation(s).is_some_and(|p| p.text.to_string().contains("Compilation finished")));
    let text = compilation(&s).unwrap().text.to_string();
    assert!(text.contains("BUILT_WITH=from-direnv-env"), "expected direnv tool & var in compile output:\n{text}");
    assert!(rt.take_errors().is_empty());
}

#[test]
fn lsp_discovers_and_launches_server_from_direnv_path_asynchronously() {
    let Some(real_direnv) = direnv_bin() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let (wrapper, direnv_data) = write_isolated_direnv(tmp.path(), &real_direnv);
    let proj = tmp.path().join("proj");
    let sub = proj.join("src");
    let bin_dir = proj.join("devshell-bin");
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::create_dir_all(&bin_dir).unwrap();

    let log_file = tmp.path().join("lsp-env.log");
    let server_bin = bin_dir.join("techne-fake-direnv-lsp");
    let server_script = format!(
        "#!/usr/bin/env python3\nimport sys, os, json\nLOG = {:?}\n{}",
        log_file.to_string_lossy(),
        r#"def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line.strip() == b'':
            break
        if line.lower().startswith(b'content-length:'):
            length = int(line.split(b':')[1])
    return json.loads(sys.stdin.buffer.read(length))

def send(val):
    body = json.dumps(val).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(body) + body)
    sys.stdout.buffer.flush()

while True:
    msg = read()
    if msg is None:
        break
    m = msg.get('method')
    if m == 'initialize':
        with open(LOG, 'w') as f:
            f.write('DEVSHELL_MARKER=' + os.environ.get('DEVSHELL_MARKER', '') + '\n')
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': {'capabilities': {'textDocumentSync': 1}}})
    elif m == 'shutdown':
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': None})
    elif m == 'exit':
        break
"#
    );
    make_executable(&server_bin, &server_script);

    std::fs::write(proj.join(".envrc"), "PATH_add devshell-bin\nexport DEVSHELL_MARKER=\"nix-devshell-ok\"\n").unwrap();
    // Pre-allow via isolated direnv so opening the file auto-warms and auto-attaches.
    let status = Command::new(&wrapper).args(["allow", proj.to_str().unwrap()]).current_dir(&proj).status().unwrap();
    assert!(status.success());

    let dummy = tmp.path().join("scratch.txt");
    std::fs::write(&dummy, "outside\n").unwrap();
    let proj_file = sub.join("app.txt");
    std::fs::write(&proj_file, "inside\n").unwrap();

    let mut rt = Runtime::open(&dummy, &tmp.path().join("lsp.journal"), "emacs").unwrap().0;
    rt.eval(&format!("(set-option! 'direnv-program {:?})", wrapper.to_string_lossy())).unwrap();
    rt.eval(&format!("(set-option! 'direnv-data-dir {:?})", direnv_data.to_string_lossy())).unwrap();
    rt.eval("(execution-site-invalidate!)").unwrap();

    // Register a server whose bare command exists ONLY in devshell-bin (not system PATH).
    rt.eval(
        "(register-lsp-server! 'direnv-fake \"Fake LSP from direnv PATH.\" \
           #:command '(\"techne-fake-direnv-lsp\") \
           #:modes '(text-mode))",
    )
    .unwrap();

    // Open the project subdirectory file: file-buffer-added must NOT block synchronously!
    rt.eval(&format!("(show-document! (current-session) (file-document {:?}))", proj_file.to_string_lossy())).unwrap();
    assert!(
        rt.eval(&format!("(execution-site-cached-snapshot {:?})", sub.to_string_lossy())).unwrap() == "#f",
        "file open should defer direnv evaluation asynchronously rather than blocking"
    );

    // Once background tasks run, direnv warms and LSP initializes with the devshell env.
    until(&mut rt, "direnv LSP to initialize", |_| log_file.exists());
    let logged = std::fs::read_to_string(&log_file).unwrap();
    assert_eq!(logged.trim(), "DEVSHELL_MARKER=nix-devshell-ok");
    assert!(rt.take_errors().is_empty());
}

#[test]
fn execution_site_first_import_during_package_staging_evaluates() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let envrc = proj.join(".envrc");
    std::fs::write(&envrc, "export STAGED_OK=yes\n").unwrap();
    let fake_direnv = tmp.path().join("fake-direnv.sh");
    let canon_envrc = envrc.canonicalize().unwrap();
    make_executable(
        &fake_direnv,
        &format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"status\" ]; then\n\
               printf '{{\"state\":{{\"foundRC\":{{\"path\":\"{}\",\"allowed\":0}}}}}}\\n'\n\
               exit 0\n\
             fi\n\
             if [ \"$1\" = \"export\" ]; then\n\
               printf '{{\"STAGED_OK\":\"yes\"}}\\n'\n\
               exit 0\n\
             fi\n\
             exit 0\n",
            canon_envrc.to_string_lossy()
        ),
    );
    let pkg_dir = tmp.path().join("staging-pkg");
    std::fs::create_dir_all(&pkg_dir).unwrap();
    let pkg_file = pkg_dir.join("main.scm");
    std::fs::write(&pkg_file, "(import (techne execution-site))\n(define staged-loaded #t)\n").unwrap();
    let mut vm = techne_vm::vm::Vm::new();
    techne_node::install(&mut vm).unwrap();
    techne_editor::install(&mut vm);
    // Confirm execution-site has not been loaded yet before package staging.
    let loaded_before =
        vm.eval_in(0, "<test>", "(guard (e (#t #f)) (eval 'execution-site-env-snapshot \"(techne execution-site)\") #t)").unwrap();
    assert_eq!(vm.repr(&loaded_before), "#f");
    vm.eval_in(0, "<test>", &format!("(load-package 'staging-es-pkg {:?})", pkg_file.to_string_lossy())).unwrap();
    vm.eval_in(0, "<test>", "(unload-package 'staging-es-pkg)").unwrap();
    // Now evaluate direnv through the adopted %spawn-in-root-scope callback.
    let status = vm
        .eval_in(
            0,
            "<test>",
            &format!(
                "(let* ((es (environment '(techne execution-site))) \
                        (p-prog (eval 'default-direnv-program es)) \
                        (snap-fn (eval 'execution-site-env-snapshot es)) \
                        (status-fn (eval 'env-snapshot-status es)) \
                        (msg-fn (eval 'env-snapshot-message es))) \
                   (parameterize ((p-prog {:?})) \
                     (let ((snap (snap-fn {:?}))) \
                       (list (status-fn snap) (msg-fn snap)))))",
                fake_direnv.to_string_lossy(),
                proj.to_string_lossy()
            ),
        )
        .unwrap();
    assert_eq!(vm.repr(&status), "(active \"Active\")");
}
