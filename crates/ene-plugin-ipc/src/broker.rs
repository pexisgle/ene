//! Plugin-facing broker request/response codec.

use crate::frame::{read_frame, write_frame};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};

/// Client for the host broker socket injected into a plugin process.
#[derive(Debug)]
pub struct BrokerClient<S> {
    stream: S,
}

#[cfg(unix)]
impl BrokerClient<tokio::net::UnixStream> {
    /// Connect to `ENE_BROKER_SOCKET`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::IpcError::Io`] when the env is missing or connect fails.
    pub async fn from_env() -> Result<Self, crate::IpcError> {
        let path = std::env::var("ENE_BROKER_SOCKET").map_err(|_| {
            crate::IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "ENE_BROKER_SOCKET is not set",
            ))
        })?;
        Self::connect(&path).await
    }

    pub async fn connect(path: &str) -> Result<Self, crate::IpcError> {
        Ok(Self {
            stream: tokio::net::UnixStream::connect(path).await?,
        })
    }

    /// Connect to an explicit broker endpoint, primarily for host tests.
    ///
    /// # Errors
    ///
    /// Returns [`crate::IpcError::Io`] when connect fails.
    pub async fn from_path(path: &str) -> Result<Self, crate::IpcError> {
        Self::connect(path).await
    }
}

#[cfg(windows)]
impl BrokerClient<tokio::net::TcpStream> {
    /// Connect to `ENE_BROKER_SOCKET`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::IpcError::Io`] when the env is missing or connect fails.
    pub async fn from_env() -> Result<Self, crate::IpcError> {
        let path = std::env::var("ENE_BROKER_SOCKET").map_err(|_| {
            crate::IpcError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "ENE_BROKER_SOCKET is not set",
            ))
        })?;
        Self::from_path(&path).await
    }

    /// Connect to an explicit broker endpoint, primarily for host tests.
    ///
    /// # Errors
    ///
    /// Returns [`crate::IpcError::Io`] when connect fails.
    pub async fn from_path(path: &str) -> Result<Self, crate::IpcError> {
        Ok(Self {
            stream: tokio::net::TcpStream::connect(path).await?,
        })
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> BrokerClient<S> {
    /// Send one request and wait for its response.
    ///
    /// # Errors
    ///
    /// Returns an IPC error on transport or codec failures.
    pub async fn call(
        &mut self,
        request: BrokerRequest,
    ) -> Result<BrokerResponse, crate::IpcError> {
        write_broker_request(&mut self.stream, request).await?;
        read_broker_response(&mut self.stream).await
    }
}

/// Machine-readable broker failure classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokerErrorCode {
    Denied,
    PathEscape,
    Io,
    InvalidUrl,
    Ssrf,
    Fetch,
    InvalidArgument,
    Internal,
}

/// One host-broker operation requested by a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrokerRequest {
    Hello {
        token: String,
    },
    FsRead {
        path: String,
    },
    FsWrite {
        path: String,
        text: String,
    },
    FsSearch {
        path: String,
        query: String,
        regex: bool,
        case_insensitive: bool,
        include: Option<String>,
        context_lines: u32,
        count: bool,
        max: u32,
    },
    NetFetch {
        url: String,
    },
}

/// Broker response for [`BrokerRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrokerResponse {
    HelloOk,
    FsReadOk {
        text: String,
    },
    FsWriteOk,
    FsSearchOk {
        matches: serde_json::Value,
    },
    NetFetchOk {
        value: serde_json::Value,
    },
    Error {
        code: BrokerErrorCode,
        message: String,
    },
}

/// Read one length-prefixed broker request. `Ok(None)` means disconnect.
///
/// # Errors
///
/// Returns an IPC error on frame or `MessagePack` decoding failures.
pub async fn read_broker_request<S: AsyncRead + Unpin>(
    stream: &mut S,
) -> Result<Option<BrokerRequest>, crate::IpcError> {
    let bytes = match read_frame(stream, crate::MAX_FRAME_BYTES).await {
        Ok(bytes) => bytes,
        Err(crate::IpcError::Closed) => return Ok(None),
        Err(err) => return Err(err),
    };
    Ok(Some(
        rmp_serde::from_slice(&bytes).map_err(crate::IpcError::codec)?,
    ))
}

/// Write one broker response with the shared frame cap.
///
/// # Errors
///
/// Returns an IPC error when encoding or writing fails.
pub async fn write_broker_request<S: AsyncWrite + Unpin>(
    stream: &mut S,
    request: BrokerRequest,
) -> Result<(), crate::IpcError> {
    let bytes = rmp_serde::to_vec_named(&request).map_err(crate::IpcError::codec)?;
    write_frame(stream, &bytes, crate::MAX_FRAME_BYTES).await
}

pub async fn write_broker_response<S: AsyncWrite + Unpin>(
    stream: &mut S,
    response: BrokerResponse,
) -> Result<(), crate::IpcError> {
    let bytes = rmp_serde::to_vec_named(&response).map_err(crate::IpcError::codec)?;
    write_frame(stream, &bytes, crate::MAX_FRAME_BYTES).await
}

pub async fn read_broker_response<S: AsyncRead + Unpin>(
    stream: &mut S,
) -> Result<BrokerResponse, crate::IpcError> {
    let bytes = match read_frame(stream, crate::MAX_FRAME_BYTES).await {
        Ok(bytes) => bytes,
        Err(crate::IpcError::Closed) => {
            return Err(crate::IpcError::Closed);
        }
        Err(err) => return Err(err),
    };
    rmp_serde::from_slice(&bytes).map_err(crate::IpcError::codec)
}
