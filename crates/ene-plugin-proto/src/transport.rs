use std::io;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Cross-platform IPC stream.
///
/// On Unix: wraps `UnixStream`
/// On Windows: wraps `NamedPipeServer` (server-end) or `NamedPipeClient` (client-end)
pub enum IpcStream {
    /// Unix domain socket stream (Unix only).
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
    /// Named pipe server end (Windows only).
    #[cfg(windows)]
    Server(tokio::net::windows::named_pipe::NamedPipeServer),
    /// Named pipe client end (Windows only).
    #[cfg(windows)]
    Client(tokio::net::windows::named_pipe::NamedPipeClient),
}

impl IpcStream {
    /// Connect to a listening IPC endpoint.
    #[cfg(unix)]
    pub async fn connect(path: &Path) -> io::Result<Self> {
        let stream = tokio::net::UnixStream::connect(path).await?;
        Ok(Self::Unix(stream))
    }

    #[cfg(windows)]
    /// Connect to a Windows named pipe server.
    pub async fn connect(path: &Path) -> io::Result<Self> {
        use tokio::net::windows::named_pipe::ClientOptions;
        let path_str = path.to_string_lossy();
        let client = ClientOptions::new().open(path_str.as_ref())?;
        Ok(IpcStream::Client(client))
    }
}

impl AsyncRead for IpcStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(unix)]
            Self::Unix(s) => Pin::new(s).poll_read(cx, buf),
            #[cfg(windows)]
            Self::Server(s) => Pin::new(s).poll_read(cx, buf),
            #[cfg(windows)]
            Self::Client(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for IpcStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            #[cfg(unix)]
            Self::Unix(s) => Pin::new(s).poll_write(cx, buf),
            #[cfg(windows)]
            Self::Server(s) => Pin::new(s).poll_write(cx, buf),
            #[cfg(windows)]
            Self::Client(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(unix)]
            Self::Unix(s) => Pin::new(s).poll_flush(cx),
            #[cfg(windows)]
            Self::Server(s) => Pin::new(s).poll_flush(cx),
            #[cfg(windows)]
            Self::Client(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(unix)]
            Self::Unix(s) => Pin::new(s).poll_shutdown(cx),
            #[cfg(windows)]
            Self::Server(s) => Pin::new(s).poll_shutdown(cx),
            #[cfg(windows)]
            Self::Client(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// Cross-platform IPC listener.
///
/// On Unix: wraps `UnixListener` (shared via `&self` accept)
/// On Windows: wraps `NamedPipeServer` (one instance at a time, recreated after accept)
pub enum IpcListener {
    /// Unix domain socket listener (Unix only).
    #[cfg(unix)]
    Unix(tokio::net::UnixListener),
    /// Named pipe listener (Windows only).
    #[cfg(windows)]
    Pipe {
        /// The current pipe server instance.
        current: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
        /// Name of the pipe.
        pipe_name: String,
    },
}

impl IpcListener {
    /// Bind to an IPC endpoint.
    ///
    /// On Unix, if the initial bind fails with `AddrInUse` (typically a
    /// stale socket file left by a crashed server), the socket file is
    /// removed and the bind is retried once. This closes the TOCTOU gap
    /// between the caller's `cleanup_path` and the actual bind.
    pub fn bind(path: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            match tokio::net::UnixListener::bind(path) {
                Ok(listener) => Ok(IpcListener::Unix(listener)),
                Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
                    // Stale socket file — remove and retry once.
                    let _ = std::fs::remove_file(path);
                    tokio::net::UnixListener::bind(path).map(IpcListener::Unix)
                }
                Err(e) => Err(e),
            }
        }
        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ServerOptions;
            let pipe_name = path.to_string_lossy().to_string();
            let server = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&pipe_name)?;
            Ok(IpcListener::Pipe {
                current: Some(server),
                pipe_name,
            })
        }
    }

    /// Accept a new connection.
    ///
    /// On Unix: delegates to `UnixListener::accept`.
    /// On Windows: connects the current pipe instance and creates a new instance
    /// for the next connection.
    pub async fn accept(&mut self) -> io::Result<IpcStream> {
        match self {
            #[cfg(unix)]
            Self::Unix(listener) => {
                let (stream, _) = listener.accept().await?;
                Ok(IpcStream::Unix(stream))
            }
            #[cfg(windows)]
            IpcListener::Pipe { current, pipe_name } => {
                use tokio::net::windows::named_pipe::ServerOptions;
                let server = current.take().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotConnected,
                        "no available pipe server instance",
                    )
                })?;
                let next = ServerOptions::new().create(pipe_name)?;
                *current = Some(next);
                server.connect().await?;
                Ok(IpcStream::Server(server))
            }
        }
    }
}

/// Clean up a server socket path.
///
/// On Unix: removes the socket file.
/// On Windows: no-op (named pipes are not filesystem objects).
pub fn cleanup_path(path: &Path) {
    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(path);
    }
    #[cfg(windows)]
    {
        let _ = path;
    }
}
