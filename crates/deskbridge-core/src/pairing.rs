//! Interactive device pairing.
//!
//! Typing the same long secret on two machines is error-prone, so pairing turns
//! it into a one-tap flow: the two devices run an anonymous Noise handshake,
//! each shows a short numeric code derived from the handshake's channel binding,
//! and the user confirms the codes match. A man-in-the-middle relaying two
//! separate handshakes would produce *different* codes on each leg, so the
//! comparison authenticates the channel (the same model as Bluetooth numeric
//! comparison). Once confirmed, the host sends a freshly generated strong secret
//! over the now-trusted channel; both sides persist it and use it for all later
//! encrypted sessions.

use crate::codec::FrameError;
use crate::secure::{
    pairing_handshake_initiator, pairing_handshake_responder, read_raw, write_raw,
};
use tokio::io::{AsyncRead, AsyncWrite};

/// Number of digits in the comparison code shown to the user.
pub const SAS_DIGITS: u32 = 6;

/// Length in bytes of the secret minted during pairing.
const PAIRED_SECRET_BYTES: usize = 32;

/// The outcome of a successful pairing: the shared secret to persist on both
/// devices (hex-encoded so it drops straight into `security.psk`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingResult {
    pub secret: String,
}

/// Derive the human-comparable short authentication string from the handshake
/// hash. Both peers compute the same value from the same channel binding.
pub fn sas_from_hash(hash: &[u8; 32]) -> String {
    let modulus = 10u32.pow(SAS_DIGITS);
    let value = u32::from_be_bytes([hash[0], hash[1], hash[2], hash[3]]) % modulus;
    // Render as a zero-padded, space-grouped code, e.g. "042 137".
    let digits = format!("{value:0width$}", width = SAS_DIGITS as usize);
    let (left, right) = digits.split_at(digits.len() / 2);
    format!("{left} {right}")
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Pair as the joining device (initiator). `confirm` is shown the SAS and must
/// return `true` for pairing to continue.
pub async fn pair_join<S, F>(stream: &mut S, confirm: F) -> Result<PairingResult, FrameError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: FnOnce(&str) -> bool,
{
    let (mut session, hash) = pairing_handshake_initiator(stream).await?;
    let sas = sas_from_hash(&hash);
    if !confirm(&sas) {
        return Err(FrameError::Crypto(
            "pairing rejected at this device".to_string(),
        ));
    }
    let record = read_raw(stream).await?;
    let secret = session.decrypt_raw(&record)?;
    if secret.len() != PAIRED_SECRET_BYTES {
        return Err(FrameError::Crypto(
            "unexpected paired secret length".to_string(),
        ));
    }
    Ok(PairingResult {
        secret: hex_encode(&secret),
    })
}

/// Pair as the hosting device (responder). On confirmation, mint and send a
/// strong secret over the confidential channel.
pub async fn pair_host<S, F>(stream: &mut S, confirm: F) -> Result<PairingResult, FrameError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: FnOnce(&str) -> bool,
{
    let (mut session, hash) = pairing_handshake_responder(stream).await?;
    let sas = sas_from_hash(&hash);
    if !confirm(&sas) {
        return Err(FrameError::Crypto(
            "pairing rejected at this device".to_string(),
        ));
    }
    let secret = random_secret()?;
    let record = session.encrypt_raw(&secret)?;
    write_raw(stream, &record).await?;
    Ok(PairingResult {
        secret: hex_encode(&secret),
    })
}

fn random_secret() -> Result<[u8; PAIRED_SECRET_BYTES], FrameError> {
    let mut secret = [0u8; PAIRED_SECRET_BYTES];
    getrandom::fill(&mut secret)
        .map_err(|err| FrameError::Crypto(format!("failed to generate secret: {err}")))?;
    Ok(secret)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn pairing_agrees_on_secret_and_code() {
        let (mut join_side, mut host_side) = duplex(1 << 16);

        let host = tokio::spawn(async move { pair_host(&mut host_side, |_sas| true).await });
        let join = pair_join(&mut join_side, |_sas| true).await.unwrap();
        let host = host.await.unwrap().unwrap();

        // Both ends derive the same persisted secret.
        assert_eq!(join.secret, host.secret);
        assert_eq!(join.secret.len(), PAIRED_SECRET_BYTES * 2);
    }

    #[tokio::test]
    async fn both_ends_compute_the_same_sas() {
        // Capture the SAS each side shows; without a MITM they must match.
        let (mut join_side, mut host_side) = duplex(1 << 16);
        let (tx, rx) = std::sync::mpsc::channel();
        let host_tx = tx.clone();

        let host = tokio::spawn(async move {
            pair_host(&mut host_side, move |sas| {
                host_tx.send(("host", sas.to_string())).unwrap();
                true
            })
            .await
        });
        pair_join(&mut join_side, move |sas| {
            tx.send(("join", sas.to_string())).unwrap();
            true
        })
        .await
        .unwrap();
        host.await.unwrap().unwrap();

        let a = rx.recv().unwrap();
        let b = rx.recv().unwrap();
        assert_eq!(a.1, b.1, "both devices must show the same code");
    }

    #[tokio::test]
    async fn rejecting_the_code_fails_pairing() {
        let (mut join_side, mut host_side) = duplex(1 << 16);
        let host = tokio::spawn(async move { pair_host(&mut host_side, |_sas| true).await });
        let join = pair_join(&mut join_side, |_sas| false).await;
        assert!(join.is_err());
        let _ = host.await.unwrap();
    }

    #[tokio::test]
    async fn mitm_relay_makes_the_two_codes_differ() {
        // join <-> relay <-> host. A relay can only forward by running two
        // independent handshakes, which have different ephemeral keys and thus
        // different channel bindings — so the two devices show different codes
        // and a user comparing them detects the attack.
        let (mut join_side, mut relay_to_join) = duplex(1 << 16);
        let (mut relay_to_host, mut host_side) = duplex(1 << 16);

        let host = tokio::spawn(async move {
            crate::secure::pairing_handshake_responder(&mut host_side)
                .await
                .unwrap()
                .1
        });
        let join = tokio::spawn(async move {
            crate::secure::pairing_handshake_initiator(&mut join_side)
                .await
                .unwrap()
                .1
        });
        let relay_join = tokio::spawn(async move {
            crate::secure::pairing_handshake_responder(&mut relay_to_join)
                .await
                .unwrap();
        });
        let relay_host = tokio::spawn(async move {
            crate::secure::pairing_handshake_initiator(&mut relay_to_host)
                .await
                .unwrap();
        });

        let host_hash = host.await.unwrap();
        let join_hash = join.await.unwrap();
        relay_join.await.unwrap();
        relay_host.await.unwrap();

        assert_ne!(
            host_hash, join_hash,
            "a relayed handshake must not share a channel binding"
        );
        assert_ne!(sas_from_hash(&host_hash), sas_from_hash(&join_hash));
    }

    #[test]
    fn sas_is_grouped_six_digits() {
        let sas = sas_from_hash(&[
            0x00, 0x01, 0x86, 0xa0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0,
        ]);
        // 0x000186a0 = 100000 -> mod 1_000_000 = 100000 -> "100 000"
        assert_eq!(sas, "100 000");
    }
}
