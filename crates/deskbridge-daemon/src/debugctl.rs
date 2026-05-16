use anyhow::{Context, Result, anyhow};
use deskbridge_core::{
    DebugCommand, DebugRequest, DebugResponse, DisplaySnapshot, Hello, Message, read_frame,
    write_frame,
};
use std::{net::SocketAddr, time::Duration};
use tokio::{net::TcpStream, time::timeout};
use uuid::Uuid;

pub async fn run(server: SocketAddr, target: String, command: DebugCommand) -> Result<()> {
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

    write_frame(
        &mut stream,
        &Message::Hello(Hello::diagnostic(target).with_app_metadata(
            crate::build_info::version(),
            crate::build_info::platform(),
            crate::build_info::commit(),
        )),
    )
    .await?;
    match timeout(Duration::from_secs(3), read_frame(&mut stream)).await {
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
    write_frame(
        &mut stream,
        &Message::DebugRequest(DebugRequest {
            request_id,
            command,
        }),
    )
    .await?;

    match timeout(Duration::from_secs(5), read_frame(&mut stream)).await {
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
