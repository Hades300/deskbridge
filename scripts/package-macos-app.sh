#!/bin/zsh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/build/DeskBridge.app"
CONTENTS="$APP/Contents"
MACOS="$CONTENTS/MacOS"
RESOURCES="$CONTENTS/Resources"
HELPERS="$CONTENTS/Helpers"
HELPER_APP="$HELPERS/DeskBridgeHelper.app"
HELPER_CONTENTS="$HELPER_APP/Contents"
HELPER_MACOS="$HELPER_CONTENTS/MacOS"
SIGN_IDENTITY="${DESKBRIDGE_CODESIGN_IDENTITY:-}"
SIGN_KEYCHAIN="${DESKBRIDGE_CODESIGN_KEYCHAIN:-}"

if [[ -z "$SIGN_IDENTITY" && "${DESKBRIDGE_USE_LOCAL_SIGNING:-0}" == "1" ]]; then
  eval "$("$ROOT/scripts/ensure-local-macos-signing-identity.sh")"
fi

codesign_app() {
  local identifier="$1"
  local target="$2"
  local args=(--force)

  if [[ -n "$SIGN_KEYCHAIN" ]]; then
    args+=(--keychain "$SIGN_KEYCHAIN")
  fi

  if [[ -n "$SIGN_IDENTITY" ]]; then
    args+=(--sign "$SIGN_IDENTITY")
  else
    args+=(--sign -)
  fi

  if [[ -n "$identifier" ]]; then
    args+=(--identifier "$identifier")
  fi

  codesign "${args[@]}" "$target"
}

"$ROOT/scripts/build-macos.sh"

rm -rf "$APP"
mkdir -p "$MACOS" "$RESOURCES" "$HELPER_MACOS"
cp "$ROOT/apps/macos/.build/debug/DeskBridgeMac" "$MACOS/DeskBridgeMac"
cp "$ROOT/target/debug/deskbridge" "$HELPER_MACOS/deskbridge"

/usr/bin/python3 - <<PY
from pathlib import Path
app_plist = """<?xml version="1.0" encoding="UTF-8"?>
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
helper_plist = """<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>deskbridge</string>
  <key>CFBundleIdentifier</key>
  <string>dev.deskbridge.helper</string>
  <key>CFBundleName</key>
  <string>DeskBridge Helper</string>
  <key>CFBundleDisplayName</key>
  <string>DeskBridge Helper</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>LSBackgroundOnly</key>
  <true/>
</dict>
</plist>
"""
Path("$CONTENTS/Info.plist").write_text(app_plist)
Path("$HELPER_CONTENTS/Info.plist").write_text(helper_plist)
PY

codesign_app "" "$HELPER_APP"
codesign_app "" "$APP"
echo "$APP"
