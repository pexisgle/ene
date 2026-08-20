use crate::error::IpcError;
use crate::frame::{read_frame, write_frame};
use crate::protocol::{
    HelloAck, HostHello, Message, Negotiated, ProtoId, ProtocolRanges, ToolCall, ToolResult,
    ToolSpecWire, VersionRange,
};
use crate::provider::{
    EmbedRequest, EmbedResult, InstallAssetRequest, InstallAssetResult, InstallStatusRequest,
    InstallStatusResult, ListAssetsResult, ListModelsRequest, ListModelsResult, LlmGenerateRequest,
    LlmGeneration, ProviderFaces, SetActiveAssetRequest, SetActiveAssetResult, SttRequest,
    SttResult, TtsAudio, TtsRequest,
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
        let max_frame = hello.frame_limit();
        write_frame(
            &mut stream,
            &Message::Hello {
                body: hello.clone(),
            }
            .encode()?,
            max_frame,
        )
        .await?;
        let bytes = read_frame(&mut stream, max_frame).await?;
        match Message::decode(&bytes)? {
            Message::HelloAck { body } => {
                validate_ack(&hello, &body, plugin_declared, expected_spawn_token)?;
                Ok(Self {
                    stream,
                    next_id: 1,
                    negotiated: body.protocols,
                    max_frame,
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

    pub async fn generate_llm(
        &mut self,
        request: LlmGenerateRequest,
    ) -> Result<LlmGeneration, IpcError> {
        if self
            .negotiated
            .provider
            .as_ref()
            .and_then(|faces| faces.llm)
            .is_none()
        {
            return Err(IpcError::Unexpected("llm disabled".to_owned()));
        }
        let id = self.alloc();
        self.send(&Message::LlmGenerate { id, body: request })
            .await?;
        loop {
            match self.recv().await? {
                Message::LlmChunk { id: got, .. } if got == id => {}
                Message::LlmDone { id: got, body } if got == id => return Ok(body),
                Message::LlmDone { .. } => {
                    tracing::debug!("ignoring stray llm_done after cancel or id mismatch");
                }
                other => return Err(IpcError::Unexpected(other.kind_name().to_owned())),
            }
        }
    }

    pub async fn embed(&mut self, request: EmbedRequest) -> Result<EmbedResult, IpcError> {
        if self
            .negotiated
            .provider
            .as_ref()
            .and_then(|faces| faces.embed)
            .is_none()
        {
            return Err(IpcError::Unexpected("embed disabled".to_owned()));
        }
        let id = self.alloc();
        self.send(&Message::EmbedEncode { id, body: request })
            .await?;
        match self.recv().await? {
            Message::EmbedResult { id: got, body } if got == id => Ok(body),
            other => Err(IpcError::Unexpected(other.kind_name().to_owned())),
        }
    }

    pub async fn synthesize_tts(&mut self, request: TtsRequest) -> Result<TtsAudio, IpcError> {
        if self
            .negotiated
            .provider
            .as_ref()
            .and_then(|faces| faces.tts)
            .is_none()
        {
            return Err(IpcError::Unexpected("tts disabled".to_owned()));
        }
        let id = self.alloc();
        self.send(&Message::TtsSynthesize { id, body: request })
            .await?;
        match self.recv().await? {
            Message::TtsResult { id: got, body } if got == id => Ok(body),
            other => Err(IpcError::Unexpected(other.kind_name().to_owned())),
        }
    }

    pub async fn transcribe(&mut self, request: SttRequest) -> Result<SttResult, IpcError> {
        if self
            .negotiated
            .provider
            .as_ref()
            .and_then(|faces| faces.stt)
            .is_none()
        {
            return Err(IpcError::Unexpected("stt disabled".to_owned()));
        }
        let id = self.alloc();
        self.send(&Message::SttTranscribe { id, body: request })
            .await?;
        match self.recv().await? {
            Message::SttResult { id: got, body } if got == id => Ok(body),
            other => Err(IpcError::Unexpected(other.kind_name().to_owned())),
        }
    }

    pub async fn list_models(
        &mut self,
        request: ListModelsRequest,
    ) -> Result<ListModelsResult, IpcError> {
        if self
            .negotiated
            .provider
            .as_ref()
            .and_then(|faces| faces.models)
            .is_none()
        {
            return Ok(ListModelsResult::default());
        }
        let id = self.alloc();
        self.send(&Message::ProviderListModels { id, body: request })
            .await?;
        match self.recv().await? {
            Message::ProviderModels { id: got, body } if got == id => Ok(body),
            other => Err(IpcError::Unexpected(other.kind_name().to_owned())),
        }
    }

    pub async fn list_assets(&mut self) -> Result<ListAssetsResult, IpcError> {
        if self
            .negotiated
            .provider
            .as_ref()
            .and_then(|faces| faces.assets)
            .is_none()
        {
            return Ok(ListAssetsResult::default());
        }
        let id = self.alloc();
        self.send(&Message::ProviderListAssets { id }).await?;
        match self.recv().await? {
            Message::ProviderAssets { id: got, body } if got == id => Ok(body),
            other => Err(IpcError::Unexpected(other.kind_name().to_owned())),
        }
    }

    pub async fn install_asset(
        &mut self,
        request: InstallAssetRequest,
    ) -> Result<InstallAssetResult, IpcError> {
        if self
            .negotiated
            .provider
            .as_ref()
            .and_then(|faces| faces.assets)
            .is_none()
        {
            return Err(IpcError::Unexpected("assets disabled".to_owned()));
        }
        let id = self.alloc();
        self.send(&Message::ProviderInstallAsset { id, body: request })
            .await?;
        match self.recv().await? {
            Message::ProviderInstallAssetAck { id: got, body } if got == id => Ok(body),
            other => Err(IpcError::Unexpected(other.kind_name().to_owned())),
        }
    }

    pub async fn install_status(
        &mut self,
        request: InstallStatusRequest,
    ) -> Result<InstallStatusResult, IpcError> {
        if self
            .negotiated
            .provider
            .as_ref()
            .and_then(|faces| faces.assets)
            .is_none()
        {
            return Err(IpcError::Unexpected("assets disabled".to_owned()));
        }
        let id = self.alloc();
        self.send(&Message::ProviderInstallStatus { id, body: request })
            .await?;
        match self.recv().await? {
            Message::ProviderInstallStatusResult { id: got, body } if got == id => Ok(body),
            other => Err(IpcError::Unexpected(other.kind_name().to_owned())),
        }
    }

    pub async fn set_active_asset(
        &mut self,
        request: SetActiveAssetRequest,
    ) -> Result<SetActiveAssetResult, IpcError> {
        if self
            .negotiated
            .provider
            .as_ref()
            .and_then(|faces| faces.assets)
            .is_none()
        {
            return Err(IpcError::Unexpected("assets disabled".to_owned()));
        }
        let id = self.alloc();
        self.send(&Message::ProviderSetActiveAsset { id, body: request })
            .await?;
        match self.recv().await? {
            Message::ProviderSetActiveAssetResult { id: got, body } if got == id => Ok(body),
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
    if ack.manifest_digest != hello.expected_digest && !hello.allow_unverified {
        return Err(IpcError::DigestMismatch);
    }
    if ack.spawn_token != expected_spawn_token {
        return Err(IpcError::DigestMismatch);
    }
    if ack.protocols.tool.is_some() && !declared.contains(&ProtoId::Tool) {
        return Err(IpcError::UndeclaredProtocol("tool".to_owned()));
    }
    if ack.protocols.provider.is_some() && !declared.contains(&ProtoId::Provider) {
        return Err(IpcError::UndeclaredProtocol("provider".to_owned()));
    }
    Ok(())
}

/// Negotiate versions. Core mismatch is an error; other faces disable independently.
pub fn negotiate(
    host: &ProtocolRanges,
    plugin_core: VersionRange,
    plugin_tool: Option<VersionRange>,
    plugin_provider: Option<ProviderFaces>,
) -> Result<Negotiated, IpcError> {
    let core = host
        .core
        .negotiate(plugin_core)
        .ok_or(IpcError::CoreIncompatible)?;
    let tool = match (host.tool, plugin_tool) {
        (Some(host_tool), Some(plugin_tool)) => host_tool.negotiate(plugin_tool),
        _ => None,
    };
    Ok(Negotiated {
        core,
        tool,
        provider: negotiate_faces(host.provider, plugin_provider),
    })
}

fn negotiate_faces(
    host: Option<VersionRange>,
    offered: Option<ProviderFaces>,
) -> Option<ProviderFaces> {
    let host = host?;
    let offered = offered?;
    let pick = |version: Option<u32>| {
        version.and_then(|version| host.negotiate(VersionRange::exact(version)))
    };
    let faces = ProviderFaces {
        llm: pick(offered.llm),
        embed: pick(offered.embed),
        tts: pick(offered.tts),
        stt: pick(offered.stt),
        models: pick(offered.models),
        assets: pick(offered.assets),
        vad: pick(offered.vad),
    };
    (!faces.is_empty()).then_some(faces)
}
