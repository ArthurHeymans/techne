//! Start an editor session on an empty document, and exit: what loading
//! the editor's Lisp costs, for `runtime/bench/icount.sh`.

use techne_editor::runtime::Runtime;

fn main() {
    Runtime::with_document(techne_text::Document::new(""), "emacs").expect("the editor starts");
}
