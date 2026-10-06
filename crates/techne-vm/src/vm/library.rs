//! R7RS libraries over the module system: `define-library` makes a module
//! named by the library name, `import` brings in bindings (with only,
//! except, prefix and rename), `cond-expand` chooses by features (R7RS 5.6,
//! 7.1.7, 4.2.1).
//!
//! A library `(foo bar)` that is not yet defined is looked for as
//! `foo/bar.sld` or `foo/bar.scm`: next to the importing file (for a
//! library, at the root of its directory tree), in the directories of
//! `TECHNE_LIBRARY_PATH`, then in the working directory. The standard `(scheme ...)`
//! libraries are the root module: its bindings are visible everywhere, so
//! importing one changes nothing unless a modifier renames or selects (a
//! deviation: the standard bindings cannot be hidden by not importing them).

use std::path::{Path, PathBuf};

use super::{Error, GlobalBinding, ROOT_MODULE, Vm};
use crate::reader::{self, Sexp, strip, symbol_name};

const STANDARD: &[&str] = &[
    "base",
    "case-lambda",
    "char",
    "complex",
    "cxr",
    "eval",
    "file",
    "inexact",
    "lazy",
    "load",
    "process-context",
    "read",
    "repl",
    "time",
    "write",
    "r5rs",
];

fn sym_name(s: &Sexp) -> Option<std::rc::Rc<str>> {
    s.sym().map(|x| symbol_name(strip(x)))
}

fn bad(what: &str, s: &Sexp) -> Error {
    Error::new(format!("{what}: {}", reader::display_sexp(s)))
}

impl Vm {
    /// The parts of a library name, `(foo bar 1)`.
    fn library_parts(name: &Sexp) -> Result<Vec<String>, Error> {
        let parts = name.list().filter(|p| !p.is_empty()).ok_or_else(|| bad("bad library name", name))?;
        parts
            .iter()
            .map(|p| match p {
                Sexp::Sym(s) => Ok(symbol_name(strip(*s)).to_string()),
                Sexp::Int(i) if *i >= 0 => Ok(i.to_string()),
                _ => Err(bad("bad library name", name)),
            })
            .collect()
    }

    fn library_key(parts: &[String]) -> String {
        format!("({})", parts.join(" "))
    }

    fn is_standard(parts: &[String]) -> bool {
        matches!(parts, [scheme, lib] if scheme == "scheme" && STANDARD.contains(&lib.as_str()))
    }

    /// `(define-library name declaration ...)`, at top level of `from`.
    pub fn define_library(&mut self, from: u32, file: u32, items: &[Sexp]) -> Result<(), Error> {
        let name = items.get(1).ok_or_else(|| Error::new("define-library: missing name"))?;
        let key = Self::library_key(&Self::library_parts(name)?);
        if self.libraries.contains_key(&key) {
            return Err(Error::new(format!("define-library: {key} is already defined")));
        }
        let path = self.modules[from as usize].path.clone();
        let m = self.new_module(&key, path);
        self.libraries.insert(key, m);
        items[2..].iter().try_for_each(|d| self.library_declaration(m, file, d))
    }

    fn library_declaration(&mut self, m: u32, file: u32, decl: &Sexp) -> Result<(), Error> {
        let items = decl.list().filter(|d| !d.is_empty()).ok_or_else(|| bad("bad library declaration", decl))?;
        let head = sym_name(&items[0]).ok_or_else(|| bad("bad library declaration", decl))?;
        match &*head {
            "export" => {
                for spec in &items[1..] {
                    let (internal, external) = match spec {
                        Sexp::Sym(s) => (strip(*s), strip(*s)),
                        Sexp::List(r, None, _) if r.len() == 3 && sym_name(&r[0]).as_deref() == Some("rename") => {
                            let (a, b) = (r[1].sym(), r[2].sym());
                            (a.map(strip).ok_or_else(|| bad("bad export", spec))?, b.map(strip).ok_or_else(|| bad("bad export", spec))?)
                        }
                        _ => return Err(bad("bad export", spec)),
                    };
                    let module = &mut self.modules[m as usize];
                    module.exports.get_or_insert_with(Vec::new).push(external);
                    if internal != external {
                        module.renamed.insert(external, internal);
                    }
                }
                Ok(())
            }
            "import" => self.import(m, &items[1..]),
            "begin" => items[1..].iter().try_for_each(|f| self.eval_sexp_in_file(m, file, f).map(drop)),
            "include" | "include-ci" => {
                for f in &items[1..] {
                    let (path, text) = self.read_included(m, f)?;
                    self.eval_in(m, &path, &text)?;
                }
                Ok(())
            }
            "include-library-declarations" => {
                for f in &items[1..] {
                    let (_, text) = self.read_included(m, f)?;
                    for d in reader::read(&text).map_err(Error::new)? {
                        self.library_declaration(m, file, &d)?;
                    }
                }
                Ok(())
            }
            "cond-expand" => match self.cond_expand(m, &items[1..])? {
                Some(body) => body.iter().try_for_each(|d| self.library_declaration(m, file, d)),
                None => Ok(()),
            },
            _ => Err(bad("unknown library declaration", decl)),
        }
    }

    /// The directory of `m`'s file, or the working directory.
    fn module_dir(&self, m: u32) -> PathBuf {
        self.modules[m as usize].path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("."))
    }

    /// Where libraries imported by `m` are looked for first: next to its
    /// file, or for a library `(a b c)` in `a/b/c.sld`, where `a` is.
    fn library_root(&self, m: u32) -> PathBuf {
        let mut dir = self.module_dir(m);
        if let Some((key, _)) = self.libraries.iter().find(|(_, l)| **l == m) {
            for _ in 1..key.split(' ').count() {
                dir.pop();
            }
        }
        dir
    }

    fn read_included(&mut self, m: u32, f: &Sexp) -> Result<(String, String), Error> {
        self.check_loading()?;
        let Sexp::Str(name) = f else { return Err(bad("include: expected a file name", f)) };
        let path = self.module_dir(m).join(&**name);
        let text = std::fs::read_to_string(&path).map_err(|e| Error::new(format!("include {name}: {e}")))?;
        Ok((path.to_string_lossy().into_owned(), text))
    }

    /// `(import set ...)` into `into`.
    pub fn import(&mut self, into: u32, sets: &[Sexp]) -> Result<(), Error> {
        for set in sets {
            // A standard library imported whole is already visible.
            if let Ok(parts) = Self::library_parts(set)
                && Self::is_standard(&parts)
            {
                continue;
            }
            for (local, binding) in self.import_set(into, set)? {
                self.modules[into as usize].imports.insert(local, binding);
            }
        }
        Ok(())
    }

    /// The bindings an import set brings, under their local names.
    fn import_set(&mut self, into: u32, set: &Sexp) -> Result<Vec<(u32, GlobalBinding)>, Error> {
        let items = set.list().filter(|s| !s.is_empty()).ok_or_else(|| bad("bad import set", set))?;
        let modifier = sym_name(&items[0])
            .filter(|h| matches!(&**h, "only" | "except" | "prefix" | "rename"))
            .filter(|_| items.len() >= 2 && items[1].list().is_some());
        let Some(modifier) = modifier else {
            let m = self.library_module(into, set)?;
            return self.exported(m, set);
        };
        let base = self.import_set(into, &items[1])?;
        let syms = |xs: &[Sexp]| {
            xs.iter().map(|x| x.sym().map(strip).ok_or_else(|| bad("bad import set", set))).collect::<Result<Vec<u32>, Error>>()
        };
        Ok(match &*modifier {
            "only" => {
                let keep = syms(&items[2..])?;
                base.into_iter().filter(|(s, _)| keep.contains(s)).collect()
            }
            "except" => {
                let drop = syms(&items[2..])?;
                base.into_iter().filter(|(s, _)| !drop.contains(s)).collect()
            }
            "prefix" => {
                let p = items.get(2).and_then(sym_name).ok_or_else(|| bad("bad import set", set))?;
                base.into_iter().map(|(s, b)| (reader::intern(&format!("{p}{}", symbol_name(s))), b)).collect()
            }
            _ => {
                let mut pairs = Vec::new();
                for r in &items[2..] {
                    match r.list() {
                        Some([a, b]) => pairs.push((a.sym().map(strip), b.sym().map(strip))),
                        _ => return Err(bad("bad rename", r)),
                    }
                }
                base.into_iter().map(|(s, b)| (pairs.iter().find(|(a, _)| *a == Some(s)).and_then(|(_, to)| *to).unwrap_or(s), b)).collect()
            }
        })
    }

    /// What module `m` exports, by external name.
    fn exported(&self, m: u32, name: &Sexp) -> Result<Vec<(u32, GlobalBinding)>, Error> {
        if m == ROOT_MODULE {
            return Ok(self.bindings.iter().filter(|((module, _), _)| *module == ROOT_MODULE).map(|((_, s), b)| (*s, b.clone())).collect());
        }
        let module = &self.modules[m as usize];
        let names = module.exports.clone().unwrap_or_else(|| module.defined.clone());
        names
            .into_iter()
            .map(|ext| {
                let internal = module.renamed.get(&ext).copied().unwrap_or(ext);
                self.bindings
                    .get(&(m, internal))
                    .or_else(|| module.imports.get(&internal))
                    .cloned()
                    .map(|b| (ext, b))
                    .ok_or_else(|| Error::new(format!("{} exports undefined {}", reader::display_sexp(name), symbol_name(internal))))
            })
            .collect()
    }

    /// The module of a library, loading its file if it is not defined yet.
    fn library_module(&mut self, from: u32, name: &Sexp) -> Result<u32, Error> {
        let parts = Self::library_parts(name)?;
        if Self::is_standard(&parts) {
            return Ok(ROOT_MODULE);
        }
        let key = Self::library_key(&parts);
        if let Some(&m) = self.libraries.get(&key) {
            return Ok(m);
        }
        self.check_loading()?;
        let relative: PathBuf = parts.iter().collect();
        let mut dirs = vec![self.library_root(from)];
        if let Some(path) = std::env::var_os("TECHNE_LIBRARY_PATH") {
            dirs.extend(std::env::split_paths(&path));
        }
        dirs.push(PathBuf::from("."));
        for dir in dirs {
            for ext in ["sld", "scm"] {
                let file = dir.join(&relative).with_extension(ext);
                if let Ok(file) = file.canonicalize() {
                    self.load_module(&file).map_err(|e| Error::new(format!("{key}: {}", e.msg)))?;
                    if let Some(&m) = self.libraries.get(&key) {
                        return Ok(m);
                    }
                }
            }
        }
        Err(Error::new(format!("no library {key}")))
    }

    /// The declarations or forms of the first `cond-expand` clause whose
    /// feature requirement holds.
    pub fn cond_expand<'a>(&mut self, from: u32, clauses: &'a [Sexp]) -> Result<Option<&'a [Sexp]>, Error> {
        for clause in clauses {
            let items = clause.list().filter(|c| !c.is_empty()).ok_or_else(|| bad("bad cond-expand clause", clause))?;
            if self.feature_holds(from, &items[0])? {
                return Ok(Some(&items[1..]));
            }
        }
        Ok(None)
    }

    fn feature_holds(&mut self, from: u32, req: &Sexp) -> Result<bool, Error> {
        if let Some(name) = sym_name(req) {
            return Ok(&*name == "else" || crate::r7rs::FEATURES.contains(&&*name));
        }
        let items = req.list().filter(|r| !r.is_empty()).ok_or_else(|| bad("bad feature requirement", req))?;
        let head = sym_name(&items[0]).ok_or_else(|| bad("bad feature requirement", req))?;
        Ok(match (&*head, &items[1..]) {
            ("and", rest) => {
                for r in rest {
                    if !self.feature_holds(from, r)? {
                        return Ok(false);
                    }
                }
                true
            }
            ("or", rest) => {
                for r in rest {
                    if self.feature_holds(from, r)? {
                        return Ok(true);
                    }
                }
                false
            }
            ("not", [r]) => !self.feature_holds(from, r)?,
            ("library", [name]) => self.library_module(from, name).is_ok(),
            _ => return Err(bad("bad feature requirement", req)),
        })
    }
}
