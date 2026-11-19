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
The store is managed data, not a working tree to edit. There is no central
catalog, dependency solver or automatic update of moving branches in this version.

In the root of the repository put **one datum**, `techne-package.scm`:

```scheme
(package rust-mode "rust.scm"
  ((rust-check "Check the Rust project."))
  ((rust-edition "2024" string "Edition used for new projects.")))
```

The first list advertises public command names and short descriptions. The
second declares early options as `(name default schema documentation)`; use
`()` for no options. `rust.scm` imports `(techne editor)` and uses the normal
`define-command`; it need not redeclare its early options. Neither installation
nor completion evaluates this file. First use runs **trusted Scheme code** in
the editor, not in a package sandbox. Review the source before installing.

For a built-in, `register-command-package!` registers the same command offers
from a local path. Mail, Ibuffer and Markdown preview use this mechanism. Keep
options and global bindings available early; package-private modes and hooks
needed only after opening a view can live with its implementation. An agent,
check-only or staged invocation cannot activate an unloaded package. A user's
normal invocation activates before argument acquisition/validation, and then
uses the real contract and operation. `unload-package` unloads the running
implementation, not the installed declarations or checkout.
