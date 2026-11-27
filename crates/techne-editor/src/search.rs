//! What a search service's world reads files with
//! (`lisp/editor/services/search.scm`): a project's files, as ripgrep
//! walks them, and the lines of a text that have every part of a
//! pattern. Its Lisp calls them a file at a time, so a request stays
//! stoppable between files.

use std::{io::Read, path::Path, sync::atomic::Ordering};

use memchr::memmem;
use techne_vm::{
    api::Foreign,
    data::Data,
    vm::{Capability, Error, INTERRUPTED, Vm},
};

use regex_cursor::engines::meta::Regex;

use crate::{
    matcher::{FilterPart, PartKind, compile_regexp, flex_match, parse_filter_parts, regexp_is_match},
    service::Snapshot,
};

/// Files larger than this are not searched.
const FILE_BYTES: u64 = 8 << 20;
/// Of a line found, the bytes returned, at most.
const LINE_BYTES: usize = 300;

/// A compiled Orderless part for line searching.
enum Part {
    Literal { bytes: Vec<u8>, fold: bool },
    Flex { needle: String, fold: bool },
    LiteralOrRegexp { bytes: Vec<u8>, fold: bool, re: Regex },
    Regexp(Regex),
}

impl Part {
    fn from_filter(part: &FilterPart) -> Part {
        let lit_bytes = |p: &FilterPart| {
            if p.fold { p.text.to_lowercase().into_bytes() } else { p.text.as_bytes().to_vec() }
        };
        match part.kind {
            PartKind::Literal => Part::Literal { bytes: lit_bytes(part), fold: part.fold },
            PartKind::Flex => Part::Flex { needle: if part.fold { part.text.to_lowercase() } else { part.text.clone() }, fold: part.fold },
            PartKind::LiteralOrRegexp => match compile_regexp(&part.text, part.fold) {
                Some(re) => Part::LiteralOrRegexp { bytes: lit_bytes(part), fold: part.fold, re },
                None => Part::Literal { bytes: lit_bytes(part), fold: part.fold },
            },
            PartKind::Regexp => match compile_regexp(&part.text, part.fold) {
                Some(re) => Part::Regexp(re),
                None => Part::Literal { bytes: lit_bytes(part), fold: part.fold },
            },
        }
    }

    fn in_line(&self, line: &[u8], folded: &[u8]) -> bool {
        match self {
            Part::Literal { bytes, fold } => memmem::find(if *fold { folded } else { line }, bytes).is_some(),
            Part::Flex { needle, fold } => {
                let hay = String::from_utf8_lossy(if *fold { folded } else { line });
                flex_match(&hay, needle)
            }
            Part::LiteralOrRegexp { bytes, fold, re } => {
                memmem::find(if *fold { folded } else { line }, bytes).is_some() || regexp_is_match(re, line)
            }
            Part::Regexp(re) => regexp_is_match(re, line),
        }
    }
}

/// (line position text) of each line of `text` that has every one of
/// `parts`, `limit` at most: its number from 1, the byte position of its
/// start, and its first `LINE_BYTES` bytes.
fn matching_lines(text: &[u8], parts: &[Part], limit: usize) -> Vec<(usize, usize, String)> {
    if parts.is_empty() || text.is_empty() {
        return Vec::new();
    }
    // When ASCII and at least one pure literal part exists (without embedded
    // line breaks), prefilter the whole text with memmem on the longest one.
    if text.is_ascii()
        && let Some((idx, _)) = parts
            .iter()
            .enumerate()
            .filter_map(|(i, p)| match p {
                Part::Literal { bytes, .. } if !bytes.contains(&b'\n') && !bytes.contains(&b'\r') => Some((i, bytes.len())),
                _ => None,
            })
            .max_by_key(|&(_, len)| len)
    {
        return ascii_lines(text, parts, idx, limit);
    }
    // Strip a single trailing LF so `^$` does not match a synthetic empty
    // segment past the last line.
    let body = text.strip_suffix(b"\n").unwrap_or(text);
    // A line at a time, lowered as the minibuffer lowers it.
    let starts = std::iter::once(0).chain(memchr::memchr_iter(b'\n', body).map(|i| i + 1));
    body.split(|&b| b == b'\n')
        .zip(starts)
        .enumerate()
        .filter(|(_, (line, _))| {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let folded = techne_text::motion::lowered(&String::from_utf8_lossy(line)).0;
            parts.iter().all(|p| p.in_line(line, folded.as_bytes()))
        })
        .take(limit)
        .map(|(i, (line, start))| (i + 1, start, shown(line)))
        .collect()
}

/// `matching_lines` of ASCII text with a literal prefilter at `lead`:
/// found in the whole text at once, the others then in its lines.
fn ascii_lines(text: &[u8], parts: &[Part], lead: usize, limit: usize) -> Vec<(usize, usize, String)> {
    let Part::Literal { bytes, fold } = &parts[lead] else { unreachable!() };
    let folded = text.to_ascii_lowercase();
    let haystack = if *fold { &folded } else { text };
    let finder = memmem::Finder::new(bytes);
    let (mut found, mut from) = (Vec::new(), 0);
    // Lines are counted up to `counted`, which is at line `line`.
    let (mut counted, mut line) = (0, 1);
    while found.len() < limit
        && from < text.len()
        && let Some(at) = finder.find(&haystack[from..]).map(|i| from + i)
    {
        let start = memchr::memrchr(b'\n', &text[..at]).map_or(0, |i| i + 1);
        let end = memchr::memchr(b'\n', &text[at..]).map_or(text.len(), |i| at + i);
        let raw_end = if text[start..end].ends_with(b"\r") { end - 1 } else { end };
        if at + bytes.len() <= raw_end {
            let raw_line = &text[start..raw_end];
            let folded_line = &folded[start..raw_end];
            if parts.iter().enumerate().all(|(i, p)| i == lead || p.in_line(raw_line, folded_line)) {
                line += memchr::memchr_iter(b'\n', &text[counted..start]).count();
                counted = start;
                found.push((line, start, shown(&text[start..end])));
            }
        }
        from = end + 1;
    }
    found
}

/// A line as returned: its first `LINE_BYTES` bytes, not cutting a
/// character.
fn shown(line: &[u8]) -> String {
    let mut cut = line.len().min(LINE_BYTES);
    while cut < line.len() && line[cut] & 0xc0 == 0x80 {
        cut -= 1;
    }
    String::from_utf8_lossy(&line[..cut]).trim_end_matches('\r').to_string()
}

/// The text of the regular file at `path`, unless it is over
/// `FILE_BYTES`, binary (a zero byte early on), gone or unreadable. It
/// is opened without waiting, so that a FIFO with no writer is passed
/// over rather than blocking the search.
fn read_text(path: &Path) -> Option<Vec<u8>> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(path)
        .ok()
        .filter(|f| f.metadata().is_ok_and(|m| m.is_file()))?;
    let mut text = Vec::new();
    file.take(FILE_BYTES + 1).read_to_end(&mut text).ok()?;
    (text.len() as u64 <= FILE_BYTES && memchr::memchr(0, &text[..text.len().min(8192)]).is_none()).then_some(text)
}

/// `root` canonical with a slash, then the names of the files under it,
/// sorted, as ripgrep finds them:
/// what `.gitignore` and `.ignore` files ignore, and hidden files, left
/// out. Stopped as Lisp is between entries. A confined world reads no
/// ignore files outside its root: its walk takes `.gitignore` files as
/// it takes `.ignore` files, those under `root` only, and none of Git's
/// own (global, `info/exclude`, above `root`). An ignore file under
/// `root` that is a link to one outside is still read.
fn project_files(vm: &Vm, root: &Path) -> Result<Vec<String>, String> {
    if !vm.may_read(root) {
        return Err(format!("project-files: {} may not be read here", root.display()));
    }
    let root = root.canonicalize().map_err(|e| format!("{}: {e}", root.display()))?;
    let confined = vm.reading_root().is_some();
    let stop = vm.interrupt_handle();
    let mut files = vec![];
    let mut walk = ignore::WalkBuilder::new(&root);
    // A Jujutsu repository need not be Git's.
    walk.require_git(false);
    if confined {
        walk.parents(false).git_ignore(false).git_exclude(false).git_global(false).add_custom_ignore_filename(".gitignore");
    }
    for entry in walk.build() {
        if stop.flag().load(Ordering::Relaxed) {
            stop.take();
            return Err(INTERRUPTED.to_string());
        }
        if let Ok(e) = entry
            && e.file_type().is_some_and(|t| t.is_file())
        {
            files.push(e.path().strip_prefix(&root).unwrap_or(e.path()).display().to_string());
        }
    }
    files.sort();
    Ok(std::iter::once(root.join("").display().to_string()).chain(files).collect())
}

fn as_data(lines: Vec<(usize, usize, String)>) -> Vec<Data> {
    lines.into_iter().map(|(line, at, text)| Data::List(vec![Data::Int(line as i64), Data::Int(at as i64), Data::String(text)])).collect()
}

fn compile_parts(items: &[Data]) -> Result<Vec<Part>, String> {
    Ok(parse_filter_parts(items, true)?.iter().map(Part::from_filter).collect())
}

/// The natives of a search service's world, which has the files
/// capability; they read only where it may (`Vm::may_read`).
pub(crate) fn install(vm: &mut Vm) {
    vm.requiring(Capability::Files, |vm| {
        techne_vm::procedures! { vm;
            /// Return (root name ...): ROOT canonical, with a slash, then the
            /// names under it of its files, sorted, as ripgrep finds them:
            /// what `.gitignore` and `.ignore` files ignore, and hidden files,
            /// left out.
            #[vm]
            "(project-files root)" => |vm: &mut Vm, root: String| -> Result<Vec<String>, Error> {
                project_files(vm, Path::new(&root)).map_err(Error::new)
            };
            /// Return (line position text) of each line of the file at PATH
            /// that has all of PARTS, LIMIT at most. Each part is either a
            /// string (smart-case literal) or `(kind text fold?)` as
            /// `orderless-filter-specs` produces. A binary file, one over
            /// 8 MiB, or one that cannot be read has none.
            #[vm]
            "(search-file path parts limit)" => |vm: &mut Vm, path: String, ps: techne_vm::api::Value, limit: usize| -> Result<Vec<Data>, Error> {
                let p = Path::new(&path);
                if !vm.may_read(p) {
                    return Err(Error::new(format!("search-file: {path} may not be read here")));
                }
                let Data::List(items) = vm.to_data(&ps, 64 << 20)? else {
                    return Err(Error::new("search-file: parts must be a list"));
                };
                let compiled = compile_parts(&items).map_err(Error::new)?;
                Ok(read_text(p).map_or_else(Vec::new, |t| as_data(matching_lines(&t, &compiled, limit))))
            };
            /// Return (line position text) of each line of SNAPSHOT's text
            /// that has all of PARTS, as `search-file` does.
            #[vm]
            "(search-snapshot snapshot parts limit)" => |vm: &mut Vm, s: Foreign<Snapshot>, ps: techne_vm::api::Value, limit: usize| -> Result<Vec<Data>, Error> {
                let Data::List(items) = vm.to_data(&ps, 64 << 20)? else {
                    return Err(Error::new("search-snapshot: parts must be a list"));
                };
                let compiled = compile_parts(&items).map_err(Error::new)?;
                Ok(as_data(matching_lines(s.0.text.to_string().as_bytes(), &compiled, limit)))
            };
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str, ps: &[&str]) -> Vec<(usize, usize, String)> {
        let compiled: Vec<Part> = ps.iter().filter(|p| !p.is_empty()).map(|p| Part::from_filter(&FilterPart::from_legacy(p))).collect();
        matching_lines(text.as_bytes(), &compiled, 10)
    }

    #[test]
    fn lines_with_every_part_once_each() {
        let text = "alpha beta\nbeta\nBeta alpha alpha\nalpha";
        assert_eq!(lines(text, &["alpha", "beta"]), [(1, 0, "alpha beta".into()), (3, 16, "Beta alpha alpha".into())]);
        // An upper-case letter makes a part match case.
        assert_eq!(lines(text, &["Beta"]), [(3, 16, "Beta alpha alpha".into())]);
        assert_eq!(lines(text, &[]), []);
    }

    #[test]
    fn lines_match_orderless_flex_and_regexp_parts() {
        let text = "describe-named-command\nalpha-one\nbeta-two\nhello world\n";
        let flex = vec![Part::from_filter(&FilterPart { kind: PartKind::Flex, text: "dscrb".into(), fold: true })];
        assert_eq!(matching_lines(text.as_bytes(), &flex, 10), [(1, 0, "describe-named-command".into())]);
        let alt = vec![Part::from_filter(&FilterPart { kind: PartKind::Regexp, text: "alpha|beta".into(), fold: true })];
        assert_eq!(matching_lines(text.as_bytes(), &alt, 10), [(2, 23, "alpha-one".into()), (3, 33, "beta-two".into())]);
        let spaced = vec![Part::from_filter(&FilterPart::literal("hello world", true))];
        assert_eq!(matching_lines(text.as_bytes(), &spaced, 10), [(4, 42, "hello world".into())]);

        // CRLF: literal CR must not match a stripped line; `^$` must not
        // match a synthetic empty segment after a trailing newline.
        let crlf = "one\r\ntwo\r\n";
        let cr = vec![Part::from_filter(&FilterPart::literal("\r", false))];
        assert_eq!(matching_lines(crlf.as_bytes(), &cr, 10), []);
        let empty = vec![Part::from_filter(&FilterPart { kind: PartKind::Regexp, text: "^$".into(), fold: true })];
        assert_eq!(matching_lines(crlf.as_bytes(), &empty, 10), []);
        let with_blank = "one\n\ntwo\n";
        assert_eq!(matching_lines(with_blank.as_bytes(), &empty, 10), [(2, 4, "".into())]);
    }

    /// Listing a project's files stops as Lisp does, between entries.
    #[test]
    fn listing_files_stops_for_an_interrupt() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), "").unwrap();
        let mut vm = Vm::new();
        let exec = vm.new_execution();
        vm.enter_execution(exec);
        assert_eq!(project_files(&vm, dir.path()).unwrap().len(), 2);
        vm.interrupt_handle().stop(exec, techne_vm::vm::Stop::Break);
        assert_eq!(project_files(&vm, dir.path()), Err(INTERRUPTED.to_string()));
    }

    /// A confined world's walk, which reads no ignore file above its root,
    /// still takes those under it, `.gitignore` too.
    #[test]
    fn a_confined_walk_reads_ignore_files_under_its_root_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        // Above the root: not taken.
        std::fs::write(dir.path().join(".gitignore"), "kept\n").unwrap();
        std::fs::write(root.join(".gitignore"), "dropped\n").unwrap();
        std::fs::write(root.join("sub/.ignore"), "also-dropped\n").unwrap();
        for f in ["kept", "dropped", "sub/also-dropped", "sub/file"] {
            std::fs::write(root.join(f), "").unwrap();
        }
        let mut vm = Vm::new();
        vm.confine_reading(&root).unwrap();
        let files = project_files(&vm, &root).unwrap();
        assert_eq!(files[1..], ["kept", "sub/file"]);
    }

    /// A FIFO is passed over, not waited on.
    #[test]
    fn a_fifo_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("fifo");
        assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        assert_eq!(read_text(&fifo), None);
    }

    /// Lowered as the minibuffer lowers, beyond ASCII too, the positions
    /// those of the text.
    #[test]
    fn letters_beyond_ascii_match_without_case() {
        assert_eq!(lines("één\nÉÉN twee\n", &["één", "TWEE"]), []);
        assert_eq!(lines("één\nÉÉN twee\n", &["één", "twee"]), [(2, 6, "ÉÉN twee".into())]);
    }
}
