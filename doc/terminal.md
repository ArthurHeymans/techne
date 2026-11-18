# Terminal buffers

`M-x terminal` opens a new PTY running `$SHELL` (or `sh`) in the current
buffer's local directory. `M-x terminal-command` prompts for a command to
run via `sh -c` on its own PTY. Both commands work in the window and
terminal frontends, with either editor key profile.

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

The child can request bracketed paste. In copy mode, `M-x terminal-yank`
can also send the latest kill to the child. Switching buffers leaves the
child running. To stop it and discard the buffer, use `M-x kill-buffer`.
An interactive shell that exits successfully (for example, after `C-d` at
an empty prompt) closes its buffer. Set `terminal-close-on-exit` to `#f`
to keep it. Failed or interrupted shells and `terminal-command` runs
retain their output and exit status.

Ebb's prompt and hyperlink navigation shortcuts are not bound: they need
shell integration or linked cells, which this terminal does not yet track.

The terminal uses the existing PTY primitives and advertises
`xterm-256color` with `COLORTERM=truecolor`. It implements common VT and
xterm operations used by shells and text TUIs: scroll margins, main and
alternate screens, cursor addressing, application cursor keys,
ANSI/indexed/RGB colors, and DEC line drawing. It also handles horizontal
margins, protected cells, rectangular editing, and VT mode, setting, and
screen checksum reports. Child-requested grid sizes are bounded, and do
not resize the editor window. A focused terminal follows pane size changes.
DEC double-width and double-height rows use double-width spacing with
normal-size glyphs. Scrollback retains up to 2,000 physical rows of the
main screen; it is not reflowed, and visual wraps copy as separate lines.
Wide CJK characters and common combining sequences are supported; this is
not a complete Unicode grapheme or xterm implementation. Mouse and focus
reporting, graphics, and OSC clipboard access are not supported.

The Scheme implementation follows the architecture of `el-be-back`: an
independent cell grid and incremental VT parser, cached row projections
into a read-only editor document, and asynchronous PTY I/O. The parser
works in bounded slices, while a separate Scheme task batches output into
one projection roughly every 16 ms during a burst. EOF flushes the last
update immediately. The screen, parser and key encoder can be used
without the editor as `(techne terminal)`.

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
Emacs-specific renderer, shell integration, graphics, or serial support.
