use crate::input::{EnigoSink, InputSink, LogSink};
use anyhow::{Context, Result, anyhow};
use deskbridge_core::{
    CLIPBOARD_PROTOCOL_VERSION, Capability, ClipboardConfig, ClipboardPacket, DEFAULT_HEARTBEAT_MS,
    DebugCommand, DebugRequest, DebugResponse, DisplaySnapshot, EventAck, FrameError, Hello,
    InputEvent, InputPacket, Message, Ping, Pong, REPLACED_SESSION_REASON, read_frame, write_frame,
};
use std::collections::VecDeque;
use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};
use tokio::{net::TcpStream, sync::mpsc, time};
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub server: SocketAddr,
    pub name: String,
    pub dry_run: bool,
    pub reconnect: bool,
    pub reverse_scroll: bool,
    pub reconnect_max_ms: u64,
    pub stale_after_ms: u64,
    pub max_events: Option<u64>,
    pub clipboard: ClipboardConfig,
}

pub async fn run(options: ClientOptions) -> Result<()> {
    let mut attempt = 0_u32;

    loop {
        attempt += 1;
        match connect_once(&options).await {
            Ok(ClientSessionOutcome::Ended) => {
                info!("client session ended");
                break;
            }
            Ok(ClientSessionOutcome::Replaced) => {
                info!("client session was replaced by a newer local session; stopping");
                break;
            }
            Err(err) => warn!(attempt, error = %err, "client session failed"),
        }

        if !options.reconnect {
            break;
        }

        let max_backoff = options.reconnect_max_ms.max(500);
        let backoff = Duration::from_millis(
            (500_u64 * 2_u64.saturating_pow(attempt.min(5))).min(max_backoff),
        );
        time::sleep(backoff).await;
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientSessionOutcome {
    Ended,
    Replaced,
}

async fn connect_once(options: &ClientOptions) -> Result<ClientSessionOutcome> {
    info!(server = %options.server, screen = options.name, "connecting");
    let mut stream = TcpStream::connect(options.server)
        .await
        .with_context(|| format!("failed to connect {}", options.server))?;
    stream.set_nodelay(true)?;

    let hello = client_hello(options);
    write_frame(&mut stream, &Message::Hello(hello)).await?;

    let welcome = read_frame(&mut stream).await?;
    let (heartbeat_ms, server_clipboard_supported) = match welcome {
        Message::Welcome(welcome) => {
            let server_clipboard_supported = welcome.capabilities.contains(&Capability::Clipboard)
                && welcome.clipboard_protocol == Some(CLIPBOARD_PROTOCOL_VERSION);
            info!(
                server = welcome.server_name,
                session = %welcome.session_id,
                "connected"
            );
            (welcome.heartbeat_interval_ms, server_clipboard_supported)
        }
        Message::Status(status) => {
            return Err(anyhow!("server rejected client: {}", status.message));
        }
        other => return Err(anyhow!("expected welcome, got {other:?}")),
    };

    let mut sink: Box<dyn InputSink> = if options.dry_run {
        Box::new(LogSink)
    } else {
        Box::new(EnigoSink::new()?)
    };
    let mut clipboard_options = options.clipboard.clone();
    if !server_clipboard_supported {
        clipboard_options.enabled = false;
    }
    let clipboard_runtime = crate::clipboard::ClipboardRuntime::new(clipboard_options);
    let mut clipboard_rx = clipboard_runtime
        .as_ref()
        .map(crate::clipboard::ClipboardRuntime::spawn_watcher);
    let (reader, mut writer) = stream.into_split();
    let mut inbound = spawn_reader(reader);

    let heartbeat = Duration::from_millis(heartbeat_ms.max(DEFAULT_HEARTBEAT_MS));
    let stale_after = Duration::from_millis(
        options
            .stale_after_ms
            .max(heartbeat.as_millis().saturating_mul(2) as u64),
    );
    let mut ticker = time::interval(heartbeat);
    let mut stale_check = time::interval(Duration::from_millis(
        (heartbeat.as_millis() as u64 / 2).clamp(250, 2_000),
    ));
    let mut seq = 0_u64;
    let mut received_events = 0_u64;
    let mut last_rx = Instant::now();
    let mut debug_state = ClientDebugState::new(options);
    debug_state.push(format!("connected to server {}", options.server));
    if options.clipboard.enabled && !server_clipboard_supported {
        debug_state
            .push("clipboard disabled: server did not negotiate clipboard protocol".to_string());
    }

    let session_result: Result<ClientSessionOutcome> = async {
        loop {
        tokio::select! {
            _ = ticker.tick() => {
                seq += 1;
                let ping = Message::Ping(Ping {
                    seq,
                    sent_at_ms: deskbridge_core::now_ms(),
                });
                debug!(seq, "sending heartbeat");
                write_frame(&mut writer, &ping).await?;
            }
            _ = stale_check.tick() => {
                let silent_for = last_rx.elapsed();
                if silent_for > stale_after {
                    anyhow::bail!(
                        "server heartbeat stale: no inbound frame for {}ms; reconnecting",
                        silent_for.as_millis()
                    );
                }
            }
            packet = recv_clipboard(&mut clipboard_rx), if clipboard_rx.is_some() => {
                if let Some(packet) = packet {
                    let summary = crate::clipboard::content_summary(&packet.content);
                    write_frame(&mut writer, &Message::Clipboard(packet)).await?;
                    debug_state.push(format!("sent clipboard {summary}"));
                }
            }
            msg = inbound.recv() => {
                let message = msg
                    .ok_or_else(|| anyhow!("server reader stopped"))??;
                last_rx = Instant::now();
                match message {
                    Message::Ping(ping) => {
                        write_frame(&mut writer, &Message::Pong(Pong {
                            seq: ping.seq,
                            sent_at_ms: ping.sent_at_ms,
                        })).await?;
                    }
                    Message::Pong(pong) => {
                        debug!(seq = pong.seq, "heartbeat acknowledged");
                    }
                    Message::Input(mut packet) => {
                        let received_at_ms = deskbridge_core::now_ms();
                        if options.reverse_scroll {
                            reverse_scroll_event(&mut packet.event);
                        }
                        let apply_started = Instant::now();
                        sink.apply(&packet).await?;
                        let apply_duration_us = apply_started.elapsed().as_micros();
                        let applied_at_ms = deskbridge_core::now_ms();
                        write_frame(&mut writer, &Message::Ack(EventAck {
                            seq: packet.seq,
                            received_at_ms: Some(received_at_ms),
                            applied_at_ms: Some(applied_at_ms),
                            apply_duration_us: Some(apply_duration_us),
                        })).await?;
                        received_events += 1;
                        debug_state.record_input(&packet.event, apply_duration_us, applied_at_ms);
                        if options.max_events.is_some_and(|max_events| received_events >= max_events) {
                            break Ok(ClientSessionOutcome::Ended);
                        }
                    }
                    Message::Clipboard(packet) => {
                        if let Some(runtime) = &clipboard_runtime {
                            match runtime.apply_remote(packet).await {
                                Ok(summary) => debug_state.push(format!("applied remote clipboard {summary}")),
                                Err(err) => {
                                    debug_state.push(format!("remote clipboard apply failed: {err:#}"));
                                    warn!(error = %err, "remote clipboard apply failed");
                                }
                            }
                        }
                    }
                    Message::PortalFlash(_) => {}
                    Message::DebugRequest(request) => {
                        let response = handle_debug_request(request, sink.as_mut(), &mut debug_state).await;
                        write_frame(&mut writer, &Message::DebugResponse(response)).await?;
                    }
                    Message::Status(status) => {
                        warn!(kind = ?status.kind, message = status.message, "server status");
                    }
                    Message::Goodbye { reason } => {
                        if reason == REPLACED_SESSION_REASON {
                            break Ok(ClientSessionOutcome::Replaced);
                        }
                        break Err(anyhow!("server closed session: {reason}"));
                    }
                    other => debug!(message = ?other, "ignored message"),
                }
            }
        }
    }
    }.await;

    if let Err(err) = sink.release_all().await {
        warn!(error = %err, "failed to release pressed inputs after client session");
    }

    session_result
}

async fn recv_clipboard(
    rx: &mut Option<mpsc::UnboundedReceiver<ClipboardPacket>>,
) -> Option<ClipboardPacket> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

fn reverse_scroll_event(event: &mut InputEvent) {
    if let InputEvent::Wheel { dx, dy } = event {
        *dx = dx.saturating_neg();
        *dy = dy.saturating_neg();
    }
}

fn spawn_reader<R>(mut reader: R) -> mpsc::UnboundedReceiver<Result<Message, FrameError>>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let result = read_frame(&mut reader).await;
            let should_stop = result.is_err();
            if tx.send(result).is_err() || should_stop {
                break;
            }
        }
    });
    rx
}

#[derive(Debug)]
struct ClientDebugState {
    logs: VecDeque<String>,
    server: SocketAddr,
    name: String,
    dry_run: bool,
    started_at_ms: u128,
    perf: ClientPerfMetrics,
}

impl ClientDebugState {
    fn new(options: &ClientOptions) -> Self {
        Self {
            logs: VecDeque::with_capacity(64),
            server: options.server,
            name: options.name.clone(),
            dry_run: options.dry_run,
            started_at_ms: deskbridge_core::now_ms(),
            perf: ClientPerfMetrics::new(),
        }
    }

    fn push(&mut self, line: String) {
        if self.logs.len() == 64 {
            self.logs.pop_front();
        }
        self.logs
            .push_back(format!("{} {line}", deskbridge_core::now_ms()));
    }

    fn recent_logs(&self) -> Vec<String> {
        self.logs.iter().cloned().collect()
    }

    fn peer_info_logs(&self) -> Vec<String> {
        let mut logs = crate::build_info::lines();
        logs.push("role=client".to_string());
        logs.push(format!("screen={}", self.name));
        logs.push(format!("server={}", self.server));
        logs.push(format!("dry_run={}", self.dry_run));
        logs.push(format!("started_at_ms={}", self.started_at_ms));
        logs.push(format!(
            "uptime_ms={}",
            deskbridge_core::now_ms().saturating_sub(self.started_at_ms)
        ));
        match std::env::current_exe() {
            Ok(path) => logs.push(format!("process={}", path.display())),
            Err(err) => logs.push(format!("process=unavailable ({err})")),
        }
        logs
    }

    fn record_input(&mut self, event: &InputEvent, apply_duration_us: u128, at_ms: u128) {
        self.perf.record_input(event, apply_duration_us, at_ms);
    }

    fn perf_logs(&mut self) -> Vec<String> {
        self.perf.logs(self.started_at_ms)
    }
}

#[derive(Debug)]
struct ClientPerfSample {
    at_ms: u128,
    apply_duration_us: u128,
}

#[derive(Debug)]
struct ClientPerfMetrics {
    total_events: u64,
    mouse_move_events: u64,
    mouse_abs_events: u64,
    button_events: u64,
    wheel_events: u64,
    key_events: u64,
    text_events: u64,
    samples: VecDeque<ClientPerfSample>,
}

impl ClientPerfMetrics {
    fn new() -> Self {
        Self {
            total_events: 0,
            mouse_move_events: 0,
            mouse_abs_events: 0,
            button_events: 0,
            wheel_events: 0,
            key_events: 0,
            text_events: 0,
            samples: VecDeque::with_capacity(512),
        }
    }

    fn record_input(&mut self, event: &InputEvent, apply_duration_us: u128, at_ms: u128) {
        self.total_events += 1;
        match crate::perf::event_kind(event) {
            crate::perf::EventKind::MouseMove => self.mouse_move_events += 1,
            crate::perf::EventKind::MouseAbs => self.mouse_abs_events += 1,
            crate::perf::EventKind::MouseButton => self.button_events += 1,
            crate::perf::EventKind::Wheel => self.wheel_events += 1,
            crate::perf::EventKind::Key => self.key_events += 1,
            crate::perf::EventKind::Text => self.text_events += 1,
        }
        self.samples.push_back(ClientPerfSample {
            at_ms,
            apply_duration_us,
        });
        self.trim(at_ms);
    }

    fn logs(&mut self, started_at_ms: u128) -> Vec<String> {
        let now = deskbridge_core::now_ms();
        self.trim(now);
        let mut apply_values = self
            .samples
            .iter()
            .map(|sample| sample.apply_duration_us)
            .collect::<Vec<_>>();
        let p50 = crate::perf::percentile(&mut apply_values.clone(), 50);
        let p95 = crate::perf::percentile(&mut apply_values.clone(), 95);
        let p99 = crate::perf::percentile(&mut apply_values, 99);
        let window_ms = crate::perf::PERF_WINDOW_MS.min(now.saturating_sub(started_at_ms));

        vec![
            "perf_scope=client_apply".to_string(),
            format!("window_ms={window_ms}"),
            format!("uptime_ms={}", now.saturating_sub(started_at_ms)),
            format!("events_total={}", self.total_events),
            format!("events_window={}", self.samples.len()),
            format!(
                "events_window_hz={:.1}",
                crate::perf::rate_per_second(self.samples.len(), window_ms.max(1))
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
                "apply_duration p50={} p95={} p99={}",
                crate::perf::format_us(p50),
                crate::perf::format_us(p95),
                crate::perf::format_us(p99)
            ),
        ]
    }

    fn trim(&mut self, now_ms: u128) {
        let cutoff = now_ms.saturating_sub(crate::perf::PERF_WINDOW_MS);
        while self
            .samples
            .front()
            .is_some_and(|sample| sample.at_ms < cutoff)
        {
            self.samples.pop_front();
        }
    }
}

async fn handle_debug_request(
    request: DebugRequest,
    sink: &mut dyn InputSink,
    debug_state: &mut ClientDebugState,
) -> DebugResponse {
    debug_state.push(format!("debug request {:?}", request.command));
    let response = match request.command {
        DebugCommand::DisplayInfo => match crate::input::display_info() {
            Ok(info) => DebugResponse {
                request_id: request.request_id,
                ok: true,
                message: "display info read".to_string(),
                display: Some(DisplaySnapshot {
                    size: info.size,
                    location: info.location,
                }),
                logs: Vec::new(),
            },
            Err(err) => debug_response_error(request.request_id, format!("{err:#}")),
        },
        DebugCommand::PeerInfo => DebugResponse {
            request_id: request.request_id,
            ok: true,
            message: "client peer info read".to_string(),
            display: None,
            logs: debug_state.peer_info_logs(),
        },
        DebugCommand::RecentLogs => DebugResponse {
            request_id: request.request_id,
            ok: true,
            message: "recent client debug log".to_string(),
            display: None,
            logs: debug_state.recent_logs(),
        },
        DebugCommand::Perf => DebugResponse {
            request_id: request.request_id,
            ok: true,
            message: "client perf metrics read".to_string(),
            display: None,
            logs: debug_state.perf_logs(),
        },
        DebugCommand::MoveMouse { x, y, dx, dy } => apply_debug_mouse_move(sink, x, y, dx, dy)
            .await
            .unwrap_or_else(|err| debug_response_error(request.request_id, format!("{err:#}")))
            .with_request_id(request.request_id),
        DebugCommand::RouteProbe { .. }
        | DebugCommand::RouteStatus
        | DebugCommand::InputSettings { .. }
        | DebugCommand::CaptureProbe { .. }
        | DebugCommand::ServerLogs => debug_response_error(
            request.request_id,
            "server-side debug commands are handled by the server, not the target client"
                .to_string(),
        ),
    };
    debug_state.push(format!("debug response ok={}", response.ok));
    response
}

async fn apply_debug_mouse_move(
    sink: &mut dyn InputSink,
    x: Option<i32>,
    y: Option<i32>,
    dx: i32,
    dy: i32,
) -> Result<DebugResponse> {
    match (x, y) {
        (Some(x), Some(y)) => {
            sink.apply(&InputPacket {
                seq: 0,
                event: InputEvent::MouseAbs { x, y },
            })
            .await?;
        }
        (None, None) => {}
        _ => anyhow::bail!("x and y must be provided together"),
    }

    if dx != 0 || dy != 0 {
        sink.apply(&InputPacket {
            seq: 0,
            event: InputEvent::MouseMove { dx, dy },
        })
        .await?;
    }

    Ok(DebugResponse {
        request_id: uuid::Uuid::nil(),
        ok: true,
        message: "mouse debug command applied".to_string(),
        display: crate::input::display_info()
            .ok()
            .map(|info| DisplaySnapshot {
                size: info.size,
                location: info.location,
            }),
        logs: Vec::new(),
    })
}

trait DebugResponseRequestId {
    fn with_request_id(self, request_id: uuid::Uuid) -> Self;
}

impl DebugResponseRequestId for DebugResponse {
    fn with_request_id(mut self, request_id: uuid::Uuid) -> Self {
        self.request_id = request_id;
        self
    }
}

fn debug_response_error(request_id: uuid::Uuid, message: String) -> DebugResponse {
    DebugResponse {
        request_id,
        ok: false,
        message,
        display: None,
        logs: Vec::new(),
    }
}

fn client_hello(options: &ClientOptions) -> Hello {
    let hello = Hello::client(options.name.clone()).with_app_metadata(
        crate::build_info::version(),
        crate::build_info::platform(),
        crate::build_info::commit(),
    );
    if options.dry_run {
        return hello;
    }

    match crate::input::display_info() {
        Ok(info) => {
            info!(
                width = info.size.width,
                height = info.size.height,
                "including client display size in handshake"
            );
            hello.with_screen_size(info.size)
        }
        Err(err) => {
            warn!(error = %err, "could not include client display size in handshake");
            hello
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deskbridge_core::{InputPacket, Welcome};
    use tokio::net::TcpListener;
    use uuid::Uuid;

    #[tokio::test]
    async fn stale_connection_reconnects_and_receives_input() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            assert!(matches!(
                read_frame(&mut first).await.unwrap(),
                Message::Hello(_)
            ));
            write_frame(&mut first, &welcome()).await.unwrap();
            time::sleep(Duration::from_secs(2)).await;
            drop(first);

            let (mut second, _) = listener.accept().await.unwrap();
            assert!(matches!(
                read_frame(&mut second).await.unwrap(),
                Message::Hello(_)
            ));
            write_frame(&mut second, &welcome()).await.unwrap();
            write_frame(
                &mut second,
                &Message::Input(InputPacket {
                    seq: 1,
                    event: InputEvent::MouseMove { dx: 1, dy: 0 },
                }),
            )
            .await
            .unwrap();
            assert!(matches!(
                read_frame(&mut second).await.unwrap(),
                Message::Ack(EventAck { seq: 1, .. })
            ));
        });

        time::timeout(
            Duration::from_secs(5),
            run(ClientOptions {
                server,
                name: "mac".to_string(),
                dry_run: true,
                reconnect: true,
                reverse_scroll: false,
                reconnect_max_ms: 500,
                stale_after_ms: 250,
                max_events: Some(1),
                clipboard: ClipboardConfig {
                    enabled: false,
                    ..ClipboardConfig::default()
                },
            }),
        )
        .await
        .unwrap()
        .unwrap();
    }

    fn welcome() -> Message {
        Message::Welcome(Welcome {
            session_id: Uuid::new_v4(),
            server_name: "windows".to_string(),
            heartbeat_interval_ms: 100,
            layout_revision: 1,
            capabilities: vec![Capability::Clipboard],
            clipboard_protocol: Some(CLIPBOARD_PROTOCOL_VERSION),
        })
    }
}
