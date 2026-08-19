use std::sync::Arc;

use ene_api::{
    ApiClient, ApiError, ClaimResourceRequest, CreateSessionRequest, HistoryResponse,
    ListenRequest, MessageMode, MessageRequest, ResourceKind, SendMessageResponse, SoulPatch,
    SoulView,
};
use parking_lot::Mutex;

/// Active stage session bound to one soul and surface history.
pub struct StageSession {
    client: Arc<ApiClient>,
    soul_id: String,
    session_id: String,
    turn_id: Mutex<Option<String>>,
    history: Mutex<HistoryResponse>,
}

impl StageSession {
    #[must_use]
    pub fn client(&self) -> &Arc<ApiClient> {
        &self.client
    }

    #[must_use]
    pub fn soul_id(&self) -> &str {
        &self.soul_id
    }

    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    #[must_use]
    pub fn turn_id(&self) -> Option<String> {
        self.turn_id.lock().clone()
    }

    #[must_use]
    pub fn history(&self) -> HistoryResponse {
        self.history.lock().clone()
    }

    /// Resolve a soul, open or create a session, and load surface history.
    ///
    /// Prefers stage occupants, then the first listed soul. This blocks the
    /// current async runtime thread only when called from sync context; prefer
    /// awaiting it on the app's Tokio handle.
    pub async fn bootstrap(client: Arc<ApiClient>) -> Result<Self, ApiError> {
        let soul_id = resolve_soul_id(&client).await?;
        let session_id = resolve_session_id(&client, &soul_id).await?;
        let history = client.history(&session_id, "surface").await?;
        Ok(Self {
            client,
            soul_id,
            session_id,
            turn_id: Mutex::new(None),
            history: Mutex::new(history),
        })
    }

    pub async fn send_prompt(&self, text: &str) -> Result<SendMessageResponse, ApiError> {
        let response = self
            .client
            .send_message(
                &self.session_id,
                &MessageRequest {
                    text: text.to_owned(),
                    mode: MessageMode::Prompt,
                    input_modality: None,
                },
                None,
            )
            .await?;
        if let Some(turn_id) = response.turn_id.clone() {
            *self.turn_id.lock() = Some(turn_id);
        }
        Ok(response)
    }

    pub async fn barge_in(&self) -> Result<serde_json::Value, ApiError> {
        self.client.barge_in(&self.session_id).await
    }

    pub async fn cancel_turn(&self) -> Result<serde_json::Value, ApiError> {
        let turn_id = self
            .turn_id
            .lock()
            .clone()
            .ok_or_else(|| ApiError::Transport("no active turn".to_owned()))?;
        self.client.cancel_turn(&turn_id).await
    }

    pub async fn claim_mic(&self) -> Result<ene_api::ExclusiveSnapshot, ApiError> {
        self.claim(ResourceKind::Mic).await
    }

    pub async fn release_mic(&self) -> Result<ene_api::ExclusiveSnapshot, ApiError> {
        self.release(ResourceKind::Mic).await
    }

    pub async fn claim_speaker(&self) -> Result<ene_api::ExclusiveSnapshot, ApiError> {
        self.claim(ResourceKind::Speaker).await
    }

    pub async fn release_speaker(&self) -> Result<ene_api::ExclusiveSnapshot, ApiError> {
        self.release(ResourceKind::Speaker).await
    }

    pub async fn claim_notify(&self) -> Result<ene_api::ExclusiveSnapshot, ApiError> {
        self.claim(ResourceKind::Notify).await
    }

    pub async fn release_notify(&self) -> Result<ene_api::ExclusiveSnapshot, ApiError> {
        self.release(ResourceKind::Notify).await
    }

    pub async fn refresh_history(&self) -> Result<(), ApiError> {
        let history = self.client.history(&self.session_id, "surface").await?;
        *self.history.lock() = history;
        Ok(())
    }

    pub async fn respond_approval(&self, id: &str, decision: &str) -> Result<serde_json::Value, ApiError> {
        self.client.respond_approval(id, decision).await
    }

    pub async fn listen_pcm(
        &self,
        pcm: Vec<f32>,
        sample_rate: u32,
    ) -> Result<SendMessageResponse, ApiError> {
        let response = self
            .client
            .listen(
                &self.session_id,
                &ListenRequest { pcm, sample_rate },
            )
            .await?;
        if let Some(turn_id) = response.turn_id.clone() {
            *self.turn_id.lock() = Some(turn_id);
        }
        Ok(response)
    }

    pub async fn get_soul(&self) -> Result<SoulView, ApiError> {
        self.client.get_soul(&self.soul_id).await
    }

    pub async fn patch_soul_body(&self, body_ref: &str) -> Result<SoulView, ApiError> {
        self.client
            .patch_soul_body(
                &self.soul_id,
                &SoulPatch {
                    body_ref: Some(body_ref.to_owned()),
                },
            )
            .await
    }

    pub async fn delete_memory(&self, memory_id: &str) -> Result<(), ApiError> {
        self.client.delete_memory(memory_id).await
    }

    async fn claim(&self, kind: ResourceKind) -> Result<ene_api::ExclusiveSnapshot, ApiError> {
        self.client
            .claim_resource(
                kind,
                &ClaimResourceRequest {
                    client_id: self.client.client_id().to_owned(),
                },
            )
            .await
    }

    async fn release(&self, kind: ResourceKind) -> Result<ene_api::ExclusiveSnapshot, ApiError> {
        self.client.release_resource(kind).await
    }
}

async fn resolve_soul_id(client: &ApiClient) -> Result<String, ApiError> {
    let stage = client.stage().await?;
    if let Some(occupant) = stage.occupants.first() {
        return Ok(occupant.soul_id.clone());
    }
    let souls = client.list_souls().await?;
    souls
        .items
        .first()
        .map(|soul| soul.id.clone())
        .ok_or_else(|| ApiError::Transport("no souls available".to_owned()))
}

async fn resolve_session_id(client: &ApiClient, soul_id: &str) -> Result<String, ApiError> {
    let page = client.list_sessions(Some(soul_id)).await?;
    if let Some(session) = page
        .items
        .iter()
        .find(|session| !session.archived && session.ended_at.is_none())
    {
        return Ok(session.id.clone());
    }
    let created = client
        .create_session(&CreateSessionRequest {
            soul_id: soul_id.to_owned(),
            title: None,
        })
        .await?;
    Ok(created.id)
}
