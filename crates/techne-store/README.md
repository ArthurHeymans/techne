# techne-store

A small asynchronous SQLite mechanism for Scheme. Rust is necessary for the
OS/database boundary, capability checks and cancellation-safe transaction
cleanup. Schema, migrations, mail and search policy are Scheme. This uses
rusqlite's bundled SQLite, including FTS5; it does not claim acceptance of the
full storage workstream in PLAN.md.

`techne_node::install` installs it, so the CLI, node sessions and editor can
`(import (techne sqlite))`. Other hosts can call `techne_store::install(vm)` or
`install_on(vm, tokio_handle)`. It is not implicitly exposed to tenant worlds.

## API

- `(sqlite-open PATH)` opens/creates a filename or `":memory:"`, returning a
  scope-owned handle. URI filenames are refused. Files capability is required,
  including for memory databases and every operation on an existing handle.
- `(sqlite-execute DB SQL PARAMS)` executes one non-row-returning statement and
  returns its affected-row count.
- `(sqlite-query DB SQL PARAMS)` executes one read-only statement and returns
  rows as lists. NULL is `#f`, INTEGER an exact int64, REAL a finite flonum,
  TEXT a string (invalid UTF-8 errors), BLOB a bytevector.
- `(sqlite-close DB)` is idempotent: it immediately makes the handle unusable
  and interrupts outstanding work. Worker rollback/closure is asynchronous.
- `(sqlite-closed? DB)` reports logical closure/worker termination.
- `(call-with-sqlite PATH PROC)` closes on return, failure or cancellation.
  Unowned handles also close on GC/VM drop.

PARAMS is always explicit: a list/vector of `#f`, int64s, finite flonums,
strings or bytevectors. Values are bound, never interpolated. SQL NULL and
Scheme false intentionally have the same representation.

Each statement has a native-owned transaction. Errors, cancellation and output
limits roll back even partial `INSERT OR FAIL` changes. Caller transaction
control, ATTACH/DETACH, extension loading and most PRAGMAs are denied. Only
`compile_options` and `data_version` read PRAGMAs are allowed. Foreign keys are
on, trusted schema off; only FTS5 virtual-table creation is exposed. No batches,
VACUUM, caller-managed transactions or write RETURNING. Cancellation racing a
finished commit does **not** prove a write failed: use idempotent keys.

## Bounds and execution

- SQL: 64 KiB; filename: 4096 bytes; neither may contain NUL.
- Parameters: 256, aggregate string/blob input 32 MiB; SQLite values/rows at
  most 32 MiB. This accommodates MIME sources without unbounded buffering.
- Query: 4096 rows / 40 MiB including estimated cell overhead, 128 columns.
  Exceeding limits errors, never silently truncates. Page in SQL.
- Five-second SQLite progress budget, checked every 1000 VM instructions;
  lock waits at most 250 ms. Cancellation is job-specific. These are not hard
  deadlines on filesystem calls, commit or rollback.
- At most 32 connections, each with a serialized blocking worker. VM/editor
  tasks remain runnable. A host-owned runtime must provide enough blocking
  threads, stay alive, and outlive its database handles.

Copied input/results use VM admission. SQLite's own working/cache allocations
are outside the VM heap budget. This is **not** a hostile-SQL, total-memory,
database-size or filesystem-path sandbox.

## Validation

`cargo test -p techne-store` covers binding, persistence, FTS5, atomic errors,
foreign keys, concurrency, closure/scope/VM drop, denied grants, restrictions,
limits, cancellation/kill, timeout rollback and host-owned runtimes.
