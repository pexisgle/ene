//! Plugin IPC server: accept loop, request dispatch, and streaming handler.
//!
//! [`run_plugin_server`] is the entry point for plugin binaries. It binds an
//! IPC listener, accepts connections, and dispatches
//! [`PluginIpcRequest`](ene_plugin_proto::PluginIpcRequest) messages to the
//! appropriate trait implementation via [`PluginDispatch`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ene_plugin_proto::{DeferredOutcome, VersionRange};
use ene_plugin_proto::{
    IpcListener, IpcStream, PLUGIN_IPC_PROTOCOL_VERSION, PluginCapabilities, PluginError,
    PluginIpcRequest, PluginIpcResponse, cleanup_path, read_plugin_request, write_plugin_response,
};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::plugin::{EmbedPlugin, LlmPlugin, SttPlugin, ToolPlugin, TtsPlugin};

/// How often an idle connection polls the tool plugin for deferred task
/// completions to push to the host (Cr-5). Completions are delivered on this
/// cadence even when no request is in flight, rather than only piggybacking on
/// the next request/response cycle.
const DEFERRED_DRAIN_INTERVAL: Duration = Duration::from_millis(100);

/// Dispatch table holding up to five focused trait implementations.
///
/// A plugin struct implements any subset of [`ToolPlugin`], [`LlmPlugin`],
/// [`EmbedPlugin`], [`TtsPlugin`], and [`SttPlugin`]; the server routes
/// incoming IPC requests to the corresponding trait object.
pub struct PluginDispatch {
    /// Optional tool plugin implementation.
    pub tool: Option<Arc<dyn ToolPlugin>>,
    /// Optional LLM plugin implementation.
    pub llm: Option<Arc<dyn LlmPlugin>>,
    /// Optional embedding plugin implementation.
    pub embed: Option<Arc<dyn EmbedPlugin>>,
    /// Optional TTS plugin implementation.
    pub tts: Option<Arc<dyn TtsPlugin>>,
    /// Optional STT plugin implementation.
    pub stt: Option<Arc<dyn SttPlugin>>,
}

impl PluginDispatch {
    /// Creates a dispatch table with the given trait implementations.
    pub fn new(
        tool: Option<Arc<dyn ToolPlugin>>,
        llm: Option<Arc<dyn LlmPlugin>>,
        embed: Option<Arc<dyn EmbedPlugin>>,
        tts: Option<Arc<dyn TtsPlugin>>,
        stt: Option<Arc<dyn SttPlugin>>,
    ) -> Self {
        Self {
            tool,
            llm,
            embed,
            tts,
            stt,
        }
    }

    /// Delivers the plugin configuration blob to every registered trait
    /// implementation (called once during Handshake).
    ///
    /// A single plugin struct may implement several traits (e.g. `ToolPlugin`
    /// and `LlmPlugin`); each is stored as a separate trait object, so the
    /// config is delivered to every one present. `set_config` is idempotent
    /// (plugins store the blob), so repeated delivery is harmless (#313).
    fn set_config(&self, config: &serde_json::Value) {
        if let Some(tool) = &self.tool {
            tool.set_config(config);
        }
        if let Some(llm) = &self.llm {
            llm.set_config(config);
        }
        if let Some(embed) = &self.embed {
            embed.set_config(config);
        }
        if let Some(tts) = &self.tts {
            tts.set_config(config);
        }
        if let Some(stt) = &self.stt {
            stt.set_config(config);
        }
    }

    /// Delivers the per-profile configuration blob to every registered trait
    /// implementation (called once during Handshake when
    /// `plugins.list.<name>.profiles` is configured).
    fn set_profiles(&self, profiles: &serde_json::Value) {
        if let Some(tool) = &self.tool {
            tool.set_profiles(profiles);
        }
        if let Some(llm) = &self.llm {
            llm.set_profiles(profiles);
        }
        if let Some(embed) = &self.embed {
            embed.set_profiles(profiles);
        }
        if let Some(tts) = &self.tts {
            tts.set_profiles(profiles);
        }
        if let Some(stt) = &self.stt {
            stt.set_profiles(profiles);
        }
    }

    /// Returns the first non-`None` config schema among the registered trait
    /// implementations, preferring the tool plugin (the historical source).
    ///
    /// Method-call syntax (closures, not function pointers) is required here:
    /// `ConfigurablePlugin` is implemented for the trait objects themselves,
    /// not for `Arc<dyn …>`, so `and_then(ConfigurablePlugin::config_schema)`
    /// would fail to satisfy the receiver type (#313).
    fn config_schema(&self) -> Option<serde_json::Value> {
        self.tool
            .as_ref()
            .and_then(|t| t.config_schema())
            .or_else(|| self.llm.as_ref().and_then(|l| l.config_schema()))
            .or_else(|| self.embed.as_ref().and_then(|e| e.config_schema()))
            .or_else(|| self.tts.as_ref().and_then(|t| t.config_schema()))
            .or_else(|| self.stt.as_ref().and_then(|s| s.config_schema()))
    }
}

/// Starts a plugin as an IPC server.
///
/// Reads the socket path from the `ENE_PLUGIN_SOCKET` environment variable
/// (falling back to `/tmp/ene-plugin.sock` on Unix or `\\.\pipe\ene-plugin`
/// on Windows) and listens for requests over IPC. Shuts down upon receiving
/// a `Shutdown` request, `SIGINT`, or `SIGTERM`.
///
/// # Handshake latency
///
/// The host bounds how long it waits for the handshake response
/// (`plugins.handshake_timeout_ms`, default 10 s). A plugin that performs
/// heavy initialization (loading model weights, opening databases, etc.)
/// **before** answering the `Handshake` request risks exceeding that bound,
/// which makes the host fail the plugin's startup and skip it. Plugins
/// should answer the handshake promptly and defer expensive work until
/// afterwards (e.g. lazily on first use, or on a background task spawned
/// once the server is listening).
///
/// # Usage
///
/// ```rust,no_run
/// # use ene_plugin::{PluginDispatch, ToolPlugin, PluginStreamChunk};
/// # struct MyTool;
/// # impl ene_plugin::ConfigurablePlugin for MyTool {}
/// # #[async_trait::async_trait]
/// # impl ToolPlugin for MyTool {
/// #     fn tool_capabilities(&self) -> ene_plugin::ToolPluginCapabilities {
/// #         ene_plugin::ToolPluginCapabilities { tool_count: 0 }
/// #     }
/// # }
/// #[tokio::main]
/// async fn main() -> Result<(), ene_plugin::PluginError> {
///     ene_plugin::run_plugin_server(PluginDispatch::new(
///         Some(std::sync::Arc::new(MyTool)),
///         None,
///         None,
///         None,
///         None,
///     )).await
/// }
/// ```
pub async fn run_plugin_server(dispatch: PluginDispatch) -> Result<(), PluginError> {
    let socket_path = std::env::var("ENE_PLUGIN_SOCKET").unwrap_or_else(|_| {
        #[cfg(unix)]
        {
            "/tmp/ene-plugin.sock".to_string()
        }
        #[cfg(windows)]
        {
            r"\\.\pipe\ene-plugin".to_string()
        }
    });
    let socket_path = PathBuf::from(&socket_path);

    cleanup_path(&socket_path);

    let mut listener = IpcListener::bind(&socket_path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&socket_path, perms)?;
    }

    tracing::info!(
        component = "PluginServer",
        socket = %socket_path.display(),
        "Listening"
    );

    let dispatch: Arc<PluginDispatch> = Arc::new(dispatch);
    let shutdown = Arc::new(tokio::sync::Notify::new());
    let tasks: Arc<tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>> =
        Arc::new(tokio::sync::Mutex::new(Vec::new()));

    #[cfg(unix)]
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    loop {
        #[cfg(unix)]
        let sigterm_fut = sigterm.recv();
        #[cfg(not(unix))]
        let sigterm_fut = std::future::pending::<()>();

        tokio::select! {
            result = listener.accept() => {
                let stream = result?;
                let dispatch = Arc::clone(&dispatch);
                let shutdown = Arc::clone(&shutdown);
                let tasks = Arc::clone(&tasks);
                let handle = tokio::spawn(async move {
                    handle_connection(dispatch, stream, shutdown).await;
                });
                tasks.lock().await.push(handle);
            }
            () = shutdown.notified() => {
                break;
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!(component = "PluginServer", "Received SIGINT, shutting down");
                break;
            }
            _ = sigterm_fut => {
                tracing::info!(component = "PluginServer", "Received SIGTERM, shutting down");
                break;
            }
        }
    }

    // Join all spawned connection tasks before cleanup. This prevents
    // orphaned tasks from writing to a closed socket or holding references
    // to the dispatch Arc after the server has exited.
    {
        let mut guard = tasks.lock().await;
        let handles: Vec<_> = guard.drain(..).collect();
        drop(guard);
        for handle in handles {
            drop(tokio::time::timeout(Duration::from_secs(5), handle).await);
        }
    }

    cleanup_path(&socket_path);
    tracing::info!(component = "PluginServer", "Shut down");
    Ok(())
}

/// Handles a single IPC connection.
///
/// The connection is split into a read half and a write half. A dedicated
/// writer task serializes every outgoing frame received over an internal
/// channel, so any number of producer tasks (request handlers, chat-stream
/// tasks, the deferred-completion drainer) can emit responses concurrently
/// without contending for the socket (Cr-4 / H-12).
///
/// Long-running requests are dispatched in their own spawned tasks while cheap
/// state mutations are handled inline (see [`connection_read_loop`]), so a slow
/// request never blocks the read loop: pings are answered while tool calls run,
/// and multiple tool calls can be in flight at once (#431). Each
/// `CreateChatStream` request is additionally guarded by a [`CancellationToken`]
/// keyed by its `request_id`; a `CancelStream` request looks up that token and
/// cancels the stream mid-flight (Cr-4).
///
/// A periodic drainer pushes [`PluginIpcResponse::DeferredCompleted`] messages
/// on a timer, so background task completions reach the host even while the
/// connection is idle instead of piggybacking on the next request (Cr-5).
async fn handle_connection(
    dispatch: Arc<PluginDispatch>,
    stream: IpcStream,
    shutdown: Arc<tokio::sync::Notify>,
) {
    let (read_half, write_half) = tokio::io::split(stream);
    let (tx, rx) = mpsc::channel::<PluginIpcResponse>(64);

    let writer_task = tokio::spawn(write_loop(write_half, rx));

    // In-flight chat streams keyed by request_id, for cancellation (Cr-4).
    let streams: Arc<parking_lot::Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(parking_lot::Mutex::new(HashMap::new()));

    // Periodic deferred-completion drainer (Cr-5 idle delivery).
    let drain_task = spawn_deferred_drain(Arc::clone(&dispatch), tx.clone());

    let read_result = connection_read_loop(&dispatch, read_half, &tx, &streams, &shutdown).await;

    // Tear down: cancel any in-flight streams, stop the drainer, then let the
    // writer flush remaining frames and finish once all senders are dropped.
    //
    // Note on teardown latency: spawned dispatch tasks hold `tx` clones, so
    // `drop(tx); writer_task.await` below waits for every in-flight request to
    // finish (or be cancelled) before the writer channel closes. This is new
    // with the spawned-dispatch design but bounded: `run_plugin_server` joins
    // each connection task with a 5 s timeout, so a hung request cannot stall
    // shutdown indefinitely.
    for (_, token) in streams.lock().drain() {
        token.cancel();
    }
    drain_task.abort();
    drop(tx);
    drop(writer_task.await);

    if let Err(e) = read_result {
        tracing::error!(component = "PluginServer", error = %e, "Connection read loop ended");
    }
}

/// Serializes outgoing frames: writes each response received on `rx` to the
/// socket in order. Exits when the channel closes or a write fails.
async fn write_loop<W: tokio::io::AsyncWrite + Unpin>(
    mut writer: W,
    mut rx: mpsc::Receiver<PluginIpcResponse>,
) {
    while let Some(resp) = rx.recv().await {
        if write_plugin_response(&mut writer, &resp).await.is_err() {
            break;
        }
    }
}

/// Spawns a task that periodically drains deferred task completions from the
/// tool plugin and pushes them to the host over `tx` (Cr-5).
fn spawn_deferred_drain(
    dispatch: Arc<PluginDispatch>,
    tx: mpsc::Sender<PluginIpcResponse>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(DEFERRED_DRAIN_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let Some(tool) = &dispatch.tool else {
                continue;
            };
            for (task_id, result) in tool.drain_deferred_completions() {
                let resp = PluginIpcResponse::DeferredCompleted { task_id, result };
                if tx.send(resp).await.is_err() {
                    return;
                }
            }
        }
    })
}

/// Reads requests in a loop and dispatches them.
///
/// Long-running requests (`CallTool`, `ChatCompletion`, `EmbedBatch`,
/// `SynthesizeSpeech`, `TranscribeAudio`, `PollDeferred`) are dispatched in
/// their own spawned tasks so the read loop stays responsive regardless of how
/// long any single one takes: a slow `CallTool` cannot block a `Ping`, and
/// multiple tool calls can be in flight at once (#431). Responses are sent over
/// `tx`, whose dedicated writer task serializes the outgoing frames, so
/// concurrent handlers never interleave bytes on the socket (Cr-4 / H-12).
///
/// Cheap *state-mutating* requests (`Handshake`, `SetCallContext`,
/// `ApprovePermission`, `AllowPattern`, `RevokePattern`) and lightweight
/// queries (`Ping`, `ListTools`, `GetConfigSchema`, `CancelDeferred`) are
/// handled **inline**, in read order, rather than spawned. This makes their
/// ordering relative to the spawned requests they gate a server-enforced
/// invariant instead of a client convention: `Handshake` applies the sandbox
/// (`tool.set_sandbox` / `set_config`) and completes before the first
/// `CallTool` is even read off the socket, and a permission/pattern mutation
/// is committed before any subsequent call that depends on it (#431 review).
///
/// `CreateChatStream` additionally registers a [`CancellationToken`] so
/// `CancelStream` can abort it mid-flight; `Shutdown` notifies the server and
/// ends the loop. Returns `Ok(())` on EOF or graceful shutdown.
async fn connection_read_loop<R: tokio::io::AsyncRead + Unpin>(
    dispatch: &Arc<PluginDispatch>,
    mut reader: R,
    tx: &mpsc::Sender<PluginIpcResponse>,
    streams: &Arc<parking_lot::Mutex<HashMap<String, CancellationToken>>>,
    shutdown: &Arc<tokio::sync::Notify>,
) -> Result<(), PluginError> {
    loop {
        let req = match read_plugin_request(&mut reader).await {
            Ok(None) => return Ok(()),
            Ok(Some(req)) => req,
            Err(e) => {
                tracing::error!(component = "PluginServer", error = %e, "IPC read error");
                drop(
                    tx.send(PluginIpcResponse::Error {
                        request_id: String::new(),
                        message: e.to_string(),
                    })
                    .await,
                );
                return Err(e);
            }
        };

        match req {
            PluginIpcRequest::CreateChatStream { ref request_id, .. } => {
                let token = CancellationToken::new();
                streams.lock().insert(request_id.clone(), token.clone());
                let dispatch = Arc::clone(dispatch);
                let req = req.clone();
                let tx = tx.clone();
                let streams = Arc::clone(streams);
                let stream_id = request_id.clone();
                tokio::spawn(async move {
                    run_chat_stream(&dispatch, &req, tx, token).await;
                    streams.lock().remove(&stream_id);
                });
            }
            PluginIpcRequest::CancelStream {
                request_id,
                stream_request_id,
            } => {
                if let Some(token) = streams.lock().remove(&stream_request_id) {
                    token.cancel();
                }
                drop(tx.send(PluginIpcResponse::Ack { request_id }).await);
            }
            PluginIpcRequest::Shutdown => {
                drop(
                    tx.send(PluginIpcResponse::Ack {
                        request_id: String::new(),
                    })
                    .await,
                );
                shutdown.notify_one();
                return Ok(());
            }
            // Long-running requests: dispatch in a spawned task so a slow one
            // (e.g. a tool call or an LLM completion) does not block the read
            // loop from answering pings or accepting further concurrent
            // requests (#431). Correlation is by `request_id`, so response
            // order need not match request order.
            long_running @ (PluginIpcRequest::CallTool { .. }
            | PluginIpcRequest::ChatCompletion { .. }
            | PluginIpcRequest::EmbedBatch { .. }
            | PluginIpcRequest::SynthesizeSpeech { .. }
            | PluginIpcRequest::TranscribeAudio { .. }
            | PluginIpcRequest::PollDeferred { .. }) => {
                let dispatch = Arc::clone(dispatch);
                let tx = tx.clone();
                tokio::spawn(async move {
                    let resp = dispatch_request(&dispatch, &long_running).await;
                    drop(tx.send(resp).await);
                });
            }
            // Everything else — `Handshake` (applies the sandbox/config),
            // `SetCallContext`, `ApprovePermission`, `AllowPattern`,
            // `RevokePattern`, and the cheap queries (`Ping`, `ListTools`,
            // `GetConfigSchema`, `CancelDeferred`) — is handled inline, in
            // read order. State mutations must be committed before any later
            // request that depends on them is dispatched, so they cannot be
            // reordered behind a spawned task (#431 review).
            other => {
                let resp = dispatch_request(dispatch, &other).await;
                drop(tx.send(resp).await);
            }
        }
    }
}

/// Dispatches a non-streaming request and returns the single response.
#[expect(
    clippy::manual_let_else,
    reason = "match-based return is clearer for multi-branch dispatch with early returns"
)]
async fn dispatch_request(dispatch: &PluginDispatch, req: &PluginIpcRequest) -> PluginIpcResponse {
    match req {
        PluginIpcRequest::Handshake {
            version: host_range,
            sandbox,
            plugin_config,
            plugin_profiles,
        } => {
            let our_range = VersionRange {
                min: PLUGIN_IPC_PROTOCOL_VERSION,
                max: PLUGIN_IPC_PROTOCOL_VERSION,
            };
            // Negotiate the highest protocol version supported by both sides.
            // `negotiate` returns `None` when the ranges do not overlap.
            let Some(negotiated) = our_range.negotiate(host_range) else {
                tracing::error!(
                    component = "PluginServer",
                    host_min = host_range.min,
                    host_max = host_range.max,
                    our_min = our_range.min,
                    our_max = our_range.max,
                    "Handshake version range mismatch"
                );
                return PluginIpcResponse::Error {
                    request_id: String::new(),
                    message: format!(
                        "protocol version mismatch: host supports {}-{}, \
                         plugin supports {}-{}",
                        host_range.min, host_range.max, our_range.min, our_range.max
                    ),
                };
            };
            if let Some(tool) = &dispatch.tool {
                tool.set_sandbox(sandbox);
            }
            // Configuration is delivered to **every** registered trait
            // implementation (tool or provider), not just the tool plugin
            // (#313). Both blobs are opaque to the plugin server; the plugin
            // stores them and selects as it needs.
            if let Some(config) = plugin_config {
                dispatch.set_config(config);
            }
            if let Some(profiles) = plugin_profiles {
                dispatch.set_profiles(profiles);
            }
            PluginIpcResponse::HandshakeAck {
                version: negotiated,
                capabilities: collect_capabilities(dispatch),
            }
        }
        PluginIpcRequest::Ping { request_id } => PluginIpcResponse::Pong {
            request_id: request_id.clone(),
        },
        PluginIpcRequest::GetConfigSchema { request_id } => PluginIpcResponse::ConfigSchema {
            request_id: request_id.clone(),
            schema: dispatch.config_schema(),
        },
        PluginIpcRequest::ListTools { request_id } => PluginIpcResponse::Tools {
            request_id: request_id.clone(),
            tools: dispatch
                .tool
                .as_ref()
                .map_or(Vec::new(), |t| t.list_tool_specs()),
        },
        PluginIpcRequest::CallTool {
            request_id,
            name,
            arguments,
            deferred,
            context,
        } => {
            let ctx_ref = context.as_ref();
            let Some(tool) = &dispatch.tool else {
                return PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: "no tool plugin registered".to_string(),
                };
            };
            if *deferred {
                match tool.call_tool_deferred(name, arguments, ctx_ref).await {
                    Ok(DeferredOutcome::Sync(result)) => PluginIpcResponse::CallResult {
                        request_id: request_id.clone(),
                        result: Ok(result),
                    },
                    Ok(DeferredOutcome::Deferred { task_id }) => {
                        PluginIpcResponse::DeferredAccepted {
                            request_id: request_id.clone(),
                            task_id,
                        }
                    }
                    Err(e) => PluginIpcResponse::CallResult {
                        request_id: request_id.clone(),
                        result: Err(e),
                    },
                }
            } else {
                let result = tool.call_tool(name, arguments, ctx_ref).await;
                PluginIpcResponse::CallResult {
                    request_id: request_id.clone(),
                    result,
                }
            }
        }
        PluginIpcRequest::ChatCompletion {
            request_id,
            provider_kind,
            provider_config,
            model,
            max_tokens,
            messages,
            json_schema,
        } => {
            let llm = match &dispatch.llm {
                Some(l) => l,
                None => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: "no LLM plugin registered".to_string(),
                    };
                }
            };
            match llm
                .chat_completion(
                    provider_kind,
                    provider_config.clone(),
                    model.clone(),
                    *max_tokens,
                    messages.clone(),
                    json_schema.clone(),
                )
                .await
            {
                Ok(completion) => PluginIpcResponse::ChatCompletionResult {
                    request_id: request_id.clone(),
                    content: completion.text,
                    usage: completion.usage,
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::EmbedBatch {
            request_id,
            provider_kind,
            provider_config,
            model,
            dimensions,
            items,
        } => {
            let embed = match &dispatch.embed {
                Some(e) => e,
                None => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: "no embed plugin registered".to_string(),
                    };
                }
            };
            match embed
                .embed_batch(
                    provider_kind,
                    provider_config.clone(),
                    model.clone(),
                    *dimensions,
                    items.clone(),
                )
                .await
            {
                Ok(embeddings) => PluginIpcResponse::EmbedBatchResult {
                    request_id: request_id.clone(),
                    embeddings,
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::PollDeferred {
            request_id,
            task_id,
        } => {
            let tool = match &dispatch.tool {
                Some(t) => t,
                None => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: "no tool plugin registered".to_string(),
                    };
                }
            };
            match tool.poll_deferred(task_id) {
                Ok(status) => PluginIpcResponse::DeferredStatus {
                    request_id: request_id.clone(),
                    task_id: task_id.clone(),
                    status,
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::SetCallContext {
            request_id,
            conversation_id,
            turn_id,
        } => {
            tracing::warn!(
                component = "PluginServer",
                "SetCallContext is deprecated; per-call context is now passed directly \
                 in CallTool requests (conversation_id={conversation_id}, turn_id={turn_id})"
            );
            PluginIpcResponse::Ack {
                request_id: request_id.clone(),
            }
        }
        PluginIpcRequest::ApprovePermission {
            request_id,
            permission_request_id,
        } => {
            let tool = match &dispatch.tool {
                Some(t) => t,
                None => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: "no tool plugin registered".to_string(),
                    };
                }
            };
            match tool.approve_permission(permission_request_id) {
                Ok(()) => PluginIpcResponse::Ack {
                    request_id: request_id.clone(),
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::AllowPattern {
            request_id,
            action,
            target_pattern,
        } => {
            let tool = match &dispatch.tool {
                Some(t) => t,
                None => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: "no tool plugin registered".to_string(),
                    };
                }
            };
            match tool.allow_pattern(action, target_pattern) {
                Ok(()) => PluginIpcResponse::Ack {
                    request_id: request_id.clone(),
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::RevokePattern {
            request_id,
            action,
            target_pattern,
        } => {
            let tool = match &dispatch.tool {
                Some(t) => t,
                None => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: "no tool plugin registered".to_string(),
                    };
                }
            };
            match tool.revoke_pattern(action, target_pattern) {
                Ok(()) => PluginIpcResponse::Ack {
                    request_id: request_id.clone(),
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::CancelDeferred {
            request_id,
            task_id,
        } => {
            let tool = match &dispatch.tool {
                Some(t) => t,
                None => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: "no tool plugin registered".to_string(),
                    };
                }
            };
            match tool.cancel_deferred(task_id) {
                Ok(()) => PluginIpcResponse::Ack {
                    request_id: request_id.clone(),
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::SynthesizeSpeech {
            request_id,
            provider_kind,
            provider_config,
            text,
            voice,
            format,
        } => {
            let Some(tts) = &dispatch.tts else {
                return PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: "no TTS plugin registered".to_string(),
                };
            };
            match tts
                .synthesize(
                    provider_kind,
                    provider_config.clone(),
                    text.clone(),
                    voice.clone(),
                    format.clone(),
                )
                .await
            {
                Ok(audio_data) => PluginIpcResponse::SpeechResult {
                    request_id: request_id.clone(),
                    audio_base64: base64_encode(&audio_data),
                    format: format.clone(),
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::TranscribeAudio {
            request_id,
            provider_kind,
            provider_config,
            audio_base64,
            format,
        } => {
            let Some(stt) = &dispatch.stt else {
                return PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: "no STT plugin registered".to_string(),
                };
            };
            let audio_data = match base64_decode(audio_base64) {
                Ok(data) => data,
                Err(e) => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: format!("invalid base64 audio: {e}"),
                    };
                }
            };
            match stt
                .transcribe(
                    provider_kind,
                    provider_config.clone(),
                    audio_data,
                    format.clone(),
                )
                .await
            {
                Ok(text) => PluginIpcResponse::TranscriptionResult {
                    request_id: request_id.clone(),
                    text,
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::Shutdown => PluginIpcResponse::Ack {
            request_id: String::new(),
        },
        // CreateChatStream and CancelStream are handled by the connection read
        // loop (spawned stream task / cancellation map), never dispatched here.
        PluginIpcRequest::CreateChatStream { .. } => PluginIpcResponse::Error {
            request_id: String::new(),
            message: "CreateChatStream must be handled by the streaming path".to_string(),
        },
        PluginIpcRequest::CancelStream { request_id, .. } => PluginIpcResponse::Error {
            request_id: request_id.clone(),
            message: "CancelStream must be handled by the connection read loop".to_string(),
        },
    }
}

/// Base64-encode bytes to a string.
fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// Base64-decode a string to bytes.
fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| e.to_string())
}

/// Collects capabilities from all registered dispatch components.
fn collect_capabilities(dispatch: &PluginDispatch) -> PluginCapabilities {
    PluginCapabilities {
        tools: dispatch
            .tool
            .as_ref()
            .map_or(0, |t| t.tool_capabilities().tool_count),
        llm_providers: dispatch
            .llm
            .as_ref()
            .map_or(Vec::new(), |l| l.llm_capabilities()),
        tts_providers: dispatch
            .tts
            .as_ref()
            .map_or(Vec::new(), |t| t.tts_capabilities()),
        stt_providers: dispatch
            .stt
            .as_ref()
            .map_or(Vec::new(), |s| s.stt_capabilities()),
    }
}

/// Runs a `CreateChatStream` request to completion, sending `StreamChunk` /
/// `StreamEnd` / `StreamError` responses over `tx`.
///
/// The stream is aborted as soon as `cancel` fires (Cr-4): the chunk loop
/// selects between the next stream item and the cancellation token, so a
/// `CancelStream` request stops the underlying LLM stream promptly rather than
/// waiting for it to drain.
async fn run_chat_stream(
    dispatch: &PluginDispatch,
    req: &PluginIpcRequest,
    tx: mpsc::Sender<PluginIpcResponse>,
    cancel: CancellationToken,
) {
    let PluginIpcRequest::CreateChatStream {
        request_id,
        provider_kind,
        provider_config,
        model,
        max_tokens,
        messages,
        tools,
    } = req
    else {
        tracing::error!(
            component = "PluginServer",
            "expected CreateChatStream request in streaming handler"
        );
        return;
    };

    let Some(llm) = &dispatch.llm else {
        drop(
            tx.send(PluginIpcResponse::StreamError {
                request_id: request_id.clone(),
                message: "no LLM plugin registered".to_string(),
            })
            .await,
        );
        return;
    };

    let stream = match llm
        .create_chat_stream(
            provider_kind,
            provider_config.clone(),
            model.clone(),
            *max_tokens,
            messages.clone(),
            tools.clone(),
        )
        .await
    {
        Ok(stream) => stream,
        Err(e) => {
            drop(
                tx.send(PluginIpcResponse::StreamError {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                })
                .await,
            );
            return;
        }
    };

    let mut stream = stream;
    loop {
        tokio::select! {
            // `biased` checks cancellation first so a cancel that races with a
            // ready stream item wins deterministically (Cr-4).
            biased;
            () = cancel.cancelled() => {
                tracing::info!(
                    component = "PluginServer",
                    request_id = %request_id,
                    "Chat stream cancelled by host"
                );
                drop(tx
                    .send(PluginIpcResponse::StreamError {
                        request_id: request_id.clone(),
                        message: "stream cancelled".to_string(),
                    })
                    .await);
                return;
            }
            next = stream.next() => {
                match next {
                    Some(Ok(chunk)) => {
                        let resp = PluginIpcResponse::StreamChunk {
                            request_id: request_id.clone(),
                            text_delta: chunk.text_delta.unwrap_or_default(),
                            tool_calls_delta: chunk.tool_calls_delta.unwrap_or_default(),
                            usage: chunk.usage,
                        };
                        if tx.send(resp).await.is_err() {
                            return;
                        }
                    }
                    Some(Err(e)) => {
                        drop(tx
                            .send(PluginIpcResponse::StreamError {
                                request_id: request_id.clone(),
                                message: e.to_string(),
                            })
                            .await);
                        return;
                    }
                    None => {
                        drop(tx
                            .send(PluginIpcResponse::StreamEnd {
                                request_id: request_id.clone(),
                            })
                            .await);
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::panic,
    reason = "test assertions use panic for unexpected variants"
)]
#[expect(
    clippy::indexing_slicing,
    reason = "test assertions index into known-length vectors"
)]
#[expect(
    clippy::expect_used,
    reason = "tests use expect for concise failure messages"
)]
mod tests {
    use super::*;
    use crate::plugin::{
        ConfigurablePlugin, PluginCompletion, PluginStreamChunk, ToolPluginCapabilities,
    };
    use async_trait::async_trait;
    use ene_plugin_proto::ToolName;
    use ene_plugin_proto::{
        ConcurrencyHint, DeferredStatus, LlmProviderSpec, SttProviderSpec, ToolSpec,
        TtsProviderSpec, VersionRange,
    };

    /// A mock tool plugin for testing dispatch logic.
    struct MockToolPlugin;

    #[async_trait]
    impl ToolPlugin for MockToolPlugin {
        fn tool_capabilities(&self) -> ToolPluginCapabilities {
            ToolPluginCapabilities { tool_count: 1 }
        }

        fn list_tool_specs(&self) -> Vec<ToolSpec> {
            vec![ToolSpec::new(
                ToolName::new("test.echo"),
                "Echoes the input arguments.",
                serde_json::json!({}),
            )]
        }

        async fn call_tool(
            &self,
            name: &str,
            args: &str,
            _context: Option<&ene_plugin_proto::CallContext>,
        ) -> Result<ene_plugin_proto::ToolResult, ene_plugin_proto::ToolError> {
            if name == "test.echo" {
                Ok(ene_plugin_proto::ToolResult::text(args.to_string()))
            } else {
                Err(ene_plugin_proto::ToolError::NotFound {
                    tool_name: name.to_string(),
                })
            }
        }
    }

    impl ConfigurablePlugin for MockToolPlugin {
        fn config_schema(&self) -> Option<serde_json::Value> {
            Some(serde_json::json!({"type": "object"}))
        }
    }

    /// A tool plugin whose `call_tool` blocks on a shared gate until released,
    /// counting how many calls are concurrently in flight. Used to prove the
    /// server dispatches non-streaming requests concurrently (#431): under the
    /// old inline dispatch the counter could never exceed 1.
    struct GatedToolPlugin {
        in_flight: std::sync::atomic::AtomicUsize,
        released: std::sync::atomic::AtomicBool,
        notify: tokio::sync::Notify,
    }

    impl GatedToolPlugin {
        fn new() -> Self {
            Self {
                in_flight: std::sync::atomic::AtomicUsize::new(0),
                released: std::sync::atomic::AtomicBool::new(false),
                notify: tokio::sync::Notify::new(),
            }
        }

        /// Blocks until [`release`](Self::release) is called.
        async fn wait(&self) {
            use std::sync::atomic::Ordering;
            while !self.released.load(Ordering::Acquire) {
                self.notify.notified().await;
            }
        }

        /// Releases every current and future [`wait`](Self::wait) call.
        fn release(&self) {
            use std::sync::atomic::Ordering;
            self.released.store(true, Ordering::Release);
            self.notify.notify_waiters();
        }

        /// Current number of blocked `gated.slow` calls.
        fn in_flight(&self) -> usize {
            self.in_flight.load(std::sync::atomic::Ordering::Acquire)
        }
    }

    #[async_trait]
    impl ToolPlugin for GatedToolPlugin {
        fn tool_capabilities(&self) -> ToolPluginCapabilities {
            ToolPluginCapabilities { tool_count: 1 }
        }

        async fn call_tool(
            &self,
            name: &str,
            args: &str,
            _context: Option<&ene_plugin_proto::CallContext>,
        ) -> Result<ene_plugin_proto::ToolResult, ene_plugin_proto::ToolError> {
            use std::sync::atomic::Ordering;
            if name == "gated.slow" {
                self.in_flight.fetch_add(1, Ordering::AcqRel);
                self.wait().await;
                self.in_flight.fetch_sub(1, Ordering::AcqRel);
            }
            Ok(ene_plugin_proto::ToolResult::text(args.to_string()))
        }
    }

    impl ConfigurablePlugin for GatedToolPlugin {}

    /// Waits (with a deadlock backstop) until `plugin.in_flight()` reaches
    /// `expected`.
    async fn wait_for_in_flight(plugin: &GatedToolPlugin, expected: usize) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while plugin.in_flight() < expected {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {expected} in-flight call(s)"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// A mock LLM plugin for testing dispatch logic.
    struct MockLlmPlugin;

    #[async_trait]
    impl LlmPlugin for MockLlmPlugin {
        fn llm_capabilities(&self) -> Vec<LlmProviderSpec> {
            vec![LlmProviderSpec {
                kind: "mock".into(),
                supported_models: vec!["mock-model".into()],
                supports_streaming: true,
                supports_vision: false,
                concurrency: ConcurrencyHint::default(),
                context_window: None,
            }]
        }

        async fn create_chat_stream(
            &self,
            _kind: &str,
            _config: serde_json::Value,
            _model: String,
            _max_tokens: Option<u32>,
            _messages: Vec<serde_json::Value>,
            _tools: Vec<serde_json::Value>,
        ) -> Result<crate::plugin::PluginStream, PluginError> {
            let chunks = vec![
                Ok(PluginStreamChunk {
                    text_delta: Some("Hello".into()),
                    tool_calls_delta: None,
                    usage: None,
                }),
                Ok(PluginStreamChunk {
                    text_delta: Some(" world".into()),
                    tool_calls_delta: None,
                    usage: None,
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(chunks)))
        }

        async fn chat_completion(
            &self,
            _kind: &str,
            _config: serde_json::Value,
            _model: String,
            _max_tokens: Option<u32>,
            _messages: Vec<serde_json::Value>,
            _json_schema: Option<serde_json::Value>,
        ) -> Result<PluginCompletion, PluginError> {
            Ok(PluginCompletion::text_only(
                "Mock completion response".into(),
            ))
        }
    }

    impl ConfigurablePlugin for MockLlmPlugin {}

    /// A mock LLM plugin whose stream emits one chunk and then blocks until
    /// cancelled, used to exercise `CancelStream` (Cr-4).
    struct SlowMockLlmPlugin;

    #[async_trait]
    impl LlmPlugin for SlowMockLlmPlugin {
        fn llm_capabilities(&self) -> Vec<LlmProviderSpec> {
            vec![LlmProviderSpec {
                kind: "mock".into(),
                supported_models: vec!["mock-model".into()],
                supports_streaming: true,
                supports_vision: false,
                concurrency: ConcurrencyHint::default(),
                context_window: None,
            }]
        }

        async fn create_chat_stream(
            &self,
            _kind: &str,
            _config: serde_json::Value,
            _model: String,
            _max_tokens: Option<u32>,
            _messages: Vec<serde_json::Value>,
            _tools: Vec<serde_json::Value>,
        ) -> Result<crate::plugin::PluginStream, PluginError> {
            // Emit one chunk, then block "forever" so the only way to end the
            // stream is cancellation.
            let first = tokio_stream::iter(vec![Ok(PluginStreamChunk {
                text_delta: Some("partial".into()),
                tool_calls_delta: None,
                usage: None,
            })]);
            let pending = tokio_stream::pending::<Result<PluginStreamChunk, PluginError>>();
            Ok(Box::pin(first.chain(pending)))
        }
    }

    impl ConfigurablePlugin for SlowMockLlmPlugin {}

    /// A mock TTS plugin for testing dispatch logic.
    struct MockTtsPlugin;

    #[async_trait]
    impl TtsPlugin for MockTtsPlugin {
        fn tts_capabilities(&self) -> Vec<TtsProviderSpec> {
            vec![TtsProviderSpec {
                kind: "mock_tts".into(),
                voices: vec!["default".into()],
                formats: vec!["wav".into()],
                concurrency: ConcurrencyHint::default(),
            }]
        }

        async fn synthesize(
            &self,
            _kind: &str,
            _config: serde_json::Value,
            text: String,
            _voice: String,
            _format: String,
        ) -> Result<Vec<u8>, PluginError> {
            Ok(text.into_bytes())
        }
    }

    impl ConfigurablePlugin for MockTtsPlugin {}

    /// A mock STT plugin for testing dispatch logic.
    struct MockSttPlugin;

    #[async_trait]
    impl SttPlugin for MockSttPlugin {
        fn stt_capabilities(&self) -> Vec<SttProviderSpec> {
            vec![SttProviderSpec {
                kind: "mock_stt".into(),
                models: vec!["mock-model".into()],
                formats: vec!["wav".into()],
                concurrency: ConcurrencyHint::default(),
            }]
        }

        async fn transcribe(
            &self,
            _kind: &str,
            _config: serde_json::Value,
            _audio_data: Vec<u8>,
            _format: String,
        ) -> Result<String, PluginError> {
            Ok("Mock transcription".into())
        }
    }

    impl ConfigurablePlugin for MockSttPlugin {}

    /// A mock embed plugin for testing dispatch logic.
    struct MockEmbedPlugin;

    #[async_trait]
    impl EmbedPlugin for MockEmbedPlugin {
        async fn embed_batch(
            &self,
            _kind: &str,
            _config: serde_json::Value,
            _model: String,
            _dimensions: Option<u32>,
            _items: Vec<String>,
        ) -> Result<Vec<Vec<f32>>, PluginError> {
            Ok(_items.iter().map(|_| vec![0.1, 0.2, 0.3]).collect())
        }
    }

    impl ConfigurablePlugin for MockEmbedPlugin {}

    fn make_dispatch(tool: bool, llm: bool, embed: bool) -> PluginDispatch {
        PluginDispatch {
            tool: tool.then(|| Arc::new(MockToolPlugin) as Arc<dyn ToolPlugin>),
            llm: llm.then(|| Arc::new(MockLlmPlugin) as Arc<dyn LlmPlugin>),
            embed: embed.then(|| Arc::new(MockEmbedPlugin) as Arc<dyn EmbedPlugin>),
            tts: None,
            stt: None,
        }
    }

    fn make_tts_dispatch() -> PluginDispatch {
        PluginDispatch {
            tool: None,
            llm: None,
            embed: None,
            tts: Some(Arc::new(MockTtsPlugin) as Arc<dyn TtsPlugin>),
            stt: None,
        }
    }

    fn make_stt_dispatch() -> PluginDispatch {
        PluginDispatch {
            tool: None,
            llm: None,
            embed: None,
            tts: None,
            stt: Some(Arc::new(MockSttPlugin) as Arc<dyn SttPlugin>),
        }
    }

    #[tokio::test]
    async fn dispatch_handshake_ok() {
        let dispatch = make_dispatch(true, true, false);
        let req = PluginIpcRequest::Handshake {
            version: VersionRange {
                min: PLUGIN_IPC_PROTOCOL_VERSION,
                max: PLUGIN_IPC_PROTOCOL_VERSION,
            },
            sandbox: ene_plugin_proto::SandboxConfigData::default(),
            plugin_config: Some(serde_json::json!({"key": "value"})),
            plugin_profiles: None,
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::HandshakeAck {
                version,
                capabilities,
            } => {
                assert_eq!(version, PLUGIN_IPC_PROTOCOL_VERSION);
                assert_eq!(capabilities.tools, 1);
                assert_eq!(capabilities.llm_providers.len(), 1);
            }
            other => panic!("expected HandshakeAck, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_handshake_version_mismatch() {
        let dispatch = make_dispatch(true, false, false);
        let req = PluginIpcRequest::Handshake {
            version: VersionRange { min: 999, max: 999 },
            sandbox: ene_plugin_proto::SandboxConfigData::default(),
            plugin_config: None,
            plugin_profiles: None,
        };
        let resp = dispatch_request(&dispatch, &req).await;
        assert!(matches!(resp, PluginIpcResponse::Error { .. }));
    }

    #[tokio::test]
    async fn dispatch_handshake_partial_overlap_negotiates_highest_common() {
        // Host advertises {3,4}, plugin supports {4,4}; the negotiated version
        // must be the highest common version (4), not the lowest (H-11).
        let dispatch = make_dispatch(true, false, false);
        let req = PluginIpcRequest::Handshake {
            version: VersionRange {
                min: PLUGIN_IPC_PROTOCOL_VERSION - 1,
                max: PLUGIN_IPC_PROTOCOL_VERSION,
            },
            sandbox: ene_plugin_proto::SandboxConfigData::default(),
            plugin_config: None,
            plugin_profiles: None,
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::HandshakeAck { version, .. } => {
                assert_eq!(version, PLUGIN_IPC_PROTOCOL_VERSION);
            }
            other => panic!("expected HandshakeAck, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_ping() {
        let dispatch = make_dispatch(false, false, false);
        let resp = dispatch_request(
            &dispatch,
            &PluginIpcRequest::Ping {
                request_id: "ping-1".into(),
            },
        )
        .await;
        assert_eq!(
            resp,
            PluginIpcResponse::Pong {
                request_id: "ping-1".into()
            }
        );
    }

    #[tokio::test]
    async fn dispatch_shutdown() {
        let dispatch = make_dispatch(false, false, false);
        let resp = dispatch_request(&dispatch, &PluginIpcRequest::Shutdown).await;
        assert_eq!(
            resp,
            PluginIpcResponse::Ack {
                request_id: String::new()
            }
        );
    }

    #[tokio::test]
    async fn dispatch_get_config_schema() {
        let dispatch = make_dispatch(true, false, false);
        let req = PluginIpcRequest::GetConfigSchema {
            request_id: "req-1".into(),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::ConfigSchema { request_id, schema } => {
                assert_eq!(request_id, "req-1");
                assert!(schema.is_some());
            }
            other => panic!("expected ConfigSchema, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_get_config_schema_no_tool() {
        let dispatch = make_dispatch(false, false, false);
        let req = PluginIpcRequest::GetConfigSchema {
            request_id: "req-1".into(),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::ConfigSchema { request_id, schema } => {
                assert_eq!(request_id, "req-1");
                assert!(schema.is_none());
            }
            other => panic!("expected ConfigSchema, got {other:?}"),
        }
    }

    /// A mock LLM plugin that records the config / profiles delivered via
    /// [`ConfigurablePlugin::set_config`] / `set_profiles` and advertises a
    /// schema with a secret-marked field (#313).
    struct RecordingLlmPlugin {
        config: std::sync::Mutex<Option<serde_json::Value>>,
        profiles: std::sync::Mutex<Option<serde_json::Value>>,
    }

    impl RecordingLlmPlugin {
        fn new() -> Self {
            Self {
                config: std::sync::Mutex::new(None),
                profiles: std::sync::Mutex::new(None),
            }
        }
    }

    #[async_trait]
    impl LlmPlugin for RecordingLlmPlugin {
        fn llm_capabilities(&self) -> Vec<LlmProviderSpec> {
            Vec::new()
        }
    }

    impl ConfigurablePlugin for RecordingLlmPlugin {
        fn set_config(&self, config: &serde_json::Value) {
            *self.config.lock().unwrap() = Some(config.clone());
        }

        fn set_profiles(&self, profiles: &serde_json::Value) {
            *self.profiles.lock().unwrap() = Some(profiles.clone());
        }

        fn config_schema(&self) -> Option<serde_json::Value> {
            Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "api_key": { "type": "string", "x-ene-secret": true }
                }
            }))
        }
    }

    #[tokio::test]
    async fn handshake_delivers_config_and_profiles_to_provider_plugins() {
        // #313: `set_config` / `set_profiles` must reach provider traits
        // (LLM/embed/TTS/STT), not just the tool trait, so provider plugins
        // can receive their configuration at handshake time.
        let plugin = Arc::new(RecordingLlmPlugin::new());
        let dispatch = PluginDispatch {
            tool: None,
            llm: Some(Arc::clone(&plugin) as Arc<dyn LlmPlugin>),
            embed: None,
            tts: None,
            stt: None,
        };
        let req = PluginIpcRequest::Handshake {
            version: VersionRange {
                min: PLUGIN_IPC_PROTOCOL_VERSION,
                max: PLUGIN_IPC_PROTOCOL_VERSION,
            },
            sandbox: ene_plugin_proto::SandboxConfigData::default(),
            plugin_config: Some(serde_json::json!({"api_key": {"source": "env"}})),
            plugin_profiles: Some(serde_json::json!({"default": {"voice": "af_heart"}})),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        assert!(matches!(resp, PluginIpcResponse::HandshakeAck { .. }));
        assert_eq!(
            plugin.config.lock().unwrap().as_ref(),
            Some(&serde_json::json!({"api_key": {"source": "env"}}))
        );
        assert_eq!(
            plugin.profiles.lock().unwrap().as_ref(),
            Some(&serde_json::json!({"default": {"voice": "af_heart"}}))
        );
    }

    #[tokio::test]
    async fn dispatch_get_config_schema_from_provider_plugin() {
        // #313: `GetConfigSchema` must aggregate schemas from provider traits
        // (here: an LLM-only dispatch) rather than returning `None` when no
        // tool plugin is registered.
        let dispatch = PluginDispatch {
            tool: None,
            llm: Some(Arc::new(RecordingLlmPlugin::new()) as Arc<dyn LlmPlugin>),
            embed: None,
            tts: None,
            stt: None,
        };
        let resp = dispatch_request(
            &dispatch,
            &PluginIpcRequest::GetConfigSchema {
                request_id: "req-1".into(),
            },
        )
        .await;
        match resp {
            PluginIpcResponse::ConfigSchema { request_id, schema } => {
                assert_eq!(request_id, "req-1");
                let schema = schema.expect("LLM plugin must advertise a schema");
                assert_eq!(
                    schema.pointer("/properties/api_key/x-ene-secret"),
                    Some(&serde_json::json!(true))
                );
            }
            other => panic!("expected ConfigSchema, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_list_tools() {
        let dispatch = make_dispatch(true, false, false);
        let req = PluginIpcRequest::ListTools {
            request_id: "req-1".into(),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::Tools { request_id, tools } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(tools.len(), 1);
            }
            other => panic!("expected Tools, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_list_tools_no_tool() {
        let dispatch = make_dispatch(false, false, false);
        let req = PluginIpcRequest::ListTools {
            request_id: "req-1".into(),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::Tools { request_id, tools } => {
                assert_eq!(request_id, "req-1");
                assert!(tools.is_empty());
            }
            other => panic!("expected Tools, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_call_tool_found() {
        let dispatch = make_dispatch(true, false, false);
        let req = PluginIpcRequest::CallTool {
            request_id: "req-1".into(),
            name: "test.echo".into(),
            arguments: r#"{"msg":"hi"}"#.into(),
            deferred: false,
            context: None,
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::CallResult { request_id, result } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(result.unwrap().text_for_llm(), r#"{"msg":"hi"}"#);
            }
            other => panic!("expected CallResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_call_tool_not_found() {
        let dispatch = make_dispatch(true, false, false);
        let req = PluginIpcRequest::CallTool {
            request_id: "req-1".into(),
            name: "nonexistent".into(),
            arguments: "{}".into(),
            deferred: false,
            context: None,
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::CallResult { request_id, result } => {
                assert_eq!(request_id, "req-1");
                assert!(result.is_err());
            }
            other => panic!("expected CallResult with error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_call_tool_no_tool_returns_error() {
        let dispatch = make_dispatch(false, false, false);
        let req = PluginIpcRequest::CallTool {
            request_id: "req-1".into(),
            name: "test.echo".into(),
            arguments: "{}".into(),
            deferred: false,
            context: None,
        };
        let resp = dispatch_request(&dispatch, &req).await;
        assert!(matches!(resp, PluginIpcResponse::Error { .. }));
    }

    #[tokio::test]
    async fn dispatch_call_tool_deferred_falls_back_to_sync() {
        let dispatch = make_dispatch(true, false, false);
        let req = PluginIpcRequest::CallTool {
            request_id: "req-1".into(),
            name: "test.echo".into(),
            arguments: r#"{"msg":"hi"}"#.into(),
            deferred: true,
            context: None,
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::CallResult { request_id, result } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(result.unwrap().text_for_llm(), r#"{"msg":"hi"}"#);
            }
            other => panic!("expected CallResult (sync fallback), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_chat_completion() {
        let dispatch = make_dispatch(false, true, false);
        let req = PluginIpcRequest::ChatCompletion {
            request_id: "req-1".into(),
            provider_kind: "mock".into(),
            provider_config: serde_json::json!({}),
            model: "mock-model".into(),
            max_tokens: Some(100),
            messages: vec![serde_json::json!({"role": "user", "content": "Hi"})],
            json_schema: None,
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::ChatCompletionResult {
                request_id,
                content,
                usage,
            } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(content, "Mock completion response");
                assert_eq!(usage, None);
            }
            other => panic!("expected ChatCompletionResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_chat_completion_no_llm_returns_error() {
        let dispatch = make_dispatch(false, false, false);
        let req = PluginIpcRequest::ChatCompletion {
            request_id: "req-1".into(),
            provider_kind: "mock".into(),
            provider_config: serde_json::json!({}),
            model: "mock-model".into(),
            max_tokens: Some(100),
            messages: vec![],
            json_schema: None,
        };
        let resp = dispatch_request(&dispatch, &req).await;
        assert!(matches!(resp, PluginIpcResponse::Error { .. }));
    }

    #[tokio::test]
    async fn dispatch_embed_batch() {
        let dispatch = make_dispatch(false, false, true);
        let req = PluginIpcRequest::EmbedBatch {
            request_id: "req-2".into(),
            provider_kind: "mock".into(),
            provider_config: serde_json::json!({}),
            model: "mock-embed".into(),
            dimensions: Some(3),
            items: vec!["hello".into(), "world".into()],
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::EmbedBatchResult {
                request_id,
                embeddings,
            } => {
                assert_eq!(request_id, "req-2");
                assert_eq!(embeddings.len(), 2);
                assert_eq!(embeddings[0], vec![0.1, 0.2, 0.3]);
            }
            other => panic!("expected EmbedBatchResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_embed_batch_no_embed_returns_error() {
        let dispatch = make_dispatch(false, false, false);
        let req = PluginIpcRequest::EmbedBatch {
            request_id: "req-1".into(),
            provider_kind: "mock".into(),
            provider_config: serde_json::json!({}),
            model: "model".into(),
            dimensions: None,
            items: vec![],
        };
        let resp = dispatch_request(&dispatch, &req).await;
        assert!(matches!(resp, PluginIpcResponse::Error { .. }));
    }

    #[tokio::test]
    async fn dispatch_poll_deferred_returns_unknown() {
        let dispatch = make_dispatch(true, false, false);
        let req = PluginIpcRequest::PollDeferred {
            request_id: "req-1".into(),
            task_id: "task-1".into(),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::DeferredStatus {
                request_id,
                task_id,
                status,
            } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(task_id, "task-1");
                assert_eq!(status, DeferredStatus::Unknown);
            }
            other => panic!("expected DeferredStatus, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_ack_variants() {
        let dispatch = make_dispatch(true, false, false);
        let ack_requests = vec![
            PluginIpcRequest::SetCallContext {
                request_id: "req-1".into(),
                conversation_id: "c1".into(),
                turn_id: "t1".into(),
            },
            PluginIpcRequest::ApprovePermission {
                request_id: "req-1".into(),
                permission_request_id: "p1".into(),
            },
            PluginIpcRequest::AllowPattern {
                request_id: "req-1".into(),
                action: "fs_write".into(),
                target_pattern: "/tmp/**".into(),
            },
            PluginIpcRequest::RevokePattern {
                request_id: "req-1".into(),
                action: "fs_write".into(),
                target_pattern: "/tmp/**".into(),
            },
            PluginIpcRequest::CancelDeferred {
                request_id: "req-1".into(),
                task_id: "task-1".into(),
            },
        ];
        for req in &ack_requests {
            let resp = dispatch_request(&dispatch, req).await;
            assert_eq!(
                resp,
                PluginIpcResponse::Ack {
                    request_id: "req-1".into()
                },
                "expected Ack for {req:?}"
            );
        }
    }

    #[tokio::test]
    async fn chat_stream_writes_chunks_and_end() {
        let dispatch = make_dispatch(false, true, false);
        let req = PluginIpcRequest::CreateChatStream {
            request_id: "stream-1".into(),
            provider_kind: "mock".into(),
            provider_config: serde_json::json!({}),
            model: "mock-model".into(),
            max_tokens: None,
            messages: vec![serde_json::json!({"role": "user", "content": "Hi"})],
            tools: vec![],
        };

        let (tx, mut rx) = mpsc::channel::<PluginIpcResponse>(64);
        let token = CancellationToken::new();
        run_chat_stream(&dispatch, &req, tx, token).await;

        let mut responses = Vec::new();
        while let Ok(resp) = rx.try_recv() {
            let is_terminal = matches!(
                resp,
                PluginIpcResponse::StreamEnd { .. } | PluginIpcResponse::StreamError { .. }
            );
            responses.push(resp);
            if is_terminal {
                break;
            }
        }

        assert_eq!(responses.len(), 3);
        assert!(
            matches!(&responses[0], PluginIpcResponse::StreamChunk { text_delta, .. } if text_delta == "Hello")
        );
        assert!(
            matches!(&responses[1], PluginIpcResponse::StreamChunk { text_delta, .. } if text_delta == " world")
        );
        assert!(
            matches!(&responses[2], PluginIpcResponse::StreamEnd { request_id } if request_id == "stream-1")
        );
    }

    #[tokio::test]
    async fn chat_stream_cancel_aborts_mid_stream() {
        // A pre-cancelled token must stop the stream before any chunk is
        // emitted and produce a terminal StreamError (Cr-4).
        let dispatch = make_dispatch(false, true, false);
        let req = PluginIpcRequest::CreateChatStream {
            request_id: "stream-cancel".into(),
            provider_kind: "mock".into(),
            provider_config: serde_json::json!({}),
            model: "mock-model".into(),
            max_tokens: None,
            messages: vec![serde_json::json!({"role": "user", "content": "Hi"})],
            tools: vec![],
        };

        let (tx, mut rx) = mpsc::channel::<PluginIpcResponse>(64);
        let token = CancellationToken::new();
        token.cancel();
        run_chat_stream(&dispatch, &req, tx, token).await;

        let resp = rx.try_recv().unwrap();
        assert!(
            matches!(resp, PluginIpcResponse::StreamError { ref message, .. } if message == "stream cancelled"),
            "expected cancellation StreamError, got {resp:?}"
        );
        // No chunks should follow the cancellation.
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn dispatch_empty_dispatch_returns_default_capabilities() {
        let dispatch = make_dispatch(false, false, false);
        let req = PluginIpcRequest::Handshake {
            version: VersionRange {
                min: PLUGIN_IPC_PROTOCOL_VERSION,
                max: PLUGIN_IPC_PROTOCOL_VERSION,
            },
            sandbox: ene_plugin_proto::SandboxConfigData::default(),
            plugin_config: None,
            plugin_profiles: None,
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::HandshakeAck { capabilities, .. } => {
                assert_eq!(capabilities.tools, 0);
                assert!(capabilities.llm_providers.is_empty());
                assert!(capabilities.tts_providers.is_empty());
                assert!(capabilities.stt_providers.is_empty());
            }
            other => panic!("expected HandshakeAck, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_synthesize_speech_ok() {
        let dispatch = make_tts_dispatch();
        let req = PluginIpcRequest::SynthesizeSpeech {
            request_id: "req-tts-1".into(),
            provider_kind: "mock_tts".into(),
            provider_config: serde_json::json!({}),
            text: "Hello world".into(),
            voice: "default".into(),
            format: "wav".into(),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::SpeechResult {
                request_id,
                audio_base64,
                format,
            } => {
                assert_eq!(request_id, "req-tts-1");
                assert!(!audio_base64.is_empty());
                assert_eq!(format, "wav");
            }
            other => panic!("expected SpeechResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_synthesize_speech_no_tts_returns_error() {
        let dispatch = make_dispatch(false, false, false);
        let req = PluginIpcRequest::SynthesizeSpeech {
            request_id: "req-tts-1".into(),
            provider_kind: "mock_tts".into(),
            provider_config: serde_json::json!({}),
            text: "Hello".into(),
            voice: "default".into(),
            format: "wav".into(),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        assert!(matches!(resp, PluginIpcResponse::Error { .. }));
    }

    #[tokio::test]
    async fn dispatch_transcribe_audio_ok() {
        let dispatch = make_stt_dispatch();
        let req = PluginIpcRequest::TranscribeAudio {
            request_id: "req-stt-1".into(),
            provider_kind: "mock_stt".into(),
            provider_config: serde_json::json!({}),
            audio_base64: base64_encode(b"fake audio"),
            format: "wav".into(),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::TranscriptionResult { request_id, text } => {
                assert_eq!(request_id, "req-stt-1");
                assert_eq!(text, "Mock transcription");
            }
            other => panic!("expected TranscriptionResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_transcribe_audio_no_stt_returns_error() {
        let dispatch = make_dispatch(false, false, false);
        let req = PluginIpcRequest::TranscribeAudio {
            request_id: "req-stt-1".into(),
            provider_kind: "mock_stt".into(),
            provider_config: serde_json::json!({}),
            audio_base64: "AAAA".into(),
            format: "wav".into(),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        assert!(matches!(resp, PluginIpcResponse::Error { .. }));
    }

    #[tokio::test]
    async fn dispatch_transcribe_audio_invalid_base64_returns_error() {
        let dispatch = make_stt_dispatch();
        let req = PluginIpcRequest::TranscribeAudio {
            request_id: "req-stt-1".into(),
            provider_kind: "mock_stt".into(),
            provider_config: serde_json::json!({}),
            audio_base64: "!!!invalid base64!!!".into(),
            format: "wav".into(),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        assert!(matches!(resp, PluginIpcResponse::Error { .. }));
    }

    /// Full `CancelStream` round-trip through `handle_connection` (Cr-4).
    ///
    /// A slow stream emits one chunk and then blocks; a `CancelStream` request
    /// must abort it and produce a terminal `StreamError`, while the read loop
    /// stays responsive enough to acknowledge the cancel.
    #[cfg(unix)]
    #[tokio::test]
    async fn cancel_stream_round_trip() {
        use ene_plugin_proto::{read_plugin_response, write_plugin_request};

        let dispatch = Arc::new(PluginDispatch::new(
            None,
            Some(Arc::new(SlowMockLlmPlugin) as Arc<dyn LlmPlugin>),
            None,
            None,
            None,
        ));
        let shutdown = Arc::new(tokio::sync::Notify::new());

        let (client, server) = tokio::net::UnixStream::pair().expect("unix pair");
        let server_stream = IpcStream::Unix(server);
        tokio::spawn(handle_connection(
            Arc::clone(&dispatch),
            server_stream,
            Arc::clone(&shutdown),
        ));

        let mut client = client;

        // Start a chat stream that will block after the first chunk.
        write_plugin_request(
            &mut client,
            &PluginIpcRequest::CreateChatStream {
                request_id: "stream-1".into(),
                provider_kind: "mock".into(),
                provider_config: serde_json::json!({}),
                model: "mock-model".into(),
                max_tokens: None,
                messages: vec![serde_json::json!({"role": "user", "content": "Hi"})],
                tools: vec![],
            },
        )
        .await
        .expect("write create stream");

        // Read the first (and only) chunk before cancelling.
        let first = read_plugin_response(&mut client)
            .await
            .expect("read")
            .expect("non-EOF");
        assert!(
            matches!(&first, PluginIpcResponse::StreamChunk { text_delta, .. } if text_delta == "partial"),
            "expected partial chunk, got {first:?}"
        );

        // Cancel the in-flight stream.
        write_plugin_request(
            &mut client,
            &PluginIpcRequest::CancelStream {
                request_id: "cancel-1".into(),
                stream_request_id: "stream-1".into(),
            },
        )
        .await
        .expect("write cancel");

        // Collect responses until the terminal StreamError arrives. The cancel
        // Ack and the StreamError may arrive in either order.
        let mut saw_ack = false;
        let mut saw_cancel_error = false;
        for _ in 0..5 {
            let Ok(Some(resp)) =
                tokio::time::timeout(Duration::from_secs(2), read_plugin_response(&mut client))
                    .await
                    .expect("no timeout")
            else {
                break;
            };
            match resp {
                PluginIpcResponse::Ack { request_id } if request_id == "cancel-1" => {
                    saw_ack = true;
                }
                PluginIpcResponse::StreamError {
                    request_id,
                    message,
                } if request_id == "stream-1" && message == "stream cancelled" => {
                    saw_cancel_error = true;
                    break;
                }
                _ => {}
            }
        }

        assert!(saw_ack, "expected CancelStream Ack");
        assert!(
            saw_cancel_error,
            "expected terminal StreamError after cancel"
        );
    }

    /// Two `CallTool` requests on one connection must both be in flight
    /// concurrently (#431). Under the old inline dispatch the read loop blocked
    /// on the first request, so the second was not even read until the first
    /// completed and `in_flight` could never exceed 1.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_tool_calls_overlap_in_flight() {
        use std::sync::Arc as StdArc;

        use ene_plugin_proto::{read_plugin_response, write_plugin_request};

        let plugin = StdArc::new(GatedToolPlugin::new());
        let dispatch = StdArc::new(PluginDispatch::new(
            Some(StdArc::clone(&plugin) as StdArc<dyn ToolPlugin>),
            None,
            None,
            None,
            None,
        ));
        let shutdown = StdArc::new(tokio::sync::Notify::new());

        let (client, server) = tokio::net::UnixStream::pair().expect("unix pair");
        tokio::spawn(handle_connection(
            StdArc::clone(&dispatch),
            IpcStream::Unix(server),
            StdArc::clone(&shutdown),
        ));
        let mut client = client;

        for (id, args) in [("c1", "one"), ("c2", "two")] {
            write_plugin_request(
                &mut client,
                &PluginIpcRequest::CallTool {
                    request_id: id.into(),
                    name: "gated.slow".into(),
                    arguments: args.into(),
                    deferred: false,
                    context: None,
                },
            )
            .await
            .expect("write call");
        }

        // Both calls must reach the plugin and be blocked at the same time.
        wait_for_in_flight(&plugin, 2).await;

        plugin.release();

        let mut got = Vec::new();
        for _ in 0..2 {
            let resp =
                tokio::time::timeout(Duration::from_secs(5), read_plugin_response(&mut client))
                    .await
                    .expect("no timeout")
                    .expect("read ok")
                    .expect("non-EOF");
            if let PluginIpcResponse::CallResult { request_id, result } = resp {
                got.push((request_id, result.expect("ok").text_for_llm()));
            }
        }
        got.sort();
        assert_eq!(
            got,
            vec![
                ("c1".to_string(), "one".to_string()),
                ("c2".to_string(), "two".to_string())
            ]
        );
    }

    /// A `Ping` must be answered while a slow tool call is still in flight
    /// (#431): the read loop dispatches each request in its own task, so the
    /// probe is not queued behind the blocked call.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ping_answered_while_tool_call_in_flight() {
        use std::sync::Arc as StdArc;

        use ene_plugin_proto::{read_plugin_response, write_plugin_request};

        let plugin = StdArc::new(GatedToolPlugin::new());
        let dispatch = StdArc::new(PluginDispatch::new(
            Some(StdArc::clone(&plugin) as StdArc<dyn ToolPlugin>),
            None,
            None,
            None,
            None,
        ));
        let shutdown = StdArc::new(tokio::sync::Notify::new());

        let (client, server) = tokio::net::UnixStream::pair().expect("unix pair");
        tokio::spawn(handle_connection(
            StdArc::clone(&dispatch),
            IpcStream::Unix(server),
            StdArc::clone(&shutdown),
        ));
        let mut client = client;

        write_plugin_request(
            &mut client,
            &PluginIpcRequest::CallTool {
                request_id: "slow".into(),
                name: "gated.slow".into(),
                arguments: "x".into(),
                deferred: false,
                context: None,
            },
        )
        .await
        .expect("write call");

        // Wait until the slow call is in flight, then ping.
        wait_for_in_flight(&plugin, 1).await;

        write_plugin_request(
            &mut client,
            &PluginIpcRequest::Ping {
                request_id: "p1".into(),
            },
        )
        .await
        .expect("write ping");

        // The Pong must arrive while the slow call is still blocked (the gate
        // is not released until after this assertion).
        let resp = tokio::time::timeout(Duration::from_secs(5), read_plugin_response(&mut client))
            .await
            .expect("no timeout")
            .expect("read ok")
            .expect("non-EOF");
        assert!(
            matches!(&resp, PluginIpcResponse::Pong { request_id } if request_id == "p1"),
            "expected Pong while slow call pending, got {resp:?}"
        );
        assert_eq!(plugin.in_flight(), 1, "slow call must still be blocked");

        plugin.release();
    }
}
