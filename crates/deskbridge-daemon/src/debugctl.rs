use anyhow::{Context, Result, anyhow};
use deskbridge_core::secure::{recv, send};
use deskbridge_core::{
    DebugCommand, DebugRequest, DebugResponse, DisplaySnapshot, Encryption, Hello, Message,
    client_handshake,
};
use std::{net::SocketAddr, time::Duration};
use tokio::{net::TcpStream, time::timeout};
use uuid::Uuid;

pub async fn run(
    server: SocketAddr,
    target: String,
    command: DebugCommand,
    psk: Option<String>,
) -> Result<()> {
    println!("DeskBridge debug");
    println!("local_version: {}", crate::build_info::version());
    println!("local_platform: {}", crate::build_info::platform());
    println!("server: {server}");
    println!("target: {target}");

    let stream = timeout(Duration::from_secs(3), TcpStream::connect(server))
        .await
        .context("tcp connect timed out")?
        .context("tcp connect failed")?;
    let mut stream = stream;

    let enc = match psk.as_deref() {
        Some(secret) if !secret.is_empty() => {
            let session = timeout(
                Duration::from_secs(3),
                client_handshake(&mut stream, secret),
            )
            .await
            .context("PSK handshake timed out")?
            .context("PSK handshake failed (check the secret matches the server)")?;
            Encryption::secure(session)
        }
        _ => Encryption::Plain,
    };

    send(
        &mut stream,
        &enc,
        &Message::Hello(Hello::diagnostic(target).with_app_metadata(
            crate::build_info::version(),
            crate::build_info::platform(),
            crate::build_info::commit(),
        )),
    )
    .await?;
    match timeout(Duration::from_secs(3), recv(&mut stream, &enc)).await {
        Ok(Ok(Message::Welcome(welcome))) => {
            println!("server_name: {}", welcome.server_name);
            println!("session_id: {}", welcome.session_id);
        }
        Ok(Ok(Message::Status(status))) => {
            return Err(anyhow!("server rejected debug session: {}", status.message));
        }
        Ok(Ok(other)) => return Err(anyhow!("unexpected protocol response: {other:?}")),
        Ok(Err(err)) => return Err(anyhow!("protocol handshake failed: {err}")),
        Err(_) => return Err(anyhow!("protocol handshake timed out")),
    }

    let request_id = Uuid::new_v4();
    send(
        &mut stream,
        &enc,
        &Message::DebugRequest(DebugRequest {
            request_id,
            command,
        }),
    )
    .await?;

    match timeout(Duration::from_secs(5), recv(&mut stream, &enc)).await {
        Ok(Ok(Message::DebugResponse(response))) if response.request_id == request_id => {
            print_response(response);
            Ok(())
        }
        Ok(Ok(Message::Status(status))) => Err(anyhow!("debug request failed: {}", status.message)),
        Ok(Ok(other)) => Err(anyhow!("unexpected debug response: {other:?}")),
        Ok(Err(err)) => Err(anyhow!("debug response failed: {err}")),
        Err(_) => Err(anyhow!("debug response timed out")),
    }
}

fn print_response(response: DebugResponse) {
    println!("ok: {}", response.ok);
    println!("message: {}", response.message);

    if let Some(display) = response.display {
        print_display(display);
    }

    if !response.logs.is_empty() {
        println!("logs:");
        for line in response.logs {
            println!("  {line}");
        }
    }
}

fn print_display(display: DisplaySnapshot) {
    println!("display: {}x{}", display.size.width, display.size.height);
    match display.location {
        Some((x, y)) => println!("mouse_location: x={x} y={y}"),
        None => println!("mouse_location: unavailable"),
    }
}
