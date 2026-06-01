use anyhow::{Context, Result, anyhow};
use arboard::{Clipboard, ImageData};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use deskbridge_core::{ClipboardConfig, ClipboardContent, ClipboardFile, ClipboardPacket};
use std::{
    borrow::Cow,
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::mpsc, time};
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub struct ClipboardRuntime {
    options: ClipboardConfig,
    state: Arc<Mutex<ClipboardState>>,
    operation_lock: Arc<Mutex<()>>,
}

#[derive(Debug, Default)]
struct ClipboardState {
    last_observed: Option<u64>,
    last_applied_remote: Option<u64>,
}

impl ClipboardRuntime {
    pub fn new(options: ClipboardConfig) -> Option<Self> {
        options.enabled.then_some(Self {
            options,
            state: Arc::new(Mutex::new(ClipboardState::default())),
            operation_lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn spawn_watcher(&self) -> mpsc::UnboundedReceiver<ClipboardPacket> {
        let (tx, rx) = mpsc::unbounded_channel();
        let runtime = self.clone();

        tokio::spawn(async move {
            let mut seq = 0_u64;
            let mut ticker =
                time::interval(Duration::from_millis(runtime.options.poll_ms.max(250)));

            // Record whatever is already on the local clipboard as the baseline
            // so we do not immediately broadcast pre-existing content (which the
            // user never copied during this session) to the peer.
            let baseline_runtime = runtime.clone();
            if let Ok(Ok(Some(content))) =
                tokio::task::spawn_blocking(move || baseline_runtime.read_local_snapshot()).await
            {
                runtime.prime_baseline(clipboard_fingerprint(&content));
            }

            loop {
                ticker.tick().await;
                let runtime_for_read = runtime.clone();
                let snapshot = match tokio::task::spawn_blocking(move || {
                    runtime_for_read.read_local_snapshot()
                })
                .await
                {
                    Ok(Ok(snapshot)) => snapshot,
                    Ok(Err(err)) => {
                        debug!(error = %err, "clipboard poll skipped");
                        continue;
                    }
                    Err(err) => {
                        warn!(error = %err, "clipboard poll task failed");
                        continue;
                    }
                };

                let Some(content) = snapshot else {
                    continue;
                };

                let fingerprint = clipboard_fingerprint(&content);
                if !runtime.should_publish_observed(fingerprint) {
                    continue;
                }

                seq = seq.saturating_add(1);
                if tx
                    .send(ClipboardPacket {
                        seq,
                        sent_at_ms: deskbridge_core::now_ms(),
                        content,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        rx
    }

    pub async fn apply_remote(&self, packet: ClipboardPacket) -> Result<String> {
        let runtime = self.clone();
        tokio::task::spawn_blocking(move || runtime.apply_remote_blocking(packet))
            .await
            .context("clipboard apply task failed")?
    }

    fn read_local_snapshot(&self) -> Result<Option<ClipboardContent>> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| anyhow!("clipboard operation lock poisoned"))?;

        let mut clipboard = Clipboard::new().context("failed to open clipboard")?;

        if self.options.files {
            match clipboard.get().file_list() {
                Ok(paths) if !paths.is_empty() => {
                    return files_to_content(paths, self.options.max_transfer_bytes)
                        .map(Some)
                        .context("failed to read clipboard file list");
                }
                Ok(_) => {}
                Err(_) => {}
            }
        }

        if self.options.image
            && let Ok(image) = clipboard.get_image()
        {
            return image_to_content(image, self.options.max_transfer_bytes).map(Some);
        }

        if self.options.text {
            match clipboard.get_text() {
                Ok(text) if !text.is_empty() => {
                    return Ok(Some(ClipboardContent::Text { text }));
                }
                Ok(_) => {}
                Err(_) => {}
            }
        }

        Ok(None)
    }

    fn apply_remote_blocking(&self, packet: ClipboardPacket) -> Result<String> {
        let fingerprint = clipboard_fingerprint(&packet.content);
        if !self.content_allowed(&packet.content) {
            return Ok(format!(
                "ignored disabled {}",
                content_summary(&packet.content)
            ));
        }

        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| anyhow!("clipboard operation lock poisoned"))?;

        let summary = content_summary(&packet.content);
        let mut clipboard = Clipboard::new().context("failed to open clipboard")?;
        match packet.content {
            ClipboardContent::Text { text } => {
                clipboard
                    .set_text(text)
                    .context("failed to write remote text clipboard")?;
            }
            ClipboardContent::Image {
                width,
                height,
                rgba_base64,
            } => {
                let bytes = STANDARD
                    .decode(rgba_base64)
                    .context("invalid remote image clipboard data")?;
                validate_image_bytes(width, height, bytes.len())?;
                clipboard
                    .set_image(ImageData {
                        width: width as usize,
                        height: height as usize,
                        bytes: Cow::Owned(bytes),
                    })
                    .context("failed to write remote image clipboard")?;
            }
            ClipboardContent::Files { files } => {
                let staged = stage_remote_files(files)?;
                clipboard
                    .set()
                    .file_list(&staged)
                    .context("failed to write remote file clipboard")?;
            }
        }

        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("clipboard state lock poisoned"))?;
        state.last_applied_remote = Some(fingerprint);
        state.last_observed = Some(fingerprint);
        Ok(summary)
    }

    fn content_allowed(&self, content: &ClipboardContent) -> bool {
        match content {
            ClipboardContent::Text { .. } => self.options.text,
            ClipboardContent::Image { .. } => self.options.image,
            ClipboardContent::Files { .. } => self.options.files,
        }
    }

    fn prime_baseline(&self, fingerprint: u64) {
        if let Ok(mut state) = self.state.lock() {
            state.last_observed = Some(fingerprint);
        }
    }

    fn should_publish_observed(&self, fingerprint: u64) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };

        if state.last_observed == Some(fingerprint) {
            return false;
        }

        state.last_observed = Some(fingerprint);
        if state.last_applied_remote == Some(fingerprint) {
            return false;
        }

        true
    }
}

fn image_to_content(image: ImageData<'static>, max_bytes: u64) -> Result<ClipboardContent> {
    let byte_len = image.bytes.len() as u64;
    if byte_len > max_bytes {
        return Err(anyhow!(
            "clipboard image is {} bytes, over configured limit {}",
            byte_len,
            max_bytes
        ));
    }
    let width = u32::try_from(image.width).context("clipboard image width is too large")?;
    let height = u32::try_from(image.height).context("clipboard image height is too large")?;
    validate_image_bytes(width, height, image.bytes.len())?;

    Ok(ClipboardContent::Image {
        width,
        height,
        rgba_base64: STANDARD.encode(image.bytes.as_ref()),
    })
}

fn files_to_content(paths: Vec<PathBuf>, max_bytes: u64) -> Result<ClipboardContent> {
    let mut files = Vec::new();
    let mut total = 0_u64;

    for path in paths {
        let metadata = fs::metadata(&path)
            .with_context(|| format!("failed to inspect clipboard file {}", path.display()))?;
        if !metadata.is_file() {
            return Err(anyhow!(
                "clipboard file sync currently supports regular files only: {}",
                path.display()
            ));
        }

        total = total.saturating_add(metadata.len());
        if total > max_bytes {
            return Err(anyhow!(
                "clipboard files are {} bytes total, over configured limit {}",
                total,
                max_bytes
            ));
        }

        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(sanitize_file_name)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "clipboard-file".to_string());
        let bytes = fs::read(&path)
            .with_context(|| format!("failed to read clipboard file {}", path.display()))?;
        files.push(ClipboardFile {
            name,
            size: metadata.len(),
            data_base64: STANDARD.encode(bytes),
        });
    }

    if files.is_empty() {
        return Err(anyhow!("clipboard file list was empty"));
    }

    Ok(ClipboardContent::Files { files })
}

/// Staged clipboard file batches older than this are pruned on the next remote
/// file paste, so the staging directory does not grow without bound.
const STAGING_TTL_MS: u128 = 60 * 60 * 1000;

fn stage_remote_files(files: Vec<ClipboardFile>) -> Result<Vec<PathBuf>> {
    let root = clipboard_staging_dir();
    prune_staging_dir(&root, deskbridge_core::now_ms());
    let directory = root.join(format!("{}", deskbridge_core::now_ms()));
    fs::create_dir_all(&directory).with_context(|| {
        format!(
            "failed to create clipboard staging directory {}",
            directory.display()
        )
    })?;

    let mut staged = Vec::new();
    for file in files {
        let bytes = STANDARD
            .decode(file.data_base64)
            .with_context(|| format!("invalid clipboard file data for {}", file.name))?;
        if bytes.len() as u64 != file.size {
            return Err(anyhow!(
                "clipboard file {} size mismatch: declared {}, decoded {}",
                file.name,
                file.size,
                bytes.len()
            ));
        }

        let mut path = unique_staged_path(&directory, &sanitize_file_name(&file.name));
        if path.file_name().is_none() {
            path = unique_staged_path(&directory, "clipboard-file");
        }
        fs::write(&path, bytes)
            .with_context(|| format!("failed to write staged clipboard file {}", path.display()))?;
        staged.push(path);
    }

    if staged.is_empty() {
        return Err(anyhow!("remote clipboard file payload was empty"));
    }

    Ok(staged)
}

/// Best-effort removal of stale staging batches. Each batch lives in a
/// subdirectory whose name is the millisecond timestamp it was created at;
/// anything older than [`STAGING_TTL_MS`] is deleted.
fn prune_staging_dir(root: &Path, now_ms: u128) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let created_ms = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse::<u128>().ok());
        let stale = match created_ms {
            Some(created_ms) => now_ms.saturating_sub(created_ms) > STAGING_TTL_MS,
            // Unrecognized directory names are also cleaned up so manual or
            // legacy entries do not linger forever.
            None => true,
        };
        if stale {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

fn unique_staged_path(directory: &Path, name: &str) -> PathBuf {
    let clean = sanitize_file_name(name);
    let clean = if clean.is_empty() {
        "clipboard-file".to_string()
    } else {
        clean
    };
    let candidate = directory.join(&clean);
    if !candidate.exists() {
        return candidate;
    }

    let path = Path::new(&clean);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("clipboard-file");
    let extension = path.extension().and_then(|value| value.to_str());
    for index in 1..1000 {
        let name = match extension {
            Some(extension) if !extension.is_empty() => format!("{stem}-{index}.{extension}"),
            _ => format!("{stem}-{index}"),
        };
        let candidate = directory.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }

    directory.join(format!("clipboard-file-{}", deskbridge_core::now_ms()))
}

fn clipboard_staging_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("DeskBridge").join("Clipboard");
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("DeskBridge")
                .join("Clipboard");
        }
    }

    std::env::temp_dir().join("DeskBridge").join("Clipboard")
}

fn sanitize_file_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string()
}

fn validate_image_bytes(width: u32, height: u32, byte_len: usize) -> Result<()> {
    let expected = width as usize * height as usize * 4;
    if expected != byte_len {
        return Err(anyhow!(
            "invalid image byte length: {}x{} expects {}, got {}",
            width,
            height,
            expected,
            byte_len
        ));
    }
    Ok(())
}

fn clipboard_fingerprint(content: &ClipboardContent) -> u64 {
    let mut hasher = DefaultHasher::new();
    match content {
        ClipboardContent::Text { text } => {
            "text".hash(&mut hasher);
            text.hash(&mut hasher);
        }
        ClipboardContent::Image {
            width,
            height,
            rgba_base64,
        } => {
            "image".hash(&mut hasher);
            width.hash(&mut hasher);
            height.hash(&mut hasher);
            rgba_base64.hash(&mut hasher);
        }
        ClipboardContent::Files { files } => {
            "files".hash(&mut hasher);
            for file in files {
                file.name.hash(&mut hasher);
                file.size.hash(&mut hasher);
                file.data_base64.hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

pub fn content_summary(content: &ClipboardContent) -> String {
    match content {
        ClipboardContent::Text { text } => format!("text chars={}", text.chars().count()),
        ClipboardContent::Image { width, height, .. } => format!("image {width}x{height}"),
        ClipboardContent::Files { files } => {
            let bytes = files.iter().map(|file| file.size).sum::<u64>();
            format!("files count={} bytes={bytes}", files.len())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_file_names() {
        assert_eq!(sanitize_file_name("../bad:name?.txt"), "_bad_name_.txt");
        assert_eq!(sanitize_file_name(""), "");
        assert_eq!(sanitize_file_name("..."), "");
    }

    #[test]
    fn rejects_mismatched_image_size() {
        assert!(validate_image_bytes(2, 2, 15).is_err());
        assert!(validate_image_bytes(2, 2, 16).is_ok());
    }

    #[test]
    fn prune_staging_dir_removes_stale_batches() {
        let root =
            std::env::temp_dir().join(format!("deskbridge-prune-{}", deskbridge_core::now_ms()));
        let now_ms = 5_000_000_u128;
        let fresh = root.join((now_ms - 1_000).to_string());
        let stale = root.join("10");
        let junk = root.join("not-a-timestamp");
        fs::create_dir_all(&fresh).unwrap();
        fs::create_dir_all(&stale).unwrap();
        fs::create_dir_all(&junk).unwrap();

        // "now" is just past the fresh batch but far beyond the stale one.
        prune_staging_dir(&root, now_ms);

        assert!(fresh.exists());
        assert!(!stale.exists());
        assert!(!junk.exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn file_payload_round_trips_through_staging() {
        let files = vec![ClipboardFile {
            name: "hello.txt".to_string(),
            size: 5,
            data_base64: STANDARD.encode(b"hello"),
        }];

        let staged = stage_remote_files(files).unwrap();
        assert_eq!(staged.len(), 1);
        assert_eq!(fs::read_to_string(&staged[0]).unwrap(), "hello");
    }
}
