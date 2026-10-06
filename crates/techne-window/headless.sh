#!/usr/bin/env bash
# Run a command in a headless Wayland session (sway), never the desktop's.
#
#   crates/techne-window/headless.sh COMMAND...
#
# Inside, WAYLAND_DISPLAY is the headless sway and DISPLAY is unset, so a
# window cannot land on the real desktop. `grim FILE.png` takes a
# screenshot and `wtype` types; a new wtype loses its first key, so start
# each with a throwaway one (`wtype -M shift -m shift ...`). With a GPU
# render node, sway composites the client's GPU buffers; without one it
# falls back to software rendering. Needs `nix develop .#window`.
set -euo pipefail

dir=$(mktemp -d /tmp/techne-headless.XXXXXX)
chmod 700 "$dir"
cleanup() {
    [[ -n ${sway:-} ]] && kill "$sway" 2>/dev/null && wait "$sway" 2>/dev/null
    rm -rf "$dir"
}
trap cleanup EXIT

cat >"$dir/sway.conf" <<'EOF'
xwayland disable
output HEADLESS-1 resolution 1600x1000 scale 1
default_border none
EOF

render=$(ls /dev/dri/renderD* 2>/dev/null | head -1 || true)
# On NixOS with NVIDIA's EGL installed next to Mesa's, sway must use Mesa's
# for the render node.
mesa_egl=/run/opengl-driver/share/glvnd/egl_vendor.d/50_mesa.json
[[ -e $mesa_egl ]] && export __EGL_VENDOR_LIBRARY_FILENAMES=$mesa_egl

env -u WAYLAND_DISPLAY -u DISPLAY -u SWAYSOCK \
    XDG_RUNTIME_DIR="$dir" WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 \
    ${render:+WLR_RENDER_DRM_DEVICE=$render} \
    sway -c "$dir/sway.conf" >"$dir/sway.log" 2>&1 &
sway=$!

for _ in $(seq 100); do
    [[ -S $dir/wayland-1 ]] && break
    sleep 0.05
done
[[ -S $dir/wayland-1 ]] || { cat "$dir/sway.log" >&2; exit 1; }

export XDG_RUNTIME_DIR=$dir WAYLAND_DISPLAY=wayland-1
unset DISPLAY SWAYSOCK
"$@"
