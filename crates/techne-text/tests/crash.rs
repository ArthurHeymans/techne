//! The journal after crashes: a torn last record, and a writer killed with
//! SIGKILL while editing.

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
};

use techne_text::{Actor, Document, Recovery};

/// The `i`th edit of a deterministic session: an insertion or a deletion
/// somewhere in the current text.
fn nth_edit(doc: &Document, i: u64) -> (std::ops::Range<usize>, String) {
    let mut x = i.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0xdead_beef;
    let mut next = |n: usize| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x % n.max(1) as u64) as usize
    };
    let text = doc.text().to_string();
    let bounds: Vec<usize> = text.char_indices().map(|(i, _)| i).chain([text.len()]).collect();
    let a = bounds[next(bounds.len())];
    if next(3) == 0 && a < text.len() {
        let b = bounds[(bounds.iter().position(|&p| p == a).unwrap() + 1 + next(4)).min(bounds.len() - 1)];
        (a..b, String::new())
    } else {
        (a..a, ["é", "x", "\n", "漢字"][next(4)].to_string())
    }
}

fn session_step(doc: &mut Document, actor: &Actor, i: u64) {
    let tx = doc.edit(actor, [nth_edit(doc, i)]).unwrap();
    doc.apply(tx).unwrap();
}

/// The text after each number of steps.
fn model(start: &str, steps: u64) -> Vec<String> {
    let actor: Actor = Arc::from("writer");
    let mut doc = Document::new(start);
    let mut texts = vec![doc.text().to_string()];
    for i in 0..steps {
        session_step(&mut doc, &actor, i);
        texts.push(doc.text().to_string());
    }
    texts
}

#[test]
fn a_torn_last_record_is_discarded() {
    let dir = tempfile::tempdir().unwrap();
    let (path, journal) = (dir.path().join("f.txt"), dir.path().join("f.journal"));
    fs::write(&path, "start\n").unwrap();
    let actor: Actor = Arc::from("writer");
    let (mut doc, _) = Document::open(&path, &journal).unwrap();
    let mut sizes = vec![fs::metadata(&journal).unwrap().len()];
    for i in 0..20 {
        session_step(&mut doc, &actor, i);
        sizes.push(fs::metadata(&journal).unwrap().len());
    }
    drop(doc);
    let texts = model("start\n", 20);
    let full = fs::read(&journal).unwrap();

    for cut in sizes[19]..=sizes[20] {
        fs::write(&journal, &full[..cut as usize]).unwrap();
        let (doc, recovery) = Document::open(&path, &journal).unwrap();
        let whole = cut == sizes[20];
        let steps = if whole { 20 } else { 19 };
        assert_eq!(doc.text().to_string(), texts[steps], "cut at {cut}");
        assert_eq!(recovery, Recovery::Replayed { transactions: steps, torn: !whole && cut != sizes[19] });
    }
}

const CHILD_DIR: &str = "TECHNE_TEXT_CRASH_DIR";

/// The writer: edits until it is killed, acknowledging each applied edit.
#[test]
fn crash_child() {
    let Ok(dir) = std::env::var(CHILD_DIR) else { return };
    let dir = Path::new(&dir);
    let actor: Actor = Arc::from("writer");
    let (mut doc, _) = Document::open(&dir.join("f.txt"), &dir.join("f.journal")).unwrap();
    let mut out = std::io::stdout().lock();
    for i in 0..u64::MAX {
        session_step(&mut doc, &actor, i);
        writeln!(out, "ack {}", i + 1).unwrap();
        out.flush().unwrap();
    }
}

#[test]
fn a_killed_writer_loses_nothing_it_acknowledged() {
    for target in [1, 37, 400] {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("f.txt"), "start\n").unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_child", "--nocapture", "--test-threads=1"])
            .env(CHILD_DIR, dir.path())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut acked = 0u64;
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        for line in lines.by_ref() {
            if let Some(n) = line.unwrap().strip_prefix("ack ") {
                acked = n.parse().unwrap();
                if acked >= target {
                    break;
                }
            }
        }
        child.kill().unwrap();
        for line in lines {
            if let Some(n) = line.unwrap().strip_prefix("ack ") {
                acked = n.parse().unwrap();
            }
        }
        child.wait().unwrap();

        let (doc, _) = Document::open(&dir.path().join("f.txt"), &dir.path().join("f.journal")).unwrap();
        let steps = doc.revision();
        assert!(steps >= acked, "recovered {steps} edits, {acked} were acknowledged");
        assert_eq!(doc.text().to_string(), model("start\n", steps)[steps as usize]);
        // The file itself was never touched: the edits are unsaved.
        assert_eq!(fs::read_to_string(dir.path().join("f.txt")).unwrap(), "start\n");
        assert!(doc.is_dirty());
    }
}
