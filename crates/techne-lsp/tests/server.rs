//! The server over stdio: diagnostics for a file using R7RS libraries,
//! included files and the runtime's procedures.

use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
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

/// The diagnostics' messages for `text` opened as `path`.
fn diagnostics(path: &std::path::Path, text: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_techne-lsp")).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    send(&mut stdin, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#);
    recv(&mut out);
    send(&mut stdin, r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
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
(define-library (here) (export f) (import (scheme base)) (begin (define (f) 1)))
(import (rename (here) (f g)))
(list (t:twice t:shown) (g) (process-spawn "true" '()) (make-document "x") nowhere)"#;
    let reply = diagnostics(&main, text);
    let messages: Vec<&str> = reply.split("\"message\":\"").skip(1).map(|m| m.split('"').next().unwrap()).collect();
    assert_eq!(messages, ["cannot find library (no such)", "unbound identifier: nowhere"], "{reply}");
}
