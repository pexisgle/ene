use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use ene_api::{
    ApiClient, ApiError, CharacterView, ClaimResourceRequest, CreateSessionRequest,
    HistoryResponse, ListenRequest, MessageMode, MessageRequest, OccupantView, ResourceKind,
    SendMessageResponse, SoulPatch, SoulView,
};
use parking_lot::Mutex;
use uuid::Uuid;

use crate::bundle::{self, motions_dir_for_package};

/// Active stage session bound to one soul and surface history.
pub struct StageSession {
    client: Arc<ApiClient>,
    soul_id: String,
    session_id: String,
    turn_id: Arc<Mutex<Option<String>>>,
    history: Arc<Mutex<HistoryResponse>>,
    occupants: Vec<OccupantView>,
    avatar_path: Option<PathBuf>,
    motions_dir: Option<PathBuf>,
}

/// Cheap clone used by async tasks spawned from the UI thread.
#[derive(Clone)]
pub struct SessionHandle {
    client: Arc<ApiClient>,
    #[expect(
        dead_code,
        reason = "kept so a handle can retarget later without a StageSession"
    )]
    soul_id: String,
    session_id: String,
    turn_id: Arc<Mutex<Option<String>>>,
    history: Arc<Mutex<HistoryResponse>>,
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

    pub fn clear_turn(&self) {
        *self.turn_id.lock() = None;
    }

    #[must_use]
    pub fn history(&self) -> HistoryResponse {
        self.history.lock().clone()
    }

    pub fn replace_history(&self, history: HistoryResponse) {
        *self.history.lock() = history;
    }

    #[must_use]
    pub fn occupants(&self) -> &[OccupantView] {
        &self.occupants
    }

    #[must_use]
    pub fn avatar_path(&self) -> Option<&PathBuf> {
        self.avatar_path.as_ref()
    }

    #[must_use]
    pub fn motions_dir(&self) -> Option<&PathBuf> {
        self.motions_dir.as_ref()
    }

    #[must_use]
    pub fn clone_handle(&self) -> SessionHandle {
        SessionHandle {
            client: Arc::clone(&self.client),
            soul_id: self.soul_id.clone(),
            session_id: self.session_id.clone(),
            turn_id: Arc::clone(&self.turn_id),
            history: Arc::clone(&self.history),
        }
    }

    /// Resolve a soul (preferring an occupant with an avatar), open a session, load history.
    pub async fn bootstrap(client: Arc<ApiClient>) -> Result<Self, ApiError> {
        let (soul_id, occupants, avatar_path, motions_dir) = resolve_stage(&client).await?;
        let session_id = resolve_session_id(&client, &soul_id).await?;
        let history = client.history(&session_id, "surface").await?;
        Ok(Self {
            client,
            soul_id,
            session_id,
            turn_id: Arc::new(Mutex::new(None)),
            history: Arc::new(Mutex::new(history)),
            occupants,
            avatar_path,
            motions_dir,
        })
    }

    pub async fn retarget_soul(&mut self, soul_id: &str) -> Result<(), ApiError> {
        let session_id = resolve_session_id(&self.client, soul_id).await?;
        let history = self.client.history(&session_id, "surface").await?;
        soul_id.clone_into(&mut self.soul_id);
        self.session_id = session_id;
        *self.turn_id.lock() = None;
        *self.history.lock() = history;
        if let Some(occupant) = self.occupants.iter().find(|item| item.soul_id == soul_id) {
            self.avatar_path = occupant.avatar_path.as_ref().map(PathBuf::from);
            self.motions_dir = occupant
                .package_id
                .as_ref()
                .and(occupant.avatar_path.as_ref())
                .and_then(|avatar| {
                    PathBuf::from(avatar)
                        .parent()
                        .and_then(std::path::Path::parent)
                        .map(|body| body.join("motions"))
                });
        }
        Ok(())
    }

    pub async fn send_prompt(&self, text: &str) -> Result<SendMessageResponse, ApiError> {
        self.send(text, MessageMode::Prompt).await
    }

    pub async fn send_steer(&self, text: &str) -> Result<SendMessageResponse, ApiError> {
        self.send(text, MessageMode::Steer).await
    }

    pub async fn send_follow_up(&self, text: &str) -> Result<SendMessageResponse, ApiError> {
        self.send(text, MessageMode::FollowUp).await
    }

    async fn send(&self, text: &str, mode: MessageMode) -> Result<SendMessageResponse, ApiError> {
        let key = Uuid::new_v4().to_string();
        let response = self
            .client
            .send_message(
                &self.session_id,
                &MessageRequest {
                    text: text.to_owned(),
                    mode,
                    input_modality: None,
                },
                Some(&key),
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

    pub async fn claim_notify(&self) -> Result<ene_api::ExclusiveSnapshot, ApiError> {
        self.claim(ResourceKind::Notify).await
    }

    pub async fn refresh_history(&self) -> Result<HistoryResponse, ApiError> {
        let history = self.client.history(&self.session_id, "surface").await?;
        *self.history.lock() = history.clone();
        Ok(history)
    }

    pub async fn respond_approval(
        &self,
        id: &str,
        decision: &str,
    ) -> Result<serde_json::Value, ApiError> {
        self.client.respond_approval(id, decision).await
    }

    pub async fn listen_pcm(
        &self,
        pcm: Vec<f32>,
        sample_rate: u32,
    ) -> Result<SendMessageResponse, ApiError> {
        let response = self
            .client
            .listen(&self.session_id, &ListenRequest { pcm, sample_rate })
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

impl SessionHandle {
    pub async fn refresh_history(&self) -> Result<HistoryResponse, ApiError> {
        let history = self.client.history(&self.session_id, "surface").await?;
        *self.history.lock() = history.clone();
        Ok(history)
    }

    pub async fn send(
        &self,
        text: &str,
        mode: MessageMode,
    ) -> Result<SendMessageResponse, ApiError> {
        let key = Uuid::new_v4().to_string();
        let response = self
            .client
            .send_message(
                &self.session_id,
                &MessageRequest {
                    text: text.to_owned(),
                    mode,
                    input_modality: None,
                },
                Some(&key),
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

    pub async fn respond_approval(
        &self,
        id: &str,
        decision: &str,
    ) -> Result<serde_json::Value, ApiError> {
        self.client.respond_approval(id, decision).await
    }

    pub async fn listen_pcm(
        &self,
        pcm: Vec<f32>,
        sample_rate: u32,
    ) -> Result<SendMessageResponse, ApiError> {
        let response = self
            .client
            .listen(&self.session_id, &ListenRequest { pcm, sample_rate })
            .await?;
        if let Some(turn_id) = response.turn_id.clone() {
            *self.turn_id.lock() = Some(turn_id);
        }
        Ok(response)
    }

    pub async fn claim_mic(&self) -> Result<ene_api::ExclusiveSnapshot, ApiError> {
        self.claim(ResourceKind::Mic).await
    }

    pub async fn release_mic(&self) -> Result<ene_api::ExclusiveSnapshot, ApiError> {
        self.client.release_resource(ResourceKind::Mic).await
    }

    pub async fn claim_speaker(&self) -> Result<ene_api::ExclusiveSnapshot, ApiError> {
        self.claim(ResourceKind::Speaker).await
    }

    pub async fn claim_notify(&self) -> Result<ene_api::ExclusiveSnapshot, ApiError> {
        self.claim(ResourceKind::Notify).await
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
}

/// Import or activate Alicia so a stage occupant exposes `avatar_path`.
pub async fn ensure_alicia(client: &ApiClient) -> Result<Option<OccupantView>, ApiError> {
    if let Some(occupant) = occupant_with_avatar(&client.stage().await?.occupants) {
        return Ok(Some(occupant));
    }
    let packages = client.list_characters().await?.items;
    if let Some(pkg) = find_alicia_package(&packages) {
        tracing::info!(id = %pkg.id, "activating installed Alicia package");
        let _ = client.activate_character(&pkg.id).await?;
    } else {
        match bundle::pack_bundled_alicia() {
            Ok(bytes) => {
                tracing::info!(bytes = bytes.len(), "importing bundled Alicia .enechar");
                let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
                let imported = client.import_character_archive_b64(&encoded).await?;
                if imported.soul_id.is_none() {
                    let _ = client.activate_character(&imported.id).await?;
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "bundled Alicia package unavailable");
                return Ok(occupant_with_avatar(&client.stage().await?.occupants));
            }
        }
    }
    Ok(occupant_with_avatar(&client.stage().await?.occupants))
}

fn find_alicia_package(packages: &[CharacterView]) -> Option<&CharacterView> {
    packages.iter().find(|pkg| {
        let id = pkg.id.to_ascii_lowercase();
        id.contains("alicia") || id == "char.alicia"
    })
}

#[must_use]
pub fn occupant_with_avatar(occupants: &[OccupantView]) -> Option<OccupantView> {
    occupants
        .iter()
        .find(|occupant| {
            occupant
                .avatar_path
                .as_ref()
                .is_some_and(|path| !path.is_empty())
        })
        .cloned()
}

#[must_use]
pub fn pick_avatar_occupant(occupants: &[OccupantView]) -> Option<OccupantView> {
    occupant_with_avatar(occupants).or_else(|| occupants.first().cloned())
}

async fn resolve_stage(
    client: &ApiClient,
) -> Result<(String, Vec<OccupantView>, Option<PathBuf>, Option<PathBuf>), ApiError> {
    if let Err(err) = ensure_alicia(client).await {
        tracing::warn!(error = %err, "Alicia package bootstrap failed");
    }
    let occupants = client.stage().await?.occupants;
    let occupant = pick_avatar_occupant(&occupants);
    let soul_id = if let Some(occupant) = occupant.as_ref() {
        occupant.soul_id.clone()
    } else {
        client
            .list_souls()
            .await?
            .items
            .first()
            .map(|soul| soul.id.clone())
            .ok_or_else(|| ApiError::Transport("no souls available".to_owned()))?
    };
    let avatar_path = occupant
        .as_ref()
        .and_then(|item| item.avatar_path.clone())
        .map(PathBuf::from);
    let motions_dir = occupant.as_ref().and_then(|item| {
        item.avatar_path.as_ref().map(|avatar| {
            PathBuf::from(avatar)
                .parent()
                .and_then(std::path::Path::parent)
                .map_or_else(
                    || motions_dir_for_package(item.package_id.as_deref().unwrap_or("")),
                    |body| body.join("motions"),
                )
        })
    });
    Ok((soul_id, occupants, avatar_path, motions_dir))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_avatar_occupant_prefers_avatar_path() {
        let occupants = vec![
            OccupantView {
                soul_id: "text".into(),
                body_id: None,
                package_id: None,
                avatar_path: None,
            },
            OccupantView {
                soul_id: "alicia".into(),
                body_id: Some("body".into()),
                package_id: Some("char.alicia@1.0.0".into()),
                avatar_path: Some(
                    "/data/characters/char.alicia@1.0.0/body/avatar/model.vrm".into(),
                ),
            },
        ];
        let picked = pick_avatar_occupant(&occupants).expect("occupant");
        assert_eq!(picked.soul_id, "alicia");
        assert!(occupant_with_avatar(&occupants[..1]).is_none());
    }

    #[test]
    fn find_alicia_matches_id() {
        let packages = vec![CharacterView {
            id: "char.alicia".into(),
            version: "1.0.0".into(),
            kind: "character".into(),
            path: "/tmp".into(),
            soul_id: None,
        }];
        assert_eq!(
            find_alicia_package(&packages).map(|p| p.id.as_str()),
            Some("char.alicia")
        );
    }
}
