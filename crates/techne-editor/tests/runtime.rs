//! The runtime over the presentation protocol: keys through Lisp, clicks and
//! scrolling resolved against older snapshots, quitting and saving.

use std::time::Instant;

use techne_editor::{
    present::{CursorShape, Input, Output, Snapshot},
    runtime::Runtime,
};
use techne_text::Document;

fn runtime(text: &str, profile: &str) -> Runtime {
    Runtime::with_document(Document::new(text), profile).unwrap()
}

fn keys(rt: &mut Runtime, keys: &str) -> Option<Output> {
    keys.split(' ').filter_map(|k| rt.handle(Input::Key { key: k.to_string(), at: Instant::now() })).last()
}

fn click(rt: &mut Runtime, revision: u64, pos: usize) {
    let view = rt.snapshot().pane().view;
    assert!(rt.handle(Input::Click { view, revision, pos, extend: false, at: Instant::now() }).is_none());
}

#[test]
fn snapshot_positions_follow_edits_made_by_a_layer() {
    let mut rt = runtime("hello", "emacs");
    keys(&mut rt, "C-e");
    rt.eval(
        "(define-minor-mode deleting-layer \"Delete while presenting.\"
           #:layer (lambda (d from to)
             (view-edit! (make-view d \"layer\") (list (list 0 (document-length d) \"\")) \"new\")
             '()))
         (set-option! 'deleting-layer #t)",
    )
    .unwrap();
    let s = rt.snapshot();
    let p = s.pane();
    assert_eq!(p.text.len_bytes(), 0);
    assert_eq!(p.head(), 0);
    assert_eq!(p.scroll, 0);
    assert!(p.layers.is_empty());
    assert_eq!(p.revision.to_string(), rt.eval("(document-revision (session-document (current-session)))").unwrap());
}

#[test]
fn layers_are_retried_when_the_text_moves_and_bounded_when_it_keeps_moving() {
    let mut rt = runtime("hello", "emacs");
    rt.eval(
        "(define layer-calls 0)
         (define keep-editing #f)
         (define-minor-mode changing-layer \"Edit while presenting.\"
           #:layer (lambda (d from to)
             (set! layer-calls (+ layer-calls 1))
             (when (or (= layer-calls 1) keep-editing)
               (view-edit! (make-view d \"layer\") '((0 0 \"x\")) \"new\"))
             '((0 1 keyword))))
         (set-option! 'changing-layer #t)",
    )
    .unwrap();
    let s = rt.snapshot();
    assert_eq!(s.pane().text.to_string(), "xhello");
    assert!(
        s.pane().layers.iter().any(|h| h.from == 0 && h.to == 1 && s.style(h.style).fg.map(|c| c.rgb) == Some([0xcb, 0xa6, 0xf7])),
        "keyword's colour"
    );
    assert_eq!(rt.eval("layer-calls").unwrap(), "2");
    rt.eval("(set! layer-calls 0) (set! keep-editing #t)").unwrap();
    let s = rt.snapshot();
    assert_eq!(rt.eval("layer-calls").unwrap(), "3");
    assert!(s.pane().layers.is_empty(), "unstable results must be dropped");
    assert!(s.pane().selections.iter().all(|&(a, h)| a <= s.pane().text.len_bytes() && h <= s.pane().text.len_bytes()));
    assert!(rt.take_errors().iter().any(|e| e.contains("changed under it")));
}

#[test]
fn stalled_layers_keep_callback_results_but_refresh_presentation_faces() {
    let mut rt = runtime("", "emacs");
    rt.eval(
        "(define p (make-presentation))
         (presentation-set-rows! p (list (list \"row\" (list \"abc\" 'comment))))
         (show-document! (current-session) p)
         (define stall-layer #f)
         (define-minor-mode face-layer \"A layer that may stall.\"
           #:layer (lambda (d from to)
             (when stall-layer (let spin () (spin)))
             '((0 1 keyword))))
         (set-option! 'face-layer #t)",
    )
    .unwrap();
    let before = rt.snapshot();
    let has = |s: &Snapshot, rgb: [u8; 3]| s.pane().layers.iter().any(|h| s.style(h.style).fg.map(|c| c.rgb) == Some(rgb));
    assert!(has(&before, [0x7f, 0x84, 0x9c]), "comment's colour");
    assert!(has(&before, [0xcb, 0xa6, 0xf7]), "keyword's colour");
    rt.eval("(presentation-set-rows! p (list (list \"row\" (list \"abc\" 'string)))) (set! stall-layer #t)").unwrap();
    rt.set_part_deadline(std::time::Duration::from_millis(250));
    let after = rt.snapshot();
    assert_eq!(before.pane().revision, after.pane().revision);
    assert!(has(&after, [0xcb, 0xa6, 0xf7]));
    assert!(has(&after, [0xa6, 0xe3, 0xa1]), "string's colour");
    assert!(!has(&after, [0x7f, 0x84, 0x9c]));
}

/// Faces end to end: a layer's names are composed into flat styles,
/// dotted names fall back to their prefix, unknown ones are said once,
/// and setting a face in Lisp restyles the next snapshot.
#[test]
fn faces_are_composed_and_follow_lisp() {
    let mut rt = runtime("hello", "emacs");
    rt.eval(
        "(define-minor-mode two-faces \"Overlap two faces.\"
           #:layer (lambda (d from to) '((0 5 comment.doc) (2 3 keyword) (0 2 sparkle))))
         (set-option! 'two-faces #t)",
    )
    .unwrap();
    let s = rt.snapshot();
    let spans: Vec<(usize, usize, Option<[u8; 3]>)> =
        s.pane().layers.iter().map(|h| (h.from, h.to, s.style(h.style).fg.map(|c| c.rgb))).collect();
    assert_eq!(
        spans,
        [(0, 2, Some([0x7f, 0x84, 0x9c])), (2, 3, Some([0xcb, 0xa6, 0xf7])), (3, 5, Some([0x7f, 0x84, 0x9c]))],
        "comment (by its dotted name), keyword patched over its middle, sparkle nowhere"
    );
    assert!(rt.take_errors().iter().filter(|e| e.contains("unknown face: sparkle")).count() == 1);
    let middle = s.style(s.pane().layers[1].style);
    assert_eq!(middle.slant, Some(techne_editor::present::Slant::Italic), "the comment's slant patches under the keyword's colour");
    // A setting in Lisp: the next snapshot's keyword takes the new colour.
    rt.eval("(set-face! 'keyword #:fg \"#000001\")").unwrap();
    let s = rt.snapshot();
    assert_eq!(s.style(s.pane().layers[1].style).fg.map(|c| c.rgb), Some([0, 0, 1]));
    rt.eval("(unset-face! 'keyword)").unwrap();
    let s = rt.snapshot();
    assert_eq!(s.style(s.pane().layers[1].style).fg.map(|c| c.rgb), Some([0xcb, 0xa6, 0xf7]), "and back");
}

/// A theme is a package of face settings: unloading it from under a
/// setting of the user's own brings its faces back in the snapshot,
/// though the setting in effect for them never changed.
#[test]
fn a_theme_unloads_under_a_user_setting() {
    let mut rt = runtime("hello", "emacs");
    let dir = tempfile::tempdir().unwrap();
    let theme = dir.path().join("th.scm");
    std::fs::write(&theme, "(import (techne editor))\n(set-face! 'keyword #:weight 'light)\n").unwrap();
    rt.eval("(define-minor-mode kw \"A keyword.\" #:layer (lambda (d from to) '((0 5 keyword)))) (set-option! 'kw #t)").unwrap();
    rt.eval(&format!("(load-theme 'th {:?})", theme.display().to_string())).unwrap();
    rt.eval("(set-face! 'keyword #:fg \"#010203\")").unwrap();
    let weight = |s: &Snapshot| s.style(s.pane().layers[0].style).weight;
    assert_eq!(weight(&rt.snapshot()), Some(techne_editor::present::Weight::Light), "the theme's weight, the user's colour");
    rt.eval("(unload-theme 'th)").unwrap();
    let s = rt.snapshot();
    assert_eq!(weight(&s), Some(techne_editor::present::Weight::Bold), "unloaded from underneath, the declaration's weight is back");
    assert_eq!(s.style(s.pane().layers[0].style).fg.map(|c| c.rgb), Some([1, 2, 3]), "the user's colour stays");
}

/// A face that clears an attribute turns it off where it is drawn over
/// one that has it, and a setting that clears one clears what the face
/// inherits.
#[test]
fn clearing_an_attribute_turns_it_off() {
    let mut rt = runtime("hello", "emacs");
    rt.eval(
        "(define-face plain-link \"A link without its underline.\" #:inherit 'link #:underline #f)
         (define-minor-mode links \"Links, one plain in the middle.\"
           #:layer (lambda (d from to) '((0 5 link) (1 3 plain-link))))
         (set-option! 'links #t)",
    )
    .unwrap();
    let s = rt.snapshot();
    let underlines: Vec<(usize, usize, Option<techne_editor::present::Underline>)> =
        s.pane().layers.iter().map(|h| (h.from, h.to, s.style(h.style).underline.map(|(u, _)| u))).collect();
    use techne_editor::present::Underline::{Line, None as NoLine};
    assert_eq!(underlines, [(0, 1, Some(Line)), (1, 3, Some(NoLine)), (3, 5, Some(Line))]);
}

/// A file is coloured by its suffix's syntax under everything else,
/// and not at all with the option off. A theme that paints every face
/// in sentinel colours leaves no other colour in a snapshot: nothing
/// is hardcoded outside the faces.
#[test]
fn files_are_coloured_by_their_syntax_and_themes_reach_everything() {
    let dir = tempfile::tempdir().unwrap();
    let (path, journal) = (dir.path().join("x.rs"), dir.path().join("x.journal"));
    std::fs::write(&path, "fn main() { let s = \"hi\"; } // done\n").unwrap();
    let (mut rt, _) = Runtime::open(&path, &journal, "emacs").unwrap();
    let s = rt.snapshot();
    let at = |s: &Snapshot, pos: usize| s.pane().layers.iter().find(|h| h.from <= pos && pos < h.to).map(|h| s.style(h.style));
    assert_eq!(at(&s, 0).and_then(|st| st.fg).map(|c| c.rgb), Some([0xcb, 0xa6, 0xf7]), "fn in keyword's colour");
    assert_eq!(at(&s, 32).and_then(|st| st.fg).map(|c| c.rgb), Some([0x7f, 0x84, 0x9c]), "the comment's");
    // Search paints over syntax, not the other way.
    keys(&mut rt, "C-s h i");
    let s = rt.snapshot();
    assert_eq!(at(&s, 22).and_then(|st| st.bg).map(|c| c.rgb), Some([0x8b, 0x3a, 0x8b]), "isearch over the string");
    keys(&mut rt, "RET");
    rt.eval("(set-option! 'syntax-highlight #f)").unwrap();
    assert!(rt.snapshot().pane().layers.is_empty(), "off is off");
    rt.eval(
        "(set-option! 'syntax-highlight #t)
         (set-palette! 'sentinel \"#010203\")
         (for-each (lambda (f) (set-face! f #:fg 'sentinel #:bg 'sentinel)) (face-names))",
    )
    .unwrap();
    let s = rt.snapshot();
    assert!(!s.pane().layers.is_empty());
    // Every style this snapshot uses (the interned table keeps older
    // generations' styles too, for parts of snapshots still held).
    let chrome = s.chrome;
    let mut used: Vec<u32> = s.pane().layers.iter().map(|h| h.style).collect();
    used.extend([
        s.pane().default,
        chrome.region,
        chrome.cursor,
        chrome.mode_line,
        chrome.line_number,
        chrome.completion,
        chrome.selected,
        chrome.prompt,
        chrome.eob_marker,
        chrome.which_key_key,
        chrome.which_key_separator,
        chrome.which_key_group,
    ]);
    for id in used {
        let style = s.style(id);
        for colour in [style.fg, style.bg, style.underline.and_then(|(_, c)| c)].into_iter().flatten() {
            assert_eq!(colour.rgb, [1, 2, 3], "a colour no face gives: {style:?}");
        }
    }
}

#[test]
fn keys_go_through_the_profile() {
    let mut rt = runtime("hello", "emacs");
    keys(&mut rt, "C-e ! C-x");
    let s = rt.snapshot();
    assert_eq!(s.pane().text.to_string(), "hello!");
    assert_eq!(s.pane().head(), 6);
    assert_eq!(s.pane().cursor, CursorShape::Bar);
    assert_eq!(s.answers.len(), 3);
    assert!(s.echo.contains("C-x-"), "pending prefix shown: {}", s.echo);
    assert!(rt.snapshot().answers.is_empty());
}

#[test]
fn a_click_on_an_older_snapshot_is_re_resolved() {
    let mut rt = runtime("world", "emacs");
    let old = rt.snapshot();
    // Text inserted before the clicked position since the snapshot.
    keys(&mut rt, "h e l l o SPC C-e");
    click(&mut rt, old.pane().revision, 2);
    let s = rt.snapshot();
    assert_eq!(s.pane().text.to_string(), "hello world");
    assert_eq!(s.pane().head(), 8, "the click lands on the same character");
}

#[test]
fn a_click_on_deleted_text_is_refused() {
    let mut rt = runtime("one two", "emacs");
    let old = rt.snapshot();
    keys(&mut rt, "M-f M-d");
    let before = rt.snapshot();
    click(&mut rt, old.pane().revision, 5);
    let s = rt.snapshot();
    assert_eq!(s.pane().head(), before.pane().head(), "the caret did not move");
    assert!(s.echo.contains("changed"), "{}", s.echo);
    // A position past the end of that revision's text is refused too.
    click(&mut rt, old.pane().revision, 99);
    assert_eq!(rt.snapshot().pane().head(), before.pane().head());
}

#[test]
fn the_scroll_anchor_follows_edits() {
    let mut rt = runtime("a\nb\nc\nd\n", "emacs");
    let s = rt.snapshot();
    rt.handle(Input::Scroll { view: s.pane().view, revision: s.pane().revision, anchor: 4, caret: None });
    assert_eq!(rt.snapshot().pane().scroll, 4);
    keys(&mut rt, "x RET");
    assert_eq!(rt.snapshot().pane().scroll, 6, "text inserted above the screen does not move it");
}

#[test]
fn modal_clicks_and_cursor() {
    let mut rt = runtime("abc\n", "modal");
    let s = rt.snapshot();
    assert_eq!(s.pane().cursor, CursorShape::Block);
    click(&mut rt, s.pane().revision, 3);
    assert_eq!(rt.snapshot().pane().head(), 2, "the cursor stays on a character");
    keys(&mut rt, "i");
    assert_eq!(rt.snapshot().pane().cursor, CursorShape::Bar);
}

#[test]
fn keys_a_frontend_cannot_send_are_reported() {
    let mut rt = runtime("", "emacs");
    let bound = rt.bindings();
    assert!(["C-/", "C-x C-s", "M-<"].iter().all(|k| bound.iter().any(|b| b == k)), "{bound:?}");
    rt.handle(Input::Unsendable { keys: vec!["C-/".into(), "C-?".into()] });
    let status = rt.snapshot().echo;
    assert!(status.contains("cannot send: C-/ (undo), C-? (redo)"), "{status}");
    rt.handle(Input::Unrecognized { input: "\\x1b[99~".into() });
    let status = rt.snapshot().echo;
    assert!(status.contains("Unrecognized input: \\x1b[99~"), "{status}");
    // The modal profile's: its leader keys and the modes' keys, which a
    // terminal must be able to send too.
    let modal = runtime("", "modal").bindings();
    assert!(["SPC f f", "C-c C-r"].iter().all(|k| modal.iter().any(|b| b == k)), "{modal:?}");
}

#[test]
fn saving_and_quitting() {
    let dir = tempfile::tempdir().unwrap();
    let (path, journal) = (dir.path().join("f.txt"), dir.path().join("f.journal"));
    std::fs::write(&path, "x").unwrap();
    let (mut rt, _) = Runtime::open(&path, &journal, "modal").unwrap();
    assert!(matches!(keys(&mut rt, "A y ESC : w q RET"), Some(Output::Quit)));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "xy");
    drop(rt);

    let (mut rt, _) = Runtime::open(&path, &journal, "emacs").unwrap();
    keys(&mut rt, "z");
    assert!(rt.snapshot().pane().status.contains("[+]"));
    assert!(matches!(keys(&mut rt, "C-x C-c"), Some(Output::Quit)));
    drop(rt);
    // Quitting without saving keeps the edit in the journal.
    let (mut rt, _) = Runtime::open(&path, &journal, "emacs").unwrap();
    let s = rt.snapshot();
    assert_eq!(s.pane().text.to_string(), "zxy");
    assert!(s.echo.contains("Recovered 1 unsaved edits"), "{}", s.echo);
}

#[test]
fn wq_does_not_quit_when_the_write_fails() {
    // A scratch buffer has no file to write, nor a journal to keep it.
    let mut rt = runtime("", "modal");
    assert!(keys(&mut rt, "i y ESC : w q RET").is_none());
    assert_eq!(rt.snapshot().pane().text.to_string(), "y");
    assert!(keys(&mut rt, ": x RET").is_none());

    // A file changed on disk since it was read is not written over.
    let dir = tempfile::tempdir().unwrap();
    let (path, journal) = (dir.path().join("f.txt"), dir.path().join("f.journal"));
    std::fs::write(&path, "x").unwrap();
    let (mut rt, _) = Runtime::open(&path, &journal, "modal").unwrap();
    keys(&mut rt, "A y ESC");
    std::fs::write(&path, "changed").unwrap();
    assert!(keys(&mut rt, ": w q RET").is_none());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "changed");
}

/// A world over its memory limit says so in the echo area, which needs no
/// Lisp, and stops saying so once it is back under.
#[test]
fn memory_pressure_is_shown() {
    let mut rt = runtime("hello", "emacs");
    rt.eval("(define ballast (make-vector (* 8 1024 1024) 0))").unwrap();
    let held = rt.held();
    rt.set_memory_limit(held - (16 << 20));
    assert!(rt.snapshot().echo.contains("Out of memory"), "{}", rt.snapshot().echo);
    rt.set_memory_limit(2 * held);
    assert!(!rt.snapshot().echo.contains("Out of memory"), "{}", rt.snapshot().echo);
}

/// An allocation flood cannot stall the editor: a task flooding, catching
/// each refusal and trying again, is killed, and keys go on being
/// answered.
#[test]
fn a_flood_does_not_stall_the_editor() {
    let mut rt = runtime("hello", "emacs");
    rt.eval("(define kept '())").unwrap();
    let limit = rt.held() + (64 << 20);
    rt.set_memory_limit(limit);
    rt.eval("(define flood (spawn (lambda () (let retry () (guard (e (#t (retry))) (let loop () (set! kept (cons 0 kept)) (loop)))))))")
        .unwrap();
    for _ in 0..40 {
        rt.run_tasks(std::time::Duration::from_millis(50)).unwrap();
    }
    // Killed after its third refusal, a step of growth each.
    assert_eq!(rt.eval("(task-done? flood)").unwrap(), "#t");
    assert!(rt.held() < limit + (64 << 20), "{} MB held", rt.held() >> 20);
    keys(&mut rt, "C-e !");
    let s = rt.snapshot();
    assert_eq!(s.pane().text.to_string(), "hello!", "echo: {}", s.echo);
}

/// A motion, or a search or indentation, given a position inside a
/// character refuses it, rather than panicking in a native, which would
/// retire the world.
#[test]
fn a_motion_inside_a_character_is_an_error() {
    let mut rt = runtime("", "emacs");
    let e = rt.eval("(prev-grapheme (make-document \"\u{e9}\u{e9}\") 1)").unwrap_err();
    assert!(e.to_string().contains("inside a character"), "{e}");
    assert_eq!(rt.eval("(char-start (make-document \"\u{e9}\u{e9}\") 3)").unwrap(), "2");
    for native in [
        "(search-text-all (make-document \"\u{e9}\u{e9}\") \"\u{e9}\" 1 4 #f)",
        "(lisp-indent-edits (make-document \"\u{e9}\") '(1) (make-indent-specs))",
    ] {
        let e = rt.eval(native).unwrap_err();
        assert!(e.to_string().contains("character"), "{native}: {e}");
    }
    assert_eq!(rt.eval("(+ 1 2)").unwrap(), "3");
}
