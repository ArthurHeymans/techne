//! Lisp indentation, as Emacs's `lisp-mode` indents Scheme
//! (`calculate-lisp-indent`, with `scheme-indent-function`): a line is
//! indented by the list it is in. A form whose head has a spec, its number
//! of distinguished arguments, indents them by 4 and its body by 2, as do
//! heads starting with "def"; a call aligns its arguments under the first
//! when that is on the head's line, else under the head; data (a quoted
//! list, a vector, a bracketed list, a list whose head is not a symbol)
//! align under the first element.
//!
//! The text is scanned, not read: code being typed is seldom balanced.
//! The scan starts at the last line before the first one indented that
//! opens a list in its first column, as Emacs takes a defun's start. A line
//! starting inside a string or a block comment is left as it is.
//!
//! The scan goes byte by byte, what Lisp's syntax sees being ASCII, and
//! skips atoms, strings and comments in loops of their own: a text of
//! megabytes without a defun start takes a few instructions a byte. It
//! keeps positions, turned into columns only for what a line aligns with.

use std::{collections::HashMap, ops::Range};

use techne_text::ropey::Rope;

/// How many distinguished arguments each head has.
pub type Specs = HashMap<String, usize>;

/// The specs of Scheme's own forms, as Emacs's scheme-mode puts them;
/// others are added from Lisp, as Emacs's `scheme-indent-function`
/// property is put.
pub fn default_specs() -> Specs {
    [
        ("begin", 0),
        ("case", 1),
        ("case-lambda", 0),
        ("delay", 0),
        ("do", 2),
        ("guard", 1),
        ("lambda", 1),
        ("let", 1),
        ("let*", 1),
        ("let-syntax", 1),
        ("let-values", 1),
        ("let*-values", 1),
        ("letrec", 1),
        ("letrec*", 1),
        ("letrec-syntax", 1),
        ("parameterize", 1),
        ("receive", 2),
        ("syntax-case", 2),
        ("syntax-rules", 1),
        ("unless", 1),
        ("when", 1),
    ]
    .into_iter()
    .map(|(name, n)| (name.to_string(), n))
    .collect()
}

/// A list open where the scan is; positions are in the text scanned.
struct Frame {
    /// Its open bracket.
    open: usize,
    /// Quoted, a vector or bracketed: aligned under its first element.
    data: bool,
    /// Elements begun in it so far.
    count: usize,
    /// Its first element's name, if a symbol, and where it begins.
    head: Option<String>,
    head_at: usize,
    /// Where its second element begins, if on the head's line, and whether
    /// that is a symbol (a named let).
    arg_at: Option<usize>,
    named: bool,
}

impl Frame {
    /// The column the next element, on a line of its own, goes to, `col`
    /// giving a position's.
    fn indent(&self, specs: &Specs, col: impl Fn(usize) -> usize) -> usize {
        let open = col(self.open);
        if self.count == 0 {
            return open + 1;
        }
        let head_col = col(self.head_at);
        let Some(head) = self.head.as_deref().filter(|_| !self.data) else { return head_col };
        // Arguments begun before the line: the index of the one on it.
        let arg = self.count - 1;
        let spec = match head {
            "let" if self.named => Some(2),
            _ => specs.get(head).copied(),
        };
        match spec {
            Some(n) if arg < n => open + 4,
            Some(_) => open + 2,
            None if head.starts_with("def") => open + 2,
            None => self.arg_at.map_or(head_col, col),
        }
    }
}

/// The scanner's place in the text.
#[derive(Clone, Copy, PartialEq)]
enum In {
    Code,
    /// An atom; the head's name is gathered if it is a list's first.
    Atom,
    String,
    LineComment,
    /// Nested block comments, so deep.
    BlockComment(usize),
    /// A symbol between bars.
    BarSymbol,
}

fn delimiter(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'(' | b')' | b'[' | b']' | b'"' | b';')
}

struct Scan<'a> {
    src: &'a [u8],
    at: usize,
    stack: Vec<Frame>,
    state: In,
    /// A prefix (quote, quasiquote, unquote, syntax) began the next datum:
    /// whether it makes a list data.
    prefix: Option<bool>,
    /// `#;` hides the next datum from the count.
    hidden: bool,
    /// The name of a list's head being read.
    head: Option<Range<usize>>,
    /// A list was opened, for `form_around`.
    opened: bool,
    /// Where the next `(`, `[` or `"`, and the next `;`, `#` or `|`, are
    /// from where they were last looked for.
    next: [Option<usize>; 2],
}

impl<'a> Scan<'a> {
    fn new(src: &'a [u8]) -> Scan<'a> {
        Scan { src, at: 0, stack: Vec::new(), state: In::Code, prefix: None, hidden: false, head: None, opened: false, next: [None; 2] }
    }

    /// A datum begins at `at`, unless a prefix began it already or `#;`
    /// hides it: count it in the open list. Return whether it is the
    /// list's head.
    fn begin(&mut self, at: usize) -> bool {
        if self.prefix.take().is_some() || std::mem::take(&mut self.hidden) {
            return false;
        }
        let Some(f) = self.stack.last_mut() else { return false };
        f.count += 1;
        match f.count {
            1 => {
                f.head_at = at;
                true
            }
            2 => {
                f.arg_at = memchr::memchr(b'\n', &self.src[f.head_at..at]).is_none().then_some(at);
                false
            }
            _ => false,
        }
    }

    /// Where the next byte from `i` is that counts outside any list: each
    /// set is searched once past where it was found.
    fn next_top(&mut self, i: usize) -> usize {
        let src = self.src;
        let sets = [(b'(', b'[', b'"'), (b';', b'#', b'|')];
        sets.iter().zip(&mut self.next).fold(src.len(), |nearest, (&(a, b, c), next)| {
            if next.is_none_or(|n| n < i) {
                *next = Some(memchr::memchr3(a, b, c, &src[i..]).map_or(src.len(), |n| i + n));
            }
            nearest.min(next.unwrap_or(src.len()))
        })
    }

    fn open(&mut self, at: usize, data: bool) {
        // A list outside any other, quoted: its quote was skipped.
        let data = data || (self.stack.is_empty() && at > 0 && matches!(self.src[at - 1], b'\'' | b'`'));
        self.begin(at);
        self.stack.push(Frame { open: at, data, count: 0, head: None, head_at: at + 1, arg_at: None, named: false });
        self.opened = true;
    }

    /// Scan up to `to`; with `until_closed`, stop where the first list
    /// opened closes, and return where.
    fn run(&mut self, to: usize, until_closed: bool) -> Option<usize> {
        let src = self.src;
        while self.at < to {
            let i = self.at;
            let b = src[i];
            match self.state {
                In::Atom => {
                    let end = src[i..to].iter().position(|&b| delimiter(b)).map_or(to, |n| i + n);
                    self.at = end;
                    if end < to {
                        if let (Some(name), Some(f)) = (self.head.take(), self.stack.last_mut()) {
                            f.head = Some(String::from_utf8_lossy(&src[name.start..end]).into_owned());
                        }
                        self.state = In::Code;
                    }
                }
                In::String => match memchr::memchr2(b'"', b'\\', &src[i..to]) {
                    Some(n) if src[i + n] == b'\\' => self.at = i + n + 2,
                    Some(n) => (self.at, self.state) = (i + n + 1, In::Code),
                    None => self.at = to,
                },
                In::LineComment => match memchr::memchr(b'\n', &src[i..to]) {
                    Some(n) => (self.at, self.state) = (i + n, In::Code),
                    None => self.at = to,
                },
                In::BlockComment(depth) => match memchr::memchr2(b'|', b'#', &src[i..to]) {
                    Some(n) => {
                        let (k, next) = (i + n, src.get(i + n + 1).copied());
                        self.at = k + 1;
                        match (src[k], next) {
                            (b'|', Some(b'#')) => {
                                self.at += 1;
                                self.state = if depth == 1 { In::Code } else { In::BlockComment(depth - 1) };
                            }
                            (b'#', Some(b'|')) => {
                                self.at += 1;
                                self.state = In::BlockComment(depth + 1);
                            }
                            _ => {}
                        }
                    }
                    None => self.at = to,
                },
                In::BarSymbol => match memchr::memchr(b'|', &src[i..to]) {
                    Some(n) => (self.at, self.state) = (i + n + 1, In::Code),
                    None => self.at = to,
                },
                // Outside any list only what opens one, a string or a
                // comment counts: found with memchr, skipping atoms and
                // quotes (a quote before a list is seen from it).
                In::Code if self.stack.is_empty() && !matches!(b, b'(' | b'[' | b'"' | b';' | b'#' | b'|') => {
                    (self.prefix, self.hidden) = (None, false);
                    self.at = self.next_top(i).min(to);
                }
                In::Code => {
                    self.at = i + 1;
                    let next = src.get(i + 1).copied();
                    match b {
                        b'(' | b'[' => {
                            let data = b == b'[' || self.prefix == Some(true);
                            self.open(i, data);
                        }
                        b')' | b']' => {
                            self.stack.pop();
                            self.prefix = None;
                            if until_closed && self.stack.is_empty() {
                                return Some(i + 1);
                            }
                        }
                        b'"' => {
                            self.begin(i);
                            self.state = In::String;
                        }
                        b';' => self.state = In::LineComment,
                        b'\'' | b'`' | b',' => {
                            if self.prefix.is_none() {
                                self.begin(i);
                            }
                            self.prefix = Some(self.prefix.unwrap_or(false) || b != b',');
                            if b == b',' && next == Some(b'@') {
                                self.at += 1;
                            }
                        }
                        b'#' => self.after_hash(i, next),
                        b'|' => {
                            self.begin(i);
                            self.state = In::BarSymbol;
                        }
                        b if delimiter(b) => {}
                        _ => {
                            let head = self.begin(i);
                            self.head = head.then_some(i..i);
                            // A let's second element, a symbol, names it.
                            if let Some(f) = self.stack.last_mut().filter(|f| f.count == 2 && f.head.as_deref() == Some("let")) {
                                f.named = true;
                            }
                            self.state = In::Atom;
                        }
                    }
                }
            }
        }
        None
    }

    /// What follows a `#` at `i`.
    fn after_hash(&mut self, i: usize, next: Option<u8>) {
        match next {
            Some(b'|') => (self.at, self.state) = (i + 2, In::BlockComment(1)),
            Some(b';') => (self.at, self.hidden) = (i + 2, true),
            Some(b'(') => {
                self.begin(i);
                // The vector's elements are its own, not again the vector.
                self.prefix = Some(true);
                self.open(i + 1, true);
                self.at = i + 2;
            }
            Some(b'\'' | b'`' | b',') => {
                if self.prefix.is_none() {
                    self.begin(i);
                }
                (self.prefix, self.at) = (Some(false), i + 2);
            }
            Some(b'\\') => {
                // A character: the one after `#\` whatever it is, then a
                // name (`#\space`) to a delimiter.
                self.begin(i);
                let c = i + 2;
                let len = self.src.get(c).map_or(0, |&b| if b < 0x80 { 1 } else { b.leading_ones() as usize });
                (self.at, self.state) = ((c + len).min(self.src.len()), In::Atom);
            }
            _ => {
                self.begin(i);
                self.state = In::Atom;
            }
        }
    }
}

/// The start of the last line before `pos` that opens a list in its first
/// column, as Emacs takes a defun's start; else the text's start.
fn defun_start(text: &Rope, pos: usize) -> usize {
    let mut end = pos;
    while end > 0 {
        let (chunk, start, _, _) = text.chunk_at_byte(end - 1);
        let bytes = &chunk.as_bytes()[..end - start];
        for i in memchr::memrchr_iter(b'(', bytes) {
            let line_start = match (i, start) {
                (0, 0) => true,
                (0, _) => text.byte(start - 1) == b'\n',
                _ => bytes[i - 1] == b'\n',
            };
            if line_start {
                return start + i;
            }
        }
        end = start;
    }
    0
}

/// The bytes of `text` in `range`, copied in one piece.
fn bytes(text: &Rope, range: Range<usize>) -> Vec<u8> {
    let mut out = Vec::with_capacity(range.len());
    text.byte_slice(range).chunks().for_each(|c| out.extend_from_slice(c.as_bytes()));
    out
}

/// The columns `bytes` take from column `col`: tabs to the next stop of
/// 8, a character each else.
fn width(bytes: &[u8], col: usize) -> usize {
    bytes.iter().filter(|&&b| b & 0xc0 != 0x80).fold(col, |c, &b| if b == b'\t' { c + 8 - c % 8 } else { c + 1 })
}

/// The column each line at `starts` (sorted line starts) should begin at,
/// or `None` when it starts inside a string or a block comment. Each line
/// is taken as already indented when the next is measured, as Emacs
/// indents a region line by line.
pub fn columns(text: &Rope, starts: &[usize], specs: &Specs) -> Vec<Option<usize>> {
    let (Some(&first), Some(&last)) = (starts.first(), starts.last()) else { return Vec::new() };
    let from = defun_start(text, text.line_to_byte(text.byte_to_line(first)));
    // To the end of the last line: its blanks are what it is indented past.
    let to = text.line_to_byte((text.byte_to_line(last) + 1).min(text.len_lines()));
    let src = bytes(text, from..to);
    let mut scan = Scan::new(&src);
    // Lines indented already: their blanks' end and the column it is at.
    let mut indented: HashMap<usize, (usize, usize)> = HashMap::new();
    let mut out = Vec::with_capacity(starts.len());
    for &start in starts {
        let at = start - from;
        scan.run(at, false);
        let column = match scan.state {
            In::String | In::BlockComment(_) => None,
            _ => Some(scan.stack.last().map_or(0, |f| {
                f.indent(specs, |pos| {
                    let line = memchr::memrchr(b'\n', &src[..pos]).map_or(0, |n| n + 1);
                    match indented.get(&line) {
                        Some(&(blanks_end, col)) if pos >= blanks_end => width(&src[blanks_end..pos], col),
                        _ => width(&src[line..pos], 0),
                    }
                })
            })),
        };
        if let Some(column) = column {
            let blanks = src[at..].iter().take_while(|&&b| b == b' ' || b == b'\t').count();
            indented.insert(at, (at + blanks, column));
        }
        out.push(column);
    }
    out
}

/// The span of the top-level form around `pos`, or the last before it:
/// from the line opening a list in its first column at or before `pos`'s
/// line to where that list closes (the text's end if it does not).
pub fn form_around(text: &Rope, pos: usize) -> Option<Range<usize>> {
    let line_end = text.line_to_byte((text.byte_to_line(pos) + 1).min(text.len_lines()));
    let from = defun_start(text, line_end);
    // No line before opens a list in its first column: no form.
    if text.len_bytes() == from || text.byte(from) != b'(' {
        return None;
    }
    let src = bytes(text, from..text.len_bytes());
    let mut scan = Scan::new(&src);
    match scan.run(src.len(), true) {
        Some(end) => Some(from..from + end),
        None => scan.opened.then(|| from..text.len_bytes()),
    }
}

/// The edits (from to text) that indent the lines at `starts` (sorted
/// line starts) as `columns` says, replacing their leading blanks; lines
/// already right are left.
pub fn edits(text: &Rope, starts: &[usize], specs: &Specs) -> Vec<(usize, usize, String)> {
    starts
        .iter()
        .zip(columns(text, starts, specs))
        .filter_map(|(&start, column)| {
            let column = column?;
            let blanks = text.byte_slice(start..).bytes().take_while(|&b| b == b' ' || b == b'\t').count();
            let current = text.byte_slice(start..start + blanks);
            let wanted = " ".repeat(column);
            (current != wanted.as_str()).then(|| (start, start + blanks, wanted))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text with each line indented as `columns` says.
    fn indented(text: &str) -> String {
        let rope = Rope::from_str(text);
        let starts: Vec<usize> = (0..rope.len_lines()).map(|l| rope.line_to_byte(l)).collect();
        let mut edits = edits(&rope, &starts, &default_specs());
        edits.reverse();
        let mut out = text.to_string();
        for (from, to, text) in edits {
            out.replace_range(from..to, &text);
        }
        out
    }

    #[test]
    fn bodies_by_two_arguments_under_the_first() {
        let flat = "(define (f x)\n(let ((a 1)\n(b 2))\n(when (> a b)\n(display a)\n(newline))\n(+ a\nb\n(* x\n2))))\n";
        assert_eq!(
            indented(flat),
            "(define (f x)\n  (let ((a 1)\n        (b 2))\n    (when (> a b)\n      (display a)\n      (newline))\n    (+ a\n       b\n       (* x\n          2))))\n"
        );
    }

    #[test]
    fn distinguished_arguments_named_let_and_calls_without_one() {
        assert_eq!(indented("(lambda\n(x)\nx)\n"), "(lambda\n    (x)\n  x)\n");
        assert_eq!(indented("(let loop ((i 0))\n(loop i))\n"), "(let loop ((i 0))\n  (loop i))\n");
        assert_eq!(indented("(let loop\n((i 0))\n(loop i))\n"), "(let loop\n    ((i 0))\n  (loop i))\n");
        assert_eq!(indented("(foo\nbar\nbaz)\n"), "(foo\n bar\n baz)\n");
        assert_eq!(
            indented("(define-command (greet s n)\n\"Greet.\"\n(message! s \"hi\"))\n"),
            "(define-command (greet s n)\n  \"Greet.\"\n  (message! s \"hi\"))\n"
        );
    }

    #[test]
    fn data_aligns_under_its_first_element() {
        assert_eq!(indented("'(a\nb)\n"), "'(a\n  b)\n");
        assert_eq!(indented("(list '(define\nb)\n#(1\n2))\n"), "(list '(define\n        b)\n      #(1\n        2))\n");
        assert_eq!(indented("((f x)\ny)\n"), "((f x)\n y)\n");
    }

    #[test]
    fn strings_comments_and_characters_are_not_code() {
        // A docstring's continuation lines are left in their column.
        let docstring = "(define (f)\n\"A (paren\nin a string.\"\n#\\( ; (\n1)\n";
        assert_eq!(indented(docstring), "(define (f)\n  \"A (paren\nin a string.\"\n  #\\( ; (\n  1)\n");
        // Its first argument on a line of its own, `b` aligns under the head.
        assert_eq!(indented("(f #| (\n|# a\nb)\n"), "(f #| (\n|# a\n b)\n");
        assert_eq!(indented("(f #\\space\nb)\n"), "(f #\\space\n   b)\n");
    }

    #[test]
    fn the_form_around_a_position() {
        // Far back, across the rope's chunks: no line opens a list since.
        let text = format!("(a)\n(define (f)\n  \"(\"\n{}  (g))\n(b)\n", "  x\n".repeat(5000));
        let rope = Rope::from_str(&text);
        let inside = text.find("(g)").unwrap();
        let span = form_around(&rope, inside).unwrap();
        assert_eq!(&text[span.clone()], &text[4..text.find("(b)").unwrap() - 1]);
        // On the opening line, and past the form's end on its last line.
        assert_eq!(form_around(&rope, 5), Some(span.clone()));
        assert_eq!(form_around(&rope, span.end - 1), Some(span));
        assert_eq!(form_around(&rope, 0), Some(0..3));
    }

    #[test]
    fn unbalanced_text_and_the_scan_start() {
        // Typed so far: no closing parentheses.
        assert_eq!(indented("(define (f x)\n(if x\n"), "(define (f x)\n  (if x\n      ");
        // Only the lines asked for, from the defun before them.
        let rope = Rope::from_str("(a)\n(define (f)\n(g\nx))\n");
        let line = |l| rope.line_to_byte(l);
        assert_eq!(columns(&rope, &[line(2), line(3)], &default_specs()), [Some(2), Some(3)]);
    }
}
