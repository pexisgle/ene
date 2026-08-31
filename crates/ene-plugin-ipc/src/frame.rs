use crate::error::IpcError;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Default frame cap. Hitting it is a design failure; bulk goes out-of-band.
pub const MAX_FRAME_BYTES: usize = 1_048_576;

/// Resolve `plugins.ipc.max_frame_bytes`. `0` keeps the compile-time default.
#[must_use]
pub fn frame_limit(configured: u32) -> usize {
    if configured == 0 {
        MAX_FRAME_BYTES
    } else {
        usize::try_from(configured).unwrap_or(MAX_FRAME_BYTES)
    }
}

/// Write one length-prefixed (32-bit BE) frame.
///
/// Callers pass a generic [`AsyncWrite`], often a half of a Unix/TCP stream
/// shared with other IPC logic. `tokio_util::codec::LengthDelimitedCodec`
/// wants a `Framed` that owns the I/O and defaults to an 8-byte little-endian
/// length that includes the header — more adapter code than this file, for
/// the same 4-byte BE exclusive-length wire.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    payload: &[u8],
    max: usize,
) -> Result<(), IpcError> {
    if payload.len() > max {
        return Err(IpcError::FrameTooLarge {
            got: payload.len(),
            max,
        });
    }
    let len = u32::try_from(payload.len()).map_err(|_| IpcError::FrameTooLarge {
        got: payload.len(),
        max,
    })?;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(payload).await?;
    writer.flush().await?;
    Ok(())
}

/// Read one length-prefixed frame.
///
/// Oversize lengths fail before the payload buffer is allocated. Unexpected
/// EOF on the 4-byte header is [`IpcError::Closed`], which host/plugin loops
/// treat as a clean peer drop rather than a generic I/O error.
pub async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    max: usize,
) -> Result<Vec<u8>, IpcError> {
    let mut header = [0_u8; 4];
    if let Err(err) = reader.read_exact(&mut header).await {
        if err.kind() == std::io::ErrorKind::UnexpectedEof {
            return Err(IpcError::Closed);
        }
        return Err(err.into());
    }
    let len = usize::try_from(u32::from_be_bytes(header)).map_err(|_| IpcError::FrameTooLarge {
        got: usize::MAX,
        max,
    })?;
    if len > max {
        return Err(IpcError::FrameTooLarge { got: len, max });
    }
    let mut buf = vec![0_u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::{read_frame, write_frame};
    use crate::IpcError;
    use tokio::io::{AsyncWriteExt, duplex};

    #[tokio::test]
    async fn round_trip_is_four_byte_big_endian_exclusive_length() {
        let (mut client, mut server) = duplex(64);
        write_frame(&mut client, b"hi", 16).await.unwrap();
        drop(client);
        let mut header = [0_u8; 4];
        tokio::io::AsyncReadExt::read_exact(&mut server, &mut header)
            .await
            .unwrap();
        assert_eq!(header, 2_u32.to_be_bytes());
        let mut payload = [0_u8; 2];
        tokio::io::AsyncReadExt::read_exact(&mut server, &mut payload)
            .await
            .unwrap();
        assert_eq!(&payload, b"hi");
    }

    #[tokio::test]
    async fn oversize_header_fails_before_allocating_payload() {
        let (mut client, mut server) = duplex(16);
        client.write_all(&100_u32.to_be_bytes()).await.unwrap();
        drop(client);
        let err = read_frame(&mut server, 8).await.unwrap_err();
        assert!(
            matches!(err, IpcError::FrameTooLarge { got: 100, max: 8 }),
            "{err}"
        );
    }

    #[tokio::test]
    async fn eof_on_header_is_closed() {
        let (client, mut server) = duplex(8);
        drop(client);
        let err = read_frame(&mut server, 8).await.unwrap_err();
        assert!(matches!(err, IpcError::Closed), "{err}");
    }

    #[tokio::test]
    async fn write_rejects_payload_over_max() {
        let (mut client, _server) = duplex(8);
        let err = write_frame(&mut client, &[0_u8; 9], 8).await.unwrap_err();
        assert!(
            matches!(err, IpcError::FrameTooLarge { got: 9, max: 8 }),
            "{err}"
        );
    }
}
