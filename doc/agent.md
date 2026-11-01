# Compact questions for agents

`(techne agent)` keeps repository snapshots and running programs in a Techne
world and returns bounded answers. It is available in `techne run`, node
sessions and nREPL; it does not require an editor window.

Start by asking what is available:

```sh
techne agent . '(agent-discover "read symbol")'
techne agent . '(agent-describe (quote agent-read))'
```

`techne agent ROOT FORM [SESSION]` is a small client of the existing node
protocol. `FORM` is one Scheme expression, with `project` bound to ROOT's
project. The form runs inside a lexical `let`: its internal definitions
are local to that invocation. For persistent helpers, define module bindings
in nREPL (or with `eval-source`), not internal CLI definitions. Use `list`
or `begin` for fail-fast batches, or `agent-batch` to keep partial answers.
The named session (`agent`
by default) survives client exits, so a handle from one invocation works in the
next. Use a different session name for independent work. These are trusted
sessions: other clients of the same session share the world, not isolated
workspaces or permissions.

## Questions, not dumps

```scheme
(agent-files project #:prefix "crates/" #:limit 5)
(agent-search project "string_ref" #:prefix "crates/techne-vm/" #:limit 3)
(agent-symbols project "crates/techne-vm/src/builtins.rs" #:name "string_ref")
(agent-read-symbol project "crates/techne-vm/src/builtins.rs" "string_ref")
(agent-read project "doc/manual.org" #:from 10 #:lines 12 #:chars 2000)
```

Replies are proper association lists: use `(agent-ref reply 'items)`, not `cdr`
of an entry. Lists report `total`, `complete`, `truncated`, and `next` (an
item offset or `#f`). Search totals count matching lines, not occurrences.
A search with skipped files or a work limit has `complete #f`: its total is
only a lower bound. Inspect `skipped` and `problems`, and narrow `#:prefix`.
Non-UTF-8 files are counted as `binary` and excluded from text search, not
treated as traversal failures. Directory errors yield partial listings with
problems instead of aborting the scan. The `revision` from `agent-files`
hashes the cached relative file-name list, not file contents or a VCS revision.
Results are ordered by file name and line. A search page is a fresh query,
not a frozen repository-wide transaction. Search items include
`preview-truncated`, a zero-based `preview-offset` within the line handle,
and the 1-based Unicode `column` of the first match.

Reads and individual search/symbol items carry a content revision and an
opaque handle. A read's `next` is a Unicode character offset **within that
handle**, not a line or a byte offset. `characters` gives the full selected
handle length, not just the returned text. Read line numbers start at 1; a final
newline terminates the final line, not an extra line. An empty file has an
empty line 1. Expanding at the end of a handle may return empty text with
an EOF cursor position:

```scheme
(define answer (agent-read project "doc/manual.org" #:chars 100))
(agent-expand project (agent-ref answer 'handle)
              #:offset (agent-ref answer 'next) #:chars 100)
```

A handle reads the immutable snapshot it names, even after the file changes.
A new `agent-read` sees disk changes. Handles are scoped to their project and
cannot silently identify another snapshot after a server restart. Evicted
handles fail explicitly: query again. Call `(agent-refresh! project)` after
creating or removing files; it relists files and expires cached handles.

Discovery searches actual live binding names, signatures and docstrings via
`defined-names` and `binding-description`. Module-level definitions and
redefinitions in nREPL or `eval-source` update their signatures and docs
immediately. Lexical variables are not module bindings: a local CLI helper
or local shadow does not appear in module discovery. The default module is
`user`; choose `#:module` explicitly when inspecting another module. There
is no second, manually maintained tool catalog.

Signature previews have a separate `signature-truncated` flag. Retrieve a
long signature with `(agent-describe name #:part (quote signature))`; `text`,
`truncated` and `next` page the selected part. The default part is `doc`,
which preserves the docstring window in the `doc` field as well.

Symbols use exact `#:name` matching, Scheme reader spans and Rust syntax
trees. Scope labels distinguish `module`, `member`, `local` and `library`
definitions; library names retain their full list spelling. `line` and
`end-line` are 1-based and inclusive. Rust spans include attached outer
attributes and doc comments, including cfg, test and derive attributes.
Rust parse recovery sets `complete #f`. Completeness describes syntactic
traversal of supported forms, not every semantic or generated runtime
binding: macros, record accessors and conditional compilation are not
evaluated. Unsupported languages fail explicitly; use literal search instead. This first version
reads disk files: it does **not** include unsaved editor buffers, LSP semantic
symbols, or project-wide references. It neither loads source modules to
inspect them nor executes the repository's code during a source query.

## Several answers, with isolated failures

`agent-batch` calls up to 20 thunks, returning an `ok` flag and either a
`value` or a bounded `error` per thunk. A bad path does not discard answers
from the other regions. Each thunk should itself return bounded data.

```scheme
(agent-batch
  (list (lambda () (agent-read project "one.scm" #:from 10 #:lines 8))
        (lambda () (agent-read project "two.rs" #:from 40 #:lines 12))
        (lambda () (agent-describe (quote agent-expand)))))
```

## Checked edits and compact change summaries

```scheme
(define before (agent-read project "example.scm"))
(agent-edit! project "example.scm" (agent-ref before 'revision)
             '(("(define answer 41)" "(define answer 42)")
               ("old unique text" "new text")))
```

All replacements refer to the **original** file. Old text must be nonempty
and occur exactly once, including overlapping occurrences; replacement
ranges cannot overlap. Every replacement is validated before a write. The
reply gives the new revision, a handle, and bounded before/after summaries.
There is no implicit retry on a stale revision. A lost connection may hide
a committed write: read the file again before deciding whether to retry.

Writes use the existing conditional atomic-rename file operation and preserve
permissions. This is not a lock against external writers between the last
check and rename, and batches spanning multiple files are not transactional.
This interface edits existing UTF-8 files; creation/deletion and VCS diff
summaries are not included yet. `agent-edit!` supplies per-edit diff summaries.

## Processes without transcript-sized logs

```scheme
(define build
  (agent-run project "cargo" '("test" "-p" "techne-node") #:retain 4000))
(agent-process-result build #:chars 2000)
```

`agent-run` takes an argv list, not a command string. It runs in the project
root. `#:environment` is a list of `(name . value)` strings added to the
inherited environment. Entries are validated before launching: names are
nonempty, cannot contain `=`, and neither names nor values can contain NUL.
Malformed values are not echoed in errors. Stdout and stderr drain
concurrently into bounded tails. Each stream reports `characters` seen,
`retained` characters, `dropped` (retention loss), `clipped` (a smaller
requested report window), and `truncated` (either kind of omission). These
tails do not claim to classify every error. The result is repeatable. `agent-process-done?` checks completion and
`agent-process-cancel!` kills the process group and drains remaining output.
The current Techne scope owns the job.

In the command-line adapter, normally combine run and result in one form:

```sh
techne agent . '(agent-process-result (agent-run project "cargo" (quote ("test" "-p" "techne-node"))))'
```

Use nREPL for long-lived helper definitions and job variables. Import
`(techne agent)`, define `(agent-project "/absolute/root")`, then issue the
same queries. The adapter adds no wire protocol or nREPL operations. A
running session retains the version of the library it loaded: after changing
native code, use a new session/server; refresh only reloads source snapshots,
not running code.

## Bounds and trust

- A text file or edited result is at most 1 MiB, in UTF-8.
- Lists return at most 50 items; read windows at most 200 lines or 16,000
  characters. Defaults are deliberately smaller.
- File discovery respects ignore rules and does not traverse symlink
  directories; it lists at most 10,000 files. Use a smaller project root if
  listing hits that bound; a prefix filters the already-listed names.
- A search processes at most 64 MiB of file data, including binary files;
  it can fetch one additional file while checking that boundary. It rereads
  files to check revisions; this is not an incremental content index.
- A project retains at most 16 MiB and 128 snapshots. Returned search handles
  are retained even while later files are counted. A page may be shorter
  than requested to keep its snapshots within that budget; follow `next`.
- The session cache retains at most eight project roots: at full capacity
  that is 128 MiB of source and 1024 snapshots, plus file lists and metadata.
  Cache hits reuse the project without allocating another random namespace.
  Explicitly retained `agent-project` values are additional and live as long
  as their owners retain them.
- Processes retain at most 16,000 characters per stream; result text is at
  most 8,000 characters per stream.

These limits bound the library's answers and retained source data, not
arbitrary Scheme evaluation, user-created values, OS process resources, or
the entire session's memory. Queries require the files capability; starting
processes also requires processes. Paths are canonicalized and refused when
outside the selected root, but this is a trusted-local convenience, **not a
filesystem sandbox** against concurrent path replacement or hostile code.

Batch related queries and keep their results as Scheme values. Only return
the fields needed for the next decision: changing syntax alone does not
reduce transcript traffic.
