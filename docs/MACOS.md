# macOS

## Native Shell

`apps/macos` contains a SwiftPM AppKit/SwiftUI status app:

- status bar item
- small settings window
- server address and screen name fields
- connect/disconnect
- diagnostics output
- auto reconnect preference
- default config writer

Build:

```bash
cd apps/macos
swift build
DESKBRIDGE_BIN=../../target/debug/deskbridge .build/debug/DeskBridgeMac
```

Package:

```bash
./scripts/package-macos-app.sh
open build/DeskBridge.app
```

## Permissions

Input injection requires macOS Accessibility permission. For development, grant permission to the launched app or the `deskbridge` binary that performs injection.

The product should provide a first-run health panel that checks:

- Accessibility granted
- Input Monitoring granted, if capture is enabled
- local network reachability
- server handshake result

## Login Auto Start

Preferred production behavior:

- register the native app as a Login Item
- let the app supervise the daemon process
- restart the daemon when the server port is open but there is no established session

Avoid launching the bare daemon from `launchd` for the main user experience unless it has a signed app wrapper and clear permission identity.

## Local Verification

Use `scripts/verify-local.sh` from the repository root. It runs Rust format/test/lint, builds the SwiftPM app, packages `DeskBridge.app`, writes a config smoke-test file, and validates the protocol with a loopback server/client pair.
