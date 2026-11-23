//! Mode 2 live in-world library patching (`Vm::patch_library` / `patch-library!`).
//!
//! Diffs a newly supplied `(define-library <name> ...)` source against the
//! loaded library's stored [`LibrarySnapshot`]. Unchanged forms (including
//! `define-record-type`, `define-syntax`, and state initializers) are never
//! re-evaluated. Only pure procedure definitions owned by the library module
//! whose runtime slot still holds the installed procedure may change;
//! structural changes are refused with typed [`PatchRefusal`] errors before
//! any VM state is mutated. All changed procedures are compiled and
//! instantiated first in read-only `patch_staging` mode, then committed
//! atomically in a single non-yielding loop over `vm.globals`.

use std::{fmt, rc::Rc};

use rustc_hash::{FxHashMap, FxHashSet};
use sha2::{Digest, Sha256};

use crate::{
    compiler::Compiler,
    library::{FEATURES, builtin, module_name, name_parts, syms},
    reader::{self, Sexp, display_sexp, strip, symbol_name},
    value::Value,
    vm::{Capability, Error, GlobalBinding, Vm, meter::Meter},
};

/// Maximum heap nodes visited by a single-pass Mode 2 retention scan.
pub const MAX_RETENTION_NODES: usize = 200_000;
/// Maximum stale-holder entries returned in a single [`PatchReport`].
pub const MAX_STALE_HOLDERS: usize = 256;
/// Maximum steps reported per stale-holder retention chain.
pub const MAX_STALE_CHAIN: usize = 64;
/// Maximum number of components in a Scheme library name accepted over RPC/IPC.
pub const MAX_LIBRARY_PARTS: usize = 32;
/// Maximum UTF-8 byte length of a single library name component.
pub const MAX_LIBRARY_PART_BYTES: usize = 256;
/// Maximum UTF-8 byte length of a Scheme library name string.
pub const MAX_LIBRARY_NAME_BYTES: usize = 4096;
/// Maximum UTF-8 byte length of a single Mode 2 library source patch (4 MiB).
pub const MAX_PATCH_BYTES: usize = 4 << 20;

/// Load-time `cond-expand` clause decision captured in a [`LibrarySnapshot`]
/// so live patching can verify that `cond-expand` branches have not diverged.
#[derive(Clone)]
pub(crate) struct CondExpandDecision {
    pub(crate) requirements: Vec<Sexp>,
    pub(crate) chosen: Option<usize>,
}

/// Expanded library declarations and body forms retained at load time so
/// Mode 2 patching can perform span-insensitive form-by-form diffs without
/// re-evaluating unchanged forms or touching the filesystem.
#[derive(Clone)]
pub(crate) struct LibrarySnapshot {
    /// Canonical export specs `(internal_sym, exported_sym)` in declaration order.
    pub(crate) exports: Vec<(u32, u32)>,
    /// Canonical import sets from all `(import set ...)` declarations.
    pub(crate) imports: Vec<Sexp>,
    /// Expanded top-level body forms (flattened across unshadowed `begin`,
    /// `cond-expand`, `include`, `include-ci`, and `include-library-declarations`).
    pub(crate) body_forms: Vec<Sexp>,
    /// Pre-order `cond-expand` clause selections recorded at library load time.
    pub(crate) cond_expand_decisions: Vec<CondExpandDecision>,
    /// Expected closed-procedure `code_id` for each owned procedure slot,
    /// populated at load finalization and updated on each successful patch.
    pub(crate) installed_procs: FxHashMap<u32, u32>,
    /// Approximate retained memory in bytes, charged via `Module::table_bytes`.
    approx_bytes: usize,
}

impl LibrarySnapshot {
    pub(crate) fn new(
        exports: Vec<(u32, u32)>,
        imports: Vec<Sexp>,
        body_forms: Vec<Sexp>,
        cond_expand_decisions: Vec<CondExpandDecision>,
        installed_procs: FxHashMap<u32, u32>,
    ) -> Self {
        let mut approx_bytes = std::mem::size_of::<LibrarySnapshot>()
            + exports.capacity() * std::mem::size_of::<(u32, u32)>()
            + imports.capacity() * std::mem::size_of::<Sexp>()
            + body_forms.capacity() * std::mem::size_of::<Sexp>()
            + cond_expand_decisions.capacity() * std::mem::size_of::<CondExpandDecision>()
            + installed_procs.capacity() * (std::mem::size_of::<(u32, u32)>() + 1);
        for s in &imports {
            approx_bytes += sexp_bytes(s);
        }
        for s in &body_forms {
            approx_bytes += sexp_bytes(s);
        }
        for d in &cond_expand_decisions {
            approx_bytes += d.requirements.capacity() * std::mem::size_of::<Sexp>();
            for r in &d.requirements {
                approx_bytes += sexp_bytes(r);
            }
        }
        Self { exports, imports, body_forms, cond_expand_decisions, installed_procs, approx_bytes }
    }

    pub(crate) fn approx_bytes(&self) -> usize {
        self.approx_bytes
    }
}

fn sexp_bytes(s: &Sexp) -> usize {
    crate::nested(|| sexp_bytes_step(s))
}

fn sexp_bytes_step(s: &Sexp) -> usize {
    let mut n = std::mem::size_of::<Sexp>();
    match s {
        Sexp::BigInt(_) => n += 32,
        Sexp::Ratio(_) => n += 64,
        Sexp::Complex(a, b) => n += sexp_bytes(a) + sexp_bytes(b),
        Sexp::Str(s) => n += s.len(),
        Sexp::Bytes(b) => n += b.len(),
        Sexp::Labeled(_, inner) => n += sexp_bytes(inner),
        Sexp::List(items, tail, _) => {
            n += items.capacity() * std::mem::size_of::<Sexp>();
            for item in items {
                n += sexp_bytes(item);
            }
            if let Some(t) = tail {
                n += sexp_bytes(t);
            }
        }
        Sexp::Vector(items) | Sexp::Dict(items) | Sexp::Set(items) => {
            n += items.capacity() * std::mem::size_of::<Sexp>();
            for item in items {
                n += sexp_bytes(item);
            }
        }
        _ => {}
    }
    n
}

/// Typed refusal reason returned when Mode 2 live patching cannot apply a
/// library update safely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchRefusal {
    NotLoaded,
    SyntaxError,
    LibraryNameMismatch,
    IncludeNotSupported,
    UnsupportedDeclaration,
    ImportsChanged,
    ExportRebound,
    BindingRemoved,
    BindingAdded,
    RecordLayoutChanged,
    MacroChanged,
    StateInitializerChanged,
    CompileError,
    Poisoned,
}

impl PatchRefusal {
    /// Stable kebab-case identifier used across Rust, Scheme, nREPL, and HTTP APIs.
    pub const fn code(self) -> &'static str {
        match self {
            PatchRefusal::NotLoaded => "not-loaded",
            PatchRefusal::SyntaxError => "syntax-error",
            PatchRefusal::LibraryNameMismatch => "library-name-mismatch",
            PatchRefusal::IncludeNotSupported => "include-not-supported",
            PatchRefusal::UnsupportedDeclaration => "unsupported-declaration",
            PatchRefusal::ImportsChanged => "imports-changed",
            PatchRefusal::ExportRebound => "export-rebound",
            PatchRefusal::BindingRemoved => "binding-removed",
            PatchRefusal::BindingAdded => "binding-added",
            PatchRefusal::RecordLayoutChanged => "record-layout-changed",
            PatchRefusal::MacroChanged => "macro-changed",
            PatchRefusal::StateInitializerChanged => "state-initializer-changed",
            PatchRefusal::CompileError => "compile-error",
            PatchRefusal::Poisoned => "poisoned",
        }
    }
}

impl fmt::Display for PatchRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// Structured error returned by [`Vm::patch_library`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchError {
    pub reason: PatchRefusal,
    pub message: String,
    pub binding: Option<String>,
}

impl PatchError {
    pub fn new(reason: PatchRefusal, message: impl Into<String>) -> Self {
        Self { reason, message: message.into(), binding: None }
    }

    pub fn with_binding(reason: PatchRefusal, binding: impl Into<String>, message: impl Into<String>) -> Self {
        Self { reason, message: message.into(), binding: Some(binding.into()) }
    }
}

impl fmt::Display for PatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.reason.code(), self.message)
    }
}

impl std::error::Error for PatchError {}

/// Diagnostic entry describing a live reference chain still retaining a
/// replaced procedure closure after patching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleHolder {
    pub binding: String,
    pub chain: Vec<String>,
}

/// Summary returned after an atomic Mode 2 library patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchReport {
    pub library: Vec<String>,
    pub patched: Vec<String>,
    pub unchanged: usize,
    pub stale_holders: Vec<StaleHolder>,
    pub stale_holders_truncated: bool,
    pub source_sha256: String,
}

/// Compute lowercase hex SHA-256 of UTF-8 source bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        use fmt::Write as _;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

fn is_library_delimiter(c: char) -> bool {
    c.is_control() || c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']' | '"' | ';' | '#' | '\'' | '`' | ',' | '|')
}

/// Validate canonical library name parts without interning any symbols.
/// Enforces `1..=MAX_LIBRARY_PARTS` components, each `1..=MAX_LIBRARY_PART_BYTES`
/// UTF-8 bytes, with no control characters, whitespace, or Scheme delimiters.
pub fn validate_library_parts(parts: &[String]) -> Result<(), String> {
    if parts.is_empty() {
        return Err("library name must not be empty".into());
    }
    if parts.len() > MAX_LIBRARY_PARTS {
        return Err(format!("library name has {} parts (maximum {MAX_LIBRARY_PARTS})", parts.len()));
    }
    for part in parts {
        if part.is_empty() {
            return Err("library name part must not be empty".into());
        }
        if part.len() > MAX_LIBRARY_PART_BYTES {
            return Err(format!("library name part exceeds {MAX_LIBRARY_PART_BYTES} bytes"));
        }
        if part == "." {
            return Err("library name part must not be `.`".into());
        }
        if part.chars().any(is_library_delimiter) {
            return Err(format!("invalid character in library name part `{part}`"));
        }
        // Reject signed numbers, decimals, ratios, or tokens starting with digits
        // unless they are canonical non-negative exact integers.
        let bytes = part.as_bytes();
        if bytes[0].is_ascii_digit() {
            if !bytes.iter().all(u8::is_ascii_digit) {
                return Err(format!("invalid numeric library name part `{part}`"));
            }
            if part.parse::<i64>().is_err() {
                return Err(format!("library name integer out of range: `{part}`"));
            }
        } else if ((bytes[0] == b'+' || bytes[0] == b'-') && bytes.len() > 1 && (bytes[1].is_ascii_digit() || bytes[1] == b'.'))
            || (bytes[0] == b'.' && bytes.len() > 1 && bytes[1].is_ascii_digit())
        {
            return Err(format!("invalid numeric library name part `{part}`"));
        }
    }
    Ok(())
}

/// Parse a Scheme library name string such as `"(alice math)"` into canonical parts
/// without invoking the Scheme reader or interning any symbols.
pub fn parse_library_name(text: &str) -> Result<Vec<String>, String> {
    if text.len() > MAX_LIBRARY_NAME_BYTES {
        return Err(format!("bad library name: exceeds {MAX_LIBRARY_NAME_BYTES} bytes"));
    }
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix('(') else {
        return Err("bad library name: expected a parenthesized library name list".into());
    };
    let Some(inner) = rest.strip_suffix(')') else {
        return Err("bad library name: missing closing `)`".into());
    };
    let mut parts = Vec::new();
    let mut chars = inner.char_indices().peekable();
    while let Some(&(start, c)) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if is_library_delimiter(c) {
            return Err(format!("bad library name: unexpected `{c}`"));
        }
        let mut end = inner.len();
        while let Some(&(i, ch)) = chars.peek() {
            if ch.is_whitespace() {
                end = i;
                break;
            }
            if is_library_delimiter(ch) {
                return Err(format!("bad library name: unexpected `{ch}`"));
            }
            chars.next();
        }
        let tok = &inner[start..end];
        if tok.bytes().all(|b| b.is_ascii_digit()) {
            let n = tok.parse::<i64>().map_err(|_| format!("bad library name: integer out of range `{tok}`"))?;
            parts.push(n.to_string());
        } else {
            parts.push(tok.to_string());
        }
        if parts.len() > MAX_LIBRARY_PARTS {
            return Err(format!("bad library name: exceeds {MAX_LIBRARY_PARTS} parts"));
        }
    }
    validate_library_parts(&parts).map_err(|e| format!("bad library name: {e}"))?;
    Ok(parts)
}

/// Flatten nested unshadowed `(begin ...)` forms in top-level library bodies
/// so load-time snapshots and patch sources share an identical form sequence.
/// Respects both imported `begin` shadowing and in-body `(define-syntax begin ...)`
/// or `(define begin ...)` declarations.
fn flatten_body_forms_in_module(vm: &Vm, m: u32, forms: &[Sexp], out: &mut Vec<Sexp>) {
    let begin_sym = reader::intern("begin");
    let mut begin_shadowed = vm.modules[m as usize].imports.contains_key(&begin_sym);
    flatten_body_forms_step(vm, m, begin_sym, &mut begin_shadowed, forms, out);
}

fn flatten_body_forms_step(vm: &Vm, m: u32, begin_sym: u32, begin_shadowed: &mut bool, forms: &[Sexp], out: &mut Vec<Sexp>) {
    for form in forms {
        if !*begin_shadowed
            && let Some(items) = form.list()
            && let Some(head) = items.first().and_then(Sexp::sym)
            && strip(head) == begin_sym
        {
            crate::nested(|| flatten_body_forms_step(vm, m, begin_sym, begin_shadowed, &items[1..], out));
            continue;
        }
        if !*begin_shadowed && defines_symbol(vm, m, form, begin_sym) {
            *begin_shadowed = true;
        }
        out.push(form.clone());
    }
}

fn defines_symbol(vm: &Vm, m: u32, form: &Sexp, target: u32) -> bool {
    let Some(special) = top_level_special(vm, m, form) else {
        return false;
    };
    let Some(items) = form.list() else {
        return false;
    };
    match special {
        "define-syntax" => items.get(1).and_then(Sexp::sym).map(strip) == Some(target),
        "define" => match items.get(1) {
            Some(Sexp::Sym(s)) => strip(*s) == target,
            Some(sig @ Sexp::List(..)) => procedure_define_name(sig) == Some(target),
            _ => false,
        },
        "define-record-type" => items.get(1).and_then(Sexp::sym).map(strip) == Some(target),
        _ => false,
    }
}

/// Span-insensitive structural equality on parsed S-expressions.
pub(crate) fn sexp_eq_ignore_pos(a: &Sexp, b: &Sexp) -> bool {
    crate::nested(|| sexp_eq_step(a, b))
}

fn sexp_eq_step(a: &Sexp, b: &Sexp) -> bool {
    match (a, b) {
        (Sexp::Int(x), Sexp::Int(y)) => x == y,
        (Sexp::BigInt(x), Sexp::BigInt(y)) => x == y,
        (Sexp::Ratio(x), Sexp::Ratio(y)) => x == y,
        (Sexp::Complex(ar, ai), Sexp::Complex(br, bi)) => sexp_eq_ignore_pos(ar, br) && sexp_eq_ignore_pos(ai, bi),
        (Sexp::Float(x), Sexp::Float(y)) => x.to_bits() == y.to_bits(),
        (Sexp::Bool(x), Sexp::Bool(y)) => x == y,
        (Sexp::Char(x), Sexp::Char(y)) => x == y,
        (Sexp::Str(x), Sexp::Str(y)) => x == y,
        (Sexp::Bytes(x), Sexp::Bytes(y)) => x == y,
        (Sexp::Sym(x), Sexp::Sym(y)) => strip(*x) == strip(*y),
        (Sexp::Keyword(x), Sexp::Keyword(y)) => strip(*x) == strip(*y),
        (Sexp::LabelRef(x), Sexp::LabelRef(y)) => x == y,
        (Sexp::Labeled(na, da), Sexp::Labeled(nb, db)) => na == nb && sexp_eq_ignore_pos(da, db),
        (Sexp::List(ai, at, _), Sexp::List(bi, bt, _)) => {
            ai.len() == bi.len()
                && ai.iter().zip(bi.iter()).all(|(x, y)| sexp_eq_ignore_pos(x, y))
                && match (at, bt) {
                    (None, None) => true,
                    (Some(x), Some(y)) => sexp_eq_ignore_pos(x, y),
                    _ => false,
                }
        }
        (Sexp::Vector(ai), Sexp::Vector(bi)) | (Sexp::Dict(ai), Sexp::Dict(bi)) | (Sexp::Set(ai), Sexp::Set(bi)) => {
            ai.len() == bi.len() && ai.iter().zip(bi.iter()).all(|(x, y)| sexp_eq_ignore_pos(x, y))
        }
        _ => false,
    }
}

/// Top-level special-form name of `form` in module `m` (if not shadowed by a
/// local binding or macro in `m`).
fn top_level_special(vm: &Vm, m: u32, form: &Sexp) -> Option<&'static str> {
    let items = form.list()?;
    let sym = items.first()?.sym()?;
    let sym = strip(sym);
    if vm.lookup_global(m, sym).is_some() {
        return None;
    }
    reader::special_form(sym)
}

/// Drill through `(f ...)`, `((f x) y)`, etc. to find the defined procedure symbol.
fn procedure_define_name(sig: &Sexp) -> Option<u32> {
    match sig {
        Sexp::List(items, _, _) if !items.is_empty() => match &items[0] {
            Sexp::Sym(s) => Some(strip(*s)),
            inner @ Sexp::List(..) => crate::nested(|| procedure_define_name(inner)),
            _ => None,
        },
        _ => None,
    }
}

enum FormKind {
    RecordType(Option<u32>),
    Macro(Option<u32>),
    DefineProc { name: u32, var_doc: Option<Rc<str>> },
    DefineValue { name: u32 },
    Include,
    OtherExpr,
}

fn classify_form(vm: &Vm, m: u32, form: &Sexp) -> FormKind {
    let Some(special) = top_level_special(vm, m, form) else {
        return FormKind::OtherExpr;
    };
    let items = form.list().expect("special form is a proper list");
    match special {
        "define-record-type" => FormKind::RecordType(items.get(1).and_then(Sexp::sym).map(strip)),
        "define-syntax" => FormKind::Macro(items.get(1).and_then(Sexp::sym).map(strip)),
        "include" | "include-ci" => FormKind::Include,
        "define" => match items.get(1) {
            Some(sig @ Sexp::List(..)) => match procedure_define_name(sig) {
                Some(name) => FormKind::DefineProc { name, var_doc: None },
                None => FormKind::OtherExpr,
            },
            Some(Sexp::Sym(name)) => {
                let sym = strip(*name);
                let valid_len = items.len() == 3 || (items.len() == 4 && matches!(items[3], Sexp::Str(_)));
                if valid_len && top_level_special(vm, m, &items[2]) == Some("lambda") {
                    let var_doc = match items.get(3) {
                        Some(Sexp::Str(doc)) => Some(doc.clone()),
                        _ => None,
                    };
                    FormKind::DefineProc { name: sym, var_doc }
                } else {
                    FormKind::DefineValue { name: sym }
                }
            }
            _ => FormKind::OtherExpr,
        },
        _ => FormKind::OtherExpr,
    }
}

struct PendingProc {
    name: String,
    slot: u32,
    form: Sexp,
    var_doc: Option<Rc<str>>,
}

impl Vm {
    /// Whether any library in this world has been mutated by Mode 2 live patching.
    pub fn is_patch_dirty(&self) -> bool {
        !self.patched_libraries.is_empty()
    }

    /// Libraries patched in this world instance `(library_parts, source_sha256)`.
    pub fn patch_deltas(&self) -> Vec<(Vec<String>, String)> {
        let mut out: Vec<(Vec<String>, String)> = self.patched_libraries.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Enforce Scheme-level authority for `(patch-library! ...)`:
    /// requires `Capability::Patching`, and in bundle/locked mode additionally
    /// requires package ownership and bundle module accessibility so untrusted
    /// packages or production initializers cannot patch other libraries.
    pub(crate) fn check_scheme_patch_authority(&mut self, parts: &[String]) -> Result<(), Error> {
        self.need(Capability::Patching, "patch-library!")?;
        if self.bundle.is_some() || self.locked_libraries {
            let provenance = self.current_eval_provenance();
            self.check_library_ownership(parts, &provenance, "patch-library!")?;
            if self.bundle.is_some()
                && let Some(m) = self.loaded_library_parts(parts)?
            {
                let caller = self.active_caller_package();
                if !self.bundle_module_accessible(caller.as_ref(), m) {
                    return Err(Error::new(format!("patch-library!: {} is not accessible from this package", module_name(parts))));
                }
            }
        }
        Ok(())
    }

    /// Save the raw declarations, body forms, and `cond-expand` decisions of a
    /// freshly declared library before its imports and body run.
    pub(crate) fn record_library_snapshot(
        &mut self,
        m: u32,
        imports: &[(Sexp, u32)],
        body: &[(Sexp, u32)],
        cond_expand_decisions: Vec<CondExpandDecision>,
    ) {
        let exports = self.modules[m as usize].exports.clone().unwrap_or_default();
        let mut import_sets = Vec::new();
        for (decl, _) in imports {
            if let Some(items) = decl.list() {
                import_sets.extend(items[1..].iter().cloned());
            }
        }
        let raw_body: Vec<Sexp> = body.iter().map(|(f, _)| f.clone()).collect();
        self.modules[m as usize].library_snapshot =
            Some(Rc::new(LibrarySnapshot::new(exports, import_sets, raw_body, cond_expand_decisions, FxHashMap::default())));
        self.recharge_module(m);
    }

    /// Finalize a library's snapshot once its imports and body have finished
    /// evaluating: flatten unshadowed `begin` forms using the module's final
    /// bindings and record the installed `code_id` for each owned procedure.
    pub(crate) fn finalize_library_snapshot(&mut self, m: u32) {
        let Some(prev) = self.modules[m as usize].library_snapshot.clone() else {
            return;
        };
        let mut body_forms = Vec::new();
        flatten_body_forms_in_module(self, m, &prev.body_forms, &mut body_forms);
        let mut installed_procs = FxHashMap::default();
        for form in &body_forms {
            if let FormKind::DefineProc { name, .. } = classify_form(self, m, form)
                && let Some(GlobalBinding::Var(slot)) = self.bindings.get(&(m, name))
                && self.global_module[*slot as usize] == m
                && let Some(code_id) = self.closed_closure_code_id(self.globals[*slot as usize])
            {
                installed_procs.insert(*slot, code_id);
            }
        }
        self.modules[m as usize].library_snapshot = Some(Rc::new(LibrarySnapshot::new(
            prev.exports.clone(),
            prev.imports.clone(),
            body_forms,
            prev.cond_expand_decisions.clone(),
            installed_procs,
        )));
        self.recharge_module(m);
    }

    /// Live-patch procedures of an already loaded library in place (Mode 2).
    ///
    /// Executes zero user code during parsing, diffing, and staging, and
    /// commits all changed procedure slots atomically in a single loop.
    pub fn patch_library(&mut self, parts: &[String], source: &str, check_retained: bool) -> Result<PatchReport, PatchError> {
        match self.host(|vm| vm.patch_library_raw(parts, source, check_retained)) {
            Ok(result) => result,
            Err(e) => Err(PatchError::new(PatchRefusal::Poisoned, e.msg.clone())),
        }
    }

    /// Raw entry point used both by [`Vm::patch_library`] (inside `Vm::host`)
    /// and by the Scheme `patch-library!` native (already inside the world's cage).
    pub(crate) fn patch_library_raw(&mut self, parts: &[String], source: &str, check_retained: bool) -> Result<PatchReport, PatchError> {
        if let Some(why) = self.poisoned() {
            return Err(PatchError::new(PatchRefusal::Poisoned, format!("the world is poisoned: {why}")));
        }
        let lib_display = module_name(parts);
        if parts.is_empty() || builtin(parts).is_some() {
            return Err(PatchError::new(PatchRefusal::NotLoaded, format!("library {lib_display} is built-in or cannot be live-patched")));
        }
        let m = match self.loaded_library_parts(parts) {
            Ok(Some(m)) => m,
            Ok(None) => {
                return Err(PatchError::new(PatchRefusal::NotLoaded, format!("library {lib_display} is not loaded")));
            }
            Err(e) => {
                return Err(PatchError::new(PatchRefusal::NotLoaded, e.msg.clone()));
            }
        };
        let Some(snapshot) = self.modules[m as usize].library_snapshot.clone() else {
            return Err(PatchError::new(PatchRefusal::NotLoaded, format!("library {lib_display} has no define-library snapshot")));
        };

        let source_sha256 = sha256_hex(source.as_bytes());

        // 1. Pure parse of patch source (metered, zero VM mutation).
        let forms = {
            let meter = Meter::new(self);
            let read = reader::read_metered(source, false, &meter);
            if let Some(e) = meter.error(self) {
                return Err(PatchError::new(PatchRefusal::SyntaxError, e.msg.clone()));
            }
            match read {
                Ok(forms) => forms,
                Err(e) => {
                    let msg = match e.pos {
                        Some(pos) => {
                            let (line, col) = reader::line_col(source, pos);
                            format!("{lib_display}:{line}:{col}: {}", e.message)
                        }
                        None => format!("{lib_display}: {}", e.message),
                    };
                    return Err(PatchError::new(PatchRefusal::SyntaxError, msg));
                }
            }
        };
        if forms.len() != 1 {
            return Err(PatchError::new(
                PatchRefusal::SyntaxError,
                format!("expected a single (define-library {lib_display} ...) form, found {}", forms.len()),
            ));
        }
        let Some(lib_items) = forms[0].list() else {
            return Err(PatchError::new(PatchRefusal::SyntaxError, "expected a (define-library ...) list"));
        };
        if !lib_items.first().is_some_and(|h| h.is_sym("define-library")) {
            return Err(PatchError::new(PatchRefusal::SyntaxError, "expected a (define-library ...) form"));
        }
        let Some(parsed_parts) = lib_items.get(1).and_then(name_parts) else {
            return Err(PatchError::new(PatchRefusal::SyntaxError, "define-library: bad library name"));
        };
        if parsed_parts != parts {
            return Err(PatchError::new(
                PatchRefusal::LibraryNameMismatch,
                format!("patch defines {} but target is {lib_display}", module_name(&parsed_parts)),
            ));
        }

        // 2. Pure declaration extraction & validation (no filesystem reads, no library loads).
        let mut new_exports: Vec<(u32, u32)> = Vec::new();
        let mut new_imports: Vec<Sexp> = Vec::new();
        let mut raw_body: Vec<Sexp> = Vec::new();
        let mut decision_idx: usize = 0;
        self.parse_patch_declarations(
            &lib_items[2..],
            &snapshot.cond_expand_decisions,
            &mut decision_idx,
            &mut new_exports,
            &mut new_imports,
            &mut raw_body,
        )?;
        if decision_idx != snapshot.cond_expand_decisions.len() {
            return Err(PatchError::new(
                PatchRefusal::UnsupportedDeclaration,
                format!("library {lib_display}: cond-expand declarations changed since load"),
            ));
        }
        let mut new_body_forms = Vec::new();
        flatten_body_forms_in_module(self, m, &raw_body, &mut new_body_forms);

        // Compare exports: removed -> binding-removed; added/renamed/changed -> export-rebound.
        check_exports(&snapshot.exports, &new_exports)?;

        // Compare imports structurally.
        if snapshot.imports.len() != new_imports.len()
            || !snapshot.imports.iter().zip(new_imports.iter()).all(|(a, b)| sexp_eq_ignore_pos(a, b))
        {
            return Err(PatchError::new(
                PatchRefusal::ImportsChanged,
                format!("library {lib_display}: import declarations changed; use a full reload or snapshot swap"),
            ));
        }

        // 3. Structural form-by-form diff (span-insensitive).
        let (changes, unchanged) = self.diff_library_body(m, &lib_display, &snapshot, &new_body_forms)?;

        if changes.is_empty() {
            // Only whitespace/comments/procedure order changed: update snapshot without recompiling or marking dirty.
            self.modules[m as usize].library_snapshot = Some(Rc::new(LibrarySnapshot::new(
                new_exports,
                new_imports,
                new_body_forms,
                snapshot.cond_expand_decisions.clone(),
                snapshot.installed_procs.clone(),
            )));
            self.recharge_module(m);
            return Ok(PatchReport {
                library: parts.to_vec(),
                patched: Vec::new(),
                unchanged,
                stale_holders: Vec::new(),
                stale_holders_truncated: false,
                source_sha256,
            });
        }

        // 4. Stage & compile ALL changed procedures before any commit.
        let orig_provenance = self.modules[m as usize].provenance.clone();
        let file_name = self.modules[m as usize]
            .path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.modules[m as usize].name.to_string());

        type CommitOutcome = (Vec<String>, Vec<(String, Value)>);
        let (patched_names, old_values) = self.pinning(|vm| -> Result<CommitOutcome, PatchError> {
            let file = vm
                .add_file_with_provenance(&file_name, source, orig_provenance)
                .map_err(|e| PatchError::new(PatchRefusal::CompileError, e.msg.clone()))?;

            let mut staged = Vec::with_capacity(changes.len());
            for change in &changes {
                let code = Compiler::new(vm, m, file)
                    .compile_named_procedure(&change.form)
                    .map_err(|(refusal, e)| PatchError::with_binding(refusal, &change.name, e.msg.clone()))?;
                // Allocate the closed procedure closure directly without executing user code.
                let closure = vm.alloc_closed_closure_raw(code);
                staged.push((code, vm.root(closure)));
            }

            // 5. Atomic commit: single non-yielding loop, zero GC allocations.
            let mut old_values = Vec::with_capacity(changes.len());
            let mut patched_names = Vec::with_capacity(changes.len());
            let mut new_installed = snapshot.installed_procs.clone();
            for (change, (code_id, root)) in changes.iter().zip(staged.iter()) {
                let old = vm.globals[change.slot as usize];
                old_values.push((change.name.clone(), old));
                vm.globals[change.slot as usize] = root.get();
                new_installed.insert(change.slot, *code_id);
                patched_names.push(change.name.clone());
            }
            // Drop staging roots before inspecting retention so our own roots are gone.
            drop(staged);

            for change in &changes {
                match &change.var_doc {
                    Some(doc) => {
                        vm.variable_docs.insert(change.slot, doc.clone());
                    }
                    None => {
                        vm.variable_docs.remove(&change.slot);
                    }
                }
            }
            vm.modules[m as usize].library_snapshot = Some(Rc::new(LibrarySnapshot::new(
                new_exports,
                new_imports,
                new_body_forms,
                snapshot.cond_expand_decisions.clone(),
                new_installed,
            )));
            vm.recharge_module(m);
            vm.patched_libraries.insert(parts.to_vec(), source_sha256.clone());

            Ok((patched_names, old_values))
        })?;

        // 6. Optional bounded single-pass stale-holder diagnostics.
        let mut stale_holders = Vec::new();
        let mut stale_holders_truncated = false;
        if check_retained && !old_values.is_empty() {
            let targets: Vec<Value> = old_values.iter().map(|(_, v)| *v).collect();
            let (chains, truncated) = self.why_retained_many_raw(&targets, MAX_RETENTION_NODES, MAX_STALE_CHAIN);
            stale_holders_truncated = truncated;
            for ((binding, _), chain) in old_values.into_iter().zip(chains) {
                if let Some(chain) = chain {
                    if stale_holders.len() >= MAX_STALE_HOLDERS {
                        stale_holders_truncated = true;
                        break;
                    }
                    stale_holders.push(StaleHolder { binding, chain });
                }
            }
        }

        Ok(PatchReport {
            library: parts.to_vec(),
            patched: patched_names,
            unchanged,
            stale_holders,
            stale_holders_truncated,
            source_sha256,
        })
    }

    fn parse_patch_declarations(
        &self,
        decls: &[Sexp],
        recorded: &[CondExpandDecision],
        decision_idx: &mut usize,
        exports: &mut Vec<(u32, u32)>,
        imports: &mut Vec<Sexp>,
        body: &mut Vec<Sexp>,
    ) -> Result<(), PatchError> {
        for decl in decls {
            let items = decl
                .list()
                .filter(|l| !l.is_empty())
                .ok_or_else(|| PatchError::new(PatchRefusal::SyntaxError, "define-library: bad declaration"))?;
            let head = items[0].sym().map(|s| symbol_name(strip(s))).unwrap_or_else(|| "".into());
            match &*head {
                "export" => {
                    for spec in &items[1..] {
                        match spec {
                            Sexp::Sym(s) => exports.push((strip(*s), strip(*s))),
                            Sexp::List(l, None, _) if l.len() == 3 && l[0].is_sym("rename") => {
                                let v = syms(&l[1..], "export").map_err(|e| PatchError::new(PatchRefusal::SyntaxError, e.msg.clone()))?;
                                exports.push((v[0], v[1]));
                            }
                            _ => {
                                return Err(PatchError::new(
                                    PatchRefusal::SyntaxError,
                                    format!("export: bad export spec {}", display_sexp(spec)),
                                ));
                            }
                        }
                    }
                }
                "import" => imports.extend(items[1..].iter().cloned()),
                "begin" => body.extend(items[1..].iter().cloned()),
                "include" | "include-ci" | "include-library-declarations" => {
                    return Err(PatchError::new(
                        PatchRefusal::IncludeNotSupported,
                        format!("define-library declaration `{head}` is not supported in live patches"),
                    ));
                }
                "cond-expand" => {
                    let clauses = &items[1..];
                    let Some(expected) = recorded.get(*decision_idx) else {
                        return Err(PatchError::new(
                            PatchRefusal::UnsupportedDeclaration,
                            "cond-expand declarations changed since library load",
                        ));
                    };
                    *decision_idx += 1;
                    let mut reqs = Vec::with_capacity(clauses.len());
                    for clause in clauses {
                        let clause_items = clause
                            .list()
                            .filter(|l| !l.is_empty())
                            .ok_or_else(|| PatchError::new(PatchRefusal::SyntaxError, "cond-expand: bad clause"))?;
                        reqs.push(&clause_items[0]);
                    }
                    if expected.requirements.len() != reqs.len()
                        || !expected.requirements.iter().zip(reqs.iter()).all(|(a, b)| sexp_eq_ignore_pos(a, b))
                    {
                        return Err(PatchError::new(
                            PatchRefusal::UnsupportedDeclaration,
                            "cond-expand clauses changed since library load",
                        ));
                    }
                    let (current_chosen, chosen_decls) = self.eval_cond_expand_pure(clauses)?;
                    if current_chosen != expected.chosen {
                        return Err(PatchError::new(
                            PatchRefusal::UnsupportedDeclaration,
                            "cond-expand branch diverged from load-time selection; use a full reload or snapshot swap",
                        ));
                    }
                    crate::nested(|| self.parse_patch_declarations(&chosen_decls, recorded, decision_idx, exports, imports, body))?;
                }
                _ => {
                    return Err(PatchError::new(
                        PatchRefusal::UnsupportedDeclaration,
                        format!("define-library: unsupported declaration {}", display_sexp(decl)),
                    ));
                }
            }
        }
        Ok(())
    }

    fn eval_cond_expand_pure(&self, clauses: &[Sexp]) -> Result<(Option<usize>, Vec<Sexp>), PatchError> {
        for (idx, clause) in clauses.iter().enumerate() {
            let items = clause
                .list()
                .filter(|l| !l.is_empty())
                .ok_or_else(|| PatchError::new(PatchRefusal::SyntaxError, "cond-expand: bad clause"))?;
            if items[0].is_sym("else") || self.cond_expand_req_pure(&items[0])? {
                return Ok((Some(idx), items[1..].to_vec()));
            }
        }
        Ok((None, Vec::new()))
    }

    fn cond_expand_req_pure(&self, req: &Sexp) -> Result<bool, PatchError> {
        crate::nested(|| self.cond_expand_req_pure_step(req))
    }

    fn cond_expand_req_pure_step(&self, req: &Sexp) -> Result<bool, PatchError> {
        if let Some(s) = req.sym() {
            return Ok(FEATURES.contains(&&*symbol_name(strip(s))));
        }
        let items = req
            .list()
            .filter(|l| !l.is_empty())
            .ok_or_else(|| PatchError::new(PatchRefusal::SyntaxError, "cond-expand: bad requirement"))?;
        let head = items[0].sym().map(|s| symbol_name(strip(s))).unwrap_or_else(|| "".into());
        match &*head {
            "and" => {
                for r in &items[1..] {
                    if !self.cond_expand_req_pure(r)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            "or" => {
                for r in &items[1..] {
                    if self.cond_expand_req_pure(r)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            "not" if items.len() == 2 => Ok(!self.cond_expand_req_pure(&items[1])?),
            "library" if items.len() == 2 => {
                let parts =
                    name_parts(&items[1]).ok_or_else(|| PatchError::new(PatchRefusal::SyntaxError, "cond-expand: bad library name"))?;
                Ok(builtin(&parts).is_some() || self.loaded_module_by_parts(&parts).is_some())
            }
            _ => Err(PatchError::new(PatchRefusal::SyntaxError, format!("cond-expand: bad requirement {}", display_sexp(req)))),
        }
    }

    fn diff_library_body(
        &self,
        m: u32,
        lib_display: &str,
        snapshot: &LibrarySnapshot,
        new_forms: &[Sexp],
    ) -> Result<(Vec<PendingProc>, usize), PatchError> {
        let old_forms = &snapshot.body_forms;

        // Index old forms by structural kind, keeping combined sequence order
        // for all non-procedure top-level forms (state initializers and expressions).
        let mut old_records: Vec<(Option<u32>, &Sexp)> = Vec::new();
        let mut old_macros: Vec<(Option<u32>, &Sexp)> = Vec::new();
        let mut old_procs: FxHashMap<u32, &Sexp> = FxHashMap::default();
        let mut old_values: FxHashMap<u32, &Sexp> = FxHashMap::default();
        let mut old_non_proc: Vec<&Sexp> = Vec::new();

        for form in old_forms {
            match classify_form(self, m, form) {
                FormKind::RecordType(name) => old_records.push((name, form)),
                FormKind::Macro(name) => old_macros.push((name, form)),
                FormKind::DefineProc { name, .. } => {
                    old_procs.insert(name, form);
                }
                FormKind::DefineValue { name } => {
                    old_values.insert(name, form);
                    old_non_proc.push(form);
                }
                FormKind::Include | FormKind::OtherExpr => old_non_proc.push(form),
            }
        }

        // Index new forms, rejecting duplicate definitions and `include`.
        let mut new_records: Vec<(Option<u32>, &Sexp)> = Vec::new();
        let mut new_macros: Vec<(Option<u32>, &Sexp)> = Vec::new();
        let mut new_procs: Vec<(u32, Option<Rc<str>>, &Sexp)> = Vec::new();
        let mut new_values: Vec<(u32, &Sexp)> = Vec::new();
        let mut new_others_count: usize = 0;
        let mut new_non_proc: Vec<&Sexp> = Vec::new();
        let mut seen_defs: FxHashSet<u32> = FxHashSet::default();

        for form in new_forms {
            match classify_form(self, m, form) {
                FormKind::Include => {
                    return Err(PatchError::new(
                        PatchRefusal::IncludeNotSupported,
                        format!("library {lib_display}: `include` is not supported in live patches"),
                    ));
                }
                FormKind::RecordType(name) => {
                    if let Some(sym) = name
                        && !seen_defs.insert(sym)
                    {
                        let n = symbol_name(sym).to_string();
                        return Err(PatchError::with_binding(
                            PatchRefusal::SyntaxError,
                            &n,
                            format!("duplicate definition of `{n}` in {lib_display}"),
                        ));
                    }
                    new_records.push((name, form));
                }
                FormKind::Macro(name) => {
                    if let Some(sym) = name
                        && !seen_defs.insert(sym)
                    {
                        let n = symbol_name(sym).to_string();
                        return Err(PatchError::with_binding(
                            PatchRefusal::SyntaxError,
                            &n,
                            format!("duplicate definition of `{n}` in {lib_display}"),
                        ));
                    }
                    new_macros.push((name, form));
                }
                FormKind::DefineProc { name, var_doc } => {
                    if !seen_defs.insert(name) {
                        let n = symbol_name(name).to_string();
                        return Err(PatchError::with_binding(
                            PatchRefusal::SyntaxError,
                            &n,
                            format!("duplicate definition of `{n}` in {lib_display}"),
                        ));
                    }
                    new_procs.push((name, var_doc, form));
                }
                FormKind::DefineValue { name } => {
                    if !seen_defs.insert(name) {
                        let n = symbol_name(name).to_string();
                        return Err(PatchError::with_binding(
                            PatchRefusal::SyntaxError,
                            &n,
                            format!("duplicate definition of `{n}` in {lib_display}"),
                        ));
                    }
                    new_values.push((name, form));
                    new_non_proc.push(form);
                }
                FormKind::OtherExpr => {
                    new_others_count += 1;
                    new_non_proc.push(form);
                }
            }
        }

        let mut unchanged: usize = 0;

        // 1. Check define-record-type forms (added, removed, or changed -> record-layout-changed).
        check_named_structural(
            &old_records,
            &new_records,
            PatchRefusal::RecordLayoutChanged,
            "define-record-type",
            lib_display,
            &mut unchanged,
        )?;

        // 2. Check define-syntax forms (added, removed, or changed -> macro-changed).
        check_named_structural(&old_macros, &new_macros, PatchRefusal::MacroChanged, "define-syntax", lib_display, &mut unchanged)?;

        // 3. Check if any previously defined binding (procedure or value) was removed.
        let new_proc_names: FxHashSet<u32> = new_procs.iter().map(|(n, _, _)| *n).collect();
        let new_val_names: FxHashSet<u32> = new_values.iter().map(|(n, _)| *n).collect();

        for &old_name in old_procs.keys().chain(old_values.keys()) {
            if !new_proc_names.contains(&old_name) && !new_val_names.contains(&old_name) {
                let n = symbol_name(old_name).to_string();
                return Err(PatchError::with_binding(
                    PatchRefusal::BindingRemoved,
                    &n,
                    format!("binding `{n}` was removed from {lib_display}"),
                ));
            }
        }

        // 4. Check non-lambda top-level defines (added or changed -> state-initializer-changed).
        for (name, new_form) in &new_values {
            let n = symbol_name(*name).to_string();
            match old_values.get(name) {
                Some(old_form) if sexp_eq_ignore_pos(old_form, new_form) => {
                    unchanged += 1;
                }
                Some(_) => {
                    return Err(PatchError::with_binding(
                        PatchRefusal::StateInitializerChanged,
                        &n,
                        format!("state initializer for `{n}` changed in {lib_display}"),
                    ));
                }
                None => {
                    return Err(PatchError::with_binding(
                        PatchRefusal::StateInitializerChanged,
                        &n,
                        format!("non-procedure definition `{n}` added or changed kind in {lib_display}"),
                    ));
                }
            }
        }

        // 5. Check exact sequence order of all non-procedure top-level forms
        //    (both state initializers and top-level expressions).
        if old_non_proc.len() != new_non_proc.len() || !old_non_proc.iter().zip(new_non_proc.iter()).all(|(a, b)| sexp_eq_ignore_pos(a, b))
        {
            return Err(PatchError::new(
                PatchRefusal::StateInitializerChanged,
                format!("top-level state initializers or expressions changed in {lib_display}"),
            ));
        }
        unchanged += new_others_count;

        // 6. Check procedure definitions.
        let mut changes = Vec::new();
        for (name, var_doc, new_form) in new_procs {
            let n = symbol_name(name).to_string();
            if old_values.contains_key(&name) {
                return Err(PatchError::with_binding(
                    PatchRefusal::StateInitializerChanged,
                    &n,
                    format!("binding `{n}` changed from value initializer to procedure in {lib_display}"),
                ));
            }
            if let Some(old_form) = old_procs.get(&name) {
                if sexp_eq_ignore_pos(old_form, new_form) {
                    unchanged += 1;
                    continue;
                }
                // Changed procedure: verify module `m` owns its global slot.
                let slot = match self.bindings.get(&(m, name)) {
                    Some(GlobalBinding::Var(slot)) => *slot,
                    _ => {
                        if self.modules[m as usize].imports.contains_key(&name) {
                            return Err(PatchError::with_binding(
                                PatchRefusal::ExportRebound,
                                &n,
                                format!("binding `{n}` is imported from another library and not owned by {lib_display}"),
                            ));
                        }
                        return Err(PatchError::with_binding(
                            PatchRefusal::BindingAdded,
                            &n,
                            format!("binding `{n}` has no existing variable slot in {lib_display}"),
                        ));
                    }
                };
                if self.global_module[slot as usize] != m {
                    return Err(PatchError::with_binding(
                        PatchRefusal::ExportRebound,
                        &n,
                        format!("slot for `{n}` is owned by another library, not {lib_display}"),
                    ));
                }
                // Verify the slot still holds the installed closed procedure (not repurposed
                // as state or wrapped at runtime via `set!`).
                let current_code_id = self.closed_closure_code_id(self.globals[slot as usize]);
                if current_code_id.is_none() || snapshot.installed_procs.get(&slot).copied() != current_code_id {
                    return Err(PatchError::with_binding(
                        PatchRefusal::StateInitializerChanged,
                        &n,
                        format!("binding `{n}` in {lib_display} was mutated at runtime and no longer holds its installed procedure"),
                    ));
                }
                changes.push(PendingProc { name: n, slot, form: new_form.clone(), var_doc });
            } else {
                // Not previously defined as a procedure in this library.
                // Check if it collides with an imported/re-exported slot owned by another library!
                match self.lookup_global(m, name) {
                    Some(GlobalBinding::Var(slot)) if self.global_module[slot as usize] != m => {
                        return Err(PatchError::with_binding(
                            PatchRefusal::ExportRebound,
                            &n,
                            format!("cannot patch `{n}` in {lib_display}: binding is owned by another library"),
                        ));
                    }
                    Some(GlobalBinding::Macro(_)) => {
                        let reason = if self.modules[m as usize].imports.contains_key(&name) {
                            PatchRefusal::ExportRebound
                        } else {
                            PatchRefusal::MacroChanged
                        };
                        return Err(PatchError::with_binding(
                            reason,
                            &n,
                            format!("cannot replace macro `{n}` with a procedure in {lib_display}"),
                        ));
                    }
                    _ => {
                        return Err(PatchError::with_binding(
                            PatchRefusal::BindingAdded,
                            &n,
                            format!("adding new binding `{n}` to {lib_display} requires a full reload or snapshot swap"),
                        ));
                    }
                }
            }
        }

        Ok((changes, unchanged))
    }
}

fn check_exports(old: &[(u32, u32)], new: &[(u32, u32)]) -> Result<(), PatchError> {
    let mut old_by_ext: FxHashMap<u32, u32> = FxHashMap::default();
    for &(int, ext) in old {
        old_by_ext.insert(ext, int);
    }
    let mut new_by_ext: FxHashMap<u32, u32> = FxHashMap::default();
    let mut new_internals: FxHashSet<u32> = FxHashSet::default();
    for &(int, ext) in new {
        if let Some(prev) = new_by_ext.insert(ext, int)
            && prev != int
        {
            let name = symbol_name(ext).to_string();
            return Err(PatchError::with_binding(PatchRefusal::ExportRebound, &name, format!("duplicate conflicting export `{name}`")));
        }
        new_internals.insert(int);
    }

    // Check removals vs renames/rebinds.
    for (&ext, &old_int) in &old_by_ext {
        match new_by_ext.get(&ext) {
            None => {
                let name = symbol_name(ext).to_string();
                if new_internals.contains(&old_int) {
                    return Err(PatchError::with_binding(PatchRefusal::ExportRebound, &name, format!("export `{name}` was renamed")));
                }
                return Err(PatchError::with_binding(PatchRefusal::BindingRemoved, &name, format!("export `{name}` was removed")));
            }
            Some(&new_int) if new_int != old_int => {
                let name = symbol_name(ext).to_string();
                return Err(PatchError::with_binding(
                    PatchRefusal::ExportRebound,
                    &name,
                    format!("export `{name}` was rebound from `{}` to `{}`", symbol_name(old_int), symbol_name(new_int)),
                ));
            }
            Some(_) => {}
        }
    }

    // Check added exports.
    for &ext in new_by_ext.keys() {
        if !old_by_ext.contains_key(&ext) {
            let name = symbol_name(ext).to_string();
            return Err(PatchError::with_binding(PatchRefusal::ExportRebound, &name, format!("export `{name}` was added")));
        }
    }

    Ok(())
}

fn check_named_structural(
    old: &[(Option<u32>, &Sexp)],
    new: &[(Option<u32>, &Sexp)],
    reason: PatchRefusal,
    kind_label: &str,
    lib_display: &str,
    unchanged: &mut usize,
) -> Result<(), PatchError> {
    if old.len() != new.len() {
        // Pick a representative binding name if available.
        let binding = new.iter().chain(old.iter()).find_map(|(n, form)| {
            let matches_other = |list: &[(Option<u32>, &Sexp)]| list.iter().any(|(_, other)| sexp_eq_ignore_pos(form, other));
            if !(matches_other(old) && matches_other(new)) { n.map(|s| symbol_name(s).to_string()) } else { None }
        });
        return Err(match binding {
            Some(b) => PatchError::with_binding(reason, &b, format!("{kind_label} `{b}` added or removed in {lib_display}")),
            None => PatchError::new(reason, format!("{kind_label} form added or removed in {lib_display}")),
        });
    }
    let mut matched = vec![false; new.len()];
    for (old_name, old_form) in old {
        if let Some(pos) = new.iter().enumerate().position(|(i, (_, new_form))| !matched[i] && sexp_eq_ignore_pos(old_form, new_form)) {
            matched[pos] = true;
            *unchanged += 1;
        } else {
            return Err(match old_name {
                Some(s) => {
                    let b = symbol_name(*s).to_string();
                    PatchError::with_binding(reason, &b, format!("{kind_label} `{b}` changed in {lib_display}"))
                }
                None => PatchError::new(reason, format!("{kind_label} form changed in {lib_display}")),
            });
        }
    }
    Ok(())
}
