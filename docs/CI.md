# CI

GitHub Actions can validate the portable parts of DeskBridge:

- Rust format, tests, and clippy on Linux, macOS, and Windows.
- Windows ARM64 target compilation for the Rust daemon.
- WPF admin panel restore/build on Windows.
- macOS SwiftPM app packaging.
- Protocol loopback between server, diagnostics, and dry-run client.
- Local reconnect smoke test where the client starts first, observes connection failures, then connects after the server appears and receives a routed input event.
- Debug route-probe smoke test where a diagnostic request asks the live server to synthesize an edge crossing, deliver `MouseAbs` and continued `MouseMove` packets to the connected client, and wait for acknowledgements.
- Local debug-control smoke test for remote display info, mouse test command, and target-side debug logs.

It cannot prove the full physical product behavior by itself. The following still need real Windows host validation:

- Low-level mouse and keyboard hook behavior in an interactive desktop session.
- Physical monitor edge detection using the user's real display layout.
- Firewall prompts, sleep/wake, reboot, and service startup behavior.
- End-to-end control of the actual Mac from the actual Windows keyboard and mouse.

The workflow lives at `.github/workflows/ci.yml`.

Release packaging lives at `.github/workflows/release.yml`. It runs on `v*` tags and manual dispatch, then publishes:

- macOS `.dmg` and `.zip`
- Windows x64 and ARM64 `.zip`
- Linux x64 `.tar.gz`

If GitHub-hosted Windows ARM64 runners are available to your account, a future job can switch from cross-checking `aarch64-pc-windows-msvc` on `windows-latest` to running natively on `windows-11-arm`.
