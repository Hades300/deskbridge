#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! DeskBridge cross-platform desktop control panel (Tauri 2).
//!
//! The UI drives the already-verified `deskbridge` CLI: discovery, and a managed
//! `client` session for connect/disconnect/status. A later step can link
//! `deskbridge-core` in-process and drop the subprocess hop.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State};

/// An in-progress interactive pairing (`deskbridge pair --join`).
struct PairingProc {
    child: Child,
    stdin: ChildStdin,
}

/// Managed runtime state: the live `deskbridge client` child, if any.
#[derive(Default)]
struct AppState {
    client: Mutex<Option<Child>>,
    pairing: Mutex<Option<PairingProc>>,
}

/// Resolve the `deskbridge` binary: an explicit override, then next to this
/// executable (the bundled layout), then `PATH`.
fn deskbridge_binary() -> String {
    if let Ok(path) = std::env::var("DESKBRIDGE_BIN") {
        if !path.is_empty() {
            return path;
        }
    }

    let binary_name = if cfg!(windows) {
        "deskbridge.exe"
    } else {
        "deskbridge"
    };

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(binary_name);
            if candidate.exists() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }

    binary_name.to_string()
}

fn run_deskbridge(args: &[&str]) -> Result<String, String> {
    let output = Command::new(deskbridge_binary())
        .args(args)
        .output()
        .map_err(|err| format!("failed to run deskbridge: {err}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        return Ok(stdout);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Err(if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    })
}

#[derive(serde::Serialize)]
struct DiscoveredServer {
    name: String,
    address: String,
    version: Option<String>,
}

#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
fn daemon_version() -> Result<String, String> {
    run_deskbridge(&["version"])
}

/// The server address to prefill the connection field with: the
/// `DESKBRIDGE_SERVER` environment variable if set, else a sensible default.
#[tauri::command]
fn default_server() -> String {
    std::env::var("DESKBRIDGE_SERVER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "192.168.2.5:24800".to_string())
}

#[tauri::command]
fn discover_servers() -> Result<Vec<DiscoveredServer>, String> {
    let output = run_deskbridge(&["discover", "--timeout-ms", "2500"])?;
    let mut servers = Vec::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 && parts[1].contains(':') {
            servers.push(DiscoveredServer {
                name: parts[0].trim().to_string(),
                address: parts[1].trim().to_string(),
                version: parts.get(2).map(|value| value.trim().to_string()),
            });
        }
    }
    Ok(servers)
}

/// Start (or restart) a managed `deskbridge client` session.
#[tauri::command]
fn connect(
    state: State<'_, AppState>,
    server: String,
    name: String,
    secret: Option<String>,
) -> Result<(), String> {
    let server = if server.contains(':') {
        server
    } else {
        format!("{server}:24800")
    };
    let name = if name.trim().is_empty() {
        "mac".to_string()
    } else {
        name
    };
    eprintln!("connect requested: server={server} name={name} secret={}", secret.is_some());

    let mut guard = state.client.lock().map_err(|_| "state poisoned".to_string())?;
    if let Some(mut child) = guard.take() {
        let _ = child.kill();
    }

    let mut args = vec![
        "client".to_string(),
        "--server".to_string(),
        server,
        "--name".to_string(),
        name,
        "--reconnect".to_string(),
    ];
    if let Some(secret) = secret {
        if !secret.is_empty() {
            args.push("--psk".to_string());
            args.push(secret);
        }
    }

    let child = Command::new(deskbridge_binary())
        .args(&args)
        .spawn()
        .map_err(|err| format!("failed to start client: {err}"))?;
    *guard = Some(child);
    Ok(())
}

/// Stop the managed client session, if any.
#[tauri::command]
fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
    let mut guard = state.client.lock().map_err(|_| "state poisoned".to_string())?;
    if let Some(mut child) = guard.take() {
        let _ = child.kill();
    }
    Ok(())
}

/// Report whether the managed client session is still running.
#[tauri::command]
fn connection_status(state: State<'_, AppState>) -> Result<bool, String> {
    let mut guard = state.client.lock().map_err(|_| "state poisoned".to_string())?;
    let running = match guard.as_mut() {
        Some(child) => match child.try_wait() {
            Ok(Some(_)) => false,
            Ok(None) => true,
            Err(_) => false,
        },
        None => false,
    };
    if !running {
        *guard = None;
    }
    Ok(running)
}

/// Path to the desktop app's config file (where pairing writes the secret).
fn config_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|err| format!("no config dir: {err}"))?;
    std::fs::create_dir_all(&dir).map_err(|err| format!("failed to create config dir: {err}"))?;
    Ok(dir.join("deskbridge.json"))
}

/// Read `security.psk` from a config file, if present.
fn read_secret(path: &std::path::Path) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&data).ok()?;
    value
        .get("security")?
        .get("psk")?
        .as_str()
        .filter(|psk| !psk.is_empty())
        .map(ToString::to_string)
}

/// Begin interactive pairing as the joining device. Streams the pairing code
/// and result to the frontend via events: `pairing:code`, `pairing:done`,
/// `pairing:closed`.
#[tauri::command]
fn start_pairing(app: AppHandle, state: State<'_, AppState>, server: String) -> Result<(), String> {
    // Cancel any pairing already in progress.
    {
        let mut guard = state.pairing.lock().map_err(|_| "state poisoned".to_string())?;
        if let Some(mut pairing) = guard.take() {
            let _ = pairing.stdin.write_all(b"n\n");
            let _ = pairing.child.kill();
        }
    }

    let server = if server.contains(':') {
        server
    } else {
        format!("{server}:24800")
    };
    let config = config_path(&app)?;
    let config_arg = config.to_string_lossy().into_owned();

    let mut child = Command::new(deskbridge_binary())
        .args(["pair", "--join", &server, "--config", &config_arg])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("failed to start pairing: {err}"))?;

    let stdout = child.stdout.take().ok_or("no pairing stdout")?;
    let stdin = child.stdin.take().ok_or("no pairing stdin")?;

    let emitter = app.clone();
    let server_for_thread = server.clone();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if let Some(rest) = line.split("Pairing code:").nth(1) {
                let code = rest.trim().to_string();
                if !code.is_empty() {
                    let _ = emitter.emit("pairing:code", code);
                }
            }
            if line.contains("Paired.") {
                let secret = read_secret(&config).unwrap_or_default();
                let _ = emitter.emit(
                    "pairing:done",
                    serde_json::json!({ "server": server_for_thread, "secret": secret }),
                );
            }
        }
        let _ = emitter.emit("pairing:closed", ());
    });

    *state.pairing.lock().map_err(|_| "state poisoned".to_string())? =
        Some(PairingProc { child, stdin });
    Ok(())
}

/// Confirm the codes match: let the host send the secret.
#[tauri::command]
fn confirm_pairing(state: State<'_, AppState>) -> Result<(), String> {
    let mut guard = state.pairing.lock().map_err(|_| "state poisoned".to_string())?;
    if let Some(pairing) = guard.as_mut() {
        pairing
            .stdin
            .write_all(b"y\n")
            .map_err(|err| format!("failed to confirm: {err}"))?;
        let _ = pairing.stdin.flush();
    }
    Ok(())
}

/// Abort an in-progress pairing.
#[tauri::command]
fn cancel_pairing(state: State<'_, AppState>) -> Result<(), String> {
    let mut guard = state.pairing.lock().map_err(|_| "state poisoned".to_string())?;
    if let Some(mut pairing) = guard.take() {
        let _ = pairing.stdin.write_all(b"n\n");
        let _ = pairing.child.kill();
    }
    Ok(())
}

fn build_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let show = MenuItem::with_id(app, "show", "Show DeskBridge", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    TrayIconBuilder::new()
        .icon(app.default_window_icon().cloned().ok_or("missing app icon")?)
        .tooltip("DeskBridge")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .manage(AppState::default())
        .setup(|app| {
            // The tray is optional: on a headless or DBus-less session (CI, some
            // Linux setups) it may be unavailable, which must not stop the app.
            if let Err(err) = build_tray(app) {
                eprintln!("tray unavailable: {err}");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_version,
            daemon_version,
            default_server,
            discover_servers,
            connect,
            disconnect,
            connection_status,
            start_pairing,
            confirm_pairing,
            cancel_pairing
        ])
        .run(tauri::generate_context!())
        .expect("error while running DeskBridge desktop");
}
