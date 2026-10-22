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

use crate::service::Snapshot;

/// Files larger than this are not searched.
const FILE_BYTES: u64 = 8 << 20;
/// Of a line found, the bytes returned, at most.
const LINE_BYTES: usize = 300;

/// A part of a pattern, as the minibuffer matches it (`matcher.rs`):
/// without case, lowered as `str::to_lowercase` lowers, unless it has
/// an upper-case letter.
struct Part {
    bytes: Vec<u8>,
    fold: bool,
}

impl Part {
    fn new(part: &str) -> Part {
        let fold = !part.chars().any(char::is_uppercase);
        Part { bytes: if fold { part.to_lowercase().into_bytes() } else { part.as_bytes().to_vec() }, fold }
    }

    fn in_line(&self, line: &[u8], folded: &[u8]) -> bool {
        memmem::find(if self.fold { folded } else { line }, &self.bytes).is_some()
    }
}

/// (line position text) of each line of `text` that has every one of
/// `parts`, `limit` at most: its number from 1, the byte position of its
/// start, and its first `LINE_BYTES` bytes.
fn matching_lines(text: &[u8], parts: &[Part], limit: usize) -> Vec<(usize, usize, String)> {
    if parts.is_empty() {
        return Vec::new();
    }
    if text.is_ascii() {
        return ascii_lines(text, parts, limit);
    }
    // A line at a time, lowered as the minibuffer lowers it.
    let starts = std::iter::once(0).chain(memchr::memchr_iter(b'\n', text).map(|i| i + 1));
    text.split(|&b| b == b'\n')
        .zip(starts)
        .enumerate()
        .filter(|(_, (line, _))| {
            let folded = techne_text::motion::lowered(&String::from_utf8_lossy(line)).0;
            parts.iter().all(|p| p.in_line(line, folded.as_bytes()))
        })
        .take(limit)
        .map(|(i, (line, start))| (i + 1, start, shown(line)))
        .collect()
}

/// `matching_lines` of ASCII text, the most: the first part found in the
/// whole text at once, the others then in its lines.
fn ascii_lines(text: &[u8], parts: &[Part], limit: usize) -> Vec<(usize, usize, String)> {
    let Some((first, rest)) = parts.split_first() else { return Vec::new() };
    let folded = text.to_ascii_lowercase();
    let haystack = if first.fold { &folded } else { text };
    let finder = memmem::Finder::new(&first.bytes);
    let (mut found, mut from) = (Vec::new(), 0);
    // Lines are counted up to `counted`, which is at line `line`.
    let (mut counted, mut line) = (0, 1);
    while found.len() < limit
        && from < text.len()
        && let Some(at) = finder.find(&haystack[from..]).map(|i| from + i)
    {
        let start = memchr::memrchr(b'\n', &text[..at]).map_or(0, |i| i + 1);
        let end = memchr::memchr(b'\n', &text[at..]).map_or(text.len(), |i| at + i);
        if rest.iter().all(|p| p.in_line(&text[start..end], &folded[start..end])) {
            line += memchr::memchr_iter(b'\n', &text[counted..start]).count();
            counted = start;
            found.push((line, start, shown(&text[start..end])));
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

fn parts(parts: &[String]) -> Vec<Part> {
    parts.iter().filter(|p| !p.is_empty()).map(|p| Part::new(p)).collect()
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
            /// that has all of PARTS, strings, LIMIT at most. A part matches
            /// without case unless it has an upper-case letter. A binary file,
            /// one over 8 MiB, or one that cannot be read has none.
            #[vm]
            "(search-file path parts limit)" => |vm: &mut Vm, path: String, ps: Vec<String>, limit: usize| -> Result<Vec<Data>, Error> {
                let p = Path::new(&path);
                if !vm.may_read(p) {
                    return Err(Error::new(format!("search-file: {path} may not be read here")));
                }
                Ok(read_text(p).map_or_else(Vec::new, |t| as_data(matching_lines(&t, &parts(&ps), limit))))
            };
            /// Return (line position text) of each line of SNAPSHOT's text
            /// that has all of PARTS, as `search-file` does.
            "(search-snapshot snapshot parts limit)" => |s: Foreign<Snapshot>, ps: Vec<String>, limit: usize| {
                as_data(matching_lines(s.0.text.to_string().as_bytes(), &parts(&ps), limit))
            };
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str, ps: &[&str]) -> Vec<(usize, usize, String)> {
        matching_lines(text.as_bytes(), &parts(&ps.iter().map(|p| p.to_string()).collect::<Vec<_>>()), 10)
    }

    #[test]
    fn lines_with_every_part_once_each() {
        let text = "alpha beta\nbeta\nBeta alpha alpha\nalpha";
        assert_eq!(lines(text, &["alpha", "beta"]), [(1, 0, "alpha beta".into()), (3, 16, "Beta alpha alpha".into())]);
        // An upper-case letter makes a part match case.
        assert_eq!(lines(text, &["Beta"]), [(3, 16, "Beta alpha alpha".into())]);
        assert_eq!(lines(text, &[]), []);
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
