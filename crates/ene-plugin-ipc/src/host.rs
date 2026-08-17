use crate::error::IpcError;
use crate::frame::{MAX_FRAME_BYTES, read_frame, write_frame};
use crate::protocol::{
    HelloAck, HostHello, Message, Negotiated, ProtoId, ProtocolRanges, ToolCall, ToolResult,
    ToolSpecWire, VersionRange,
};
use tokio::io::{AsyncRead, AsyncWrite};

/// Host side of a negotiated plugin connection.
pub struct HostConn<S> {
    stream: S,
    next_id: u64,
    negotiated: Negotiated,
    max_frame: usize,
}

impl<S> std::fmt::Debug for HostConn<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostConn")
            .field("next_id", &self.next_id)
            .field("negotiated", &self.negotiated)
            .field("max_frame", &self.max_frame)
            .finish_non_exhaustive()
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> HostConn<S> {
    /// Send hello and wait for ack. Core mismatch is fatal; tool mismatch disables tools.
    pub async fn handshake(
        mut stream: S,
        hello: HostHello,
        plugin_declared: &[ProtoId],
        expected_spawn_token: &str,
    ) -> Result<Self, IpcError> {
        write_frame(
            &mut stream,
            &Message::Hello {
                body: hello.clone(),
            }
            .encode()?,
            MAX_FRAME_BYTES,
        )
        .await?;
        let bytes = read_frame(&mut stream, MAX_FRAME_BYTES).await?;
        match Message::decode(&bytes)? {
            Message::HelloAck { body } => {
                validate_ack(&hello, &body, plugin_declared, expected_spawn_token)?;
                Ok(Self {
                    stream,
                    next_id: 1,
                    negotiated: body.protocols,
                    max_frame: MAX_FRAME_BYTES,
                })
            }
            Message::HelloReject { body } => Err(IpcError::Rejected(body.reason)),
            other => Err(IpcError::Unexpected(other.kind_name().to_owned())),
        }
    }

    #[must_use]
    pub fn negotiated(&self) -> &Negotiated {
        &self.negotiated
    }

    pub async fn ping(&mut self) -> Result<u64, IpcError> {
        let id = self.alloc();
        self.send(&Message::Ping { id }).await?;
        match self.recv().await? {
            Message::Pong { id: got } if got == id => Ok(id),
            other => Err(IpcError::Unexpected(other.kind_name().to_owned())),
        }
    }

    pub async fn list_tools(&mut self) -> Result<Vec<ToolSpecWire>, IpcError> {
        if self.negotiated.tool.is_none() {
            return Ok(Vec::new());
        }
        let id = self.alloc();
        self.send(&Message::ToolList { id }).await?;
        match self.recv().await? {
            Message::ToolSpec { id: got, tools } if got == id => Ok(tools),
            other => Err(IpcError::Unexpected(other.kind_name().to_owned())),
        }
    }

    pub async fn call_tool(&mut self, call: ToolCall) -> Result<ToolResult, IpcError> {
        if self.negotiated.tool.is_none() {
            return Err(IpcError::Unexpected("tool disabled".to_owned()));
        }
        let id = self.alloc();
        self.send(&Message::ToolCall { id, body: call }).await?;
        match self.recv().await? {
            Message::ToolResult { id: got, body } if got == id => Ok(body),
            other => Err(IpcError::Unexpected(other.kind_name().to_owned())),
        }
    }

    pub async fn drain(&mut self) -> Result<(), IpcError> {
        let id = self.alloc();
        self.send(&Message::Drain { id }).await?;
        match self.recv().await? {
            Message::DrainAck { id: got } if got == id => Ok(()),
            other => Err(IpcError::Unexpected(other.kind_name().to_owned())),
        }
    }

    async fn send(&mut self, message: &Message) -> Result<(), IpcError> {
        write_frame(&mut self.stream, &message.encode()?, self.max_frame).await
    }

    async fn recv(&mut self) -> Result<Message, IpcError> {
        let bytes = read_frame(&mut self.stream, self.max_frame).await?;
        Message::decode(&bytes)
    }

    fn alloc(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }
}

pub(crate) fn validate_ack(
    hello: &HostHello,
    ack: &HelloAck,
    declared: &[ProtoId],
    expected_spawn_token: &str,
) -> Result<(), IpcError> {
    if ack.manifest_digest != hello.expected_digest {
        return Err(IpcError::DigestMismatch);
    }
    if ack.spawn_token != expected_spawn_token {
        return Err(IpcError::DigestMismatch);
    }
    if ack.protocols.tool.is_some() && !declared.contains(&ProtoId::Tool) {
        return Err(IpcError::UndeclaredProtocol("tool".to_owned()));
    }
    Ok(())
}

/// Negotiate versions. Core mismatch is an error; tool mismatch disables the face.
pub fn negotiate(
    host: &ProtocolRanges,
    plugin_core: VersionRange,
    plugin_tool: Option<VersionRange>,
) -> Result<Negotiated, IpcError> {
    let core = host
        .core
        .negotiate(plugin_core)
        .ok_or(IpcError::CoreIncompatible)?;
    let tool = match (host.tool, plugin_tool) {
        (Some(host_tool), Some(plugin_tool)) => host_tool.negotiate(plugin_tool),
        _ => None,
    };
    Ok(Negotiated { core, tool })
}
