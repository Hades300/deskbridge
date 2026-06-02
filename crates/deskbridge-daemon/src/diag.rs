use anyhow::{Context, Result, anyhow};
use deskbridge_core::secure::{recv, send};
use deskbridge_core::{Encryption, Hello, Message, client_handshake};
use std::{net::SocketAddr, time::Duration};
use tokio::{net::TcpStream, time::timeout};

pub async fn run(server: SocketAddr, name: String, psk: Option<String>) -> Result<()> {
    println!("DeskBridge diagnostics");
    println!("local_version: {}", crate::build_info::version());
    println!("local_platform: {}", crate::build_info::platform());
    println!("server: {server}");
    println!("screen: {name}");

    let stream = timeout(Duration::from_secs(3), TcpStream::connect(server))
        .await
        .context("tcp connect timed out")?
        .context("tcp connect failed")?;
    println!("tcp: ok");

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
            println!("encryption: ok (psk)");
            Encryption::secure(session)
        }
        _ => Encryption::Plain,
    };
    send(
        &mut stream,
        &enc,
        &Message::Hello(Hello::diagnostic(name).with_app_metadata(
            crate::build_info::version(),
            crate::build_info::platform(),
            crate::build_info::commit(),
        )),
    )
    .await?;
    match timeout(Duration::from_secs(3), recv(&mut stream, &enc)).await {
        Ok(Ok(Message::Welcome(welcome))) => {
            println!("protocol: ok");
            println!("server_name: {}", welcome.server_name);
            println!("session_id: {}", welcome.session_id);
            Ok(())
        }
        Ok(Ok(Message::Status(status))) => {
            Err(anyhow!("server rejected client: {}", status.message))
        }
        Ok(Ok(other)) => Err(anyhow!("unexpected protocol response: {other:?}")),
        Ok(Err(err)) => Err(anyhow!(
            "protocol handshake failed: tcp is open, but the peer did not speak the DeskBridge protocol; check that this is not Input Leap or another service on the same port: {err}"
        )),
        Err(_) => Err(anyhow!("protocol handshake timed out")),
    }
}
