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
