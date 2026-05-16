# DeskBridge

DeskBridge is an open-source keyboard and mouse bridge focused on reliability and native desktop feel.

The first target is the setup that motivated the project:

- Windows owns the physical keyboard and mouse.
- macOS is controlled as a client.
- The client should recover when the Windows server sleeps, reboots, changes network readiness, or restarts its service.

This repository is a clean-room implementation. It does not copy Input Leap code.

## Current State

Implemented:

- Rust protocol core with framed JSON messages.
- Heartbeat, ping/pong, allow-list handshake, status messages.
- JSON config model with screen layout, physical edge links, and reliability settings.
- Rust `deskbridge` daemon with `server`, `client`, `diag`, and `simulate-route` commands.
- Windows host capture path behind `deskbridge server --capture`, using Raw Input for relative mouse motion and low-level hooks for edge detection, buttons, wheel, and keyboard.
- macOS native Swift status bar/config app built with SwiftPM.
- macOS app supervision for reconnect/restart behavior when the daemon exits.
- Windows WPF admin panel scaffold for server configuration and `--capture` launch.
- macOS input injection backend through `enigo` plus bounded CoreGraphics pointer warping.

Not complete beyond the MVP:

- Clipboard sync.
- Signed installers.
- Full Windows runtime validation from this Mac-only environment.

## Build

```bash
source "$HOME/.cargo/env"
cargo build --workspace
cargo test --workspace
```

Run the full local verification pass:

```bash
./scripts/verify-local.sh
```

If `x86_64-pc-windows-msvc` is installed, the verification script also runs Windows target `cargo check` and clippy for the server hook path.

## Releases

GitHub Actions creates release packages when a `v*` tag is pushed, or when the `Release` workflow is run manually:

- `DeskBridge-macos.dmg`
- `DeskBridge-macos.zip`
- `DeskBridge-windows-x64.zip`
- `DeskBridge-windows-arm64.zip`
- `DeskBridge-linux-x64.tar.gz`

The macOS app is ad-hoc signed for preview builds. The Windows packages include `deskbridge.exe` and the WPF admin app.

On Windows, open `DeskBridge.Admin.exe`. `deskbridge.exe` is the command-line daemon used by the admin app.
DeskBridge is not wire-compatible with Input Leap, Barrier, or Synergy clients; use the DeskBridge app/daemon on both sides.

macOS shell:

```bash
cd apps/macos
swift build
DESKBRIDGE_BIN=../../target/debug/deskbridge .build/debug/DeskBridgeMac
```

## CLI Usage

Create a default config:

```bash
deskbridge init-config --path deskbridge.json
```

Run a server:

```bash
deskbridge server --listen 0.0.0.0:24800 --name windows --allow mac
deskbridge server --config examples/deskbridge.json
deskbridge server --config examples/deskbridge.json --capture
```

`--capture` is available on Windows and macOS. On macOS, the server process needs Accessibility and Input Monitoring permissions because it installs a CoreGraphics event tap.

Run a macOS client:

```bash
deskbridge client --server 192.168.2.5:24800 --name mac
deskbridge client --config examples/deskbridge.json
```

Run diagnostics:

```bash
deskbridge diag --server 192.168.2.5:24800 --name mac
deskbridge diag --config examples/deskbridge.json
deskbridge display-info
```

Simulate a configured edge crossing without moving the real mouse:

```bash
deskbridge simulate-route --config examples/deskbridge.json --steps 3 --dx 80 --dy -2
deskbridge simulate-route --config examples/deskbridge.json --steps 2 --dx 80 --dy 0 --return-dx -200 --return-dy 0
```

Expected output starts with a `MouseAbs` event that lands on the linked Mac edge, followed by repeated `MouseMove` events still targeted at `mac`.
When `--return-dx` or `--return-dy` is provided, the simulation continues with reverse movement and prints a `release` line when input returns to the Windows screen.
When a real client connects, DeskBridge includes the client display size in the handshake so the server can map the crossing height to the actual Mac screen instead of a default size.

Test local macOS injection without Windows:

```bash
deskbridge inject-test --x 1 --y 559 --dx 80 --dy -2
deskbridge inject-test --x 1 --y 559 --dx -500 --dy 0
```

This moves the local pointer through the same input path used by the DeskBridge client and prints the pointer location before and after injection. On macOS, the injected pointer is clamped to the main display bounds so remote relative movement cannot push the cursor into invisible negative coordinates.

Test the capture-to-protocol path on macOS without Windows by running a macOS server with `--capture` and a dry-run client:

```bash
deskbridge server --listen 127.0.0.1:24903 --allow mac --capture
deskbridge client --server 127.0.0.1:24903 --name mac --dry-run
```

Move the server pointer across the configured edge. The client log should show `MouseAbs` followed by relative `MouseMove` events.
For a same-Mac automated capture smoke test, move to the edge and send a diagnostic evented relative move:

```bash
deskbridge inject-test --x 1727 --y 559 --dx 80 --dy 0 --evented-rel
```

`--evented-rel` is only for validating the macOS capture hook; normal client injection uses the bounded path above.

Use `--dry-run` on the client to validate the protocol without injecting input.

## Product Principles

- Native lifecycle first: LaunchAgent/Login Item on macOS, Service/Startup Task on Windows.
- The app must explain connection state in human terms, not just logs.
- Reconnect is a product feature, not a retry loop hidden in a daemon.
- UI stays small and native: status, layout, diagnostics, permissions, service state.
- Rust owns protocol, state machine, diagnostics, and input backends.

## Configuration

The default layout places the Mac to the right of the Windows screen:

```json
{
  "layout": {
    "links": [
      { "from": "windows", "edge": "right", "to": "mac" }
    ]
  }
}
```

Use `examples/deskbridge.json` as the editable starting point.

## License

MIT.
