//! Length-prefixed JSON framing shared by host-service protocols.
//!
//! Every host-service passenger (`db`, `credential`, …) and the multiplexed
//! `Open` handshake use the same wire format: a 4-byte little-endian length
//! followed by the JSON body. The helpers here are generic over any serde
//! type so the `Open` framing in [`crate::host_service`] and the session
//! framing in [`crate::credential`] stay symmetric.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Maximum framed message size on the host-service channel (64 MiB).
pub const MAX_FRAMED_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

/// Writes a length-prefixed JSON frame.
pub async fn write_framed_json<W, T>(writer: &mut W, value: &T) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let json = serde_json::to_vec(value).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("serialize failed: {e}"),
        )
    })?;
    let Ok(len) = u32::try_from(json.len()) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "message too large to frame",
        ));
    };
    writer.write_all(&len.to_le_bytes()).await?;
    writer.write_all(&json).await?;
    writer.flush().await?;
    Ok(())
}

/// Reads a length-prefixed JSON frame.
///
/// Returns `Ok(None)` when the peer closes cleanly before sending a frame and
/// an error when the framing is malformed or the payload exceeds
/// [`MAX_FRAMED_MESSAGE_SIZE`].
pub async fn read_framed_json<R, T>(reader: &mut R) -> std::io::Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let Ok(msg_len) = usize::try_from(u32::from_le_bytes(len_buf)) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "message length overflow on this platform",
        ));
    };
    if msg_len > MAX_FRAMED_MESSAGE_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("message too large: {msg_len}"),
        ));
    }
    let mut msg_buf = vec![0u8; msg_len];
    reader.read_exact(&mut msg_buf).await?;
    let value = serde_json::from_slice(&msg_buf).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid JSON: {e}"),
        )
    })?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn framed_json_roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        let value = serde_json::json!({"hello": "world", "n": 42});
        write_framed_json(&mut a, &value).await.unwrap();
        drop(a);
        let got: serde_json::Value = read_framed_json(&mut b).await.unwrap().unwrap();
        assert_eq!(got, value);
    }

    #[tokio::test]
    async fn clean_eof_returns_none() {
        let (a, mut b) = tokio::io::duplex(4096);
        drop(a);
        let got: Option<serde_json::Value> = read_framed_json(&mut b).await.unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn oversize_frame_is_rejected() {
        let (mut a, mut b) = tokio::io::duplex(8192);
        let big = vec![0u8; MAX_FRAMED_MESSAGE_SIZE + 1];
        let len = u32::try_from(big.len()).unwrap();
        a.write_all(&len.to_le_bytes()).await.unwrap();
        drop(a);
        let result: std::io::Result<Option<serde_json::Value>> = read_framed_json(&mut b).await;
        assert!(result.is_err());
    }
}
