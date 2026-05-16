# CI

GitHub Actions can validate the portable parts of DeskBridge:

- Rust format, tests, and clippy on Linux, macOS, and Windows.
- Windows ARM64 target compilation for the Rust daemon.
- WPF admin panel restore/build on Windows.
- macOS SwiftPM app packaging.
- Protocol loopback between server, diagnostics, and dry-run client.

It cannot prove the full physical product behavior by itself. The following still need real Windows host validation:

- Low-level mouse and keyboard hook behavior in an interactive desktop session.
- Physical monitor edge detection using the user's real display layout.
- Firewall prompts, sleep/wake, reboot, and service startup behavior.
- End-to-end control of the actual Mac from the actual Windows keyboard and mouse.

The workflow lives at `.github/workflows/ci.yml`.

If GitHub-hosted Windows ARM64 runners are available to your account, a future job can switch from cross-checking `aarch64-pc-windows-msvc` on `windows-latest` to running natively on `windows-11-arm`.
