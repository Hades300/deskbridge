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
- macOS input injection backend through `enigo`.

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

Run a macOS client:

```bash
deskbridge client --server 192.168.2.5:24800 --name mac
deskbridge client --config examples/deskbridge.json
```

Run diagnostics:

```bash
deskbridge diag --server 192.168.2.5:24800 --name mac
deskbridge diag --config examples/deskbridge.json
```

Simulate a configured edge crossing without moving the real mouse:

```bash
deskbridge simulate-route --config examples/deskbridge.json --steps 3 --dx 80 --dy -2
```

Expected output starts with a `MouseAbs` event that lands on the linked Mac edge, followed by repeated `MouseMove` events still targeted at `mac`.

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
