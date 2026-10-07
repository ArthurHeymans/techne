//! `techne-term`: the editor in the terminal it is started in.
//!
//! The runtime runs on its own thread as for the window (`host`); this one
//! owns the terminal and a `Term`. Bytes from the terminal, the runtime's
//! outputs and size changes arrive on one channel; after each batch the
//! screen is brought up to date. An escape sequence cut short waits
//! `ESC_WAIT` for the rest, so a lone ESC is the Escape key. When the
//! runtime thread ends without the session quitting (it panicked), a new
//! runtime takes over the terminal with the unsaved edits from the journal;
//! the panic is reported after leaving the terminal.

use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::{Mutex, mpsc},
    time::Duration,
};

use techne_editor::{
    host::{Event, Host},
    present::Output,
    runtime::journal_for,
};
use techne_term::{RESTORE, SETUP, Term};

const USAGE: &str = "usage: techne-term [--modal] FILE";

const ESC_WAIT: Duration = Duration::from_millis(25);

enum Msg {
    Bytes(Vec<u8>),
    /// No more bytes came for an incomplete sequence.
    Pause,
    Runtime(Event),
    Resize,
    /// The terminal closed.
    Hangup,
}

/// Panics of other threads than the terminal's (the runtime's), reported
/// when leaving the terminal.
static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn args() -> Result<(PathBuf, String), String> {
    let (mut path, mut profile) = (None, "emacs");
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--modal" => profile = "modal",
            _ if path.is_none() && !arg.starts_with('-') => path = Some(PathBuf::from(arg)),
            _ => return Err(USAGE.into()),
        }
    }
    Ok((path.ok_or(USAGE)?, profile.into()))
}

fn size() -> (usize, usize) {
    crossterm::terminal::size().map_or((80, 24), |(c, r)| (c as usize, r as usize))
}

fn restore() {
    let mut out = std::io::stdout();
    let _ = out.write_all(RESTORE.as_bytes());
    let _ = out.flush();
    let _ = crossterm::terminal::disable_raw_mode();
}

fn main() {
    let (path, profile) = args().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2)
    });
    let journal = journal_for(&path).unwrap_or_else(|e| {
        eprintln!("techne-term: journal: {e}");
        std::process::exit(1)
    });
    let (tx, rx) = mpsc::channel();
    let mut host = {
        let tx = tx.clone();
        Host::start(
            path,
            journal,
            profile,
            |_| {},
            move |e| {
                let _ = tx.send(Msg::Runtime(e));
            },
        )
    };

    if let Err(e) = crossterm::terminal::enable_raw_mode() {
        eprintln!("techne-term: {e}");
        std::process::exit(1)
    }
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() == Some("main") {
            restore();
            default_hook(info)
        } else {
            PANICS.lock().unwrap_or_else(|e| e.into_inner()).push(info.to_string());
        }
    }));
    let mut out = std::io::stdout();
    let _ = out.write_all(SETUP.as_bytes());
    let _ = out.flush();

    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut buf = [0; 4096];
            loop {
                match stdin.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        if tx.send(Msg::Bytes(buf[..n].to_vec())).is_err() {
                            return;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    _ => {
                        let _ = tx.send(Msg::Hangup);
                        return;
                    }
                }
            }
        });
    }
    match signal_hook::iterator::Signals::new([signal_hook::consts::SIGWINCH]) {
        Ok(mut signals) => {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for _ in signals.forever() {
                    if tx.send(Msg::Resize).is_err() {
                        return;
                    }
                }
            });
        }
        Err(e) => eprintln!("techne-term: no resizing: {e}"),
    }
    drop(tx);

    let (cols, rows) = size();
    let mut term = Term::new(cols, rows);
    let mut failed = None;
    'run: loop {
        let first = if term.waiting() {
            match rx.recv_timeout(ESC_WAIT) {
                Ok(m) => m,
                Err(mpsc::RecvTimeoutError::Timeout) => Msg::Pause,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(m) => m,
                Err(_) => break,
            }
        };
        for msg in std::iter::once(first).chain(rx.try_iter()) {
            let inputs = match msg {
                Msg::Bytes(b) => term.feed(&b),
                Msg::Pause => term.flush(),
                Msg::Runtime(Event::Output(Output::Quit)) => break 'run,
                Msg::Runtime(Event::Output(o)) => term.output(o),
                Msg::Runtime(Event::Failed(e)) => {
                    failed = Some(e);
                    break 'run;
                }
                Msg::Runtime(Event::Ended) => {
                    if !host.restart() {
                        failed = Some("the runtime stopped".into());
                        break 'run;
                    }
                    term.restarted();
                    vec![]
                }
                Msg::Resize => {
                    let (cols, rows) = size();
                    term.resize(cols, rows);
                    vec![]
                }
                Msg::Hangup => break 'run,
            };
            for input in inputs {
                host.send(input);
            }
        }
        let _ = out.write_all(term.paint().as_bytes());
        let _ = out.flush();
    }
    restore();
    host.close();
    for p in PANICS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        eprintln!("techne-term: the runtime crashed: {p}");
    }
    if let Some(e) = failed {
        eprintln!("techne-term: {e}");
        std::process::exit(1);
    }
}
