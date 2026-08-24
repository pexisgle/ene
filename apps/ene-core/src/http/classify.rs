use std::sync::{Arc, Weak};

use async_trait::async_trait;
use ene_companion::{ArbitrateOutcome, ClassifyModel, ClassifyTask, CompanionError};
use ene_kernel::{AiTaskKind, TurnFinalizer};
use ene_plugin_ipc::{LlmGenerateRequest, LlmImage, LlmMessage, LlmRole, ProviderAuth};
use ene_session::SoulId;

use crate::CoreDaemon;

/// Classifier that maps `ai.tasks.classifier` / `proactive` (falling back to chat).
pub struct SeamedClassify {
    core: Weak<CoreDaemon>,
}

impl SeamedClassify {
    #[must_use]
    pub fn new(core: &Arc<CoreDaemon>) -> Self {
        Self {
            core: Arc::downgrade(core),
        }
    }

    fn core(&self) -> Result<Arc<CoreDaemon>, CompanionError> {
        self.core
            .upgrade()
            .ok_or_else(|| CompanionError::Classify("core stopped".to_owned()))
    }

    fn resolved(core: &CoreDaemon, task: ClassifyTask) -> crate::seam_client::ResolvedTask {
        classify_kind(core, task)
    }
}

/// Map a classify request onto its `ai.tasks.*` lane and resolved provider row.
fn classify_kind(core: &CoreDaemon, task: ClassifyTask) -> crate::seam_client::ResolvedTask {
    let kind = match task {
        ClassifyTask::ProactiveDecision | ClassifyTask::ScreenSummary => AiTaskKind::Proactive,
        _ => AiTaskKind::Classifier,
    };
    // Chat is the fallback lane for every classifier task, so the sentinel
    // below only surfaces when no chat provider is configured at all; the
    // callers then reject with their own "not configured" error.
    crate::seam_client::resolve_task(core, kind).unwrap_or_else(|| {
        crate::seam_client::ResolvedTask {
            binding: ene_kernel::TaskBinding::default(),
            row_id: String::new(),
            api_key: String::new(),
        }
    })
}

#[async_trait]
impl ClassifyModel for SeamedClassify {
    async fn complete_json(
        &self,
        task: ClassifyTask,
        input: &str,
    ) -> Result<String, CompanionError> {
        let core = self.core()?;
        let resolved = Self::resolved(&core, task);
        if resolved.binding.is_unconfigured() {
            return Err(CompanionError::Classify(
                "classifier is not configured".to_owned(),
            ));
        }
        let request = LlmGenerateRequest {
            messages: vec![
                LlmMessage {
                    role: LlmRole::System,
                    text: system_prompt(task).to_owned(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    tool_name: None,
                    images: Vec::new(),
                },
                LlmMessage {
                    role: LlmRole::User,
                    text: input.to_owned(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    tool_name: None,
                    images: Vec::new(),
                },
            ],
            tools: Vec::new(),
            model: resolved.binding.model.clone(),
            max_tokens: resolved.binding.max_tokens.or(Some(512)),
            base_url: resolved.binding.base_url.clone(),
            auth: ProviderAuth {
                api_key: resolved.api_key.clone(),
            },
        };
        let generation = crate::seam_client::generate_llm(&core, &resolved, request)
            .await
            .map_err(CompanionError::Classify)?;
        let text = generation.text.trim();
        if text.is_empty() {
            return Err(CompanionError::Classify(
                "empty classifier reply".to_owned(),
            ));
        }
        Ok(text.to_owned())
    }
}

fn system_prompt(task: ClassifyTask) -> &'static str {
    match task {
        ClassifyTask::MemoryExtract => {
            "You extract memories. Reply with a JSON object only: \
{\"candidates\":[{\"kind\":\"episodic|semantic|user_profile|preference|commitment\",\
\"title\":\"...\",\"content\":\"...\",\"scope\":\"private|shared\",\
\"confidence\":0.0,\"salience\":0.0,\"commitment_due\":null}]}. \
commitment_due is ISO-8601 or YYYY-MM-DD, else null — never a relative phrase. \
Use shared only when the user clearly wants every companion to know. \
Empty candidates is valid. No markdown."
        }
        ClassifyTask::Affect => {
            "You estimate affect from the user utterance. Reply with a JSON object only: \
{\"valence\":0.0,\"arousal\":0.0,\"irritation\":0.0,\"affinity\":0.0,\"confidence\":0.0}. \
valence/arousal/affinity are -1..1, irritation and confidence are 0..1. No markdown."
        }
        ClassifyTask::ProactiveDecision => {
            "You decide whether the companion should speak unprompted. Reply with a JSON object only: \
{\"should_speak\":false,\"confidence\":0.0,\"reason\":\"...\",\"topic_hint\":\"...\",\
\"urgency\":\"low|normal|high\",\"screen_digest\":\"\"}. Fail closed: should_speak false unless sure. No markdown."
        }
        ClassifyTask::ScreenSummary => {
            "You are a JSON classifier for task `screen_summary`. Reply with a JSON object only."
        }
    }
}

impl SeamedClassify {
    pub async fn summarize_screen(
        &self,
        png: &[u8],
        window_label: &str,
    ) -> Result<String, CompanionError> {
        let core = self.core()?;
        let task = ClassifyTask::ScreenSummary;
        let resolved = Self::resolved(&core, task);
        if resolved.binding.is_unconfigured() {
            return Err(CompanionError::Classify(
                "classifier is not configured".to_owned(),
            ));
        }
        let request = LlmGenerateRequest {
            messages: vec![
                LlmMessage::new(
                    LlmRole::System,
                    "Summarize the attached screenshot in one short paragraph. Name the focused app if obvious. Do not quote private text, URLs, emails, or numbers. Reply with plain text only.",
                ),
                LlmMessage {
                    role: LlmRole::User,
                    text: if window_label.is_empty() {
                        "Summarize this screen.".to_owned()
                    } else {
                        format!("Active window label: {window_label}")
                    },
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    tool_name: None,
                    images: vec![LlmImage {
                        mime: "image/png".to_owned(),
                        base64: base64::Engine::encode(
                            &base64::engine::general_purpose::STANDARD,
                            png,
                        ),
                    }],
                },
            ],
            tools: Vec::new(),
            model: resolved.binding.model.clone(),
            max_tokens: resolved.binding.max_tokens.or(Some(256)),
            base_url: resolved.binding.base_url.clone(),
            auth: ProviderAuth {
                api_key: resolved.api_key.clone(),
            },
        };
        let generation = crate::seam_client::generate_llm(&core, &resolved, request)
            .await
            .map_err(CompanionError::Classify)?;
        let text = generation.text.trim();
        if text.is_empty() {
            return Err(CompanionError::Classify("empty screen summary".to_owned()));
        }
        Ok(text.to_owned())
    }
}

/// Runs companion memory extract after a surface turn and stores embeddings.
pub struct MemoryFinalizer {
    core: Weak<CoreDaemon>,
    classify: Arc<SeamedClassify>,
}

impl MemoryFinalizer {
    #[must_use]
    pub fn new(core: &Arc<CoreDaemon>, classify: Arc<SeamedClassify>) -> Self {
        Self {
            core: Arc::downgrade(core),
            classify,
        }
    }
}

#[async_trait]
impl TurnFinalizer for MemoryFinalizer {
    async fn finalize_turn(&self, soul: SoulId, user_text: &str, assistant_text: &str) {
        if user_text.is_empty() && assistant_text.is_empty() {
            return;
        }
        let Some(core) = self.core.upgrade() else {
            return;
        };
        core.expire_due_commitments();
        let outcomes = match core
            .companion()
            .after_turn(
                soul,
                user_text,
                assistant_text,
                Some(self.classify.as_ref()),
            )
            .await
        {
            Ok(outcomes) => outcomes,
            Err(err) => {
                tracing::debug!(error = %err, "memory extract skipped");
                return;
            }
        };
        for outcome in outcomes {
            let record = match outcome {
                ArbitrateOutcome::Inserted(record) | ArbitrateOutcome::Updated(record) => record,
                ArbitrateOutcome::Queued(_) | ArbitrateOutcome::Rejected(_) => continue,
            };
            if let Err(err) = embed_memory(
                &core,
                record.id,
                &format!("{}\n{}", record.title, record.content),
            )
            .await
            {
                tracing::debug!(error = %err, "memory embedding skipped");
            }
        }
    }
}

async fn embed_memory(
    core: &CoreDaemon,
    id: ene_companion::MemoryId,
    text: &str,
) -> Result<(), String> {
    let vectors = match crate::seam_client::embed_texts(core, vec![text.to_owned()]).await {
        Ok(vectors) => vectors,
        Err(err) => return Err(err),
    };
    let Some(vector) = vectors.into_iter().next() else {
        return Ok(());
    };
    core.companions()
        .set_embedding(id, &vector)
        .map_err(|err| err.to_string())
}
