//! The terminal frontend headless: the runtime driven synchronously, frames
//! drawn into ratatui's test backend.

use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
};
use techne_editor::{
    present::{CursorShape, Input},
    runtime::Runtime,
    scenario::{self, Step},
};
use techne_terminal::term::Term;
use techne_text::Document;

struct Headless {
    rt: Runtime,
    term: Term,
    t: Terminal<TestBackend>,
}

impl Headless {
    fn new(text: &str, profile: &str, cols: u16, rows: u16) -> Headless {
        let rt = Runtime::with_document(Document::new(text), profile).unwrap();
        let mut h = Headless { rt, term: Term::new(cols, rows), t: Terminal::new(TestBackend::new(cols, rows)).unwrap() };
        h.settle();
        h
    }

    /// Show the runtime's state, following any scrolling the frontend asks.
    fn settle(&mut self) {
        loop {
            let more = self.term.show(self.rt.snapshot());
            self.t.draw(|f| self.term.draw(f)).unwrap();
            if more.is_empty() {
                return;
            }
            for i in more {
                self.rt.handle(i);
            }
        }
    }

    fn event(&mut self, ev: Event) {
        for i in self.term.event(&ev) {
            self.rt.handle(i);
        }
        if let Event::Resize(c, r) = ev {
            self.t.backend_mut().resize(c, r);
        }
        self.settle();
    }

    /// A key as a legacy terminal sends it.
    fn key(&mut self, key: &str) {
        self.event(Event::Key(terminal_key(key)));
    }

    fn text(&mut self, text: &str) {
        for c in text.chars() {
            self.event(Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
        }
    }

    fn click(&mut self, col: u16, row: u16, shift: bool) {
        let modifiers = if shift { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
        self.event(Event::Mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: col, row, modifiers }));
    }

    fn screen(&self) -> Vec<String> {
        let buf = self.t.backend().buffer();
        // The cell after a wide character is part of it.
        let row = |y| {
            let mut s = String::new();
            let mut x = 0;
            while x < buf.area.width {
                let sym = buf[(x, y)].symbol();
                s.push_str(sym);
                x += unicode_width::UnicodeWidthStr::width(sym).max(1) as u16;
            }
            s.trim_end().to_string()
        };
        (0..buf.area.height).map(row).collect()
    }

    fn cursor(&mut self) -> (u16, u16) {
        let p = self.t.get_cursor_position().unwrap();
        (p.x, p.y)
    }

    fn state(&mut self) -> scenario::State {
        scenario::state(&mut self.rt)
    }
}

/// The event a terminal without the kitty protocol sends for a key.
fn terminal_key(key: &str) -> KeyEvent {
    let mut mods = KeyModifiers::NONE;
    let mut rest = key;
    loop {
        if let Some(r) = rest.strip_prefix("C-") {
            mods |= KeyModifiers::CONTROL;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("M-").filter(|r| !r.is_empty()) {
            mods |= KeyModifiers::ALT;
            rest = r;
        } else {
            break;
        }
    }
    let code = match rest {
        "RET" => KeyCode::Enter,
        "DEL" => KeyCode::Backspace,
        "ESC" => KeyCode::Esc,
        "SPC" => KeyCode::Char(' '),
        // C-/ is the byte 0x1f, which crossterm reads as Control-7.
        "/" if mods.contains(KeyModifiers::CONTROL) => KeyCode::Char('7'),
        r => KeyCode::Char(r.chars().next().unwrap()),
    };
    KeyEvent::new(code, mods)
}

#[test]
fn the_scripted_session_ends_where_the_runtime_alone_does() {
    let mut h = Headless::new(scenario::TEXT, "emacs", 60, 16);
    for step in scenario::STEPS {
        match *step {
            Step::Key(k) => h.key(k),
            Step::Text(t) => h.text(t),
            Step::Click { on, offset, extend } => {
                let text = h.term.snapshot().unwrap().text.to_string();
                let pos = scenario::click_target(&text, on, offset);
                let (col, row) = h.term.cell_of(pos).unwrap_or_else(|| panic!("{on:?} is on screen"));
                h.click(col, row, extend);
            }
            Step::Resize { width, height } => h.event(Event::Resize(width as u16, height as u16)),
        }
    }
    assert_eq!(h.state(), scenario::expected("emacs"));
}

#[test]
fn wide_characters_tabs_and_continued_rows() {
    let mut h = Headless::new("ab日本語cd\n\tx\u{1}y", "emacs", 8, 5);
    // 7 columns of text, then the continuation mark.
    assert_eq!(h.screen(), ["ab日本 \\", "語cd", "       \\", "x^Ay", "*scratch"]);
    // Both cells of a wide character are that character.
    h.click(4, 0, false);
    assert_eq!(h.rt.snapshot().head(), 5);
    h.click(5, 0, false);
    assert_eq!(h.rt.snapshot().head(), 5);
    assert_eq!(h.cursor(), (4, 0));
    h.key("C-e");
    assert_eq!(h.cursor(), (4, 1), "after the last glyph of the wrapped line");
}

#[test]
fn the_caret_stays_on_screen() {
    let text: String = (0..50).map(|i| format!("line {i}\n")).collect();
    let mut h = Headless::new(&text, "emacs", 20, 6);
    for _ in 0..12 {
        h.key("C-n");
    }
    // Five text rows; line 12 is the last of them.
    assert_eq!(h.screen()[4], "line 12");
    assert_eq!(h.cursor(), (0, 4));
    h.key("M-<");
    assert_eq!(h.screen()[0], "line 0");
}

#[test]
fn selections_and_the_modal_cursor() {
    let mut h = Headless::new("one two\nthree", "emacs", 20, 4);
    h.key("C-SPC");
    h.key("C-e");
    h.key("C-f");
    let buf = h.t.backend().buffer().clone();
    let selected = |x: u16, y: u16| buf[(x, y)].bg == ratatui::style::Color::DarkGray;
    assert!(selected(0, 0) && selected(6, 0), "the line is selected");
    assert!(selected(7, 0), "and its line break");
    assert!(!selected(0, 1));

    let mut m = Headless::new("abc", "modal", 20, 4);
    assert_eq!(m.term.cursor_shape(), CursorShape::Block);
    m.key("A");
    assert_eq!(m.term.cursor_shape(), CursorShape::Bar);
    assert_eq!(m.cursor(), (3, 0));
}

#[test]
fn undo_through_the_terminal_control_character() {
    let mut h = Headless::new("", "emacs", 20, 4);
    h.text("abc");
    h.key("C-/");
    assert_eq!(h.rt.snapshot().text.to_string(), "");
    // A scroll the frontend sends is in its snapshot's revision.
    let s = h.rt.snapshot();
    h.rt.handle(Input::Scroll { revision: s.revision, anchor: 0 });
}
