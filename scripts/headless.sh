#!/usr/bin/env bash
# Runs jotty inside a headless sway with software rendering, optionally
# drives it with an actions script, and saves a screenshot. Nothing appears
# on your screen.
#
# Usage:
#   nix shell nixpkgs#sway nixpkgs#grim nixpkgs#wtype -c \
#     scripts/headless.sh [-a ACTIONS] [-w SECONDS] OUT.png -- JOTTY [ARGS...]
#
# ACTIONS is a shell script sourced while jotty runs; it can use `wtype`
# (virtual keyboard), `swaymsg` and `grim`. Type everything in one `wtype`
# call: each call creates and removes a virtual keyboard, and the first keys
# after the seat regains a keyboard can be lost.
#
# Logs end up in $JOTTY_HEADLESS_DIR (default: a fresh temporary directory).
set -euo pipefail

actions=""
wait_secs=2
while getopts "a:w:" opt; do
    case $opt in
        a) actions=$OPTARG ;;
        w) wait_secs=$OPTARG ;;
        *) exit 2 ;;
    esac
done
shift $((OPTIND - 1))
out=${1:?screenshot path required}
shift
[ "${1:-}" = "--" ] && shift
[ $# -gt 0 ] || { echo "jotty command required" >&2; exit 2; }

for tool in sway grim; do
    command -v "$tool" >/dev/null || { echo "$tool not found in PATH" >&2; exit 1; }
done

dir=${JOTTY_HEADLESS_DIR:-$(mktemp -d)}
# Wayland socket paths must be short; use the runtime directory.
runtime=$(mktemp -d "${XDG_RUNTIME_DIR:-/tmp}/jotty-XXXX")
trap 'kill $(jobs -p) 2>/dev/null || true; rm -rf "$runtime"' EXIT

cat >"$dir/sway.conf" <<'EOF'
output HEADLESS-1 resolution 900x560 scale 1
default_border none
EOF

export XDG_RUNTIME_DIR=$runtime
unset WAYLAND_DISPLAY DISPLAY SWAYSOCK
WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 \
    sway -c "$dir/sway.conf" >"$dir/sway.log" 2>&1 &

for _ in $(seq 50); do
    socket=$(find "$runtime" -maxdepth 1 -name 'wayland-*' ! -name '*.lock' | head -1)
    [ -n "$socket" ] && break
    sleep 0.1
done
export WAYLAND_DISPLAY=${socket##*/}
SWAYSOCK=$(find "$runtime" -maxdepth 1 -name 'sway-ipc.*' | head -1)
export SWAYSOCK

# Mesa's software renderer works with sway's pixman renderer.
export LIBGL_ALWAYS_SOFTWARE=1
mesa=/run/opengl-driver/share/glvnd/egl_vendor.d/50_mesa.json
[ -f "$mesa" ] && export __EGL_VENDOR_LIBRARY_FILENAMES=$mesa

RUST_LOG=${RUST_LOG:-info} "$@" >"$dir/jotty.log" 2>&1 &
sleep 1.5
if [ -n "$actions" ]; then
    # shellcheck source=/dev/null
    . "$actions"
fi
sleep "$wait_secs"
grim "$out"
echo "screenshot: $out, logs: $dir"
