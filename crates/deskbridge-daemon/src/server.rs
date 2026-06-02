use crate::capture::CaptureEvent;
use anyhow::{Context, Result};
use deskbridge_core::secure::{recv, send};
use deskbridge_core::{
    Button, CLIPBOARD_PROTOCOL_VERSION, Capability, ClipboardConfig, ClipboardPacket,
    DEFAULT_HEARTBEAT_MS, DebugCommand, DebugRequest, DebugResponse, Edge, Encryption, FrameError,
    Hello, InputEvent, InputPacket, InputRouter, KeyState, Layout, Message,
    REPLACED_SESSION_REASON, Size, Status, StatusKind, Welcome, normalize_remote_scroll_scale,
    server_handshake,
};
use std::{
    collections::HashMap,
    collections::HashSet,
    collections::VecDeque,
    io,
    net::SocketAddr,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::AsyncWrite,
    net::TcpListener,
    net::TcpStream,
    sync::{Mutex, mpsc, oneshot},
    time,
};
use tracing::{debug, info, warn};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ServerOptions {
    pub listen: SocketAddr,
    pub name: String,
    pub allow: Vec<String>,
    pub demo_events: bool,
    pub capture: bool,
    pub debug_capture_log: bool,
    pub reverse_scroll: bool,
    pub remote_scroll_scale: f64,
    pub heartbeat_ms: u64,
    pub layout: Layout,
    pub clipboard: ClipboardConfig,
    pub edge_switch_delay_ms: u64,
    pub edge_corner_size: u32,
    pub psk: Option<String>,
}

type SessionRegistry = Arc<Mutex<HashMap<String, SessionHandle>>>;
type ServerDebugLog = Arc<StdMutex<VecDeque<String>>>;
type ServerRuntimeSettings = Arc<RuntimeSettings>;

#[derive(Clone)]
struct ServerShared {
    capture_tx: crate::capture::CaptureSender,
    sessions: SessionRegistry,
    runtime_settings: ServerRuntimeSettings,
    server_log: ServerDebugLog,
}

#[derive(Debug)]
struct RuntimeSettings {
    reverse_scroll: AtomicBool,
    remote_scroll_scale: AtomicU64,
    layout_revision: AtomicU64,
    layout: StdMutex<Layout>,
}

impl RuntimeSettings {
    fn new(
        reverse_scroll: bool,
        remote_scroll_scale: f64,
        layout: Layout,
    ) -> ServerRuntimeSettings {
        Arc::new(Self {
            reverse_scroll: AtomicBool::new(reverse_scroll),
            remote_scroll_scale: AtomicU64::new(encode_remote_scroll_scale(remote_scroll_scale)),
            layout_revision: AtomicU64::new(1),
            layout: StdMutex::new(layout),
        })
    }

    fn reverse_scroll(&self) -> bool {
        self.reverse_scroll.load(Ordering::Relaxed)
    }

    fn set_reverse_scroll(&self, value: bool) -> bool {
        self.reverse_scroll.swap(value, Ordering::Relaxed)
    }

    fn remote_scroll_scale(&self) -> f64 {
        decode_remote_scroll_scale(self.remote_scroll_scale.load(Ordering::Relaxed))
    }

    fn set_remote_scroll_scale(&self, value: f64) -> (f64, f64) {
        let encoded = encode_remote_scroll_scale(value);
        let previous = self.remote_scroll_scale.swap(encoded, Ordering::Relaxed);
        (
            decode_remote_scroll_scale(previous),
            decode_remote_scroll_scale(encoded),
        )
    }

    fn layout(&self) -> Layout {
        self.layout.lock().unwrap().clone()
    }

    fn set_layout(&self, layout: Layout) -> Result<bool, deskbridge_core::LayoutError> {
        layout.validate()?;
        let mut current = self.layout.lock().unwrap();
        let changed = *current != layout;
        if changed {
            *current = layout;
            self.layout_revision.fetch_add(1, Ordering::Relaxed);
        }
        Ok(changed)
    }

    fn layout_revision(&self) -> u64 {
        self.layout_revision.load(Ordering::Relaxed)
    }
}

const REMOTE_SCROLL_SCALE_UNITS: f64 = 1000.0;

fn encode_remote_scroll_scale(value: f64) -> u64 {
    (normalize_remote_scroll_scale(value) * REMOTE_SCROLL_SCALE_UNITS).round() as u64
}

fn decode_remote_scroll_scale(value: u64) -> f64 {
    normalize_remote_scroll_scale(value as f64 / REMOTE_SCROLL_SCALE_UNITS)
}

#[derive(Debug)]
struct SessionHandle {
    session_id: Uuid,
    peer: SocketAddr,
    connected_at_ms: u128,
    client_version: Option<String>,
    client_platform: Option<String>,
    client_commit: Option<String>,
    screen_size: Option<Size>,
    shutdown_tx: mpsc::UnboundedSender<()>,
    debug_tx: mpsc::UnboundedSender<DebugEnvelope>,
    route_debug_tx: mpsc::UnboundedSender<RouteDebugEnvelope>,
}

#[derive(Debug)]
struct DebugEnvelope {
    request: DebugRequest,
    response_tx: oneshot::Sender<DebugResponse>,
}

#[derive(Debug)]
struct RouteDebugEnvelope {
    request_id: Uuid,
    command: RouteDebugCommand,
    response_tx: oneshot::Sender<DebugResponse>,
}

#[derive(Debug, Clone, Copy)]
enum RouteDebugCommand {
    Probe(RouteProbeOptions),
    CaptureProbe(RouteProbeOptions),
    Status,
    Perf,
    ApplySettings { reset_route: bool },
}

#[derive(Debug)]
struct PendingRouteProbe {
    remaining_seqs: HashSet<u64>,
    logs: Vec<String>,
    response_tx: oneshot::Sender<DebugResponse>,
}

#[derive(Debug)]
struct PendingCaptureProbe {
    expected_capture_events: usize,
    processed_capture_events: usize,
    routed_events: usize,
    remaining_seqs: HashSet<u64>,
    logs: Vec<String>,
    response_tx: oneshot::Sender<DebugResponse>,
}

#[derive(Debug, Clone, Copy)]
struct SentEventSample {
    at_ms: u128,
    kind: crate::perf::EventKind,
}

#[derive(Debug, Clone, Copy)]
struct DurationSample {
    at_ms: u128,
    value: u128,
}

#[derive(Debug, Clone, Copy)]
struct OutstandingInput {
    sent_at_ms: u128,
}

#[derive(Debug)]
struct ServerPerfMetrics {
    started_at_ms: u128,
    capture_events: u64,
    routed_events: u64,
    sent_events: u64,
    ack_events: u64,
    route_misses: u64,
    mouse_move_events: u64,
    mouse_abs_events: u64,
    button_events: u64,
    wheel_events: u64,
    key_events: u64,
    text_events: u64,
    sent_window: VecDeque<SentEventSample>,
    ack_rtt_ms_window: VecDeque<DurationSample>,
    client_apply_us_window: VecDeque<DurationSample>,
    write_us_window: VecDeque<DurationSample>,
    outstanding: HashMap<u64, OutstandingInput>,
}

impl ServerPerfMetrics {
    fn new() -> Self {
        Self {
            started_at_ms: deskbridge_core::now_ms(),
            capture_events: 0,
            routed_events: 0,
            sent_events: 0,
            ack_events: 0,
            route_misses: 0,
            mouse_move_events: 0,
            mouse_abs_events: 0,
            button_events: 0,
            wheel_events: 0,
            key_events: 0,
            text_events: 0,
            sent_window: VecDeque::with_capacity(1024),
            ack_rtt_ms_window: VecDeque::with_capacity(1024),
            client_apply_us_window: VecDeque::with_capacity(1024),
            write_us_window: VecDeque::with_capacity(1024),
            outstanding: HashMap::new(),
        }
    }

    fn record_capture(&mut self) {
        self.capture_events += 1;
    }

    fn record_route_miss(&mut self) {
        self.route_misses += 1;
    }

    fn record_sent(
        &mut self,
        seq: u64,
        kind: crate::perf::EventKind,
        sent_at_ms: u128,
        write_us: u128,
    ) {
        self.routed_events += 1;
        self.sent_events += 1;
        match kind {
            crate::perf::EventKind::MouseMove => self.mouse_move_events += 1,
            crate::perf::EventKind::MouseAbs => self.mouse_abs_events += 1,
            crate::perf::EventKind::MouseButton => self.button_events += 1,
            crate::perf::EventKind::Wheel => self.wheel_events += 1,
            crate::perf::EventKind::Key => self.key_events += 1,
            crate::perf::EventKind::Text => self.text_events += 1,
        }
        self.sent_window.push_back(SentEventSample {
            at_ms: sent_at_ms,
            kind,
        });
        self.write_us_window.push_back(DurationSample {
            at_ms: sent_at_ms,
            value: write_us,
        });
        self.outstanding
            .insert(seq, OutstandingInput { sent_at_ms });
        self.trim(sent_at_ms);
    }

    fn record_ack(&mut self, ack: &deskbridge_core::EventAck, at_ms: u128) {
        self.ack_events += 1;
        if let Some(sent) = self.outstanding.remove(&ack.seq) {
            self.ack_rtt_ms_window.push_back(DurationSample {
                at_ms,
                value: at_ms.saturating_sub(sent.sent_at_ms),
            });
        }
        if let Some(apply_us) = ack.apply_duration_us {
            self.client_apply_us_window.push_back(DurationSample {
                at_ms,
                value: apply_us,
            });
        }
        self.trim(at_ms);
    }

    fn logs(&mut self, session_id: Uuid, peer: SocketAddr, client_name: &str) -> Vec<String> {
        let now = deskbridge_core::now_ms();
        self.trim(now);
        let window_ms = crate::perf::PERF_WINDOW_MS.min(now.saturating_sub(self.started_at_ms));
        let sent_window = self.sent_window.len();
        let mouse_window = self
            .sent_window
            .iter()
            .filter(|sample| sample.kind == crate::perf::EventKind::MouseMove)
            .count();
        let mut ack_rtt = self
            .ack_rtt_ms_window
            .iter()
            .map(|sample| sample.value)
            .collect::<Vec<_>>();
        let mut apply_us = self
            .client_apply_us_window
            .iter()
            .map(|sample| sample.value)
            .collect::<Vec<_>>();
        let mut write_us = self
            .write_us_window
            .iter()
            .map(|sample| sample.value)
            .collect::<Vec<_>>();

        let ack_p50 = crate::perf::percentile(&mut ack_rtt.clone(), 50);
        let ack_p95 = crate::perf::percentile(&mut ack_rtt.clone(), 95);
        let ack_p99 = crate::perf::percentile(&mut ack_rtt, 99);
        let apply_p50 = crate::perf::percentile(&mut apply_us.clone(), 50);
        let apply_p95 = crate::perf::percentile(&mut apply_us.clone(), 95);
        let apply_p99 = crate::perf::percentile(&mut apply_us, 99);
        let write_p50 = crate::perf::percentile(&mut write_us.clone(), 50);
        let write_p95 = crate::perf::percentile(&mut write_us.clone(), 95);
        let write_p99 = crate::perf::percentile(&mut write_us, 99);

        vec![
            "perf_scope=server_route_session".to_string(),
            format!("session={session_id} peer={peer} target={client_name}"),
            format!("window_ms={window_ms}"),
            format!("uptime_ms={}", now.saturating_sub(self.started_at_ms)),
            format!(
                "events_total capture={} routed={} sent={} ack={} capture_not_routed={} pending_ack={}",
                self.capture_events,
                self.routed_events,
                self.sent_events,
                self.ack_events,
                self.route_misses,
                self.outstanding.len()
            ),
            format!(
                "events_window sent={} mouse_move={} sent_hz={:.1} mouse_hz={:.1}",
                sent_window,
                mouse_window,
                crate::perf::rate_per_second(sent_window, window_ms.max(1)),
                crate::perf::rate_per_second(mouse_window, window_ms.max(1))
            ),
            format!(
                "event_counts mouse_move={} mouse_abs={} button={} wheel={} key={} text={}",
                self.mouse_move_events,
                self.mouse_abs_events,
                self.button_events,
                self.wheel_events,
                self.key_events,
                self.text_events
            ),
            format!(
                "ack_rtt p50={} p95={} p99={}",
                crate::perf::format_ms(ack_p50),
                crate::perf::format_ms(ack_p95),
                crate::perf::format_ms(ack_p99)
            ),
            format!(
                "client_apply p50={} p95={} p99={}",
                crate::perf::format_us(apply_p50),
                crate::perf::format_us(apply_p95),
                crate::perf::format_us(apply_p99)
            ),
            format!(
                "server_write p50={} p95={} p99={}",
                crate::perf::format_us(write_p50),
                crate::perf::format_us(write_p95),
                crate::perf::format_us(write_p99)
            ),
        ]
    }

    fn trim(&mut self, now_ms: u128) {
        let cutoff = now_ms.saturating_sub(crate::perf::PERF_WINDOW_MS);
        while self
            .sent_window
            .front()
            .is_some_and(|sample| sample.at_ms < cutoff)
        {
            self.sent_window.pop_front();
        }
        while self
            .ack_rtt_ms_window
            .front()
            .is_some_and(|sample| sample.at_ms < cutoff)
        {
            self.ack_rtt_ms_window.pop_front();
        }
        while self
            .client_apply_us_window
            .front()
            .is_some_and(|sample| sample.at_ms < cutoff)
        {
            self.client_apply_us_window.pop_front();
        }
        while self
            .write_us_window
            .front()
            .is_some_and(|sample| sample.at_ms < cutoff)
        {
            self.write_us_window.pop_front();
        }
    }
}

struct ClientSessionRuntime<'a> {
    options: &'a ServerOptions,
    enc: &'a Encryption,
    runtime_settings: ServerRuntimeSettings,
    client_name: &'a str,
    session_id: Uuid,
    peer: SocketAddr,
    shutdown_rx: &'a mut mpsc::UnboundedReceiver<()>,
    debug_rx: &'a mut mpsc::UnboundedReceiver<DebugEnvelope>,
    route_debug_rx: &'a mut mpsc::UnboundedReceiver<RouteDebugEnvelope>,
    capture_tx: crate::capture::CaptureSender,
    server_log: ServerDebugLog,
    client_clipboard_supported: bool,
}

#[derive(Debug, Default)]
struct RoutedCapture {
    event: Option<InputEvent>,
    released_to_local: bool,
}

fn new_server_debug_log() -> ServerDebugLog {
    Arc::new(StdMutex::new(VecDeque::with_capacity(256)))
}

fn push_server_log(log: &ServerDebugLog, line: impl Into<String>) {
    let Ok(mut entries) = log.lock() else {
        return;
    };
    if entries.len() == 256 {
        entries.pop_front();
    }
    entries.push_back(format!("{} {}", deskbridge_core::now_ms(), line.into()));
}

fn server_log_snapshot(log: &ServerDebugLog) -> Vec<String> {
    log.lock()
        .map(|entries| entries.iter().cloned().collect())
        .unwrap_or_default()
}

pub async fn run(options: ServerOptions) -> Result<()> {
    let options = apply_platform_layout(options);
    crate::capture::set_local_input_suppressed(false);
    let listener = TcpListener::bind(options.listen)
        .await
        .with_context(|| format!("failed to bind {}", options.listen))?;
    let allow = options
        .allow
        .iter()
        .map(|name| name.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let (capture_tx, _) = crate::capture::channel();
    let sessions = SessionRegistry::default();
    let server_log = new_server_debug_log();
    let runtime_settings = RuntimeSettings::new(
        options.reverse_scroll,
        options.remote_scroll_scale,
        options.layout.clone(),
    );

    if options.capture {
        start_platform_capture(capture_tx.clone())?;
    }

    // Advertise on the LAN so clients can discover this server without anyone
    // typing an IP address. Discovery is best-effort: failures (e.g. no
    // multicast-capable interface) must not stop the server from listening.
    let advertise_port = listener
        .local_addr()
        .map(|addr| addr.port())
        .unwrap_or_else(|_| options.listen.port());
    let _discovery = match crate::discovery::register(&options.name, advertise_port) {
        Ok(handle) => {
            info!(port = advertise_port, "advertising DeskBridge over mDNS");
            push_server_log(
                &server_log,
                format!(
                    "mDNS advertising screen={} port={advertise_port}",
                    options.name
                ),
            );
            Some(handle)
        }
        Err(err) => {
            warn!(error = %err, "mDNS advertising unavailable; clients must connect by address");
            push_server_log(
                &server_log,
                format!("mDNS advertising unavailable: {err:#}"),
            );
            None
        }
    };

    info!(listen = %options.listen, "server listening");
    push_server_log(
        &server_log,
        format!(
            "server listening listen={} screen={} capture={} debug_capture_log={} reverse_scroll={} remote_scroll_scale={:.2} edge_switch_delay_ms={} edge_corner_size={} version={} platform={}",
            options.listen,
            options.name,
            options.capture,
            options.debug_capture_log,
            options.reverse_scroll,
            normalize_remote_scroll_scale(options.remote_scroll_scale),
            options.edge_switch_delay_ms,
            options.edge_corner_size,
            crate::build_info::version(),
            crate::build_info::platform()
        ),
    );
    if options.debug_capture_log {
        warn!(
            "debug capture logging is enabled; high-frequency pointer logging may reduce smoothness"
        );
        push_server_log(
            &server_log,
            "warning: debug_capture_log is enabled; high-frequency pointer logging may reduce smoothness",
        );
    }
    let shared = ServerShared {
        capture_tx,
        sessions,
        runtime_settings,
        server_log: server_log.clone(),
    };

    loop {
        let (stream, peer) = listener.accept().await?;
        let options = options.clone();
        let allow = allow.clone();
        let shared = shared.clone();
        let server_log = server_log.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_client(stream, peer, options, allow, shared).await {
                push_server_log(
                    &server_log,
                    format!("client ended peer={peer} error={err:#}"),
                );
                warn!(peer = %peer, error = %err, "client ended");
            }
        });
    }
}

fn apply_platform_layout(options: ServerOptions) -> ServerOptions {
    #[cfg(target_os = "windows")]
    {
        let mut options = options;
        if let Some((width, height)) = crate::capture::windows::primary_screen_size()
            && options
                .layout
                .set_screen_size_preserving_links(&options.name, Size { width, height })
        {
            info!(
                screen = options.name,
                width, height, "using platform screen size for routing"
            );
        }
        options
    }

    #[cfg(target_os = "macos")]
    {
        let mut options = options;
        if let Some((width, height)) = crate::capture::macos::primary_screen_size()
            && options
                .layout
                .set_screen_size_preserving_links(&options.name, Size { width, height })
        {
            info!(
                screen = options.name,
                width, height, "using platform screen size for routing"
            );
        }
        options
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    options
}

fn apply_client_screen_size(options: &mut ServerOptions, hello: &Hello) {
    let Some(size) = hello.screen_size else {
        return;
    };
    if size.width == 0 || size.height == 0 {
        return;
    }

    if options.layout.set_screen_size_preserving_links(
        &hello.screen_name,
        Size {
            width: size.width,
            height: size.height,
        },
    ) {
        info!(
            screen = hello.screen_name,
            width = size.width,
            height = size.height,
            "using client-reported screen size for routing"
        );
    }
}

fn session_route_layout(
    mut layout: Layout,
    session_layout: &Layout,
    server_name: &str,
    client_name: &str,
) -> Layout {
    apply_session_screen_size(&mut layout, session_layout, server_name);
    apply_session_screen_size(&mut layout, session_layout, client_name);
    layout
}

fn apply_session_screen_size(layout: &mut Layout, session_layout: &Layout, screen_name: &str) {
    let Some(screen) = session_layout
        .screens
        .iter()
        .find(|screen| screen.name == screen_name)
    else {
        return;
    };

    let _ = layout.set_screen_size_preserving_links(screen_name, screen.size);
}

async fn handle_client(
    mut stream: TcpStream,
    peer: SocketAddr,
    mut options: ServerOptions,
    allow: HashSet<String>,
    shared: ServerShared,
) -> Result<()> {
    let ServerShared {
        capture_tx,
        sessions,
        runtime_settings,
        server_log,
    } = shared;
    stream.set_nodelay(true)?;

    // If a shared secret is configured, require the encrypted Noise handshake
    // before any application data. A peer without the matching secret cannot
    // complete it, so unauthenticated connections are rejected here.
    let enc = match options.psk.as_deref() {
        Some(secret) if !secret.is_empty() => match server_handshake(&mut stream, secret).await {
            Ok(session) => Encryption::secure(session),
            Err(err) => {
                push_server_log(
                    &server_log,
                    format!("rejected peer={peer} psk handshake failed: {err:#}"),
                );
                warn!(peer = %peer, error = %err, "PSK handshake failed; rejecting client");
                return Ok(());
            }
        },
        _ => Encryption::Plain,
    };

    let hello = match recv(&mut stream, &enc).await {
        Ok(Message::Hello(hello)) => hello,
        Ok(other) => {
            send(
                &mut stream,
                &enc,
                &Message::Status(Status {
                    kind: StatusKind::Error,
                    message: format!("expected hello, got {other:?}"),
                }),
            )
            .await?;
            return Ok(());
        }
        Err(FrameError::ForeignProtocol { magic }) => {
            push_server_log(
                &server_log,
                format!(
                    "foreign protocol peer={peer} magic={magic}; likely Input Leap/Barrier/Synergy pointed at this port"
                ),
            );
            warn!(
                peer = %peer,
                magic,
                "non-DeskBridge client connected; this is usually an old Input Leap, Barrier, or Synergy client pointed at the DeskBridge port"
            );
            return Ok(());
        }
        Err(err) => return Err(err.into()),
    };

    if let Err(err) = validate_client(&hello, &allow, &mut stream, &enc).await {
        push_server_log(
            &server_log,
            format!(
                "rejected peer={peer} screen={} role={:?} version={} platform={} error={err:#}",
                hello.screen_name,
                hello.role,
                hello.app_version.as_deref().unwrap_or("unknown"),
                hello.platform.as_deref().unwrap_or("unknown"),
            ),
        );
        return Err(err);
    }
    if hello.is_input_client() {
        apply_client_screen_size(&mut options, &hello);
    }

    let session_id = Uuid::new_v4();
    let welcome = Message::Welcome(Welcome {
        session_id,
        server_name: options.name.clone(),
        heartbeat_interval_ms: options.heartbeat_ms.max(DEFAULT_HEARTBEAT_MS),
        layout_revision: 1,
        capabilities: server_capabilities(&options),
        clipboard_protocol: options
            .clipboard
            .enabled
            .then_some(CLIPBOARD_PROTOCOL_VERSION),
    });

    if !hello.is_input_client() {
        push_server_log(
            &server_log,
            format!(
                "diagnostic client accepted peer={peer} target={} version={} platform={}",
                hello.screen_name,
                hello.app_version.as_deref().unwrap_or("unknown"),
                hello.platform.as_deref().unwrap_or("unknown"),
            ),
        );
        info!(
            peer = %peer,
            screen = hello.screen_name,
            role = ?hello.role,
            capabilities = ?hello.capabilities,
            "diagnostic client accepted"
        );
        send(&mut stream, &enc, &welcome).await?;
        return handle_diagnostic_session(
            &mut stream,
            &enc,
            &hello.screen_name,
            &options,
            sessions,
            runtime_settings,
            server_log,
        )
        .await;
    }

    let (shutdown_tx, mut shutdown_rx) = mpsc::unbounded_channel();
    let (debug_tx, mut debug_rx) = mpsc::unbounded_channel();
    let (route_debug_tx, mut route_debug_rx) = mpsc::unbounded_channel();
    let session_key = hello.screen_name.to_ascii_lowercase();
    if let Some(previous) = sessions.lock().await.insert(
        session_key.clone(),
        SessionHandle {
            session_id,
            peer,
            connected_at_ms: deskbridge_core::now_ms(),
            client_version: hello.app_version.clone(),
            client_platform: hello.platform.clone(),
            client_commit: hello.build_commit.clone(),
            screen_size: hello.screen_size,
            shutdown_tx,
            debug_tx,
            route_debug_tx,
        },
    ) {
        let _ = previous.shutdown_tx.send(());
        push_server_log(
            &server_log,
            format!(
                "replaced existing client screen={} previous_session={} new_session={}",
                hello.screen_name, previous.session_id, session_id
            ),
        );
        warn!(
            peer = %peer,
            screen = hello.screen_name,
            previous_session = %previous.session_id,
            new_session = %session_id,
            "replaced existing client session for screen"
        );
    }

    info!(peer = %peer, screen = hello.screen_name, "client accepted");
    push_server_log(
        &server_log,
        format!(
            "client accepted peer={peer} screen={} session={} version={} platform={} screen_size={}",
            hello.screen_name,
            session_id,
            hello.app_version.as_deref().unwrap_or("unknown"),
            hello.platform.as_deref().unwrap_or("unknown"),
            hello
                .screen_size
                .map(|size| format!("{}x{}", size.width, size.height))
                .unwrap_or_else(|| "unknown".to_string())
        ),
    );

    send(&mut stream, &enc, &welcome).await?;
    let client_clipboard_supported = hello.capabilities.contains(&Capability::Clipboard)
        && hello.clipboard_protocol == Some(CLIPBOARD_PROTOCOL_VERSION);

    let result = run_client_session(
        stream,
        ClientSessionRuntime {
            options: &options,
            enc: &enc,
            runtime_settings,
            client_name: &hello.screen_name,
            session_id,
            peer,
            shutdown_rx: &mut shutdown_rx,
            debug_rx: &mut debug_rx,
            route_debug_rx: &mut route_debug_rx,
            capture_tx,
            server_log: server_log.clone(),
            client_clipboard_supported,
        },
    )
    .await;
    remove_current_session(&sessions, &session_key, session_id).await;
    crate::capture::set_local_input_suppressed(false);
    match &result {
        Ok(()) => push_server_log(
            &server_log,
            format!(
                "client session ended screen={} peer={peer} session={session_id}",
                hello.screen_name
            ),
        ),
        Err(err) => push_server_log(
            &server_log,
            format!(
                "client session failed screen={} peer={peer} session={session_id} error={err:#}",
                hello.screen_name
            ),
        ),
    }
    result
}

async fn run_client_session(stream: TcpStream, runtime: ClientSessionRuntime<'_>) -> Result<()> {
    let ClientSessionRuntime {
        options,
        enc,
        runtime_settings,
        client_name,
        session_id,
        peer,
        shutdown_rx,
        debug_rx,
        route_debug_rx,
        capture_tx,
        server_log,
        client_clipboard_supported,
    } = runtime;
    let mut ticker = time::interval(Duration::from_secs(5));
    let (reader, mut writer) = stream.into_split();
    let mut inbound = spawn_reader(reader, enc.clone());
    let mut seq = 0_u64;
    let mut demo_stage = 0_u64;
    let mut route_layout = session_route_layout(
        runtime_settings.layout(),
        &options.layout,
        &options.name,
        client_name,
    );
    let mut demo_router =
        build_session_router(route_layout.clone(), options.name.clone(), options).ok();
    let mut capture_rx = capture_tx.subscribe();
    let mut pending_debug = HashMap::<Uuid, oneshot::Sender<DebugResponse>>::new();
    let mut pending_route_probes = HashMap::<Uuid, PendingRouteProbe>::new();
    let mut route_probe_seq_index = HashMap::<u64, Uuid>::new();
    let mut pending_capture_probes = HashMap::<Uuid, PendingCaptureProbe>::new();
    let mut capture_probe_seq_index = HashMap::<u64, Uuid>::new();
    let mut perf = ServerPerfMetrics::new();
    let mut last_unrouted_capture_log_ms = 0_u128;
    let mut scroll_accumulator = ScrollScaleAccumulator::default();
    let mut remote_input_state = RemoteInputState::default();
    let mut clipboard_options = options.clipboard.clone();
    if !client_clipboard_supported {
        clipboard_options.enabled = false;
    }
    let clipboard_runtime = crate::clipboard::ClipboardRuntime::new(clipboard_options);
    let mut clipboard_rx = clipboard_runtime
        .as_ref()
        .map(crate::clipboard::ClipboardRuntime::spawn_watcher);

    loop {
        tokio::select! {
            _ = ticker.tick(), if options.demo_events => {
                seq += 1;
                let event = transform_routed_input_event(
                    next_demo_event(
                    &mut demo_router,
                    &route_layout,
                    &options.name,
                    client_name,
                    demo_stage,
                    ),
                    runtime_settings.reverse_scroll(),
                    runtime_settings.remote_scroll_scale(),
                    &mut scroll_accumulator,
                );
                demo_stage += 1;
                write_tracked_input_packet(
                    &mut writer,
                    enc,
                    &mut seq,
                    event,
                    &mut perf,
                    &mut remote_input_state,
                )
                .await?;
            }
            event = capture_rx.recv() => {
                if let Ok(event) = event {
                    perf.record_capture();
                    let capture_log_line = if options.debug_capture_log {
                        Some(describe_capture_event(&event))
                    } else {
                        None
                    };
                    let probe_id = capture_probe_id(&event);
                    let capture_event = capture_event_payload(event);
                    let routed = route_capture_event_for_client(
                        &mut demo_router,
                        capture_event,
                        client_name,
                    );
                    if probe_id.is_none() {
                        let suppress_local_input = demo_router
                            .as_ref()
                            .is_some_and(|router| router.active_screen() != options.name);
                        crate::capture::set_local_input_suppressed(suppress_local_input);
                    }

                    if probe_id.is_none()
                        && let Some(capture_log_line) = capture_log_line
                    {
                        let now_ms = deskbridge_core::now_ms();
                        let routed_to_client = routed.event.is_some();
                        let should_log = routed_to_client
                            || now_ms.saturating_sub(last_unrouted_capture_log_ms) >= 250;
                        if should_log {
                            if !routed_to_client {
                                last_unrouted_capture_log_ms = now_ms;
                            }
                            let route_log_line = routed
                                .event
                                .as_ref()
                                .map(|event| format!("routed target={client_name} {}", describe_input_event(event)))
                                .unwrap_or_else(|| "not routed".to_string());
                            push_server_log(
                                &server_log,
                                format!(
                                    "capture session={session_id} peer={peer} source={capture_log_line} {route_log_line}"
                                ),
                            );
                        }
                    }

                    if let Some(request_id) = probe_id
                        && let Some(probe) = pending_capture_probes.get_mut(&request_id)
                    {
                        probe.processed_capture_events += 1;
                        match &routed.event {
                            Some(event) => probe.logs.push(format!(
                                "capture event {} routed target={} {}",
                                probe.processed_capture_events,
                                client_name,
                                describe_input_event(event)
                            )),
                            None => probe.logs.push(format!(
                                "capture event {} did not route to target {}",
                                probe.processed_capture_events,
                                client_name
                            )),
                        }
                    }

                    if let Some(event) = routed.event {
                        let event = transform_routed_input_event(
                            event,
                            runtime_settings.reverse_scroll(),
                            runtime_settings.remote_scroll_scale(),
                            &mut scroll_accumulator,
                        );
                        write_tracked_input_packet(
                            &mut writer,
                            enc,
                            &mut seq,
                            event,
                            &mut perf,
                            &mut remote_input_state,
                        )
                        .await?;
                        if let Some(request_id) = probe_id
                            && let Some(probe) = pending_capture_probes.get_mut(&request_id)
                        {
                            capture_probe_seq_index.insert(seq, request_id);
                            probe.remaining_seqs.insert(seq);
                            probe.routed_events += 1;
                        }
                    } else {
                        if routed.released_to_local {
                            let release_count = write_remote_release_events(
                                &mut writer,
                                enc,
                                &mut seq,
                                &mut perf,
                                &mut remote_input_state,
                            )
                            .await?;
                            log_remote_release(
                                &server_log,
                                release_count,
                                session_id,
                                peer,
                                client_name,
                            );
                        }
                        perf.record_route_miss();
                    }

                    if let Some(request_id) = probe_id {
                        maybe_finish_capture_probe(
                            request_id,
                            &mut pending_capture_probes,
                            &mut capture_probe_seq_index,
                        );
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                let _ = send(&mut writer, enc, &Message::Goodbye {
                    reason: REPLACED_SESSION_REASON.to_string(),
                }).await;
                return Ok(());
            }
            packet = recv_clipboard(&mut clipboard_rx), if clipboard_rx.is_some() => {
                if let Some(packet) = packet {
                    let summary = crate::clipboard::content_summary(&packet.content);
                    send(&mut writer, enc, &Message::Clipboard(packet)).await?;
                    push_server_log(
                        &server_log,
                        format!("clipboard sent session={session_id} target={client_name} {summary}"),
                    );
                }
            }
            Some(debug) = debug_rx.recv() => {
                let request_id = debug.request.request_id;
                if let Err(err) = send(&mut writer, enc, &Message::DebugRequest(debug.request)).await {
                    let _ = debug.response_tx.send(debug_error_response(
                        request_id,
                        format!("failed to send debug request to client: {err}"),
                    ));
                    return Err(err.into());
                }
                pending_debug.insert(request_id, debug.response_tx);
            }
            Some(route_debug) = route_debug_rx.recv() => {
                match route_debug.command {
                    RouteDebugCommand::Status => {
                        let _ = route_debug.response_tx.send(DebugResponse {
                            request_id: route_debug.request_id,
                            ok: true,
                            message: "route status read".to_string(),
                            display: None,
                            logs: build_route_status_logs(
                                options,
                                &runtime_settings,
                                client_name,
                                &demo_router,
                                route_debug.request_id,
                                &route_layout,
                            ),
                        });
                        continue;
                    }
                    RouteDebugCommand::Perf => {
                        let _ = route_debug.response_tx.send(DebugResponse {
                            request_id: route_debug.request_id,
                            ok: true,
                            message: "server perf metrics read".to_string(),
                            display: None,
                            logs: perf.logs(session_id, peer, client_name),
                        });
                        continue;
                    }
                    RouteDebugCommand::Probe(probe_options) => {
                        let (events, logs) = match build_route_probe_events(
                            options,
                            &route_layout,
                            client_name,
                            probe_options,
                            route_debug.request_id,
                        ) {
                            Ok(probe_events) => probe_events,
                            Err(err) => {
                                let _ = route_debug.response_tx.send(debug_error_response(
                                    route_debug.request_id,
                                    format!("{err:#}"),
                                ));
                                continue;
                            }
                        };
                        let mut seqs = HashSet::new();
                        for event in events {
                            let event = transform_routed_input_event(
                                event,
                                runtime_settings.reverse_scroll(),
                                runtime_settings.remote_scroll_scale(),
                                &mut scroll_accumulator,
                            );
                            write_tracked_input_packet(
                                &mut writer,
                                enc,
                                &mut seq,
                                event,
                                &mut perf,
                                &mut remote_input_state,
                            )
                            .await?;
                            route_probe_seq_index.insert(seq, route_debug.request_id);
                            seqs.insert(seq);
                        }

                        if seqs.is_empty() {
                            let _ = route_debug.response_tx.send(debug_error_response(
                                route_debug.request_id,
                                "route probe did not produce input events".to_string(),
                            ));
                        } else {
                            pending_route_probes.insert(route_debug.request_id, PendingRouteProbe {
                                remaining_seqs: seqs,
                                logs,
                                response_tx: route_debug.response_tx,
                            });
                        }
                    }
                    RouteDebugCommand::CaptureProbe(probe_options) => {
                        let (events, logs) = match build_capture_probe_events(
                            options,
                            &route_layout,
                            client_name,
                            probe_options,
                            route_debug.request_id,
                        ) {
                            Ok(probe_events) => probe_events,
                            Err(err) => {
                                let _ = route_debug.response_tx.send(debug_error_response(
                                    route_debug.request_id,
                                    format!("{err:#}"),
                                ));
                                continue;
                            }
                        };

                        if events.is_empty() {
                            let _ = route_debug.response_tx.send(debug_error_response(
                                route_debug.request_id,
                                "capture probe did not produce capture events".to_string(),
                            ));
                            continue;
                        }

                        pending_capture_probes.insert(route_debug.request_id, PendingCaptureProbe {
                            expected_capture_events: events.len(),
                            processed_capture_events: 0,
                            routed_events: 0,
                            remaining_seqs: HashSet::new(),
                            logs,
                            response_tx: route_debug.response_tx,
                        });

                        for event in events {
                            if let Err(err) = capture_tx.send(event) {
                                if let Some(probe) = pending_capture_probes.remove(&route_debug.request_id) {
                                    let _ = probe.response_tx.send(debug_error_response(
                                        route_debug.request_id,
                                        format!("failed to inject synthetic capture event: {err}"),
                                    ));
                                }
                                break;
                            }
                        }
                    }
                    RouteDebugCommand::ApplySettings { reset_route } => {
                        route_layout = session_route_layout(
                            runtime_settings.layout(),
                            &options.layout,
                            &options.name,
                            client_name,
                        );
                        if reset_route || demo_router.is_none() {
                            let release_count = write_remote_release_events(
                                &mut writer,
                                enc,
                                &mut seq,
                                &mut perf,
                                &mut remote_input_state,
                            )
                            .await?;
                            log_remote_release(
                                &server_log,
                                release_count,
                                session_id,
                                peer,
                                client_name,
                            );
                            demo_router =
                                build_session_router(route_layout.clone(), options.name.clone(), options).ok();
                            crate::capture::set_local_input_suppressed(false);
                        }
                        let active_screen = demo_router
                            .as_ref()
                            .map(|router| router.active_screen().to_string())
                            .unwrap_or_else(|| "unavailable".to_string());
                        let _ = route_debug.response_tx.send(DebugResponse {
                            request_id: route_debug.request_id,
                            ok: demo_router.is_some(),
                            message: if demo_router.is_some() {
                                "runtime route settings applied".to_string()
                            } else {
                                "runtime route settings saved, but router could not be rebuilt".to_string()
                            },
                            display: None,
                            logs: vec![
                                format!("session route reset={reset_route} active_screen={active_screen}"),
                                format!("layout_revision={}", runtime_settings.layout_revision()),
                            ],
                        });
                    }
                }
            }
            msg = inbound.recv() => {
                let message = msg
                    .ok_or_else(|| anyhow::anyhow!("client reader stopped"))??;
                match message {
                    Message::Ping(ping) => {
                        send(&mut writer, enc, &Message::Pong(deskbridge_core::Pong {
                            seq: ping.seq,
                            sent_at_ms: ping.sent_at_ms,
                        })).await?;
                    }
                    Message::Pong(pong) => debug!(seq = pong.seq, "client pong"),
                    Message::Ack(ack) => {
                        perf.record_ack(&ack, deskbridge_core::now_ms());
                        debug!(seq = ack.seq, "input event acknowledged");
                        if let Some(request_id) = route_probe_seq_index.remove(&ack.seq) {
                            let completed = if let Some(probe) = pending_route_probes.get_mut(&request_id) {
                                probe.remaining_seqs.remove(&ack.seq);
                                probe.logs.push(format!("ack seq={}", ack.seq));
                                probe.remaining_seqs.is_empty()
                            } else {
                                false
                            };
                            if completed
                                && let Some(probe) = pending_route_probes.remove(&request_id)
                            {
                                let event_count = probe
                                    .logs
                                    .iter()
                                    .filter(|line| line.starts_with("event "))
                                    .count();
                                let message = format!(
                                    "route probe delivered and acknowledged {} events",
                                    event_count
                                );
                                let _ = probe.response_tx.send(DebugResponse {
                                    request_id,
                                    ok: true,
                                    message,
                                    display: None,
                                    logs: probe.logs,
                                });
                            }
                        }
                        if let Some(request_id) = capture_probe_seq_index.remove(&ack.seq) {
                            if let Some(probe) = pending_capture_probes.get_mut(&request_id) {
                                probe.remaining_seqs.remove(&ack.seq);
                                probe.logs.push(format!("ack seq={}", ack.seq));
                            }
                            maybe_finish_capture_probe(
                                request_id,
                                &mut pending_capture_probes,
                                &mut capture_probe_seq_index,
                            );
                        }
                    }
                    Message::DebugResponse(response) => {
                        if let Some(response_tx) = pending_debug.remove(&response.request_id) {
                            let _ = response_tx.send(response);
                        } else {
                            debug!(request_id = %response.request_id, "unexpected debug response");
                        }
                    }
                    Message::Clipboard(packet) => {
                        if let Some(runtime) = &clipboard_runtime {
                            match runtime.apply_remote(packet).await {
                                Ok(summary) => push_server_log(
                                    &server_log,
                                    format!("clipboard applied session={session_id} source={client_name} {summary}"),
                                ),
                                Err(err) => {
                                    push_server_log(
                                        &server_log,
                                        format!("clipboard apply failed session={session_id} source={client_name} error={err:#}"),
                                    );
                                    warn!(error = %err, "remote clipboard apply failed");
                                }
                            }
                        }
                    }
                    Message::Goodbye { reason } => {
                        info!(reason, "client goodbye");
                        return Ok(());
                    }
                    other => debug!(message = ?other, "ignored message"),
                }
            }
        }
    }
}

fn server_capabilities(options: &ServerOptions) -> Vec<Capability> {
    let mut capabilities = vec![
        Capability::InputCapture,
        Capability::Diagnostics,
        Capability::LayoutV1,
    ];
    if options.clipboard.enabled {
        capabilities.push(Capability::Clipboard);
    }
    capabilities
}

async fn recv_clipboard(
    rx: &mut Option<mpsc::UnboundedReceiver<ClipboardPacket>>,
) -> Option<ClipboardPacket> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

async fn handle_diagnostic_session(
    stream: &mut TcpStream,
    enc: &Encryption,
    target_screen: &str,
    options: &ServerOptions,
    sessions: SessionRegistry,
    runtime_settings: ServerRuntimeSettings,
    server_log: ServerDebugLog,
) -> Result<()> {
    match time::timeout(Duration::from_secs(5), recv(stream, enc)).await {
        Ok(Ok(Message::DebugRequest(request))) => match request.command.clone() {
            DebugCommand::ServerLogs => {
                let response = build_server_logs_response(
                    request.request_id,
                    target_screen,
                    options,
                    &runtime_settings,
                    &sessions,
                    &server_log,
                )
                .await;
                send(stream, enc, &Message::DebugResponse(response)).await?;
                Ok(())
            }
            DebugCommand::RouteProbe {
                edge,
                steps,
                dx,
                dy,
            } => {
                forward_route_debug_request(
                    stream,
                    enc,
                    target_screen,
                    sessions,
                    request.request_id,
                    RouteDebugCommand::Probe(RouteProbeOptions {
                        edge,
                        steps,
                        dx,
                        dy,
                    }),
                )
                .await
            }
            DebugCommand::CaptureProbe {
                edge,
                steps,
                dx,
                dy,
            } => {
                forward_route_debug_request(
                    stream,
                    enc,
                    target_screen,
                    sessions,
                    request.request_id,
                    RouteDebugCommand::CaptureProbe(RouteProbeOptions {
                        edge,
                        steps,
                        dx,
                        dy,
                    }),
                )
                .await
            }
            DebugCommand::RouteStatus => {
                forward_route_debug_request(
                    stream,
                    enc,
                    target_screen,
                    sessions,
                    request.request_id,
                    RouteDebugCommand::Status,
                )
                .await
            }
            DebugCommand::Perf => {
                forward_route_debug_request(
                    stream,
                    enc,
                    target_screen,
                    sessions,
                    request.request_id,
                    RouteDebugCommand::Perf,
                )
                .await
            }
            DebugCommand::InputSettings {
                reverse_scroll,
                remote_scroll_scale,
                layout,
                reset_route,
            } => {
                let mut response = update_runtime_input_settings(
                    request.request_id,
                    reverse_scroll,
                    remote_scroll_scale,
                    layout.clone(),
                    &runtime_settings,
                    &server_log,
                );

                if response.ok && (layout.is_some() || reset_route.unwrap_or(false)) {
                    let command = RouteDebugCommand::ApplySettings {
                        reset_route: reset_route.unwrap_or(false) || layout.is_some(),
                    };
                    match request_route_debug_response(
                        target_screen,
                        &sessions,
                        request.request_id,
                        command,
                    )
                    .await
                    {
                        Some(route_response) => {
                            response.logs.extend(route_response.logs);
                            if !route_response.ok {
                                response.ok = false;
                                response.message = route_response.message;
                            }
                        }
                        None => response.logs.push(format!(
                            "runtime settings saved; target client '{target_screen}' is not connected"
                        )),
                    }
                }

                send(stream, enc, &Message::DebugResponse(response)).await?;
                Ok(())
            }
            _ => forward_debug_request(stream, enc, target_screen, sessions, request).await,
        },
        Ok(Ok(other)) => {
            send(
                stream,
                enc,
                &Message::Status(Status {
                    kind: StatusKind::Error,
                    message: format!("expected debug request, got {other:?}"),
                }),
            )
            .await?;
            Ok(())
        }
        Ok(Err(FrameError::Io(err))) if err.kind() == io::ErrorKind::UnexpectedEof => Ok(()),
        Ok(Err(err)) => Err(err.into()),
        Err(_) => Ok(()),
    }
}

async fn write_input_packet<W>(
    writer: &mut W,
    enc: &Encryption,
    seq: u64,
    event: InputEvent,
    perf: &mut ServerPerfMetrics,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let kind = crate::perf::event_kind(&event);
    let packet = InputPacket { seq, event };
    let started = Instant::now();
    let message = Message::Input(packet);
    send(writer, enc, &message).await?;
    let sent_at_ms = deskbridge_core::now_ms();
    perf.record_sent(seq, kind, sent_at_ms, started.elapsed().as_micros());
    Ok(())
}

async fn write_tracked_input_packet<W>(
    writer: &mut W,
    enc: &Encryption,
    seq: &mut u64,
    event: InputEvent,
    perf: &mut ServerPerfMetrics,
    remote_input_state: &mut RemoteInputState,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    remote_input_state.observe(&event);
    *seq += 1;
    write_input_packet(writer, enc, *seq, event, perf).await
}

async fn write_remote_release_events<W>(
    writer: &mut W,
    enc: &Encryption,
    seq: &mut u64,
    perf: &mut ServerPerfMetrics,
    remote_input_state: &mut RemoteInputState,
) -> Result<usize>
where
    W: AsyncWrite + Unpin,
{
    let release_events = remote_input_state.release_events();
    if release_events.is_empty() {
        return Ok(0);
    }

    let release_count = release_events.len();
    for event in release_events {
        write_tracked_input_packet(writer, enc, seq, event, perf, remote_input_state).await?;
    }
    Ok(release_count)
}

fn log_remote_release(
    server_log: &ServerDebugLog,
    release_count: usize,
    session_id: Uuid,
    peer: SocketAddr,
    client_name: &str,
) {
    if release_count == 0 {
        return;
    }
    push_server_log(
        server_log,
        format!(
            "released {release_count} remote pressed inputs screen={client_name} peer={peer} session={session_id}"
        ),
    );
}

fn spawn_reader<R>(
    mut reader: R,
    enc: Encryption,
) -> mpsc::UnboundedReceiver<Result<Message, FrameError>>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let result = recv(&mut reader, &enc).await;
            let should_stop = result.is_err();
            if tx.send(result).is_err() || should_stop {
                break;
            }
        }
    });
    rx
}

#[derive(Debug, Clone, Copy)]
struct RouteProbeOptions {
    edge: Option<Edge>,
    steps: u32,
    dx: i32,
    dy: i32,
}

async fn build_server_logs_response(
    request_id: Uuid,
    target_screen: &str,
    options: &ServerOptions,
    runtime_settings: &ServerRuntimeSettings,
    sessions: &SessionRegistry,
    server_log: &ServerDebugLog,
) -> DebugResponse {
    let mut logs = crate::build_info::lines();
    logs.push("role=server".to_string());
    logs.push(format!("listen={}", options.listen));
    logs.push(format!("server_screen={}", options.name));
    logs.push(format!("target_screen={target_screen}"));
    logs.push(format!("allowed_clients={}", options.allow.join(",")));
    logs.push(format!("capture={}", options.capture));
    logs.push(format!("debug_capture_log={}", options.debug_capture_log));
    logs.push(format!("demo_events={}", options.demo_events));
    logs.push(format!(
        "reverse_scroll={}",
        runtime_settings.reverse_scroll()
    ));
    logs.push(format!(
        "remote_scroll_scale={:.2}",
        runtime_settings.remote_scroll_scale()
    ));
    logs.push(format!("startup_reverse_scroll={}", options.reverse_scroll));
    logs.push(format!(
        "startup_remote_scroll_scale={:.2}",
        normalize_remote_scroll_scale(options.remote_scroll_scale)
    ));
    logs.push(format!("heartbeat_ms={}", options.heartbeat_ms));
    logs.push(format!(
        "clipboard enabled={} text={} image={} files={} poll_ms={} max_transfer_bytes={}",
        options.clipboard.enabled,
        options.clipboard.text,
        options.clipboard.image,
        options.clipboard.files,
        options.clipboard.poll_ms,
        options.clipboard.max_transfer_bytes
    ));
    let layout = runtime_settings.layout();
    logs.push(format!(
        "layout_revision={} screens={} links={}",
        runtime_settings.layout_revision(),
        layout.screens.len(),
        layout.links.len()
    ));
    logs.extend(platform_screen_debug_lines());

    let sessions = sessions.lock().await;
    if sessions.is_empty() {
        logs.push("active_sessions=none".to_string());
    } else {
        logs.push(format!("active_sessions={}", sessions.len()));
        for (screen, session) in sessions.iter() {
            logs.push(format!(
                "session: screen={screen} id={} peer={} version={} platform={} commit={} screen_size={} connected_at_ms={} uptime_ms={}",
                session.session_id,
                session.peer,
                session.client_version.as_deref().unwrap_or("unknown"),
                session.client_platform.as_deref().unwrap_or("unknown"),
                session.client_commit.as_deref().unwrap_or("unknown"),
                session
                    .screen_size
                    .map(|size| format!("{}x{}", size.width, size.height))
                    .unwrap_or_else(|| "unknown".to_string()),
                session.connected_at_ms,
                deskbridge_core::now_ms().saturating_sub(session.connected_at_ms)
            ));
        }
    }
    drop(sessions);

    logs.push("history:".to_string());
    logs.extend(server_log_snapshot(server_log));

    DebugResponse {
        request_id,
        ok: true,
        message: "server debug log read".to_string(),
        display: None,
        logs,
    }
}

fn update_runtime_input_settings(
    request_id: Uuid,
    reverse_scroll: Option<bool>,
    remote_scroll_scale: Option<f64>,
    layout: Option<Layout>,
    runtime_settings: &ServerRuntimeSettings,
    server_log: &ServerDebugLog,
) -> DebugResponse {
    let mut logs = Vec::new();
    let mut changed = false;
    let mut ok = true;

    if let Some(value) = reverse_scroll {
        let previous = runtime_settings.set_reverse_scroll(value);
        changed = previous != value;
        logs.push(format!("reverse_scroll: {previous} -> {value}"));
        push_server_log(
            server_log,
            format!("runtime input setting updated reverse_scroll={value} previous={previous}"),
        );
    }

    if let Some(value) = remote_scroll_scale {
        let (previous, current) = runtime_settings.set_remote_scroll_scale(value);
        changed |= (previous - current).abs() >= f64::EPSILON;
        logs.push(format!(
            "remote_scroll_scale: {previous:.2} -> {current:.2}"
        ));
        push_server_log(
            server_log,
            format!(
                "runtime input setting updated remote_scroll_scale={current:.2} previous={previous:.2}"
            ),
        );
    }

    if let Some(layout) = layout {
        match runtime_settings.set_layout(layout) {
            Ok(layout_changed) => {
                changed |= layout_changed;
                logs.push(format!(
                    "layout_revision={}",
                    runtime_settings.layout_revision()
                ));
                push_server_log(
                    server_log,
                    format!(
                        "runtime layout updated changed={} revision={}",
                        layout_changed,
                        runtime_settings.layout_revision()
                    ),
                );
            }
            Err(err) => {
                ok = false;
                logs.push(format!("layout update failed: {err}"));
            }
        }
    }

    logs.push(format!(
        "reverse_scroll={}",
        runtime_settings.reverse_scroll()
    ));
    logs.push(format!(
        "remote_scroll_scale={:.2}",
        runtime_settings.remote_scroll_scale()
    ));

    DebugResponse {
        request_id,
        ok,
        message: if !ok {
            "server input settings update failed".to_string()
        } else if changed {
            "server input settings updated".to_string()
        } else {
            "server input settings unchanged".to_string()
        },
        display: None,
        logs,
    }
}

async fn forward_route_debug_request(
    stream: &mut TcpStream,
    enc: &Encryption,
    target_screen: &str,
    sessions: SessionRegistry,
    request_id: Uuid,
    command: RouteDebugCommand,
) -> Result<()> {
    let Some(response) =
        request_route_debug_response(target_screen, &sessions, request_id, command).await
    else {
        send(
            stream,
            enc,
            &Message::Status(Status {
                kind: StatusKind::Error,
                message: format!("target client '{target_screen}' is not connected"),
            }),
        )
        .await?;
        return Ok(());
    };

    send(stream, enc, &Message::DebugResponse(response)).await?;
    Ok(())
}

async fn request_route_debug_response(
    target_screen: &str,
    sessions: &SessionRegistry,
    request_id: Uuid,
    command: RouteDebugCommand,
) -> Option<DebugResponse> {
    let route_debug_tx = route_debug_sender(target_screen, sessions).await?;
    let (response_tx, response_rx) = oneshot::channel();
    if route_debug_tx
        .send(RouteDebugEnvelope {
            request_id,
            command,
            response_tx,
        })
        .is_err()
    {
        return Some(debug_error_response(
            request_id,
            format!("target client '{target_screen}' is no longer available"),
        ));
    }

    Some(
        match time::timeout(Duration::from_secs(5), response_rx).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => debug_error_response(
                request_id,
                "route debug response channel closed".to_string(),
            ),
            Err(_) => debug_error_response(request_id, "route debug request timed out".to_string()),
        },
    )
}

async fn route_debug_sender(
    target_screen: &str,
    sessions: &SessionRegistry,
) -> Option<mpsc::UnboundedSender<RouteDebugEnvelope>> {
    let session_key = target_screen.to_ascii_lowercase();
    let sessions = sessions.lock().await;
    sessions
        .get(&session_key)
        .map(|session| session.route_debug_tx.clone())
}

fn build_route_probe_events(
    options: &ServerOptions,
    layout: &Layout,
    target_screen: &str,
    probe_options: RouteProbeOptions,
    request_id: Uuid,
) -> Result<(Vec<InputEvent>, Vec<String>)> {
    let edge = match probe_options.edge {
        Some(edge) => edge,
        None => layout
            .links
            .iter()
            .find(|link| link.from == options.name && link.to == target_screen)
            .map(|link| link.edge)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "layout has no link from '{}' to '{}'",
                    options.name,
                    target_screen
                )
            })?,
    };
    let (x, y) = sample_point_for_transition(layout, &options.name, target_screen, edge)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "layout does not include server screen '{}' for route probe",
                options.name
            )
        })?;

    let mut router = Some(InputRouter::new(layout.clone(), options.name.clone())?);
    let mut events = Vec::new();
    let mut logs = vec![format!(
        "route probe request={request_id} server={} target={target_screen} edge={edge:?} steps={} dx={} dy={}",
        options.name, probe_options.steps, probe_options.dx, probe_options.dy
    )];

    let first = route_capture_event(
        &mut router,
        CaptureEvent::LocalPointer { x, y },
        target_screen,
    )
    .ok_or_else(|| {
        anyhow::anyhow!(
            "layout has no transition from '{}' on edge {:?} to '{}'",
            options.name,
            edge,
            target_screen
        )
    })?;
    logs.push(format!(
        "event 0: target={target_screen} {}",
        describe_input_event(&first)
    ));
    events.push(first);

    for index in 1..=probe_options.steps {
        let event = route_capture_event(
            &mut router,
            CaptureEvent::Input(InputEvent::MouseMove {
                dx: probe_options.dx,
                dy: probe_options.dy,
            }),
            target_screen,
        )
        .ok_or_else(|| anyhow::anyhow!("remote screen stopped receiving routed probe input"))?;
        logs.push(format!(
            "event {index}: target={target_screen} {}",
            describe_input_event(&event)
        ));
        events.push(event);
    }

    Ok((events, logs))
}

fn build_capture_probe_events(
    options: &ServerOptions,
    layout: &Layout,
    target_screen: &str,
    probe_options: RouteProbeOptions,
    request_id: Uuid,
) -> Result<(Vec<CaptureEvent>, Vec<String>)> {
    let edge = match probe_options.edge {
        Some(edge) => edge,
        None => layout
            .links
            .iter()
            .find(|link| link.from == options.name && link.to == target_screen)
            .map(|link| link.edge)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "layout has no link from '{}' to '{}'",
                    options.name,
                    target_screen
                )
            })?,
    };
    let (x, y) = sample_point_for_transition(layout, &options.name, target_screen, edge)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "layout does not include server screen '{}' for capture probe",
                options.name
            )
        })?;

    let mut events = Vec::with_capacity(probe_options.steps as usize + 1);
    let mut logs = vec![
        format!(
            "capture probe request={request_id} server={} target={target_screen} edge={edge:?} steps={} dx={} dy={}",
            options.name, probe_options.steps, probe_options.dx, probe_options.dy
        ),
        format!("capture source 0: LocalPointer x={x} y={y}"),
    ];
    events.push(CaptureEvent::ProbeLocalPointer { request_id, x, y });

    for index in 1..=probe_options.steps {
        logs.push(format!(
            "capture source {index}: MouseMove dx={} dy={}",
            probe_options.dx, probe_options.dy
        ));
        events.push(CaptureEvent::ProbeInput {
            request_id,
            event: InputEvent::MouseMove {
                dx: probe_options.dx,
                dy: probe_options.dy,
            },
        });
    }

    Ok((events, logs))
}

fn capture_probe_id(event: &CaptureEvent) -> Option<Uuid> {
    match event {
        CaptureEvent::ProbeLocalPointer { request_id, .. }
        | CaptureEvent::ProbeInput { request_id, .. } => Some(*request_id),
        CaptureEvent::LocalPointer { .. } | CaptureEvent::Input(_) => None,
    }
}

fn capture_event_payload(event: CaptureEvent) -> CaptureEvent {
    match event {
        CaptureEvent::ProbeLocalPointer { x, y, .. } => CaptureEvent::LocalPointer { x, y },
        CaptureEvent::ProbeInput { event, .. } => CaptureEvent::Input(event),
        other => other,
    }
}

fn maybe_finish_capture_probe(
    request_id: Uuid,
    pending_capture_probes: &mut HashMap<Uuid, PendingCaptureProbe>,
    capture_probe_seq_index: &mut HashMap<u64, Uuid>,
) {
    let ready = pending_capture_probes
        .get(&request_id)
        .is_some_and(|probe| {
            probe.processed_capture_events >= probe.expected_capture_events
                && probe.remaining_seqs.is_empty()
        });
    if !ready {
        return;
    }

    let Some(probe) = pending_capture_probes.remove(&request_id) else {
        return;
    };
    for seq in &probe.remaining_seqs {
        capture_probe_seq_index.remove(seq);
    }

    let ok = probe.routed_events == probe.expected_capture_events;
    let message = if ok {
        format!(
            "capture probe delivered and acknowledged {} events through capture path",
            probe.routed_events
        )
    } else {
        format!(
            "capture probe routed {}/{} synthetic capture events",
            probe.routed_events, probe.expected_capture_events
        )
    };

    let _ = probe.response_tx.send(DebugResponse {
        request_id,
        ok,
        message,
        display: None,
        logs: probe.logs,
    });
}

fn build_route_status_logs(
    options: &ServerOptions,
    runtime_settings: &ServerRuntimeSettings,
    target_screen: &str,
    router: &Option<InputRouter>,
    request_id: Uuid,
    layout: &Layout,
) -> Vec<String> {
    let mut logs = vec![
        format!(
            "route status request={request_id} server={} target={target_screen}",
            options.name
        ),
        format!(
            "listen={} capture={} demo_events={} reverse_scroll={} remote_scroll_scale={:.2} heartbeat_ms={}",
            options.listen,
            options.capture,
            options.demo_events,
            runtime_settings.reverse_scroll(),
            runtime_settings.remote_scroll_scale(),
            options.heartbeat_ms,
        ),
        format!("layout_revision={}", runtime_settings.layout_revision()),
        format!(
            "active_route_screen={}",
            router
                .as_ref()
                .map(|router| router.active_screen().to_string())
                .unwrap_or_else(|| "unavailable".to_string())
        ),
    ];
    logs.extend(platform_screen_debug_lines());

    for screen in &layout.screens {
        logs.push(format!(
            "screen: {} {}x{} origin={}",
            screen.name,
            screen.size.width,
            screen.size.height,
            screen
                .origin
                .map(|origin| format!("{},{}", origin.x, origin.y))
                .unwrap_or_else(|| "unset".to_string())
        ));
    }

    let mut target_link_count = 0_u32;
    for link in layout.links.iter().filter(|link| link.from == options.name) {
        if link.to == target_screen {
            target_link_count += 1;
        }

        match sample_point_for_transition(layout, &link.from, &link.to, link.edge).and_then(
            |(x, y)| {
                layout
                    .transition(&link.from, link.edge, x, y)
                    .map(|transition| (x, y, transition))
            },
        ) {
            Some((x, y, transition)) => logs.push(format!(
                "link: {} {:?} -> {} source=({}, {}) target=({}, {}) return_edge={:?}",
                link.from,
                link.edge,
                link.to,
                x,
                y,
                transition.x,
                transition.y,
                transition.target_edge
            )),
            None => logs.push(format!(
                "link: {} {:?} -> {} not mappable",
                link.from, link.edge, link.to
            )),
        }
    }

    if target_link_count == 0 {
        logs.push(format!(
            "warning: no link from {} to target {}",
            options.name, target_screen
        ));
    }

    logs
}

fn platform_screen_debug_lines() -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        crate::capture::windows::screen_debug_lines()
    }

    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

async fn forward_debug_request(
    stream: &mut TcpStream,
    enc: &Encryption,
    target_screen: &str,
    sessions: SessionRegistry,
    request: DebugRequest,
) -> Result<()> {
    let session_key = target_screen.to_ascii_lowercase();
    let debug_tx = {
        let sessions = sessions.lock().await;
        sessions
            .get(&session_key)
            .map(|session| session.debug_tx.clone())
    };

    let Some(debug_tx) = debug_tx else {
        send(
            stream,
            enc,
            &Message::Status(Status {
                kind: StatusKind::Error,
                message: format!("target client '{target_screen}' is not connected"),
            }),
        )
        .await?;
        return Ok(());
    };

    let request_id = request.request_id;
    let (response_tx, response_rx) = oneshot::channel();
    if debug_tx
        .send(DebugEnvelope {
            request,
            response_tx,
        })
        .is_err()
    {
        send(
            stream,
            enc,
            &Message::Status(Status {
                kind: StatusKind::Error,
                message: format!("target client '{target_screen}' is no longer available"),
            }),
        )
        .await?;
        return Ok(());
    }

    match time::timeout(Duration::from_secs(5), response_rx).await {
        Ok(Ok(response)) => send(stream, enc, &Message::DebugResponse(response)).await?,
        Ok(Err(_)) => {
            send(
                stream,
                enc,
                &Message::DebugResponse(debug_error_response(
                    request_id,
                    "debug response channel closed".to_string(),
                )),
            )
            .await?;
        }
        Err(_) => {
            send(
                stream,
                enc,
                &Message::DebugResponse(debug_error_response(
                    request_id,
                    "debug request timed out".to_string(),
                )),
            )
            .await?;
        }
    }

    Ok(())
}

fn debug_error_response(request_id: Uuid, message: String) -> DebugResponse {
    DebugResponse {
        request_id,
        ok: false,
        message,
        display: None,
        logs: Vec::new(),
    }
}

async fn remove_current_session(sessions: &SessionRegistry, session_key: &str, session_id: Uuid) {
    let mut sessions = sessions.lock().await;
    if sessions
        .get(session_key)
        .is_some_and(|session| session.session_id == session_id)
    {
        sessions.remove(session_key);
    }
}

fn start_platform_capture(capture_tx: crate::capture::CaptureSender) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        crate::capture::windows::spawn(capture_tx)
    }

    #[cfg(target_os = "macos")]
    {
        crate::capture::macos::spawn(capture_tx)
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = capture_tx;
        anyhow::bail!("input capture is only implemented for Windows and macOS hosts");
    }
}

async fn validate_client(
    hello: &Hello,
    allow: &HashSet<String>,
    stream: &mut TcpStream,
    enc: &Encryption,
) -> Result<()> {
    if hello.protocol_version != deskbridge_core::PROTOCOL_VERSION {
        send(
            stream,
            enc,
            &Message::Status(Status {
                kind: StatusKind::Error,
                message: format!("unsupported protocol {}", hello.protocol_version),
            }),
        )
        .await?;
        anyhow::bail!("unsupported protocol {}", hello.protocol_version);
    }

    if !allow.is_empty() && !allow.contains(&hello.screen_name.to_ascii_lowercase()) {
        send(
            stream,
            enc,
            &Message::Status(Status {
                kind: StatusKind::Error,
                message: format!("screen '{}' is not allowed", hello.screen_name),
            }),
        )
        .await?;
        anyhow::bail!("screen '{}' is not allowed", hello.screen_name);
    }

    Ok(())
}

fn next_demo_event(
    router: &mut Option<InputRouter>,
    layout: &Layout,
    server_name: &str,
    client_name: &str,
    stage: u64,
) -> InputEvent {
    if stage == 0
        && let Some((x, y)) = demo_transition_point(layout, server_name, client_name)
        && let Some(event) =
            route_capture_event(router, CaptureEvent::LocalPointer { x, y }, client_name)
    {
        return event;
    }

    route_capture_event(
        router,
        CaptureEvent::Input(InputEvent::MouseMove { dx: 1, dy: 0 }),
        client_name,
    )
    .unwrap_or(InputEvent::MouseMove { dx: 1, dy: 0 })
}

fn route_capture_event(
    router: &mut Option<InputRouter>,
    event: CaptureEvent,
    client_name: &str,
) -> Option<InputEvent> {
    route_capture_event_for_client(router, event, client_name).event
}

/// Build the router used for live capture routing, applying the configured
/// edge anti-misfire guards (dwell delay and corner dead zone).
fn build_session_router(
    layout: Layout,
    local_screen: String,
    options: &ServerOptions,
) -> Result<InputRouter, deskbridge_core::LayoutError> {
    Ok(InputRouter::new(layout, local_screen)?
        .with_switch_delay_ms(options.edge_switch_delay_ms)
        .with_corner_size(options.edge_corner_size))
}

fn route_capture_event_for_client(
    router: &mut Option<InputRouter>,
    event: CaptureEvent,
    client_name: &str,
) -> RoutedCapture {
    let outcome = match event {
        CaptureEvent::LocalPointer { x, y } | CaptureEvent::ProbeLocalPointer { x, y, .. } => {
            let Some(router) = router.as_mut() else {
                return RoutedCapture::default();
            };
            let outcome = router.observe_local_pointer_outcome(x, y);
            if let Some(routed) = outcome.input.as_ref()
                && let InputEvent::MouseAbs {
                    x: target_x,
                    y: target_y,
                } = &routed.event
            {
                info!(
                    source_x = x,
                    source_y = y,
                    target = %routed.target_screen,
                    target_x = *target_x,
                    target_y = *target_y,
                    "activated remote screen from local pointer edge"
                );
            }
            outcome
        }
        CaptureEvent::Input(event) | CaptureEvent::ProbeInput { event, .. } => {
            let Some(router) = router.as_mut() else {
                return RoutedCapture::default();
            };
            router.route_if_remote_active_outcome(event)
        }
    };

    let released_to_local = outcome
        .portal
        .as_ref()
        .is_some_and(|portal| portal.source_screen == client_name && outcome.input.is_none());
    let event = outcome
        .input
        .and_then(|routed| (routed.target_screen == client_name).then_some(routed.event));

    RoutedCapture {
        event,
        released_to_local,
    }
}

fn transform_routed_input_event(
    mut event: InputEvent,
    reverse_scroll: bool,
    remote_scroll_scale: f64,
    scroll_accumulator: &mut ScrollScaleAccumulator,
) -> InputEvent {
    if let InputEvent::Wheel { dx, dy } = &mut event {
        let scaled_dx = scroll_accumulator.scale_x(*dx, remote_scroll_scale);
        let scaled_dy = scroll_accumulator.scale_y(*dy, remote_scroll_scale);
        *dx = if reverse_scroll {
            scaled_dx.saturating_neg()
        } else {
            scaled_dx
        };
        *dy = if reverse_scroll {
            scaled_dy.saturating_neg()
        } else {
            scaled_dy
        };
    }
    event
}

#[derive(Debug, Default)]
struct ScrollScaleAccumulator {
    x: f64,
    y: f64,
}

impl ScrollScaleAccumulator {
    fn scale_x(&mut self, value: i32, remote_scroll_scale: f64) -> i32 {
        scale_wheel_axis(value, remote_scroll_scale, &mut self.x)
    }

    fn scale_y(&mut self, value: i32, remote_scroll_scale: f64) -> i32 {
        scale_wheel_axis(value, remote_scroll_scale, &mut self.y)
    }
}

fn scale_wheel_axis(value: i32, remote_scroll_scale: f64, remainder: &mut f64) -> i32 {
    if value == 0 {
        return 0;
    }

    let scaled = value as f64 * normalize_remote_scroll_scale(remote_scroll_scale) + *remainder;
    let output = scaled.trunc().clamp(i32::MIN as f64, i32::MAX as f64) as i32;
    *remainder = scaled - output as f64;

    if output == i32::MIN || output == i32::MAX {
        *remainder = 0.0;
    }

    output
}

#[derive(Debug, Default)]
struct RemoteInputState {
    pressed_keys: HashSet<String>,
    pressed_buttons: Vec<Button>,
}

impl RemoteInputState {
    fn observe(&mut self, event: &InputEvent) {
        match event {
            InputEvent::Key { key, state } => {
                let key = remote_key_id(key);
                match state {
                    KeyState::Pressed => {
                        self.pressed_keys.insert(key);
                    }
                    KeyState::Released => {
                        self.pressed_keys.remove(&key);
                    }
                    KeyState::Clicked => {}
                }
            }
            InputEvent::MouseButton { button, state } => match state {
                KeyState::Pressed => {
                    if !self.pressed_buttons.contains(button) {
                        self.pressed_buttons.push(*button);
                    }
                }
                KeyState::Released => {
                    self.pressed_buttons.retain(|pressed| pressed != button);
                }
                KeyState::Clicked => {}
            },
            InputEvent::MouseMove { .. }
            | InputEvent::MouseAbs { .. }
            | InputEvent::Wheel { .. }
            | InputEvent::Text { .. } => {}
        }
    }

    fn release_events(&mut self) -> Vec<InputEvent> {
        let mut events = Vec::new();
        for button in self.pressed_buttons.drain(..) {
            events.push(InputEvent::MouseButton {
                button,
                state: KeyState::Released,
            });
        }

        let mut keys = self.pressed_keys.drain().collect::<Vec<_>>();
        keys.sort();
        for key in keys {
            events.push(InputEvent::Key {
                key,
                state: KeyState::Released,
            });
        }

        events
    }
}

fn remote_key_id(key: &str) -> String {
    key.trim().to_ascii_lowercase()
}

fn describe_input_event(event: &InputEvent) -> String {
    match event {
        InputEvent::MouseMove { dx, dy } => format!("MouseMove dx={dx} dy={dy}"),
        InputEvent::MouseAbs { x, y } => format!("MouseAbs x={x} y={y}"),
        InputEvent::MouseButton { button, state } => {
            format!("MouseButton button={button:?} state={state:?}")
        }
        InputEvent::Wheel { dx, dy } => format!("Wheel dx={dx} dy={dy}"),
        InputEvent::Key { key, state } => format!("Key key={key} state={state:?}"),
        InputEvent::Text { text } => format!("Text text={text:?}"),
    }
}

fn describe_capture_event(event: &CaptureEvent) -> String {
    match event {
        CaptureEvent::LocalPointer { x, y } => format!("LocalPointer x={x} y={y}"),
        CaptureEvent::Input(event) => describe_input_event(event),
        CaptureEvent::ProbeLocalPointer { x, y, request_id } => {
            format!("ProbeLocalPointer request={request_id} x={x} y={y}")
        }
        CaptureEvent::ProbeInput { event, request_id } => {
            format!(
                "ProbeInput request={request_id} {}",
                describe_input_event(event)
            )
        }
    }
}

fn demo_transition_point(
    layout: &Layout,
    server_name: &str,
    client_name: &str,
) -> Option<(u32, u32)> {
    let link = layout
        .links
        .iter()
        .find(|link| link.from == server_name && link.to == client_name)?;
    sample_point_for_transition(layout, server_name, client_name, link.edge)
}

fn sample_point_for_transition(
    layout: &Layout,
    from: &str,
    to: &str,
    edge: Edge,
) -> Option<(u32, u32)> {
    let source = layout.screens.iter().find(|screen| screen.name == from)?;
    let target = layout.screens.iter().find(|screen| screen.name == to)?;
    let Some(source_origin) = source.origin else {
        return sample_point_on_edge(layout, from, edge);
    };
    let Some(target_origin) = target.origin else {
        return sample_point_on_edge(layout, from, edge);
    };

    match edge {
        Edge::Left | Edge::Right => {
            let source_top = source_origin.y;
            let source_bottom = source_origin.y + source.size.height.saturating_sub(1) as i32;
            let target_top = target_origin.y;
            let target_bottom = target_origin.y + target.size.height.saturating_sub(1) as i32;
            let overlap_top = source_top.max(target_top);
            let overlap_bottom = source_bottom.min(target_bottom);
            if overlap_top <= overlap_bottom {
                let y = ((overlap_top + overlap_bottom) / 2 - source_top)
                    .clamp(0, source.size.height.saturating_sub(1) as i32)
                    as u32;
                let x = match edge {
                    Edge::Left => 0,
                    Edge::Right => source.size.width.saturating_sub(1),
                    Edge::Top | Edge::Bottom => unreachable!(),
                };
                return Some((x, y));
            }
        }
        Edge::Top | Edge::Bottom => {
            let source_left = source_origin.x;
            let source_right = source_origin.x + source.size.width.saturating_sub(1) as i32;
            let target_left = target_origin.x;
            let target_right = target_origin.x + target.size.width.saturating_sub(1) as i32;
            let overlap_left = source_left.max(target_left);
            let overlap_right = source_right.min(target_right);
            if overlap_left <= overlap_right {
                let x = ((overlap_left + overlap_right) / 2 - source_left)
                    .clamp(0, source.size.width.saturating_sub(1) as i32)
                    as u32;
                let y = match edge {
                    Edge::Top => 0,
                    Edge::Bottom => source.size.height.saturating_sub(1),
                    Edge::Left | Edge::Right => unreachable!(),
                };
                return Some((x, y));
            }
        }
    }

    sample_point_on_edge(layout, from, edge)
}

fn sample_point_on_edge(layout: &Layout, screen_name: &str, edge: Edge) -> Option<(u32, u32)> {
    let screen = layout
        .screens
        .iter()
        .find(|screen| screen.name == screen_name)?;
    let max_x = screen.size.width.saturating_sub(1);
    let max_y = screen.size.height.saturating_sub(1);
    let mid_x = screen.size.width / 2;
    let mid_y = screen.size.height / 2;

    Some(match edge {
        Edge::Left => (0, mid_y),
        Edge::Right => (max_x, mid_y),
        Edge::Top => (mid_x, 0),
        Edge::Bottom => (mid_x, max_y),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use deskbridge_core::{
        DEFAULT_REMOTE_SCROLL_SCALE, DebugCommand, DebugRequest, DebugResponse, DisplaySnapshot,
        EventAck, Link, Ping, Screen, Size, read_frame, write_frame,
    };

    fn test_layout() -> Layout {
        Layout {
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
                edge: Edge::Right,
                to: "mac".to_string(),
            }],
        }
    }

    fn test_options(listen: SocketAddr) -> ServerOptions {
        ServerOptions {
            listen,
            name: "windows".to_string(),
            allow: vec!["mac".to_string()],
            demo_events: false,
            capture: false,
            debug_capture_log: false,
            reverse_scroll: false,
            remote_scroll_scale: DEFAULT_REMOTE_SCROLL_SCALE,
            heartbeat_ms: DEFAULT_HEARTBEAT_MS,
            layout: test_layout(),
            clipboard: ClipboardConfig {
                enabled: false,
                ..ClipboardConfig::default()
            },
            edge_switch_delay_ms: 0,
            edge_corner_size: 0,
            psk: None,
        }
    }

    fn test_runtime_settings(options: &ServerOptions) -> ServerRuntimeSettings {
        RuntimeSettings::new(
            options.reverse_scroll,
            options.remote_scroll_scale,
            options.layout.clone(),
        )
    }

    fn test_shared(
        capture_tx: crate::capture::CaptureSender,
        sessions: SessionRegistry,
        runtime_settings: ServerRuntimeSettings,
        server_log: ServerDebugLog,
    ) -> ServerShared {
        ServerShared {
            capture_tx,
            sessions,
            runtime_settings,
            server_log,
        }
    }

    #[test]
    fn server_perf_metrics_report_ack_rtt_and_apply_time() {
        let mut perf = ServerPerfMetrics::new();
        let sent_at_ms = deskbridge_core::now_ms();
        perf.record_sent(7, crate::perf::EventKind::MouseMove, sent_at_ms, 12);
        perf.record_ack(
            &EventAck {
                seq: 7,
                received_at_ms: Some(sent_at_ms + 1),
                applied_at_ms: Some(sent_at_ms + 2),
                apply_duration_us: Some(345),
            },
            sent_at_ms + 3,
        );

        let logs = perf
            .logs(Uuid::nil(), "127.0.0.1:24800".parse().unwrap(), "mac")
            .join("\n");
        assert!(logs.contains("perf_scope=server_route_session"));
        assert!(logs.contains("sent=1 ack=1"));
        assert!(logs.contains("pending_ack=0"));
        assert!(logs.contains("mouse_move=1"));
        assert!(logs.contains("ack_rtt p50=3ms"));
        assert!(logs.contains("client_apply p50=345us"));
        assert!(logs.contains("server_write p50=12us"));
    }

    #[test]
    fn transform_routed_input_event_scales_remote_wheel() {
        let mut accumulator = ScrollScaleAccumulator::default();
        let event = transform_routed_input_event(
            InputEvent::Wheel { dx: 8, dy: -120 },
            false,
            0.5,
            &mut accumulator,
        );
        assert_eq!(event, InputEvent::Wheel { dx: 4, dy: -60 });

        let mut accumulator = ScrollScaleAccumulator::default();
        let first = transform_routed_input_event(
            InputEvent::Wheel { dx: 1, dy: 1 },
            false,
            0.5,
            &mut accumulator,
        );
        let second = transform_routed_input_event(
            InputEvent::Wheel { dx: 1, dy: 1 },
            false,
            0.5,
            &mut accumulator,
        );
        assert_eq!(first, InputEvent::Wheel { dx: 0, dy: 0 });
        assert_eq!(second, InputEvent::Wheel { dx: 1, dy: 1 });

        let mut accumulator = ScrollScaleAccumulator::default();
        let reversed = transform_routed_input_event(
            InputEvent::Wheel { dx: 4, dy: 4 },
            true,
            0.25,
            &mut accumulator,
        );
        assert_eq!(reversed, InputEvent::Wheel { dx: -1, dy: -1 });

        let mouse = transform_routed_input_event(
            InputEvent::MouseMove { dx: 8, dy: -8 },
            true,
            0.5,
            &mut accumulator,
        );
        assert_eq!(mouse, InputEvent::MouseMove { dx: 8, dy: -8 });
    }

    #[test]
    fn route_capture_marks_return_to_local_for_remote_release() {
        let options = test_options("127.0.0.1:0".parse().unwrap());
        let mut router = Some(InputRouter::new(options.layout, "windows").unwrap());

        let enter = route_capture_event_for_client(
            &mut router,
            CaptureEvent::LocalPointer { x: 1919, y: 540 },
            "mac",
        );
        assert!(enter.event.is_some());
        assert!(!enter.released_to_local);

        let key = route_capture_event_for_client(
            &mut router,
            CaptureEvent::Input(InputEvent::Key {
                key: "alt".to_string(),
                state: KeyState::Pressed,
            }),
            "mac",
        );
        assert_eq!(
            key.event,
            Some(InputEvent::Key {
                key: "alt".to_string(),
                state: KeyState::Pressed,
            })
        );

        let returned = route_capture_event_for_client(
            &mut router,
            CaptureEvent::Input(InputEvent::MouseMove { dx: -10, dy: 0 }),
            "mac",
        );
        assert_eq!(returned.event, None);
        assert!(returned.released_to_local);
    }

    #[test]
    fn remote_input_state_releases_pressed_keys_and_buttons() {
        let mut state = RemoteInputState::default();
        state.observe(&InputEvent::Key {
            key: "Alt".to_string(),
            state: KeyState::Pressed,
        });
        state.observe(&InputEvent::MouseButton {
            button: Button::Left,
            state: KeyState::Pressed,
        });

        assert_eq!(
            state.release_events(),
            vec![
                InputEvent::MouseButton {
                    button: Button::Left,
                    state: KeyState::Released,
                },
                InputEvent::Key {
                    key: "alt".to_string(),
                    state: KeyState::Released,
                },
            ]
        );
        assert!(state.release_events().is_empty());
    }

    #[tokio::test]
    async fn diagnostic_handshake_does_not_replace_input_client() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen = listener.local_addr().unwrap();
        let options = test_options(listen);
        let allow = HashSet::from(["mac".to_string()]);
        let (capture_tx, _) = crate::capture::channel();
        let sessions = SessionRegistry::default();
        let server_log = new_server_debug_log();
        let runtime_settings = test_runtime_settings(&options);
        let shared = test_shared(
            capture_tx.clone(),
            sessions.clone(),
            runtime_settings,
            server_log.clone(),
        );

        tokio::spawn({
            let options = options.clone();
            let allow = allow.clone();
            let shared = shared.clone();
            async move {
                for _ in 0..2 {
                    let (stream, peer) = listener.accept().await.unwrap();
                    tokio::spawn(handle_client(
                        stream,
                        peer,
                        options.clone(),
                        allow.clone(),
                        shared.clone(),
                    ));
                }
            }
        });

        let mut client = TcpStream::connect(listen).await.unwrap();
        write_frame(
            &mut client,
            &Message::Hello(Hello::client("mac").with_screen_size(Size {
                width: 1512,
                height: 982,
            })),
        )
        .await
        .unwrap();
        assert!(matches!(
            read_frame(&mut client).await.unwrap(),
            Message::Welcome(_)
        ));

        let mut diag = TcpStream::connect(listen).await.unwrap();
        write_frame(&mut diag, &Message::Hello(Hello::diagnostic("mac")))
            .await
            .unwrap();
        assert!(matches!(
            read_frame(&mut diag).await.unwrap(),
            Message::Welcome(_)
        ));
        drop(diag);

        write_frame(
            &mut client,
            &Message::Ping(Ping {
                seq: 7,
                sent_at_ms: deskbridge_core::now_ms(),
            }),
        )
        .await
        .unwrap();

        let response = tokio::time::timeout(Duration::from_secs(1), read_frame(&mut client))
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(response, Message::Pong(pong) if pong.seq == 7));
    }

    #[tokio::test]
    async fn diagnostic_debug_request_is_forwarded_to_input_client() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen = listener.local_addr().unwrap();
        let options = test_options(listen);
        let allow = HashSet::from(["mac".to_string()]);
        let (capture_tx, _) = crate::capture::channel();
        let sessions = SessionRegistry::default();
        let server_log = new_server_debug_log();
        let runtime_settings = test_runtime_settings(&options);
        let shared = test_shared(
            capture_tx.clone(),
            sessions.clone(),
            runtime_settings,
            server_log.clone(),
        );

        tokio::spawn({
            let options = options.clone();
            let allow = allow.clone();
            let shared = shared.clone();
            async move {
                for _ in 0..2 {
                    let (stream, peer) = listener.accept().await.unwrap();
                    tokio::spawn(handle_client(
                        stream,
                        peer,
                        options.clone(),
                        allow.clone(),
                        shared.clone(),
                    ));
                }
            }
        });

        let mut client = TcpStream::connect(listen).await.unwrap();
        write_frame(
            &mut client,
            &Message::Hello(Hello::client("mac").with_screen_size(Size {
                width: 1512,
                height: 982,
            })),
        )
        .await
        .unwrap();
        assert!(matches!(
            read_frame(&mut client).await.unwrap(),
            Message::Welcome(_)
        ));

        let mut diag = TcpStream::connect(listen).await.unwrap();
        write_frame(&mut diag, &Message::Hello(Hello::diagnostic("mac")))
            .await
            .unwrap();
        assert!(matches!(
            read_frame(&mut diag).await.unwrap(),
            Message::Welcome(_)
        ));

        let request_id = Uuid::new_v4();
        write_frame(
            &mut diag,
            &Message::DebugRequest(DebugRequest {
                request_id,
                command: DebugCommand::DisplayInfo,
            }),
        )
        .await
        .unwrap();

        let forwarded = tokio::time::timeout(Duration::from_secs(1), read_frame(&mut client))
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(forwarded, Message::DebugRequest(DebugRequest { request_id: id, command: DebugCommand::DisplayInfo }) if id == request_id)
        );

        write_frame(
            &mut client,
            &Message::DebugResponse(DebugResponse {
                request_id,
                ok: true,
                message: "display info read".to_string(),
                display: Some(DisplaySnapshot {
                    size: Size {
                        width: 1728,
                        height: 1117,
                    },
                    location: Some((10, 20)),
                }),
                logs: Vec::new(),
            }),
        )
        .await
        .unwrap();

        let response = tokio::time::timeout(Duration::from_secs(1), read_frame(&mut diag))
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            response,
            Message::DebugResponse(DebugResponse {
                request_id: id,
                ok: true,
                ..
            }) if id == request_id
        ));
    }

    #[tokio::test]
    async fn diagnostic_route_probe_sends_routed_events_to_input_client() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen = listener.local_addr().unwrap();
        let options = test_options(listen);
        let allow = HashSet::from(["mac".to_string()]);
        let (capture_tx, _) = crate::capture::channel();
        let sessions = SessionRegistry::default();
        let server_log = new_server_debug_log();
        let runtime_settings = test_runtime_settings(&options);
        let shared = test_shared(
            capture_tx.clone(),
            sessions.clone(),
            runtime_settings,
            server_log.clone(),
        );

        tokio::spawn({
            let options = options.clone();
            let allow = allow.clone();
            let shared = shared.clone();
            async move {
                for _ in 0..2 {
                    let (stream, peer) = listener.accept().await.unwrap();
                    tokio::spawn(handle_client(
                        stream,
                        peer,
                        options.clone(),
                        allow.clone(),
                        shared.clone(),
                    ));
                }
            }
        });

        let mut client = TcpStream::connect(listen).await.unwrap();
        write_frame(
            &mut client,
            &Message::Hello(Hello::client("mac").with_screen_size(Size {
                width: 1512,
                height: 982,
            })),
        )
        .await
        .unwrap();
        assert!(matches!(
            read_frame(&mut client).await.unwrap(),
            Message::Welcome(_)
        ));

        let mut diag = TcpStream::connect(listen).await.unwrap();
        write_frame(&mut diag, &Message::Hello(Hello::diagnostic("mac")))
            .await
            .unwrap();
        assert!(matches!(
            read_frame(&mut diag).await.unwrap(),
            Message::Welcome(_)
        ));

        let request_id = Uuid::new_v4();
        write_frame(
            &mut diag,
            &Message::DebugRequest(DebugRequest {
                request_id,
                command: DebugCommand::RouteProbe {
                    edge: Some(Edge::Right),
                    steps: 2,
                    dx: 40,
                    dy: -1,
                },
            }),
        )
        .await
        .unwrap();

        let expected_events = [
            InputEvent::MouseAbs { x: 1, y: 491 },
            InputEvent::MouseMove { dx: 40, dy: -1 },
            InputEvent::MouseMove { dx: 40, dy: -1 },
        ];
        for expected in expected_events {
            let packet = match tokio::time::timeout(Duration::from_secs(1), read_frame(&mut client))
                .await
                .unwrap()
                .unwrap()
            {
                Message::Input(packet) => packet,
                other => panic!("expected route probe input, got {other:?}"),
            };
            assert_eq!(packet.event, expected);
            write_frame(&mut client, &Message::Ack(EventAck::new(packet.seq)))
                .await
                .unwrap();
        }

        let response = match tokio::time::timeout(Duration::from_secs(1), read_frame(&mut diag))
            .await
            .unwrap()
            .unwrap()
        {
            Message::DebugResponse(response) => response,
            other => panic!("expected route probe debug response, got {other:?}"),
        };
        assert_eq!(response.request_id, request_id);
        assert!(response.ok);
        assert!(response.logs.iter().any(|line| line.contains("event 0")));
        assert!(response.logs.iter().any(|line| line == "ack seq=3"));
    }

    #[tokio::test]
    async fn diagnostic_capture_probe_enters_capture_routing_path() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen = listener.local_addr().unwrap();
        let options = test_options(listen);
        let allow = HashSet::from(["mac".to_string()]);
        let (capture_tx, _) = crate::capture::channel();
        let sessions = SessionRegistry::default();
        let server_log = new_server_debug_log();
        let runtime_settings = test_runtime_settings(&options);
        let shared = test_shared(
            capture_tx.clone(),
            sessions.clone(),
            runtime_settings,
            server_log.clone(),
        );

        tokio::spawn({
            let options = options.clone();
            let allow = allow.clone();
            let shared = shared.clone();
            async move {
                for _ in 0..2 {
                    let (stream, peer) = listener.accept().await.unwrap();
                    tokio::spawn(handle_client(
                        stream,
                        peer,
                        options.clone(),
                        allow.clone(),
                        shared.clone(),
                    ));
                }
            }
        });

        let mut client = TcpStream::connect(listen).await.unwrap();
        write_frame(
            &mut client,
            &Message::Hello(Hello::client("mac").with_screen_size(Size {
                width: 1512,
                height: 982,
            })),
        )
        .await
        .unwrap();
        assert!(matches!(
            read_frame(&mut client).await.unwrap(),
            Message::Welcome(_)
        ));

        let mut diag = TcpStream::connect(listen).await.unwrap();
        write_frame(&mut diag, &Message::Hello(Hello::diagnostic("mac")))
            .await
            .unwrap();
        assert!(matches!(
            read_frame(&mut diag).await.unwrap(),
            Message::Welcome(_)
        ));

        let request_id = Uuid::new_v4();
        write_frame(
            &mut diag,
            &Message::DebugRequest(DebugRequest {
                request_id,
                command: DebugCommand::CaptureProbe {
                    edge: Some(Edge::Right),
                    steps: 2,
                    dx: 40,
                    dy: -1,
                },
            }),
        )
        .await
        .unwrap();

        let expected_events = [
            InputEvent::MouseAbs { x: 1, y: 491 },
            InputEvent::MouseMove { dx: 40, dy: -1 },
            InputEvent::MouseMove { dx: 40, dy: -1 },
        ];
        for expected in expected_events {
            let packet = match tokio::time::timeout(Duration::from_secs(1), read_frame(&mut client))
                .await
                .unwrap()
                .unwrap()
            {
                Message::Input(packet) => packet,
                other => panic!("expected capture probe input, got {other:?}"),
            };
            assert_eq!(packet.event, expected);
            write_frame(&mut client, &Message::Ack(EventAck::new(packet.seq)))
                .await
                .unwrap();
        }

        let response = match tokio::time::timeout(Duration::from_secs(1), read_frame(&mut diag))
            .await
            .unwrap()
            .unwrap()
        {
            Message::DebugResponse(response) => response,
            other => panic!("expected capture probe debug response, got {other:?}"),
        };
        assert_eq!(response.request_id, request_id);
        assert!(response.ok);
        assert!(
            response
                .message
                .contains("capture probe delivered and acknowledged 3 events")
        );
        assert!(
            response.logs.iter().any(|line| {
                line.contains("capture event 1 routed target=mac MouseAbs x=1 y=491")
            })
        );
        assert!(response.logs.iter().any(|line| line == "ack seq=3"));
    }

    #[tokio::test]
    async fn diagnostic_route_status_reports_effective_client_layout() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen = listener.local_addr().unwrap();
        let options = test_options(listen);
        let allow = HashSet::from(["mac".to_string()]);
        let (capture_tx, _) = crate::capture::channel();
        let sessions = SessionRegistry::default();
        let server_log = new_server_debug_log();
        let runtime_settings = test_runtime_settings(&options);
        let shared = test_shared(
            capture_tx.clone(),
            sessions.clone(),
            runtime_settings,
            server_log.clone(),
        );

        tokio::spawn({
            let options = options.clone();
            let allow = allow.clone();
            let shared = shared.clone();
            async move {
                for _ in 0..2 {
                    let (stream, peer) = listener.accept().await.unwrap();
                    tokio::spawn(handle_client(
                        stream,
                        peer,
                        options.clone(),
                        allow.clone(),
                        shared.clone(),
                    ));
                }
            }
        });

        let mut client = TcpStream::connect(listen).await.unwrap();
        write_frame(
            &mut client,
            &Message::Hello(Hello::client("mac").with_screen_size(Size {
                width: 1512,
                height: 982,
            })),
        )
        .await
        .unwrap();
        assert!(matches!(
            read_frame(&mut client).await.unwrap(),
            Message::Welcome(_)
        ));

        let mut diag = TcpStream::connect(listen).await.unwrap();
        write_frame(&mut diag, &Message::Hello(Hello::diagnostic("mac")))
            .await
            .unwrap();
        assert!(matches!(
            read_frame(&mut diag).await.unwrap(),
            Message::Welcome(_)
        ));

        let request_id = Uuid::new_v4();
        write_frame(
            &mut diag,
            &Message::DebugRequest(DebugRequest {
                request_id,
                command: DebugCommand::RouteStatus,
            }),
        )
        .await
        .unwrap();

        let response = match tokio::time::timeout(Duration::from_secs(1), read_frame(&mut diag))
            .await
            .unwrap()
            .unwrap()
        {
            Message::DebugResponse(response) => response,
            other => panic!("expected route status debug response, got {other:?}"),
        };
        assert_eq!(response.request_id, request_id);
        assert!(response.ok);
        assert!(
            response
                .logs
                .iter()
                .any(|line| line == "screen: mac 1512x982 origin=unset")
        );
        assert!(response.logs.iter().any(|line| {
            line == "link: windows Right -> mac source=(1919, 540) target=(1, 491) return_edge=Left"
        }));
    }

    #[tokio::test]
    async fn diagnostic_server_logs_are_available_without_target_client() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen = listener.local_addr().unwrap();
        let options = test_options(listen);
        let allow = HashSet::from(["mac".to_string()]);
        let (capture_tx, _) = crate::capture::channel();
        let sessions = SessionRegistry::default();
        let server_log = new_server_debug_log();
        let runtime_settings = test_runtime_settings(&options);
        let shared = test_shared(capture_tx, sessions, runtime_settings, server_log.clone());
        push_server_log(&server_log, "test history entry");

        tokio::spawn({
            let shared = shared.clone();
            async move {
                let (stream, peer) = listener.accept().await.unwrap();
                handle_client(stream, peer, options, allow, shared)
                    .await
                    .unwrap();
            }
        });

        let mut diag = TcpStream::connect(listen).await.unwrap();
        write_frame(&mut diag, &Message::Hello(Hello::diagnostic("mac")))
            .await
            .unwrap();
        assert!(matches!(
            read_frame(&mut diag).await.unwrap(),
            Message::Welcome(_)
        ));

        let request_id = Uuid::new_v4();
        write_frame(
            &mut diag,
            &Message::DebugRequest(DebugRequest {
                request_id,
                command: DebugCommand::ServerLogs,
            }),
        )
        .await
        .unwrap();

        let response = match tokio::time::timeout(Duration::from_secs(1), read_frame(&mut diag))
            .await
            .unwrap()
            .unwrap()
        {
            Message::DebugResponse(response) => response,
            other => panic!("expected server logs debug response, got {other:?}"),
        };
        assert_eq!(response.request_id, request_id);
        assert!(response.ok);
        assert!(response.logs.iter().any(|line| line == "role=server"));
        assert!(
            response
                .logs
                .iter()
                .any(|line| line == "active_sessions=none")
        );
        assert!(
            response
                .logs
                .iter()
                .any(|line| line.contains("test history entry"))
        );
    }

    #[tokio::test]
    async fn diagnostic_input_settings_update_changes_runtime_state() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen = listener.local_addr().unwrap();
        let options = test_options(listen);
        let allow = HashSet::from(["mac".to_string()]);
        let (capture_tx, _) = crate::capture::channel();
        let sessions = SessionRegistry::default();
        let server_log = new_server_debug_log();
        let runtime_settings = test_runtime_settings(&options);
        let shared = test_shared(
            capture_tx,
            sessions,
            runtime_settings.clone(),
            server_log.clone(),
        );

        tokio::spawn({
            let shared = shared.clone();
            async move {
                let (stream, peer) = listener.accept().await.unwrap();
                handle_client(stream, peer, options, allow, shared)
                    .await
                    .unwrap();
            }
        });

        let mut diag = TcpStream::connect(listen).await.unwrap();
        write_frame(&mut diag, &Message::Hello(Hello::diagnostic("mac")))
            .await
            .unwrap();
        assert!(matches!(
            read_frame(&mut diag).await.unwrap(),
            Message::Welcome(_)
        ));

        let request_id = Uuid::new_v4();
        write_frame(
            &mut diag,
            &Message::DebugRequest(DebugRequest {
                request_id,
                command: DebugCommand::InputSettings {
                    reverse_scroll: Some(true),
                    remote_scroll_scale: Some(0.5),
                    layout: None,
                    reset_route: None,
                },
            }),
        )
        .await
        .unwrap();

        let response = match tokio::time::timeout(Duration::from_secs(1), read_frame(&mut diag))
            .await
            .unwrap()
            .unwrap()
        {
            Message::DebugResponse(response) => response,
            other => panic!("expected input settings debug response, got {other:?}"),
        };
        assert_eq!(response.request_id, request_id);
        assert!(response.ok);
        assert!(runtime_settings.reverse_scroll());
        assert_eq!(runtime_settings.remote_scroll_scale(), 0.5);
        assert!(
            response
                .logs
                .iter()
                .any(|line| line == "reverse_scroll: false -> true")
        );
        assert!(
            response
                .logs
                .iter()
                .any(|line| { line == "remote_scroll_scale: 1.00 -> 0.50" })
        );
        assert!(server_log_snapshot(&server_log).iter().any(|line| {
            line.contains("runtime input setting updated reverse_scroll=true previous=false")
        }));
        assert!(server_log_snapshot(&server_log).iter().any(|line| {
            line.contains("runtime input setting updated remote_scroll_scale=0.50 previous=1.00")
        }));
    }

    #[tokio::test]
    async fn ended_client_session_is_removed_from_registry() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen = listener.local_addr().unwrap();
        let options = test_options(listen);
        let allow = HashSet::from(["mac".to_string()]);
        let (capture_tx, _) = crate::capture::channel();
        let sessions = SessionRegistry::default();
        let server_log = new_server_debug_log();
        let runtime_settings = test_runtime_settings(&options);
        let shared = test_shared(
            capture_tx,
            sessions.clone(),
            runtime_settings,
            server_log.clone(),
        );

        let server_task = tokio::spawn({
            let shared = shared.clone();
            async move {
                let (stream, peer) = listener.accept().await.unwrap();
                handle_client(stream, peer, options, allow, shared)
                    .await
                    .unwrap();
            }
        });

        let mut client = TcpStream::connect(listen).await.unwrap();
        write_frame(&mut client, &Message::Hello(Hello::client("mac")))
            .await
            .unwrap();
        assert!(matches!(
            read_frame(&mut client).await.unwrap(),
            Message::Welcome(_)
        ));
        write_frame(
            &mut client,
            &Message::Goodbye {
                reason: "test complete".to_string(),
            },
        )
        .await
        .unwrap();

        tokio::time::timeout(Duration::from_secs(1), server_task)
            .await
            .unwrap()
            .unwrap();
        assert!(sessions.lock().await.is_empty());
    }

    #[test]
    fn client_reported_screen_size_updates_routing_layout() {
        let listen = "127.0.0.1:0".parse().unwrap();
        let mut options = test_options(listen);
        let hello = Hello::client("mac").with_screen_size(Size {
            width: 1512,
            height: 982,
        });

        apply_client_screen_size(&mut options, &hello);

        let mac = options
            .layout
            .screens
            .iter()
            .find(|screen| screen.name == "mac")
            .unwrap();
        assert_eq!(
            mac.size,
            Size {
                width: 1512,
                height: 982,
            }
        );
    }
}
