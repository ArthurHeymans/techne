//! Packaged cloud deployment resolution, offline lockfile verification,
//! canonical `tree-sha256` hashing, and server-side tree verification.
//!
//! A packaged cloud app lives in a directory with `techne-package.scm`
//! declaring `(cloud ENTRY [(bindings (NAME KIND) ...)])`. Only the root
//! package's `(cloud ...)` clause defines the deployment's entry module
//! and binding contract; any `(cloud ...)` clauses on dependency packages
//! are ignored.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use sha2::Digest;
use techne_vm::{bundle::validate_bundle_relpath, data::Data, reader::string_repr, tasks::LifetimeLimits, vm::Vm};

use crate::{
    supervisor::{BindingKind, DeclaredBinding},
    wire::{BUNDLE_BYTES, BundledPackage},
};

/// Immutable regular-file list for one package tree, shared by `Arc`.
pub type CachedFiles = Arc<Vec<(String, Vec<u8>)>>;

/// Maximum total file bytes held in one supervisor's in-memory [`TreeCache`].
pub const TREE_CACHE_BYTES: usize = 32 << 20;

/// Bounded in-memory cache of verified dependency package trees keyed by
/// canonical `tree-sha256`, with FIFO eviction under `max_bytes`.
#[derive(Clone, Debug)]
pub struct TreeCache {
    entries: HashMap<String, (CachedFiles, usize)>,
    order: VecDeque<String>,
    total_bytes: usize,
    max_bytes: usize,
}

impl Default for TreeCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TreeCache {
    pub fn new() -> Self {
        Self::with_max_bytes(TREE_CACHE_BYTES)
    }

    pub fn with_max_bytes(max_bytes: usize) -> Self {
        Self { entries: HashMap::new(), order: VecDeque::new(), total_bytes: 0, max_bytes }
    }

    pub fn contains_key(&self, sha: &str) -> bool {
        self.entries.contains_key(sha)
    }

    pub fn get(&self, sha: &str) -> Option<&CachedFiles> {
        self.entries.get(sha).map(|(files, _)| files)
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.total_bytes = 0;
    }

    pub fn insert(&mut self, sha: String, files: CachedFiles) {
        let bytes: usize = files.iter().map(|(_, b)| b.len()).sum();
        if bytes > self.max_bytes {
            return;
        }
        if let Some((_, old_bytes)) = self.entries.remove(&sha) {
            self.total_bytes = self.total_bytes.saturating_sub(old_bytes);
            self.order.retain(|k| k != &sha);
        }
        while self.total_bytes.saturating_add(bytes) > self.max_bytes {
            let Some(oldest) = self.order.pop_front() else { break };
            if let Some((_, evicted_bytes)) = self.entries.remove(&oldest) {
                self.total_bytes = self.total_bytes.saturating_sub(evicted_bytes);
            }
        }
        self.total_bytes = self.total_bytes.saturating_add(bytes);
        self.order.push_back(sha.clone());
        self.entries.insert(sha, (files, bytes));
    }
}

/// The maximum byte size of `manifest.scm`, `techne-package.scm`, and
/// `techne.lock` checked before loading.
pub(crate) const MANIFEST_BYTES: u64 = 256 << 10;

/// CPU budget for bounded manifest/lockfile evaluation in helper VMs.
pub(crate) const MANIFEST_CPU: Duration = Duration::from_secs(5);

/// Reject files exceeding [`MANIFEST_BYTES`] before passing them to Scheme.
pub(crate) fn check_bounded_file(path: &Path, label: &str) -> Result<(), String> {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MANIFEST_BYTES) {
        return Err(format!("{label} is larger than a manifest may be"));
    }
    Ok(())
}

/// Call Scheme procedure `func` with `args` inside a CPU- and task-bounded
/// lifetime so untrusted manifest/lockfile inputs cannot spin indefinitely.
pub(crate) fn call_bounded(
    vm: &mut Vm,
    func: &techne_vm::api::Value,
    args: &[techne_vm::api::Value],
) -> Result<techne_vm::api::Value, techne_vm::vm::Error> {
    let lifetime = vm.new_lifetime(LifetimeLimits { cpu: Some(MANIFEST_CPU), tasks: Some(4) });
    let task = vm.spawn_in_with(lifetime, func, args)?;
    let run = vm.run_tasks();
    let result = vm.task_result(&task);
    vm.end_lifetime(lifetime);
    run?;
    result.unwrap_or_else(|| Err(techne_vm::vm::Error::new("bounded evaluation did not finish")))
}

/// What a cloud deployment target path resolves to on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloudTarget {
    /// A legacy plain-file deployment (no `techne-package.scm` owning it).
    Legacy { entry: PathBuf },
    /// A packaged deployment rooted at `root_dir` (`techne-package.scm`).
    /// When the user pointed to an entry file inside the package rather than
    /// the directory itself, `explicit_entry` records that canonical file so
    /// preparation verifies it matches the manifest's `(cloud ...)` entry.
    Package { root_dir: PathBuf, explicit_entry: Option<PathBuf> },
}

impl CloudTarget {
    /// The deployment directory (`root_dir` for a package, `entry`'s parent
    /// for a legacy file).
    pub fn dir(&self) -> &Path {
        match self {
            CloudTarget::Legacy { entry } => entry.parent().unwrap_or(Path::new("/")),
            CloudTarget::Package { root_dir, .. } => root_dir,
        }
    }

    /// The path passed to `control.scm` / `%cloud-prepare-directory`.
    pub fn target_path(&self) -> &Path {
        match self {
            CloudTarget::Legacy { entry } => entry,
            CloudTarget::Package { root_dir, explicit_entry } => explicit_entry.as_deref().unwrap_or(root_dir),
        }
    }
}

/// A verified, closed packaged cloud app ready to be sent to a worker or
/// uploaded to `techne cloud serve`.
#[derive(Clone, Debug)]
pub struct PackagedApp {
    pub root_dir: PathBuf,
    pub root_package: (String, String),
    pub entry: String,
    pub declared_bindings: Vec<DeclaredBinding>,
    pub packages: Vec<BundledPackage>,
}

/// Metadata for one package tree stored on a `techne cloud serve` host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageSpec {
    pub owner: String,
    pub pkg: String,
    pub tree_sha256: String,
    pub depends: Vec<(String, String)>,
}

/// Whether `s` is a canonical 64-character lowercase hexadecimal SHA-256 digest.
pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Compute the canonical 64-hex `tree-sha256` digest of a clean file set.
///
/// Matches `(directory-tree-sha256 ...)` in `lisp/techne/package.sld` 1:1:
/// files are sorted ascending by relative POSIX path and hashed as
/// `"F " <rel> "\0" <decimal-byte-len> "\0" <bytes> "\n"`.
pub fn canonical_tree_sha256(files: &[(String, Vec<u8>)]) -> String {
    let mut refs: Vec<&(String, Vec<u8>)> = files.iter().collect();
    refs.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = sha2::Sha256::new();
    for (rel, bytes) in refs {
        hasher.update(b"F ");
        hasher.update(rel.as_bytes());
        hasher.update(b"\0");
        hasher.update(bytes.len().to_string().as_bytes());
        hasher.update(b"\0");
        hasher.update(bytes);
        hasher.update(b"\n");
    }
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Read all clean regular files in a package directory `dir`, matching
/// `(directory-tree-sha256 ...)` 1:1:
/// - excludes hidden files/directories whose component begins with `.`
///   (such as `.git`, `.jj`, `.secrets.scm`, `.techne`)
/// - rejects symlinks and non-regular files
/// - validates every relative POSIX path against `BundleVfs` rules
/// - enforces a cumulative `max_bytes` bound via `total_bytes`.
pub fn read_clean_package_tree(dir: &Path, total_bytes: &mut usize, max_bytes: usize) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut files = Vec::new();
    let mut stack: Vec<(PathBuf, String)> = vec![(dir.to_path_buf(), String::new())];
    while let Some((current, prefix)) = stack.pop() {
        let entries = std::fs::read_dir(&current).map_err(|e| format!("{}: {e}", current.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("{}: {e}", current.display()))?;
            let raw_name = entry.file_name();
            let Some(name) = raw_name.to_str() else {
                return Err(format!("{}: non-UTF-8 path in package tree", entry.path().display()));
            };
            if name.starts_with('.') {
                continue;
            }
            let rel = if prefix.is_empty() { name.to_string() } else { format!("{prefix}/{name}") };
            let ft = entry.file_type().map_err(|e| format!("{}: {e}", entry.path().display()))?;
            if ft.is_symlink() {
                return Err(format!("symlink not allowed in package tree: {rel}"));
            }
            if ft.is_dir() {
                stack.push((entry.path(), rel));
            } else if ft.is_file() {
                validate_bundle_relpath(&rel).map_err(|e| e.msg.clone())?;
                let remaining = max_bytes.saturating_sub(*total_bytes);
                let meta = entry.metadata().map_err(|e| format!("{}: {e}", entry.path().display()))?;
                if meta.len() > remaining as u64 {
                    return Err(format!("{}: deployment exceeds {max_bytes} bytes", dir.display()));
                }
                let file = std::fs::File::open(entry.path()).map_err(|e| format!("{}: {e}", entry.path().display()))?;
                let cap = remaining.saturating_add(1) as u64;
                let mut bytes = Vec::new();
                use std::io::Read as _;
                file.take(cap).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", entry.path().display()))?;
                if bytes.len() > remaining {
                    return Err(format!("{}: deployment exceeds {max_bytes} bytes", dir.display()));
                }
                *total_bytes = total_bytes.saturating_add(bytes.len());
                files.push((rel, bytes));
            } else {
                return Err(format!("unsupported file type in package tree: {rel}"));
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// Read the canonical `(cloud ENTRY ...)` entry path declared in
/// `dir/techne-package.scm`, if any.
fn read_package_cloud_entry(dir: &Path) -> Result<Option<PathBuf>, String> {
    check_bounded_file(&dir.join("techne-package.scm"), "techne-package.scm")?;
    let failed = |e: techne_vm::vm::Error| e.msg.clone();
    let mut vm = Vm::new();
    vm.set_memory_limit(64 << 20);
    vm.set_locked_libraries(true);
    vm.eval_source("(import (techne package))").map_err(failed)?;
    let reader = vm
        .eval_source(
            r#"(lambda (dir)
                 (let ((cloud (manifest-cloud (read-package-manifest dir))))
                   (if cloud (car cloud) #f)))"#,
        )
        .map_err(failed)?;
    let dir_val = vm.from_data(&Data::String(dir.display().to_string())).map_err(failed)?;
    let res = call_bounded(&mut vm, &reader, &[dir_val]).map_err(failed)?;
    match vm.to_data(&res, 65536).map_err(failed)? {
        Data::Bool(false) => Ok(None),
        Data::String(path) => Ok(PathBuf::from(path).canonicalize().ok()),
        _ => Err("unexpected (cloud ...) entry".into()),
    }
}

/// Resolve a user-supplied deployment target path into either a packaged cloud
/// app (`techne-package.scm`) or a legacy plain-file deployment.
///
/// For a file path, only matches a `techne-package.scm` in the file's own
/// directory or an ancestor whose declared `(cloud ENTRY ...)` explicitly
/// resolves to that exact file.
pub fn resolve_cloud_target(target: &Path) -> Result<CloudTarget, String> {
    let canon = target.canonicalize().map_err(|e| format!("{}: {e}", target.display()))?;
    if canon.is_dir() {
        if canon.join("techne-package.scm").is_file() {
            Ok(CloudTarget::Package { root_dir: canon, explicit_entry: None })
        } else {
            Err(format!("{}: directory has no techne-package.scm", target.display()))
        }
    } else if canon.is_file() {
        if let Some(parent) = canon.parent()
            && parent.join("techne-package.scm").is_file()
        {
            return Ok(CloudTarget::Package { root_dir: parent.to_path_buf(), explicit_entry: Some(canon) });
        }
        for ancestor in canon.ancestors().skip(2) {
            if ancestor.join("techne-package.scm").is_file()
                && let Ok(Some(declared_entry)) = read_package_cloud_entry(ancestor)
                && declared_entry == canon
            {
                return Ok(CloudTarget::Package { root_dir: ancestor.to_path_buf(), explicit_entry: Some(canon) });
            }
        }
        Ok(CloudTarget::Legacy { entry: canon })
    } else {
        Err(format!("{}: not a file or directory", target.display()))
    }
}

/// Validate `dir/manifest.scm` against a package's declared `(cloud ...)`
/// bindings without reading or requiring `.secrets.scm`.
pub fn validate_manifest_contract(dir: &Path, declared: &[DeclaredBinding]) -> Result<(), String> {
    check_bounded_file(&dir.join("manifest.scm"), "manifest.scm")?;
    let failed = |e: techne_vm::vm::Error| format!("the manifest: {}", e.msg);
    let mut vm = Vm::new();
    vm.set_memory_limit(64 << 20);
    vm.set_locked_libraries(true);
    let manifest_lib = techne_vm::library::lisp_dir().join("cloud/manifest.scm").display().to_string();
    vm.eval_source(&format!("(require {})", string_repr(&manifest_lib))).map_err(failed)?;
    let checker = vm
        .eval_source(
            r#"(lambda (dir declared)
                 (let* ((path (string-append dir "/" manifest-file))
                        (manifest (if (file-exists? path)
                                      (call-with-input-file path
                                        (lambda (port)
                                          (let ((data (read port)))
                                            (if (eof-object? data)
                                                (dict)
                                                (let ((extra (read port)))
                                                  (unless (eof-object? extra)
                                                    (error "Unexpected trailing datum in:" path))
                                                  data)))))
                                      (dict)))
                        (bindings (if (dict? manifest)
                                      (dict-ref manifest 'bindings (dict))
                                      (error "The manifest is not a dict:" manifest))))
                   (validate-bindings bindings declared)))"#,
        )
        .map_err(failed)?;
    let dir_val = vm.from_data(&Data::String(dir.display().to_string())).map_err(failed)?;
    let contract =
        Data::List(declared.iter().map(|b| Data::List(vec![Data::Symbol(b.name.clone()), Data::Symbol(b.kind.as_str().into())])).collect());
    let contract_val = vm.from_data(&contract).map_err(failed)?;
    call_bounded(&mut vm, &checker, &[dir_val, contract_val]).map_err(failed)?;
    Ok(())
}

/// Prepare a packaged cloud app strictly offline from `root_dir`, verifying
/// `techne-package.scm`, `(cloud ...)` entry & bindings, and `techne.lock`
/// (honoring `TECHNE_PACKAGE_STORE`), and reusing/populating `tree_cache` for
/// dependency trees only after all validation passes.
pub fn prepare_packaged_bundle(
    root_dir: &Path,
    explicit_entry: Option<&Path>,
    tree_cache: Option<&mut TreeCache>,
) -> Result<PackagedApp, String> {
    let root_dir = root_dir.canonicalize().map_err(|e| format!("{}: {e}", root_dir.display()))?;
    let meta = inspect_package_graph(&root_dir)?;

    let entry_path = PathBuf::from(&meta.cloud_entry_abs);
    if let Some(explicit) = explicit_entry {
        let canon_entry = entry_path.canonicalize().map_err(|e| format!("{}: {e}", entry_path.display()))?;
        let canon_explicit = explicit.canonicalize().map_err(|e| format!("{}: {e}", explicit.display()))?;
        if canon_entry != canon_explicit {
            return Err(format!("{}: does not match package (cloud ...) entry {}", explicit.display(), entry_path.display()));
        }
    }
    let entry_rel = relative_posix(&root_dir, &entry_path)?;
    validate_bundle_relpath(&entry_rel).map_err(|e| e.msg.clone())?;

    let mut total_bytes = 0usize;
    let mut packages = Vec::with_capacity(1 + meta.deps.len());
    let mut pending_cache: Vec<(String, CachedFiles)> = Vec::new();

    // Root package: read clean tree and compute canonical tree-sha256.
    // Never cache root trees in TreeCache (roots change on every edit).
    let root_raw = read_clean_package_tree(&root_dir, &mut total_bytes, BUNDLE_BYTES)?;
    let root_sha = canonical_tree_sha256(&root_raw);
    let root_files = Arc::new(root_raw);
    let mut root_depends = meta.root_depends;
    root_depends.sort();
    packages.push(BundledPackage {
        owner: meta.root_name.0.clone(),
        pkg: meta.root_name.1.clone(),
        tree_sha256: root_sha,
        depends: root_depends,
        files: root_files,
    });

    // Locked dependency packages: reuse tree_cache when tree_sha256 is already verified & cached.
    for dep in meta.deps {
        let mut depends = dep.depends;
        depends.sort();
        let files = if let Some(cached) = tree_cache.as_deref().and_then(|c| c.get(&dep.tree_sha256)).cloned() {
            add_cached_bytes(&cached, &mut total_bytes, BUNDLE_BYTES, &dep.dir)?;
            cached
        } else {
            let read = read_clean_package_tree(&dep.dir, &mut total_bytes, BUNDLE_BYTES)?;
            let actual_sha = canonical_tree_sha256(&read);
            if actual_sha != dep.tree_sha256 {
                return Err(format!(
                    "package ({} {}) tree-sha256 mismatch: expected {}, got {}",
                    dep.owner, dep.pkg, dep.tree_sha256, actual_sha
                ));
            }
            let arc = Arc::new(read);
            pending_cache.push((dep.tree_sha256.clone(), arc.clone()));
            arc
        };
        packages.push(BundledPackage { owner: dep.owner, pkg: dep.pkg, tree_sha256: dep.tree_sha256, depends, files });
    }

    if let Some(cache) = tree_cache {
        for (sha, arc) in pending_cache {
            cache.insert(sha, arc);
        }
    }

    packages.sort_by(|a, b| (&a.owner, &a.pkg).cmp(&(&b.owner, &b.pkg)));
    Ok(PackagedApp { root_dir, root_package: meta.root_name, entry: entry_rel, declared_bindings: meta.declared_bindings, packages })
}

/// Verify and load a packaged deployment from immutable content-addressed tree
/// directories under `trees_dir` (`<trees_dir>/<tree_sha256>/`).
pub fn load_verified_packaged_trees(
    trees_dir: &Path,
    entry: &str,
    root_package: &(String, String),
    specs: &[PackageSpec],
    tree_cache: Option<&mut TreeCache>,
) -> Result<(Vec<BundledPackage>, Vec<DeclaredBinding>), String> {
    let (packages, declared, pending_cache) =
        load_verified_packaged_trees_staged(&[trees_dir], entry, root_package, specs, tree_cache.as_deref())?;
    if let Some(cache) = tree_cache {
        for (sha, arc) in pending_cache {
            cache.insert(sha, arc);
        }
    }
    Ok((packages, declared))
}

/// Verify and load a packaged deployment from one or more candidate tree
/// directories (e.g. `[staged_trees_dir, tenant_trees_dir]`), returning
/// `(packages, declared_bindings, pending_dep_cache)` without mutating any
/// cache so callers can commit dependency trees only after binding validation
/// and worker loading succeed.
///
/// Validates:
/// - every `tree_sha256` is valid 64-hex and matches its tree contents
/// - dependency graph is closed, acyclic (explicit DFS grey/black sets), and
///   contains no unreachable packages
/// - every package's `techne-package.scm` is within [`MANIFEST_BYTES`] and
///   declares the claimed `(owner, pkg)` and exact dependency set
/// - `root_package` has a `(cloud ...)` clause matching `entry`
/// - when `root_package` has dependencies (or carries `techne.lock`), the
///   in-memory `techne.lock` parses cleanly as data and matches every selected
///   dependency's `(owner pkg)`, `tree-sha256`, and transitive `depends`
/// - only the root package's `(cloud ...)` bindings are returned.
#[allow(clippy::type_complexity)]
pub(crate) fn load_verified_packaged_trees_staged(
    tree_dirs: &[&Path],
    entry: &str,
    root_package: &(String, String),
    specs: &[PackageSpec],
    tree_cache: Option<&TreeCache>,
) -> Result<(Vec<BundledPackage>, Vec<DeclaredBinding>, Vec<(String, CachedFiles)>), String> {
    validate_bundle_relpath(entry).map_err(|e| e.msg.clone())?;
    let mut by_name: BTreeMap<(String, String), &PackageSpec> = BTreeMap::new();
    for spec in specs {
        if !is_sha256_hex(&spec.tree_sha256) {
            return Err(format!("invalid tree-sha256 for ({} {}): {}", spec.owner, spec.pkg, spec.tree_sha256));
        }
        let key = (spec.owner.clone(), spec.pkg.clone());
        if by_name.insert(key.clone(), spec).is_some() {
            return Err(format!("duplicate package ({} {}) in deployment", key.0, key.1));
        }
    }
    if !by_name.contains_key(root_package) {
        return Err(format!("root package ({} {}) is missing from deployment packages", root_package.0, root_package.1));
    }

    // Verify closure, acyclicity (grey/visiting set in DFS), and reachability from root_package.
    fn dfs_check(
        cur: &(String, String),
        by_name: &BTreeMap<(String, String), &PackageSpec>,
        visiting: &mut BTreeSet<(String, String)>,
        visited: &mut BTreeSet<(String, String)>,
    ) -> Result<(), String> {
        if !visiting.insert(cur.clone()) {
            return Err(format!("dependency cycle detected at package ({} {})", cur.0, cur.1));
        }
        let spec = by_name[cur];
        for dep in &spec.depends {
            if !by_name.contains_key(dep) {
                return Err(format!("package ({} {}) declares missing dependency ({} {})", cur.0, cur.1, dep.0, dep.1));
            }
            if visiting.contains(dep) {
                return Err(format!("dependency cycle detected: ({} {}) -> ({} {})", cur.0, cur.1, dep.0, dep.1));
            }
            if !visited.contains(dep) {
                dfs_check(dep, by_name, visiting, visited)?;
            }
        }
        visiting.remove(cur);
        visited.insert(cur.clone());
        Ok(())
    }

    let mut visiting: BTreeSet<(String, String)> = BTreeSet::new();
    let mut visited: BTreeSet<(String, String)> = BTreeSet::new();
    dfs_check(root_package, &by_name, &mut visiting, &mut visited)?;
    if visited.len() != by_name.len() {
        return Err("deployment contains unreachable packages".into());
    }

    // Verify every package's techne-package.scm (and root's techne.lock) inside
    // one CPU- and memory-bounded locked VM.
    let mut vm = Vm::new();
    vm.set_memory_limit(64 << 20);
    vm.set_locked_libraries(true);
    let failed = |e: techne_vm::vm::Error| format!("package manifest: {}", e.msg);
    vm.eval_source("(import (techne package) (techne lock))").map_err(failed)?;
    let reader = vm
        .eval_source(
            r#"(lambda (dir)
                 (let* ((m (read-package-manifest dir))
                        (name (manifest-name m))
                        (deps (map (lambda (d) (list (symbol->string (caar d)) (symbol->string (cadar d))))
                                   (manifest-dependency-sources m)))
                        (cloud (manifest-cloud m)))
                   (list (symbol->string (car name))
                         (symbol->string (cadr name))
                         (manifest-directory m)
                         deps
                         (if cloud
                             (list (car cloud)
                                   (map (lambda (b) (list (symbol->string (car b)) (symbol->string (cadr b))))
                                        (cadr cloud)))
                             #f))))"#,
        )
        .map_err(failed)?;

    let mut declared_bindings: Option<Vec<DeclaredBinding>> = None;
    let mut total_bytes = 0usize;
    let mut packages = Vec::with_capacity(by_name.len());
    let mut pending_cache: Vec<(String, CachedFiles)> = Vec::new();
    let mut root_files_ref: Option<CachedFiles> = None;
    let mut root_deps_count = 0usize;

    for (key, spec) in &by_name {
        let Some(tree_dir) = tree_dirs.iter().map(|d| d.join(&spec.tree_sha256)).find(|p| p.is_dir()) else {
            return Err(format!("missing cached tree {} for ({} {})", spec.tree_sha256, key.0, key.1));
        };
        check_bounded_file(&tree_dir.join("techne-package.scm"), "techne-package.scm")?;
        let dir_val = vm.from_data(&Data::String(tree_dir.display().to_string())).map_err(failed)?;
        let res = call_bounded(&mut vm, &reader, &[dir_val]).map_err(failed)?;
        let data = vm.to_data(&res, 4 << 20).map_err(failed)?;
        let (manifest_name, manifest_dir, mut manifest_deps, cloud) = parse_manifest_summary(&data)?;

        if manifest_name != *key {
            return Err(format!(
                "tree {} declares package ({} {}), not ({} {})",
                spec.tree_sha256, manifest_name.0, manifest_name.1, key.0, key.1
            ));
        }
        manifest_deps.sort();
        let mut claimed_deps = spec.depends.clone();
        claimed_deps.sort();
        if manifest_deps != claimed_deps {
            return Err(format!("package ({} {}) dependency list does not match techne-package.scm", key.0, key.1));
        }

        let is_root = key == root_package;
        if is_root {
            let Some((cloud_entry_abs, bindings)) = cloud else {
                return Err(format!("root package ({} {}) techne-package.scm has no (cloud ...) clause", key.0, key.1));
            };
            let rel = relative_posix(Path::new(&manifest_dir), Path::new(&cloud_entry_abs))?;
            if rel != entry {
                return Err(format!("deployment entry {entry:?} does not match root package (cloud ...) entry {rel:?}"));
            }
            declared_bindings = Some(bindings);
            root_deps_count = manifest_deps.len();
        }

        // Never read/write root package trees via TreeCache; reuse verified dep trees only.
        let files = if !is_root && let Some(cached) = tree_cache.and_then(|c| c.get(&spec.tree_sha256)).cloned() {
            add_cached_bytes(&cached, &mut total_bytes, BUNDLE_BYTES, &tree_dir)?;
            cached
        } else {
            let read = read_clean_package_tree(&tree_dir, &mut total_bytes, BUNDLE_BYTES)?;
            let actual_sha = canonical_tree_sha256(&read);
            if actual_sha != spec.tree_sha256 {
                return Err(format!("cached tree {} for ({} {}) is corrupted (computed {})", spec.tree_sha256, key.0, key.1, actual_sha));
            }
            let arc = Arc::new(read);
            if !is_root {
                pending_cache.push((spec.tree_sha256.clone(), arc.clone()));
            }
            arc
        };

        if is_root {
            root_files_ref = Some(files.clone());
        }

        packages.push(BundledPackage {
            owner: key.0.clone(),
            pkg: key.1.clone(),
            tree_sha256: spec.tree_sha256.clone(),
            depends: manifest_deps,
            files,
        });
    }

    // Verify root's techne.lock purely as data (zero host filesystem access).
    let root_files = root_files_ref.ok_or_else(|| "root package missing from deployment packages".to_string())?;
    let lock_bytes = root_files.iter().find(|(rel, _)| rel == "techne.lock").map(|(_, b)| b.as_slice());
    if root_deps_count > 0 && lock_bytes.is_none() {
        return Err(format!("root package ({} {}) has dependencies but no techne.lock in root tree", root_package.0, root_package.1));
    }
    if let Some(lock_bytes) = lock_bytes {
        if lock_bytes.len() as u64 > MANIFEST_BYTES {
            return Err("techne.lock is larger than a manifest may be".into());
        }
        let lock_text = std::str::from_utf8(lock_bytes).map_err(|_| "techne.lock is not valid UTF-8".to_string())?;
        let lock_parser = vm
            .eval_source(
                r#"(lambda (text)
                     (let* ((port (open-input-string text))
                            (datum (read port)))
                       (when (eof-object? datum)
                         (error "techne.lock is empty"))
                       (let ((extra (read port)))
                         (unless (eof-object? extra)
                           (error "techne.lock has unexpected trailing datum")))
                       (let ((lf (parse-lockfile datum)))
                         (map (lambda (p)
                                (let ((n (locked-package-name p)))
                                  (list (symbol->string (car n))
                                        (symbol->string (cadr n))
                                        (locked-package-tree-sha256 p)
                                        (map (lambda (d) (list (symbol->string (car d)) (symbol->string (cadr d))))
                                             (locked-package-depends p)))))
                              (lockfile-packages lf)))))"#,
            )
            .map_err(|e| format!("techne.lock: {}", e.msg))?;
        let text_val = vm.from_data(&Data::String(lock_text.to_string())).map_err(|e| format!("techne.lock: {}", e.msg))?;
        let res = call_bounded(&mut vm, &lock_parser, &[text_val]).map_err(|e| format!("techne.lock: {}", e.msg))?;
        let data = vm.to_data(&res, 4 << 20).map_err(|e| format!("techne.lock: {}", e.msg))?;
        let locked = parse_locked_entries(&data)?;

        let mut locked_map: BTreeMap<(String, String), (String, Vec<(String, String)>)> = BTreeMap::new();
        for (owner, pkg, sha, mut deps) in locked {
            deps.sort();
            let key = (owner, pkg);
            if &key == root_package {
                return Err(format!("techne.lock cannot list root package ({} {})", key.0, key.1));
            }
            if locked_map.insert(key.clone(), (sha, deps)).is_some() {
                return Err(format!("techne.lock has duplicate package ({} {})", key.0, key.1));
            }
        }
        let non_root_count = packages.iter().filter(|p| (&p.owner, &p.pkg) != (&root_package.0, &root_package.1)).count();
        if locked_map.len() != non_root_count {
            return Err(format!(
                "deployment dependency set ({non_root_count} packages) does not match techne.lock ({} packages)",
                locked_map.len()
            ));
        }
        for pkg in &packages {
            if (&pkg.owner, &pkg.pkg) == (&root_package.0, &root_package.1) {
                continue;
            }
            let key = (pkg.owner.clone(), pkg.pkg.clone());
            let Some((locked_sha, locked_deps)) = locked_map.get(&key) else {
                return Err(format!("package ({} {}) is not pinned in techne.lock", key.0, key.1));
            };
            if pkg.tree_sha256 != *locked_sha {
                return Err(format!(
                    "package ({} {}) tree-sha256 {} does not match techne.lock ({locked_sha})",
                    key.0, key.1, pkg.tree_sha256
                ));
            }
            if pkg.depends != *locked_deps {
                return Err(format!("package ({} {}) dependencies do not match techne.lock", key.0, key.1));
            }
        }
    }

    let declared_bindings = declared_bindings.ok_or_else(|| "root package missing (cloud ...) clause".to_string())?;
    Ok((packages, declared_bindings, pending_cache))
}

struct GraphMeta {
    root_name: (String, String),
    cloud_entry_abs: String,
    declared_bindings: Vec<DeclaredBinding>,
    root_depends: Vec<(String, String)>,
    deps: Vec<DepMeta>,
}

struct DepMeta {
    owner: String,
    pkg: String,
    dir: PathBuf,
    tree_sha256: String,
    depends: Vec<(String, String)>,
}

fn inspect_package_graph(root_dir: &Path) -> Result<GraphMeta, String> {
    check_bounded_file(&root_dir.join("techne-package.scm"), "techne-package.scm")?;
    check_bounded_file(&root_dir.join("techne.lock"), "techne.lock")?;
    let failed = |e: techne_vm::vm::Error| e.msg.clone();
    let mut vm = Vm::new();
    vm.set_memory_limit(64 << 20);
    vm.set_locked_libraries(true);
    vm.eval_source("(import (techne package) (techne lock))").map_err(failed)?;
    let inspector = vm
        .eval_source(
            r#"(lambda (dir)
                 (let* ((root-m (read-package-manifest dir))
                        (root-name (manifest-name root-m))
                        (root-dir (manifest-directory root-m))
                        (cloud (manifest-cloud root-m))
                        (root-deps (map (lambda (d) (list (symbol->string (caar d)) (symbol->string (cadar d))))
                                        (manifest-dependency-sources root-m)))
                        (root-overrides (manifest-overrides root-m))
                        (lock-path (string-append root-dir "/" lockfile-name)))
                   (unless cloud
                     (error (string-append dir ": techne-package.scm has no (cloud ...) clause")))
                   (let ((cloud-data
                           (list (car cloud)
                                 (map (lambda (b) (list (symbol->string (car b)) (symbol->string (cadr b))))
                                      (cadr cloud)))))
                     (cond
                       ((file-exists? lock-path)
                        (call-with-values (lambda () (verify-lockfile root-dir))
                          (lambda (lf _verified-root manifests)
                            (let ((deps
                                    (map (lambda (lp dm)
                                           (let ((dn (manifest-name dm)))
                                             (list (symbol->string (car dn))
                                                   (symbol->string (cadr dn))
                                                   (manifest-directory dm)
                                                   (locked-package-tree-sha256 lp)
                                                   (map (lambda (d) (list (symbol->string (caar d)) (symbol->string (cadar d))))
                                                        (manifest-dependency-sources dm)))))
                                         (lockfile-packages lf)
                                         (cdr manifests))))
                              (list (symbol->string (car root-name))
                                    (symbol->string (cadr root-name))
                                    cloud-data
                                    root-deps
                                    deps)))))
                       ((or (pair? root-deps) (pair? root-overrides))
                        (error (string-append dir ": package has dependencies but no techne.lock; run `techne lock` first")))
                       (else
                        (list (symbol->string (car root-name))
                              (symbol->string (cadr root-name))
                              cloud-data
                              root-deps
                              '()))))))"#,
        )
        .map_err(failed)?;
    let dir_val = vm.from_data(&Data::String(root_dir.display().to_string())).map_err(failed)?;
    let res = call_bounded(&mut vm, &inspector, &[dir_val]).map_err(failed)?;
    let data = vm.to_data(&res, 4 << 20).map_err(failed)?;
    parse_graph_meta(&data)
}

#[allow(clippy::type_complexity)]
fn parse_locked_entries(data: &Data) -> Result<Vec<(String, String, String, Vec<(String, String)>)>, String> {
    let bad = || "unexpected techne.lock data".to_string();
    let Data::List(items) = data else { return Err(bad()) };
    let mut out = Vec::with_capacity(items.len());
    for entry in items {
        let Data::List(parts) = entry else { return Err(bad()) };
        let [Data::String(owner), Data::String(pkg), Data::String(sha), deps_data] = &parts[..] else {
            return Err(bad());
        };
        out.push((owner.clone(), pkg.clone(), sha.clone(), parse_name_list(deps_data)?));
    }
    Ok(out)
}

fn parse_graph_meta(data: &Data) -> Result<GraphMeta, String> {
    let bad = || "unexpected package graph metadata".to_string();
    let Data::List(items) = data else { return Err(bad()) };
    let [Data::String(owner), Data::String(pkg), cloud_data, root_deps_data, deps_data] = &items[..] else {
        return Err(bad());
    };
    let (cloud_entry_abs, declared_bindings) = parse_cloud_data(cloud_data)?;
    let root_depends = parse_name_list(root_deps_data)?;
    let Data::List(dep_entries) = deps_data else { return Err(bad()) };
    let mut deps = Vec::with_capacity(dep_entries.len());
    for entry in dep_entries {
        let Data::List(parts) = entry else { return Err(bad()) };
        let [Data::String(o), Data::String(p), Data::String(dir), Data::String(sha), ddeps] = &parts[..] else {
            return Err(bad());
        };
        deps.push(DepMeta {
            owner: o.clone(),
            pkg: p.clone(),
            dir: PathBuf::from(dir),
            tree_sha256: sha.clone(),
            depends: parse_name_list(ddeps)?,
        });
    }
    Ok(GraphMeta { root_name: (owner.clone(), pkg.clone()), cloud_entry_abs, declared_bindings, root_depends, deps })
}

#[allow(clippy::type_complexity)]
fn parse_manifest_summary(
    data: &Data,
) -> Result<((String, String), String, Vec<(String, String)>, Option<(String, Vec<DeclaredBinding>)>), String> {
    let bad = || "unexpected package manifest summary".to_string();
    let Data::List(items) = data else { return Err(bad()) };
    let [Data::String(owner), Data::String(pkg), Data::String(dir), deps_data, cloud_data] = &items[..] else {
        return Err(bad());
    };
    let deps = parse_name_list(deps_data)?;
    let cloud = match cloud_data {
        Data::Bool(false) => None,
        other => Some(parse_cloud_data(other)?),
    };
    Ok(((owner.clone(), pkg.clone()), dir.clone(), deps, cloud))
}

fn parse_cloud_data(data: &Data) -> Result<(String, Vec<DeclaredBinding>), String> {
    let bad = || "unexpected (cloud ...) metadata".to_string();
    let Data::List(items) = data else { return Err(bad()) };
    let [Data::String(entry), Data::List(bindings)] = &items[..] else { return Err(bad()) };
    let mut declared = Vec::with_capacity(bindings.len());
    for entry in bindings {
        let Data::List(pair) = entry else { return Err(bad()) };
        let [Data::String(name), Data::String(kind)] = &pair[..] else { return Err(bad()) };
        declared.push(DeclaredBinding { name: name.clone(), kind: BindingKind::parse(kind)? });
    }
    Ok((entry.clone(), declared))
}

fn parse_name_list(data: &Data) -> Result<Vec<(String, String)>, String> {
    let bad = || "unexpected package name list".to_string();
    let Data::List(items) = data else { return Err(bad()) };
    items
        .iter()
        .map(|item| match item {
            Data::List(pair) => match &pair[..] {
                [Data::String(o), Data::String(p)] => Ok((o.clone(), p.clone())),
                _ => Err(bad()),
            },
            _ => Err(bad()),
        })
        .collect()
}

fn relative_posix(root: &Path, path: &Path) -> Result<String, String> {
    let rel = path.strip_prefix(root).map_err(|_| format!("{}: path is outside package root {}", path.display(), root.display()))?;
    let mut parts = Vec::new();
    for c in rel.components() {
        let std::path::Component::Normal(os) = c else {
            return Err(format!("{}: invalid relative path in package", rel.display()));
        };
        let Some(s) = os.to_str() else {
            return Err(format!("{}: non-UTF-8 path in package", rel.display()));
        };
        parts.push(s);
    }
    if parts.is_empty() {
        return Err(format!("{}: empty relative path", path.display()));
    }
    Ok(parts.join("/"))
}

fn add_cached_bytes(files: &[(String, Vec<u8>)], total_bytes: &mut usize, max_bytes: usize, dir: &Path) -> Result<(), String> {
    for (_, bytes) in files {
        *total_bytes = total_bytes.saturating_add(bytes.len());
        if *total_bytes > max_bytes {
            return Err(format!("{}: deployment exceeds {max_bytes} bytes", dir.display()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_tree_sha256_matches_scheme_directory_tree_sha256() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("lib/acme")).unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join("techne-package.scm"), "(package (acme demo) (version \"0.1.0\"))").unwrap();
        std::fs::write(dir.path().join("app.scm"), "(provide fetch)\n(define (fetch r e) (dict 'status 200 'body \"ok\"))\n").unwrap();
        std::fs::write(
            dir.path().join("lib/acme/demo.sld"),
            "(define-library (acme demo) (export x) (import (techne)) (begin (define x 1)))\n",
        )
        .unwrap();
        std::fs::write(dir.path().join(".secrets.scm"), "{\"SECRET\" \"value\"}").unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();

        let mut total = 0;
        let files = read_clean_package_tree(dir.path(), &mut total, BUNDLE_BYTES).unwrap();
        assert_eq!(files.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), vec!["app.scm", "lib/acme/demo.sld", "techne-package.scm"]);
        let rust_sha = canonical_tree_sha256(&files);
        assert!(is_sha256_hex(&rust_sha));

        let mut vm = Vm::new();
        vm.eval_source("(import (techne package))").unwrap();
        let val = vm.eval_source(&format!("(directory-tree-sha256 {})", string_repr(&dir.path().display().to_string()))).unwrap();
        let scheme_sha: String = vm.get(&val).unwrap();
        assert_eq!(rust_sha, scheme_sha);
    }

    #[test]
    fn tree_cache_fifo_eviction_and_bounds() {
        let mut cache = TreeCache::with_max_bytes(10);
        let mk = |len: usize| Arc::new(vec![("a".to_string(), vec![b'x'; len])]);
        cache.insert("k1".into(), mk(4));
        cache.insert("k2".into(), mk(4));
        assert!(cache.contains_key("k1"));
        assert!(cache.contains_key("k2"));
        // Inserting k3 (4 bytes) exceeds 10 bytes -> evicts oldest (k1).
        cache.insert("k3".into(), mk(4));
        assert!(!cache.contains_key("k1"));
        assert!(cache.contains_key("k2"));
        assert!(cache.contains_key("k3"));
        // Single entry larger than cap is not cached and does not panic.
        cache.insert("huge".into(), mk(11));
        assert!(!cache.contains_key("huge"));
        // Clear empties cache.
        cache.clear();
        assert!(!cache.contains_key("k2"));
        assert!(!cache.contains_key("k3"));
    }

    #[test]
    fn resolve_cloud_target_ancestor_matching() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = dir.path().join("pkg");
        std::fs::create_dir_all(pkg.join("sub")).unwrap();
        std::fs::create_dir_all(pkg.join("unrelated")).unwrap();
        std::fs::write(pkg.join("techne-package.scm"), r#"(package (acme app) (version "0.1.0") (cloud "sub/app.scm"))"#).unwrap();
        std::fs::write(pkg.join("sub/app.scm"), "(provide fetch)").unwrap();
        std::fs::write(pkg.join("unrelated/plain.scm"), "(provide fetch)").unwrap();

        // File explicitly named by ancestor `(cloud "sub/app.scm")` resolves as Package.
        match resolve_cloud_target(&pkg.join("sub/app.scm")).unwrap() {
            CloudTarget::Package { root_dir, explicit_entry } => {
                assert_eq!(root_dir, pkg.canonicalize().unwrap());
                assert_eq!(explicit_entry, Some(pkg.join("sub/app.scm").canonicalize().unwrap()));
            }
            other => panic!("expected Package, got {other:?}"),
        }

        // Unrelated file under an ancestor `techne-package.scm` resolves as Legacy, NOT Package.
        match resolve_cloud_target(&pkg.join("unrelated/plain.scm")).unwrap() {
            CloudTarget::Legacy { entry } => {
                assert_eq!(entry, pkg.join("unrelated/plain.scm").canonicalize().unwrap());
            }
            other => panic!("expected Legacy for unrelated file under ancestor manifest, got {other:?}"),
        }
    }
}
