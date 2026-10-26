//! Start an editor session on an empty document, and exit: what loading
//! the editor's Lisp costs, for `runtime/bench/icount.sh`.
//!
//! `startup --data` starts it too, then prints how much Lisp it loaded:
//! the data of the prelude and of every file module, each list and atom
//! one (a docstring is one, however long). The script divides the
//! instructions of a plain run by them: a rate that the Lisp the editor
//! ships growing leaves alone, and a slower reader, expander or compiler
//! does not. Counting is not part of the run counted.

use techne_editor::runtime::Runtime;
use techne_vm::reader::{Syntax, SyntaxKind, read_syntax};

/// The data in `s`, itself included.
fn data(s: &Syntax) -> usize {
    1 + match &s.kind {
        SyntaxKind::Atom(_) => 0,
        SyntaxKind::List(items, tail) => items.iter().chain(tail.as_deref()).map(data).sum(),
        SyntaxKind::Vector(items) | SyntaxKind::Dict(items) | SyntaxKind::Set(items) => items.iter().map(data).sum(),
        SyntaxKind::Labeled(_, inner) => data(inner),
    }
}

fn source_data(source: &str) -> usize {
    read_syntax(source).expect("the editor's Lisp reads").iter().map(data).sum()
}

fn main() {
    let mut rt = Runtime::with_document(techne_text::Document::new(""), "emacs").expect("the editor starts");
    if std::env::args().nth(1).is_some_and(|a| a == "--data") {
        // The file modules' paths, a line each.
        let paths = rt.eval("(string-join (filter (lambda (m) (string-prefix? \"/\" m)) (loaded-modules)) \"\\n\")").expect("the modules");
        let paths = paths.trim_matches('"').replace("\\n", "\n");
        let files: usize = paths.lines().map(|p| source_data(&std::fs::read_to_string(p).expect("a module's file"))).sum();
        println!("{}", files + source_data(techne_vm::PRELUDE));
    }
}
