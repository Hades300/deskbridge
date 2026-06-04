#!/usr/bin/env bash
# Linux GUI end-to-end smoke test for the DeskBridge desktop app.
#
# Launches the real app under a virtual display, asserts it starts without
# crashing, drives a Connect through the GUI with simulated input, and verifies
# the click reaches the daemon (a server-side "client accepted"). Screenshots
# are written to gui-shots/ for visual review.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
APP="$ROOT/apps/desktop/src-tauri/target/release/deskbridge-desktop"
DAEMON="$ROOT/target/release/deskbridge"
SHOTS="$ROOT/gui-shots"
mkdir -p "$SHOTS"

export DISPLAY=:99
export WEBKIT_DISABLE_COMPOSITING_MODE=1
export WEBKIT_DISABLE_DMABUF_RENDERER=1
export LIBGL_ALWAYS_SOFTWARE=1
export GDK_BACKEND=x11
export DESKBRIDGE_BIN="$DAEMON"

cleanup() {
  [[ -n "${APP_PID:-}" ]] && kill "$APP_PID" 2>/dev/null || true
  [[ -n "${SRV_PID:-}" ]] && kill "$SRV_PID" 2>/dev/null || true
  [[ -n "${XVFB_PID:-}" ]] && kill "$XVFB_PID" 2>/dev/null || true
}
trap cleanup EXIT

Xvfb :99 -screen 0 1280x900x24 -nolisten tcp >/tmp/xvfb.log 2>&1 &
XVFB_PID=$!
sleep 2

# A real server to connect to (plaintext, allows the "mac" screen).
"$DAEMON" server --listen 127.0.0.1:24850 --name windows --allow mac >/tmp/dbserver.log 2>&1 &
SRV_PID=$!
for _ in $(seq 1 40); do grep -q "server listening" /tmp/dbserver.log && break; sleep 0.3; done

"$APP" >/tmp/dbapp.log 2>&1 &
APP_PID=$!

# The app must launch and create a window (this catches startup crashes).
window=""
for _ in $(seq 1 40); do
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    echo "ERROR: desktop app exited during startup"; cat /tmp/dbapp.log; exit 1
  fi
  window="$(xdotool search --name DeskBridge 2>/dev/null | head -1 || true)"
  [[ -n "$window" ]] && break
  sleep 0.5
done
[[ -n "$window" ]] || { echo "ERROR: app window never appeared"; cat /tmp/dbapp.log; exit 1; }

# Pin the window to a known position so click coordinates are deterministic.
xdotool windowmove "$window" 0 0 || true
xdotool windowactivate "$window" 2>/dev/null || true
sleep 2
import -window root "$SHOTS/01-launch.png"

# Drive a Connect through the GUI: focus the Server field, replace its text,
# then click Connect. Coordinates match the 920x720 window at the origin.
xdotool mousemove 378 173 click 1
xdotool key --clearmodifiers ctrl+a
xdotool type --clearmodifiers "127.0.0.1:24850"
xdotool mousemove 775 278 click 1

# Deterministic proof the GUI click drove a real connection.
accepted=0
for _ in $(seq 1 30); do
  if grep -q "client accepted" /tmp/dbserver.log; then accepted=1; break; fi
  sleep 0.4
done
import -window root "$SHOTS/02-connect.png"
if [[ "$accepted" != 1 ]]; then
  echo "ERROR: GUI Connect did not produce a server-side 'client accepted'"
  echo "--- server log ---"; cat /tmp/dbserver.log
  echo "--- app log ---"; cat /tmp/dbapp.log
  exit 1
fi
echo "OK: GUI Connect reached the daemon (client accepted)"

# Visual capture of discovery (best-effort; not asserted).
xdotool mousemove 788 394 click 1
sleep 5
import -window root "$SHOTS/03-discover.png"

echo "GUI end-to-end smoke passed."
