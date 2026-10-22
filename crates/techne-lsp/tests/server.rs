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
(list (t:twice t:shown) (g) (process-spawn "true" '()) (make-document "x") nowhere)
(define-command (hello s n) "Hello." (message! s "hi"))"#;
    let reply = diagnostics(&main, text);
    let messages: Vec<&str> = reply.split("\"message\":\"").skip(1).map(|m| m.split('"').next().unwrap()).collect();
    assert_eq!(messages, ["cannot find library (no such)", "unbound identifier: nowhere"], "{reply}");
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
