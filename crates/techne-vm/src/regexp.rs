//! Compiled regular expressions.
//!
//! A `regexp` is a pattern in the syntax of Rust's regex crate, compiled by
//! `regex-cursor`, which searches text in chunks: a string here, a rope in
//! the editor, without making one string of it. Searches stop between
//! chunks when the VM is interrupted. SRFI 115 (`lisp/srfi/115.sld`)
//! translates its s-expression patterns (SREs) to this syntax.
//!
//! A search reads its text from the chunk before the position it starts
//! at, so searching from match to match does not read the text again. Only
//! reading forward is interrupted: the engine also steps back over text
//! already read, to find where a match starts, which it cannot be stopped
//! in the middle of, but which goes back no further than the match.

use std::{
    cell::OnceCell,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
};

use regex_cursor::{
    Cursor, Input,
    engines::meta::Regex,
    regex_automata::{Anchored, util::captures::Captures},
};

use crate::{
    builtins::{str_arg, type_error},
    cursors::offset,
    value::Value,
    vm::{Error, Vm},
};

type R = Result<Value, Error>;

/// A compiled regular expression, with what SRFI 115 keeps of its source.
pub struct Regexp {
    pattern: String,
    regex: Regex,
    /// The pattern anchored at both ends, compiled when first needed.
    whole: OnceCell<Regex>,
    /// The SRE it was compiled from, as `write` prints it.
    sre: String,
    /// The name (a symbol) of each capturing group from 1, if it has one.
    names: Vec<Option<u32>>,
}

impl Regexp {
    pub fn new(pattern: &str, sre: String, names: Vec<Option<u32>>) -> Result<Regexp, String> {
        let regex = Regex::new(pattern).map_err(|e| e.to_string())?;
        Ok(Regexp { pattern: pattern.to_owned(), regex, whole: OnceCell::new(), sre, names })
    }

    pub fn regex(&self) -> &Regex {
        &self.regex
    }

    /// The regex matching only the whole of the text searched.
    fn whole(&self) -> Result<&Regex, String> {
        if self.whole.get().is_none() {
            let regex = Regex::new(&format!(r"(?:{})\z", self.pattern)).map_err(|e| e.to_string())?;
            let _ = self.whole.set(regex);
        }
        Ok(self.whole.get().expect("set above"))
    }

    /// The spans of the match and its groups in `cursor`'s text, searched
    /// from `from` to `to`; `None` for no match. With `whole`, the match
    /// must start at `from` and end at `to`. `Stopped` when `stop` is set
    /// before the search ends.
    pub fn search<C: Cursor>(&self, cursor: C, from: usize, to: usize, whole: bool, stop: &AtomicBool) -> Result<Option<Captures>, Stop> {
        let regex = if whole { self.whole().map_err(Stop::Error)? } else { &self.regex };
        let mut cursor = Interruptible { inner: cursor, stop, stopped: false };
        let mut caps = regex.create_captures();
        let mut input = Input::new(&mut cursor);
        input.set_range(from..to);
        if whole {
            input.set_anchored(Anchored::Yes);
        }
        regex.search_captures(input, &mut caps);
        if cursor.stopped { Err(Stop::Stopped) } else { Ok(caps.is_match().then_some(caps)) }
    }
}

/// Why a search gave no answer.
#[derive(Debug)]
pub enum Stop {
    /// It was interrupted.
    Stopped,
    /// The pattern anchored at both ends did not compile.
    Error(String),
}

/// A cursor that ends early, at the next chunk, once `stop` is set.
struct Interruptible<'a, C> {
    inner: C,
    stop: &'a AtomicBool,
    stopped: bool,
}

impl<C: Cursor> Cursor for Interruptible<'_, C> {
    fn chunk(&self) -> &[u8] {
        self.inner.chunk()
    }
    fn utf8_aware(&self) -> bool {
        self.inner.utf8_aware()
    }
    fn advance(&mut self) -> bool {
        if self.stop.load(Ordering::Relaxed) {
            self.stopped = true;
            return false;
        }
        self.inner.advance()
    }
    fn backtrack(&mut self) -> bool {
        self.inner.backtrack()
    }
    fn total_bytes(&self) -> Option<usize> {
        self.inner.total_bytes()
    }
    fn offset(&self) -> usize {
        self.inner.offset()
    }
}

/// A string in chunks of about `CHUNK` bytes, each whole characters, so
/// that searching a long string can be interrupted.
struct StrChunks<'a> {
    text: &'a str,
    start: usize,
    end: usize,
}

const CHUNK: usize = 1 << 16;

impl<'a> StrChunks<'a> {
    /// The chunks of `text` from the one holding the character before `at`
    /// (for the look-behind of `\b` and the like).
    fn at(text: &'a str, at: usize) -> Self {
        let mut start = at.saturating_sub(4);
        while !text.is_char_boundary(start) {
            start -= 1;
        }
        StrChunks { text, start, end: Self::after(text, start) }
    }
    fn after(text: &str, at: usize) -> usize {
        let mut end = (at + CHUNK).min(text.len());
        while !text.is_char_boundary(end) {
            end += 1;
        }
        end
    }
}

impl Cursor for StrChunks<'_> {
    fn chunk(&self) -> &[u8] {
        &self.text.as_bytes()[self.start..self.end]
    }
    fn advance(&mut self) -> bool {
        if self.end == self.text.len() {
            return false;
        }
        (self.start, self.end) = (self.end, Self::after(self.text, self.end));
        true
    }
    fn backtrack(&mut self) -> bool {
        if self.start == 0 {
            return false;
        }
        // Not necessarily the chunk advancing gave: any whole characters
        // before this chunk will do.
        let mut start = self.start.saturating_sub(CHUNK);
        while !self.text.is_char_boundary(start) {
            start -= 1;
        }
        (self.start, self.end) = (start, self.start);
        true
    }
    fn total_bytes(&self) -> Option<usize> {
        Some(self.text.len())
    }
    fn offset(&self) -> usize {
        self.start
    }
}

#[inline(always)]
fn arg(vm: &Vm, args: usize, i: usize) -> Value {
    vm.regs[args + i]
}

fn regexp_arg(vm: &Vm, v: Value, who: &str) -> Result<Rc<Regexp>, Error> {
    vm.foreign(v).and_then(|(rc, _)| rc.clone().downcast::<Regexp>().ok()).ok_or_else(|| type_error(who, "regexp", v))
}

/// `(%make-regexp pattern sre names)`.
fn make(vm: &mut Vm, args: usize, _: usize) -> R {
    let pattern = str_arg(arg(vm, args, 0), "regexp")?;
    let sre = str_arg(arg(vm, args, 1), "regexp")?.to_owned();
    let names = crate::builtins::list_values(arg(vm, args, 2)).ok_or_else(|| type_error("regexp", "list", arg(vm, args, 2)))?;
    let names = names
        .into_iter()
        .map(|n| match () {
            _ if n.is_symbol() => Ok(Some(n.as_symbol())),
            _ if n.is_false() => Ok(None),
            _ => Err(type_error("regexp", "symbol naming a submatch", n)),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let re = Regexp::new(pattern, sre, names).map_err(|e| Error::new(format!("regexp: {e}")))?;
    Ok(vm.make_foreign(Rc::new(re), std::any::type_name::<Regexp>()))
}

/// `(%regexp-search regexp string start end from whole?)`: searches STRING
/// from FROM to END as if it began at START (for `bos` and the like), all
/// cursors. Returns #f, or a vector of the start and end cursors of the
/// match and of each group, #f for a group that did not take part.
fn search(vm: &mut Vm, args: usize, _: usize) -> R {
    let who = "regexp-search";
    let re = regexp_arg(vm, arg(vm, args, 0), who)?;
    let [start, end, from] = [2, 3, 4].map(|i| offset(vm, args, 1, arg(vm, args, i), who));
    let (start, end, from) = (start?, end?, from?);
    if !(start <= from && from <= end) {
        return Err(Error::new(format!("{who}: bad range {start}..{from}..{end}")));
    }
    let whole = arg(vm, args, 5).is_truthy();
    let text = &str_arg(arg(vm, args, 1), who)?[start..end];
    let found = re.search(StrChunks::at(text, from - start), from - start, end - start, whole, &vm.interrupt);
    let caps = match found {
        Err(Stop::Stopped) => return Err(vm.take_interrupt()),
        Err(Stop::Error(e)) => return Err(Error::new(format!("{who}: {e}"))),
        Ok(None) => return Ok(Value::FALSE),
        Ok(Some(caps)) => caps,
    };
    let spans: Vec<Value> = (0..caps.group_len())
        .flat_map(|g| match caps.get_group(g) {
            Some(span) => [Value::cursor(start + span.start), Value::cursor(start + span.end)],
            None => [Value::FALSE, Value::FALSE],
        })
        .collect();
    Ok(vm.make_vector(&spans))
}

/// `(%case-fold-class class ascii)`: the bracket class CLASS with every
/// character's other cases, by Unicode simple case folding; with ASCII,
/// only ASCII letters gain their other case.
fn case_fold_class(vm: &mut Vm, args: usize, _: usize) -> R {
    use regex_syntax::hir::{Class, ClassUnicode, ClassUnicodeRange, HirKind};
    let pattern = str_arg(arg(vm, args, 0), "regexp")?;
    let hir = regex_syntax::parse(pattern).map_err(|e| Error::new(format!("regexp: {e}")))?;
    // A class of one character is parsed as that character.
    let class = match hir.kind() {
        HirKind::Class(Class::Unicode(class)) => class.clone(),
        HirKind::Literal(l) => ClassUnicode::new(String::from_utf8_lossy(&l.0).chars().map(|c| ClassUnicodeRange::new(c, c))),
        _ => return Err(Error::new(format!("regexp: not a character class: {pattern}"))),
    };
    let mut folded = class.clone();
    if arg(vm, args, 1).is_truthy() {
        let ascii = ClassUnicode::new([ClassUnicodeRange::new('\0', '\x7f')]);
        let mut letters = class.clone();
        letters.intersect(&ascii);
        letters.case_fold_simple();
        letters.intersect(&ascii);
        folded.union(&letters);
    } else {
        folded.case_fold_simple();
    }
    let ranges: String = folded.iter().map(|r| format!("\\x{{{:x}}}-\\x{{{:x}}}", r.start() as u32, r.end() as u32)).collect();
    let class = if ranges.is_empty() { "[^\\x{0}-\\x{10FFFF}]".to_owned() } else { format!("[{ranges}]") };
    Ok(vm.make_string(&class))
}

pub fn install(vm: &mut Vm) {
    vm.name_foreign_type::<Regexp>("regexp");
    crate::natives! { vm;
        /// Return #t if OBJ is a compiled regular expression.
        "(regexp? obj)" => |vm: &mut Vm, a, _| Ok(Value::bool(regexp_arg(vm, arg(vm, a, 0), "").is_ok()));
        "(%make-regexp pattern sre names)" => make;
        "(%regexp-search regexp string start end from whole)" => search;
        "(%regexp-sre regexp)" => |vm: &mut Vm, a, _| { let re = regexp_arg(vm, arg(vm, a, 0), "regexp->sre")?; Ok(vm.make_string(&re.sre)) };
        "(%regexp-names regexp)" => |vm: &mut Vm, a, _| {
            let re = regexp_arg(vm, arg(vm, a, 0), "regexp")?;
            let names: Vec<Value> = re.names.iter().map(|n| n.map_or(Value::FALSE, Value::symbol)).collect();
            Ok(vm.make_list(&names))
        };
        "(%case-fold-class class ascii)" => case_fold_class;
        "(%regexp-pattern regexp)" => |vm: &mut Vm, a, _| { let re = regexp_arg(vm, arg(vm, a, 0), "regexp")?; Ok(vm.make_string(&re.pattern)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_strings_are_searched_in_chunks_that_an_interrupt_stops() {
        let text = format!("{}needle", "é".repeat(100_000));
        let re = Regexp::new("needle", String::new(), Vec::new()).unwrap();
        let (stop, go) = (AtomicBool::new(true), AtomicBool::new(false));
        assert!(matches!(re.search(StrChunks::at(&text, 0), 0, text.len(), false, &stop), Err(Stop::Stopped)));
        let found = re.search(StrChunks::at(&text, 0), 0, text.len(), false, &go).unwrap().unwrap();
        assert_eq!(found.get_match().unwrap().range(), 200_000..200_006);
        // A match across chunks, and one searched for backward over them.
        let across = Regexp::new("é{3}needle", String::new(), Vec::new()).unwrap();
        assert!(across.search(StrChunks::at(&text, 0), 0, text.len(), false, &go).unwrap().is_some());
        assert!(across.search(StrChunks::at(&text, 0), 0, text.len(), true, &go).is_ok());
    }
}
