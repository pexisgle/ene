use std::sync::Arc;

use crate::error::IpcError;
use crate::frame::{MAX_FRAME_BYTES, read_frame, write_frame};
use crate::host::negotiate;
use crate::protocol::{CORE_VERSION, HelloAck, HostHello, Message, ProtoId, VersionRange};
use crate::provider::{
    EmbedRequest, EmbedResult, InstallAssetRequest, InstallAssetResult, InstallStatusRequest,
    InstallStatusResult, ListAssetsResult, ListModelsRequest, ListModelsResult, LlmGenerateRequest,
    LlmGeneration, PROVIDER_ASSETS_VERSION, PROVIDER_EMBED_VERSION, PROVIDER_LLM_VERSION,
    PROVIDER_MODELS_VERSION, PROVIDER_STT_VERSION, PROVIDER_TTS_VERSION, ProviderFaces,
    SetActiveAssetRequest, SetActiveAssetResult, SttRequest, SttResult, TtsAudio, TtsRequest,
};
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

/// Identity advertised in `hello_ack`.
#[derive(Debug, Clone)]
pub struct PluginIdentity {
    pub plugin_id: String,
    pub plugin_name: String,
    pub digest: String,
    pub spawn_token: Option<String>,
}

impl PluginIdentity {
    fn spawn_token(&self) -> Result<String, String> {
        if let Some(token) = &self.spawn_token {
            return Ok(token.clone());
        }
        std::env::var("ENE_PLUGIN_SPAWN_TOKEN")
            .map_err(|_| "ENE_PLUGIN_SPAWN_TOKEN is not set".to_owned())
    }
}

/// LLM generate face.
#[async_trait]
pub trait LlmHandler: Send + Sync {
    async fn generate(&self, request: LlmGenerateRequest) -> Result<LlmGeneration, IpcError>;
}

/// Embedding face.
#[async_trait]
pub trait EmbedHandler: Send + Sync {
    async fn encode(&self, request: EmbedRequest) -> Result<EmbedResult, IpcError>;
}

/// TTS face. Returns mono PCM.
#[async_trait]
pub trait TtsHandler: Send + Sync {
    async fn synthesize(&self, request: TtsRequest) -> Result<TtsAudio, IpcError>;
}

/// STT face. Accepts mono PCM.
#[async_trait]
pub trait SttHandler: Send + Sync {
    async fn transcribe(&self, request: SttRequest) -> Result<SttResult, IpcError>;
}

/// Optional `provider.list_models` face. Missing handler yields an empty list.
#[async_trait]
pub trait ModelsHandler: Send + Sync {
    async fn list_models(&self, request: ListModelsRequest) -> Result<ListModelsResult, IpcError>;
}

/// Optional `provider.assets` face for catalogued sidecars and weights.
#[async_trait]
pub trait AssetsHandler: Send + Sync {
    async fn list_assets(&self) -> Result<ListAssetsResult, IpcError>;
    async fn install_asset(
        &self,
        request: InstallAssetRequest,
    ) -> Result<InstallAssetResult, IpcError>;
    async fn install_status(
        &self,
        request: InstallStatusRequest,
    ) -> Result<InstallStatusResult, IpcError>;
    async fn set_active(
        &self,
        request: SetActiveAssetRequest,
    ) -> Result<SetActiveAssetResult, IpcError>;
}

/// Provider faces implemented by one plugin process.
#[derive(Clone, Default)]
pub struct ProviderHandlers {
    pub llm: Option<Arc<dyn LlmHandler>>,
    pub embed: Option<Arc<dyn EmbedHandler>>,
    pub tts: Option<Arc<dyn TtsHandler>>,
    pub stt: Option<Arc<dyn SttHandler>>,
    pub models: Option<Arc<dyn ModelsHandler>>,
    pub assets: Option<Arc<dyn AssetsHandler>>,
}

fn offered_faces(hello: &HostHello, handlers: &ProviderHandlers) -> Option<ProviderFaces> {
    if !hello.declared_protocols.contains(&ProtoId::Provider) {
        return None;
    }
    let faces = ProviderFaces {
        llm: handlers.llm.as_ref().map(|_| PROVIDER_LLM_VERSION),
        embed: handlers.embed.as_ref().map(|_| PROVIDER_EMBED_VERSION),
        tts: handlers.tts.as_ref().map(|_| PROVIDER_TTS_VERSION),
        stt: handlers.stt.as_ref().map(|_| PROVIDER_STT_VERSION),
        models: handlers.models.as_ref().map(|_| PROVIDER_MODELS_VERSION),
        assets: handlers.assets.as_ref().map(|_| PROVIDER_ASSETS_VERSION),
        vad: None,
    };
    (!faces.is_empty()).then_some(faces)
}

/// Serve a provider plugin until drain/shutdown.
pub async fn serve_provider<S>(
    mut stream: S,
    identity: PluginIdentity,
    handlers: ProviderHandlers,
) -> Result<(), IpcError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let hello = expect_hello(&mut stream).await?;
    let max_frame = hello.frame_limit();
    let ack = match build_ack(&hello, &identity, &handlers) {
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
        let bytes = read_frame(&mut stream, max_frame).await?;
        match Message::decode(&bytes)? {
            Message::Ping { id } => {
                write_msg(&mut stream, &Message::Pong { id }, max_frame).await?;
            }
            Message::LlmGenerate { id, body } => {
                let reply = dispatch_llm(handlers.llm.as_ref(), body).await;
                write_msg(
                    &mut stream,
                    &Message::LlmDone { id, body: reply },
                    max_frame,
                )
                .await?;
            }
            Message::EmbedEncode { id, body } => {
                let reply = dispatch_embed(handlers.embed.as_ref(), body).await;
                write_msg(
                    &mut stream,
                    &Message::EmbedResult { id, body: reply },
                    max_frame,
                )
                .await?;
            }
            Message::TtsSynthesize { id, body } => {
                let reply = dispatch_tts(handlers.tts.as_ref(), body).await;
                write_msg(
                    &mut stream,
                    &Message::TtsResult { id, body: reply },
                    max_frame,
                )
                .await?;
            }
            Message::SttTranscribe { id, body } => {
                let reply = dispatch_stt(handlers.stt.as_ref(), body).await;
                write_msg(
                    &mut stream,
                    &Message::SttResult { id, body: reply },
                    max_frame,
                )
                .await?;
            }
            Message::ProviderListModels { id, body } => {
                let reply = dispatch_models(handlers.models.as_ref(), body).await;
                write_msg(
                    &mut stream,
                    &Message::ProviderModels { id, body: reply },
                    max_frame,
                )
                .await?;
            }
            Message::ProviderListAssets { id } => {
                let reply = dispatch_assets_list(handlers.assets.as_ref()).await;
                write_msg(
                    &mut stream,
                    &Message::ProviderAssets { id, body: reply },
                    max_frame,
                )
                .await?;
            }
            Message::ProviderInstallAsset { id, body } => {
                let reply = dispatch_assets_install(handlers.assets.as_ref(), body).await;
                write_msg(
                    &mut stream,
                    &Message::ProviderInstallAssetAck { id, body: reply },
                    max_frame,
                )
                .await?;
            }
            Message::ProviderInstallStatus { id, body } => {
                let reply = dispatch_assets_install_status(handlers.assets.as_ref(), body).await;
                write_msg(
                    &mut stream,
                    &Message::ProviderInstallStatusResult { id, body: reply },
                    max_frame,
                )
                .await?;
            }
            Message::ProviderSetActiveAsset { id, body } => {
                let reply = dispatch_assets_set_active(handlers.assets.as_ref(), body).await;
                write_msg(
                    &mut stream,
                    &Message::ProviderSetActiveAssetResult { id, body: reply },
                    max_frame,
                )
                .await?;
            }
            Message::Drain { id } | Message::Shutdown { id } => {
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
                write_msg(&mut stream, &Message::CapabilityReleased { id }, max_frame).await?;
            }
            Message::FlowControl { .. } | Message::Log { .. } => {}
            other => return Err(IpcError::Unexpected(other.kind_name().to_owned())),
        }
    }
}

async fn dispatch_llm(
    handler: Option<&Arc<dyn LlmHandler>>,
    body: LlmGenerateRequest,
) -> LlmGeneration {
    match handler {
        Some(handler) => handler
            .generate(body)
            .await
            .unwrap_or_else(|err| LlmGeneration {
                text: String::new(),
                finish_reason: "error".to_owned(),
                model_id: err.to_string(),
                ..LlmGeneration::default()
            }),
        None => LlmGeneration {
            finish_reason: "error".to_owned(),
            model_id: "llm disabled".to_owned(),
            ..LlmGeneration::default()
        },
    }
}

async fn dispatch_embed(
    handler: Option<&Arc<dyn EmbedHandler>>,
    body: EmbedRequest,
) -> EmbedResult {
    match handler {
        Some(handler) => handler.encode(body).await.unwrap_or_else(|_| EmbedResult {
            vectors: Vec::new(),
        }),
        None => EmbedResult {
            vectors: Vec::new(),
        },
    }
}

async fn dispatch_tts(handler: Option<&Arc<dyn TtsHandler>>, body: TtsRequest) -> TtsAudio {
    match handler {
        Some(handler) => handler.synthesize(body).await.unwrap_or_else(|_| TtsAudio {
            pcm: Vec::new(),
            sample_rate: 16_000,
            bulk: None,
        }),
        None => TtsAudio {
            pcm: Vec::new(),
            sample_rate: 16_000,
            bulk: None,
        },
    }
}

async fn dispatch_stt(handler: Option<&Arc<dyn SttHandler>>, body: SttRequest) -> SttResult {
    match handler {
        Some(handler) => handler
            .transcribe(body)
            .await
            .unwrap_or_else(|err| SttResult {
                text: format!("error: {err}"),
            }),
        None => SttResult {
            text: "stt disabled".to_owned(),
        },
    }
}

async fn dispatch_models(
    handler: Option<&Arc<dyn ModelsHandler>>,
    body: ListModelsRequest,
) -> ListModelsResult {
    match handler {
        Some(handler) => handler
            .list_models(body)
            .await
            .unwrap_or_else(|err| ListModelsResult {
                models: Vec::new(),
                error: Some(err.to_string()),
            }),
        None => ListModelsResult::default(),
    }
}

async fn dispatch_assets_list(handler: Option<&Arc<dyn AssetsHandler>>) -> ListAssetsResult {
    match handler {
        Some(handler) => handler
            .list_assets()
            .await
            .unwrap_or_else(|err| ListAssetsResult {
                assets: Vec::new(),
                error: Some(err.to_string()),
            }),
        None => ListAssetsResult::default(),
    }
}

async fn dispatch_assets_install(
    handler: Option<&Arc<dyn AssetsHandler>>,
    body: InstallAssetRequest,
) -> InstallAssetResult {
    match handler {
        Some(handler) => {
            handler
                .install_asset(body)
                .await
                .unwrap_or_else(|err| InstallAssetResult {
                    job_id: String::new(),
                    error: Some(err.to_string()),
                })
        }
        None => InstallAssetResult {
            job_id: String::new(),
            error: Some("assets disabled".to_owned()),
        },
    }
}

async fn dispatch_assets_install_status(
    handler: Option<&Arc<dyn AssetsHandler>>,
    body: InstallStatusRequest,
) -> InstallStatusResult {
    match handler {
        Some(handler) => {
            handler
                .install_status(body)
                .await
                .unwrap_or_else(|err| InstallStatusResult {
                    error: Some(err.to_string()),
                    ..InstallStatusResult::default()
                })
        }
        None => InstallStatusResult {
            error: Some("assets disabled".to_owned()),
            ..InstallStatusResult::default()
        },
    }
}

async fn dispatch_assets_set_active(
    handler: Option<&Arc<dyn AssetsHandler>>,
    body: SetActiveAssetRequest,
) -> SetActiveAssetResult {
    match handler {
        Some(handler) => {
            handler
                .set_active(body)
                .await
                .unwrap_or_else(|err| SetActiveAssetResult {
                    error: Some(err.to_string()),
                })
        }
        None => SetActiveAssetResult {
            error: Some("assets disabled".to_owned()),
        },
    }
}

async fn expect_hello<S: AsyncRead + Unpin>(stream: &mut S) -> Result<HostHello, IpcError> {
    let bytes = read_frame(stream, MAX_FRAME_BYTES).await?;
    match Message::decode(&bytes)? {
        Message::Hello { body } => Ok(body),
        other => Err(IpcError::Unexpected(other.kind_name().to_owned())),
    }
}

fn build_ack(
    hello: &HostHello,
    identity: &PluginIdentity,
    handlers: &ProviderHandlers,
) -> Result<HelloAck, String> {
    if !hello.declared_protocols.contains(&ProtoId::Core) {
        return Err("manifest must declare core".to_owned());
    }
    if identity.digest != hello.expected_digest && !hello.allow_unverified {
        return Err("manifest digest mismatch".to_owned());
    }
    let plugin_tool = hello
        .declared_protocols
        .contains(&ProtoId::Tool)
        .then_some(VersionRange::exact(crate::protocol::TOOL_VERSION));
    let plugin_capability = hello
        .declared_protocols
        .contains(&ProtoId::Capability)
        .then_some(VersionRange::exact(crate::protocol::CAPABILITY_VERSION));
    let negotiated = negotiate(
        &hello.protocols,
        VersionRange::exact(CORE_VERSION),
        plugin_tool,
        offered_faces(hello, handlers),
        plugin_capability,
    )
    .map_err(|err| err.to_string())?;
    let spawn_token = identity.spawn_token()?;
    Ok(HelloAck {
        plugin_id: identity.plugin_id.clone(),
        plugin_name: identity.plugin_name.clone(),
        plugin_version: "0.1.0".to_owned(),
        manifest_digest: identity.digest.clone(),
        protocols: negotiated,
        spawn_token,
        has_config: false,
    })
}

async fn write_msg<S: AsyncWrite + Unpin>(
    stream: &mut S,
    message: &Message,
    max_frame: usize,
) -> Result<(), IpcError> {
    write_frame(stream, &message.encode()?, max_frame).await
}

/// Connect to `ENE_PLUGIN_SOCKET` and serve provider faces until drain.
pub async fn serve_provider_from_env(
    identity: PluginIdentity,
    handlers: ProviderHandlers,
) -> Result<(), IpcError> {
    let path = std::env::var("ENE_PLUGIN_SOCKET").map_err(|_| {
        IpcError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "ENE_PLUGIN_SOCKET is not set",
        ))
    })?;
    #[cfg(unix)]
    {
        let stream = tokio::net::UnixStream::connect(&path).await?;
        serve_provider(stream, identity, handlers).await
    }
    #[cfg(windows)]
    {
        let stream = tokio::net::TcpStream::connect(&path).await?;
        serve_provider(stream, identity, handlers).await
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, identity, handlers);
        Err(IpcError::Io(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "plugin IPC requires Unix domain sockets or Windows TCP",
        )))
    }
}
