//! Cross-screen drag-and-drop file transfer (receive side).
//!
//! When a user drags files off the controlling machine and across a screen
//! edge, the controller sends a [`FileDropPacket`]; the receiving device
//! materializes the files to a staging directory and, on platforms that support
//! it, synthesizes a native drop at the cursor location.
//!
//! The transfer/staging engine here is platform-independent and fully tested.
//! Capturing the originating drag gesture and injecting the final OS drop are
//! platform-specific and tracked as follow-ups; until then the files are staged
//! and their paths surfaced so they can be dropped/pasted.

use anyhow::{Context, Result};
use deskbridge_core::FileDropPacket;
use std::path::PathBuf;

#[derive(Debug)]
pub struct DropOutcome {
    pub staged: Vec<PathBuf>,
    pub at: (i32, i32),
}

impl DropOutcome {
    pub fn summary(&self) -> String {
        format!(
            "staged {} dropped file(s) at ({}, {})",
            self.staged.len(),
            self.at.0,
            self.at.1
        )
    }
}

/// Stage the dropped files and (on supported platforms) perform the drop.
pub async fn apply_file_drop(packet: FileDropPacket) -> Result<DropOutcome> {
    let at = (packet.x, packet.y);
    let files = packet.files;
    let staged = tokio::task::spawn_blocking(move || crate::clipboard::stage_remote_files(files))
        .await
        .context("file drop staging task failed")??;

    perform_native_drop(&staged, at);

    Ok(DropOutcome { staged, at })
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn perform_native_drop(_staged: &[PathBuf], _at: (i32, i32)) {
    // TODO(platform): synthesize a native drag-and-drop of `_staged` at `_at`.
    // The files are already staged on disk; until the OS drop is wired up they
    // remain available to the user to drop or paste manually.
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn perform_native_drop(_staged: &[PathBuf], _at: (i32, i32)) {}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use deskbridge_core::ClipboardFile;

    #[tokio::test]
    async fn file_drop_is_staged_to_disk() {
        let packet = FileDropPacket {
            seq: 1,
            sent_at_ms: 0,
            files: vec![ClipboardFile {
                name: "dropped.txt".to_string(),
                size: 5,
                data_base64: STANDARD.encode(b"hello"),
            }],
            x: 100,
            y: 200,
        };

        let outcome = apply_file_drop(packet).await.unwrap();
        assert_eq!(outcome.staged.len(), 1);
        assert_eq!(outcome.at, (100, 200));
        assert_eq!(
            std::fs::read_to_string(&outcome.staged[0]).unwrap(),
            "hello"
        );
        let _ = std::fs::remove_file(&outcome.staged[0]);
    }
}
