use crate::{Layout, Link, Screen, Size};
use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeskBridgeConfig {
    pub server: ServerConfig,
    pub client: ClientConfig,
    pub layout: Layout,
    pub reliability: ReliabilityConfig,
    #[serde(default)]
    pub input: InputConfig,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InputConfig {
    #[serde(default)]
    pub reverse_scroll: bool,
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
}
