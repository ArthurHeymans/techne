//! `techne-term`: the editor in a terminal.
//!
//! The runtime runs on its own thread, terminal events are read on another,
//! and this one draws. With the kitty keyboard protocol every key arrives
//! as itself; without it, the bound keys the terminal cannot send are named
//! in the status line at the start.

use std::{
    io::{self, Write},
    path::PathBuf,
    sync::mpsc,
};

use ratatui::{
    DefaultTerminal,
    crossterm::{
        cursor::SetCursorStyle,
        event::{
            self, DisableMouseCapture, EnableMouseCapture, Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
            PushKeyboardEnhancementFlags,
        },
        execute,
        terminal::supports_keyboard_enhancement,
    },
};
use techne_editor::{
    present::{CursorShape, Input, Output},
    runtime::{self, Runtime},
};
use techne_terminal::{keys, term::Term};

const USAGE: &str = "usage: techne-term [--modal] FILE";

enum Msg {
    Term(Event),
    Runtime(Output),
}

fn main() {
    let mut profile = "emacs".to_string();
    let mut path = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--modal" => profile = "modal".into(),
            a if !a.starts_with("--") && path.is_none() => path = Some(PathBuf::from(a)),
            _ => {
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    }
    let Some(path) = path else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let journal = runtime::journal_for(&path).unwrap_or_else(|e| {
        eprintln!("techne-term: journal: {e}");
        std::process::exit(1)
    });

    let mut terminal = ratatui::init();
    let kitty = supports_keyboard_enhancement().unwrap_or(false);
    let mut out = io::stdout();
    let _ = execute!(out, EnableMouseCapture);
    if kitty {
        let _ = execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
    }

    let (tx, rx) = mpsc::channel();
    let (in_tx, in_rx) = mpsc::channel();
    let events = tx.clone();
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if events.send(Msg::Term(ev)).is_err() {
                return;
            }
        }
    });
    let runtime = std::thread::spawn(move || {
        let (mut rt, _) = match Runtime::open(&path, &journal, &profile) {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(Msg::Runtime(Output::Quit));
                return Err(e);
            }
        };
        if !kitty {
            let lost = keys::unsendable(&rt.bound_keys().unwrap_or_default());
            if !lost.is_empty() {
                let _ = rt.message(&format!("This terminal cannot send: {}", lost.join(", ")));
            }
        }
        runtime::serve(rt, in_rx, |o| {
            let _ = tx.send(Msg::Runtime(o));
        });
        Ok(())
    });

    let size = terminal.size().unwrap_or_default();
    let mut term = Term::new(size.width, size.height);
    run(&mut terminal, &mut term, &rx, &in_tx);

    if kitty {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(out, DisableMouseCapture, SetCursorStyle::DefaultUserShape);
    ratatui::restore();
    drop(in_tx);
    if let Ok(Err(e)) = runtime.join() {
        eprintln!("techne-term: {e}");
        std::process::exit(1);
    }
}

fn run(terminal: &mut DefaultTerminal, term: &mut Term, rx: &mpsc::Receiver<Msg>, inputs: &mpsc::Sender<Input>) {
    let mut shape = None;
    while let Ok(first) = rx.recv() {
        // Handle everything pending, then draw once.
        for msg in std::iter::once(first).chain(rx.try_iter()) {
            let sent = match msg {
                Msg::Term(ev) => term.event(&ev),
                Msg::Runtime(Output::Snapshot(s)) => term.show(*s),
                Msg::Runtime(Output::Quit) => return,
            };
            for i in sent {
                let _ = inputs.send(i);
            }
        }
        let _ = terminal.draw(|f| term.draw(f));
        let want = term.cursor_shape();
        if shape != Some(want) {
            let style = match want {
                CursorShape::Block => SetCursorStyle::SteadyBlock,
                CursorShape::Bar => SetCursorStyle::SteadyBar,
            };
            let _ = execute!(io::stdout(), style);
            let _ = io::stdout().flush();
            shape = Some(want);
        }
    }
}
