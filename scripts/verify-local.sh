#!/bin/zsh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

source "$HOME/.cargo/env"

echo "== Rust format/test/lint =="
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

if rustup target list --installed | grep -qx "x86_64-pc-windows-msvc"; then
  echo "== Windows target check =="
  cargo check --target x86_64-pc-windows-msvc -p deskbridge
  cargo clippy --target x86_64-pc-windows-msvc -p deskbridge --all-targets -- -D warnings
else
  echo "== Windows target check skipped =="
  echo "Install with: rustup target add x86_64-pc-windows-msvc"
fi

echo "== macOS SwiftPM build =="
cd "$ROOT/apps/macos"
swift build
cd "$ROOT"

echo "== Package app bundle =="
"$ROOT/scripts/package-macos-app.sh" >/tmp/deskbridge-package.out
cat /tmp/deskbridge-package.out

echo "== Config smoke test =="
TMP_CONFIG="$(mktemp /tmp/deskbridge-config.XXXXXX)"
"$ROOT/target/debug/deskbridge" init-config --path "$TMP_CONFIG"
"$ROOT/target/debug/deskbridge" diag --config "$TMP_CONFIG" >/tmp/deskbridge-diag-offline.out 2>&1 || true
if ! grep -q "DeskBridge diagnostics" /tmp/deskbridge-diag-offline.out; then
  cat /tmp/deskbridge-diag-offline.out
  echo "diag did not start"
  exit 1
fi

echo "== Route simulation test =="
"$ROOT/target/debug/deskbridge" simulate-route --config "$TMP_CONFIG" --steps 3 --dx 80 --dy -2 >/tmp/deskbridge-route-simulation.out
if ! grep -q "event 0: target=mac MouseAbs x=1 y=559" /tmp/deskbridge-route-simulation.out; then
  cat /tmp/deskbridge-route-simulation.out
  echo "route simulation did not cross to mac"
  exit 1
fi
if ! grep -q "event 3: target=mac MouseMove dx=80 dy=-2" /tmp/deskbridge-route-simulation.out; then
  cat /tmp/deskbridge-route-simulation.out
  echo "route simulation did not keep routing relative mouse movement"
  exit 1
fi
"$ROOT/target/debug/deskbridge" simulate-route --config "$TMP_CONFIG" --steps 2 --dx 80 --dy 0 --return-dx -200 --return-dy 0 >/tmp/deskbridge-route-return.out
if ! grep -q "release 3: active=windows" /tmp/deskbridge-route-return.out; then
  cat /tmp/deskbridge-route-return.out
  echo "route simulation did not release input back to windows"
  exit 1
fi

echo "== Loopback protocol test =="
SERVER_LOG="$(mktemp /tmp/deskbridge-server.XXXXXX)"
CLIENT_LOG="$(mktemp /tmp/deskbridge-client.XXXXXX)"
"$ROOT/target/debug/deskbridge" server --listen 127.0.0.1:24881 --allow mac --demo-events >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!
cleanup() {
  kill "$SERVER_PID" >/dev/null 2>&1 || true
  if [[ -n "${RECONNECT_SERVER_PID:-}" ]]; then
    kill "$RECONNECT_SERVER_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "${RECONNECT_CLIENT_PID:-}" ]]; then
    kill "$RECONNECT_CLIENT_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "${DEBUG_SERVER_PID:-}" ]]; then
    kill "$DEBUG_SERVER_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "${DEBUG_CLIENT_PID:-}" ]]; then
    kill "$DEBUG_CLIENT_PID" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT
sleep 1
"$ROOT/target/debug/deskbridge" diag --server 127.0.0.1:24881 --name mac
"$ROOT/target/debug/deskbridge" client --server 127.0.0.1:24881 --name mac --dry-run --max-events 1 --once >"$CLIENT_LOG" 2>&1

echo "== Debug control test =="
DEBUG_SERVER_LOG="$(mktemp /tmp/deskbridge-debug-server.XXXXXX)"
DEBUG_CLIENT_LOG="$(mktemp /tmp/deskbridge-debug-client.XXXXXX)"
DEBUG_DISPLAY_OUT="$(mktemp /tmp/deskbridge-debug-display.XXXXXX)"
DEBUG_MOVE_OUT="$(mktemp /tmp/deskbridge-debug-move.XXXXXX)"
DEBUG_ROUTE_STATUS_OUT="$(mktemp /tmp/deskbridge-debug-route-status.XXXXXX)"
DEBUG_ROUTE_OUT="$(mktemp /tmp/deskbridge-debug-route.XXXXXX)"
DEBUG_CAPTURE_OUT="$(mktemp /tmp/deskbridge-debug-capture.XXXXXX)"
DEBUG_LOGS_OUT="$(mktemp /tmp/deskbridge-debug-logs.XXXXXX)"
DEBUG_PEER_OUT="$(mktemp /tmp/deskbridge-debug-peer.XXXXXX)"
DEBUG_SERVER_LOGS_OUT="$(mktemp /tmp/deskbridge-debug-server-logs.XXXXXX)"
RUST_LOG=info "$ROOT/target/debug/deskbridge" server --listen 127.0.0.1:24883 --allow mac --debug-capture-log >"$DEBUG_SERVER_LOG" 2>&1 &
DEBUG_SERVER_PID=$!
for _ in {1..20}; do
  nc -z 127.0.0.1 24883 >/dev/null 2>&1 && break
  sleep 0.2
done
RUST_LOG=info "$ROOT/target/debug/deskbridge" client --server 127.0.0.1:24883 --name mac --dry-run >"$DEBUG_CLIENT_LOG" 2>&1 &
DEBUG_CLIENT_PID=$!
for _ in {1..30}; do
  grep -q "client accepted" "$DEBUG_SERVER_LOG" && break
  if ! kill -0 "$DEBUG_CLIENT_PID" >/dev/null 2>&1; then
    cat "$DEBUG_CLIENT_LOG"
    echo "debug client exited before connecting"
    exit 1
  fi
  sleep 0.2
done
if ! grep -q "client accepted" "$DEBUG_SERVER_LOG"; then
  cat "$DEBUG_SERVER_LOG"
  cat "$DEBUG_CLIENT_LOG"
  echo "debug client did not connect"
  exit 1
fi
"$ROOT/target/debug/deskbridge" debug --server 127.0.0.1:24883 --name mac display-info >"$DEBUG_DISPLAY_OUT"
"$ROOT/target/debug/deskbridge" debug --server 127.0.0.1:24883 --name mac move-mouse --dx 1 --dy 0 >"$DEBUG_MOVE_OUT"
"$ROOT/target/debug/deskbridge" debug --server 127.0.0.1:24883 --name mac route-status >"$DEBUG_ROUTE_STATUS_OUT"
"$ROOT/target/debug/deskbridge" debug --server 127.0.0.1:24883 --name mac route-probe --steps 2 --dx 40 --dy -1 >"$DEBUG_ROUTE_OUT"
"$ROOT/target/debug/deskbridge" debug --server 127.0.0.1:24883 --name mac capture-probe --steps 2 --dx 40 --dy -1 >"$DEBUG_CAPTURE_OUT"
"$ROOT/target/debug/deskbridge" debug --server 127.0.0.1:24883 --name mac peer-info >"$DEBUG_PEER_OUT"
"$ROOT/target/debug/deskbridge" debug --server 127.0.0.1:24883 --name mac server-logs >"$DEBUG_SERVER_LOGS_OUT"
"$ROOT/target/debug/deskbridge" debug --server 127.0.0.1:24883 --name mac logs >"$DEBUG_LOGS_OUT"
if ! grep -q "display:" "$DEBUG_DISPLAY_OUT"; then
  cat "$DEBUG_DISPLAY_OUT"
  echo "debug display-info did not return display data"
  exit 1
fi
if ! grep -q "ok: true" "$DEBUG_MOVE_OUT"; then
  cat "$DEBUG_MOVE_OUT"
  echo "debug move-mouse did not succeed"
  exit 1
fi
if ! grep -q "route status read" "$DEBUG_ROUTE_STATUS_OUT"; then
  cat "$DEBUG_ROUTE_STATUS_OUT"
  echo "debug route-status did not return server route state"
  exit 1
fi
if ! grep -q "link: windows Right -> mac" "$DEBUG_ROUTE_STATUS_OUT"; then
  cat "$DEBUG_ROUTE_STATUS_OUT"
  echo "debug route-status did not report the configured Windows-to-Mac link"
  exit 1
fi
if ! grep -q "route probe delivered and acknowledged 3 events" "$DEBUG_ROUTE_OUT"; then
  cat "$DEBUG_ROUTE_OUT"
  echo "debug route-probe did not deliver the synthetic edge crossing"
  exit 1
fi
if ! grep -q "event 0: target=mac MouseAbs" "$DEBUG_ROUTE_OUT"; then
  cat "$DEBUG_ROUTE_OUT"
  echo "debug route-probe did not start with a remote absolute mouse event"
  exit 1
fi
if ! grep -q "ack seq=" "$DEBUG_ROUTE_OUT"; then
  cat "$DEBUG_ROUTE_OUT"
  echo "debug route-probe did not observe client acknowledgements"
  exit 1
fi
if ! grep -q "capture probe delivered and acknowledged 3 events through capture path" "$DEBUG_CAPTURE_OUT"; then
  cat "$DEBUG_CAPTURE_OUT"
  echo "debug capture-probe did not deliver through the capture routing path"
  exit 1
fi
if ! grep -q "capture event 1 routed target=mac MouseAbs" "$DEBUG_CAPTURE_OUT"; then
  cat "$DEBUG_CAPTURE_OUT"
  echo "debug capture-probe did not start by routing a captured edge event"
  exit 1
fi
if ! grep -q "ack seq=" "$DEBUG_CAPTURE_OUT"; then
  cat "$DEBUG_CAPTURE_OUT"
  echo "debug capture-probe did not observe client acknowledgements"
  exit 1
fi
if ! grep -q "debug request" "$DEBUG_LOGS_OUT"; then
  cat "$DEBUG_LOGS_OUT"
  echo "debug logs did not include target-side debug entries"
  exit 1
fi
if ! grep -q "role=client" "$DEBUG_PEER_OUT"; then
  cat "$DEBUG_PEER_OUT"
  echo "debug peer-info did not include client metadata"
  exit 1
fi
if ! grep -q "role=server" "$DEBUG_SERVER_LOGS_OUT"; then
  cat "$DEBUG_SERVER_LOGS_OUT"
  echo "debug server-logs did not include server metadata"
  exit 1
fi
if ! grep -q "client accepted" "$DEBUG_SERVER_LOGS_OUT"; then
  cat "$DEBUG_SERVER_LOGS_OUT"
  echo "debug server-logs did not include connection history"
  exit 1
fi

echo "== Reconnect after server start test =="
RECONNECT_SERVER_LOG="$(mktemp /tmp/deskbridge-reconnect-server.XXXXXX)"
RECONNECT_CLIENT_LOG="$(mktemp /tmp/deskbridge-reconnect-client.XXXXXX)"
RUST_LOG=info "$ROOT/target/debug/deskbridge" client --server 127.0.0.1:24882 --name mac --dry-run --max-events 1 >"$RECONNECT_CLIENT_LOG" 2>&1 &
RECONNECT_CLIENT_PID=$!
sleep 1.2
RUST_LOG=info "$ROOT/target/debug/deskbridge" server --listen 127.0.0.1:24882 --allow mac --demo-events >"$RECONNECT_SERVER_LOG" 2>&1 &
RECONNECT_SERVER_PID=$!
for _ in {1..30}; do
  if ! kill -0 "$RECONNECT_CLIENT_PID" >/dev/null 2>&1; then
    break
  fi
  sleep 0.2
done
if kill -0 "$RECONNECT_CLIENT_PID" >/dev/null 2>&1; then
  cat "$RECONNECT_CLIENT_LOG"
  echo "client did not reconnect and receive an event"
  exit 1
fi
wait "$RECONNECT_CLIENT_PID"
if ! grep -q "client session failed" "$RECONNECT_CLIENT_LOG"; then
  cat "$RECONNECT_CLIENT_LOG"
  echo "reconnect test did not observe an initial connection failure"
  exit 1
fi
if ! grep -q "connected" "$RECONNECT_CLIENT_LOG"; then
  cat "$RECONNECT_CLIENT_LOG"
  echo "reconnect test did not connect after server start"
  exit 1
fi
if ! grep -q "dry-run input event" "$RECONNECT_CLIENT_LOG"; then
  cat "$RECONNECT_CLIENT_LOG"
  echo "reconnect test did not receive routed input"
  exit 1
fi

echo "local verification passed"
