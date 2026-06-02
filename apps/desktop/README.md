# DeskBridge desktop (Tauri 2 — preview)

A single cross-platform control panel built with [Tauri 2](https://tauri.app):
a Rust backend plus a static web frontend (no Node build step — the assets in
`dist/` are served as-is via `withGlobalTauri`).

This is an early proof of concept that runs alongside the existing native
macOS/Windows apps. It currently drives the verified `deskbridge` CLI
(`version`, `discover`); pairing, layout, status, and a tray follow. A later
step can link `deskbridge-core` in-process and drop the subprocess hop.

## Layout

- `src-tauri/` — the Rust app (standalone Cargo workspace, so the repo's
  `core`/`daemon` workspace ignores it).
- `dist/` — the static frontend.

## Develop

Requires the Tauri prerequisites for your platform (a system WebView; on Linux,
`libwebkit2gtk-4.1-dev` and friends — see the CI `Tauri desktop` job).

```bash
cd apps/desktop/src-tauri
cargo build            # compile the app
cargo tauri dev        # run it (needs the tauri-cli: cargo install tauri-cli --version '^2')
```

The app looks for the `deskbridge` binary next to itself, then on `PATH`; set
`DESKBRIDGE_BIN` to point at a specific build.
