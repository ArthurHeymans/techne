# Editor packages

In an editor Scheme session:

```scheme
(import (techne packages))
(package-install! '(github "alice/techne-rust" (tag "v0.3"))) ; illustrative repository
```

`github` is data, not a procedure. Sources are `(github "owner/repo")`,
`(git "https://…")`, or `(local "/absolute/path/to/git/repository")`. Append
`(branch "name")`, `(tag "name")`, or `(commit "full-object-id")` to select a
revision. The default is the repository's HEAD. To give a source a catalog
name, `(register-package-recipe! '(recipe rust-mode (github "alice/techne-rust")))`
then `(package-install! 'rust-mode)`. The name must match the manifest.

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

In the root of the repository put **one datum**, `techne-package.scm`:

```scheme
(package rust-mode
  (editor "rust.scm"
    (commands (rust-check "Check the Rust project."))
    (options (rust-edition "2024" string "Edition used for new projects."))))
```

`commands` advertises public command names and short descriptions. `options`
declares early options as `(name default schema documentation)` and may be
left out. `rust.scm` imports `(techne editor)` and uses the normal
`define-command`; it need not redeclare its early options. Neither installation
nor completion evaluates this file. First use runs **trusted Scheme code** in
the editor, not in a package sandbox. Review the source before installing.
The form this was first written in, `(package rust-mode "rust.scm" (COMMAND
...) (OPTION ...))`, is still read and means the same.

## Package directories

A package is a directory with that manifest, for the editor or without it;
`(techne package)` reads one and needs no editor (`lisp/techne/package.sld`).
A repository can hold several, side by side, as `packages/` does here.

```
mail/
  techne-package.scm
  lib/mail/imap.sld      (mail imap)
  editor.scm             the editor's entry, if it has one
  tests/
```

- **Libraries.** The package `mail` owns the libraries whose names begin with
  `mail`: `(mail imap)` is `lib/mail/imap.sld` in its directory and nowhere
  else. They belong to the world, not to a generation: one instance,
  whoever imports them, staying when the package is reloaded or unloaded.
  A package cannot take the names `techne`, `srfi` or `scheme`, nor one
  another directory already has.
- **Dependencies.** `(depends (mail (path "../mail")))` names the packages
  whose libraries this one imports, each by its directory relative to this
  one, which must hold a package of that name. There are no versions and
  no fetching of dependencies in this version, and nothing checks yet that
  a package imports only what it declares.
- **Without the editor.** `techne run FILE` looks for the nearest manifest
  above FILE and uses that package and what it depends on first, so a
  package's programs and tests import its libraries and its dependencies'.
  `(use-package-directory! DIRECTORY)` does the same from Scheme. No code is
  loaded until something imports it.
- **In the editor.** The options of the manifest's `editor` clause are
  declared and its commands offered, without loading the entry
  (`advertise-package!`). Installed packages and the built-in mail
  (`packages/mail`) are advertised this way. The entry and the files it
  requires from its directory, other than `lib/`, load as a generation and
  reload with the package. The packages Techne ships are advertised at
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
