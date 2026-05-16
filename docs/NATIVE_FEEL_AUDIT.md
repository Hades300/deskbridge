# Native-Feel Audit

This is the project-specific audit derived from the native-feel skill principles. For this product, the practical architecture is native Swift/AppKit on macOS, native WPF/WinUI on Windows, and Rust for the protocol/core daemon rather than a shared WebView UI.

## macOS

- Status item exists and does not steal focus on launch.
- Settings window uses system controls, system font, and standard spacing.
- Permission failures explain the exact System Settings pane to open.
- The app distinguishes `Connecting`, `Connected`, `Reconnecting`, `Permission missing`, and `Server unreachable`.
- Diagnostics run without blocking the window.
- Auto reconnect is visible and user-controllable.
- Quitting the UI stops the controlled daemon unless the user explicitly enables background mode.
- No web-style full-screen modals for routine errors.
- Server address and screen name persist in `UserDefaults`.
- Diagnostics are selectable and copyable.

## Windows

- Server panel uses native WPF/WinUI controls.
- Firewall status is visible before the user has to inspect logs.
- Layout is visual, not only text fields.
- Client names are validated and case-sensitive mismatches are called out.
- Service state is obvious: stopped, starting, listening, connected.
- Logs can be copied without finding files in Explorer.

## Reliability

- Client reconnects after server sleep, reboot, network drop, and server process restart.
- Heartbeat timeout is surfaced to UI within five seconds.
- A stale process with no socket is treated as unhealthy.
- TCP-open but protocol-rejected is a separate diagnostic from TCP-closed.
- Server bind address is shown in diagnostics.
