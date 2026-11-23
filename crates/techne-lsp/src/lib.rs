//! Language server for techne Lisp (stdio).
//!
//! For editors other than Techne's own, which asks the running VM instead
//! (as nREPL clients do). Diagnostics (reader errors, unbound identifiers,
//! missing requires, libraries and included files, docstrings as checkdoc
//! checks them), go-to-definition, hover
//! (signature, docstring, built-in descriptions), completion and document
//! symbols. Files are read by the VM's reader and analysed syntactically
//! (see `analysis`); no user code is run. What the runtime defines comes
//! from the plain language and a generated catalog of the runtime libraries.

mod analysis;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Mutex,
};

use analysis::{Analysis, Def, DefKind, Span, analyze, checkdoc, head, ident, library_name, list, string_lit, unbound};
use techne_vm::reader::Syntax;
use tower_lsp::{Client, LanguageServer, LspService, Server, jsonrpc::Result, lsp_types::*};

/// A 2-part `(owner pkg)` package identity in a manifest datum.
fn package_id(form: &Syntax) -> Option<(String, String)> {
    let items = list(form)?;
    if items.len() != 2 {
        return None;
    }
    Some((ident(&items[0])?.0, ident(&items[1])?.0))
}

/// The content-addressed package store directory, matching `(package-store-dir)`
/// in `(techne lock)`: `$TECHNE_PACKAGE_STORE`, else `$XDG_DATA_HOME/techne/packages/store`,
/// else `$HOME/.local/share/techne/packages/store`.
fn package_store_dir() -> Option<PathBuf> {
    if let Some(env) = std::env::var_os("TECHNE_PACKAGE_STORE") {
        let p = PathBuf::from(env);
        if p.is_absolute() {
            return Some(p);
        }
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        let p = PathBuf::from(xdg);
        if p.is_absolute() {
            return Some(p.join("techne/packages/store"));
        }
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/techne/packages/store"))
}

/// Whether a library name segment is a safe, non-hidden path component.
fn valid_lib_part(part: &str) -> bool {
    !part.is_empty() && !part.starts_with('.') && !part.contains('/') && !part.contains('\\') && !part.contains('\0') && part != ".."
}

/// Resolve `<package>/lib/<parts>.sld`, verifying every name component and
/// canonical containment inside `<package>/lib`.
fn contained_package_library_file(package: &Path, parts: &[String]) -> Option<PathBuf> {
    if !parts.iter().all(|p| valid_lib_part(p)) {
        return None;
    }
    let lib_dir = package.join("lib").canonicalize().ok()?;
    let candidate = lib_dir.join(format!("{}.sld", parts.join("/"))).canonicalize().ok()?;
    (candidate.is_file() && candidate.starts_with(&lib_dir)).then_some(candidate)
}

/// Resolve a relative directory path `rel` against `base`, rejecting absolute
/// paths and (when `boundary` is given) any canonical path escaping `boundary`.
fn safe_relative_dir(base: &Path, rel: &str, boundary: Option<&Path>) -> Option<PathBuf> {
    if rel.is_empty() || rel.contains('\0') || rel.contains('\\') {
        return None;
    }
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() || rel_path.components().any(|c| matches!(c, std::path::Component::RootDir | std::path::Component::Prefix(_)))
    {
        return None;
    }
    let canon = base.join(rel_path).canonicalize().ok()?;
    if let Some(bound) = boundary {
        let canon_bound = bound.canonicalize().ok()?;
        if !canon.starts_with(&canon_bound) {
            return None;
        }
    }
    Some(canon)
}

/// Whether `path` resides inside the content-addressed package store.
fn is_inside_store(path: &Path) -> bool {
    let Some(store) = package_store_dir() else { return false };
    let store = store.canonicalize().unwrap_or(store);
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    canon.starts_with(&store)
}

/// Read and parse `dir/techne-package.scm` as pure data, returning its top-level
/// `(package ...)` form elements if valid.
fn read_manifest_forms(dir: &Path) -> Option<Vec<Syntax>> {
    let text = std::fs::read_to_string(dir.join("techne-package.scm")).ok()?;
    let forms = techne_vm::reader::read_syntax(&text).ok()?;
    let manifest = list(forms.first()?)?;
    if head(manifest).as_deref() != Some("package") {
        return None;
    }
    Some(manifest.to_vec())
}

/// Locate the consuming workspace/project root that owns `techne.lock` and
/// root `(override ...)` declarations, kept distinct from `pkg_root` (the
/// nearest manifest above the file being analysed).
fn find_project_root(dir: &Path, pkg_root: &Path, workspace_roots: &[PathBuf]) -> Option<PathBuf> {
    let canon_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let canon_pkg = pkg_root.canonicalize().unwrap_or_else(|_| pkg_root.to_path_buf());
    let enclosing_ws = workspace_roots.iter().find(|ws| canon_dir.starts_with(ws));

    if let Some(ws) = enclosing_ws {
        for anc in canon_dir.ancestors() {
            if !anc.starts_with(ws) {
                break;
            }
            if anc.join("techne.lock").is_file() {
                return Some(anc.to_path_buf());
            }
        }
        if ws.join("techne.lock").is_file() {
            return Some(ws.clone());
        }
    }
    // Files opened inside `store/<tree-sha256>/` or outside the workspace
    // still resolve against the open workspace project's `techne.lock`.
    if let Some(ws) = workspace_roots.iter().find(|ws| ws.join("techne.lock").is_file()) {
        return Some(ws.clone());
    }
    if let Some(anc) = canon_dir.ancestors().find(|d| d.join("techne.lock").is_file()) {
        return Some(anc.to_path_buf());
    }
    // Without a lockfile, a package inside the content-addressed store is still
    // a dependency and never acts as a root project for overrides.
    if is_inside_store(&canon_pkg) {
        return None;
    }
    Some(canon_pkg)
}

/// Find the materialized directory of `(owner pkg)` from `lock_root/techne.lock`,
/// reading the lockfile as pure data without evaluating anything.
///
/// Rejects absolute paths and relative `(path ...)` entries escaping the
/// enclosing workspace root (or `lock_root` when no workspace root encloses it).
fn locked_package_dir(lock_root: &Path, workspace_roots: &[PathBuf], owner: &str, pkg: &str) -> Option<PathBuf> {
    let text = std::fs::read_to_string(lock_root.join("techne.lock")).ok()?;
    let forms = techne_vm::reader::read_syntax(&text).ok()?;
    let lock = list(forms.first()?)?;
    if head(lock).as_deref() != Some("lock") {
        return None;
    }
    let canon_lock_root = lock_root.canonicalize().unwrap_or_else(|_| lock_root.to_path_buf());
    let boundary = workspace_roots.iter().find(|ws| canon_lock_root.starts_with(ws)).map(PathBuf::as_path).unwrap_or(&canon_lock_root);
    for clause in lock.iter().skip(1).filter_map(list) {
        if head(clause).as_deref() != Some("package") {
            continue;
        }
        if !clause.get(1).and_then(package_id).is_some_and(|(o, p)| o == owner && p == pkg) {
            continue;
        }
        let mut source_path: Option<String> = None;
        let mut is_git_source = false;
        let mut tree_hash: Option<String> = None;
        for field in clause.iter().skip(3).filter_map(list) {
            match head(field).as_deref() {
                Some("source") => {
                    if let Some(src) = field.get(1).and_then(list) {
                        match head(src).as_deref() {
                            Some("path") => source_path = src.get(1).and_then(string_lit),
                            Some("github" | "git" | "local") => is_git_source = true,
                            _ => {}
                        }
                    }
                }
                Some("tree-sha256") => {
                    tree_hash = field.get(1).and_then(string_lit);
                }
                _ => {}
            }
        }
        if let Some(rel) = source_path {
            return safe_relative_dir(lock_root, &rel, Some(boundary));
        }
        if is_git_source {
            let hash = tree_hash?;
            if hash.len() == 64 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
                return package_store_dir().map(|store| store.join(hash));
            }
        }
    }
    None
}

/// Find an entry for `(owner pkg)` inside a `(depends ...)` or `(override ...)` clause.
fn manifest_clause_source<'a>(manifest: &'a [Syntax], clause_name: &str, owner: &str, pkg: &str) -> Option<&'a [Syntax]> {
    manifest
        .iter()
        .skip(2)
        .filter_map(list)
        .filter(|clause| head(clause).as_deref() == Some(clause_name))
        .flat_map(|clause| clause.iter().skip(1))
        .filter_map(|entry| {
            let entry = list(entry)?;
            let matches = package_id(entry.first()?).is_some_and(|(o, p)| o == owner && p == pkg);
            matches.then(|| list(entry.get(1)?)).flatten()
        })
        .next()
}

/// Where a file in `dir` finds library `parts` if a package provides it,
/// as the runtime does once the file's package is used (`(techne
/// package)` / `(techne lock)`): the package the file is in, by the nearest
/// manifest above, `techne-package.scm`, for the libraries of its own
/// `(owner pkg)`, under its `lib`; or a package that manifest directly
/// depends on, resolved via the consuming project root's `(override ...)`
/// and `techne.lock` + content-addressed store, or unlocked `(path "...")`.
///
/// Direct-import permission always comes from the importing file's nearest
/// `techne-package.scm`, while `techne.lock` and root `(override ...)` come
/// solely from the consuming workspace/project root (never from a dependency
/// manifest inside `store/` or a nested package).
fn package_library(parts: &[String], dir: &Path, workspace_roots: &[PathBuf]) -> Option<Option<PathBuf>> {
    let [owner, pkg, ..] = parts else { return None };
    let pkg_root = dir.ancestors().find(|d| d.join("techne-package.scm").is_file())?;
    let manifest = read_manifest_forms(pkg_root)?;
    if package_id(manifest.get(1)?).is_some_and(|(o, p)| o == *owner && p == *pkg) {
        return Some(contained_package_library_file(pkg_root, parts));
    }
    let dep_source = manifest_clause_source(&manifest, "depends", owner, pkg)?;
    let project_root = find_project_root(dir, pkg_root, workspace_roots);
    if let Some(proj) = project_root.as_deref() {
        // Only the consuming project root's `(override ...)` is honored.
        if let Some(proj_manifest) = read_manifest_forms(proj)
            && let Some(ov) = manifest_clause_source(&proj_manifest, "override", owner, pkg)
            && head(ov).as_deref() == Some("path")
            && let Some(rel) = ov.get(1).and_then(string_lit)
            && let Some(ov_dir) = safe_relative_dir(proj, &rel, None)
        {
            return Some(contained_package_library_file(&ov_dir, parts));
        }
        // Locked dependency from the consuming project's `techne.lock` + store.
        if let Some(locked_dir) = locked_package_dir(proj, workspace_roots, owner, pkg) {
            return Some(contained_package_library_file(&locked_dir, parts));
        }
    }
    // Unlocked fallback for local `(path ...)` dependencies (never for immutable store packages).
    if !is_inside_store(pkg_root)
        && head(dep_source).as_deref() == Some("path")
        && let Some(rel) = dep_source.get(1).and_then(string_lit)
        && let Some(dep_dir) = safe_relative_dir(pkg_root, &rel, None)
    {
        return Some(contained_package_library_file(&dep_dir, parts));
    }
    Some(None)
}

/// Byte offset to an LSP position (UTF-16 columns).
/// Byte `offset` of `text`, or the start of the character it falls in.
fn boundary(text: &str, offset: usize) -> usize {
    (0..=offset.min(text.len())).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0)
}

/// The position of byte `offset` of `text`, or of the character it falls in.
fn position(text: &str, offset: u32) -> Position {
    let offset = boundary(text, offset as usize);
    let before = &text[..offset];
    let line = before.matches('\n').count() as u32;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let character = text[line_start..offset].encode_utf16().count() as u32;
    Position { line, character }
}

fn offset(text: &str, pos: Position) -> u32 {
    let mut line_start = 0;
    for _ in 0..pos.line {
        match text[line_start..].find('\n') {
            Some(i) => line_start += i + 1,
            None => return text.len() as u32,
        }
    }
    let line = &text[line_start..text[line_start..].find('\n').map_or(text.len(), |i| line_start + i)];
    let mut units = 0;
    for (i, c) in line.char_indices() {
        if units >= pos.character {
            return (line_start + i) as u32;
        }
        units += c.len_utf16() as u32;
    }
    (line_start + line.len()) as u32
}

fn range(text: &str, span: Span) -> Range {
    Range { start: position(text, span.0), end: position(text, span.1) }
}

/// A definition from another (required) file.
struct External {
    uri: Url,
    text: String,
    def: Def,
}

struct Backend {
    client: Client,
    docs: Mutex<HashMap<Url, String>>,
    workspace_roots: Mutex<Vec<PathBuf>>,
    builtins: HashMap<String, String>,
    /// The runtime's macros (from `builtins`).
    builtin_macros: HashSet<String>,
    /// Every name the runtime defines, internal ones included: not unbound.
    runtime_names: HashSet<String>,
}

impl Backend {
    fn text(&self, uri: &Url) -> Option<String> {
        self.docs.lock().unwrap().get(uri).cloned()
    }

    fn workspace_roots(&self) -> Vec<PathBuf> {
        self.workspace_roots.lock().unwrap().clone()
    }

    /// The analysis of a file, with the definitions of the files it
    /// requires (their macros taken into account) and the requires that
    /// were not found.
    fn analysis(&self, uri: &Url, text: &str) -> (Analysis, Vec<External>, Vec<(String, Span)>) {
        let a = analyze(text, &self.builtin_macros);
        let (externals, missing) = self.externals(uri, &a);
        let external_macros: Vec<String> = externals.iter().filter(|e| e.def.kind == DefKind::Macro).map(|e| e.def.name.clone()).collect();
        if external_macros.is_empty() {
            return (a, externals, missing);
        }
        let macros: HashSet<String> = self.builtin_macros.iter().cloned().chain(external_macros).collect();
        (analyze(text, &macros), externals, missing)
    }

    /// Exported top-level definitions of the files `a` requires.
    fn externals(&self, uri: &Url, a: &Analysis) -> (Vec<External>, Vec<(String, Span)>) {
        let base = uri.to_file_path().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();
        let mut out = Vec::new();
        let mut missing = Vec::new();
        for (spec, span) in &a.requires {
            let path = base.join(spec);
            let Ok(text) = std::fs::read_to_string(&path) else {
                missing.push((format!("cannot find module {spec}"), *span));
                continue;
            };
            let Ok(file_uri) = Url::from_file_path(path.canonicalize().unwrap_or(path)) else { continue };
            let other = analyze(&text, &self.builtin_macros);
            for def in other.top_level() {
                if other.provides.is_empty() || other.provides.contains(&def.name) {
                    out.push(External { uri: file_uri.clone(), text: text.clone(), def: def.clone() });
                }
            }
        }
        for (name, span) in &a.includes {
            match included(&base, name, &self.builtin_macros) {
                Some(defs) => out.extend(defs),
                None => missing.push((format!("cannot find included file {name}"), *span)),
            }
        }
        let here = Here { uri, text: &self.text(uri).unwrap_or_default(), a };
        for set in &a.imports {
            match self.import_set(&base, &here, set) {
                Ok(defs) => out.extend(defs),
                Err(name) => missing.push((name, set.span)),
            }
        }
        (out, missing)
    }

    /// The definitions an R7RS import set brings in, as the VM finds them:
    /// a library defined in this file, or `a/b.sld` beside it, on
    /// `TECHNE_LIBRARY_PATH` or among Techne's own. The built-in libraries
    /// (`(scheme ...)`, `(srfi 69)`, `(techne)`) bring builtins, known
    /// anyway. Err: a library not found.
    fn import_set(&self, base: &std::path::Path, here: &Here, set: &Syntax) -> std::result::Result<Vec<External>, String> {
        let items = list(set).unwrap_or(&[]);
        let names = |from: usize| -> Vec<String> { items[from.min(items.len())..].iter().filter_map(ident).map(|(n, _)| n).collect() };
        let inner = || items.get(1).map_or(Ok(vec![]), |s| self.import_set(base, here, s));
        let renamed = |defs: Vec<External>, f: &dyn Fn(&str) -> Option<String>| -> Vec<External> {
            defs.into_iter()
                .filter_map(|mut e| {
                    e.def.name = f(&e.def.name)?;
                    Some(e)
                })
                .collect()
        };
        Ok(match head(items).as_deref() {
            Some("only") => {
                let keep = names(2);
                renamed(inner()?, &|n| keep.iter().any(|k| k == n).then(|| n.to_string()))
            }
            Some("except") => {
                let drop = names(2);
                renamed(inner()?, &|n| (!drop.iter().any(|k| k == n)).then(|| n.to_string()))
            }
            Some("prefix") => {
                let prefix = names(2).pop().unwrap_or_default();
                renamed(inner()?, &|n| Some(format!("{prefix}{n}")))
            }
            Some("rename") => {
                let pairs: Vec<(String, String)> = items[2.min(items.len())..]
                    .iter()
                    .filter_map(|p| match list(p).map(|l| l.iter().filter_map(ident).map(|(n, _)| n).collect::<Vec<_>>()) {
                        Some(v) if v.len() == 2 => Some((v[0].clone(), v[1].clone())),
                        _ => None,
                    })
                    .collect();
                renamed(inner()?, &|n| Some(pairs.iter().find(|(f, _)| f == n).map_or(n.to_string(), |(_, t)| t.clone())))
            }
            _ => {
                let parts = library_name(set);
                let written = format!("({})", parts.join(" "));
                if matches!(parts.first().map(String::as_str), Some("scheme")) || techne_vm::library::is_builtin(&parts) {
                    return Ok(vec![]);
                }
                // A library this file defines: its definitions are here.
                if let Some((_, exports)) = here.a.libraries.iter().find(|(n, _)| *n == parts) {
                    let defs: Vec<External> = here
                        .a
                        .top_level()
                        .map(|d| External { uri: here.uri.clone(), text: here.text.to_string(), def: d.clone() })
                        .collect();
                    return Ok(exported(exports, defs));
                }
                // A package's, where the package has it and nowhere else;
                // any other where the library path finds it.
                let ws_roots = self.workspace_roots();
                let file = package_library(&parts, base, &ws_roots)
                    .unwrap_or_else(|| techne_vm::library::library_file(&parts, base))
                    .ok_or_else(|| format!("cannot find library {written}"))?;
                let text = std::fs::read_to_string(&file).map_err(|_| format!("cannot read library {written}"))?;
                let uri = Url::from_file_path(file.canonicalize().unwrap_or(file)).map_err(|_| written.clone())?;
                let other = analyze(&text, &self.builtin_macros);
                let exports = other.libraries.iter().find(|(n, _)| *n == parts).map(|(_, e)| e.clone()).unwrap_or_default();
                let dir = uri.to_file_path().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();
                let mut defs: Vec<External> =
                    other.top_level().map(|d| External { uri: uri.clone(), text: text.clone(), def: d.clone() }).collect();
                for (name, _) in &other.includes {
                    defs.extend(included(&dir, name, &self.builtin_macros).unwrap_or_default());
                }
                exported(&exports, defs)
            }
        })
    }

    async fn publish(&self, uri: Url, text: String) {
        let diagnostics = self.diagnostics(&uri, &text);
        self.client.publish_diagnostics(uri, diagnostics, None).await;
    }

    fn diagnostics(&self, uri: &Url, text: &str) -> Vec<Diagnostic> {
        let text = text.to_string();
        let (a, externals, missing) = self.analysis(uri, &text);
        let mut diagnostics = Vec::new();
        if let Some((msg, pos)) = &a.error {
            // The character the error is at, whole.
            let start = boundary(&text, *pos as usize);
            let end = text[start..].chars().next().map_or(start, |c| start + c.len_utf8());
            diagnostics.push(Diagnostic {
                range: Range { start: position(&text, start as u32), end: position(&text, end as u32) },
                severity: Some(DiagnosticSeverity::ERROR),
                message: msg.clone(),
                source: Some("techne".into()),
                ..Diagnostic::default()
            });
        } else {
            for (spec, span) in missing {
                diagnostics.push(Diagnostic {
                    range: range(&text, span),
                    severity: Some(DiagnosticSeverity::ERROR),
                    message: spec,
                    source: Some("techne".into()),
                    ..Diagnostic::default()
                });
            }
            let known: HashSet<String> = self
                .runtime_names
                .iter()
                .chain(self.builtins.keys())
                .cloned()
                .chain(externals.iter().map(|e| e.def.name.clone()))
                .collect();
            for (message, span) in checkdoc(&text, &a) {
                diagnostics.push(Diagnostic {
                    range: range(&text, span),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message,
                    source: Some("checkdoc".into()),
                    ..Diagnostic::default()
                });
            }
            for r in unbound(&a, &known) {
                diagnostics.push(Diagnostic {
                    range: range(&text, r.span),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: format!("unbound identifier: {}", r.name),
                    source: Some("techne".into()),
                    ..Diagnostic::default()
                });
            }
        }
        diagnostics
    }
}

/// The file being analysed, for libraries it defines itself.
struct Here<'a> {
    uri: &'a Url,
    text: &'a str,
    a: &'a Analysis,
}

/// The top-level definitions of an included file (relative to `dir`).
fn included(dir: &std::path::Path, name: &str, macros: &HashSet<String>) -> Option<Vec<External>> {
    let path = dir.join(name);
    let text = std::fs::read_to_string(&path).ok()?;
    let uri = Url::from_file_path(path.canonicalize().unwrap_or(path)).ok()?;
    let a = analyze(&text, macros);
    Some(a.top_level().map(|d| External { uri: uri.clone(), text: text.clone(), def: d.clone() }).collect())
}

/// The definitions a library's exports `(inside, outside)` name, by the
/// names importers see.
fn exported(exports: &[(String, String)], defs: Vec<External>) -> Vec<External> {
    exports
        .iter()
        .filter_map(|(inside, outside)| {
            let e = defs.iter().find(|e| e.def.name == *inside)?;
            Some(External { uri: e.uri.clone(), text: e.text.clone(), def: Def { name: outside.clone(), ..e.def.clone() } })
        })
        .collect()
}

fn hover_text(def: &Def) -> String {
    let header = def.signature.clone().unwrap_or_else(|| def.name.clone());
    let kind = match def.kind {
        DefKind::Function => "procedure",
        DefKind::Variable => "variable",
        DefKind::Macro => "syntax",
        DefKind::Record => "record type",
        DefKind::Generic => "generic function",
        DefKind::Local => "local",
    };
    let mut s = format!("```scheme\n{header}\n```\n{kind}");
    if let Some(doc) = &def.doc {
        s.push_str("\n\n");
        s.push_str(doc);
    }
    s
}

fn completion_kind(kind: DefKind) -> CompletionItemKind {
    match kind {
        DefKind::Function | DefKind::Generic => CompletionItemKind::FUNCTION,
        DefKind::Macro => CompletionItemKind::KEYWORD,
        DefKind::Record => CompletionItemKind::STRUCT,
        DefKind::Variable | DefKind::Local => CompletionItemKind::VARIABLE,
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let mut roots = Vec::new();
        let mut push_root = |p: PathBuf| {
            let p = p.canonicalize().unwrap_or(p);
            if !roots.contains(&p) {
                roots.push(p);
            }
        };
        if let Some(folders) = params.workspace_folders {
            for folder in folders {
                if let Ok(path) = folder.uri.to_file_path() {
                    push_root(path);
                }
            }
        }
        #[allow(deprecated)]
        if let Some(uri) = params.root_uri
            && let Ok(path) = uri.to_file_path()
        {
            push_root(path);
        }
        #[allow(deprecated)]
        if let Some(path) = params.root_path {
            push_root(PathBuf::from(path));
        }
        *self.workspace_roots.lock().unwrap() = roots;
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                completion_provider: Some(CompletionOptions::default()),
                document_symbol_provider: Some(OneOf::Left(true)),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo { name: "techne-lsp".into(), version: None }),
        })
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, p: DidOpenTextDocumentParams) {
        let uri = p.text_document.uri;
        self.docs.lock().unwrap().insert(uri.clone(), p.text_document.text.clone());
        self.publish(uri, p.text_document.text).await;
    }

    async fn did_change(&self, p: DidChangeTextDocumentParams) {
        if let Some(change) = p.content_changes.into_iter().last() {
            let uri = p.text_document.uri;
            self.docs.lock().unwrap().insert(uri.clone(), change.text.clone());
            self.publish(uri, change.text).await;
        }
    }

    async fn did_close(&self, p: DidCloseTextDocumentParams) {
        self.docs.lock().unwrap().remove(&p.text_document.uri);
        // A closed file has no diagnostics; say so, or the client's
        // squiggles for it stay.
        self.client.publish_diagnostics(p.text_document.uri, Vec::new(), None).await;
    }

    async fn hover(&self, p: HoverParams) -> Result<Option<Hover>> {
        let uri = p.text_document_position_params.text_document.uri;
        let Some(text) = self.text(&uri) else { return Ok(None) };
        let (a, externals, _) = self.analysis(&uri, &text);
        let pos = offset(&text, p.text_document_position_params.position);
        let Some((name, def)) = a.at(pos) else { return Ok(None) };
        let value = match def {
            Some(d) => hover_text(d),
            None => match externals.into_iter().find(|e| e.def.name == name) {
                Some(e) => hover_text(&e.def),
                None => match self.builtins.get(name) {
                    Some(desc) => format!("```text\n{desc}\n```"),
                    None => return Ok(None),
                },
            },
        };
        Ok(Some(Hover { contents: HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value }), range: None }))
    }

    async fn goto_definition(&self, p: GotoDefinitionParams) -> Result<Option<GotoDefinitionResponse>> {
        let uri = p.text_document_position_params.text_document.uri;
        let Some(text) = self.text(&uri) else { return Ok(None) };
        let (a, externals, _) = self.analysis(&uri, &text);
        let pos = offset(&text, p.text_document_position_params.position);
        let Some((name, def)) = a.at(pos) else { return Ok(None) };
        Ok(match def {
            Some(d) => Some(GotoDefinitionResponse::Scalar(Location { uri, range: range(&text, d.span) })),
            None => externals
                .into_iter()
                .find(|e| e.def.name == name)
                .map(|e| GotoDefinitionResponse::Scalar(Location { uri: e.uri, range: range(&e.text, e.def.span) })),
        })
    }

    async fn completion(&self, p: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = p.text_document_position.text_document.uri;
        let Some(text) = self.text(&uri) else { return Ok(None) };
        let (a, externals, _) = self.analysis(&uri, &text);
        let pos = offset(&text, p.text_document_position.position);
        let mut items: Vec<CompletionItem> = a
            .visible_at(pos)
            .map(|d| CompletionItem {
                label: d.name.clone(),
                kind: Some(completion_kind(d.kind)),
                detail: d.signature.clone(),
                ..CompletionItem::default()
            })
            .collect();
        items.extend(externals.into_iter().map(|e| CompletionItem {
            label: e.def.name.clone(),
            kind: Some(completion_kind(e.def.kind)),
            detail: e.def.signature.clone(),
            ..CompletionItem::default()
        }));
        items.extend(self.builtins.iter().map(|(name, desc)| CompletionItem {
            label: name.clone(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: desc.lines().next().map(str::to_string),
            ..CompletionItem::default()
        }));
        let mut seen = HashSet::new();
        items.retain(|i| seen.insert(i.label.clone()));
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn document_symbol(&self, p: DocumentSymbolParams) -> Result<Option<DocumentSymbolResponse>> {
        let uri = p.text_document.uri;
        let Some(text) = self.text(&uri) else { return Ok(None) };
        let a = analyze(&text, &self.builtin_macros);
        #[allow(deprecated)]
        let symbols = a
            .top_level()
            .map(|d| SymbolInformation {
                name: d.name.clone(),
                kind: match d.kind {
                    DefKind::Function | DefKind::Generic => SymbolKind::FUNCTION,
                    DefKind::Macro => SymbolKind::KEY,
                    DefKind::Record => SymbolKind::STRUCT,
                    _ => SymbolKind::VARIABLE,
                },
                tags: None,
                deprecated: None,
                location: Location { uri: uri.clone(), range: range(&text, d.span) },
                container_name: None,
            })
            .collect();
        Ok(Some(DocumentSymbolResponse::Flat(symbols)))
    }
}

/// Names the hosts add to the plain language. Generated and checked by
/// `techne-editor/tests/runtime_catalog.rs`, not by running host code in
/// the language server.
#[derive(serde::Deserialize)]
struct Catalog {
    bindings: HashMap<String, Binding>,
    root_names: HashSet<String>,
}

#[derive(serde::Deserialize)]
struct Binding {
    description: String,
    is_macro: bool,
}

fn read_catalog(path: &Path) -> std::result::Result<Catalog, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read runtime catalog {}: {e}", path.display()))?;
    let mut catalog: Catalog = serde_json::from_str(&text).map_err(|e| format!("invalid runtime catalog {}: {e}", path.display()))?;
    let lisp = path
        .parent()
        .unwrap_or(Path::new("."))
        .canonicalize()
        .map_err(|e| format!("cannot locate runtime catalog {}: {e}", path.display()))?;
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
    for binding in catalog.bindings.values_mut() {
        binding.description =
            binding.description.replace("@lisp@", &lisp.display().to_string()).replace("@source@", &source.display().to_string());
    }
    Ok(catalog)
}

/// Descriptions of everything the root and user modules define, from a VM.
fn builtin_descriptions(vm: &mut techne_vm::vm::Vm) -> HashMap<String, String> {
    let user = techne_vm::vm::USER_MODULE;
    vm.global_names(user)
        .into_iter()
        .map(|name| {
            let desc = vm.describe_binding(user, &name);
            (name.to_string(), desc)
        })
        .collect()
}

/// Serve the language server protocol on stdin/stdout until the client
/// exits: `techne lsp`.
pub fn main() -> std::process::ExitCode {
    let catalog = match read_catalog(&techne_vm::library::lisp_dir().join("runtime-catalog.json")) {
        Ok(catalog) => catalog,
        Err(e) => {
            eprintln!("techne-lsp: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
    runtime.block_on(serve(catalog));
    std::process::ExitCode::SUCCESS
}

async fn serve(catalog: Catalog) {
    let mut vm = techne_vm::vm::Vm::new();
    let mut builtins = builtin_descriptions(&mut vm);
    let mut runtime_names: HashSet<String> = vm.root_names().iter().map(|n| n.to_string()).collect();
    drop(vm);
    // The hosts' macros: the analysis does not know their shapes, so it
    // does not report names inside their uses. The plain language's are
    // understood by the analysis, or checked as calls.
    let mut builtin_macros = HashSet::new();
    for (name, binding) in catalog.bindings {
        if binding.is_macro {
            builtin_macros.insert(name.clone());
        }
        builtins.insert(name, binding.description);
    }
    runtime_names.extend(catalog.root_names);
    let (service, socket) = LspService::new(|client| Backend {
        client,
        docs: Mutex::new(HashMap::new()),
        workspace_roots: Mutex::new(Vec::new()),
        builtins: builtins.clone(),
        builtin_macros: builtin_macros.clone(),
        runtime_names: runtime_names.clone(),
    });
    Server::new(tokio::io::stdin(), tokio::io::stdout(), socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An offset within a character is its start, never a slice through it.
    #[test]
    fn positions_within_characters() {
        let text = "(\né)";
        assert_eq!(position(text, 3), Position { line: 1, character: 0 });
        assert_eq!(position(text, 4), Position { line: 1, character: 1 });
        assert_eq!(position(text, 99), Position { line: 1, character: 2 });
    }
}
