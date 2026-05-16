#!/bin/zsh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/build/DeskBridge.app"
CONTENTS="$APP/Contents"
MACOS="$CONTENTS/MacOS"
RESOURCES="$CONTENTS/Resources"

"$ROOT/scripts/build-macos.sh"

rm -rf "$APP"
mkdir -p "$MACOS" "$RESOURCES"
cp "$ROOT/apps/macos/.build/debug/DeskBridgeMac" "$MACOS/DeskBridgeMac"
cp "$ROOT/target/debug/deskbridge" "$MACOS/deskbridge"

/usr/bin/python3 - <<PY
from pathlib import Path
plist = """<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>DeskBridgeMac</string>
  <key>CFBundleIdentifier</key>
  <string>dev.deskbridge.mac</string>
  <key>CFBundleName</key>
  <string>DeskBridge</string>
  <key>CFBundleDisplayName</key>
  <string>DeskBridge</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>LSMinimumSystemVersion</key>
  <string>14.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
"""
Path("$CONTENTS/Info.plist").write_text(plist)
PY

codesign --force --sign - --identifier dev.deskbridge.helper "$MACOS/deskbridge"
codesign --force --sign - "$APP"
echo "$APP"
