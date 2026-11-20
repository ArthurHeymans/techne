//! The browser's socket boundary. It may attach, not dispatch arbitrary
//! editor-server requests. The runtime still owns all editing and state.

use std::{path::PathBuf, process::ExitCode, sync::Arc, time::Duration};

use axum::{
    Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};
use techne_editor::{
    attach::Remote,
    host::Event,
    present::{Input, Output, Snapshot},
    server::{self, OpenError},
};
use tokio::sync::{Notify, Semaphore, mpsc};

const PROTOCOL: &str = "techne-v3";
const MAX_INPUT: usize = 256 << 10;
const MAX_TEXT: usize = 2 << 20;
const MAX_OUTPUT: usize = 4 << 20;
const MAX_ROWS: usize = 4_000;
const MAX_PARTS: usize = 16_000;
const MAX_PANES: usize = 32;

#[derive(Debug, Clone, Copy)]
struct Budget;

impl std::fmt::Display for Budget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("This presentation exceeds the browser's size limit. Open a smaller buffer or use a native frontend.")
    }
}
const USAGE: &str = "usage: techne web [FILE]\n\nAttach a browser to a running editor (start one with techne daemon).\nOpen the printed private URL. The gateway listens only on IPv4 loopback.";

struct Gateway {
    authority: String,
    origin: String,
    token: String,
    socket: PathBuf,
    file: Option<PathBuf>,
    clients: Arc<Semaphore>,
}

pub fn main(args: Vec<String>) -> ExitCode {
    let file = match args.as_slice() {
        [] => None,
        [arg] if matches!(arg.as_str(), "-h" | "--help") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        [arg] if !arg.starts_with('-') => Some(PathBuf::from(arg)),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
        .and_then(|rt| rt.block_on(serve(file)));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("techne web: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(file: Option<PathBuf>) -> Result<(), String> {
    let socket = server::socket().ok_or("no private editor socket (set XDG_RUNTIME_DIR)")?;
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.map_err(|e| e.to_string())?;
    let authority = listener.local_addr().map_err(|e| e.to_string())?.to_string();
    let origin = format!("http://{authority}");
    let mut secret = [0u8; 32];
    getrandom::fill(&mut secret).map_err(|e| e.to_string())?;
    let token: String = secret.iter().map(|b| format!("{b:02x}")).collect();
    // Fragment stays out of HTTP requests and referrers. The JS removes it
    // after saving it to session storage (so reload works in the same tab).
    println!("{origin}/#{token}");
    let state = Arc::new(Gateway { authority, origin, token, socket, file, clients: Arc::new(Semaphore::new(8)) });
    let app = Router::new()
        .route("/", get(page))
        .route("/app.js", get(script))
        .route("/style.css", get(style))
        .route("/attach", get(upgrade))
        .with_state(state);
    axum::serve(listener, app).await.map_err(|e| e.to_string())
}

fn single_header(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    let all = headers.get_all(name);
    let mut values = all.iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}

fn asset(g: &Gateway, headers: &HeaderMap, kind: &'static str, body: &'static str) -> Response {
    if single_header(headers, header::HOST) != Some(g.authority.as_str()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    ([
        (header::CONTENT_TYPE, kind),
        (header::CACHE_CONTROL, "no-store"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::CONTENT_SECURITY_POLICY, "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"),
    ], body).into_response()
}

async fn page(State(g): State<Arc<Gateway>>, h: HeaderMap) -> Response {
    asset(&g, &h, "text/html; charset=utf-8", include_str!("web/index.html"))
}
async fn script(State(g): State<Arc<Gateway>>, h: HeaderMap) -> Response {
    asset(&g, &h, "text/javascript; charset=utf-8", include_str!("web/app.js"))
}
async fn style(State(g): State<Arc<Gateway>>, h: HeaderMap) -> Response {
    asset(&g, &h, "text/css; charset=utf-8", include_str!("web/style.css"))
}

fn authorized(g: &Gateway, headers: &HeaderMap) -> bool {
    single_header(headers, header::HOST) == Some(g.authority.as_str())
        && single_header(headers, header::ORIGIN) == Some(g.origin.as_str())
        && single_header(headers, header::SEC_WEBSOCKET_PROTOCOL).is_some_and(|p| {
            let mut parts = p.split(',').map(str::trim);
            parts.next() == Some(PROTOCOL) && parts.next() == Some(g.token.as_str()) && parts.next().is_none()
        })
}

async fn upgrade(State(g): State<Arc<Gateway>>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    if !authorized(&g, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(permit) = g.clients.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    ws.protocols([PROTOCOL]).max_message_size(MAX_INPUT).max_frame_size(MAX_INPUT).on_upgrade(move |socket| async move {
        let _permit = permit;
        session(socket, &g).await;
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserFrame {
    generation: u64,
    input: Browser,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
enum Browser {
    Key { key: String },
    Click { view: u64, revision: u64, pos: usize, extend: bool },
    Scroll { view: u64, revision: u64, anchor: usize, caret: Option<usize> },
    Edge { view: u64, end: bool },
    Text { text: String },
    Viewport { columns: usize, rows: usize, split_gap: usize },
    Focus { focused: bool },
}

impl Browser {
    fn apply(self, remote: &mut Remote, generation: u64) {
        let at = std::time::Instant::now();
        let input = match self {
            Self::Key { key } if !key.is_empty() && key.len() <= 64 => Input::Key { key, at },
            Self::Click { view, revision, pos, extend } => Input::Click { view, revision, pos, extend, at },
            Self::Scroll { view, revision, anchor, caret } => Input::Scroll { view, revision, anchor, caret },
            Self::Edge { view, end } => Input::Edge { view, end },
            Self::Text { text } if !text.is_empty() => Input::Text { text, at },
            Self::Viewport { columns, rows, split_gap } => {
                Input::Viewport { columns: columns.clamp(1, 4096), rows: rows.clamp(2, 4096), split_gap: split_gap.clamp(1, 64) }
            }
            Self::Focus { focused } => Input::Focus { focused },
            _ => return,
        };
        remote.send_in_generation(input, generation);
    }
}

fn snapshot(s: &Snapshot, generation: u64) -> Result<Value, Budget> {
    use techne_editor::present::Row;

    let count_parts =
        |rows: &[Row]| -> usize { rows.iter().map(|r| 1 + r.columns.len() + r.columns.iter().map(Vec::len).sum::<usize>()).sum() };
    let mut parts = s.styles.len()
        + s.key_hints.len() * 4
        + s.minibuffer.as_ref().map_or(0, |m| count_parts(&m.rows))
        + s.completion.as_ref().map_or(0, |c| count_parts(&c.rows));
    if s.panes.len() > MAX_PANES || parts > MAX_PARTS {
        return Err(Budget);
    }
    let mut bytes = 0;
    let mut rows = 0;
    let mut panes = Vec::new();
    for p in &s.panes {
        bytes += p.text.len_bytes();
        let lines = p.text.len_lines();
        rows += lines;
        // Bound both produced nodes and the selection checks per row.
        parts += p.layers.len() * 2 + p.inlays.len() * 2 + p.blocks.len() + p.hangs.len() + lines.saturating_mul(p.selections.len());
        if bytes > MAX_TEXT || rows > MAX_ROWS || parts > MAX_PARTS {
            return Err(Budget);
        }
        let mut pane = serde_json::to_value(p).map_err(|_| Budget)?;
        pane["text"] = p.text.to_string().into();
        panes.push(pane);
    }
    Ok(json!({"type": "snapshot", "generation": generation, "id": s.id, "panes": panes, "focus": s.focus, "echo": s.echo,
        "minibuffer": s.minibuffer, "completion": s.completion, "hints": s.key_hints,
        "styles": s.styles, "chrome": s.chrome}))
}

/// Stop encoding once the wire budget is exhausted, including JSON escaping.
fn encode(value: &Value) -> Result<String, Budget> {
    struct Limited(Vec<u8>);
    impl std::io::Write for Limited {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_OUTPUT - self.0.len() {
                return Err(std::io::Error::other("outbound browser frame exceeds its budget"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Limited(Vec::new());
    serde_json::to_writer(&mut output, value).map_err(|_| Budget)?;
    String::from_utf8(output.0).map_err(|_| Budget)
}

async fn send(socket: &mut WebSocket, value: Value) -> bool {
    let encoded = encode(&value);
    let fits = encoded.is_ok();
    let text = encoded.unwrap_or_else(|e| json!({"type": "error", "message": e.to_string()}).to_string());
    let sent = tokio::time::timeout(Duration::from_secs(3), socket.send(Message::Text(text.into()))).await;
    fits && matches!(sent, Ok(Ok(())))
}

async fn session(mut socket: WebSocket, g: &Gateway) {
    let (tx, mut rx) = mpsc::channel(16);
    let overflow = Arc::new(Notify::new());
    let notify = overflow.clone();
    let mut remote = match Remote::attach(&g.socket, "emacs", g.file.as_deref(), move |e| {
        if tx.try_send(e).is_err() {
            notify.notify_one();
        }
    }) {
        Ok(remote) => remote,
        Err(error) => {
            let why = match error {
                OpenError::NotRunning => "No editor is running. Start techne daemon, then reconnect.".into(),
                OpenError::Failed(e) => e,
            };
            send(&mut socket, json!({"type": "error", "message": why})).await;
            return;
        }
    };
    let mut generation = 0;
    let mut ready = false;
    loop {
        tokio::select! {
            _ = overflow.notified() => {
                send(&mut socket, json!({"type": "error", "message": "Browser fell behind. Reconnect to continue."})).await;
                break;
            }
            event = rx.recv() => {
                let (value, done) = match event {
                    Some(Event::Output(Output::Snapshot(s))) => match snapshot(&s, generation) {
                        Ok(value) => { ready = true; (value, false) },
                        Err(e) => (json!({"type": "error", "message": e.to_string()}), true),
                    },
                    Some(Event::Output(Output::Capture { id, .. })) => {
                        remote.captured(id, Err("the web frontend does not support editor captures".into()));
                        continue;
                    }
                    Some(Event::Output(Output::Clipboard(text))) => (json!({"type": "clipboard", "text": text}), false),
                    Some(Event::Output(Output::Quit)) => (json!({"type": "quit"}), true),
                    Some(Event::Failed(e) | Event::Output(Output::Refused(e))) => (json!({"type": "error", "message": e}), true),
                    Some(Event::Ended) => {
                        generation += 1;
                        ready = false;
                        (json!({"type": "restart"}), false)
                    },
                    Some(_) => continue,
                    None => break,
                };
                if !send(&mut socket, value).await || done { break; }
            }
            message = socket.recv() => match message {
                Some(Ok(Message::Text(text))) => match serde_json::from_str::<BrowserFrame>(text.as_str()) {
                    Ok(frame) if ready && frame.generation == generation => frame.input.apply(&mut remote, generation),
                    Ok(_) => {},
                    Err(_) => {
                        send(&mut socket, json!({"type": "error", "message": "Invalid frontend message."})).await;
                        break;
                    }
                },
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {},
                _ => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_budget_counts_json_escaping() {
        let text = "\u{0001}".repeat(MAX_OUTPUT / 5);
        assert!(text.len() < MAX_TEXT);
        assert!(encode(&json!({"type": "clipboard", "text": text})).is_err());
    }
}
