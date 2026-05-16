#!/bin/zsh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

source "$HOME/.cargo/env"
cargo build --workspace

cd "$ROOT/apps/macos"
swift build

echo "Rust daemon: $ROOT/target/debug/deskbridge"
echo "macOS app:   $ROOT/apps/macos/.build/debug/DeskBridgeMac"

