# Editor packages

In an editor Scheme session:

```scheme
(import (techne packages))
(package-install! '(github "alice/techne-rust" (tag "v0.3"))) ; illustrative repository
```

`github` is data, not a procedure. Sources are `(github "owner/repo")`,
`(git "https://…")`, or `(local "/absolute/path/to/git/repository")`. Append
`(branch "name")`, `(tag "name")`, or `(commit "full-object-id")` to select a
revision. The default is the repository's HEAD. Package identities are canonical
two-symbol names `(owner pkg)`, such as `(techne mail)` or `(alice rust-mode)`.
To give a source a catalog name,
`(register-package-recipe! '(recipe (alice rust-mode) (github "alice/techne-rust")))`
then `(package-install! '(alice rust-mode))`. The name must match the manifest.

A repository may hold several packages, each in a directory of its own (below).
Of such a one, say which to install with `(package (owner pkg))` after the place:
`(github "alice/techne-tools" (tag "v0.3") (package (alice rust-mode)))`. A catalog
name says so too, and a repository holding one package needs neither. The
packages it depends on must be beside it in the same repository: they come
with the checkout, at the commit pinned, and are made known with it, their
commands not advertised. The packages installed from one source (the same
place and revision) share one checkout, so two of them depending on a
third find the same one. From two revisions of a repository they do not:
a package is in one directory in an editor, and the second is refused.

Installation fetches a private checkout and records the exact commit under
`$XDG_DATA_HOME/techne/packages` (normally `~/.local/share/techne/packages`).
The same declaration reuses its pin offline. In a fresh editor call
`(package-restore!)` to register the installed packages, or repeat just the
install declarations you want. Both take `#:directory` to select another store.
An installed declaration reserves its package identity, commands and option names,
including while cold. Setting an existing option is not redeclaring it. Installation
rejects collisions before publishing the checkout; restore preflights the store.
If a publisher crashes leaving `.publishing` in the store, remove the empty directory
only after confirming that no install is in progress.
The store is managed data, not a working tree to edit. There is no central
catalog, dependency solver or automatic update of moving branches in this version.

In the package's directory, the root of a repository holding one package,
put **one datum**, `techne-package.scm`:

```scheme
(package (alice rust-mode)
  (version "0.1.0")
  (techne "0.4")
  (description "Rust editing commands and project helpers.")
  (editor "rust.scm"
    (commands (rust-check "Check the Rust project."))
    (options (rust-edition "2024" string "Edition used for new projects."))))
```

All clauses are pure data and may appear at most once; unknown or duplicate
clauses are refused. Neither installation nor completion evaluates this file:

- `(version "0.1.0")` — optional package version string.
- `(techne "0.4")` — optional minimum Techne runtime version string.
- `(description "...")` — optional human-readable description string.
- `(depends ((owner pkg) (path "relative/dir")) ...)` — optional dependencies on
  other package directories (below).
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

## Package directories

A package is a directory with that manifest, for the editor, cloud, or plain
libraries; `(techne package)` reads one and needs no editor
(`lisp/techne/package.sld`). A repository can hold several, side by side, as
`packages/` does here; a package's directory holds no other package.

```
mail/
  techne-package.scm              (package (techne mail) ...)
  lib/techne/mail/imap.sld        (techne mail imap)
  editor.scm                      the editor's entry, if it has one
  tests/
```

- **Identity & libraries.** A package's canonical name is a two-symbol list
  `(owner pkg)` where both `owner` and `pkg` are lowercase path components
  (letters, digits, `-`, `_`, `.`). Package `(owner pkg)` exclusively owns the
  libraries whose names begin with `(owner pkg)`: `(owner pkg)` is
  `lib/owner/pkg.sld` and `(owner pkg sub ...)` is `lib/owner/pkg/sub/....sld`
  in its directory and nowhere else. They belong to the world, not to a
  generation: one instance, whoever imports them, staying when the package is
  reloaded or unloaded. The top-level namespaces `scheme` and `srfi` are
  reserved and cannot be claimed by packages. First-party packages may use
  `techne` as their owner (such as `(techne mail)`): each package owns only its
  2-part prefix `(techne pkg ...)`, so it coexists beside Techne's stdlib
  libraries (`(techne http)`, `(techne path)`, `(techne package)`, ...), while
  colliding with a stdlib library or another registered `(owner pkg)` package
  is refused.
- **Dependencies.** `(depends ((techne mail) (path "../mail")))` names the
  packages whose libraries this one imports, each by its `(owner pkg)` identity
  and directory relative to this one, which must hold a package of that exact
  `(owner pkg)` name. A package's files import its own libraries and those of
  the packages it depends on itself: an import of another package's, one a
  dependency depends on included, is refused, saying what the manifest lacks.
  Libraries no package provides (Techne's own stdlib) and files in no package
  are not concerned. Cross-repository git dependencies and lockfiles are not
  part of this version.
- **Without the editor.** `techne run FILE` looks for the nearest manifest
  above FILE and uses that package and what it depends on first, so a
  package's programs and tests import its libraries and its dependencies'.
  `(use-package-directory! DIRECTORY)` does the same from Scheme. No code is
  loaded until something imports it.
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
Markdown preview use this mechanism. Keep
options and global bindings available early; package-private modes and hooks
needed only after opening a view can live with its implementation. An agent,
check-only or staged invocation cannot activate an unloaded package. A user's
normal invocation activates before argument acquisition/validation, and then
uses the real contract and operation. `unload-package` unloads the running
implementation, not the installed declarations or checkout.
