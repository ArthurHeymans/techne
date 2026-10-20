# techne-hermit

Experiment, not for upstreaming: the Lisp (`techne-vm`, JIT included) as a
[Hermit](https://github.com/hermit-os/hermit-rs) unikernel, run in QEMU.

```sh
cd crates/techne-hermit
cargo build --release      # x86_64-unknown-hermit, build-std (.cargo/config.toml)
./run.sh                   # the REPL on the serial console
./run-file.py prog.scm     # like `techne-vm prog.scm`: output and exit status
# the Scheme suites in the guest, every mode:
TECHNE_VM_RUNNER=$PWD/run-file.py nix develop -c \
  cargo test --release -p techne-vm --features counting-allocator --test suites
```

All 402 suite runs pass in the guest, the expected failures the same as
natively. Speed is on par with Linux (interpreted fib 32: 0.18 s both).

## How

- **Files.** The guest has no disk. `lisp/` and the language suites are
  built in (`include_dir`); `run-file.py` hands over the program, the
  current directory and `TECHNE_LIBRARY_PATH` as an archive through QEMU's
  fw_cfg (`rep insb`, a page per VM exit). All are unpacked into Hermit's
  in-memory file system at their host paths.
- **Memory.** Hermit has no demand paging. The heap cage is 4 GiB of
  address space (`mmap` without access) backed with frames as it grows
  (`mprotect`), in 8 MiB steps: the guest gets 8 GiB, of which Hermit
  keeps a quarter for frames.
- **JIT.** Patched copies of `region` (Hermit's mman calls) and
  `cranelift-jit` (no `dlsym`; `region` instead of `memmap2`) in
  `patches/`.
- **Speed.** Hermit's `memset`/`memmove` (compiler-builtins) start `rep`
  instructions even for a few bytes; the interpreter clears and slides
  registers on every call. `src/mem.rs` replaces them. `run.sh` passes the
  host's TSC frequency (`-freq`), which Hermit would otherwise read from
  the CPU's name.
- **techne-vm**, under `cfg(target_os = "hermit")`: no Ctrl-C handler nor
  rustyline; `os_path` resolves `.`/`..` (Hermit's VFS does neither) and
  `canonicalize` is built on it; output files are removed before being
  created (Hermit ignores `O_TRUNC`). `suites.rs` takes `TECHNE_VM_RUNNER`.

## Not done

Networking (`techne-net`), processes, a REPL with line editing, other
architectures.
