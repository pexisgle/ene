use std::sync::{Arc, Weak};

use async_trait::async_trait;
use ene_companion::{ArbitrateOutcome, ClassifyModel, ClassifyTask, CompanionError};
use ene_kernel::{TaskBinding, TurnFinalizer};
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

    fn binding_for(core: &CoreDaemon, task: ClassifyTask) -> TaskBinding {
        let guard = core.ai();
        let ai = guard.lock();
        let specific = match task {
            ClassifyTask::ProactiveDecision | ClassifyTask::ScreenSummary => &ai.tasks.proactive,
            _ => &ai.tasks.classifier,
        };
        if !specific.is_unconfigured() {
            return specific.clone();
        }
        ai.tasks.chat.clone()
    }

    fn row_id_for(core: &CoreDaemon, task: ClassifyTask) -> String {
        let guard = core.ai();
        let ai = guard.lock();
        let (name, specific) = match task {
            ClassifyTask::ProactiveDecision | ClassifyTask::ScreenSummary => {
                ("proactive", &ai.tasks.proactive)
            }
            _ => ("classifier", &ai.tasks.classifier),
        };
        if specific.is_unconfigured() {
            crate::plugin_profile::task_row_id("chat")
        } else {
            crate::plugin_profile::task_row_id(name)
        }
    }

    fn secret_for(core: &CoreDaemon, task: ClassifyTask) -> String {
        match task {
            ClassifyTask::ProactiveDecision | ClassifyTask::ScreenSummary => {
                core.secret_for("proactive")
            }
            _ => core.secret_for("classifier"),
        }
    }
}

#[async_trait]
impl ClassifyModel for SeamedClassify {
    async fn complete_json(
        &self,
        task: ClassifyTask,
        input: &str,
    ) -> Result<String, CompanionError> {
        let core = self.core()?;
        let binding = Self::binding_for(&core, task);
        if binding.is_unconfigured() {
            return Err(CompanionError::Classify(
                "classifier is not configured".to_owned(),
            ));
        }
        let request = LlmGenerateRequest {
            messages: vec![
                LlmMessage {
                    role: LlmRole::System,
                    text: format!(
                        "You are a JSON classifier for task `{}`. Reply with a JSON object only.",
                        task.as_str()
                    ),
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
            model: binding.model,
            max_tokens: binding.max_tokens.or(Some(512)),
            base_url: binding.base_url,
            auth: ProviderAuth {
                api_key: Self::secret_for(&core, task),
            },
        };
        let generation = core
            .supervisor()
            .generate_llm(&Self::row_id_for(&core, task), request)
            .await
            .map_err(|err| CompanionError::Classify(err.to_string()))?;
        if generation.finish_reason == "error" {
            return Err(CompanionError::Classify(generation.model_id));
        }
        let text = generation.text.trim();
        if text.is_empty() {
            return Err(CompanionError::Classify(
                "empty classifier reply".to_owned(),
            ));
        }
        Ok(text.to_owned())
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
        let binding = Self::binding_for(&core, task);
        if binding.is_unconfigured() {
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
            model: binding.model,
            max_tokens: binding.max_tokens.or(Some(256)),
            base_url: binding.base_url,
            auth: ProviderAuth {
                api_key: Self::secret_for(&core, task),
            },
        };
        let generation = core
            .supervisor()
            .generate_llm(&Self::row_id_for(&core, task), request)
            .await
            .map_err(|err| CompanionError::Classify(err.to_string()))?;
        if generation.finish_reason == "error" {
            return Err(CompanionError::Classify(generation.model_id));
        }
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
    let (binding, row_id) = {
        let guard = core.ai();
        let ai = guard.lock();
        if ai.tasks.embedding.is_unconfigured() {
            (
                ai.tasks.chat.clone(),
                crate::plugin_profile::task_row_id("chat"),
            )
        } else {
            (
                ai.tasks.embedding.clone(),
                crate::plugin_profile::task_row_id("embedding"),
            )
        }
    };
    if binding.is_unconfigured() {
        return Ok(());
    }
    let result = core
        .supervisor()
        .embed(
            &row_id,
            ene_plugin_ipc::EmbedRequest {
                texts: vec![text.to_owned()],
                model: binding.model,
                base_url: binding.base_url,
                auth: ProviderAuth {
                    api_key: core.secret_for("embedding"),
                },
            },
        )
        .await
        .map_err(|err| err.to_string())?;
    let Some(vector) = result.vectors.into_iter().next() else {
        return Ok(());
    };
    core.companions()
        .set_embedding(id, &vector)
        .map_err(|err| err.to_string())
}
