use crate::protocol::Message;
use std::io;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid frame length {0}")]
    InvalidLength(usize),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub async fn read_frame<R>(reader: &mut R) -> Result<Message, FrameError>
where
    R: AsyncRead + Unpin,
{
    let len = match reader.read_u32().await {
        Ok(len) => len as usize,
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Err(err.into()),
        Err(err) => return Err(err.into()),
    };

    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(FrameError::InvalidLength(len));
    }

    let mut buf = vec![0_u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}

pub async fn write_frame<W>(writer: &mut W, msg: &Message) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
{
    let bytes = serde_json::to_vec(msg)?;
    if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
        return Err(FrameError::InvalidLength(bytes.len()));
    }

    writer.write_u32(bytes.len() as u32).await?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Hello;
    use tokio::io::duplex;

    #[tokio::test]
    async fn frame_round_trip() {
        let (mut a, mut b) = duplex(4096);
        let msg = Message::Hello(Hello::client("mac"));

        write_frame(&mut a, &msg).await.unwrap();
        let read = read_frame(&mut b).await.unwrap();
        assert_eq!(msg, read);
    }
}
