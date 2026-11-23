//! Immutable, read-only, in-memory virtual filesystem (`BundleVfs`) for
//! packaged and plain-file deployments.
//!
//! A `BundleVfs` holds zero host OS `PathBuf`s: every file lives under a
//! deterministic virtual path:
//! - `/bundle/<relpath>` for root files (plain-file deployments or root files),
//! - `/pkg/<owner>/<pkg>/<relpath>` for files belonging to package `(owner pkg)`.
//!
//! Installing a `BundleVfs` on a [`crate::vm::Vm`] locks library resolution
//! completely: `TECHNE_LIBRARY_PATH`, CWD fallback, and ambient OS filesystem
//! lookups are disabled for user and package code. Only built-ins, shipped
//! stdlib libraries under [`crate::library::lisp_dir`], and packages in the
//! bundle may load.

use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    library::{is_stdlib_package, is_valid_package_component, valid_library_part},
    vm::Error,
};

/// A canonical 2-part `(owner pkg)` package identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackagePrefix {
    pub(crate) owner: Box<str>,
    pub(crate) name: Box<str>,
}

impl PackagePrefix {
    /// Construct a 2-part `(owner pkg)` package prefix.
    pub fn new(owner: &str, name: &str) -> Self {
        Self { owner: owner.into(), name: name.into() }
    }

    /// The package owner component.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// The package name component.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether this prefix matches `(owner pkg)`.
    pub fn matches(&self, owner: &str, name: &str) -> bool {
        &*self.owner == owner && &*self.name == name
    }
}

impl fmt::Display for PackagePrefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({} {})", self.owner, self.name)
    }
}

/// An immutable package tree inside a [`BundleVfs`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundlePackage {
    /// Canonical `tree-sha256` digest of this package's files.
    pub tree_sha256: String,
    /// Package files keyed by clean relative POSIX paths (no leading `/`, no `.` / `..`, no hidden segments).
    pub files: BTreeMap<String, Arc<[u8]>>,
    /// Direct dependencies this package is allowed to import (excluding itself, which is always allowed).
    pub declared_deps: Vec<PackagePrefix>,
}

/// An immutable, read-only, `Arc`-shared deployment filesystem in memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleVfs {
    root_files: BTreeMap<String, Arc<[u8]>>,
    root_package: Option<PackagePrefix>,
    packages: BTreeMap<PackagePrefix, BundlePackage>,
}

/// Parsed virtual file location inside a [`BundleVfs`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VirtualFile<'a> {
    Root(&'a str),
    Package { owner: &'a str, pkg: &'a str, rel: &'a str },
}

/// Parsed virtual directory location inside a [`BundleVfs`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VirtualDir<'a> {
    Root(&'a str),
    Package { owner: &'a str, pkg: &'a str, rel: &'a str },
}

/// Validate a canonical 2-part `(owner pkg)` prefix for a bundle package.
pub fn validate_package_prefix(prefix: &PackagePrefix) -> Result<(), Error> {
    if !is_valid_package_component(&prefix.owner) || !is_valid_package_component(&prefix.name) {
        return Err(Error::new(format!("bundle: invalid package name {prefix}")));
    }
    if matches!(&*prefix.owner, "scheme" | "srfi") {
        return Err(Error::new(format!("bundle: namespace ({} ...) is reserved", prefix.owner)));
    }
    if is_stdlib_package(&prefix.owner, &prefix.name) {
        return Err(Error::new(format!("bundle: libraries ({} {} ...) are part of Techne's own libraries", prefix.owner, prefix.name)));
    }
    Ok(())
}

/// Validate a clean relative POSIX file path for storage in a [`BundleVfs`]:
/// non-empty, `/`-separated, no leading or trailing `/`, no `\` or NUL,
/// no `.` or `..` or empty components, and no dot-prefixed hidden components.
pub fn validate_bundle_relpath(rel: &str) -> Result<(), Error> {
    if rel.is_empty() || rel.starts_with('/') || rel.ends_with('/') || rel.bytes().any(|b| matches!(b, b'\\' | b'\0')) {
        return Err(Error::new(format!("bundle: invalid relative path: {rel}")));
    }
    for seg in rel.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(Error::new(format!("bundle: invalid relative path: {rel}")));
        }
        if seg.starts_with('.') {
            return Err(Error::new(format!("bundle: hidden path not allowed: {rel}")));
        }
    }
    Ok(())
}

/// Format the deterministic virtual path for a root file: `/bundle/<rel>`.
pub fn root_virtual_path(rel: &str) -> PathBuf {
    PathBuf::from(format!("/bundle/{rel}"))
}

/// Format the deterministic virtual path for a package file: `/pkg/<owner>/<pkg>/<rel>`.
pub fn package_virtual_path(prefix: &PackagePrefix, rel: &str) -> PathBuf {
    PathBuf::from(format!("/pkg/{}/{}/{rel}", prefix.owner, prefix.name))
}

pub(crate) fn parse_virtual_file(path: &Path) -> Option<VirtualFile<'_>> {
    let s = path.to_str()?;
    if let Some(rel) = s.strip_prefix("/bundle/") {
        if !rel.is_empty() && validate_bundle_relpath(rel).is_ok() {
            return Some(VirtualFile::Root(rel));
        }
        return None;
    }
    if let Some(rest) = s.strip_prefix("/pkg/") {
        let mut split = rest.splitn(3, '/');
        let owner = split.next()?;
        let pkg = split.next()?;
        let rel = split.next()?;
        if is_valid_package_component(owner) && is_valid_package_component(pkg) && validate_bundle_relpath(rel).is_ok() {
            return Some(VirtualFile::Package { owner, pkg, rel });
        }
    }
    None
}

pub(crate) fn parse_virtual_dir(dir: &Path) -> Option<VirtualDir<'_>> {
    let s = dir.to_str()?;
    if s == "/bundle" {
        return Some(VirtualDir::Root(""));
    }
    if let Some(rel) = s.strip_prefix("/bundle/") {
        if validate_bundle_relpath(rel).is_ok() {
            return Some(VirtualDir::Root(rel));
        }
        return None;
    }
    if let Some(rest) = s.strip_prefix("/pkg/") {
        let mut split = rest.splitn(3, '/');
        let owner = split.next()?;
        let pkg = split.next()?;
        if !is_valid_package_component(owner) || !is_valid_package_component(pkg) {
            return None;
        }
        match split.next() {
            None | Some("") => return Some(VirtualDir::Package { owner, pkg, rel: "" }),
            Some(rel) if validate_bundle_relpath(rel).is_ok() => {
                return Some(VirtualDir::Package { owner, pkg, rel });
            }
            _ => return None,
        }
    }
    None
}

/// Resolve `spec` relative to virtual directory `base_rel` within a single
/// bundle or package root, rejecting absolute paths, path escapes, and hidden
/// dot-prefixed components.
fn resolve_relative_segments(base_rel: &str, spec: &str, who: &str) -> Result<String, Error> {
    if spec.is_empty() || spec.bytes().any(|b| matches!(b, b'\\' | b'\0')) {
        return Err(Error::new(format!("{who}: invalid path: {spec}")));
    }
    if spec.starts_with('/') {
        return Err(Error::new(format!("{who}: absolute path not allowed: {spec}")));
    }
    let mut parts: Vec<&str> = if base_rel.is_empty() { Vec::new() } else { base_rel.split('/').collect() };
    for seg in spec.split('/') {
        match seg {
            "" => return Err(Error::new(format!("{who}: invalid path: {spec}"))),
            "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(Error::new(format!("{who}: path escapes package directory: {spec}")));
                }
            }
            s if s.starts_with('.') => {
                return Err(Error::new(format!("{who}: hidden path not allowed: {spec}")));
            }
            s => parts.push(s),
        }
    }
    if parts.is_empty() {
        return Err(Error::new(format!("{who}: invalid path: {spec}")));
    }
    Ok(parts.join("/"))
}

impl BundleVfs {
    /// Construct and strictly validate a read-only [`BundleVfs`].
    pub fn new(
        root_files: BTreeMap<String, Arc<[u8]>>,
        root_package: Option<PackagePrefix>,
        packages: BTreeMap<PackagePrefix, BundlePackage>,
    ) -> Result<Self, Error> {
        for rel in root_files.keys() {
            validate_bundle_relpath(rel)?;
        }
        if let Some(rp) = &root_package {
            validate_package_prefix(rp)?;
            if !packages.contains_key(rp) {
                return Err(Error::new(format!("bundle: root package {rp} is missing from packages")));
            }
        }
        for (prefix, pkg) in &packages {
            validate_package_prefix(prefix)?;
            for rel in pkg.files.keys() {
                validate_bundle_relpath(rel)?;
            }
            for dep in &pkg.declared_deps {
                validate_package_prefix(dep)?;
                if !packages.contains_key(dep) {
                    return Err(Error::new(format!("bundle: package {prefix} declares missing dependency {dep}")));
                }
            }
        }
        Ok(Self { root_files, root_package, packages })
    }

    /// Construct a [`BundleVfs`] for a legacy plain-file deployment.
    pub fn from_root_files(root_files: BTreeMap<String, Arc<[u8]>>) -> Result<Self, Error> {
        Self::new(root_files, None, BTreeMap::new())
    }

    /// Root files (legacy plain-file deployment or supplementary root files).
    pub fn root_files(&self) -> &BTreeMap<String, Arc<[u8]>> {
        &self.root_files
    }

    /// The root package identity, if deployed from `techne-package.scm`.
    pub fn root_package(&self) -> Option<&PackagePrefix> {
        self.root_package.as_ref()
    }

    /// All packages in the bundle, keyed by canonical `(owner pkg)` prefix.
    pub fn packages(&self) -> &BTreeMap<PackagePrefix, BundlePackage> {
        &self.packages
    }

    /// Whether package `(owner pkg)` is provided by this bundle.
    pub fn has_package(&self, owner: &str, pkg: &str) -> bool {
        self.package(owner, pkg).is_some()
    }

    /// Look up package `(owner pkg)` in this bundle.
    pub fn package(&self, owner: &str, pkg: &str) -> Option<&BundlePackage> {
        self.packages.get(&PackagePrefix::new(owner, pkg))
    }

    /// Resolve a deployment entry relative path to its deterministic virtual path.
    pub fn entry_virtual_path(&self, entry: &str) -> Result<PathBuf, Error> {
        validate_bundle_relpath(entry)?;
        if let Some(rp) = &self.root_package {
            if self.packages.get(rp).is_some_and(|p| p.files.contains_key(entry)) {
                return Ok(package_virtual_path(rp, entry));
            }
            if self.root_files.contains_key(entry) {
                return Ok(root_virtual_path(entry));
            }
            return Err(Error::new(format!("{entry}: no such file in bundle")));
        }
        if self.root_files.contains_key(entry) {
            return Ok(root_virtual_path(entry));
        }
        Err(Error::new(format!("{entry}: no such file in bundle")))
    }

    /// Resolve library `parts` (e.g. `["alice", "util", "fmt"]`) to its virtual
    /// `.sld` path inside its owning package, if present in the bundle.
    pub fn library_virtual_path(&self, parts: &[String]) -> Option<PathBuf> {
        if parts.len() < 2 || !parts.iter().all(|p| valid_library_part(p)) {
            return None;
        }
        let prefix = PackagePrefix::new(&parts[0], &parts[1]);
        let pkg = self.packages.get(&prefix)?;
        let rel = if parts.len() == 2 {
            format!("lib/{}/{}.sld", parts[0], parts[1])
        } else {
            format!("lib/{}/{}/{}.sld", parts[0], parts[1], parts[2..].join("/"))
        };
        pkg.files.contains_key(&rel).then(|| package_virtual_path(&prefix, &rel))
    }

    /// For legacy plain-file deployments (`root_package: None`), resolve library
    /// `parts` (e.g. `["my", "helper"]`) to a sibling `.sld` contained in
    /// `root_files`, checking relative to `dir` (if under `/bundle`) and then
    /// at the bundle root `/bundle`.
    pub fn root_library_virtual_path(&self, parts: &[String], dir: &Path) -> Option<PathBuf> {
        if self.root_package.is_some() || parts.is_empty() || !parts.iter().all(|p| valid_library_part(p)) {
            return None;
        }
        if matches!(parts[0].as_str(), "scheme" | "srfi") || crate::library::is_stdlib_library(parts) {
            return None;
        }
        let rel_sld = format!("{}.sld", parts.join("/"));
        if let Some(VirtualDir::Root(base_rel)) = parse_virtual_dir(dir)
            && !base_rel.is_empty()
        {
            let nested = format!("{base_rel}/{rel_sld}");
            if self.root_files.contains_key(&nested) {
                return Some(root_virtual_path(&nested));
            }
        }
        self.root_files.contains_key(&rel_sld).then(|| root_virtual_path(&rel_sld))
    }

    /// Whether `path` is a valid virtual file path (under `/bundle/...` or `/pkg/...`).
    pub fn is_virtual_file_path(&self, path: &Path) -> bool {
        parse_virtual_file(path).is_some()
    }

    /// Whether `dir` is a valid virtual directory path (under `/bundle` or `/pkg/...`).
    pub fn is_virtual_dir_path(&self, dir: &Path) -> bool {
        parse_virtual_dir(dir).is_some()
    }

    /// Read bytes of a virtual file path from the bundle, if present.
    pub fn read_virtual_path(&self, path: &Path) -> Option<&Arc<[u8]>> {
        match parse_virtual_file(path)? {
            VirtualFile::Root(rel) => self.root_files.get(rel),
            VirtualFile::Package { owner, pkg, rel } => self.package(owner, pkg)?.files.get(rel),
        }
    }

    /// Whether `source_path` is a library source owned by package `(owner pkg)`
    /// (i.e. lives under `/pkg/<owner>/<pkg>/lib/...`).
    pub fn owns_library_source(&self, owner: &str, pkg: &str, source_path: Option<&Path>) -> bool {
        let Some(path) = source_path else { return false };
        match parse_virtual_file(path) {
            Some(VirtualFile::Package { owner: o, pkg: p, rel }) => o == owner && p == pkg && rel.starts_with("lib/"),
            _ => false,
        }
    }

    /// The package that files in virtual directory `dir` belong to, if any.
    pub fn package_for_dir(&self, dir: &Path) -> Option<&PackagePrefix> {
        match parse_virtual_dir(dir)? {
            VirtualDir::Package { owner, pkg, .. } => self.packages.get_key_value(&PackagePrefix::new(owner, pkg)).map(|(k, _)| k),
            VirtualDir::Root(_) => self.root_package.as_ref(),
        }
    }

    /// Whether package `importer` is allowed to import libraries of `(owner pkg)`.
    pub fn package_allows(&self, importer: &PackagePrefix, owner: &str, pkg: &str) -> bool {
        if importer.matches(owner, pkg) {
            return true;
        }
        self.packages.get(importer).is_some_and(|p| p.declared_deps.iter().any(|d| d.matches(owner, pkg)))
    }

    /// Resolve a relative `require` or `include` path `spec` from virtual directory `dir`,
    /// confined strictly to that directory's virtual root (`/bundle` or `/pkg/<o>/<p>`).
    pub fn resolve_relative_from_dir(&self, dir: &Path, spec: &str, who: &str) -> Result<PathBuf, Error> {
        match parse_virtual_dir(dir) {
            Some(VirtualDir::Root(base_rel)) => {
                let rel = resolve_relative_segments(base_rel, spec, who)?;
                if !self.root_files.contains_key(&rel) {
                    return Err(Error::new(format!("{who}: {rel}: no such file in bundle")));
                }
                Ok(root_virtual_path(&rel))
            }
            Some(VirtualDir::Package { owner, pkg, rel: base_rel }) => {
                let rel = resolve_relative_segments(base_rel, spec, who)?;
                let prefix = PackagePrefix::new(owner, pkg);
                if !self.packages.get(&prefix).is_some_and(|p| p.files.contains_key(&rel)) {
                    return Err(Error::new(format!("{who}: {rel}: no such file in package {prefix}")));
                }
                Ok(package_virtual_path(&prefix, &rel))
            }
            None => Err(Error::new(format!("{who}: cannot resolve {spec} outside bundle"))),
        }
    }
}
