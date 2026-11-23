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
//! else searched for as `a/b.sld` (for `(a b)`): under the directory that
//! provides the libraries named `(a b ...)` and nowhere else, if a package
//! does (`Vm::provide_libraries`); otherwise in the importing file's
//! directory, then in the directories of `TECHNE_LIBRARY_PATH`, then in
//! Techne's own Lisp ([`lisp_dir`]). Ordinary modules
//! (files, the REPL's) also see the root module, imports or not.

use std::{
    path::{Path, PathBuf},
    sync::LazyLock,
};

pub use crate::bundle::PackagePrefix;
use crate::{
    reader::{self, Sexp, display_sexp, intern, strip, symbol_name},
    vm::{CallerPackage, Error, Forms, GlobalBinding, ROOT_MODULE, SourceProvenance, Vm, predeclare},
};

type R<T> = Result<T, Error>;

/// What the files of a package may import (`Vm::declare_imports`).
#[derive(Clone)]
pub(crate) struct PackageImports {
    /// The package's directory.
    pub(crate) root: PathBuf,
    package: PackagePrefix,
    /// The packages whose libraries its files import: itself and those
    /// its manifest depends on.
    names: Vec<PackagePrefix>,
}

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
    "srfi-27",
    "srfi-69",
    "regexp-non-greedy",
    "regexp-unicode",
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

/// Techne's own Lisp: its libraries, such as `(srfi 130)`, and the
/// editor's (`editor/`). It is `TECHNE_LISP_DIR` if set and not empty; else
/// `../share/techne/lisp` from the directory of the running program,
/// where an installation puts it; else the source tree's `lisp`
/// directory, for a program built and run there. Where neither is, the
/// installed place, which errors then name: not the build machine's.
pub fn lisp_dir() -> &'static Path {
    static DIR: LazyLock<PathBuf> = LazyLock::new(|| {
        let installed = || {
            let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
            Some(exe.parent()?.join("../share/techne/lisp")).filter(|d| d.is_dir())
        };
        let installed_place = || Some(std::env::current_exe().ok()?.parent()?.join("../share/techne/lisp"));
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lisp");
        std::env::var_os("TECHNE_LISP_DIR")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(installed)
            .or_else(|| source.is_dir().then_some(source))
            .or_else(installed_place)
            .unwrap_or_else(|| PathBuf::from("share/techne/lisp"))
    });
    &DIR
}

/// Whether `s` is a valid ASCII package owner or package name component:
/// starts with ASCII `[a-z0-9]` and contains only `[a-z0-9._-]`.
pub(crate) fn is_valid_package_component(s: &str) -> bool {
    let bytes = s.as_bytes();
    !bytes.is_empty()
        && matches!(bytes[0], b'a'..=b'z' | b'0'..=b'9')
        && bytes.iter().all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.'))
}

/// Whether `part` is safe as a single path segment in library lookup.
pub(crate) fn valid_library_part(part: &str) -> bool {
    !part.is_empty() && !part.starts_with('.') && !part.bytes().any(|b| matches!(b, b'/' | b'\\' | b'\0'))
}

/// Whether `(owner pkg)` collides with Techne's own shipped stdlib libraries.
pub(crate) fn is_stdlib_package(owner: &str, pkg: &str) -> bool {
    if owner != "techne" {
        return false;
    }
    if pkg == "editor" {
        return true;
    }
    if valid_library_part(pkg) {
        let stdlib = lisp_dir().join("techne");
        if stdlib.join(format!("{pkg}.sld")).is_file() || stdlib.join(pkg).is_dir() {
            return true;
        }
    }
    false
}

/// Whether `path` resolves inside Techne's own `lisp_dir()`.
pub(crate) fn is_under_lisp_dir(path: &Path) -> bool {
    let Ok(root) = lisp_dir().canonicalize() else { return false };
    path.canonicalize().is_ok_and(|p| p.starts_with(&root))
}

/// Resolve `parts` to `<root>/<p0>/.../<pn>.sld`, validating every segment and
/// verifying canonical containment inside `root`.
fn contained_library_file(root: &Path, parts: &[String]) -> Option<PathBuf> {
    if parts.is_empty() || !parts.iter().all(|p| valid_library_part(p)) {
        return None;
    }
    let root = if root.as_os_str().is_empty() { Path::new(".") } else { root };
    let root = root.canonicalize().ok()?;
    let mut candidate = root.clone();
    for part in &parts[..parts.len() - 1] {
        candidate.push(part);
    }
    candidate.push(format!("{}.sld", parts.last()?));
    if !candidate.is_file() {
        return None;
    }
    let canonical = candidate.canonicalize().ok()?;
    canonical.starts_with(&root).then_some(canonical)
}

/// Whether library `parts` belongs to Techne's own standard or built-in libraries.
pub(crate) fn is_stdlib_library(parts: &[String]) -> bool {
    if builtin(parts).is_some() {
        return true;
    }
    if contained_library_file(lisp_dir(), parts).is_some() {
        return true;
    }
    if let [owner, pkg, ..] = parts
        && is_stdlib_package(owner, pkg)
    {
        return true;
    }
    false
}

/// Resolve an `(include name)` path inside `root`, rejecting absolute paths,
/// dot-prefixed hidden components, and paths escaping `root`.
fn resolve_contained_include(dir: &Path, name: &str, root: &Path) -> R<PathBuf> {
    let rel = Path::new(name);
    if rel.is_absolute() {
        return err(format!("include: absolute path not allowed: {name}"));
    }
    for c in rel.components() {
        if let std::path::Component::Normal(part) = c
            && part.to_string_lossy().starts_with('.')
        {
            return err(format!("include: hidden path not allowed: {name}"));
        }
    }
    let candidate = dir.join(rel);
    let canonical = candidate.canonicalize().map_err(|e| Error::new(format!("include: {}: {e}", candidate.display())))?;
    if !canonical.starts_with(root) {
        return err(format!("include: path escapes package directory: {name}"));
    }
    if let Ok(inside) = canonical.strip_prefix(root) {
        for c in inside.components() {
            if let std::path::Component::Normal(part) = c
                && part.to_string_lossy().starts_with('.')
            {
                return err(format!("include: hidden path not allowed: {name}"));
            }
        }
    }
    Ok(canonical)
}

/// A library name's parts: identifiers and exact integers.
pub(crate) fn name_parts(name: &Sexp) -> Option<Vec<String>> {
    let list = name.list()?;
    if list.is_empty() {
        return None;
    }
    list.iter()
        .map(|p| match p {
            Sexp::Sym(s) => Some(symbol_name(strip(*s)).to_string()),
            Sexp::Int(i) if *i >= 0 => Some(i.to_string()),
            _ => None,
        })
        .collect()
}

/// The identifiers a built-in library exports: an R7RS or SRFI library, or
/// every root binding for `(techne)`.
pub(crate) enum Builtin {
    Standard(&'static str),
    Techne,
}

pub(crate) fn builtin(parts: &[String]) -> Option<Builtin> {
    match parts {
        [t] if t == "techne" => Some(Builtin::Techne),
        [s, name] if s == "scheme" => STANDARD.iter().find(|(n, _)| n == name).map(|(_, ids)| Builtin::Standard(ids)),
        [s, n] if s == "srfi" => SRFIS.iter().find(|(m, _)| m == n).map(|(_, ids)| Builtin::Standard(ids)),
        _ => None,
    }
}

pub(crate) fn module_name(parts: &[String]) -> String {
    format!("({})", parts.join(" "))
}

pub(crate) fn syms(items: &[Sexp], who: &str) -> R<Vec<u32>> {
    items.iter().map(|i| i.sym().map(strip).ok_or_else(|| Error::new(format!("{who}: expected identifiers")))).collect()
}

impl Vm {
    /// Resolve the package/caller attribution for imports or `cond-expand`
    /// evaluated in `(module, file)`: prefer the file's trusted provenance,
    /// then the module's attributed package; in bundle/locked mode also derive
    /// the caller from the active VM call stack so dynamic `(environment ...)`
    /// and non-file `eval-source` fail closed under the calling package.
    fn resolve_caller_package(&self, module: u32, file: Option<u32>) -> Option<CallerPackage> {
        if let Some(f) = file
            && let Some(caller) = self.caller_from_provenance(&self.files[f as usize].provenance)
        {
            return Some(caller);
        }
        let restricted = self.bundle.is_some() || self.locked_libraries;
        if !restricted {
            if file.is_some() {
                return None;
            }
            return self.modules.get(module as usize).and_then(|m| m.caller_package.clone());
        }
        if let Some(m) = self.modules.get(module as usize) {
            if m.isolated {
                return m.caller_package.clone();
            }
            if let Some(active) = self.active_caller_package() {
                return Some(active);
            }
            if let Some(caller) = &m.caller_package {
                return Some(caller.clone());
            }
        }
        self.active_caller_package()
    }

    /// `(import set ...)` into `module`; libraries are looked up from `dir`.
    pub fn import(&mut self, module: u32, sets: &[Sexp], dir: &Path) -> R<()> {
        self.import_for(module, None, sets, dir)
    }

    pub(crate) fn import_for(&mut self, module: u32, file: Option<u32>, sets: &[Sexp], dir: &Path) -> R<()> {
        let caller = self.resolve_caller_package(module, file);
        for set in sets {
            let bindings = self.import_set(set, caller.as_ref(), dir)?;
            self.add_imports(module, bindings);
        }
        Ok(())
    }

    /// A fresh environment (R7RS `environment`): an isolated module that
    /// sees what `sets` import. In bundle/locked mode it inherits the active
    /// caller's package attribution so dynamic imports cannot bypass
    /// direct-dependency rules.
    pub fn environment(&mut self, sets: &[Sexp]) -> R<u32> {
        let caller = (self.bundle.is_some() || self.locked_libraries).then(|| self.active_caller_package()).flatten();
        let m = self.new_module(&format!("#<environment {}>", self.modules.len()), None);
        self.modules[m as usize].isolated = true;
        self.modules[m as usize].caller_package = caller;
        let dir = if self.bundle.as_ref().is_some_and(|b| b.root_package().is_none()) { Path::new("/bundle") } else { Path::new("") };
        // This host/compiler fallback still nests if an environment loads
        // a library. Unlike top-level import, it has no forms loop to run
        // dependencies before returning the environment's module id.
        self.import_for(m, None, sets, dir)?;
        Ok(m)
    }

    fn import_set(&mut self, set: &Sexp, caller: Option<&CallerPackage>, dir: &Path) -> R<Vec<(u32, GlobalBinding)>> {
        crate::nested(|| self.import_set_step(set, caller, dir))
    }

    fn import_set_step(&mut self, set: &Sexp, caller: Option<&CallerPackage>, dir: &Path) -> R<Vec<(u32, GlobalBinding)>> {
        let items =
            set.list().filter(|l| !l.is_empty()).ok_or_else(|| Error::new(format!("import: bad import set {}", display_sexp(set))))?;
        let head = items[0].sym().map(|s| symbol_name(strip(s)));
        let inner = |vm: &mut Vm| -> R<Vec<(u32, GlobalBinding)>> {
            let set = items.get(1).ok_or_else(|| Error::new(format!("import: bad import set {}", display_sexp(set))))?;
            vm.import_set(set, caller, dir)
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
                        let m = self.library(&parts, caller, dir)?;
                        self.exported(m, &module_name(&parts))
                    }
                }
            }
        }
    }

    /// Every root binding, for an import set that selects from the root:
    /// natives as well as what the prelude defines.
    fn root_bindings(&self) -> Vec<(u32, GlobalBinding)> {
        self.bindings.iter().filter(|((m, _), _)| *m == ROOT_MODULE).map(|(&(_, s), b)| (s, b.clone())).collect()
    }

    /// Verify that code with loader-issued `provenance` may define or claim library `parts`.
    pub(crate) fn check_library_ownership(&self, parts: &[String], provenance: &SourceProvenance, who: &str) -> R<()> {
        if parts.is_empty() {
            return err(format!("{who}: bad library name"));
        }
        let name = module_name(parts);
        if is_stdlib_library(parts) {
            if matches!(provenance, SourceProvenance::Stdlib(_)) {
                return Ok(());
            }
            return err(format!("{who}: {name} belongs to Techne's own libraries"));
        }
        if let Some(bundle) = &self.bundle {
            if let Some([owner, pkg]) = parts.get(..2)
                && bundle.has_package(owner, pkg)
            {
                let owned = matches!(
                    provenance,
                    SourceProvenance::BundlePackage { prefix, path, .. }
                        if prefix.matches(owner, pkg) && bundle.owns_library_source(owner, pkg, Some(path.as_path()))
                );
                if !owned {
                    return err(format!("{who}: {name} is owned by package ({owner} {pkg})"));
                }
                return Ok(());
            }
            if bundle.root_package().is_none() {
                if matches!(parts.first().map(String::as_str), Some("scheme" | "srfi")) {
                    return err(format!("{who}: namespace ({} ...) is reserved", parts[0]));
                }
                if matches!(provenance, SourceProvenance::BundleRoot { .. }) {
                    return Ok(());
                }
            }
            return err(format!("{who}: {name} is not provided in locked mode"));
        }
        if let Some(provider) = parts.get(..2).and_then(|p| self.library_provider(&p[0], &p[1])) {
            let owned = matches!(
                provenance,
                SourceProvenance::HostFile(p) | SourceProvenance::Stdlib(p) if p.starts_with(provider)
            );
            if !owned {
                return err(format!("{who}: {name} is owned by package ({} {})", parts[0], parts[1]));
            }
            return Ok(());
        }
        if self.locked_libraries {
            return err(format!("{who}: {name} is not provided in locked mode"));
        }
        Ok(())
    }

    /// The module of library `parts`, loading its `.sld` file if needed.
    fn library(&mut self, parts: &[String], caller: Option<&CallerPackage>, dir: &Path) -> R<u32> {
        let name = module_name(parts);
        self.check_declared(parts, caller, dir)?;
        if let Some(m) = self.loaded_library_parts(parts)? {
            let prov = self.modules[m as usize].provenance.clone();
            self.check_library_ownership(parts, &prov, "import")?;
            return Ok(m);
        }
        let file = self.library_file(parts, dir).ok_or_else(|| Error::new(format!("import: no library {name}")))?;
        self.check_loading().map_err(|e| Error::new(format!("import {name}: {}", e.msg)))?;
        let result = self.load_module(&file).and_then(|_| {
            let m = self
                .loaded_library_parts(parts)?
                .ok_or_else(|| Error::new(format!("import: {} does not define {name}", file.display())))?;
            let prov = self.modules[m as usize].provenance.clone();
            self.check_library_ownership(parts, &prov, "import")?;
            Ok(m)
        });
        if result.is_err() {
            self.abort_library_file(&file);
        }
        result
    }

    /// Make a pending registration before processing declarations, so cycles
    /// are rejected on every import path. No importer may use it yet.
    fn new_library(&mut self, form: &[Sexp], owner: u32, file: u32) -> R<u32> {
        let parts = form.get(1).and_then(name_parts).ok_or_else(|| Error::new("define-library: bad library name"))?;
        let provenance = self.source_provenance(file);
        self.check_library_ownership(&parts, &provenance, "define-library")?;
        let name = module_name(&parts);
        if self.loaded_library_parts(&parts)?.is_some() {
            return err(format!("define-library: {name} is already defined"));
        }
        let source_path = Vm::provenance_path(&provenance);
        let caller = self
            .caller_from_provenance(&provenance)
            .or_else(|| self.modules.get(owner as usize).and_then(|m| m.caller_package.clone()))
            .or_else(|| (self.bundle.is_some() || self.locked_libraries).then(|| self.active_caller_package()).flatten());
        let m = self.new_module(&name, source_path);
        self.modules[m as usize].provenance = provenance;
        self.modules[m as usize].caller_package = caller;
        self.modules[m as usize].library_parts = Some(parts.into_iter().map(Into::into).collect());
        self.modules[m as usize].exports = Some(Vec::new());
        self.recharge_module(m);
        self.modules[m as usize].isolated = true;
        self.begin_library_load(m, owner);
        Ok(m)
    }

    /// `(define-library name declaration ...)`, from a file in `dir`.
    pub fn define_library(&mut self, form: &[Sexp], owner: u32, file: u32, dir: &Path) -> R<()> {
        let m = self.new_library(form, owner, file)?;
        // A compiler/host fallback: it must return with the library loaded
        // before the enclosing form can compile. The Scheme forms loop uses
        // library_forms instead, so its bodies can wait without Rust nesting.
        let result = (|| {
            let mut imports = Vec::new();
            let mut body = Vec::new();
            self.library_declarations(m, &form[2..], file, dir, &mut imports, &mut body)?;
            self.record_library_snapshot(m, &imports, &body);
            for (form, file) in imports {
                let dir = self.import_dir(file);
                self.import_for(m, Some(file), &form.list().unwrap()[1..], &dir)?;
            }
            for (form, _) in &body {
                predeclare(self, m, form);
            }
            for (form, file) in &body {
                self.eval_form(m, *file, form)?;
            }
            self.exported(m, &self.modules[m as usize].name)?;
            Ok(())
        })();
        match result {
            Ok(()) => self.end_module_load(m),
            Err(_) => self.abort_library(m),
        }
        result
    }

    /// Register a library and queue its imports before its body. Nothing
    /// here runs Scheme; the caller evaluates the forms on its VM stack.
    pub(crate) fn library_forms(&mut self, form: &[Sexp], owner: u32, file: u32, dir: &Path) -> R<Forms> {
        let m = self.new_library(form, owner, file)?;
        let mut imports = Vec::new();
        let mut body = Vec::new();
        if let Err(e) = self.library_declarations(m, &form[2..], file, dir, &mut imports, &mut body) {
            self.abort_library(m);
            return Err(e);
        }
        self.record_library_snapshot(m, &imports, &body);
        imports.extend(body);
        Ok(self.queued_forms(m, file, imports, true))
    }

    /// The next not-yet-loaded library selected by import sets, looking
    /// through modifiers without applying their binding transformations.
    pub(crate) fn next_imported(&mut self, module: u32, file: Option<u32>, sets: &[Sexp], dir: &Path) -> R<Option<Forms>> {
        let caller = self.resolve_caller_package(module, file);
        self.next_imported_with_caller(caller.as_ref(), sets, dir)
    }

    fn next_imported_with_caller(&mut self, caller: Option<&CallerPackage>, sets: &[Sexp], dir: &Path) -> R<Option<Forms>> {
        for set in sets {
            let items =
                set.list().filter(|l| !l.is_empty()).ok_or_else(|| Error::new(format!("import: bad import set {}", display_sexp(set))))?;
            if items[0].sym().is_some_and(|s| matches!(&*symbol_name(strip(s)), "only" | "except" | "prefix" | "rename")) {
                let inner = items.get(1).ok_or_else(|| Error::new("import: missing import set"))?;
                if let Some(forms) = crate::nested(|| self.next_imported_with_caller(caller, std::slice::from_ref(inner), dir))? {
                    return Ok(Some(forms));
                }
                continue;
            }
            let parts = name_parts(set).ok_or_else(|| Error::new(format!("import: bad library name {}", display_sexp(set))))?;
            if builtin(&parts).is_some() {
                continue;
            }
            let name = module_name(&parts);
            // Before it loads: one refused is not run either.
            self.check_declared(&parts, caller, dir)?;
            if let Some(m) = self.loaded_library_parts(&parts)? {
                let prov = self.modules[m as usize].provenance.clone();
                self.check_library_ownership(&parts, &prov, "import")?;
                self.exported(m, &name)?;
                continue;
            }
            self.check_loading()?;
            let path = self.library_file(&parts, dir).ok_or_else(|| Error::new(format!("import: no library {name}")))?;
            let text = self.read_file_admitted(&path.to_string_lossy(), 2)?;
            match self.load_module_forms(&path, text) {
                Ok(Some(forms)) => return Ok(Some(forms)),
                Ok(None) => {
                    self.abort_library_file(&path);
                    return err(format!("import: {} does not define {name}", path.display()));
                }
                Err(e) => {
                    self.abort_library_file(&path);
                    return Err(e);
                }
            }
        }
        Ok(None)
    }

    /// Collect declarations without running their imports or body forms.
    fn library_declarations(
        &mut self,
        m: u32,
        decls: &[Sexp],
        file: u32,
        dir: &Path,
        imports: &mut Vec<(Sexp, u32)>,
        body: &mut Vec<(Sexp, u32)>,
    ) -> R<()> {
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
                    self.recharge_module(m);
                }
                "import" => imports.push((decl.clone(), file)),
                "begin" => body.extend(items[1..].iter().map(|f| (f.clone(), file))),
                "include" | "include-ci" => {
                    for (forms, file) in self.include(&items[1..], dir, &*head == "include-ci")? {
                        body.extend(forms.into_iter().map(|f| (f, file)));
                    }
                }
                "include-library-declarations" => {
                    for (forms, included_file) in self.include(&items[1..], dir, false)? {
                        let child_dir = self.form_source_dir(included_file);
                        self.library_declarations(m, &forms, included_file, &child_dir, imports, body)?;
                    }
                }
                "cond-expand" => {
                    let chosen = self.cond_expand_for(m, Some(file), &items[1..], dir)?;
                    self.library_declarations(m, &chosen, file, dir, imports, body)?;
                }
                _ => return err(format!("define-library: unknown declaration {}", display_sexp(decl))),
            }
        }
        Ok(())
    }

    /// The package/locked containment root for `include` from `dir`, if any.
    fn include_containment_root(&self, dir: &Path) -> Option<PathBuf> {
        let effective = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
        let canon_dir = effective.canonicalize().ok();
        if let Some(canon_dir) = &canon_dir {
            if let Some(p) = self.package_imports.iter().filter(|p| canon_dir.starts_with(&p.root)).max_by_key(|p| p.root.as_os_str().len())
            {
                return Some(p.root.clone());
            }
            if let Some((_, provider)) =
                self.library_providers.iter().filter(|(_, p)| canon_dir.starts_with(p)).max_by_key(|(_, p)| p.as_os_str().len())
            {
                return Some(provider.clone());
            }
        }
        if self.locked_libraries {
            if let Some(canon_dir) = canon_dir {
                if let Ok(lisp) = lisp_dir().canonicalize()
                    && canon_dir.starts_with(&lisp)
                {
                    return Some(lisp);
                }
                return Some(canon_dir);
            }
            return Path::new(".").canonicalize().ok();
        }
        None
    }

    /// The forms of each file named by `(include "file" ...)`, relative to
    /// `dir`, with the source file each was read as.
    pub fn include(&mut self, names: &[Sexp], dir: &Path, fold_case: bool) -> R<Vec<(Vec<Sexp>, u32)>> {
        self.check_loading().map_err(|e| Error::new(format!("include: {}", e.msg)))?;
        if let Some(bundle) = self.bundle.clone() {
            return names
                .iter()
                .map(|n| {
                    let Sexp::Str(name) = n else { return err("include: expected file names") };
                    let path = if bundle.is_virtual_dir_path(dir) {
                        bundle.resolve_relative_from_dir(dir, name, "include")?
                    } else if is_under_lisp_dir(dir) {
                        let lisp_root = lisp_dir().canonicalize().map_err(|e| Error::new(format!("include: {e}")))?;
                        resolve_contained_include(dir, name, &lisp_root)?
                    } else {
                        return err(format!("include: cannot resolve {name} outside bundle"));
                    };
                    let text =
                        self.read_file_admitted(&path.to_string_lossy(), 2).map_err(|e| Error::new(format!("include: {}", e.msg)))?;
                    let meter = crate::vm::meter::Meter::new(self);
                    let forms = reader::read_metered(&text, fold_case, &meter)
                        .map_err(|e| meter.error(self).unwrap_or_else(|| Error::new(format!("{}: {}", path.display(), e.message))))?;
                    let provenance = self.derive_provenance(&path);
                    let file = self.add_file_with_provenance(&path.to_string_lossy(), &text, provenance)?;
                    Ok((forms, file))
                })
                .collect();
        }
        let containment_root = self.include_containment_root(dir);
        let effective_dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
        names
            .iter()
            .map(|n| {
                let Sexp::Str(name) = n else { return err("include: expected file names") };
                let path = match &containment_root {
                    Some(root) => resolve_contained_include(effective_dir, name, root)?,
                    None => dir.join(&**name),
                };
                // The text, and the copy kept as the file's source.
                let text = self.read_file_admitted(&path.to_string_lossy(), 2).map_err(|e| Error::new(format!("include: {}", e.msg)))?;
                let meter = crate::vm::meter::Meter::new(self);
                let forms = reader::read_metered(&text, fold_case, &meter)
                    .map_err(|e| meter.error(self).unwrap_or_else(|| Error::new(format!("{}: {}", path.display(), e.message))))?;
                let provenance = self.derive_provenance(&path);
                let file = self.add_file_with_provenance(&path.to_string_lossy(), &text, provenance)?;
                Ok((forms, file))
            })
            .collect()
    }

    /// The body of the first `cond-expand` clause whose requirement holds.
    pub fn cond_expand(&mut self, clauses: &[Sexp], dir: &Path) -> R<Vec<Sexp>> {
        self.cond_expand_for(self.current_module(), None, clauses, dir)
    }

    pub(crate) fn cond_expand_for(&mut self, module: u32, file: Option<u32>, clauses: &[Sexp], dir: &Path) -> R<Vec<Sexp>> {
        let caller = self.resolve_caller_package(module, file);
        for clause in clauses {
            let items = clause.list().filter(|l| !l.is_empty()).ok_or_else(|| Error::new("cond-expand: bad clause"))?;
            if items[0].is_sym("else") || self.requirement(&items[0], caller.as_ref(), dir)? {
                return Ok(items[1..].to_vec());
            }
        }
        Ok(Vec::new())
    }

    fn requirement(&mut self, req: &Sexp, caller: Option<&CallerPackage>, dir: &Path) -> R<bool> {
        crate::nested(|| self.requirement_step(req, caller, dir))
    }

    fn requirement_step(&mut self, req: &Sexp, caller: Option<&CallerPackage>, dir: &Path) -> R<bool> {
        if let Some(s) = req.sym() {
            return Ok(FEATURES.contains(&&*symbol_name(strip(s))));
        }
        let items = req.list().filter(|l| !l.is_empty()).ok_or_else(|| Error::new("cond-expand: bad requirement"))?;
        let all = |vm: &mut Vm, and: bool| -> R<bool> {
            for r in &items[1..] {
                if vm.requirement(r, caller, dir)? != and {
                    return Ok(!and);
                }
            }
            Ok(and)
        };
        match &*items[0].sym().map(|s| symbol_name(strip(s))).unwrap_or_else(|| "".into()) {
            "and" => all(self, true),
            "or" => all(self, false),
            "not" if items.len() == 2 => Ok(!self.requirement(&items[1], caller, dir)?),
            "library" if items.len() == 2 => {
                let parts = name_parts(&items[1]).ok_or_else(|| Error::new("cond-expand: bad library name"))?;
                // File discovery still establishes availability; a dead
                // inline registration does not, even before any import.
                self.abandon_dead_libraries();
                if builtin(&parts).is_some() {
                    return Ok(true);
                }
                if self.check_declared(&parts, caller, dir).is_err() {
                    return Ok(false);
                }
                let loaded = self
                    .loaded_module_by_parts(&parts)
                    .is_some_and(|m| self.check_library_ownership(&parts, &self.modules[m as usize].provenance, "cond-expand").is_ok());
                Ok(loaded || self.library_file(&parts, dir).is_some())
            }
            _ => err(format!("cond-expand: bad requirement {}", display_sexp(req))),
        }
    }

    /// Find the libraries whose names begin with `(owner pkg)` under `dir`,
    /// and nowhere else: `(owner pkg)` is `dir/owner/pkg.sld` and
    /// `(owner pkg a b)` is `dir/owner/pkg/a/b.sld`. A package's manifest says
    /// so of its own (`(techne package)`); the libraries are the world's,
    /// shared by every importer. `dir` need not be there yet, in a directory
    /// that is: a package without libraries owns its 2-part prefix all the
    /// same, and its first library is found once written. It is an error if
    /// another directory provides them already, if the prefix collides with
    /// Techne's own libraries, or if one is already loaded from elsewhere.
    pub fn provide_libraries(&mut self, owner: &str, pkg: &str, dir: &Path) -> R<()> {
        self.check_loading()?;
        if self.bundle.is_some() {
            return err("provide-libraries: cannot register OS library providers when a bundle is installed");
        }
        if !is_valid_package_component(owner) || !is_valid_package_component(pkg) {
            return err(format!("invalid package name ({owner} {pkg})"));
        }
        let missing = || Error::new(format!("{}: no such directory", dir.display()));
        let resolved = match dir.canonicalize() {
            Ok(dir) => Some(dir).filter(|d| d.is_dir()),
            Err(_) => dir.parent().and_then(|parent| Some(parent.canonicalize().ok()?.join(dir.file_name()?))),
        };
        let dir = resolved.filter(|d| self.may_read(d.parent().unwrap_or(d))).ok_or_else(missing)?;
        if let Some(conflict) = self.library_conflict(owner, pkg, &dir) {
            return err(conflict);
        }
        if self.library_provider(owner, pkg).is_none() {
            self.library_providers.push((PackagePrefix::new(owner, pkg), dir));
        }
        Ok(())
    }

    /// Why the libraries whose names begin with `(owner pkg)` cannot be
    /// provided from `dir` (`provide_libraries`), or None if they can: the
    /// owner is reserved (`scheme` or `srfi`), another directory provides
    /// them, `dir` is something else than a directory, Techne's own Lisp
    /// provides `(owner pkg)`, or one of them is loaded from elsewhere
    /// already, which importers would go on finding whatever `dir` holds.
    /// What makes several packages known at once asks before it changes
    /// anything.
    pub fn library_conflict(&self, owner: &str, pkg: &str, dir: &Path) -> Option<String> {
        if self.bundle.is_some() {
            return Some("cannot register OS library providers when a bundle is installed".into());
        }
        if !is_valid_package_component(owner) || !is_valid_package_component(pkg) {
            return Some(format!("invalid package name ({owner} {pkg})"));
        }
        if matches!(owner, "scheme" | "srfi") {
            return Some(format!("namespace ({owner} ...) is reserved"));
        }
        let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        match self.library_provider(owner, pkg) {
            Some(old) if old == dir => return None,
            Some(old) => return Some(format!("libraries ({owner} {pkg} ...) are provided from {} already", old.display())),
            None => {}
        }
        if dir.exists() && !dir.is_dir() {
            return Some(format!("{} is not a directory", dir.display()));
        }
        if is_stdlib_package(owner, pkg) {
            return Some(format!("libraries ({owner} {pkg} ...) are part of Techne's own libraries"));
        }
        self.library_loaded_outside(owner, pkg, &dir).map(|library| format!("library {library} is loaded from elsewhere already"))
    }

    /// The files under `root`, the directory of the package `package`,
    /// import the libraries of the packages `names` only, among those a
    /// package provides (`provide_libraries`): its own and those of the
    /// packages its manifest depends on. Libraries no package provides
    /// (Techne's own, those beside the importing file) are not concerned.
    /// Declared again for a directory, this replaces what was declared.
    pub fn declare_imports(&mut self, root: &Path, package: (&str, &str), names: &[(String, String)]) -> R<()> {
        self.check_loading()?;
        if self.bundle.is_some() {
            return err("declare-imports: cannot declare OS package imports when a bundle is installed");
        }
        if !is_valid_package_component(package.0) || !is_valid_package_component(package.1) {
            return err(format!("invalid package name ({} {})", package.0, package.1));
        }
        for (o, n) in names {
            if !is_valid_package_component(o) || !is_valid_package_component(n) {
                return err(format!("invalid package name ({o} {n})"));
            }
        }
        let missing = || Error::new(format!("{}: no such directory", root.display()));
        let root = root.canonicalize().ok().filter(|d| d.is_dir() && self.may_read(d)).ok_or_else(missing)?;
        let declared = PackageImports {
            root,
            package: PackagePrefix::new(package.0, package.1),
            names: names.iter().map(|(o, n)| PackagePrefix::new(o, n)).collect(),
        };
        match self.package_imports.iter_mut().find(|p| p.root == declared.root) {
            Some(old) => *old = declared,
            None => self.package_imports.push(declared),
        }
        Ok(())
    }

    /// Refuse the import of library `parts` from `caller` if that caller is a
    /// package that does not declare a dependency on the library's package
    /// (`declare_imports` or `BundleVfs`). In locked/bundle mode, fail closed
    /// for unattributed callers and unprovided non-stdlib libraries.
    fn check_declared(&self, parts: &[String], caller: Option<&CallerPackage>, dir: &Path) -> R<()> {
        if is_stdlib_library(parts) {
            return Ok(());
        }
        if let Some(bundle) = &self.bundle {
            if bundle.root_package().is_none() {
                if matches!(caller, Some(CallerPackage::LegacyBundleRoot))
                    && (bundle.root_library_virtual_path(parts, dir).is_some()
                        || self
                            .loaded_module_by_parts(parts)
                            .is_some_and(|m| matches!(self.modules[m as usize].provenance, SourceProvenance::BundleRoot { .. })))
                {
                    return Ok(());
                }
                return err(format!("import: {}: library is not provided in locked mode", module_name(parts)));
            }
            let Some([owner, pkg]) = parts.get(..2) else {
                return err(format!("import: {}: library is not provided in locked mode", module_name(parts)));
            };
            if !bundle.has_package(owner, pkg) {
                return err(format!("import: {}: library is not provided in locked mode", module_name(parts)));
            }
            match caller {
                Some(CallerPackage::Package(importer)) => {
                    if !bundle.package_allows(importer, owner, pkg) {
                        return err(format!(
                            "import: {}: the package {importer} does not depend on ({owner} {pkg}) (its techne-package.scm says what it depends on)",
                            module_name(parts),
                        ));
                    }
                    return Ok(());
                }
                _ => {
                    return err(format!(
                        "import: {}: unattributed caller cannot import ({owner} {pkg}) in bundle mode",
                        module_name(parts)
                    ));
                }
            }
        }
        let provider = parts.get(..2).and_then(|p| self.library_provider(&p[0], &p[1]));
        if self.locked_libraries && provider.is_none() {
            return err(format!("import: {}: library is not provided in locked mode", module_name(parts)));
        }
        let Some([owner, pkg]) = parts.get(..2) else {
            return Ok(());
        };
        if self.package_imports.is_empty() || provider.is_none() {
            return Ok(());
        }
        let check_dir = |d: &Path| -> R<()> {
            let Ok(d) = d.canonicalize() else { return Ok(()) };
            match self.package_imports.iter().filter(|p| d.starts_with(&p.root)).max_by_key(|p| p.root.as_os_str().len()) {
                Some(p) if !p.names.iter().any(|n| n.matches(owner, pkg)) => err(format!(
                    "import: {}: the package {} does not depend on ({owner} {pkg}) (its techne-package.scm says what it depends on)",
                    module_name(parts),
                    p.package
                )),
                _ => Ok(()),
            }
        };
        match caller {
            Some(CallerPackage::Package(importer)) => match self.package_imports.iter().find(|p| p.package == *importer) {
                Some(p) if !p.names.iter().any(|n| n.matches(owner, pkg)) => err(format!(
                    "import: {}: the package {} does not depend on ({owner} {pkg}) (its techne-package.scm says what it depends on)",
                    module_name(parts),
                    p.package
                )),
                _ => Ok(()),
            },
            Some(CallerPackage::HostDir(d)) => check_dir(d),
            Some(CallerPackage::Stdlib) => Ok(()),
            None | Some(CallerPackage::LegacyBundleRoot) => {
                if self.locked_libraries {
                    return err(format!(
                        "import: {}: unattributed caller cannot import ({owner} {pkg}) in locked mode",
                        module_name(parts)
                    ));
                }
                if dir.as_os_str().is_empty() {
                    return Ok(());
                }
                check_dir(dir)
            }
        }
    }

    /// The directory providing the libraries whose names begin with `(owner pkg)`.
    pub fn library_provider(&self, owner: &str, pkg: &str) -> Option<&Path> {
        self.library_providers.iter().find(|(p, _)| p.matches(owner, pkg)).map(|(_, dir)| &**dir)
    }

    /// Where library `parts` is defined: Techne's own `lisp_dir()` for stdlib
    /// libraries, under its provider's directory if its 2-part prefix has one,
    /// else (only when not locked) where [`library_file`] finds it.
    pub(crate) fn library_file(&self, parts: &[String], dir: &Path) -> Option<PathBuf> {
        if parts.is_empty() || !parts.iter().all(|p| valid_library_part(p)) {
            return None;
        }
        if is_stdlib_library(parts) {
            return contained_library_file(lisp_dir(), parts);
        }
        if let Some(bundle) = &self.bundle {
            if bundle.root_package().is_none() {
                return bundle.root_library_virtual_path(parts, dir);
            }
            return bundle.library_virtual_path(parts);
        }
        if let Some(provider) = parts.get(..2).and_then(|p| self.library_provider(&p[0], &p[1])) {
            return contained_library_file(provider, parts);
        }
        if self.locked_libraries {
            return None;
        }
        library_file(parts, dir)
    }
}

/// Whether library `parts` is built in: an R7RS or SRFI view of the root
/// module, or `(techne)`.
pub fn is_builtin(parts: &[String]) -> bool {
    builtin(parts).is_some()
}

/// Where library `parts` is defined: Techne's own Lisp ([`lisp_dir`]) first so
/// stdlib cannot be shadowed, then `a/b.sld` in `dir`, then on `TECHNE_LIBRARY_PATH`.
pub fn library_file(parts: &[String], dir: &Path) -> Option<PathBuf> {
    if parts.is_empty() || !parts.iter().all(|p| valid_library_part(p)) {
        return None;
    }
    if let Some(own) = contained_library_file(lisp_dir(), parts) {
        return Some(own);
    }
    if is_stdlib_library(parts) {
        return None;
    }
    let search_dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    if let Some(local) = contained_library_file(search_dir, parts) {
        return Some(local);
    }
    let path_dirs = std::env::var("TECHNE_LIBRARY_PATH").unwrap_or_default();
    std::env::split_paths(&path_dirs).find_map(|d| contained_library_file(&d, parts))
}
