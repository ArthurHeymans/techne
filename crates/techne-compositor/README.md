# techne-compositor

Techne's Wayland compositor. It is a fork of the compositor of EWM, the Emacs
Wayland Manager (<https://codeberg.org/ezemtsov/ewm>), by Evgeny Zemtsov and
contributors, imported from EWM's `compositor/` directory at commit
`41c0c125772d06d28de7cfb2983cea48bf4c38d1`.

EWM and this fork are licensed under the GNU General Public License, version 3
or (at your option) any later version; see `LICENSE` at the repository root.
Files keep the copyright of their authors; the history of the imported code is
in the EWM repository.

The import is kept verbatim in its own change, so later changes show what
Techne altered: the Emacs dynamic-module boundary is replaced by Techne's
policy protocol, and the layout and focus models are generalized beyond Emacs
frames (see `PLAN.md`, Stage 1 workstream B).

## Running

Build and run inside `shell.nix` (the system libraries):

    nix-shell crates/techne-compositor/shell.nix --run \
      'cargo run -p techne-compositor -- --nested'

`--nested` runs in a window of the current Wayland or X11 session; without it
the compositor takes this TTY (DRM). The socket is `wayland-techne` (plus
`-vtN` on a VT). No policy owner connects yet: windows are mapped but not
placed, and the events the policy would receive are logged.
Headless check: run it nested under Xvfb with `LIBGL_ALWAYS_SOFTWARE=1`, then
a client and `grim` against the socket.
