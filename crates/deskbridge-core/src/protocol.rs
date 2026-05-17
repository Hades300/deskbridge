use crate::{Edge, Layout, Size};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub const PROTOCOL_VERSION: u16 = 1;
pub const CLIPBOARD_PROTOCOL_VERSION: u16 = 1;
pub const DEFAULT_HEARTBEAT_MS: u64 = 2_000;
pub const REPLACED_SESSION_REASON: &str = "replaced by a newer session for the same screen";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Server,
    Client,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CryptoMode {
    None,
    Psk,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    InputInject,
    InputCapture,
    Clipboard,
    Diagnostics,
    LayoutV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Hello {
    pub protocol_version: u16,
    pub device_id: Uuid,
    pub screen_name: String,
    pub role: Role,
    pub crypto: CryptoMode,
    pub capabilities: Vec<Capability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_size: Option<Size>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clipboard_protocol: Option<u16>,
}

impl Hello {
    pub fn client(screen_name: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            device_id: Uuid::new_v4(),
            screen_name: screen_name.into(),
            role: Role::Client,
            crypto: CryptoMode::None,
            capabilities: vec![
                Capability::InputInject,
                Capability::Clipboard,
                Capability::Diagnostics,
                Capability::LayoutV1,
            ],
            screen_size: None,
            app_version: None,
            platform: None,
            build_commit: None,
            clipboard_protocol: Some(CLIPBOARD_PROTOCOL_VERSION),
        }
    }

    pub fn diagnostic(screen_name: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            device_id: Uuid::new_v4(),
            screen_name: screen_name.into(),
            role: Role::Client,
            crypto: CryptoMode::None,
            capabilities: vec![Capability::Diagnostics],
            screen_size: None,
            app_version: None,
            platform: None,
            build_commit: None,
            clipboard_protocol: None,
        }
    }

    pub fn server(screen_name: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            device_id: Uuid::new_v4(),
            screen_name: screen_name.into(),
            role: Role::Server,
            crypto: CryptoMode::None,
            capabilities: vec![
                Capability::InputCapture,
                Capability::Clipboard,
                Capability::Diagnostics,
                Capability::LayoutV1,
            ],
            screen_size: None,
            app_version: None,
            platform: None,
            build_commit: None,
            clipboard_protocol: Some(CLIPBOARD_PROTOCOL_VERSION),
        }
    }

    pub fn is_input_client(&self) -> bool {
        self.role == Role::Client && self.capabilities.contains(&Capability::InputInject)
    }

    pub fn with_screen_size(mut self, screen_size: Size) -> Self {
        self.screen_size = Some(screen_size);
        self
    }

    pub fn with_app_metadata(
        mut self,
        version: impl Into<String>,
        platform: impl Into<String>,
        commit: Option<&str>,
    ) -> Self {
        self.app_version = Some(version.into());
        self.platform = Some(platform.into());
        self.build_commit = commit.map(ToString::to_string);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Welcome {
    pub session_id: Uuid,
    pub server_name: String,
    pub heartbeat_interval_ms: u64,
    pub layout_revision: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<Capability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clipboard_protocol: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ping {
    pub seq: u64,
    pub sent_at_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Pong {
    pub seq: u64,
    pub sent_at_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventAck {
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at_ms: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_at_ms: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apply_duration_us: Option<u128>,
}

impl EventAck {
    pub fn new(seq: u64) -> Self {
        Self {
            seq,
            received_at_ms: None,
            applied_at_ms: None,
            apply_duration_us: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KeyState {
    Pressed,
    Released,
    Clicked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputEvent {
    MouseMove { dx: i32, dy: i32 },
    MouseAbs { x: i32, y: i32 },
    MouseButton { button: Button, state: KeyState },
    Wheel { dx: i32, dy: i32 },
    Key { key: String, state: KeyState },
    Text { text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InputPacket {
    pub seq: u64,
    pub event: InputEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClipboardPacket {
    pub seq: u64,
    pub sent_at_ms: u128,
    pub content: ClipboardContent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PortalFlashRole {
    Exit,
    Entry,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortalFlashPacket {
    pub seq: u64,
    pub screen: String,
    pub role: PortalFlashRole,
    pub edge: Edge,
    pub x: u32,
    pub y: u32,
    pub color: String,
    pub duration_ms: u32,
    pub speed_px_per_sec: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClipboardContent {
    Text {
        text: String,
    },
    Image {
        width: u32,
        height: u32,
        rgba_base64: String,
    },
    Files {
        files: Vec<ClipboardFile>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClipboardFile {
    pub name: String,
    pub size: u64,
    pub data_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DebugCommand {
    DisplayInfo,
    PeerInfo,
    RecentLogs,
    ServerLogs,
    MoveMouse {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        y: Option<i32>,
        #[serde(default)]
        dx: i32,
        #[serde(default)]
        dy: i32,
    },
    RouteProbe {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edge: Option<Edge>,
        #[serde(default = "default_route_probe_steps")]
        steps: u32,
        #[serde(default = "default_route_probe_dx")]
        dx: i32,
        #[serde(default)]
        dy: i32,
    },
    RouteStatus,
    Perf,
    InputSettings {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reverse_scroll: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote_scroll_scale: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layout: Option<Layout>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reset_route: Option<bool>,
    },
    CaptureProbe {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edge: Option<Edge>,
        #[serde(default = "default_route_probe_steps")]
        steps: u32,
        #[serde(default = "default_route_probe_dx")]
        dx: i32,
        #[serde(default)]
        dy: i32,
    },
}

fn default_route_probe_steps() -> u32 {
    3
}

fn default_route_probe_dx() -> i32 {
    80
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DebugRequest {
    pub request_id: Uuid,
    pub command: DebugCommand,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DisplaySnapshot {
    pub size: Size,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<(i32, i32)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DebugResponse {
    pub request_id: Uuid,
    pub ok: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<DisplaySnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StatusKind {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Status {
    pub kind: StatusKind,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum Message {
    Hello(Hello),
    Welcome(Welcome),
    Ping(Ping),
    Pong(Pong),
    Input(InputPacket),
    Ack(EventAck),
    Clipboard(ClipboardPacket),
    PortalFlash(PortalFlashPacket),
    DebugRequest(DebugRequest),
    DebugResponse(DebugResponse),
    Status(Status),
    Goodbye { reason: String },
}

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_round_trips_as_json() {
        let msg = Message::Hello(Hello::client("mac"));
        let encoded = serde_json::to_string(&msg).unwrap();
        let decoded: Message = serde_json::from_str(&encoded).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn diagnostic_hello_is_not_an_input_session() {
        assert!(Hello::client("mac").is_input_client());
        assert!(!Hello::diagnostic("mac").is_input_client());
    }

    #[test]
    fn client_hello_can_include_screen_size() {
        let hello = Hello::client("mac").with_screen_size(Size {
            width: 1512,
            height: 982,
        });
        let encoded = serde_json::to_string(&hello).unwrap();
        let decoded: Hello = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            decoded.screen_size,
            Some(Size {
                width: 1512,
                height: 982,
            })
        );
    }

    #[test]
    fn hello_advertises_clipboard_subprotocol_only_for_real_peers() {
        assert_eq!(
            Hello::client("mac").clipboard_protocol,
            Some(CLIPBOARD_PROTOCOL_VERSION)
        );
        assert_eq!(
            Hello::server("windows").clipboard_protocol,
            Some(CLIPBOARD_PROTOCOL_VERSION)
        );
        assert_eq!(Hello::diagnostic("mac").clipboard_protocol, None);
    }

    #[test]
    fn welcome_defaults_missing_capabilities_for_older_servers() {
        let json = r#"{
            "session_id":"00000000-0000-0000-0000-000000000001",
            "server_name":"windows",
            "heartbeat_interval_ms":2000,
            "layout_revision":1
        }"#;
        let decoded: Welcome = serde_json::from_str(json).unwrap();
        assert!(decoded.capabilities.is_empty());
        assert_eq!(decoded.clipboard_protocol, None);
    }

    #[test]
    fn debug_messages_round_trip() {
        let request = Message::DebugRequest(DebugRequest {
            request_id: Uuid::new_v4(),
            command: DebugCommand::MoveMouse {
                x: Some(10),
                y: Some(20),
                dx: 3,
                dy: -4,
            },
        });
        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: Message = serde_json::from_str(&encoded).unwrap();
        assert_eq!(request, decoded);
    }

    #[test]
    fn debug_peer_info_round_trips() {
        let request = Message::DebugRequest(DebugRequest {
            request_id: Uuid::new_v4(),
            command: DebugCommand::PeerInfo,
        });
        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: Message = serde_json::from_str(&encoded).unwrap();
        assert_eq!(request, decoded);
    }

    #[test]
    fn debug_server_logs_round_trips() {
        let request = Message::DebugRequest(DebugRequest {
            request_id: Uuid::new_v4(),
            command: DebugCommand::ServerLogs,
        });
        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: Message = serde_json::from_str(&encoded).unwrap();
        assert_eq!(request, decoded);
    }

    #[test]
    fn debug_route_probe_round_trips() {
        let request = Message::DebugRequest(DebugRequest {
            request_id: Uuid::new_v4(),
            command: DebugCommand::RouteProbe {
                edge: Some(Edge::Right),
                steps: 2,
                dx: 40,
                dy: -1,
            },
        });
        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: Message = serde_json::from_str(&encoded).unwrap();
        assert_eq!(request, decoded);
    }

    #[test]
    fn debug_route_status_round_trips() {
        let request = Message::DebugRequest(DebugRequest {
            request_id: Uuid::new_v4(),
            command: DebugCommand::RouteStatus,
        });
        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: Message = serde_json::from_str(&encoded).unwrap();
        assert_eq!(request, decoded);
    }

    #[test]
    fn debug_perf_round_trips() {
        let request = Message::DebugRequest(DebugRequest {
            request_id: Uuid::new_v4(),
            command: DebugCommand::Perf,
        });
        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: Message = serde_json::from_str(&encoded).unwrap();
        assert_eq!(request, decoded);
    }

    #[test]
    fn input_ack_can_include_latency_metadata() {
        let ack = Message::Ack(EventAck {
            seq: 42,
            received_at_ms: Some(100),
            applied_at_ms: Some(105),
            apply_duration_us: Some(420),
        });
        let encoded = serde_json::to_string(&ack).unwrap();
        let decoded: Message = serde_json::from_str(&encoded).unwrap();
        assert_eq!(ack, decoded);
    }

    #[test]
    fn clipboard_text_round_trips() {
        let packet = Message::Clipboard(ClipboardPacket {
            seq: 7,
            sent_at_ms: 123,
            content: ClipboardContent::Text {
                text: "hello".to_string(),
            },
        });
        let encoded = serde_json::to_string(&packet).unwrap();
        let decoded: Message = serde_json::from_str(&encoded).unwrap();
        assert_eq!(packet, decoded);
    }

    #[test]
    fn clipboard_image_round_trips() {
        let packet = Message::Clipboard(ClipboardPacket {
            seq: 8,
            sent_at_ms: 123,
            content: ClipboardContent::Image {
                width: 1,
                height: 1,
                rgba_base64: "AAAA/w==".to_string(),
            },
        });
        let encoded = serde_json::to_string(&packet).unwrap();
        let decoded: Message = serde_json::from_str(&encoded).unwrap();
        assert_eq!(packet, decoded);
    }

    #[test]
    fn clipboard_files_round_trips() {
        let packet = Message::Clipboard(ClipboardPacket {
            seq: 9,
            sent_at_ms: 123,
            content: ClipboardContent::Files {
                files: vec![ClipboardFile {
                    name: "note.txt".to_string(),
                    size: 5,
                    data_base64: "aGVsbG8=".to_string(),
                }],
            },
        });
        let encoded = serde_json::to_string(&packet).unwrap();
        let decoded: Message = serde_json::from_str(&encoded).unwrap();
        assert_eq!(packet, decoded);
    }

    #[test]
    fn legacy_portal_flash_round_trips_for_compatibility() {
        let packet = Message::PortalFlash(PortalFlashPacket {
            seq: 11,
            screen: "mac".to_string(),
            role: PortalFlashRole::Entry,
            edge: Edge::Left,
            x: 1,
            y: 540,
            color: "lime".to_string(),
            duration_ms: 320,
            speed_px_per_sec: 1400,
        });
        let encoded = serde_json::to_string(&packet).unwrap();
        let decoded: Message = serde_json::from_str(&encoded).unwrap();
        assert_eq!(packet, decoded);
    }

    #[test]
    fn debug_input_settings_round_trips() {
        let request = Message::DebugRequest(DebugRequest {
            request_id: Uuid::new_v4(),
            command: DebugCommand::InputSettings {
                reverse_scroll: Some(true),
                remote_scroll_scale: Some(0.75),
                layout: None,
                reset_route: None,
            },
        });
        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: Message = serde_json::from_str(&encoded).unwrap();
        assert_eq!(request, decoded);
    }

    #[test]
    fn debug_capture_probe_round_trips() {
        let request = Message::DebugRequest(DebugRequest {
            request_id: Uuid::new_v4(),
            command: DebugCommand::CaptureProbe {
                edge: Some(Edge::Right),
                steps: 2,
                dx: 40,
                dy: -1,
            },
        });
        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: Message = serde_json::from_str(&encoded).unwrap();
        assert_eq!(request, decoded);
    }
}
