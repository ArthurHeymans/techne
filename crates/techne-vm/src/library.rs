//! R7RS libraries (5.6) over the module system.
//!
//! `(define-library (name ...) declaration ...)` makes a module named by
//! the written library name, e.g. `(srfi 1)`; `export` sets what it
//! provides, `begin`, `include` and `import` fill it. `(import set ...)`
//! adds the bindings of import sets to a module.
//!
//! A library and an environment are isolated modules: they see only what
//! they import, not the root module. The R7RS libraries `(scheme base)` and
//! the like are views of the root module's bindings, `(techne)` is all of
//! them. Any other library is defined where it is first imported from, or
//! else searched for as `a/b.sld` (for `(a b)`) in the importing file's
//! directory, then in the directories of `TECHNE_LIBRARY_PATH`. Ordinary
//! modules (files, the REPL's) also see the root module, imports or not.

use std::path::{Path, PathBuf};

use crate::{
    reader::{self, Sexp, display_sexp, intern, strip, symbol_name},
    vm::{Error, GlobalBinding, ROOT_MODULE, Vm, predeclare},
};

type R<T> = Result<T, Error>;

fn err<T>(msg: impl Into<String>) -> R<T> {
    Err(Error::new(msg))
}

/// The features `cond-expand` and `features` know.
pub const FEATURES: &[&str] = &[
    "r7rs",
    "exact-closed",
    "exact-complex",
    "ratios",
    "complex",
    "full-unicode",
    std::env::consts::OS,
    std::env::consts::ARCH,
    "techne",
    "srfi-69",
];

/// R7RS-small libraries and the identifiers they export (from chibi-scheme's
/// lib/scheme). Identifiers Techne lacks (see runtime/R7RS.md) are skipped
/// on import; syntax such as `define` and `lambda` is visible everywhere.
const STANDARD: &[(&str, &str)] = &[
    ("case-lambda", "case-lambda"),
    ("complex", "angle imag-part magnitude make-polar make-rectangular real-part"),
    (
        "base",
        "* + - ... / < <= = => > >= _ abs and append apply assoc assq assv begin binary-port? \
         boolean? boolean=? bytevector bytevector-append bytevector-copy bytevector-copy! \
         bytevector-length bytevector-u8-ref bytevector-u8-set! bytevector? caar cadr \
         call-with-current-continuation call-with-port call-with-values call/cc car case cdr cdar \
         cddr ceiling char->integer char-ready? char<=? char<? char=? char>=? char>? char? \
         close-input-port close-output-port close-port complex? cond cond-expand cons \
         current-error-port current-input-port current-output-port define define-record-type \
         define-syntax define-values denominator do dynamic-wind else eof-object? eof-object eq? \
         equal? eqv? error error-object-irritants error-object-message error-object? even? exact \
         exact-integer-sqrt exact-integer? exact? expt features file-error? floor flush-output-port \
         for-each gcd get-output-bytevector get-output-string guard if include include-ci inexact \
         inexact? input-port? integer->char integer? lambda lcm length let let* let*-values \
         let-syntax let-values letrec letrec* letrec-syntax list list->string list->vector \
         list-copy list-ref list-set! list-tail list? make-bytevector make-list make-parameter \
         make-string make-vector map max member memq memv min modulo negative? newline not null? \
         number->string number? numerator odd? open-input-bytevector open-input-string \
         open-output-bytevector open-output-string or output-port? pair? parameterize peek-char \
         peek-u8 input-port-open? output-port-open? port? positive? procedure? quasiquote quote \
         quotient raise raise-continuable rational? rationalize read-bytevector read-bytevector! \
         read-char read-error? read-line read-string read-u8 real? remainder reverse round set! \
         set-car! set-cdr! square string string->list string->number string->symbol string->utf8 \
         string->vector string-append string-copy string-copy! string-fill! string-for-each \
         string-length string-map string-ref string-set! string<=? string<? string=? string>=? \
         string>? string? substring symbol->string symbol? symbol=? syntax-error syntax-rules \
         textual-port? truncate u8-ready? unless unquote unquote-splicing utf8->string values \
         vector vector-append vector->list vector->string vector-copy vector-copy! vector-fill! \
         vector-for-each vector-length vector-map vector-ref vector-set! vector? when \
         with-exception-handler write-bytevector write-char write-string write-u8 zero? \
         truncate-quotient truncate-remainder truncate/ floor-quotient floor-remainder floor/",
    ),
    (
        "char",
        "char-alphabetic? char-ci<=? char-ci<? char-ci=? char-ci>=? char-ci>? char-downcase \
         char-foldcase char-lower-case? char-numeric? char-upcase char-upper-case? char-whitespace? \
         digit-value string-ci<=? string-ci<? string-ci=? string-ci>=? string-ci>? string-downcase \
         string-foldcase string-upcase",
    ),
    (
        "cxr",
        "caaar caadr cadar caddr cdaar cdadr cddar cdddr caaaar caaadr caadar caaddr cadaar cadadr \
         caddar cadddr cdaaar cdaadr cdadar cdaddr cddaar cddadr cdddar cddddr",
    ),
    ("eval", "eval environment"),
    (
        "file",
        "call-with-input-file call-with-output-file delete-file file-exists? open-binary-input-file \
         open-binary-output-file open-input-file open-output-file with-input-from-file \
         with-output-to-file",
    ),
    ("inexact", "acos asin atan cos exp finite? infinite? log nan? sin sqrt tan"),
    ("lazy", "delay force delay-force make-promise promise?"),
    ("process-context", "get-environment-variable get-environment-variables command-line exit emergency-exit"),
    ("read", "read"),
    ("repl", "interaction-environment"),
    ("time", "current-second current-jiffy jiffies-per-second"),
    ("write", "display write write-shared write-simple"),
    (
        "r5rs",
        "- ... * / + < <= = => > >= _ abs acos and angle append apply asin assoc assq assv atan \
         begin boolean? caaaar caaadr caadar caaddr cadaar cadadr caddar cadddr cdaaar cdaadr \
         cdadar cdaddr cddaar cddadr cdddar cddddr caaar caadr cadar caddr cdaar cdadr cddar cdddr \
         caar cadr cdar cddr call-with-current-continuation call-with-input-file \
         call-with-output-file call-with-values car case cdr ceiling char->integer char-alphabetic? \
         char-ci<? char-ci<=? char-ci=? char-ci>? char-ci>=? char-downcase char-lower-case? \
         char-numeric? char-ready? char-upcase char-upper-case? char-whitespace? char? char<? \
         char<=? char=? char>? char>=? close-input-port close-output-port complex? cond cons cos \
         current-input-port current-output-port define define-syntax delay denominator display do \
         dynamic-wind else eof-object? eq? equal? eqv? eval even? exact->inexact exact? exp expt \
         floor for-each force gcd if imag-part inexact->exact inexact? input-port? integer->char \
         integer? interaction-environment lambda lcm length let let-syntax let* letrec \
         letrec-syntax list list->string list->vector list-ref list-tail list? load log magnitude \
         make-polar make-rectangular make-string make-vector map max member memq memv min modulo \
         negative? newline not null-environment null? number->string number? numerator odd? \
         open-input-file open-output-file or output-port? pair? peek-char positive? procedure? \
         quasiquote quote quotient rational? rationalize read read-char real-part real? remainder \
         reverse round scheme-report-environment set-car! set-cdr! set! sin sqrt string \
         string->list string->number string->symbol string-append string-ci<? string-ci<=? \
         string-ci=? string-ci>? string-ci>=? string-copy string-fill! string-length string-ref \
         string-set! string? string<? string<=? string=? string>? string>=? substring \
         symbol->string symbol? syntax-rules tan truncate values vector vector->list vector-fill! \
         vector-length vector-ref vector-set! vector? with-input-from-file with-output-to-file \
         write write-char zero?",
    ),
];

/// SRFI libraries Techne provides itself, as views of the root module like
/// the R7RS ones, and the identifiers they export.
const SRFIS: &[(&str, &str)] = &[(
    "69",
    "make-hash-table hash-table? alist->hash-table hash-table-equivalence-function \
     hash-table-hash-function hash-table-ref hash-table-ref/default hash-table-set! \
     hash-table-delete! hash-table-exists? hash-table-update! hash-table-update!/default \
     hash-table-size hash-table-keys hash-table-values hash-table-walk hash-table-fold \
     hash-table->alist hash-table-copy hash-table-merge! hash string-hash string-ci-hash \
     hash-by-identity",
)];

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

/// The identifiers a built-in library exports: an R7RS or SRFI library, or
/// every root binding for `(techne)`.
enum Builtin {
    Standard(&'static str),
    Techne,
}

fn builtin(parts: &[String]) -> Option<Builtin> {
    match parts {
        [t] if t == "techne" => Some(Builtin::Techne),
        [s, name] if s == "scheme" => STANDARD.iter().find(|(n, _)| n == name).map(|(_, ids)| Builtin::Standard(ids)),
        [s, n] if s == "srfi" => SRFIS.iter().find(|(m, _)| m == n).map(|(_, ids)| Builtin::Standard(ids)),
        _ => None,
    }
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
            for (name, binding) in self.import_set(set, dir)? {
                self.modules[module as usize].imports.insert(name, binding);
            }
        }
        Ok(())
    }

    /// A fresh environment (R7RS `environment`): an isolated module that
    /// sees what `sets` import.
    pub fn environment(&mut self, sets: &[Sexp]) -> R<u32> {
        let m = self.new_module(&format!("#<environment {}>", self.modules.len()), None);
        self.modules[m as usize].isolated = true;
        self.import(m, sets, Path::new("."))?;
        Ok(m)
    }

    fn import_set(&mut self, set: &Sexp, dir: &Path) -> R<Vec<(u32, GlobalBinding)>> {
        let items =
            set.list().filter(|l| !l.is_empty()).ok_or_else(|| Error::new(format!("import: bad import set {}", display_sexp(set))))?;
        let head = items[0].sym().map(|s| symbol_name(strip(s)));
        let inner = |vm: &mut Vm| -> R<Vec<(u32, GlobalBinding)>> {
            let set = items.get(1).ok_or_else(|| Error::new(format!("import: bad import set {}", display_sexp(set))))?;
            vm.import_set(set, dir)
        };
        match head.as_deref() {
            Some("only") if items.len() >= 2 => {
                let keep = syms(&items[2..], "only")?;
                let b = inner(self)?;
                Ok(b.into_iter().filter(|(n, _)| keep.contains(n)).collect())
            }
            Some("except") if items.len() >= 2 => {
                let drop = syms(&items[2..], "except")?;
                let b = inner(self)?;
                Ok(b.into_iter().filter(|(n, _)| !drop.contains(n)).collect())
            }
            Some("prefix") if items.len() == 3 => {
                let prefix = syms(&items[2..], "prefix")?[0];
                let b = inner(self)?;
                let renamed = |n: u32| intern(&format!("{}{}", symbol_name(prefix), symbol_name(n)));
                Ok(b.into_iter().map(|(n, g)| (renamed(n), g)).collect())
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
                Ok(b.into_iter().map(|(n, g)| (rename(n), g)).collect())
            }
            _ => {
                let parts = name_parts(set).ok_or_else(|| Error::new(format!("import: bad library name {}", display_sexp(set))))?;
                match builtin(&parts) {
                    Some(Builtin::Techne) => Ok(self.root_bindings()),
                    Some(Builtin::Standard(ids)) => Ok(ids
                        .split_whitespace()
                        .filter_map(|id| {
                            let sym = intern(id);
                            self.bindings.get(&(ROOT_MODULE, sym)).map(|b| (sym, b.clone()))
                        })
                        .collect()),
                    None => {
                        let m = self.library(&parts, dir)?;
                        self.exported(m, &module_name(&parts))
                    }
                }
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
        self.modules[m as usize].isolated = true;
        // Imports and exports first; then the body, every definition known
        // before any of it compiles.
        let mut body = Vec::new();
        self.library_declarations(m, &form[2..], file, dir, &mut body)?;
        for (form, _) in &body {
            predeclare(self, m, form);
        }
        for (form, file) in &body {
            self.eval_form(m, *file, form)?;
        }
        Ok(())
    }

    /// Carries out a library's imports and exports, and collects its body
    /// forms with the source file of each.
    fn library_declarations(&mut self, m: u32, decls: &[Sexp], file: u32, dir: &Path, body: &mut Vec<(Sexp, u32)>) -> R<()> {
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
                "begin" => body.extend(items[1..].iter().map(|f| (f.clone(), file))),
                "include" | "include-ci" => {
                    for (forms, file) in self.include(&items[1..], dir, &*head == "include-ci")? {
                        body.extend(forms.into_iter().map(|f| (f, file)));
                    }
                }
                "include-library-declarations" => {
                    for (forms, file) in self.include(&items[1..], dir, false)? {
                        self.library_declarations(m, &forms, file, dir, body)?;
                    }
                }
                "cond-expand" => {
                    let chosen = self.cond_expand(&items[1..], dir)?;
                    self.library_declarations(m, &chosen, file, dir, body)?;
                }
                _ => return err(format!("define-library: unknown declaration {}", display_sexp(decl))),
            }
        }
        Ok(())
    }

    /// The forms of each file named by `(include "file" ...)`, relative to
    /// `dir`, with the source file each was read as.
    pub fn include(&mut self, names: &[Sexp], dir: &Path, fold_case: bool) -> R<Vec<(Vec<Sexp>, u32)>> {
        self.check_loading().map_err(|e| Error::new(format!("include: {}", e.msg)))?;
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
                Ok(builtin(&parts).is_some() || self.loaded_module(&module_name(&parts)).is_some() || library_file(&parts, dir).is_some())
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
