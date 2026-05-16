use crate::input::{EnigoSink, InputSink, LogSink};
use anyhow::{Context, Result, anyhow};
use deskbridge_core::{
    DEFAULT_HEARTBEAT_MS, DebugCommand, DebugRequest, DebugResponse, DisplaySnapshot, EventAck,
    FrameError, Hello, InputEvent, InputPacket, Message, Ping, Pong, REPLACED_SESSION_REASON,
    read_frame, write_frame,
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
    pub reconnect_max_ms: u64,
    pub stale_after_ms: u64,
    pub max_events: Option<u64>,
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
    let heartbeat_ms = match welcome {
        Message::Welcome(welcome) => {
            info!(
                server = welcome.server_name,
                session = %welcome.session_id,
                "connected"
            );
            welcome.heartbeat_interval_ms
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
                    return Err(anyhow!(
                        "server heartbeat stale: no inbound frame for {}ms; reconnecting",
                        silent_for.as_millis()
                    ));
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
                    Message::Input(packet) => {
                        sink.apply(&packet).await?;
                        write_frame(&mut writer, &Message::Ack(EventAck { seq: packet.seq })).await?;
                        received_events += 1;
                        debug_state.push(format!("applied input seq={} event={:?}", packet.seq, packet.event));
                        if options.max_events.is_some_and(|max_events| received_events >= max_events) {
                            return Ok(ClientSessionOutcome::Ended);
                        }
                    }
                    Message::DebugRequest(request) => {
                        let response = handle_debug_request(request, sink.as_mut(), &mut debug_state).await;
                        write_frame(&mut writer, &Message::DebugResponse(response)).await?;
                    }
                    Message::Status(status) => {
                        warn!(kind = ?status.kind, message = status.message, "server status");
                    }
                    Message::Goodbye { reason } => {
                        if reason == REPLACED_SESSION_REASON {
                            return Ok(ClientSessionOutcome::Replaced);
                        }
                        return Err(anyhow!("server closed session: {reason}"));
                    }
                    other => debug!(message = ?other, "ignored message"),
                }
            }
        }
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
}

impl ClientDebugState {
    fn new(options: &ClientOptions) -> Self {
        Self {
            logs: VecDeque::with_capacity(64),
            server: options.server,
            name: options.name.clone(),
            dry_run: options.dry_run,
            started_at_ms: deskbridge_core::now_ms(),
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
        DebugCommand::MoveMouse { x, y, dx, dy } => apply_debug_mouse_move(sink, x, y, dx, dy)
            .await
            .unwrap_or_else(|err| debug_response_error(request.request_id, format!("{err:#}")))
            .with_request_id(request.request_id),
        DebugCommand::RouteProbe { .. }
        | DebugCommand::RouteStatus
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
                Message::Ack(EventAck { seq: 1 })
            ));
        });

        time::timeout(
            Duration::from_secs(5),
            run(ClientOptions {
                server,
                name: "mac".to_string(),
                dry_run: true,
                reconnect: true,
                reconnect_max_ms: 500,
                stale_after_ms: 250,
                max_events: Some(1),
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
        })
    }
}
