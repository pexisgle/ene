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
