use crate::capture::CaptureEvent;
use anyhow::{Context, Result};
use deskbridge_core::{
    DEFAULT_HEARTBEAT_MS, DebugRequest, DebugResponse, Edge, FrameError, Hello, InputEvent,
    InputPacket, InputRouter, Layout, Message, REPLACED_SESSION_REASON, Size, Status, StatusKind,
    Welcome, read_frame, write_frame,
};
use std::{
    collections::HashMap, collections::HashSet, io, net::SocketAddr, sync::Arc, time::Duration,
};
use tokio::{
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
    pub heartbeat_ms: u64,
    pub layout: Layout,
}

type SessionRegistry = Arc<Mutex<HashMap<String, SessionHandle>>>;

#[derive(Debug)]
struct SessionHandle {
    session_id: Uuid,
    shutdown_tx: mpsc::UnboundedSender<()>,
    debug_tx: mpsc::UnboundedSender<DebugEnvelope>,
}

#[derive(Debug)]
struct DebugEnvelope {
    request: DebugRequest,
    response_tx: oneshot::Sender<DebugResponse>,
}

pub async fn run(options: ServerOptions) -> Result<()> {
    let options = apply_platform_layout(options);
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

fn apply_platform_layout(options: ServerOptions) -> ServerOptions {
    #[cfg(target_os = "windows")]
    {
        let mut options = options;
        if let Some((width, height)) = crate::capture::windows::primary_screen_size()
            && let Some(screen) = options
                .layout
                .screens
                .iter_mut()
                .find(|screen| screen.name == options.name)
        {
            screen.size.width = width;
            screen.size.height = height;
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
            && let Some(screen) = options
                .layout
                .screens
                .iter_mut()
                .find(|screen| screen.name == options.name)
        {
            screen.size.width = width;
            screen.size.height = height;
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

    if let Some(screen) = options
        .layout
        .screens
        .iter_mut()
        .find(|screen| screen.name == hello.screen_name)
    {
        screen.size = Size {
            width: size.width,
            height: size.height,
        };
        info!(
            screen = hello.screen_name,
            width = size.width,
            height = size.height,
            "using client-reported screen size for routing"
        );
    }
}

async fn handle_client(
    mut stream: TcpStream,
    peer: SocketAddr,
    mut options: ServerOptions,
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
    if hello.is_input_client() {
        apply_client_screen_size(&mut options, &hello);
    }

    let session_id = Uuid::new_v4();
    let welcome = Message::Welcome(Welcome {
        session_id,
        server_name: options.name.clone(),
        heartbeat_interval_ms: options.heartbeat_ms.max(DEFAULT_HEARTBEAT_MS),
        layout_revision: 1,
    });

    if !hello.is_input_client() {
        info!(
            peer = %peer,
            screen = hello.screen_name,
            role = ?hello.role,
            capabilities = ?hello.capabilities,
            "diagnostic client accepted"
        );
        write_frame(&mut stream, &welcome).await?;
        return handle_diagnostic_session(&mut stream, &hello.screen_name, sessions).await;
    }

    let (shutdown_tx, mut shutdown_rx) = mpsc::unbounded_channel();
    let (debug_tx, mut debug_rx) = mpsc::unbounded_channel();
    let session_key = hello.screen_name.to_ascii_lowercase();
    if let Some(previous) = sessions.lock().await.insert(
        session_key.clone(),
        SessionHandle {
            session_id,
            shutdown_tx,
            debug_tx,
        },
    ) {
        let _ = previous.shutdown_tx.send(());
        warn!(
            peer = %peer,
            screen = hello.screen_name,
            previous_session = %previous.session_id,
            new_session = %session_id,
            "replaced existing client session for screen"
        );
    }

    info!(peer = %peer, screen = hello.screen_name, "client accepted");

    write_frame(&mut stream, &welcome).await?;

    let result = run_client_session(
        &mut stream,
        &options,
        &hello.screen_name,
        &mut shutdown_rx,
        &mut debug_rx,
        capture_tx,
    )
    .await;
    remove_current_session(&sessions, &session_key, session_id).await;
    result
}

async fn run_client_session(
    mut stream: &mut TcpStream,
    options: &ServerOptions,
    client_name: &str,
    shutdown_rx: &mut mpsc::UnboundedReceiver<()>,
    debug_rx: &mut mpsc::UnboundedReceiver<DebugEnvelope>,
    capture_tx: crate::capture::CaptureSender,
) -> Result<()> {
    let mut ticker = time::interval(Duration::from_secs(5));
    let mut seq = 0_u64;
    let mut demo_stage = 0_u64;
    let mut demo_router = InputRouter::new(options.layout.clone(), options.name.clone()).ok();
    let mut capture_rx = capture_tx.subscribe();
    let mut pending_debug = HashMap::<Uuid, oneshot::Sender<DebugResponse>>::new();

    loop {
        tokio::select! {
            _ = ticker.tick(), if options.demo_events => {
                seq += 1;
                let event = next_demo_event(
                    &mut demo_router,
                    &options.layout,
                    &options.name,
                    client_name,
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
                    && let Some(event) = route_capture_event(&mut demo_router, event, client_name)
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
            Some(debug) = debug_rx.recv() => {
                let request_id = debug.request.request_id;
                if let Err(err) = write_frame(&mut stream, &Message::DebugRequest(debug.request)).await {
                    let _ = debug.response_tx.send(debug_error_response(
                        request_id,
                        format!("failed to send debug request to client: {err}"),
                    ));
                    return Err(err.into());
                }
                pending_debug.insert(request_id, debug.response_tx);
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
                    Message::DebugResponse(response) => {
                        if let Some(response_tx) = pending_debug.remove(&response.request_id) {
                            let _ = response_tx.send(response);
                        } else {
                            debug!(request_id = %response.request_id, "unexpected debug response");
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

async fn handle_diagnostic_session(
    stream: &mut TcpStream,
    target_screen: &str,
    sessions: SessionRegistry,
) -> Result<()> {
    match time::timeout(Duration::from_secs(5), read_frame(stream)).await {
        Ok(Ok(Message::DebugRequest(request))) => {
            forward_debug_request(stream, target_screen, sessions, request).await
        }
        Ok(Ok(other)) => {
            write_frame(
                stream,
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

async fn forward_debug_request(
    stream: &mut TcpStream,
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
        write_frame(
            stream,
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
        write_frame(
            stream,
            &Message::Status(Status {
                kind: StatusKind::Error,
                message: format!("target client '{target_screen}' is no longer available"),
            }),
        )
        .await?;
        return Ok(());
    }

    match time::timeout(Duration::from_secs(5), response_rx).await {
        Ok(Ok(response)) => write_frame(stream, &Message::DebugResponse(response)).await?,
        Ok(Err(_)) => {
            write_frame(
                stream,
                &Message::DebugResponse(debug_error_response(
                    request_id,
                    "debug response channel closed".to_string(),
                )),
            )
            .await?;
        }
        Err(_) => {
            write_frame(
                stream,
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
        CaptureEvent::LocalPointer { x, y } => {
            let routed = router.as_mut()?.observe_local_pointer(x, y)?;
            if let InputEvent::MouseAbs {
                x: target_x,
                y: target_y,
            } = routed.event
            {
                info!(
                    source_x = x,
                    source_y = y,
                    target = %routed.target_screen,
                    target_x,
                    target_y,
                    "activated remote screen from local pointer edge"
                );
            }
            routed
        }
        CaptureEvent::Input(event) => router.as_mut()?.route_if_remote_active(event)?,
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

#[cfg(test)]
mod tests {
    use super::*;
    use deskbridge_core::{
        DebugCommand, DebugRequest, DebugResponse, DisplaySnapshot, Link, Ping, Screen, Size,
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
                },
                Screen {
                    name: "mac".to_string(),
                    size: Size {
                        width: 1728,
                        height: 1117,
                    },
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
            heartbeat_ms: DEFAULT_HEARTBEAT_MS,
            layout: test_layout(),
        }
    }

    #[tokio::test]
    async fn diagnostic_handshake_does_not_replace_input_client() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen = listener.local_addr().unwrap();
        let options = test_options(listen);
        let allow = HashSet::from(["mac".to_string()]);
        let (capture_tx, _) = crate::capture::channel();
        let sessions = SessionRegistry::default();

        tokio::spawn({
            let options = options.clone();
            let allow = allow.clone();
            let capture_tx = capture_tx.clone();
            let sessions = sessions.clone();
            async move {
                for _ in 0..2 {
                    let (stream, peer) = listener.accept().await.unwrap();
                    tokio::spawn(handle_client(
                        stream,
                        peer,
                        options.clone(),
                        allow.clone(),
                        capture_tx.clone(),
                        sessions.clone(),
                    ));
                }
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

        tokio::spawn({
            let options = options.clone();
            let allow = allow.clone();
            let capture_tx = capture_tx.clone();
            let sessions = sessions.clone();
            async move {
                for _ in 0..2 {
                    let (stream, peer) = listener.accept().await.unwrap();
                    tokio::spawn(handle_client(
                        stream,
                        peer,
                        options.clone(),
                        allow.clone(),
                        capture_tx.clone(),
                        sessions.clone(),
                    ));
                }
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
    async fn ended_client_session_is_removed_from_registry() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen = listener.local_addr().unwrap();
        let options = test_options(listen);
        let allow = HashSet::from(["mac".to_string()]);
        let (capture_tx, _) = crate::capture::channel();
        let sessions = SessionRegistry::default();

        let server_task = tokio::spawn({
            let sessions = sessions.clone();
            async move {
                let (stream, peer) = listener.accept().await.unwrap();
                handle_client(stream, peer, options, allow, capture_tx, sessions)
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
