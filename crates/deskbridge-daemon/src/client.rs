use crate::input::{EnigoSink, InputSink, LogSink};
use anyhow::{Context, Result, anyhow};
use deskbridge_core::{
    DEFAULT_HEARTBEAT_MS, EventAck, Hello, Message, Ping, Pong, REPLACED_SESSION_REASON,
    read_frame, write_frame,
};
use std::{net::SocketAddr, time::Duration};
use tokio::{net::TcpStream, time};
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub server: SocketAddr,
    pub name: String,
    pub dry_run: bool,
    pub reconnect: bool,
    pub reconnect_max_ms: u64,
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

    let heartbeat = Duration::from_millis(heartbeat_ms.max(DEFAULT_HEARTBEAT_MS));
    let mut ticker = time::interval(heartbeat);
    let mut seq = 0_u64;
    let mut received_events = 0_u64;

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                seq += 1;
                let ping = Message::Ping(Ping {
                    seq,
                    sent_at_ms: deskbridge_core::now_ms(),
                });
                debug!(seq, "sending heartbeat");
                write_frame(&mut stream, &ping).await?;
            }
            msg = read_frame(&mut stream) => {
                match msg? {
                    Message::Ping(ping) => {
                        write_frame(&mut stream, &Message::Pong(Pong {
                            seq: ping.seq,
                            sent_at_ms: ping.sent_at_ms,
                        })).await?;
                    }
                    Message::Pong(pong) => {
                        debug!(seq = pong.seq, "heartbeat acknowledged");
                    }
                    Message::Input(packet) => {
                        sink.apply(&packet).await?;
                        write_frame(&mut stream, &Message::Ack(EventAck { seq: packet.seq })).await?;
                        received_events += 1;
                        if options.max_events.is_some_and(|max_events| received_events >= max_events) {
                            return Ok(ClientSessionOutcome::Ended);
                        }
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

fn client_hello(options: &ClientOptions) -> Hello {
    let hello = Hello::client(options.name.clone());
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
