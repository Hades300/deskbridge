use crate::capture::CaptureEvent;
use anyhow::{Context, Result};
use deskbridge_core::{
    DEFAULT_HEARTBEAT_MS, Edge, FrameError, Hello, InputEvent, InputPacket, InputRouter, Layout,
    Message, REPLACED_SESSION_REASON, Status, StatusKind, Welcome, read_frame, write_frame,
};
use std::{collections::HashMap, collections::HashSet, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    net::TcpListener,
    net::TcpStream,
    sync::{Mutex, mpsc},
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
    pub heartbeat_ms: u64,
    pub layout: Layout,
}

type SessionRegistry = Arc<Mutex<HashMap<String, mpsc::UnboundedSender<()>>>>;

pub async fn run(options: ServerOptions) -> Result<()> {
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

    if options.capture {
        start_platform_capture(capture_tx.clone())?;
    }

    info!(listen = %options.listen, "server listening");

    loop {
        let (stream, peer) = listener.accept().await?;
        let options = options.clone();
        let allow = allow.clone();
        let capture_tx = capture_tx.clone();
        let sessions = sessions.clone();
        tokio::spawn(async move {
            if let Err(err) =
                handle_client(stream, peer, options, allow, capture_tx, sessions).await
            {
                warn!(peer = %peer, error = %err, "client ended");
            }
        });
    }
}

async fn handle_client(
    mut stream: TcpStream,
    peer: SocketAddr,
    options: ServerOptions,
    allow: HashSet<String>,
    capture_tx: crate::capture::CaptureSender,
    sessions: SessionRegistry,
) -> Result<()> {
    stream.set_nodelay(true)?;
    let hello = match read_frame(&mut stream).await {
        Ok(Message::Hello(hello)) => hello,
        Ok(other) => {
            write_frame(
                &mut stream,
                &Message::Status(Status {
                    kind: StatusKind::Error,
                    message: format!("expected hello, got {other:?}"),
                }),
            )
            .await?;
            return Ok(());
        }
        Err(FrameError::ForeignProtocol { magic }) => {
            warn!(
                peer = %peer,
                magic,
                "non-DeskBridge client connected; this is usually an old Input Leap, Barrier, or Synergy client pointed at the DeskBridge port"
            );
            return Ok(());
        }
        Err(err) => return Err(err.into()),
    };

    validate_client(&hello, &allow, &mut stream).await?;

    let (shutdown_tx, mut shutdown_rx) = mpsc::unbounded_channel();
    let session_key = hello.screen_name.to_ascii_lowercase();
    if let Some(previous) = sessions.lock().await.insert(session_key, shutdown_tx) {
        let _ = previous.send(());
        warn!(
            peer = %peer,
            screen = hello.screen_name,
            "replaced existing client session for screen"
        );
    }

    info!(peer = %peer, screen = hello.screen_name, "client accepted");

    write_frame(
        &mut stream,
        &Message::Welcome(Welcome {
            session_id: Uuid::new_v4(),
            server_name: options.name.clone(),
            heartbeat_interval_ms: options.heartbeat_ms.max(DEFAULT_HEARTBEAT_MS),
            layout_revision: 1,
        }),
    )
    .await?;

    let mut ticker = time::interval(Duration::from_secs(5));
    let mut seq = 0_u64;
    let mut demo_stage = 0_u64;
    let mut demo_router = InputRouter::new(options.layout.clone(), options.name.clone()).ok();
    let mut capture_rx = capture_tx.subscribe();

    loop {
        tokio::select! {
            _ = ticker.tick(), if options.demo_events => {
                seq += 1;
                let event = next_demo_event(
                    &mut demo_router,
                    &options.layout,
                    &options.name,
                    &hello.screen_name,
                    demo_stage,
                );
                demo_stage += 1;
                write_frame(&mut stream, &Message::Input(InputPacket {
                    seq,
                    event,
                })).await?;
            }
            event = capture_rx.recv(), if options.capture => {
                if let Ok(event) = event
                    && let Some(event) = route_capture_event(&mut demo_router, event, &hello.screen_name)
                {
                    seq += 1;
                    write_frame(&mut stream, &Message::Input(InputPacket {
                        seq,
                        event,
                    })).await?;
                }
            }
            _ = shutdown_rx.recv() => {
                let _ = write_frame(&mut stream, &Message::Goodbye {
                    reason: REPLACED_SESSION_REASON.to_string(),
                }).await;
                return Ok(());
            }
            msg = read_frame(&mut stream) => {
                match msg? {
                    Message::Ping(ping) => {
                        write_frame(&mut stream, &Message::Pong(deskbridge_core::Pong {
                            seq: ping.seq,
                            sent_at_ms: ping.sent_at_ms,
                        })).await?;
                    }
                    Message::Pong(pong) => debug!(seq = pong.seq, "client pong"),
                    Message::Ack(ack) => debug!(seq = ack.seq, "input event acknowledged"),
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

fn start_platform_capture(capture_tx: crate::capture::CaptureSender) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        crate::capture::windows::spawn(capture_tx)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = capture_tx;
        anyhow::bail!("input capture is only implemented for Windows hosts");
    }
}

async fn validate_client(
    hello: &Hello,
    allow: &HashSet<String>,
    stream: &mut TcpStream,
) -> Result<()> {
    if hello.protocol_version != deskbridge_core::PROTOCOL_VERSION {
        write_frame(
            stream,
            &Message::Status(Status {
                kind: StatusKind::Error,
                message: format!("unsupported protocol {}", hello.protocol_version),
            }),
        )
        .await?;
        anyhow::bail!("unsupported protocol {}", hello.protocol_version);
    }

    if !allow.is_empty() && !allow.contains(&hello.screen_name.to_ascii_lowercase()) {
        write_frame(
            stream,
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
    let routed = match event {
        CaptureEvent::LocalPointer { x, y } => router.as_mut()?.observe_local_pointer(x, y)?,
        CaptureEvent::Input(event) => router.as_ref()?.route_if_remote_active(event)?,
    };
    (routed.target_screen == client_name).then_some(routed.event)
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
    sample_point_on_edge(layout, server_name, link.edge)
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
