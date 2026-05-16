#!/bin/zsh
set -euo pipefail

IDENTITY="${DESKBRIDGE_LOCAL_CODESIGN_IDENTITY:-DeskBridge Local Code Signing}"
KEYCHAIN="${DESKBRIDGE_LOCAL_CODESIGN_KEYCHAIN:-$HOME/Library/Keychains/DeskBridgeLocal.keychain-db}"
KEYCHAIN_PASSWORD="${DESKBRIDGE_LOCAL_CODESIGN_KEYCHAIN_PASSWORD:-deskbridge-local-signing}"

log() {
  print -r -- "$*" >&2
}

current_keychains() {
  security list-keychains -d user | sed 's/^[[:space:]]*"//;s/"$//'
}

ensure_keychain_in_search_list() {
  local -a keychains
  keychains=("${(@f)$(current_keychains)}")

  for keychain in "${keychains[@]}"; do
    if [[ "$keychain" == "$KEYCHAIN" ]]; then
      return
    fi
  done

  security list-keychains -d user -s "$KEYCHAIN" "${keychains[@]}"
}

if [[ ! -f "$KEYCHAIN" ]]; then
  log "Creating local DeskBridge signing keychain: $KEYCHAIN"
  security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
fi

security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
ensure_keychain_in_search_list

if ! security find-certificate -c "$IDENTITY" "$KEYCHAIN" >/dev/null 2>&1; then
  log "Creating local DeskBridge code signing identity: $IDENTITY"
  TMPDIR="$(mktemp -d /tmp/deskbridge-signing.XXXXXX)"

  openssl req \
    -newkey rsa:2048 \
    -nodes \
    -keyout "$TMPDIR/key.pem" \
    -x509 \
    -days 3650 \
    -out "$TMPDIR/cert.pem" \
    -subj "/CN=$IDENTITY" \
    -addext "extendedKeyUsage=codeSigning" \
    -addext "keyUsage=digitalSignature" >/dev/null 2>&1

  if ! openssl pkcs12 \
    -legacy \
    -export \
    -out "$TMPDIR/identity.p12" \
    -inkey "$TMPDIR/key.pem" \
    -in "$TMPDIR/cert.pem" \
    -passout "pass:$KEYCHAIN_PASSWORD" >/dev/null 2>&1; then
    openssl pkcs12 \
      -export \
      -out "$TMPDIR/identity.p12" \
      -inkey "$TMPDIR/key.pem" \
      -in "$TMPDIR/cert.pem" \
      -passout "pass:$KEYCHAIN_PASSWORD" >/dev/null 2>&1
  fi

  security import "$TMPDIR/identity.p12" \
    -k "$KEYCHAIN" \
    -P "$KEYCHAIN_PASSWORD" \
    -T /usr/bin/codesign >/dev/null

  rm -rf "$TMPDIR"
fi

security set-key-partition-list \
  -S apple-tool:,apple: \
  -s \
  -k "$KEYCHAIN_PASSWORD" \
  "$KEYCHAIN" >/dev/null 2>&1 || true

TESTDIR="$(mktemp -d /tmp/deskbridge-signing-test.XXXXXX)"
trap 'rm -rf "$TESTDIR"' EXIT
cp /bin/echo "$TESTDIR/echo"
codesign \
  --force \
  --keychain "$KEYCHAIN" \
  --sign "$IDENTITY" \
  --identifier dev.deskbridge.signing-test \
  "$TESTDIR/echo" >/dev/null

print -r -- "SIGN_IDENTITY='${IDENTITY//\'/\'\\\'\'}'"
print -r -- "SIGN_KEYCHAIN='${KEYCHAIN//\'/\'\\\'\'}'"
