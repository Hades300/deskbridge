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

echo "== Loopback protocol test =="
SERVER_LOG="$(mktemp /tmp/deskbridge-server.XXXXXX)"
CLIENT_LOG="$(mktemp /tmp/deskbridge-client.XXXXXX)"
"$ROOT/target/debug/deskbridge" server --listen 127.0.0.1:24881 --allow mac --demo-events >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!
cleanup() {
  kill "$SERVER_PID" >/dev/null 2>&1 || true
}
trap cleanup EXIT
sleep 1
"$ROOT/target/debug/deskbridge" diag --server 127.0.0.1:24881 --name mac
"$ROOT/target/debug/deskbridge" client --server 127.0.0.1:24881 --name mac --dry-run --max-events 1 --once >"$CLIENT_LOG" 2>&1

echo "local verification passed"
