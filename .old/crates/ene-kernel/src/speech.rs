use async_trait::async_trait;
use ene_session::{SessionId, SoulId};

/// Plays assistant speech after the lane commits it. Core injects a provider-backed
/// implementation; tests leave this unset.
#[async_trait]
pub trait SpeechPresenter: Send + Sync {
    async fn present_speech(&self, text: &str);
}

/// Runs after surface speech is committed (memory extract, etc.). Must not fail the turn.
#[async_trait]
pub trait TurnFinalizer: Send + Sync {
    async fn finalize_turn(&self, soul: SoulId, user_text: &str, assistant_text: &str);
}

/// Logged System Context lines. The lane merges these into `ContextRegistry`
/// (canonical keys, draw order) before the model runs.
#[async_trait]
pub trait TurnPrefetch: Send + Sync {
    async fn lines(
        &self,
        soul: SoulId,
        session: SessionId,
        user_text: &str,
    ) -> Vec<(String, String)>;
}
