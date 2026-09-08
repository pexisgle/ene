use crate::error::IpcError;
use crate::frame::{MAX_FRAME_BYTES, read_frame, write_frame};
use crate::host::negotiate;
use crate::protocol::{
    CORE_VERSION, HelloAck, HostHello, Message, ProtoId, TOOL_VERSION, ToolBackgroundCancelAck,
    ToolBackgroundStarted, ToolBackgroundStatusResult, ToolExecutionComplete, ToolResult,
    ToolSpecWire, VersionRange,
};
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::{Duration, sleep};

/// Tool surface implemented by a plugin process (or an in-process stand-in).
#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn plugin_id(&self) -> &str;
    fn plugin_name(&self) -> &str;
    fn digest(&self) -> &str;
    fn specs(&self) -> Vec<ToolSpecWire>;
    async fn call(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, IpcError>;
    fn spawn_token(&self) -> Result<String, String> {
        std::env::var("ENE_PLUGIN_SPAWN_TOKEN")
            .map_err(|_| "ENE_PLUGIN_SPAWN_TOKEN is not set".to_owned())
    }
    fn has_config(&self) -> bool {
        false
    }

    async fn config_schema(&self) -> Result<crate::PluginConfigSchema, IpcError> {
        Ok(crate::PluginConfigSchema::default())
    }

    async fn config_validate(
        &self,
        _values: serde_json::Value,
    ) -> Result<crate::PluginConfigValidateResult, IpcError> {
        Ok(crate::PluginConfigValidateResult::ok())
    }

    async fn config_options(
        &self,
        _field: &str,
    ) -> Result<crate::PluginConfigOptionsResult, IpcError> {
        Ok(crate::PluginConfigOptionsResult::unsupported())
    }

    async fn config_apply(
        &self,
        _values: serde_json::Value,
    ) -> Result<crate::PluginConfigApplyResult, IpcError> {
        Ok(crate::PluginConfigApplyResult::ok(false))
    }
    async fn start_background(
        &self,
        _execution_id: &str,
        name: &str,
        _args: serde_json::Value,
        _deadline_ms: Option<u64>,
    ) -> Result<(), IpcError> {
        Err(IpcError::UnknownTool(name.to_owned()))
    }
    fn cancel_background(&self, execution_id: &str) -> ToolBackgroundCancelAck {
        ToolBackgroundCancelAck {
            execution_id: execution_id.to_owned(),
            status: "unknown".to_owned(),
        }
    }
    fn status_background(&self, execution_id: &str) -> ToolBackgroundStatusResult {
        ToolBackgroundStatusResult {
            execution_id: execution_id.to_owned(),
            phase: "unknown".to_owned(),
            error_class: Some("unknown_execution".to_owned()),
        }
    }
    fn poll_background(&self) -> Vec<ToolExecutionComplete> {
        Vec::new()
    }
}

/// Which bundled tool set a harness plugin binary serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinKind {
    Fs,
    Exec,
    Web,
    Utility,
    App,
}

impl BuiltinKind {
    #[must_use]
    pub const fn plugin_id(self) -> &'static str {
        match self {
            Self::Fs => "tool.fs",
            Self::Exec => "tool.exec",
            Self::Web => "tool.web",
            Self::Utility => "tool.utility",
            Self::App => "tool.app",
        }
    }
}

/// Serve one plugin connection until drain/shutdown.
pub async fn serve_plugin<S, H>(mut stream: S, handler: H) -> Result<(), IpcError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    H: ToolHandler,
{
    let hello = expect_hello(&mut stream).await?;
    let max_frame = hello.frame_limit();
    let ack = match build_ack(&hello, &handler) {
        Ok(ack) => ack,
        Err(reason) => {
            write_msg(
                &mut stream,
                &Message::HelloReject {
                    body: crate::protocol::HelloReject {
                        reason: reason.clone(),
                    },
                },
                max_frame,
            )
            .await?;
            return Err(IpcError::Rejected(reason));
        }
    };
    write_msg(&mut stream, &Message::HelloAck { body: ack }, max_frame).await?;
    loop {
        tokio::select! {
            biased;
            result = read_frame(&mut stream, max_frame) => {
                let bytes = result?;
                flush_background(&mut stream, &handler, max_frame).await?;
                match Message::decode(&bytes)? {
                    Message::Ping { id } => {
                        write_msg(&mut stream, &Message::Pong { id }, max_frame).await?;
                    }
                    Message::ToolList { id } => {
                        write_msg(
                            &mut stream,
                            &Message::ToolSpec {
                                id,
                                tools: handler.specs(),
                            },
                            max_frame,
                        )
                        .await?;
                    }
                    Message::ToolCall { id, body } => {
                        let result = match handler.call(&body.tool_name, body.args).await {
                            Ok(value) => ToolResult {
                                call_id: body.call_id,
                                status: "ok".to_owned(),
                                value,
                            },
                            Err(err) => ToolResult {
                                call_id: body.call_id,
                                status: "error".to_owned(),
                                value: serde_json::json!({ "error": err.to_string() }),
                            },
                        };
                        write_msg(
                            &mut stream,
                            &Message::ToolResult { id, body: result },
                            max_frame,
                        )
                        .await?;
                    }
                    Message::ToolCancel { id, .. } => {
                        write_msg(
                            &mut stream,
                            &Message::ToolResult {
                                id,
                                body: ToolResult {
                                    call_id: String::new(),
                                    status: "cancelled".to_owned(),
                                    value: serde_json::Value::Null,
                                },
                            },
                            max_frame,
                        )
                        .await?;
                    }
                    Message::ToolBackgroundStart { id, body } => {
                        let capable = handler
                            .specs()
                            .iter()
                            .any(|spec| spec.name == body.tool_name && spec.background);
                        let started = if capable {
                            match handler
                                .start_background(
                                    &body.execution_id,
                                    &body.tool_name,
                                    body.args,
                                    body.deadline_ms,
                                )
                                .await
                            {
                                Ok(()) => ToolBackgroundStarted {
                                    execution_id: body.execution_id,
                                    accepted: true,
                                    error: None,
                                },
                                Err(err) => ToolBackgroundStarted {
                                    execution_id: body.execution_id,
                                    accepted: false,
                                    error: Some(err.to_string()),
                                },
                            }
                        } else {
                            ToolBackgroundStarted {
                                execution_id: body.execution_id,
                                accepted: false,
                                error: Some("not background-capable".to_owned()),
                            }
                        };
                        write_msg(
                            &mut stream,
                            &Message::ToolBackgroundStarted { id, body: started },
                            max_frame,
                        )
                        .await?;
                    }
                    Message::ToolBackgroundCancel { id, execution_id } => {
                        let body = handler.cancel_background(&execution_id);
                        write_msg(
                            &mut stream,
                            &Message::ToolBackgroundCancelAck { id, body },
                            max_frame,
                        )
                        .await?;
                    }
                    Message::ToolBackgroundStatus { id, execution_id } => {
                        let body = handler.status_background(&execution_id);
                        write_msg(
                            &mut stream,
                            &Message::ToolBackgroundStatusResult { id, body },
                            max_frame,
                        )
                        .await?;
                    }
                    Message::Drain { id } | Message::Shutdown { id } => {
                        flush_background(&mut stream, &handler, max_frame).await?;
                        write_msg(&mut stream, &Message::DrainAck { id }, max_frame).await?;
                        return Ok(());
                    }
                    Message::StreamOpen { id, body } => {
                        write_msg(
                            &mut stream,
                            &Message::StreamOpened {
                                id,
                                body: crate::protocol::StreamOpened {
                                    stream_id: body.stream_id,
                                    fd_count: 0,
                                },
                            },
                            max_frame,
                        )
                        .await?;
                    }
                    Message::CapabilityGrant { id, body } => {
                        write_msg(
                            &mut stream,
                            &Message::CapabilityGranted {
                                id,
                                body: crate::protocol::CapabilityGranted {
                                    grant_id: body.grant_id,
                                    status: "applied".to_owned(),
                                },
                            },
                            max_frame,
                        )
                        .await?;
                    }
                    Message::CapabilityRelease { id, .. } => {
                        write_msg(&mut stream, &Message::CapabilityReleased { id }, max_frame)
                            .await?;
                    }
                    Message::FlowControl { .. }
                    | Message::Log { .. }
                    | Message::ToolExecutionComplete { .. } => {}
                    Message::PluginConfigSchema { id } => {
                        let body = handler.config_schema().await.unwrap_or_default();
                        write_msg(
                            &mut stream,
                            &Message::PluginConfigSchemaResult { id, body },
                            max_frame,
                        )
                        .await?;
                    }
                    Message::PluginConfigValidate { id, values } => {
                        let body = handler.config_validate(values).await.unwrap_or_else(|_| {
                            crate::PluginConfigValidateResult {
                                ok: false,
                                errors: vec![crate::PluginConfigError {
                                    path: String::new(),
                                    message: "validation failed".to_owned(),
                                }],
                                restart_required: false,
                            }
                        });
                        write_msg(
                            &mut stream,
                            &Message::PluginConfigValidateResult { id, body },
                            max_frame,
                        )
                        .await?;
                    }
                    Message::PluginConfigOptions { id, field } => {
                        let body = handler
                            .config_options(&field)
                            .await
                            .unwrap_or_else(|_| crate::PluginConfigOptionsResult::unsupported());
                        write_msg(
                            &mut stream,
                            &Message::PluginConfigOptionsResult { id, body },
                            max_frame,
                        )
                        .await?;
                    }
                    Message::PluginConfigApply { id, values } => {
                        let body = handler.config_apply(values).await.unwrap_or_else(|_| {
                            crate::PluginConfigApplyResult {
                                ok: false,
                                errors: vec![crate::PluginConfigError {
                                    path: String::new(),
                                    message: "apply failed".to_owned(),
                                }],
                                restart_required: false,
                            }
                        });
                        write_msg(
                            &mut stream,
                            &Message::PluginConfigApplyResult { id, body },
                            max_frame,
                        )
                        .await?;
                    }
                    other => return Err(IpcError::Unexpected(other.kind_name().to_owned())),
                }
                flush_background(&mut stream, &handler, max_frame).await?;
            }
            () = sleep(Duration::from_millis(25)) => {
                flush_background(&mut stream, &handler, max_frame).await?;
            }
        }
    }
}

async fn flush_background<S, H>(
    stream: &mut S,
    handler: &H,
    max_frame: usize,
) -> Result<(), IpcError>
where
    S: AsyncWrite + Unpin,
    H: ToolHandler,
{
    for body in handler.poll_background() {
        write_msg(stream, &Message::ToolExecutionComplete { body }, max_frame).await?;
    }
    Ok(())
}

async fn expect_hello<S: AsyncRead + Unpin>(stream: &mut S) -> Result<HostHello, IpcError> {
    let bytes = read_frame(stream, MAX_FRAME_BYTES).await?;
    match Message::decode(&bytes)? {
        Message::Hello { body } => Ok(body),
        other => Err(IpcError::Unexpected(other.kind_name().to_owned())),
    }
}

fn build_ack<H: ToolHandler>(hello: &HostHello, handler: &H) -> Result<HelloAck, String> {
    if !hello.declared_protocols.contains(&ProtoId::Core) {
        return Err("manifest must declare core".to_owned());
    }
    let plugin_tool = hello
        .declared_protocols
        .contains(&ProtoId::Tool)
        .then_some(VersionRange::exact(TOOL_VERSION));
    let plugin_capability = hello
        .declared_protocols
        .contains(&ProtoId::Capability)
        .then_some(VersionRange::exact(crate::protocol::CAPABILITY_VERSION));
    let negotiated = negotiate(
        &hello.protocols,
        VersionRange::exact(CORE_VERSION),
        plugin_tool,
        None,
        plugin_capability,
    )
    .map_err(|err| err.to_string())?;
    if handler.digest() != hello.expected_digest && !hello.allow_unverified {
        return Err("manifest digest mismatch".to_owned());
    }
    let spawn_token = handler.spawn_token()?;
    Ok(HelloAck {
        plugin_id: handler.plugin_id().to_owned(),
        plugin_name: handler.plugin_name().to_owned(),
        plugin_version: "0.1.0".to_owned(),
        manifest_digest: handler.digest().to_owned(),
        protocols: negotiated,
        spawn_token,
        has_config: handler.has_config(),
    })
}

async fn write_msg<S: AsyncWrite + Unpin>(
    stream: &mut S,
    message: &Message,
    max_frame: usize,
) -> Result<(), IpcError> {
    write_frame(stream, &message.encode()?, max_frame).await
}

/// Connect to `ENE_PLUGIN_SOCKET` and serve until drain/shutdown.
#[cfg(feature = "net")]
pub async fn serve_from_env<H: ToolHandler>(handler: H) -> Result<(), IpcError> {
    let path = std::env::var("ENE_PLUGIN_SOCKET").map_err(|_| {
        IpcError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "ENE_PLUGIN_SOCKET is not set",
        ))
    })?;
    #[cfg(unix)]
    {
        let stream = tokio::net::UnixStream::connect(&path).await?;
        serve_plugin(stream, handler).await
    }
    #[cfg(windows)]
    {
        let stream = tokio::net::TcpStream::connect(&path).await?;
        serve_plugin(stream, handler).await
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, handler);
        Err(IpcError::Io(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "plugin IPC requires Unix domain sockets or Windows TCP",
        )))
    }
}

/// Report that the optional network transport was not compiled in.
#[cfg(not(feature = "net"))]
pub async fn serve_from_env<H: ToolHandler>(handler: H) -> Result<(), IpcError> {
    let _ = handler;
    Err(IpcError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "plugin IPC network transport is disabled",
    )))
}
