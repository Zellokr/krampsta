#!/usr/bin/env bash
# Renders Krune's reference stills headlessly into assets/screenshots/ (no window, real renderer).
# Usage: scripts/snapshots.sh            (Git Bash on Windows works)
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -p krune-app
BIN=target/debug/krune
[ -x "$BIN.exe" ] && BIN="$BIN.exe"
OUT=assets/screenshots
mkdir -p "$OUT"
snap() { "$BIN" --snap "$OUT/$1.png" --size "${2:-1200x800}" --scale 1 "${@:3}" > /dev/null; echo "  $OUT/$1.png"; }
snap krune
for scene in empty running completed failed cancelled settings light; do snap "$scene" 1200x800 --scene "$scene"; done
# Narrow windows: give the side panes a moment to fold away (they close after the first layout).
snap narrow 800x560 --scene completed --at 1.0
snap send-message 1200x800 --script scripts/send-message.json --at mark:end
"$BIN" --tree "$OUT/krune-tree.json" --size 1200x800 > /dev/null && echo "  $OUT/krune-tree.json"
