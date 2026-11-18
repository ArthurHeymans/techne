# Terminal buffers

`M-x terminal` opens a new PTY running `$SHELL` (or `sh`) in the current
buffer's directory. `M-x terminal-command` prompts for a command to run via
`sh -c` on its own PTY. Both work in the window and terminal frontends.

From a remote file such as `/ssh:HOST:/work/notes`, the editor and emulator
stay local while the shell and PTY run on the node for that route. The
interactive command uses the endpoint account's shell; Bash, Zsh and Fish
start as login shells. The usual Techne remote access configuration and
noninteractive SSH authentication apply. Killing the buffer stops the PTY.
If the connection is lost, the output remains readable; open a new terminal
to reconnect rather than expecting a detached session to resume.

Terminals start in **semi-char** mode, as in Ebb. Typing goes to the child;
editor shortcuts such as `M-x` and `C-x` stay with the active editor profile.
`terminal-editor-keys` controls which ordinary keys are left to the editor.

| Key | Action |
| --- | --- |
| `C-c C-e` | Copy mode: use the usual editor keys to browse, select, and copy. |
| `C-c C-j` | Return to semi-char mode. |
| `C-c M-d` | Char mode: send editor shortcuts as well as ordinary keys to the child. |
| `C-M-m` (or `M-RET`) | Leave char mode for semi-char mode. |
| `F12` | Toggle between terminal input and copy mode, returning to semi-char. |
| `C-c C-c` | Interrupt the foreground child; in char mode use plain `C-c`. |
| `C-q` | In semi-char mode, send the next key literally, even an editor shortcut. |
| `C-y`, `S-Insert`, `C-S-v` | In semi-char mode, paste the latest kill or imported clipboard text. |
| `M-y` | In semi-char mode, paste the next older kill. It does not erase text already sent. |
| `C-c C-k` | Stop the process without discarding the buffer. |
| `C-c C-p` / `C-c C-n` | Go to the preceding / following shell prompt. |
| `C-c M-p` / `C-c M-n` | Go to the preceding / following hyperlink. |
| `RET` in copy mode | Open the HTTP(S) hyperlink at point with `xdg-open`. |
| `C-c C-w` | Read a hidden answer for a child already waiting for a password. |

The child can request bracketed paste. In copy mode, `M-x terminal-yank`
can also send the latest kill to the child. Switching buffers leaves the
child running. To stop it and discard the buffer, use `M-x kill-buffer`.
An interactive shell that exits successfully (for example, after `C-d` at
an empty prompt) closes its buffer. Set `terminal-close-on-exit` to `#f`
to keep it. Failed or interrupted shells and `terminal-command` runs
retain their output and exit status.

For prompt navigation and current-directory tracking, source the matching
script in `lisp/terminal-integration/` from your Bash, Zsh or Fish config,
on the machine running the shell. Techne sets `TERM_PROGRAM=techne`.
Scripts are not injected into a remote account. OSC 7 can update the path
of a terminal but cannot change its route or connect to a new host. OSC 133
marks prompts and OSC 8 marks HTTP(S) hyperlinks in the grid.

Use the hidden password prompt only when the child has disabled echo. It
bypasses the editor's normal key hooks and does not insert the answer into
a buffer or command history. `terminal-detect-passwords` is opt-in: it
checks for canonical input with echo off after output, on both local and
updated remote nodes. That signal is a heuristic, not proof of a password
prompt. It is not an SSH askpass or a credential store.

`M-x terminal-serial` opens a local device under `/dev` through `socat`
(which must be on `PATH`). Set `terminal-serial-baud` to select the speed
(default 115200). This is a basic raw serial console, not a serial settings
or reconnect manager.

The terminal uses the existing PTY primitives and advertises
`xterm-256color` with `COLORTERM=truecolor`. It implements common VT and
xterm operations used by shells and text TUIs: scroll margins, main and
alternate screens, cursor addressing, application cursor keys,
ANSI/indexed/RGB colors, and DEC line drawing. It also handles horizontal
margins, protected cells, rectangular editing, and VT mode, setting, and
screen checksum reports. Child-requested grid sizes are bounded, and do
not resize the editor window. A focused terminal follows pane size changes.
DEC double-width and double-height rows use double-width spacing with
normal-size glyphs. Scrollback retains up to 2,000 rows of the main screen.
Saved text reflows on width changes; the live and alternate grids do not.
Visual wraps copy as separate lines. Wide CJK characters and common
combining sequences are supported. Applications can request xterm mouse
and focus reports in both frontends. Shift or copy mode keeps mouse input
with the editor. OSC clipboard access is not supported.

The window frontend draws traditional Kitty inline placements. The direct
base64 medium accepts chunked PNG and raw RGB/RGBA within fixed storage
and pixel limits. File, temporary-file and shared-memory transfers are
refused, even for local children. Virtual placements support whole rectangles
with an on-screen origin placeholder, explicit cell dimensions, and a 24-bit
image ID. Per-cell slices, source cropping, compression, animation, and image
scrollback are not implemented.
The text frontend does not draw image overlays.

The Scheme implementation follows the architecture of `el-be-back`: an
independent cell grid and incremental VT parser, cached row projections
into a read-only editor document, and asynchronous PTY I/O. The parser
works in bounded slices, while a separate Scheme task batches output into
one projection roughly every 16 ms during a burst. EOF flushes the last
update immediately. The screen, parser and key encoder can be used
without the editor as `(techne terminal)`.

## Benchmarks

The `terminal-bench` example runs the Scheme parser and document projection
on the same four byte streams Ebb uses: plain scrolling text, indexed SGR
colors, uniform full-screen updates, and per-cell checkerboard colors. Each
sample starts with a fresh 80×24 grid and document, feeds the entire stream,
and projects it once. Both runners warm up and report the median of five
samples by default. No PTY, asynchronous batching, or frontend drawing is
measured. The projects retain and materialize scrollback differently, so
these are measurements of their shipped pipelines, not a language benchmark.

Run the two on the same machine, one after the other, without a competing
build. The arguments are bytes, iterations, and an optional TSV output path:

```sh
cargo run --release -p techne-editor --example terminal-bench -- 262144 5 /tmp/techne-terminal.tsv
EBB_BENCH_NATIVE=1 bash ../el-be-back/bench/run-ebb-bench.sh \
  --size 262144 --iterations 5 --out /tmp/ebb-terminal.tsv
techne run lisp/bench/compare-terminal.scm /tmp/techne-terminal.tsv /tmp/ebb-terminal.tsv
```

Use an Emacs with native compilation for the Ebb numbers; omitting
`EBB_BENCH_NATIVE` measures byte-compiled Elisp instead. The Techne example's
`--fixtures DIR [BYTES]` mode writes the four input streams without timing
for checking or replaying them.

## Compatibility testing

The terminal passes Ebb's focused `esctest2` selection: **258 passed,
28 known bugs, 0 failed**. The test runs the unmodified Python suite
through a real child PTY in `techne term`. To reproduce, use `tmux`,
`python3`, and [esctest2](https://github.com/ThomasDickey/esctest2) at
revision `664be3cf2c1e3f06bc93a8bafb48a0db83c607db`:

```sh
./target/debug/techne run lisp/tests/terminal/esctest.scm \
  /path/to/esctest2 ./target/debug/techne
```

This is VT protocol parity on that selection, not parity with Ebb's
Emacs-specific features or every graphics and serial extension.
