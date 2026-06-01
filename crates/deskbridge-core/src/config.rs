use crate::{Layout, Link, Screen, Size};
use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path};
use thiserror::Error;

pub const DEFAULT_REMOTE_SCROLL_SCALE: f64 = 1.0;
pub const MIN_REMOTE_SCROLL_SCALE: f64 = 0.25;
pub const MAX_REMOTE_SCROLL_SCALE: f64 = 2.0;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeskBridgeConfig {
    pub server: ServerConfig,
    pub client: ClientConfig,
    pub layout: Layout,
    pub reliability: ReliabilityConfig,
    #[serde(default)]
    pub input: InputConfig,
    #[serde(default)]
    pub clipboard: ClipboardConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerConfig {
    pub name: String,
    pub listen: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientConfig {
    pub name: String,
    pub server_addr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReliabilityConfig {
    pub heartbeat_ms: u64,
    pub reconnect_max_ms: u64,
    pub stale_after_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InputConfig {
    #[serde(default)]
    pub reverse_scroll: bool,
    #[serde(default = "default_remote_scroll_scale")]
    pub remote_scroll_scale: f64,
    /// Milliseconds the pointer must rest against a linked edge before the
    /// screen switches. `0` keeps the original switch-on-contact behavior.
    #[serde(default)]
    pub edge_switch_delay_ms: u64,
    /// Pixels from a perpendicular edge that count as a corner dead zone where
    /// switching is suppressed. `0` disables the corner guard.
    #[serde(default)]
    pub edge_corner_size: u32,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            reverse_scroll: false,
            remote_scroll_scale: DEFAULT_REMOTE_SCROLL_SCALE,
            edge_switch_delay_ms: 0,
            edge_corner_size: 0,
        }
    }
}

pub fn normalize_remote_scroll_scale(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(MIN_REMOTE_SCROLL_SCALE, MAX_REMOTE_SCROLL_SCALE)
    } else {
        DEFAULT_REMOTE_SCROLL_SCALE
    }
}

fn default_remote_scroll_scale() -> f64 {
    DEFAULT_REMOTE_SCROLL_SCALE
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClipboardConfig {
    #[serde(default = "default_clipboard_enabled")]
    pub enabled: bool,
    #[serde(default = "default_clipboard_enabled")]
    pub text: bool,
    #[serde(default = "default_clipboard_enabled")]
    pub image: bool,
    #[serde(default = "default_clipboard_enabled")]
    pub files: bool,
    #[serde(default = "default_clipboard_poll_ms")]
    pub poll_ms: u64,
    #[serde(default = "default_clipboard_max_transfer_bytes")]
    pub max_transfer_bytes: u64,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            text: true,
            image: true,
            files: true,
            poll_ms: default_clipboard_poll_ms(),
            max_transfer_bytes: default_clipboard_max_transfer_bytes(),
        }
    }
}

fn default_clipboard_enabled() -> bool {
    true
}

fn default_clipboard_poll_ms() -> u64 {
    750
}

fn default_clipboard_max_transfer_bytes() -> u64 {
    32 * 1024 * 1024
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("layout error: {0}")]
    Layout(#[from] crate::LayoutError),
}

impl Default for DeskBridgeConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                name: "windows".to_string(),
                listen: "0.0.0.0:24800".to_string(),
            },
            client: ClientConfig {
                name: "mac".to_string(),
                server_addr: "192.168.2.5:24800".to_string(),
            },
            layout: Layout {
                screens: vec![
                    Screen {
                        name: "windows".to_string(),
                        size: Size {
                            width: 1920,
                            height: 1080,
                        },
                        origin: None,
                    },
                    Screen {
                        name: "mac".to_string(),
                        size: Size {
                            width: 1728,
                            height: 1117,
                        },
                        origin: None,
                    },
                ],
                links: vec![Link {
                    from: "windows".to_string(),
                    edge: crate::Edge::Right,
                    to: "mac".to_string(),
                }],
            },
            reliability: ReliabilityConfig {
                heartbeat_ms: 2_000,
                reconnect_max_ms: 10_000,
                stale_after_ms: 6_000,
            },
            input: InputConfig::default(),
            clipboard: ClipboardConfig::default(),
        }
    }
}

impl DeskBridgeConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let text = fs::read_to_string(path)?;
        let config: Self = serde_json::from_str(&text)?;
        config.validate()?;
        Ok(config)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), ConfigError> {
        self.validate()?;
        let text = serde_json::to_string_pretty(self)?;
        fs::write(path, text)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        self.layout.validate()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_round_trips() {
        let config = DeskBridgeConfig::default();
        let json = serde_json::to_string_pretty(&config).unwrap();
        let decoded: DeskBridgeConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, decoded);
        decoded.validate().unwrap();
    }

    #[test]
    fn input_config_defaults_scroll_scale_for_old_configs() {
        let decoded: InputConfig = serde_json::from_str(r#"{"reverse_scroll":true}"#).unwrap();
        assert!(decoded.reverse_scroll);
        assert_eq!(decoded.remote_scroll_scale, DEFAULT_REMOTE_SCROLL_SCALE);
    }
}
