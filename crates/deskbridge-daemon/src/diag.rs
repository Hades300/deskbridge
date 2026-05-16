use anyhow::{Context, Result, anyhow};
use deskbridge_core::{Hello, Message, read_frame, write_frame};
use std::{net::SocketAddr, time::Duration};
use tokio::{net::TcpStream, time::timeout};

pub async fn run(server: SocketAddr, name: String) -> Result<()> {
    println!("DeskBridge diagnostics");
    println!("server: {server}");
    println!("screen: {name}");

    let stream = timeout(Duration::from_secs(3), TcpStream::connect(server))
        .await
        .context("tcp connect timed out")?
        .context("tcp connect failed")?;
    println!("tcp: ok");

    let mut stream = stream;
    write_frame(&mut stream, &Message::Hello(Hello::diagnostic(name))).await?;
    match timeout(Duration::from_secs(3), read_frame(&mut stream)).await {
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
