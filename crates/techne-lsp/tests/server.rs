//! The server over stdio: diagnostics for a file using R7RS libraries,
//! included files and the runtime's procedures.

use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

fn send(stdin: &mut impl Write, msg: &str) {
    write!(stdin, "Content-Length: {}\r\n\r\n{msg}", msg.len()).unwrap();
    stdin.flush().unwrap();
}

fn recv(out: &mut impl BufRead) -> String {
    let mut len = 0;
    loop {
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        if line == "\r\n" {
            break;
        }
        if let Some(n) = line.strip_prefix("Content-Length: ") {
            len = n.trim().parse().unwrap();
        }
    }
    let mut body = vec![0; len];
    out.read_exact(&mut body).unwrap();
    String::from_utf8(body).unwrap()
}

fn server() -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_techne-lsp")).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    send(&mut stdin, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#);
    recv(&mut out);
    send(&mut stdin, r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    (child, stdin, out)
}

/// The diagnostics' messages for `text` opened as `path`.
fn diagnostics(path: &std::path::Path, text: &str) -> String {
    let (mut child, mut stdin, mut out) = server();
    let open = format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"file://{}","languageId":"scheme","version":1,"text":{:?}}}}}}}"#,
        path.display(),
        text
    );
    send(&mut stdin, &open);
    let reply = loop {
        let msg = recv(&mut out);
        if msg.contains("publishDiagnostics") {
            break msg;
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    reply
}

#[test]
fn libraries_includes_and_runtime_procedures() {
    let dir = std::env::temp_dir().join(format!("techne-lsp-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    std::fs::write(
        dir.join("lib/tools.sld"),
        "(define-library (lib tools) (export twice (rename hidden shown)) (import (scheme base)) (include \"tools-body.scm\") (begin (define hidden 1)))",
    )
    .unwrap();
    std::fs::write(dir.join("lib/tools-body.scm"), "(define (twice x) (* 2 x))").unwrap();
    let main = dir.join("main.scm");
    let text = r#"(import (scheme base) (prefix (lib tools) t:) (no such))
(define-library (here) (export f) (import (scheme base)) (begin (define (f) "Return 1." 1)))
(import (rename (here) (f g)))
(list (t:twice t:shown) (g) (process-spawn "true" '()) (make-document "x") (tcp-connect "localhost" 1) nowhere)
(define-command (hello s n) "Hello." (message! s "hi"))"#;
    let reply = diagnostics(&main, text);
    let messages: Vec<&str> = reply.split("\"message\":\"").skip(1).map(|m| m.split('"').next().unwrap()).collect();
    assert_eq!(messages, ["cannot find library (no such)", "unbound identifier: nowhere"], "{reply}");
}

#[test]
fn a_file_in_a_package_finds_the_packages_libraries() {
    let packages = std::env::temp_dir().join(format!("techne-lsp-packages-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&packages);
    let dir = packages.join("pkg");
    std::fs::create_dir_all(dir.join("lib/techne/pkg")).unwrap();
    // A library of the package's (owner pkg) beside the importing file, where a
    // file in no package would find it: not the package's, which is in lib/.
    std::fs::create_dir_all(dir.join("tests/techne/pkg")).unwrap();
    std::fs::write(dir.join("tests/techne/pkg/tools.sld"), "(define-library (techne pkg tools) (export) (import (scheme base)) (begin))")
        .unwrap();
    std::fs::write(dir.join("tests/techne/pkg/absent.sld"), "(define-library (techne pkg absent) (export) (import (scheme base)) (begin))")
        .unwrap();
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    // The package depends on one beside it, not on the other there.
    std::fs::write(dir.join("techne-package.scm"), "(package (techne pkg) (depends ((acme dep) (path \"../dep\"))))").unwrap();
    for name in ["dep", "undeclared"] {
        std::fs::create_dir_all(packages.join(format!("{name}/lib/acme/{name}"))).unwrap();
        std::fs::write(packages.join(format!("{name}/techne-package.scm")), format!("(package (acme {name}))")).unwrap();
        std::fs::write(
            packages.join(format!("{name}/lib/acme/{name}/more.sld")),
            format!(
                "(define-library (acme {name} more) (export thrice-{name}) (import (scheme base)) (begin (define (thrice-{name} x) (* 3 x))))"
            ),
        )
        .unwrap();
    }
    // A file of another (owner pkg) under lib/ is there, and no library of (techne pkg)'s.
    std::fs::create_dir_all(dir.join("lib/other/pkg")).unwrap();
    std::fs::write(dir.join("lib/other/pkg/tools.sld"), "(define-library (other pkg tools) (export) (import (scheme base)) (begin))")
        .unwrap();
    std::fs::write(
        dir.join("lib/techne/pkg/tools.sld"),
        "(define-library (techne pkg tools) (export twice) (import (scheme base)) (begin (define (twice x) (* 2 x))))",
    )
    .unwrap();
    let reply = diagnostics(
        &dir.join("tests/main.scm"),
        "(import (scheme base) (techne path) (techne pkg tools) (acme dep more) (acme undeclared more) (other pkg tools) (techne pkg absent))\n(list (path-join \"a\" \"b\") (twice (thrice-dep 2)))",
    );
    let messages: Vec<&str> = reply.split("\"message\":\"").skip(1).map(|m| m.split('"').next().unwrap()).collect();
    let _ = std::fs::remove_dir_all(&packages);
    assert_eq!(
        messages,
        ["cannot find library (acme undeclared more)", "cannot find library (other pkg tools)", "cannot find library (techne pkg absent)",],
        "{reply}"
    );
}

#[test]
fn runtime_completion_and_hover() {
    let (mut child, mut stdin, mut out) = server();
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/catalog.scm","languageId":"scheme","version":1,"text":"(buffer-named \"x\")"}}}"#,
    );
    while !recv(&mut out).contains("publishDiagnostics") {}
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///tmp/catalog.scm"},"position":{"line":0,"character":0}}}"#,
    );
    let completion: serde_json::Value = serde_json::from_str(&recv(&mut out)).unwrap();
    let labels: Vec<_> = completion["result"].as_array().unwrap().iter().filter_map(|i| i["label"].as_str()).collect();
    for name in ["car", "process-spawn", "node-connect", "make-document", "buffer-named", "define-syntax"] {
        assert!(labels.contains(&name), "no completion for {name}");
    }
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":3,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///tmp/catalog.scm"},"position":{"line":0,"character":5}}}"#,
    );
    let hover: serde_json::Value = serde_json::from_str(&recv(&mut out)).unwrap();
    let description = hover["result"]["contents"]["value"].as_str().unwrap();
    assert!(description.contains("(buffer-named name)"), "{description}");
    assert!(description.contains("Return the buffer called NAME"), "{description}");
    let source = techne_vm::library::lisp_dir().canonicalize().unwrap().join("editor/buffers.scm");
    assert!(description.contains(&source.display().to_string()), "{description}");
    let _ = child.kill();
    child.wait().unwrap();
}

/// The diagnostics messages of the next publishDiagnostics.
fn published(out: &mut impl BufRead) -> Vec<String> {
    loop {
        let msg = recv(out);
        if msg.contains("publishDiagnostics") {
            let value: serde_json::Value = serde_json::from_str(&msg).unwrap();
            return value["params"]["diagnostics"].as_array().unwrap().iter().map(|d| d["message"].as_str().unwrap().to_string()).collect();
        }
    }
}

#[test]
fn initialize_offers_the_implemented_capabilities_and_shutdown_exits() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_techne-lsp")).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    send(&mut stdin, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#);
    let init: serde_json::Value = serde_json::from_str(&recv(&mut out)).unwrap();
    let caps = &init["result"]["capabilities"];
    assert_eq!(init["result"]["serverInfo"]["name"], "techne-lsp");
    assert_eq!(caps["textDocumentSync"], 1, "full document sync: {caps}");
    for cap in ["hoverProvider", "definitionProvider", "documentSymbolProvider"] {
        assert_eq!(caps[cap], true, "{cap} is not enabled: {caps}");
    }
    assert!(caps["completionProvider"].is_object(), "no completionProvider: {caps}");
    send(&mut stdin, r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    send(&mut stdin, r#"{"jsonrpc":"2.0","id":9,"method":"shutdown","params":null}"#);
    let reply: serde_json::Value = serde_json::from_str(&recv(&mut out)).unwrap();
    assert_eq!(reply["id"], 9);
    assert!(reply["result"].is_null());
    send(&mut stdin, r#"{"jsonrpc":"2.0","method":"exit"}"#);
    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success(), "a shut-down server exits 0: {status}");
}

#[test]
fn a_change_replaces_diagnostics_a_close_clears_them_and_definitions_resolve() {
    let (mut child, mut stdin, mut out) = server();
    let uri = "file:///tmp/lifecycle.scm";
    let open = format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{uri}","languageId":"scheme","version":1,"text":{:?}}}}}}}"#,
        "(nowhere 1)"
    );
    send(&mut stdin, &open);
    assert_eq!(published(&mut out), ["unbound identifier: nowhere"]);
    // Full sync: the change's text is the whole file.
    let text = "(define (twice x) (* 2 x))\n(twice 3)";
    let change = format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didChange","params":{{"textDocument":{{"uri":"{uri}","version":2}},"contentChanges":[{{"text":{:?}}}]}}}}"#,
        text
    );
    send(&mut stdin, &change);
    assert_eq!(published(&mut out), Vec::<String>::new(), "nothing unbound any more");
    // Go to definition, from the call to the name it binds.
    let goto = format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"textDocument/definition","params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":1,"character":2}}}}}}"#
    );
    send(&mut stdin, &goto);
    let reply: serde_json::Value = serde_json::from_str(&recv(&mut out)).unwrap();
    let range = &reply["result"]["range"];
    let at = |v: &serde_json::Value| v.as_u64().unwrap();
    assert_eq!((at(&range["start"]["line"]), at(&range["start"]["character"]), at(&range["end"]["character"])), (0, 9, 14), "{reply}");
    // The outline: the one top-level definition.
    let symbols =
        format!(r#"{{"jsonrpc":"2.0","id":3,"method":"textDocument/documentSymbol","params":{{"textDocument":{{"uri":"{uri}"}}}}}}"#);
    send(&mut stdin, &symbols);
    let reply: serde_json::Value = serde_json::from_str(&recv(&mut out)).unwrap();
    let names: Vec<_> = reply["result"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["twice"], "{reply}");
    // Closing clears what was published for the file.
    let close = format!(r#"{{"jsonrpc":"2.0","method":"textDocument/didClose","params":{{"textDocument":{{"uri":"{uri}"}}}}}}"#);
    send(&mut stdin, &close);
    assert_eq!(published(&mut out), Vec::<String>::new());
    // And a closed file answers nothing.
    let hover = format!(
        r#"{{"jsonrpc":"2.0","id":4,"method":"textDocument/hover","params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":0,"character":1}}}}}}"#
    );
    send(&mut stdin, &hover);
    let reply: serde_json::Value = serde_json::from_str(&recv(&mut out)).unwrap();
    assert!(reply["result"].is_null(), "{reply}");
    let _ = child.kill();
    child.wait().unwrap();
}

#[test]
fn missing_unreadable_and_invalid_catalogs_fail_clearly() {
    let dir = std::env::temp_dir().join(format!("techne-lsp-catalog-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let catalog = dir.join("runtime-catalog.json");
    let failed = |message: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_techne-lsp")).env("TECHNE_LISP_DIR", &dir).output().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(message), "{stderr}");
        assert!(stderr.contains(&catalog.display().to_string()), "{stderr}");
        assert!(output.stdout.is_empty(), "no protocol output on startup failure");
    };
    failed("cannot read runtime catalog");
    std::fs::write(&catalog, "not JSON").unwrap();
    failed("invalid runtime catalog");
    std::fs::remove_file(&catalog).unwrap();
    std::fs::create_dir(&catalog).unwrap();
    failed("cannot read runtime catalog");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_package_resolves_locked_git_dependencies_and_overrides() {
    let root = std::env::temp_dir().join(format!("techne-lsp-lock-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let store = root.join("store");
    let app = root.join("app");
    let hash_dep = "1111111111111111111111111111111111111111111111111111111111111111";
    let hash_trans = "2222222222222222222222222222222222222222222222222222222222222222";
    let commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    // Materialized store entry for direct Git dependency (acme dep).
    let dep_dir = store.join(hash_dep);
    std::fs::create_dir_all(dep_dir.join("lib/acme/dep")).unwrap();
    std::fs::write(dep_dir.join("techne-package.scm"), "(package (acme dep) (version \"0.1.0\"))").unwrap();
    std::fs::write(
        dep_dir.join("lib/acme/dep/api.sld"),
        "(define-library (acme dep api) (export dep-fn) (import (scheme base)) (begin (define (dep-fn x) (+ x 1))))",
    )
    .unwrap();

    // Materialized store entry for transitive dep (acme trans) — NOT in app's depends.
    let trans_dir = store.join(hash_trans);
    std::fs::create_dir_all(trans_dir.join("lib/acme/trans")).unwrap();
    std::fs::write(trans_dir.join("techne-package.scm"), "(package (acme trans) (version \"0.1.0\"))").unwrap();
    std::fs::write(
        trans_dir.join("lib/acme/trans/secret.sld"),
        "(define-library (acme trans secret) (export secret-fn) (import (scheme base)) (begin (define (secret-fn) 9)))",
    )
    .unwrap();

    // Local override checkout for (acme ov).
    let ov_dir = root.join("local-ov");
    std::fs::create_dir_all(ov_dir.join("lib/acme/ov")).unwrap();
    std::fs::write(ov_dir.join("techne-package.scm"), "(package (acme ov) (version \"0.1.0\"))").unwrap();
    std::fs::write(
        ov_dir.join("lib/acme/ov/core.sld"),
        "(define-library (acme ov core) (export ov-fn) (import (scheme base)) (begin (define (ov-fn) 7)))",
    )
    .unwrap();

    // Root package declaring Git deps + local path override + techne.lock.
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(
        app.join("techne-package.scm"),
        "(package (acme app)
           (version \"0.1.0\")
           (depends
             ((acme dep) (github \"acme/dep\" (tag \"v1\")))
             ((acme ov) (github \"acme/ov\" (tag \"v1\"))))
           (override
             ((acme ov) (path \"../local-ov\"))))",
    )
    .unwrap();
    std::fs::write(
        app.join("techne.lock"),
        format!(
            "(lock
               (techne \"0.4\")
               (package (acme dep) \"0.1.0\"
                 (source (github \"acme/dep\" (commit \"{commit}\")))
                 (tree-sha256 \"{hash_dep}\")
                 (depends (acme trans)))
               (package (acme ov) \"0.1.0\"
                 (source (path \"../local-ov\"))
                 (tree-sha256 \"{hash_dep}\")
                 (depends))
               (package (acme trans) \"0.1.0\"
                 (source (github \"acme/trans\" (commit \"{commit}\")))
                 (tree-sha256 \"{hash_trans}\")
                 (depends)))"
        ),
    )
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_techne-lsp"))
        .env("TECHNE_PACKAGE_STORE", &store)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    send(&mut stdin, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#);
    recv(&mut out);
    send(&mut stdin, r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    let main = app.join("main.scm");
    let text = "(import (scheme base) (acme dep api) (acme ov core) (acme trans secret))\n(list (dep-fn 1) (ov-fn))";
    let open = format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"file://{}","languageId":"scheme","version":1,"text":{:?}}}}}}}"#,
        main.display(),
        text
    );
    send(&mut stdin, &open);
    let messages = published(&mut out);
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(messages, ["cannot find library (acme trans secret)"], "expected only undeclared transitive dep missing");
}
