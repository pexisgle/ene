//! End-to-end persistent scheduler tests through `EneHandle` + the real
//! actor loop and scheduler timer task.

#![expect(
    clippy::expect_used,
    reason = "integration tests use expect for assertions"
)]

use async_trait::async_trait;
use ene_ai::{
    EmbeddingError, EmbeddingKind, EmbeddingProvider, EmbeddingProviderFactory, LlmCompletion,
    LlmMessage, LlmProvider, LlmProviderError, LlmProviderFactory, LlmResponseChunk,
};
use ene_config::{CharacterCardV3, EneConfig};
use ene_runtime::{
    EneEvent, EneEventReceiver, EneHandle, NewSchedule, PermissionDecision, ScheduleAction,
    ScheduleConfirmation, ScheduleKind, ScheduleRunStatus, TerminalReason, TurnOrigin,
};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_stream::Stream;

fn test_card() -> CharacterCardV3 {
    let mut card = CharacterCardV3::default();
    card.data.name = "SchedulerTest".into();
    card.data.system_prompt = "Be brief.".into();
    card
}

/// Chat provider that never completes, keeping the single-flight gate held
/// so tests can exercise the "scheduled runs never interrupt a conversation"
/// policy.
struct HangingLlmProvider;

#[async_trait]
impl LlmProvider for HangingLlmProvider {
    fn name(&self) -> &'static str {
        "scheduler-test-hanging"
    }

    async fn create_chat_stream(
        &self,
        _messages: &[LlmMessage],
        _tools: &[ene_plugin_proto::ToolSpec],
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
        LlmProviderError,
    > {
        std::future::pending().await
    }

    async fn chat_completion(
        &self,
        _messages: &[LlmMessage],
        _json_schema: Option<serde_json::Value>,
    ) -> Result<LlmCompletion, LlmProviderError> {
        std::future::pending().await
    }
}

struct HangingLlmFactory;

impl LlmProviderFactory for HangingLlmFactory {
    fn provider_name(&self) -> &'static str {
        "scheduler-test-hanging"
    }

    fn create_provider(
        &self,
        _config: &EneConfig,
        _task: &ene_ai::config::TaskRef,
    ) -> Result<Box<dyn LlmProvider>, LlmProviderError> {
        Ok(Box::new(HangingLlmProvider))
    }
}

/// Embedding provider that never returns vectors, so a user turn stays in
/// flight even before the chat request.
struct HangingEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for HangingEmbeddingProvider {
    async fn embed_batch(
        &self,
        _items: &[(&str, EmbeddingKind)],
    ) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        std::future::pending().await
    }

    fn dimensions(&self) -> usize {
        8
    }

    fn model_name(&self) -> &'static str {
        "scheduler-test-hanging"
    }
}

struct HangingEmbeddingFactory;

impl EmbeddingProviderFactory for HangingEmbeddingFactory {
    fn provider_kind(&self) -> &str {
        ene_ai::config::OPENAI_PROVIDER_KIND
    }

    fn create_embedding_provider(
        &self,
        _config: &EneConfig,
    ) -> Result<Arc<dyn EmbeddingProvider>, EmbeddingError> {
        Ok(Arc::new(HangingEmbeddingProvider))
    }
}

/// Stub host standing in for the plugin registry: serves the hanging LLM
/// factory under its custom kind and the hanging embedding factory under
/// `openai`.
struct StubProviderHost {
    llm: std::collections::HashMap<String, Arc<dyn LlmProviderFactory>>,
    embedding: std::collections::HashMap<String, Arc<dyn EmbeddingProviderFactory>>,
}

#[async_trait]
impl ene_ai::ProviderHost for StubProviderHost {
    async fn create_llm_provider(
        &self,
        kind: &str,
        config: &EneConfig,
        task: &ene_ai::config::TaskRef,
    ) -> Result<Box<dyn LlmProvider>, LlmProviderError> {
        self.llm
            .get(kind)
            .ok_or_else(|| {
                LlmProviderError::Provider(format!(
                    "No LlmProviderFactory registered for provider kind: '{kind}'"
                ))
            })?
            .create_provider(config, task)
    }

    async fn create_embedding_provider(
        &self,
        kind: &str,
        config: &EneConfig,
    ) -> Result<Arc<dyn EmbeddingProvider>, EmbeddingError> {
        self.embedding
            .get(kind)
            .ok_or_else(|| {
                EmbeddingError::Init(format!(
                    "No embedding provider factory registered for provider kind: '{kind}'"
                ))
            })?
            .create_embedding_provider(config)
    }

    async fn create_tts_provider(
        &self,
        _kind: &str,
        _config: &EneConfig,
    ) -> Result<Box<dyn ene_ai::TtsProvider>, ene_ai::AudioProviderError> {
        Err(ene_ai::AudioProviderError::Provider(
            "stub host serves no TTS providers".to_string(),
        ))
    }

    async fn create_stt_provider(
        &self,
        _kind: &str,
        _config: &EneConfig,
    ) -> Result<Box<dyn ene_ai::SttProvider>, ene_ai::AudioProviderError> {
        Err(ene_ai::AudioProviderError::Provider(
            "stub host serves no STT providers".to_string(),
        ))
    }

    async fn create_vad_engine(
        &self,
        _kind: &str,
        _config: &EneConfig,
    ) -> Result<Box<dyn ene_ai::VadEngine>, ene_ai::AudioProviderError> {
        Err(ene_ai::AudioProviderError::Provider(
            "stub host serves no VAD engines".to_string(),
        ))
    }
}

/// Memory-enabled config whose chat and embedding providers hang forever,
/// routed to stub-host factories so no plugin host or network is needed.
fn test_config_memory_on(db_path: Option<&str>) -> (EneConfig, Arc<dyn ene_ai::ProviderHost>) {
    let mut config = EneConfig::default();
    let store = ene_store::StoreConfig {
        enabled: true,
        in_memory: db_path.is_none(),
        db_path: db_path.unwrap_or_default().to_string(),
        ..Default::default()
    };
    config.set_section(&store).expect("store config merges");
    let plugins = ene_plugin_host::PluginConfig {
        enabled: false,
        ..Default::default()
    };
    drop(config.set_section(&plugins));
    // Keep `system.search_tools` on the fast registry-list path: the Tool
    // RAG pipeline would try to embed through the hanging test embedder and
    // time out on every call.
    let rag = ene_rag::ToolRagConfig {
        enabled: false,
        ..Default::default()
    };
    drop(config.set_section(&rag));
    let mut ai = ene_ai::AiConfig::default();
    if let Some(provider) = ai.providers.get_mut("default") {
        provider.kind = "scheduler-test-hanging".to_string();
        provider.base_url = "http://127.0.0.1:1".to_string();
    }
    ai.providers.insert(
        "embed-stub".to_string(),
        ene_ai::config::AiProviderDef {
            kind: ene_ai::config::OPENAI_PROVIDER_KIND.to_string(),
            base_url: "http://127.0.0.1:1".to_string(),
            api_key: ene_ai::config::ApiKeyConfig {
                source: "inline".to_string(),
                inline: "sk-test".to_string(),
                env: String::new(),
            },
            ..ene_ai::config::AiProviderDef::default()
        },
    );
    ai.tasks.embedding = ene_ai::config::TaskRef {
        provider: "embed-stub".to_string(),
        model: Some("test-embedding-model".to_string()),
        dimensions: Some(8),
        ..ene_ai::config::TaskRef::default()
    };
    drop(config.set_section(&ai));
    let host: Arc<dyn ene_ai::ProviderHost> = Arc::new(StubProviderHost {
        llm: std::collections::HashMap::from([(
            "scheduler-test-hanging".to_string(),
            Arc::new(HangingLlmFactory) as Arc<dyn LlmProviderFactory>,
        )]),
        embedding: std::collections::HashMap::from([(
            ene_ai::config::OPENAI_PROVIDER_KIND.to_string(),
            Arc::new(HangingEmbeddingFactory) as Arc<dyn EmbeddingProviderFactory>,
        )]),
    });
    (config, host)
}

/// Open a memory-enabled runtime against the hanging stub host.
async fn open_memory_on(db_path: Option<&str>) -> EneHandle {
    let (config, host) = test_config_memory_on(db_path);
    EneHandle::open_with_provider_host(config, test_card(), host)
        .await
        .expect("open initializes handle")
}

fn one_shot_tool_schedule(confirm: bool) -> NewSchedule {
    NewSchedule {
        name: format!(
            "test-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ),
        kind: ScheduleKind::OneShot,
        timezone: "UTC".to_string(),
        cron_expr: None,
        interval_secs: None,
        start_at: Some(chrono::Utc::now() + chrono::Duration::milliseconds(150)),
        action: ScheduleAction::Tool {
            name: "system.search_tools".to_string(),
            arguments: serde_json::json!({ "query": "scheduler" }),
        },
        confirmation: if confirm {
            ScheduleConfirmation::Confirm
        } else {
            ScheduleConfirmation::None
        },
        max_retries: 0,
        retry_delay_secs: 60,
    }
}

async fn wait_for_run_status(
    handle: &EneHandle,
    schedule_id: i64,
    wanted: ScheduleRunStatus,
) -> Vec<ene_runtime::ScheduleRun> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let runs = handle
            .list_schedule_runs(schedule_id, 20)
            .await
            .expect("list runs");
        if let Some(run) = runs.first()
            && run.status == wanted
        {
            return runs;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for run status {wanted:?}; got {runs:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A scheduled tool action fires, streams turn-scoped events, records a
/// success run, and completes the one-shot schedule.
#[tokio::test]
async fn scheduled_tool_action_runs_and_records_success() {
    let handle = open_memory_on(None).await;
    let mut rx = handle.subscribe();
    let schedule = handle
        .add_schedule(one_shot_tool_schedule(false))
        .await
        .expect("add schedule");

    let mut saw_started = false;
    let deadline = Instant::now() + Duration::from_secs(20);
    let terminal = loop {
        let event = tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            rx.recv(),
        )
        .await
        .expect("timed out waiting for the scheduled turn to finish")
        .expect("event channel open");
        match event {
            EneEvent::TurnStarted {
                origin: TurnOrigin::Scheduled,
                ..
            } => saw_started = true,
            EneEvent::Terminal {
                origin: TurnOrigin::Scheduled,
                reason,
                ..
            } => break reason,
            _ => {}
        }
    };
    assert!(saw_started, "a scheduled TurnStarted must precede Terminal");
    assert!(matches!(terminal, TerminalReason::Done));
    let runs = wait_for_run_status(&handle, schedule.id, ScheduleRunStatus::Success).await;
    assert_eq!(runs[0].retries, 0);
    let schedules = handle.list_schedules().await.expect("list schedules");
    let stored = schedules
        .iter()
        .find(|s| s.id == schedule.id)
        .expect("schedule kept");
    assert!(
        stored.next_run_at.is_none(),
        "one-shot completes after its fire"
    );
    drop(handle.shutdown(Duration::from_secs(2)).await);
}

/// A fire due mid-conversation is recorded `skipped_busy` and never starts a
/// scheduled turn; the conversation's single-flight gate stays untouched.
#[tokio::test]
async fn scheduled_fire_never_interrupts_conversation() {
    let handle = open_memory_on(None).await;
    let mut rx = handle.subscribe();
    let turn = handle.run("hello").expect("run claims the gate");
    let schedule = handle
        .add_schedule(one_shot_tool_schedule(false))
        .await
        .expect("add schedule");

    wait_for_run_status(&handle, schedule.id, ScheduleRunStatus::SkippedBusy).await;

    // No scheduled turn may have started while the conversation was in flight.
    while let Ok(event) = rx.try_recv() {
        assert!(
            !matches!(
                event,
                EneEvent::TurnStarted {
                    origin: TurnOrigin::Scheduled,
                    ..
                }
            ),
            "scheduled turn must not start during a conversation"
        );
    }
    assert_eq!(handle.active_turn().as_ref(), Some(&turn));

    drop(handle.cancel(&turn));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let event = tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            rx.recv(),
        )
        .await
        .expect("timed out waiting for the cancelled turn's Terminal")
        .expect("event channel open");
        if matches!(event, EneEvent::Terminal { .. }) {
            break;
        }
    }
    assert!(handle.active_turn().is_none(), "gate released after cancel");
    drop(handle.shutdown(Duration::from_secs(2)).await);
}

/// A denied confirmation records `denied` and never executes the action.
#[tokio::test]
async fn confirmation_deny_records_denied_run() {
    let handle = open_memory_on(None).await;
    let mut rx = handle.subscribe();
    let schedule = handle
        .add_schedule(one_shot_tool_schedule(true))
        .await
        .expect("add schedule");

    let request_id = wait_for_confirmation(&mut rx).await;
    drop(handle.decide_permission(request_id, PermissionDecision::Deny));

    let runs = wait_for_run_status(&handle, schedule.id, ScheduleRunStatus::Denied).await;
    assert_eq!(runs[0].error, None);
    drop(handle.shutdown(Duration::from_secs(2)).await);
}

/// An approved confirmation executes the action and records success.
#[tokio::test]
async fn confirmation_approve_executes_run() {
    let handle = open_memory_on(None).await;
    let mut rx = handle.subscribe();
    let schedule = handle
        .add_schedule(one_shot_tool_schedule(true))
        .await
        .expect("add schedule");

    let request_id = wait_for_confirmation(&mut rx).await;
    drop(handle.decide_permission(request_id, PermissionDecision::AllowOnce));

    let runs = wait_for_run_status(&handle, schedule.id, ScheduleRunStatus::Success).await;
    assert_eq!(runs[0].retries, 0);
    drop(handle.shutdown(Duration::from_secs(2)).await);
}

async fn wait_for_confirmation(rx: &mut EneEventReceiver) -> ene_runtime::RequestId {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let event = tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            rx.recv(),
        )
        .await
        .expect("timed out waiting for a confirmation prompt")
        .expect("event channel open");
        if let EneEvent::PermissionRequired {
            origin: TurnOrigin::Scheduled,
            request_id,
            ..
        } = event
        {
            return request_id;
        }
    }
}

/// Acceptance criterion 1: schedules and run history survive a restart and
/// are restored from the database.
#[tokio::test]
async fn schedules_and_history_restore_after_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("scheduler.db");
    let db_str = db_path.to_str().expect("utf8 path").to_string();

    let handle = open_memory_on(Some(&db_str)).await;
    let schedule = handle
        .add_schedule(one_shot_tool_schedule(false))
        .await
        .expect("add schedule");
    wait_for_run_status(&handle, schedule.id, ScheduleRunStatus::Success).await;
    drop(handle.shutdown(Duration::from_secs(2)).await);

    // "Restart": reopen against the same database file.
    let handle = open_memory_on(Some(&db_str)).await;
    let schedules = handle.list_schedules().await.expect("list schedules");
    let stored = schedules
        .iter()
        .find(|s| s.id == schedule.id)
        .expect("schedule restored");
    assert_eq!(stored.name, schedule.name);
    assert!(stored.next_run_at.is_none());
    assert_eq!(stored.run_count, 1);
    let runs = handle
        .list_schedule_runs(schedule.id, 10)
        .await
        .expect("list runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, ScheduleRunStatus::Success);
    assert_eq!(
        runs[0].scheduled_at,
        schedule.next_run_at.expect("first fire time")
    );
    drop(handle.shutdown(Duration::from_secs(2)).await);
}
