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

Input injection requires macOS Accessibility permission. The visible app launches
the bundled helper at `DeskBridge.app/Contents/MacOS/deskbridge`; that helper is
the process that posts keyboard and mouse events. The app checks this permission
before starting the client, opens the Accessibility settings page if it is
missing, and stops reconnecting so macOS does not show repeated permission
prompts.

Manual check:

```bash
/Applications/DeskBridge.app/Contents/MacOS/deskbridge permissions --prompt
```

If permission was granted to an older build path, remove the stale DeskBridge or
deskbridge entries from System Settings, then grant the current helper when this
command prompts.

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
