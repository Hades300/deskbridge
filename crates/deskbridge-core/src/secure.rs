//! Optional pre-shared-key encryption for the DeskBridge transport.
//!
//! When both peers are configured with the same secret, the connection is
//! upgraded to a Noise (`Noise_NNpsk0`) channel before any application data is
//! exchanged. That gives mutual authentication (only holders of the secret can
//! complete the handshake), confidentiality and integrity for every frame
//! (ChaCha20-Poly1305), and forward secrecy (ephemeral X25519). Without a
//! secret the transport stays plaintext, preserving backwards compatibility.
//!
//! Keystrokes carry passwords and the clipboard carries arbitrary user data, so
//! this closes the biggest gap versus comparable tools, which ship TLS.

use crate::codec::{FrameError, MAX_FRAME_BYTES, read_frame, write_frame};
use crate::protocol::Message;
use sha2::{Digest, Sha256};
use snow::{Builder, HandshakeState, TransportState};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Noise pattern: no static keys, pre-shared key mixed in before the first
/// message, X25519 DH, ChaCha20-Poly1305 AEAD, BLAKE2s hashing.
const NOISE_PARAMS: &str = "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s";

/// Anonymous handshake used for interactive pairing: same primitives, no PSK.
/// The resulting channel is confidential but unauthenticated, so pairing layers
/// a short-authentication-string (SAS) comparison on top to detect a MITM.
const PAIRING_PARAMS: &str = "Noise_NN_25519_ChaChaPoly_BLAKE2s";

/// Noise limits a single transport message to 65535 bytes including the tag.
const MAX_NOISE_MESSAGE: usize = 65535;
const TAG_LEN: usize = 16;
const MAX_CHUNK_PLAINTEXT: usize = MAX_NOISE_MESSAGE - TAG_LEN;
/// Handshake messages are tiny (an ephemeral key plus a tag); cap them so a
/// peer cannot make us allocate before authentication.
const MAX_HANDSHAKE_FRAME: usize = 4096;

/// Derive a 32-byte Noise PSK from an arbitrary user passphrase.
fn derive_psk(secret: &str) -> [u8; 32] {
    let digest = Sha256::digest(secret.as_bytes());
    let mut psk = [0u8; 32];
    psk.copy_from_slice(&digest);
    psk
}

fn crypto_err<E: std::fmt::Display>(err: E) -> FrameError {
    FrameError::Crypto(err.to_string())
}

pub(crate) async fn write_raw<W>(writer: &mut W, bytes: &[u8]) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
{
    if bytes.is_empty() || bytes.len() > MAX_HANDSHAKE_FRAME {
        return Err(FrameError::InvalidLength(bytes.len()));
    }
    writer.write_u32(bytes.len() as u32).await?;
    writer.write_all(bytes).await?;
    writer.flush().await?;
    Ok(())
}

pub(crate) async fn read_raw<R>(reader: &mut R) -> Result<Vec<u8>, FrameError>
where
    R: AsyncRead + Unpin,
{
    let len = reader.read_u32().await? as usize;
    if len == 0 || len > MAX_HANDSHAKE_FRAME {
        return Err(FrameError::InvalidLength(len));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
}

/// Build a handshake state for the given pattern, optionally mixing in a PSK.
fn build_handshake(
    params: &str,
    psk: Option<&str>,
    initiator: bool,
) -> Result<HandshakeState, FrameError> {
    let psk_bytes = psk.map(derive_psk);
    let mut builder = Builder::new(params.parse().map_err(crypto_err)?);
    if let Some(bytes) = psk_bytes.as_ref() {
        builder = builder.psk(0, bytes).map_err(crypto_err)?;
    }
    if initiator {
        builder.build_initiator()
    } else {
        builder.build_responder()
    }
    .map_err(crypto_err)
}

/// Drive the two-message handshake (`-> e`, `<- e, ee`) as the initiator and
/// return the transport plus the channel-binding handshake hash.
async fn run_initiator<S>(
    stream: &mut S,
    mut handshake: HandshakeState,
) -> Result<(SecureSession, [u8; 32]), FrameError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; MAX_HANDSHAKE_FRAME];
    let len = handshake.write_message(&[], &mut buf).map_err(crypto_err)?;
    write_raw(stream, &buf[..len]).await?;

    let response = read_raw(stream).await?;
    let mut scratch = vec![0u8; MAX_HANDSHAKE_FRAME];
    handshake
        .read_message(&response, &mut scratch)
        .map_err(crypto_err)?;

    finish_handshake(handshake)
}

/// Drive the two-message handshake as the responder.
async fn run_responder<S>(
    stream: &mut S,
    mut handshake: HandshakeState,
) -> Result<(SecureSession, [u8; 32]), FrameError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let first = read_raw(stream).await?;
    let mut scratch = vec![0u8; MAX_HANDSHAKE_FRAME];
    handshake
        .read_message(&first, &mut scratch)
        .map_err(crypto_err)?;

    let mut buf = vec![0u8; MAX_HANDSHAKE_FRAME];
    let len = handshake.write_message(&[], &mut buf).map_err(crypto_err)?;
    write_raw(stream, &buf[..len]).await?;

    finish_handshake(handshake)
}

fn finish_handshake(handshake: HandshakeState) -> Result<(SecureSession, [u8; 32]), FrameError> {
    let mut hash = [0u8; 32];
    let raw = handshake.get_handshake_hash();
    if raw.len() < 32 {
        return Err(FrameError::Crypto("short handshake hash".to_string()));
    }
    hash.copy_from_slice(&raw[..32]);
    let transport = handshake.into_transport_mode().map_err(crypto_err)?;
    Ok((SecureSession { transport }, hash))
}

/// Perform the PSK-authenticated Noise handshake as the initiator (client).
pub async fn client_handshake<S>(stream: &mut S, secret: &str) -> Result<SecureSession, FrameError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let handshake = build_handshake(NOISE_PARAMS, Some(secret), true)?;
    Ok(run_initiator(stream, handshake).await?.0)
}

/// Perform the PSK-authenticated Noise handshake as the responder (server).
pub async fn server_handshake<S>(stream: &mut S, secret: &str) -> Result<SecureSession, FrameError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let handshake = build_handshake(NOISE_PARAMS, Some(secret), false)?;
    Ok(run_responder(stream, handshake).await?.0)
}

/// Anonymous pairing handshake as the initiator (joining device). Returns the
/// confidential transport and the SAS channel-binding hash.
pub async fn pairing_handshake_initiator<S>(
    stream: &mut S,
) -> Result<(SecureSession, [u8; 32]), FrameError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let handshake = build_handshake(PAIRING_PARAMS, None, true)?;
    run_initiator(stream, handshake).await
}

/// Anonymous pairing handshake as the responder (hosting device).
pub async fn pairing_handshake_responder<S>(
    stream: &mut S,
) -> Result<(SecureSession, [u8; 32]), FrameError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let handshake = build_handshake(PAIRING_PARAMS, None, false)?;
    run_responder(stream, handshake).await
}

/// An established Noise transport. `send`/`recv` serialize a [`Message`], split
/// it across as many AEAD records as needed (clipboard payloads can exceed the
/// Noise per-message limit), and reassemble on the far side.
pub struct SecureSession {
    transport: TransportState,
}

impl SecureSession {
    /// Encrypt a message into a wire body: a record count followed by
    /// length-prefixed ciphertext records.
    fn encrypt_message(&mut self, msg: &Message) -> Result<Vec<u8>, FrameError> {
        let plaintext = serde_json::to_vec(msg)?;
        let mut body = Vec::with_capacity(plaintext.len() + plaintext.len() / 256 + 16);
        let mut cipher = vec![0u8; MAX_CHUNK_PLAINTEXT + TAG_LEN];

        // A serialized message is never empty, but treat it uniformly: at least
        // one record so the reader's count is always >= 1.
        let chunks: Vec<&[u8]> = if plaintext.is_empty() {
            vec![&[]]
        } else {
            plaintext.chunks(MAX_CHUNK_PLAINTEXT).collect()
        };

        body.extend_from_slice(&(chunks.len() as u32).to_be_bytes());
        for chunk in chunks {
            let len = self
                .transport
                .write_message(chunk, &mut cipher)
                .map_err(crypto_err)?;
            body.extend_from_slice(&(len as u32).to_be_bytes());
            body.extend_from_slice(&cipher[..len]);
        }
        Ok(body)
    }

    /// Encrypt a single small payload (≤ one Noise record) into one AEAD
    /// record. Used by pairing to transfer the freshly minted secret.
    pub fn encrypt_raw(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, FrameError> {
        if plaintext.len() > MAX_CHUNK_PLAINTEXT {
            return Err(FrameError::InvalidLength(plaintext.len()));
        }
        let mut cipher = vec![0u8; plaintext.len() + TAG_LEN];
        let len = self
            .transport
            .write_message(plaintext, &mut cipher)
            .map_err(crypto_err)?;
        cipher.truncate(len);
        Ok(cipher)
    }

    /// Decrypt a single AEAD record produced by [`Self::encrypt_raw`].
    pub fn decrypt_raw(&mut self, record: &[u8]) -> Result<Vec<u8>, FrameError> {
        let mut out = vec![0u8; record.len()];
        let len = self
            .transport
            .read_message(record, &mut out)
            .map_err(crypto_err)?;
        out.truncate(len);
        Ok(out)
    }

    /// Decrypt a wire body produced by [`Self::encrypt_message`].
    fn decrypt_message(&mut self, body: &[u8]) -> Result<Message, FrameError> {
        let mut cursor = 0usize;
        let count = read_u32(body, &mut cursor)? as usize;
        let mut plaintext = Vec::new();
        let mut scratch = vec![0u8; MAX_CHUNK_PLAINTEXT + TAG_LEN];

        for _ in 0..count {
            let len = read_u32(body, &mut cursor)? as usize;
            if len > MAX_CHUNK_PLAINTEXT + TAG_LEN || cursor + len > body.len() {
                return Err(FrameError::Crypto(
                    "invalid encrypted record length".to_string(),
                ));
            }
            let written = self
                .transport
                .read_message(&body[cursor..cursor + len], &mut scratch)
                .map_err(crypto_err)?;
            plaintext.extend_from_slice(&scratch[..written]);
            cursor += len;
        }

        Ok(serde_json::from_slice(&plaintext)?)
    }
}

fn read_u32(buf: &[u8], cursor: &mut usize) -> Result<u32, FrameError> {
    if *cursor + 4 > buf.len() {
        return Err(FrameError::Crypto("truncated encrypted frame".to_string()));
    }
    let value = u32::from_be_bytes(buf[*cursor..*cursor + 4].try_into().unwrap());
    *cursor += 4;
    Ok(value)
}

/// Transport encryption state shared by a connection's read and write halves.
///
/// `Plain` keeps the original length-prefixed JSON wire format untouched.
#[derive(Clone)]
pub enum Encryption {
    Plain,
    Secure(Arc<Mutex<SecureSession>>),
}

impl Encryption {
    pub fn secure(session: SecureSession) -> Self {
        Encryption::Secure(Arc::new(Mutex::new(session)))
    }

    pub fn is_secure(&self) -> bool {
        matches!(self, Encryption::Secure(_))
    }
}

/// Write a [`Message`], encrypting it when the connection is secured.
pub async fn send<W>(writer: &mut W, enc: &Encryption, msg: &Message) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
{
    match enc {
        Encryption::Plain => write_frame(writer, msg).await,
        Encryption::Secure(session) => {
            let body = {
                let mut guard = session
                    .lock()
                    .map_err(|_| FrameError::Crypto("secure session poisoned".to_string()))?;
                guard.encrypt_message(msg)?
            };
            if body.is_empty() || body.len() > MAX_FRAME_BYTES {
                return Err(FrameError::InvalidLength(body.len()));
            }
            let mut frame = Vec::with_capacity(4 + body.len());
            frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
            frame.extend_from_slice(&body);
            writer.write_all(&frame).await?;
            writer.flush().await?;
            Ok(())
        }
    }
}

/// Read a [`Message`], decrypting it when the connection is secured.
pub async fn recv<R>(reader: &mut R, enc: &Encryption) -> Result<Message, FrameError>
where
    R: AsyncRead + Unpin,
{
    match enc {
        Encryption::Plain => read_frame(reader).await,
        Encryption::Secure(session) => {
            let len = reader.read_u32().await? as usize;
            if len == 0 || len > MAX_FRAME_BYTES {
                return Err(FrameError::InvalidLength(len));
            }
            let mut body = vec![0u8; len];
            reader.read_exact(&mut body).await?;
            let mut guard = session
                .lock()
                .map_err(|_| FrameError::Crypto("secure session poisoned".to_string()))?;
            guard.decrypt_message(&body)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ClipboardContent, ClipboardPacket, Hello};
    use tokio::io::duplex;

    async fn established() -> (Encryption, Encryption) {
        let (mut client, mut server) = duplex(1 << 20);
        let server_task = tokio::spawn(async move {
            let session = server_handshake(&mut server, "swordfish").await.unwrap();
            (Encryption::secure(session), server)
        });
        let client_session = client_handshake(&mut client, "swordfish").await.unwrap();
        let (server_enc, _server_stream) = server_task.await.unwrap();
        // Keep both stream halves alive by leaking them into the returned tuple
        // is unnecessary here; tests below use fresh duplexes for I/O.
        let _ = (client, _server_stream);
        (Encryption::secure(client_session), server_enc)
    }

    #[tokio::test]
    async fn handshake_with_matching_secret_succeeds() {
        let (client_enc, server_enc) = established().await;
        assert!(client_enc.is_secure());
        assert!(server_enc.is_secure());
    }

    #[tokio::test]
    async fn mismatched_secret_fails_handshake() {
        let (mut client, mut server) = duplex(4096);
        let server_task =
            tokio::spawn(async move { server_handshake(&mut server, "correct").await.is_ok() });
        let client_ok = client_handshake(&mut client, "wrong").await.is_ok();
        let server_ok = server_task.await.unwrap();
        assert!(
            !(client_ok && server_ok),
            "mismatched secrets must not both succeed"
        );
    }

    #[tokio::test]
    async fn encrypted_round_trip_small_and_large() {
        // Build a real connected pair and exchange messages both directions.
        let (mut client, mut server) = duplex(1 << 22);
        let server_task = tokio::spawn(async move {
            let session = server_handshake(&mut server, "shared").await.unwrap();
            (Encryption::secure(session), server)
        });
        let client_session = client_handshake(&mut client, "shared").await.unwrap();
        let client_enc = Encryption::secure(client_session);
        let (server_enc, mut server) = server_task.await.unwrap();

        // Small control message client -> server.
        let hello = Message::Hello(Hello::client("mac"));
        send(&mut client, &client_enc, &hello).await.unwrap();
        assert_eq!(recv(&mut server, &server_enc).await.unwrap(), hello);

        // Large clipboard payload exceeding a single Noise record, server -> client.
        let big = "x".repeat(500_000);
        let clip = Message::Clipboard(ClipboardPacket {
            seq: 1,
            sent_at_ms: 42,
            content: ClipboardContent::Text { text: big },
        });
        send(&mut server, &server_enc, &clip).await.unwrap();
        assert_eq!(recv(&mut client, &client_enc).await.unwrap(), clip);

        // The plaintext must not appear on the wire: send and capture bytes.
        let (mut a, mut b) = duplex(4096);
        let secret_msg = Message::Hello(Hello::client("topsecretscreen"));
        send(&mut a, &client_enc, &secret_msg).await.unwrap();
        let mut buf = vec![0u8; 256];
        let n = b.read(&mut buf).await.unwrap();
        assert!(
            !buf[..n].windows(15).any(|w| w == b"topsecretscreen"),
            "screen name leaked in ciphertext"
        );
    }

    #[tokio::test]
    async fn plain_encryption_matches_legacy_wire_format() {
        let (mut a, mut b) = duplex(4096);
        let msg = Message::Hello(Hello::client("mac"));
        send(&mut a, &Encryption::Plain, &msg).await.unwrap();
        // A legacy read_frame must still decode it.
        assert_eq!(read_frame(&mut b).await.unwrap(), msg);
    }
}
