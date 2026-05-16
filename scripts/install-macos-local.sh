#!/bin/zsh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="/Applications/DeskBridge.app"

DESKBRIDGE_USE_LOCAL_SIGNING=1 "$ROOT/scripts/package-macos-app.sh"

osascript -e 'quit app id "dev.deskbridge.mac"' >/dev/null 2>&1 || true
pkill -f "$APP/Contents/MacOS/deskbridge client" >/dev/null 2>&1 || true
sleep 1

rm -rf "$APP"
ditto --rsrc --extattr "$ROOT/build/DeskBridge.app" "$APP"
open "$APP"

"$APP/Contents/MacOS/deskbridge" permissions || true
