# Packages

Techne packages are self-contained directories with a pure-data manifest
(`techne-package.scm`) and canonical two-symbol identity `(owner pkg)`. They
serve editor extensions, `(cloud ...)` deployments, and plain R7RS libraries
alike.

## Installing packages in the editor

In an editor Scheme session:

```scheme
(import (techne packages))
(package-install! '(github "alice/techne-rust" (tag "v0.3"))) ; illustrative repository
```

`github` is data, not a procedure. Sources are `(github "owner/repo")`,
`(git "https://…")`, or `(local "/absolute/path/to/git/repository")`. Append
`(branch "name")`, `(tag "name")`, or `(commit "full-object-id")` to select a
revision (default: the repository's `HEAD`). Package identities are canonical
two-symbol names `(owner pkg)`, such as `(techne mail)` or `(alice rust-mode)`.
To give a source a catalog name,
`(register-package-recipe! '(recipe (alice rust-mode) (github "alice/techne-rust")))`
then `(package-install! '(alice rust-mode))`. The name must match the manifest.

A repository may hold several packages, each in a directory of its own.
Select one with `(package (owner pkg))` after the place:
`(github "alice/techne-tools" (tag "v0.3") (package (alice rust-mode)))`. A catalog
name selects it too, and a repository holding one package needs neither.
Transitive dependencies — whether `(path "...")` siblings inside the same
repository or cross-repository Git dependencies — are resolved together under
the one-version-per-world rule (below).

Installation fetches and stages each package cleanly (excluding `.git`, `.jj`,
and hidden dot-prefixed entries, and rejecting symlinks), computes its
canonical `tree-sha256`, and atomically publishes it into the content-addressed
store under `$XDG_DATA_HOME/techne/packages/store/<tree-sha256>/` (normally
`~/.local/share/techne/packages/store/<tree-sha256>/`, overridable via
`$TECHNE_PACKAGE_STORE` or `#:store`; passing an explicit `#:directory` without
`#:store` uses `<DIRECTORY>/store` for isolated environments and tests).
Installed roots and the resolved dependency graph are recorded in
`installed.scm` and `world.lock` in the packages data directory.

- Repeating the same `package-install!` declaration reuses `world.lock` and the
  content-addressed store offline without running Git.
- Pass `#:upgrade #t` to re-fetch a moving branch or tag: the new checkout is
  published under its new `<tree-sha256>` without clobbering or mutating earlier
  store entries, while still checking diamond-dependency compatibility with
  other installed packages.
- In a fresh editor, call `(package-restore!)` to verify the exact transitive
  closure of `installed.scm` against `world.lock` and every entry's
  `tree-sha256` strictly offline, register library providers, and advertise
  editor declarations.
- Packages without an `(editor ...)` clause — pure library packages and
  `(cloud ...)` packages — install, lock, verify, and restore cleanly without
  requiring commands or options.

An installed editor declaration reserves its package identity, commands and
option names, including while cold. Setting an existing option is not
redeclaring it. Installation rejects collisions before publishing; restore
preflights the store and verifies every `tree-sha256`. If a publisher crashes
leaving `.publishing` in the packages directory, remove the empty directory
only after confirming that no install is in progress. The store is managed
immutable data, not a working tree to edit.

## `techne-package.scm`

In the package's directory, put **one datum**, `techne-package.scm`:

```scheme
(package (alice rust-mode)
  (version "0.1.0")
  (techne "0.4")
  (description "Rust editing commands and project helpers.")
  (depends
    ((alice text-util) (path "../text-util"))
    ((bob lsp-helpers) (github "bob/techne-lsp-helpers" (tag "v1.2")))
    ((bob mono-fmt) (git "https://example.org/bob/mono.git"
                         (commit "0123456789abcdef0123456789abcdef01234567")
                         (package (bob mono-fmt)))))
  (override
    ((bob lsp-helpers) (path "../my-lsp-helpers-fork")))
  (editor "rust.scm"
    (commands (rust-check "Check the Rust project."))
    (options (rust-edition "2024" string "Edition used for new projects."))))
```

All clauses are pure data and may appear at most once; unknown or duplicate
clauses are refused. Neither locking, installation, nor completion evaluates
this file:

- `(version "0.1.0")` — optional package version string.
- `(techne "0.4")` — optional minimum Techne runtime version string.
- `(description "...")` — optional human-readable description string.
- `(depends ((owner pkg) <source>) ...)` — optional direct dependencies. Each
  `<source>` is one of:
  - `(path "relative/dir")` — local directory relative to this manifest.
  - `(github "owner/repo" [(tag "...") | (branch "...") | (commit "...")] [(package (owner pkg))])`
  - `(git "https://..." [(tag "...") | (branch "...") | (commit "...")] [(package (owner pkg))])`
  - `(local "/absolute/path/to/repo" [(tag "...") | (branch "...") | (commit "...")] [(package (owner pkg))])`
    — allowed only in the root manifest and root `(override ...)` (rejected in
    non-root/transitive dependency manifests).
  At most one ref selector (`tag`, `branch`, or `commit`) and one optional
  `(package (owner pkg))` monorepo selector may appear. Duplicate dependency
  names and self-dependencies are refused.
- `(override ((owner pkg) <source>) ...)` — optional root-only overrides using
  the same validated `<source>` forms. When resolving a root project, overrides
  replace the source used for any direct or transitive dependency on
  `(owner pkg)`, allowing a project to resolve diamond conflicts or test a fork
  or local `(path "...")` checkout without editing intermediate packages.
  Overrides declared inside non-root dependency manifests are ignored.
- `(editor "rust.scm" (commands ...) [(options ...)])` — optional editor entry.
  `commands` advertises public command names and short descriptions. `options`
  declares early options as `(name default schema documentation)` and may be
  left out. `rust.scm` imports `(techne editor)` and uses the normal
  `define-command`; it need not redeclare its early options. First use runs
  **trusted Scheme code** in the editor, not in a package sandbox. Review the
  source before installing. The shorthand form
  `(package (alice rust-mode) "rust.scm" (COMMAND ...) (OPTION ...))` is still
  read and means the same.
- `(cloud "app.scm" [(bindings (name kind) ...)])` — optional cloud worker
  entry contained in the package directory. `bindings` declares binding names
  and kinds (`kv`, `secret`, `text`) as distinct symbols, never secret values.

## Canonical `tree-sha256`

Every materialized package tree is identified and integrity-checked by its
canonical `tree-sha256` (64 lowercase hexadecimal characters), exported as
`directory-tree-sha256` and `package-tree-sha256` in `(techne package)`:

1. Walk the package directory recursively.
2. Skip any file or directory whose path component begins with `.` (including
   `.git`, `.jj`, `.secrets.scm`, `.DS_Store`, and hidden subdirectories).
3. Reject symlinks and non-regular files (sockets, FIFOs, devices) with an
   error so hashed trees are self-contained and reproducible.
4. Collect all remaining regular files with relative POSIX paths (`/`
   separators, no leading `./`) and sort them in ascending UTF-8 code-point
   order (`string<?`).
5. Hash the concatenation of one record per regular file with SHA-256:

   ```
   "F " <relpath-utf8> "\0" <decimal-byte-length> "\0" <raw-file-bytes> "\n"
   ```

An empty directory hashes the empty byte stream (`e3b0c442…`).

## `techne.lock` & One-Version Resolution

`techne.lock` is a pure-data S-expression written beside `techne-package.scm`
and read by `(techne lock)` without evaluation:

```scheme
(lock
  (techne "0.4")
  (package (alice text-util) "0.1.0"
    (source (path "../text-util"))
    (tree-sha256 "4f9e...64-hex...")
    (depends))
  (package (bob lsp-helpers) "1.2.0"
    (source (github "bob/techne-lsp-helpers"
              (commit "0123456789abcdef0123456789abcdef01234567")
              (ref (tag "v1.2"))))
    (tree-sha256 "9a1c...64-hex...")
    (depends))
  (package (bob mono-fmt) "0.3.0"
    (source (git "https://example.org/bob/mono.git"
              (commit "0123456789abcdef0123456789abcdef01234567")
              (ref (commit "0123456789abcdef0123456789abcdef01234567"))
              (subdir "packages/mono-fmt")))
    (tree-sha256 "7b2d...64-hex...")
    (depends (bob lsp-helpers))))
```

- Package entries and their `(depends ...)` lists are sorted deterministically
  by canonical `(owner pkg)` name for minimal diffs.
- Every Git source (`github`, `git`, `local`) is pinned to its full resolved
  `(commit "<sha>")`, along with its normalized requested `(ref ...)` clause
  (`(ref (tag "..."))`, `(ref (branch "..."))`, or `(ref (commit "..."))`,
  omitted for default `HEAD`) and optional `(subdir "...")` for monorepo
  packages. Persisting `(ref ...)` ensures that changing a declared tag, branch,
  or commit in `techne-package.scm` immediately invalidates `techne.lock` during
  offline `--check`, `--frozen`, and `techne run --locked`.
- **One version per `(owner pkg)` per world:** Each canonical `(owner pkg)`
  may appear at most once in a resolved world.
  - If two dependency paths resolve the same `(owner pkg)` to the same origin
    source, requested ref, commit, subdir, and `tree-sha256` (or the same
    canonical local path), they deduplicate into one entry.
  - If they resolve to different sources/mirrors, refs, commits, paths, or
    `tree-sha256` hashes, resolution fails loudly with a diagnostic naming
    **both** full dependency chains (for example
    `(root app) -> (alice a) -> (bob util)` vs
    `(root app) -> (alice b) -> (bob util)`). Add a root `(override ...)` in
    `techne-package.scm` to pick one version explicitly.
- Dependency cycles are rejected with the full cycle chain.

## CLI & LSP

- **`techne lock [DIR] [--check | --frozen | --update] [--store DIR]`**
  - Resolves `techne-package.scm` in `DIR` (default `.`), fetches and
    materializes Git dependencies into the content-addressed store
    (`$TECHNE_PACKAGE_STORE` or `$XDG_DATA_HOME/techne/packages/store`, or
    `--store DIR`), checks for diamond conflicts and cycles, and writes
    `techne.lock` beside `techne-package.scm`. When `techne.lock` already
    exists, existing valid pins for unchanged dependency edges are preserved
    (hydrating any missing store entries at their pinned commit and verifying
    `tree-sha256`).
  - With `--update`, ignores existing pins in `techne.lock` and re-resolves all
    dependencies from their declared refs.
  - With `--frozen`, strictly materializes any missing store entries by the
    exact pinned `(commit ...)` in `techne.lock`, verifies every `tree-sha256`
    and freshness against `techne-package.scm`, and fails if `techne.lock` is
    missing or stale — without ever modifying `techne.lock`.
  - With `--check`, verifies strictly offline (without running Git or modifying
    `techne.lock`) that `techne.lock` exists, matches `techne-package.scm` and
    root overrides, and every locked package's `tree-sha256` matches its
    directory on disk or in the store.
- **`techne run [--locked] [--package DIR] [--store DIR] FILE [ARGS]`**
  - Without `--locked`, locates the nearest `techne-package.scm` above `FILE`
    (or `--package DIR`) and registers local `(path ...)` packages via
    `(use-package-directory!)`. Passing `--store DIR` without `--locked` is an
    error.
  - With `--locked`, enables locked library isolation (ignoring
    `TECHNE_LIBRARY_PATH` and working-directory fallbacks), requires a valid,
    up-to-date `techne.lock`, verifies every locked package's `tree-sha256`
    strictly offline against the content-addressed store and local paths (never
    invoking Git or touching the network; directs you to `techne lock --frozen`
    if store entries are missing), wires every `(owner pkg)` library provider
    and direct import boundary into the VM, and runs `FILE`.
- **`techne lsp`**
  - Resolves a package's own `(owner pkg ...)` libraries from `lib/` and its
    direct `(depends ...)` via the consuming workspace/project root's
    `(override ...)`, `techne.lock` + the content-addressed store (honoring
    `$TECHNE_PACKAGE_STORE`), or unlocked `(path ...)`, reading manifests and
    lockfiles strictly as pure data while preserving direct-dependency
    boundaries and ignoring overrides inside non-root dependency manifests.

## Package directories

A package is a directory with `techne-package.scm`, for the editor, cloud, or
plain libraries; `(techne package)` and `(techne lock)` need no editor. A
repository can hold several packages side by side, as `packages/` does here; a
package's directory holds no other package.

```
mail/
  techne-package.scm              (package (techne mail) ...)
  lib/techne/mail/imap.sld        (techne mail imap)
  editor.scm                      the editor's entry, if it has one
  tests/
```

- **Identity & libraries.** A package's canonical name is a two-symbol list
  `(owner pkg)` where both `owner` and `pkg` are ASCII lowercase identifiers
  starting with `[a-z0-9]` followed by `[a-z0-9._-]` (never starting with `.`
  or `-`, and no non-ASCII characters). Package `(owner pkg)` exclusively owns
  the libraries whose names begin with `(owner pkg)`: `(owner pkg)` is
  `lib/owner/pkg.sld` and `(owner pkg sub ...)` is `lib/owner/pkg/sub/....sld`
  in its directory and nowhere else. They belong to the world, not to a
  generation: one instance, whoever imports them, staying when the package is
  reloaded or unloaded. The top-level namespaces `scheme` and `srfi` are
  reserved and cannot be claimed by packages. First-party packages may use
  `techne` as their owner (such as `(techne mail)`): each package owns only its
  2-part prefix `(techne pkg ...)`, so it coexists beside Techne's stdlib
  libraries (`(techne http)`, `(techne path)`, `(techne package)`,
  `(techne lock)`, ...), while colliding with a stdlib library or another
  registered `(owner pkg)` package is refused.
- **Direct dependency boundaries.** A package's files may import its own
  libraries and those of the packages listed in its own `(depends ...)`:
  importing another package's library — including a transitive dependency of a
  dependency — is refused. Libraries no package provides (Techne's own stdlib)
  and files in no package are not concerned.
- **In the editor.** The options of the manifest's `editor` clause are
  declared and its commands offered, without loading the entry
  (`advertise-package!`). Installed packages and the built-in mail
  (`packages/mail`, `(techne mail)`) are advertised this way. The entry and the
  files it requires from its directory, other than `lib/`, load as a generation
  and reload with the package. The packages Techne ships are advertised at
  start without `(techne package)`, to keep startup in its budget: its
  tests check their manifests and that they depend on nothing.

For a built-in without a directory of its own, `register-command-package!`
registers the same command offers from a local path. Ibuffer, Compile and
Markdown preview use this mechanism. Keep options and global bindings available
early; package-private modes and hooks needed only after opening a view can
live with its implementation. An agent, check-only or staged invocation cannot
activate an unloaded package. A user's normal invocation activates before
argument acquisition/validation, and then uses the real contract and operation.
`unload-package` unloads the running implementation, not the installed
declarations or store entries.
