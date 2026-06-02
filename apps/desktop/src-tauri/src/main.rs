#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! DeskBridge cross-platform desktop control panel (Tauri 2 proof of concept).
//!
//! The UI drives the already-verified `deskbridge` CLI. A later step can link
//! `deskbridge-core` in-process and drop the subprocess hop entirely.

use std::process::Command;

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

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            app_version,
            daemon_version,
            discover_servers
        ])
        .run(tauri::generate_context!())
        .expect("error while running DeskBridge desktop");
}
