#!/usr/bin/env bash
# Renders Krune's reference stills headlessly into assets/screenshots/ (no window, real renderer).
# Usage: scripts/snapshots.sh            (Git Bash on Windows works)
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -p krune-app
# Honour CARGO_TARGET_DIR, so stills can be rendered while target/debug/krune.exe is running
# (Windows locks a running executable).
BIN="${CARGO_TARGET_DIR:-target}/debug/krune"
[ -x "$BIN.exe" ] && BIN="$BIN.exe"
OUT=assets/screenshots
# The simulated agent, whatever keys this machine has: stills must not depend on the environment
# (nor wait on a network model list).
export KRUNE_PROVIDER=mock
mkdir -p "$OUT"
snap() { "$BIN" --snap "$OUT/$1.png" --size "${2:-1200x800}" --scale 1 "${@:3}" > /dev/null; echo "  $OUT/$1.png"; }
snap krune
for scene in empty running completed failed cancelled settings light provider-picker model-picker model-error; do snap "$scene" 1200x800 --scene "$scene"; done
# Narrow windows: give the side panes a moment to fold away (they close after the first layout).
snap narrow 800x560 --scene completed --at 1.0
snap send-message 1200x800 --script scripts/send-message.json --at mark:end
"$BIN" --tree "$OUT/krune-tree.json" --size 1200x800 > /dev/null && echo "  $OUT/krune-tree.json"
