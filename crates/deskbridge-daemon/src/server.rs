use crate::capture::CaptureEvent;
use anyhow::{Context, Result};
use deskbridge_core::{
    DEFAULT_HEARTBEAT_MS, DebugCommand, DebugRequest, DebugResponse, Edge, FrameError, Hello,
    InputEvent, InputPacket, InputRouter, Layout, Message, REPLACED_SESSION_REASON, Size, Status,
    StatusKind, Welcome, read_frame, write_frame,
};
use std::{
    collections::HashMap,
    collections::HashSet,
    collections::VecDeque,
    io,
    net::SocketAddr,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
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
    pub debug_capture_log: bool,
    pub reverse_scroll: bool,
    pub heartbeat_ms: u64,
    pub layout: Layout,
}

type SessionRegistry = Arc<Mutex<HashMap<String, SessionHandle>>>;
type ServerDebugLog = Arc<StdMutex<VecDeque<String>>>;

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

struct ClientSessionRuntime<'a> {
    options: &'a ServerOptions,
    client_name: &'a str,
    session_id: Uuid,
    peer: SocketAddr,
    shutdown_rx: &'a mut mpsc::UnboundedReceiver<()>,
    debug_rx: &'a mut mpsc::UnboundedReceiver<DebugEnvelope>,
    route_debug_rx: &'a mut mpsc::UnboundedReceiver<RouteDebugEnvelope>,
    capture_tx: crate::capture::CaptureSender,
    server_log: ServerDebugLog,
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

    if options.capture {
        start_platform_capture(capture_tx.clone())?;
    }

    info!(listen = %options.listen, "server listening");
    push_server_log(
        &server_log,
        format!(
            "server listening listen={} screen={} capture={} debug_capture_log={} reverse_scroll={} version={} platform={}",
            options.listen,
            options.name,
            options.capture,
            options.debug_capture_log,
            options.reverse_scroll,
            crate::build_info::version(),
            crate::build_info::platform()
        ),
    );

    loop {
        let (stream, peer) = listener.accept().await?;
        let options = options.clone();
        let allow = allow.clone();
        let capture_tx = capture_tx.clone();
        let sessions = sessions.clone();
        let server_log = server_log.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_client(
                stream,
                peer,
                options,
                allow,
                capture_tx,
                sessions,
                server_log.clone(),
            )
            .await
            {
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

async fn handle_client(
    mut stream: TcpStream,
    peer: SocketAddr,
    mut options: ServerOptions,
    allow: HashSet<String>,
    capture_tx: crate::capture::CaptureSender,
    sessions: SessionRegistry,
    server_log: ServerDebugLog,
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

    if let Err(err) = validate_client(&hello, &allow, &mut stream).await {
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
        write_frame(&mut stream, &welcome).await?;
        return handle_diagnostic_session(
            &mut stream,
            &hello.screen_name,
            &options,
            sessions,
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

    write_frame(&mut stream, &welcome).await?;

    let result = run_client_session(
        stream,
        ClientSessionRuntime {
            options: &options,
            client_name: &hello.screen_name,
            session_id,
            peer,
            shutdown_rx: &mut shutdown_rx,
            debug_rx: &mut debug_rx,
            route_debug_rx: &mut route_debug_rx,
            capture_tx,
            server_log: server_log.clone(),
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
        client_name,
        session_id,
        peer,
        shutdown_rx,
        debug_rx,
        route_debug_rx,
        capture_tx,
        server_log,
    } = runtime;
    let mut ticker = time::interval(Duration::from_secs(5));
    let (reader, mut writer) = stream.into_split();
    let mut inbound = spawn_reader(reader);
    let mut seq = 0_u64;
    let mut demo_stage = 0_u64;
    let mut demo_router = InputRouter::new(options.layout.clone(), options.name.clone()).ok();
    let mut capture_rx = capture_tx.subscribe();
    let mut pending_debug = HashMap::<Uuid, oneshot::Sender<DebugResponse>>::new();
    let mut pending_route_probes = HashMap::<Uuid, PendingRouteProbe>::new();
    let mut route_probe_seq_index = HashMap::<u64, Uuid>::new();
    let mut pending_capture_probes = HashMap::<Uuid, PendingCaptureProbe>::new();
    let mut capture_probe_seq_index = HashMap::<u64, Uuid>::new();

    loop {
        tokio::select! {
            _ = ticker.tick(), if options.demo_events => {
                seq += 1;
                let event = transform_routed_input_event(
                    next_demo_event(
                    &mut demo_router,
                    &options.layout,
                    &options.name,
                    client_name,
                    demo_stage,
                    ),
                    options.reverse_scroll,
                );
                demo_stage += 1;
                write_frame(&mut writer, &Message::Input(InputPacket {
                    seq,
                    event,
                })).await?;
            }
            event = capture_rx.recv() => {
                if let Ok(event) = event {
                    let capture_log_line = if options.debug_capture_log {
                        Some(describe_capture_event(&event))
                    } else {
                        None
                    };
                    let probe_id = capture_probe_id(&event);
                    let capture_event = capture_event_payload(event);
                    let routed = route_capture_event(&mut demo_router, capture_event, client_name);
                    if probe_id.is_none() {
                        let suppress_local_input = demo_router
                            .as_ref()
                            .is_some_and(|router| router.active_screen() != options.name);
                        crate::capture::set_local_input_suppressed(suppress_local_input);
                    }

                    if probe_id.is_none()
                        && let Some(capture_log_line) = capture_log_line
                    {
                        let route_log_line = routed
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

                    if let Some(request_id) = probe_id
                        && let Some(probe) = pending_capture_probes.get_mut(&request_id)
                    {
                        probe.processed_capture_events += 1;
                        match &routed {
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

                    if let Some(event) = routed {
                        seq += 1;
                        let event = transform_routed_input_event(event, options.reverse_scroll);
                        write_frame(&mut writer, &Message::Input(InputPacket {
                            seq,
                            event,
                        })).await?;
                        if let Some(request_id) = probe_id
                            && let Some(probe) = pending_capture_probes.get_mut(&request_id)
                        {
                            capture_probe_seq_index.insert(seq, request_id);
                            probe.remaining_seqs.insert(seq);
                            probe.routed_events += 1;
                        }
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
                let _ = write_frame(&mut writer, &Message::Goodbye {
                    reason: REPLACED_SESSION_REASON.to_string(),
                }).await;
                return Ok(());
            }
            Some(debug) = debug_rx.recv() => {
                let request_id = debug.request.request_id;
                if let Err(err) = write_frame(&mut writer, &Message::DebugRequest(debug.request)).await {
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
                            logs: build_route_status_logs(options, client_name, &demo_router, route_debug.request_id),
                        });
                        continue;
                    }
                    RouteDebugCommand::Probe(probe_options) => {
                        let (events, logs) = match build_route_probe_events(
                            options,
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
                            seq += 1;
                            let event = transform_routed_input_event(event, options.reverse_scroll);
                            write_frame(&mut writer, &Message::Input(InputPacket {
                                seq,
                                event,
                            })).await?;
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
                }
            }
            msg = inbound.recv() => {
                let message = msg
                    .ok_or_else(|| anyhow::anyhow!("client reader stopped"))??;
                match message {
                    Message::Ping(ping) => {
                        write_frame(&mut writer, &Message::Pong(deskbridge_core::Pong {
                            seq: ping.seq,
                            sent_at_ms: ping.sent_at_ms,
                        })).await?;
                    }
                    Message::Pong(pong) => debug!(seq = pong.seq, "client pong"),
                    Message::Ack(ack) => {
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
    options: &ServerOptions,
    sessions: SessionRegistry,
    server_log: ServerDebugLog,
) -> Result<()> {
    match time::timeout(Duration::from_secs(5), read_frame(stream)).await {
        Ok(Ok(Message::DebugRequest(request))) => match request.command.clone() {
            DebugCommand::ServerLogs => {
                let response = build_server_logs_response(
                    request.request_id,
                    target_screen,
                    options,
                    &sessions,
                    &server_log,
                )
                .await;
                write_frame(stream, &Message::DebugResponse(response)).await?;
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
                    target_screen,
                    sessions,
                    request.request_id,
                    RouteDebugCommand::Status,
                )
                .await
            }
            _ => forward_debug_request(stream, target_screen, sessions, request).await,
        },
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
    logs.push(format!("reverse_scroll={}", options.reverse_scroll));
    logs.push(format!("heartbeat_ms={}", options.heartbeat_ms));
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

async fn forward_route_debug_request(
    stream: &mut TcpStream,
    target_screen: &str,
    sessions: SessionRegistry,
    request_id: Uuid,
    command: RouteDebugCommand,
) -> Result<()> {
    let Some(route_debug_tx) = route_debug_sender(target_screen, &sessions).await else {
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

    let (response_tx, response_rx) = oneshot::channel();
    if route_debug_tx
        .send(RouteDebugEnvelope {
            request_id,
            command,
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
                    "route debug response channel closed".to_string(),
                )),
            )
            .await?;
        }
        Err(_) => {
            write_frame(
                stream,
                &Message::DebugResponse(debug_error_response(
                    request_id,
                    "route debug request timed out".to_string(),
                )),
            )
            .await?;
        }
    }

    Ok(())
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
    target_screen: &str,
    probe_options: RouteProbeOptions,
    request_id: Uuid,
) -> Result<(Vec<InputEvent>, Vec<String>)> {
    let edge = match probe_options.edge {
        Some(edge) => edge,
        None => options
            .layout
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
    let (x, y) = sample_point_for_transition(&options.layout, &options.name, target_screen, edge)
        .ok_or_else(|| {
        anyhow::anyhow!(
            "layout does not include server screen '{}' for route probe",
            options.name
        )
    })?;

    let mut router = Some(InputRouter::new(
        options.layout.clone(),
        options.name.clone(),
    )?);
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
    target_screen: &str,
    probe_options: RouteProbeOptions,
    request_id: Uuid,
) -> Result<(Vec<CaptureEvent>, Vec<String>)> {
    let edge = match probe_options.edge {
        Some(edge) => edge,
        None => options
            .layout
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
    let (x, y) = sample_point_for_transition(&options.layout, &options.name, target_screen, edge)
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
    target_screen: &str,
    router: &Option<InputRouter>,
    request_id: Uuid,
) -> Vec<String> {
    let mut logs = vec![
        format!(
            "route status request={request_id} server={} target={target_screen}",
            options.name
        ),
        format!(
            "listen={} capture={} demo_events={} reverse_scroll={} heartbeat_ms={}",
            options.listen,
            options.capture,
            options.demo_events,
            options.reverse_scroll,
            options.heartbeat_ms
        ),
        format!(
            "active_route_screen={}",
            router
                .as_ref()
                .map(|router| router.active_screen().to_string())
                .unwrap_or_else(|| "unavailable".to_string())
        ),
    ];
    logs.extend(platform_screen_debug_lines());

    for screen in &options.layout.screens {
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
    for link in options
        .layout
        .links
        .iter()
        .filter(|link| link.from == options.name)
    {
        if link.to == target_screen {
            target_link_count += 1;
        }

        match sample_point_for_transition(&options.layout, &link.from, &link.to, link.edge)
            .and_then(|(x, y)| {
                options
                    .layout
                    .transition(&link.from, link.edge, x, y)
                    .map(|transition| (x, y, transition))
            }) {
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
        CaptureEvent::LocalPointer { x, y } | CaptureEvent::ProbeLocalPointer { x, y, .. } => {
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
        CaptureEvent::Input(event) | CaptureEvent::ProbeInput { event, .. } => {
            router.as_mut()?.route_if_remote_active(event)?
        }
    };
    (routed.target_screen == client_name).then_some(routed.event)
}

fn transform_routed_input_event(mut event: InputEvent, reverse_scroll: bool) -> InputEvent {
    if reverse_scroll && let InputEvent::Wheel { dx, dy } = &mut event {
        *dx = dx.saturating_neg();
        *dy = dy.saturating_neg();
    }
    event
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
        DebugCommand, DebugRequest, DebugResponse, DisplaySnapshot, EventAck, Link, Ping, Screen,
        Size,
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
        let server_log = new_server_debug_log();

        tokio::spawn({
            let options = options.clone();
            let allow = allow.clone();
            let capture_tx = capture_tx.clone();
            let sessions = sessions.clone();
            let server_log = server_log.clone();
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
                        server_log.clone(),
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

        tokio::spawn({
            let options = options.clone();
            let allow = allow.clone();
            let capture_tx = capture_tx.clone();
            let sessions = sessions.clone();
            let server_log = server_log.clone();
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
                        server_log.clone(),
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

        tokio::spawn({
            let options = options.clone();
            let allow = allow.clone();
            let capture_tx = capture_tx.clone();
            let sessions = sessions.clone();
            let server_log = server_log.clone();
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
                        server_log.clone(),
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
            write_frame(&mut client, &Message::Ack(EventAck { seq: packet.seq }))
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

        tokio::spawn({
            let options = options.clone();
            let allow = allow.clone();
            let capture_tx = capture_tx.clone();
            let sessions = sessions.clone();
            let server_log = server_log.clone();
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
                        server_log.clone(),
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
            write_frame(&mut client, &Message::Ack(EventAck { seq: packet.seq }))
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

        tokio::spawn({
            let options = options.clone();
            let allow = allow.clone();
            let capture_tx = capture_tx.clone();
            let sessions = sessions.clone();
            let server_log = server_log.clone();
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
                        server_log.clone(),
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
        push_server_log(&server_log, "test history entry");

        tokio::spawn({
            let sessions = sessions.clone();
            let server_log = server_log.clone();
            async move {
                let (stream, peer) = listener.accept().await.unwrap();
                handle_client(
                    stream, peer, options, allow, capture_tx, sessions, server_log,
                )
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
    async fn ended_client_session_is_removed_from_registry() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen = listener.local_addr().unwrap();
        let options = test_options(listen);
        let allow = HashSet::from(["mac".to_string()]);
        let (capture_tx, _) = crate::capture::channel();
        let sessions = SessionRegistry::default();
        let server_log = new_server_debug_log();

        let server_task = tokio::spawn({
            let sessions = sessions.clone();
            let server_log = server_log.clone();
            async move {
                let (stream, peer) = listener.accept().await.unwrap();
                handle_client(
                    stream, peer, options, allow, capture_tx, sessions, server_log,
                )
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
