//! `techne-term`: the editor in the terminal it is started in.
//!
//! The runtime runs on its own thread as for the window; this one owns the
//! terminal and a `Term`. Bytes from the terminal, the runtime's outputs and
//! size changes arrive on one channel; after each batch the screen is
//! brought up to date. An escape sequence cut short waits `ESC_WAIT` for
//! the rest, so a lone ESC is the Escape key.

use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::mpsc,
    time::Duration,
};

use techne_editor::{
    present::{Input, Output},
    runtime::{Runtime, journal_for},
};
use techne_term::{RESTORE, SETUP, Term};

const USAGE: &str = "usage: techne-term [--modal] FILE";

const ESC_WAIT: Duration = Duration::from_millis(25);

enum Msg {
    Bytes(Vec<u8>),
    /// No more bytes came for an incomplete sequence.
    Pause,
    Output(Output),
    Resize,
    /// The terminal closed.
    Hangup,
    /// The runtime could not start.
    Failed(String),
}

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
    let (in_tx, in_rx) = mpsc::channel::<Input>();
    // The VM is not Send: the runtime is made on its own thread.
    let runtime = {
        let tx = tx.clone();
        std::thread::spawn(move || match Runtime::open(&path, &journal, &profile) {
            Ok((rt, _)) => rt.serve(in_rx, |o| {
                let _ = tx.send(Msg::Output(o));
            }),
            Err(e) => {
                let _ = tx.send(Msg::Failed(e));
            }
        })
    };

    if let Err(e) = crossterm::terminal::enable_raw_mode() {
        eprintln!("techne-term: {e}");
        std::process::exit(1)
    }
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        default_hook(info)
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
                Msg::Output(Output::Quit) => break 'run,
                Msg::Output(o) => term.output(o).into_iter().collect(),
                Msg::Resize => {
                    let (cols, rows) = size();
                    term.resize(cols, rows);
                    vec![]
                }
                Msg::Hangup => {
                    let _ = in_tx.send(Input::Close);
                    break 'run;
                }
                Msg::Failed(e) => {
                    failed = Some(e);
                    break 'run;
                }
            };
            for input in inputs {
                let _ = in_tx.send(input);
            }
        }
        let _ = out.write_all(term.paint().as_bytes());
        let _ = out.flush();
    }
    restore();
    drop(in_tx);
    let _ = runtime.join();
    if let Some(e) = failed {
        eprintln!("techne-term: {e}");
        std::process::exit(1);
    }
}
