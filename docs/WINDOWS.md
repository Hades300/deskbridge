# Windows

## Server Responsibilities

The Windows side owns:

- global input capture
- physical edge detection
- layout configuration
- firewall/service setup
- server lifecycle and logs

## Admin Panel

`apps/windows/DeskBridge.Admin` is a WPF scaffold for a native management panel. It currently covers:

- listen address
- server name
- allowed client screen name
- client position
- config file path and `deskbridge.json` writer
- visual two-screen layout summary
- start/stop server process
- diagnostic text
- firewall rule helper text

The next implementation step is to replace the process runner with a proper Windows Service controller and validate the low-level hook on a real Windows host. The current admin panel already writes the Rust JSON config and starts `deskbridge.exe server --config ... --capture`.

## Development Commands

For release zips, start with `DeskBridge.Admin.exe`. Do not double-click `deskbridge.exe` unless you intentionally want the command-line daemon; without arguments it prints help and exits.

Server from config:

```powershell
deskbridge.exe server --config .\deskbridge.json
deskbridge.exe server --config .\deskbridge.json --capture
```

`--capture` installs low-level mouse and keyboard hooks with `SetWindowsHookExW`, routes pointer-edge transitions through the Rust layout router, and forwards keyboard/mouse events to connected clients while the remote screen is active.

Protocol diagnostic:

```powershell
deskbridge.exe diag --server 127.0.0.1:24800 --name mac
```

DeskBridge is not wire-compatible with Input Leap, Barrier, or Synergy. If the
server log shows `IHEL`, an old client from one of those tools is still trying to
connect to the DeskBridge port. Stop that client, or use the DeskBridge macOS app
and daemon on the Mac side.

## Firewall

Development firewall rule:

```powershell
New-NetFirewallRule -DisplayName "DeskBridge TCP 24800" -Direction Inbound -Protocol TCP -LocalPort 24800 -Action Allow
```

Verify:

```powershell
Get-NetTCPConnection -LocalPort 24800 -State Listen | Format-Table LocalAddress,LocalPort,OwningProcess
```

`LocalAddress` should be `0.0.0.0` or the LAN address, not only a tunnel or loopback address.
