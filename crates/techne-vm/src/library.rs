//! R7RS libraries (5.6) over the module system.
//!
//! `(define-library (name ...) declaration ...)` makes a module named by
//! the written library name, e.g. `(srfi 1)`; `export` sets what it
//! provides, `begin`, `include` and `import` fill it. `(import set ...)`
//! adds the bindings of import sets to a module.
//!
//! Every library named `(scheme ...)` or `(techne ...)` is the root module,
//! which every module already sees, so importing one adds nothing unless
//! `only`, `except`, `prefix` or `rename` select from it. Any other library
//! is defined where it is first imported from, or else searched for as
//! `a/b.sld` (for `(a b)`) in the importing file's directory, then in the
//! directories of `TECHNE_LIBRARY_PATH`.

use std::path::{Path, PathBuf};

use crate::{
    reader::{self, Sexp, display_sexp, intern, strip, symbol_name},
    vm::{Error, GlobalBinding, ROOT_MODULE, Vm},
};

type R<T> = Result<T, Error>;

fn err<T>(msg: impl Into<String>) -> R<T> {
    Err(Error::new(msg))
}

/// The features `cond-expand` and `features` know.
pub const FEATURES: &[&str] =
    &["r7rs", "exact-closed", "ratios-as-floats", "full-unicode", std::env::consts::OS, std::env::consts::ARCH, "techne"];

/// What an import set denotes.
enum Set {
    /// The root module: already visible.
    Root,
    Bindings(Vec<(u32, GlobalBinding)>),
}

/// A library name's parts: identifiers and exact integers.
fn name_parts(name: &Sexp) -> Option<Vec<String>> {
    name.list()?
        .iter()
        .map(|p| match p {
            Sexp::Sym(s) => Some(symbol_name(strip(*s)).to_string()),
            Sexp::Int(i) if *i >= 0 => Some(i.to_string()),
            _ => None,
        })
        .collect()
}

fn is_builtin(parts: &[String]) -> bool {
    matches!(parts.first().map(String::as_str), Some("scheme" | "techne"))
}

fn module_name(parts: &[String]) -> String {
    format!("({})", parts.join(" "))
}

fn syms(items: &[Sexp], who: &str) -> R<Vec<u32>> {
    items.iter().map(|i| i.sym().map(strip).ok_or_else(|| Error::new(format!("{who}: expected identifiers")))).collect()
}

impl Vm {
    /// `(import set ...)` into `module`; libraries are looked up from `dir`.
    pub fn import(&mut self, module: u32, sets: &[Sexp], dir: &Path) -> R<()> {
        for set in sets {
            if let Set::Bindings(bindings) = self.import_set(set, dir)? {
                for (name, binding) in bindings {
                    self.modules[module as usize].imports.insert(name, binding);
                }
            }
        }
        Ok(())
    }

    fn import_set(&mut self, set: &Sexp, dir: &Path) -> R<Set> {
        let items =
            set.list().filter(|l| !l.is_empty()).ok_or_else(|| Error::new(format!("import: bad import set {}", display_sexp(set))))?;
        let head = items[0].sym().map(|s| symbol_name(strip(s)));
        let inner = |vm: &mut Vm| -> R<Vec<(u32, GlobalBinding)>> {
            let set = items.get(1).ok_or_else(|| Error::new(format!("import: bad import set {}", display_sexp(set))))?;
            Ok(match vm.import_set(set, dir)? {
                Set::Root => vm.root_bindings(),
                Set::Bindings(b) => b,
            })
        };
        match head.as_deref() {
            Some("only") if items.len() >= 2 => {
                let keep = syms(&items[2..], "only")?;
                let b = inner(self)?;
                Ok(Set::Bindings(b.into_iter().filter(|(n, _)| keep.contains(n)).collect()))
            }
            Some("except") if items.len() >= 2 => {
                let drop = syms(&items[2..], "except")?;
                let b = inner(self)?;
                Ok(Set::Bindings(b.into_iter().filter(|(n, _)| !drop.contains(n)).collect()))
            }
            Some("prefix") if items.len() == 3 => {
                let prefix = syms(&items[2..], "prefix")?[0];
                let b = inner(self)?;
                let renamed = |n: u32| intern(&format!("{}{}", symbol_name(prefix), symbol_name(n)));
                Ok(Set::Bindings(b.into_iter().map(|(n, g)| (renamed(n), g)).collect()))
            }
            Some("rename") if items.len() >= 2 => {
                let pairs = items[2..]
                    .iter()
                    .map(|p| match p.list().map(|l| syms(l, "rename")) {
                        Some(Ok(v)) if v.len() == 2 => Ok((v[0], v[1])),
                        _ => err("rename: expected (from to) pairs"),
                    })
                    .collect::<R<Vec<_>>>()?;
                let b = inner(self)?;
                let rename = |n: u32| pairs.iter().find(|(from, _)| *from == n).map_or(n, |(_, to)| *to);
                Ok(Set::Bindings(b.into_iter().map(|(n, g)| (rename(n), g)).collect()))
            }
            _ => {
                let parts = name_parts(set).ok_or_else(|| Error::new(format!("import: bad library name {}", display_sexp(set))))?;
                if is_builtin(&parts) {
                    return Ok(Set::Root);
                }
                let m = self.library(&parts, dir)?;
                Ok(Set::Bindings(self.exported(m, &module_name(&parts))?))
            }
        }
    }

    /// Every root binding, for an import set that selects from the root.
    fn root_bindings(&self) -> Vec<(u32, GlobalBinding)> {
        let root = &self.modules[ROOT_MODULE as usize];
        root.defined.iter().filter_map(|&s| self.bindings.get(&(ROOT_MODULE, s)).map(|b| (s, b.clone()))).collect()
    }

    /// The module of library `parts`, loading its `.sld` file if needed.
    fn library(&mut self, parts: &[String], dir: &Path) -> R<u32> {
        let name = module_name(parts);
        if let Some(m) = self.loaded_module(&name) {
            return Ok(m);
        }
        let file = library_file(parts, dir).ok_or_else(|| Error::new(format!("import: no library {name}")))?;
        self.check_loading().map_err(|e| Error::new(format!("import {name}: {}", e.msg)))?;
        self.load_module(&file)?;
        self.loaded_module(&name).ok_or_else(|| Error::new(format!("import: {} does not define {name}", file.display())))
    }

    /// `(define-library name declaration ...)`, from a file in `dir`.
    pub fn define_library(&mut self, form: &[Sexp], file: u32, dir: &Path) -> R<()> {
        let parts = form.get(1).and_then(name_parts).ok_or_else(|| Error::new("define-library: bad library name"))?;
        let name = module_name(&parts);
        if self.loaded_module(&name).is_some() {
            return err(format!("define-library: {name} is already defined"));
        }
        let m = self.new_module(&name, Some(dir.join(format!("{}.sld", parts.join("/")))));
        self.modules[m as usize].exports = Some(Vec::new());
        self.library_declarations(m, &form[2..], file, dir)
    }

    fn library_declarations(&mut self, m: u32, decls: &[Sexp], file: u32, dir: &Path) -> R<()> {
        for decl in decls {
            let items = decl.list().filter(|l| !l.is_empty()).ok_or_else(|| Error::new("define-library: bad declaration"))?;
            let head = items[0].sym().map(|s| symbol_name(strip(s))).unwrap_or_else(|| "".into());
            match &*head {
                "export" => {
                    let specs = items[1..]
                        .iter()
                        .map(|spec| match spec {
                            Sexp::Sym(s) => Ok((strip(*s), strip(*s))),
                            Sexp::List(l, None, _) if l.len() == 3 && l[0].is_sym("rename") => {
                                let v = syms(&l[1..], "export")?;
                                Ok((v[0], v[1]))
                            }
                            _ => err(format!("export: bad export spec {}", display_sexp(spec))),
                        })
                        .collect::<R<Vec<_>>>()?;
                    self.modules[m as usize].exports.get_or_insert_with(Vec::new).extend(specs);
                }
                "import" => self.import(m, &items[1..], dir)?,
                "begin" => {
                    for form in &items[1..] {
                        self.eval_form(m, file, form)?;
                    }
                }
                "include" | "include-ci" => {
                    for (forms, file) in self.include(&items[1..], dir, &*head == "include-ci")? {
                        for form in &forms {
                            self.eval_form(m, file, form)?;
                        }
                    }
                }
                "include-library-declarations" => {
                    for (forms, file) in self.include(&items[1..], dir, false)? {
                        self.library_declarations(m, &forms, file, dir)?;
                    }
                }
                "cond-expand" => {
                    let chosen = self.cond_expand(&items[1..], dir)?;
                    self.library_declarations(m, &chosen, file, dir)?;
                }
                _ => return err(format!("define-library: unknown declaration {}", display_sexp(decl))),
            }
        }
        Ok(())
    }

    /// The forms of each file named by `(include "file" ...)`, relative to
    /// `dir`, with the source file each was read as.
    pub fn include(&mut self, names: &[Sexp], dir: &Path, fold_case: bool) -> R<Vec<(Vec<Sexp>, u32)>> {
        names
            .iter()
            .map(|n| {
                let Sexp::Str(name) = n else { return err("include: expected file names") };
                let path = dir.join(&**name);
                let text = std::fs::read_to_string(&path).map_err(|e| Error::new(format!("include {}: {e}", path.display())))?;
                let source = if fold_case { format!("#!fold-case\n{text}") } else { text.clone() };
                let forms = reader::read_located(&source).map_err(|e| Error::new(format!("{}: {}", path.display(), e.message)))?;
                let file = self.add_file(&path.to_string_lossy(), &text);
                Ok((forms, file))
            })
            .collect()
    }

    /// The body of the first `cond-expand` clause whose requirement holds.
    pub fn cond_expand(&mut self, clauses: &[Sexp], dir: &Path) -> R<Vec<Sexp>> {
        for clause in clauses {
            let items = clause.list().filter(|l| !l.is_empty()).ok_or_else(|| Error::new("cond-expand: bad clause"))?;
            if items[0].is_sym("else") || self.requirement(&items[0], dir)? {
                return Ok(items[1..].to_vec());
            }
        }
        Ok(Vec::new())
    }

    fn requirement(&mut self, req: &Sexp, dir: &Path) -> R<bool> {
        if let Some(s) = req.sym() {
            return Ok(FEATURES.contains(&&*symbol_name(strip(s))));
        }
        let items = req.list().filter(|l| !l.is_empty()).ok_or_else(|| Error::new("cond-expand: bad requirement"))?;
        let all = |vm: &mut Vm, and: bool| -> R<bool> {
            for r in &items[1..] {
                if vm.requirement(r, dir)? != and {
                    return Ok(!and);
                }
            }
            Ok(and)
        };
        match &*items[0].sym().map(|s| symbol_name(strip(s))).unwrap_or_else(|| "".into()) {
            "and" => all(self, true),
            "or" => all(self, false),
            "not" if items.len() == 2 => Ok(!self.requirement(&items[1], dir)?),
            "library" if items.len() == 2 => {
                let parts = name_parts(&items[1]).ok_or_else(|| Error::new("cond-expand: bad library name"))?;
                Ok(is_builtin(&parts) || self.loaded_module(&module_name(&parts)).is_some() || library_file(&parts, dir).is_some())
            }
            _ => err(format!("cond-expand: bad requirement {}", display_sexp(req))),
        }
    }
}

/// Where library `parts` is defined: `a/b.sld` in `dir` or on the library path.
fn library_file(parts: &[String], dir: &Path) -> Option<PathBuf> {
    let rel = format!("{}.sld", parts.join("/"));
    let path_dirs = std::env::var("TECHNE_LIBRARY_PATH").unwrap_or_default();
    std::iter::once(dir.to_path_buf())
        .chain(std::env::split_paths(&path_dirs))
        .map(|d| d.join(&rel))
        .find(|p| p.is_file())
        .and_then(|p| p.canonicalize().ok())
}
