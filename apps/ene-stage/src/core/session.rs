use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use ene_api::{
    AnswerJobRequest, ApiClient, ApiError, CharacterView, ClaimResourceRequest,
    CreateSessionRequest, GreetingView, HistoryResponse, MessageMode, MessageRequest, OccupantView,
    ResourceKind, SelectGreetingResponse, SendMessageResponse, SoulPatch, SoulView,
    SplitSessionResponse,
};
use parking_lot::Mutex;
use uuid::Uuid;

use crate::bundle::{self, BundleError, motions_dir_for_package};

/// Overlay draws at most this many VRM bodies (`body.render.max_concurrent` default).
pub const MAX_OVERLAY_BODIES: usize = 2;

/// Active stage session bound to one soul and surface history.
pub struct StageSession {
    client: Arc<ApiClient>,
    soul_id: String,
    session_id: String,
    turn_id: Arc<Mutex<Option<String>>>,
    history: Arc<Mutex<HistoryResponse>>,
    greetings: Vec<GreetingView>,
    occupants: Vec<OccupantView>,
    avatar_path: Option<PathBuf>,
    motions_dir: Option<PathBuf>,
}

/// Session state fetched before changing the stage's active dialogue lane.
pub(crate) struct PreparedSessionTarget {
    soul_id: String,
    session_id: String,
    history: HistoryResponse,
    greetings: Vec<GreetingView>,
    occupants: Vec<OccupantView>,
    avatar_path: Option<PathBuf>,
    motions_dir: Option<PathBuf>,
}

impl PreparedSessionTarget {
    #[cfg(test)]
    pub(crate) fn new_for_test(session_id: &str, history: HistoryResponse) -> Self {
        Self::new_for_test_with_soul("soul", session_id, history)
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_with_soul(
        soul_id: &str,
        session_id: &str,
        history: HistoryResponse,
    ) -> Self {
        Self {
            soul_id: soul_id.to_owned(),
            session_id: session_id.to_owned(),
            history,
            greetings: Vec::new(),
            occupants: Vec::new(),
            avatar_path: None,
            motions_dir: None,
        }
    }

    #[must_use]
    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }
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

    pub fn adopt_new_session(&mut self, split: &SplitSessionResponse) {
        self.session_id.clone_from(&split.session.id);
        *self.turn_id.lock() = None;
        *self.history.lock() = HistoryResponse {
            messages: Vec::new(),
            depth: "surface".to_owned(),
        };
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(
        client: Arc<ApiClient>,
        soul_id: &str,
        session_id: &str,
        history: HistoryResponse,
    ) -> Self {
        Self {
            client,
            soul_id: soul_id.to_owned(),
            session_id: session_id.to_owned(),
            turn_id: Arc::new(Mutex::new(None)),
            history: Arc::new(Mutex::new(history)),
            greetings: Vec::new(),
            occupants: Vec::new(),
            avatar_path: None,
            motions_dir: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn set_for_test(
        &mut self,
        client: Arc<ApiClient>,
        soul_id: &str,
        session_id: &str,
        history: HistoryResponse,
    ) {
        *self = Self::new_for_test(client, soul_id, session_id, history);
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

    #[must_use]
    pub fn greetings(&self) -> &[GreetingView] {
        &self.greetings
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
        let (soul_id, occupants, avatar_path, motions_dir) =
            resolve_stage(&client, MAX_OVERLAY_BODIES).await?;
        let session_id = resolve_session_id(&client, &soul_id).await?;
        let history = normalize_history(client.history(&session_id, "surface").await?);
        let greetings = client.list_greetings(&soul_id).await?.items;
        Ok(Self {
            client,
            soul_id,
            session_id,
            turn_id: Arc::new(Mutex::new(None)),
            history: Arc::new(Mutex::new(history)),
            greetings,
            occupants,
            avatar_path,
            motions_dir,
        })
    }

    pub async fn retarget_soul(&mut self, soul_id: &str) -> Result<(), ApiError> {
        let target = prepare_soul_target(&self.client, soul_id).await?;
        self.commit_retarget(target);
        Ok(())
    }

    pub(crate) fn commit_retarget(&mut self, target: PreparedSessionTarget) {
        target.soul_id.clone_into(&mut self.soul_id);
        self.session_id = target.session_id;
        self.turn_id = Arc::new(Mutex::new(None));
        self.history = Arc::new(Mutex::new(target.history));
        self.greetings = target.greetings;
        self.occupants = target.occupants;
        self.avatar_path = target.avatar_path;
        self.motions_dir = target.motions_dir;
    }

    #[must_use]
    pub fn avatar_loads(&self) -> Vec<crate::overlay::AvatarLoad> {
        avatar_slots(&self.occupants)
            .into_iter()
            .filter_map(|occupant| {
                let path = PathBuf::from(occupant.avatar_path.as_ref()?);
                let motions_dir = motions_dir_for_occupant(&occupant);
                Some(crate::overlay::AvatarLoad {
                    soul_id: occupant.soul_id,
                    path,
                    motions_dir,
                })
            })
            .collect()
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
        let history = normalize_history(self.client.history(&self.session_id, "surface").await?);
        *self.history.lock() = history.clone();
        Ok(history)
    }

    pub async fn select_greeting(&self, index: u32) -> Result<SelectGreetingResponse, ApiError> {
        let response = self.client.select_greeting(&self.session_id, index).await?;
        if response.committed {
            let _ = self.refresh_history().await?;
        }
        Ok(response)
    }

    pub async fn respond_approval(
        &self,
        id: &str,
        decision: &str,
    ) -> Result<serde_json::Value, ApiError> {
        self.client.respond_approval(id, decision).await
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
        let history = normalize_history(self.client.history(&self.session_id, "surface").await?);
        *self.history.lock() = history.clone();
        Ok(history)
    }

    pub async fn select_greeting(&self, index: u32) -> Result<HistoryResponse, ApiError> {
        self.client.select_greeting(&self.session_id, index).await?;
        self.refresh_history().await
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

    pub async fn answer_job(&self, job_id: &str, text: &str) -> Result<(), ApiError> {
        self.client
            .answer_job(
                job_id,
                &AnswerJobRequest {
                    text: text.to_owned(),
                    answers: Vec::new(),
                },
            )
            .await
            .map(|_| ())
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
pub async fn ensure_alicia(
    client: &ApiClient,
    bundle: impl Fn(&str, &str) -> Result<Vec<u8>, BundleError>,
) -> Result<Option<OccupantView>, ApiError> {
    let occupants = client.stage().await?.occupants;
    if let Some(occupant) = occupant_with_avatar(&occupants) {
        return Ok(Some(occupant));
    }
    import_named_companion(client, "char.alicia", "Alicia", &bundle).await?;
    Ok(occupant_with_avatar(&client.stage().await?.occupants))
}

/// Ensure up to `want` occupants expose a VRM `avatar_path`.
pub async fn ensure_avatar_occupants(
    client: &ApiClient,
    want: usize,
) -> Result<Vec<OccupantView>, ApiError> {
    ensure_avatar_occupants_with(client, want, bundle::pack_bundled_named).await
}

/// Test seam over the bundled-character pack step.
async fn ensure_avatar_occupants_with(
    client: &ApiClient,
    want: usize,
    bundle: impl Fn(&str, &str) -> Result<Vec<u8>, BundleError>,
) -> Result<Vec<OccupantView>, ApiError> {
    let _ = ensure_alicia(client, &bundle).await?;
    let occupants = client.stage().await?.occupants;
    if avatar_slots(&occupants).len() >= want {
        return Ok(occupants);
    }
    import_named_companion(client, "char.alicia-b", "Alicia B", &bundle).await?;
    Ok(client.stage().await?.occupants)
}

async fn import_named_companion(
    client: &ApiClient,
    id: &str,
    display_name: &str,
    bundle: &impl Fn(&str, &str) -> Result<Vec<u8>, BundleError>,
) -> Result<(), ApiError> {
    let packages = client.list_characters().await?.items;
    let existing = packages.iter().find(|pkg| pkg.id == id).or_else(|| {
        if id == "char.alicia" {
            find_alicia_package(&packages)
        } else {
            None
        }
    });
    if let Some(pkg) = existing {
        tracing::info!(id = %pkg.id, "activating installed VRM package");
        let _ = client.activate_character(&pkg.id).await?;
        return Ok(());
    }
    match bundle(id, display_name) {
        Ok(bytes) => {
            tracing::info!(id, bytes = bytes.len(), "importing bundled VRM .enechar");
            let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
            let imported = client.import_character_archive_b64(&encoded).await?;
            if imported.soul_id.is_none() {
                let _ = client.activate_character(&imported.id).await?;
            }
            Ok(())
        }
        Err(err) => Err(ApiError::Transport(err.to_string())),
    }
}

fn find_alicia_package(packages: &[CharacterView]) -> Option<&CharacterView> {
    packages.iter().find(|pkg| {
        let id = pkg.id.to_ascii_lowercase();
        id == "char.alicia" || id == "alicia"
    })
}

#[cfg(test)]
mod bundle_seam_tests {
    #![cfg_attr(test, expect(clippy::unwrap_used, reason = "tests"))]
    use super::*;

    fn recording_client() -> ApiClient {
        ApiClient::new("http://127.0.0.1:9", "token", "stage")
    }

    fn failing_bundle(_id: &str, _name: &str) -> Result<Vec<u8>, BundleError> {
        Err(BundleError::Missing("/no/AliciaSolid.vrm".to_owned()))
    }

    #[tokio::test]
    async fn missing_bundle_is_reported_not_swallowed() {
        let client = recording_client();
        let err = import_named_companion(&client, "char.alicia", "Alicia", &failing_bundle)
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::Transport(_)));
    }

    #[tokio::test]
    async fn ensure_avatar_occupants_surfaces_missing_bundle() {
        let client = recording_client();
        let err = ensure_avatar_occupants_with(&client, 1, failing_bundle)
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::Transport(_)));
    }
}

#[must_use]
pub fn avatar_slots(occupants: &[OccupantView]) -> Vec<OccupantView> {
    occupants
        .iter()
        .filter(|occupant| occupant_has_avatar(occupant))
        .take(MAX_OVERLAY_BODIES)
        .cloned()
        .collect()
}

#[must_use]
pub fn motions_dir_for_occupant(occupant: &OccupantView) -> Option<PathBuf> {
    occupant.avatar_path.as_ref().map(|avatar| {
        PathBuf::from(avatar)
            .parent()
            .and_then(std::path::Path::parent)
            .map_or_else(
                || motions_dir_for_package(occupant.package_id.as_deref().unwrap_or("")),
                |body| body.join("motions"),
            )
    })
}

#[must_use]
pub fn occupant_with_avatar(occupants: &[OccupantView]) -> Option<OccupantView> {
    occupants
        .iter()
        .find(|occupant| occupant_has_avatar(occupant))
        .cloned()
}

#[must_use]
pub fn occupant_has_avatar(occupant: &OccupantView) -> bool {
    occupant
        .avatar_path
        .as_ref()
        .is_some_and(|path| !path.is_empty())
}

#[must_use]
pub fn occupant_label(occupant: &OccupantView) -> String {
    occupant
        .package_id
        .as_deref()
        .and_then(|id| id.split('@').next())
        .map(|id| id.trim_start_matches("char.").to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| occupant.soul_id.clone())
}

#[must_use]
pub fn next_avatar_occupant(
    occupants: &[OccupantView],
    current_soul: &str,
    delta: i32,
) -> Option<OccupantView> {
    let avatars: Vec<OccupantView> = occupants
        .iter()
        .filter(|occupant| occupant_has_avatar(occupant))
        .cloned()
        .collect();
    if avatars.is_empty() {
        return None;
    }
    let current = avatars
        .iter()
        .position(|item| item.soul_id == current_soul)
        .unwrap_or(0);
    let len = i32::try_from(avatars.len()).unwrap_or(1);
    let next = (i32::try_from(current).unwrap_or(0) + delta).rem_euclid(len);
    avatars.get(usize::try_from(next).unwrap_or(0)).cloned()
}

#[must_use]
pub fn pick_avatar_occupant(occupants: &[OccupantView]) -> Option<OccupantView> {
    occupant_with_avatar(occupants).or_else(|| occupants.first().cloned())
}

async fn resolve_stage(
    client: &ApiClient,
    want_avatars: usize,
) -> Result<(String, Vec<OccupantView>, Option<PathBuf>, Option<PathBuf>), ApiError> {
    if let Err(err) = ensure_avatar_occupants(client, want_avatars).await {
        tracing::warn!(error = %err, "VRM package bootstrap failed");
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
    let motions_dir = occupant.as_ref().and_then(motions_dir_for_occupant);
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

pub(crate) async fn prepare_soul_target(
    client: &ApiClient,
    soul_id: &str,
) -> Result<PreparedSessionTarget, ApiError> {
    let session_id = resolve_session_id(client, soul_id).await?;
    let history = normalize_history(client.history(&session_id, "surface").await?);
    let greetings = client.list_greetings(soul_id).await?.items;
    let occupants = client.stage().await?.occupants;
    let occupant = occupants.iter().find(|item| item.soul_id == soul_id);
    let avatar_path = occupant.and_then(|item| item.avatar_path.as_ref().map(PathBuf::from));
    let motions_dir = occupant.and_then(motions_dir_for_occupant);
    Ok(PreparedSessionTarget {
        soul_id: soul_id.to_owned(),
        session_id,
        history,
        greetings,
        occupants,
        avatar_path,
        motions_dir,
    })
}

/// Keep the chat transcript in chronological order regardless of API ordering.
#[must_use]
pub fn normalize_history(mut history: HistoryResponse) -> HistoryResponse {
    history.messages.sort_by_key(|message| message.seq);
    history
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_api::{MessageResponse, SessionView};

    fn session_view(id: &str) -> SessionView {
        SessionView {
            id: id.to_owned(),
            soul_id: "soul".to_owned(),
            kind: "conversation".to_owned(),
            title: None,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            archived: false,
            next_seq: 0,
            ended_at: None,
            end_reason: None,
            delegation_id: None,
        }
    }

    #[test]
    fn history_is_normalized_oldest_to_newest() {
        let message = |seq: u64, text: &'static str| MessageResponse {
            seq,
            role: "user".to_owned(),
            text: text.to_owned(),
        };
        let normalized = normalize_history(HistoryResponse {
            messages: vec![
                message(3, "third"),
                message(1, "first"),
                message(2, "second"),
            ],
            depth: "surface".to_owned(),
        });

        let seqs: Vec<_> = normalized.messages.iter().map(|m| m.seq).collect();
        assert_eq!(seqs, [1, 2, 3]);
        assert_eq!(normalized.depth, "surface");
    }

    #[test]
    fn adopt_new_session_switches_before_history_refresh() {
        let client = Arc::new(ApiClient::new("http://127.0.0.1:9", "token", "stage"));
        let old_history = HistoryResponse {
            messages: vec![MessageResponse {
                seq: 1,
                role: "assistant".to_owned(),
                text: "old".to_owned(),
            }],
            depth: "surface".to_owned(),
        };
        let mut session = StageSession::new_for_test(
            Arc::clone(&client),
            "soul",
            "old-session",
            old_history.clone(),
        );

        let split = SplitSessionResponse {
            previous: session_view("old-session"),
            session: session_view("new-session"),
        };
        session.adopt_new_session(&split);
        assert_eq!(session.session_id(), "new-session");
        assert_eq!(session.turn_id(), None);
        assert!(session.history().messages.is_empty());
        assert_eq!(session.history().depth, "surface");
        assert_eq!(old_history.messages[0].text, "old");
    }

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
        let cycled = next_avatar_occupant(&occupants, "text", 1).expect("avatar");
        assert_eq!(cycled.soul_id, "alicia");
        assert_eq!(occupant_label(&cycled), "alicia");
        assert!(next_avatar_occupant(&occupants[..1], "text", 1).is_none());
        let with_other = vec![
            occupants[0].clone(),
            occupants[1].clone(),
            OccupantView {
                soul_id: "other".into(),
                body_id: Some("body2".into()),
                package_id: Some("char.other@1.0.0".into()),
                avatar_path: Some("/data/other.vrm".into()),
            },
        ];
        assert_eq!(
            next_avatar_occupant(&with_other, "alicia", 1)
                .expect("next")
                .soul_id,
            "other"
        );
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
        let mixed = vec![
            CharacterView {
                id: "char.alicia-b".into(),
                version: "1.0.0".into(),
                kind: "character".into(),
                path: "/tmp".into(),
                soul_id: None,
            },
            packages[0].clone(),
        ];
        assert_eq!(
            find_alicia_package(&mixed).map(|p| p.id.as_str()),
            Some("char.alicia")
        );
    }

    #[test]
    fn avatar_slots_caps_at_two_and_skips_text() {
        let occupants = vec![
            OccupantView {
                soul_id: "text".into(),
                body_id: None,
                package_id: None,
                avatar_path: None,
            },
            OccupantView {
                soul_id: "a".into(),
                body_id: Some("b1".into()),
                package_id: Some("char.alicia@1.0.0".into()),
                avatar_path: Some("/data/a.vrm".into()),
            },
            OccupantView {
                soul_id: "b".into(),
                body_id: Some("b2".into()),
                package_id: Some("char.alicia-b@1.0.0".into()),
                avatar_path: Some("/data/b.vrm".into()),
            },
            OccupantView {
                soul_id: "c".into(),
                body_id: Some("b3".into()),
                package_id: Some("char.other@1.0.0".into()),
                avatar_path: Some("/data/c.vrm".into()),
            },
        ];
        let slots = avatar_slots(&occupants);
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0].soul_id, "a");
        assert_eq!(slots[1].soul_id, "b");
    }
}
