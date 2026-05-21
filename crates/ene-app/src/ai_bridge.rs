use bevy::prelude::*;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::mpsc::{self, UnboundedReceiver, error::TryRecvError};
use tokio_stream::StreamExt;

use crate::app_config::CharacterSettings;
use crate::character::ResolvedExpressionMap;
use ene_ai_core::{
    PendingSplitTask, SplitTaskInput, init_memory,
    mcp_client::McpToolRegistry,
    poll_split_result,
    session::ConversationSession,
    spawn_split_task,
    stream::{AiStreamEvent as CoreAiStreamEvent, run_ai_with_tools},
    tool_host_manager::ToolHostManager,
    tools::ToolRegistry,
    truncate,
};

pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<AiRequestEvent>()
            .add_message::<AiStreamEvent>()
            .init_resource::<AiRuntimeState>()
            .init_resource::<AiTokioRuntime>()
            .add_systems(
                Update,
                (
                    enqueue_ai_requests,
                    process_embedding,
                    start_next_ai_request,
                    poll_ai_worker,
                )
                    .chain(),
            );
    }
}

#[derive(Message, Debug, Clone)]
pub struct AiRequestEvent {
    pub user_input: String,
}

#[derive(Message, Debug, Clone)]
#[allow(dead_code)]
pub enum AiStreamEvent {
    TextDelta(String),
    SpecialToken(String),
    ToolCallStart {
        name: String,
        arguments: String,
    },
    ToolCallResult {
        name: String,
        result: String,
    },
    PermissionRequired {
        request_id: String,
        action: String,
        target: String,
        description: String,
    },
    TaskProgress {
        task_id: String,
        step: usize,
        total_steps: usize,
        description: String,
    },
    Finished,
    Error(String),
}

#[derive(Resource)]
pub struct AiTokioRuntime(pub tokio::runtime::Runtime);

impl FromWorld for AiTokioRuntime {
    fn from_world(_world: &mut World) -> Self {
        Self(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime"),
        )
    }
}

#[derive(Resource)]
pub struct AiRuntimeState {
    pub processing: bool,
    pub pending: VecDeque<String>,
    pub session: ConversationSession,
    pub pending_split: Option<PendingSplitTask>,
    pub embedding_in_progress: bool,
    worker_rx: Mutex<Option<UnboundedReceiver<CoreAiStreamEvent>>>,
}

impl Default for AiRuntimeState {
    fn default() -> Self {
        Self {
            processing: false,
            pending: VecDeque::new(),
            session: ConversationSession::new(),
            pending_split: None,
            embedding_in_progress: false,
            worker_rx: Mutex::new(None),
        }
    }
}

impl AiRuntimeState {
    fn init_memory_from_settings(
        &mut self,
        settings: &ene_ai_core::config::AiSettings,
    ) -> Result<(), String> {
        let (store, embedder) = init_memory(settings)?;
        self.session.init_memory(store, embedder);
        Ok(())
    }
}

fn enqueue_ai_requests(
    mut requests: MessageReader<AiRequestEvent>,
    mut runtime_state: ResMut<AiRuntimeState>,
) {
    for request in requests.read() {
        if !request.user_input.trim().is_empty() {
            runtime_state.pending.push_back(request.user_input.clone());
        }
    }
}

/// Poll for asynchronous embedding task completion without blocking the game loop.
fn process_embedding(
    mut runtime_state: ResMut<AiRuntimeState>,
    settings: Res<CharacterSettings>,
    rt: Res<AiTokioRuntime>,
    mut stream_writer: MessageWriter<AiStreamEvent>,
) {
    if !runtime_state.embedding_in_progress {
        return;
    }

    runtime_state.embedding_in_progress = false;
    launch_ai_request(&mut runtime_state, &settings, &rt, &mut stream_writer);
}

fn launch_ai_request(
    runtime_state: &mut AiRuntimeState,
    settings: &CharacterSettings,
    rt: &AiTokioRuntime,
    stream_writer: &mut MessageWriter<AiStreamEvent>,
) {
    let Some(user_input) = runtime_state.pending.front().cloned() else {
        return;
    };

    let (tx, rx) = mpsc::unbounded_channel();
    {
        let Ok(mut guard) = runtime_state.worker_rx.lock() else {
            runtime_state.pending.pop_front();
            stream_writer.write(AiStreamEvent::Error(
                "failed to acquire AI worker receiver lock".to_string(),
            ));
            runtime_state.processing = false;
            return;
        };
        *guard = Some(rx);
    }

    runtime_state.pending.pop_front();
    runtime_state.session.add_user_message(&user_input);

    let ai_settings = settings.ai.clone();
    let session_clone = runtime_state.session.clone();

    rt.0.spawn(async move {
        let registry = build_registry(&ai_settings).await;
        match run_ai_with_tools(&ai_settings, &session_clone, &user_input, registry).await {
            Ok(stream) => {
                tokio::pin!(stream);
                while let Some(event) = stream.next().await {
                    if tx.send(event).is_err() {
                        return;
                    }
                }
            }
            Err(err) => {
                let _ = tx.send(CoreAiStreamEvent::Error(err.to_string()));
            }
        }
    });

    runtime_state.processing = true;
}

fn start_next_ai_request(
    mut runtime_state: ResMut<AiRuntimeState>,
    settings: Res<CharacterSettings>,
    rt: Res<AiTokioRuntime>,
    mut stream_writer: MessageWriter<AiStreamEvent>,
    mut expression_map: ResMut<ResolvedExpressionMap>,
) {
    if runtime_state.processing || runtime_state.embedding_in_progress {
        return;
    }

    let Some(user_input) = runtime_state.pending.front().cloned() else {
        return;
    };

    // Initialize memory if not yet initialized
    if runtime_state.session.embedding_provider.is_none() && settings.ai.memory.enabled {
        if let Err(e) = runtime_state.init_memory_from_settings(&settings.ai) {
            eprintln!("[Memory] Warning: Failed to initialize memory: {}", e);
            eprintln!("[Memory] Continuing without long-term memory.");
        } else {
            eprintln!("\x1b[36m[Memory] Long-term memory enabled.\x1b[0m");
            eprintln!(
                "\x1b[36m[Memory] DB: {}\x1b[0m",
                settings.ai.resolve_memory_db_path().display()
            );
        }
    }

    // Character card loading
    if runtime_state.session.current_card_path != settings.ai.character_card_path {
        match runtime_state
            .session
            .load_card(&settings.ai.character_card_path)
        {
            Ok(resolved) => {
                expression_map.map = resolved.into_iter().map(|e| (e.name, e.vrm)).collect();
            }
            Err(e) => {
                runtime_state.pending.pop_front();
                stream_writer.write(AiStreamEvent::Error(e));
                return;
            }
        }
    }

    if settings.ai.memory.enabled && settings.ai.memory.auto_session_split {
        let split_params = {
            let session = &runtime_state.session;
            session
                .memory_store
                .as_ref()
                .zip(session.embedding_provider.as_ref())
                .map(|(store, embedder)| {
                    (
                        session.last_input_embedding.clone(),
                        session.last_message_time,
                        session.current_turn_count,
                        session.conversation_history.clone(),
                        session.session_id.clone(),
                        session.card_name().to_string(),
                        store.clone(),
                        embedder.clone(),
                    )
                })
        };
        if let Some((
            last_input_embedding,
            last_message_time,
            current_turn_count,
            history,
            session_id,
            card_name,
            store,
            embedder,
        )) = split_params
        {
            spawn_split_task(
                &mut runtime_state.pending_split,
                SplitTaskInput {
                    last_input_embedding,
                    last_message_time,
                    current_turn_count,
                    user_input: user_input.clone(),
                    settings: settings.ai.clone(),
                    history,
                    session_id,
                    card_name,
                    user_name: settings.ai.user_name.clone(),
                    store,
                    embedder,
                },
            );
        }
    }

    // Embed asynchronously without blocking the game loop.
    // If the embedding takes longer than 50ms, we defer to the next frame
    // via the embedding_in_progress flag and process_embedding system.
    if let Some(embedder) = runtime_state.session.embedding_provider.clone() {
        let user_input_clone = user_input.clone();
        let session_ptr = &mut runtime_state.session;

        let (embed_tx, embed_rx) = tokio::sync::oneshot::channel();
        rt.0.spawn(async move {
            let result = embedder.embed_query(&user_input_clone).await;
            let _ = embed_tx.send(result);
        });

        match rt.0.block_on(tokio::time::timeout(
            std::time::Duration::from_millis(50),
            embed_rx,
        )) {
            Ok(Ok(Ok(embedding))) => {
                session_ptr.set_pending_embedding(embedding.clone());
                session_ptr.set_last_input_embedding(embedding);
            }
            Ok(Ok(Err(e))) => {
                eprintln!("[Embedding] Error: {}", e);
            }
            Ok(Err(_)) => {
                eprintln!("[Embedding] Channel closed without value");
            }
            Err(_) => {
                runtime_state.embedding_in_progress = true;
                return;
            }
        }
    }

    runtime_state.session.record_user_input();

    launch_ai_request(&mut runtime_state, &settings, &rt, &mut stream_writer);
}

fn poll_ai_worker(
    mut runtime_state: ResMut<AiRuntimeState>,
    mut stream_writer: MessageWriter<AiStreamEvent>,
    settings: Res<CharacterSettings>,
) {
    if settings.ai.memory.enabled && settings.ai.memory.auto_session_split {
        if let Some(result) = poll_split_result(&mut runtime_state.pending_split) {
            match result {
                Ok(split_result) => {
                    eprintln!("\x1b[33m[Session] {} \x1b[0m", split_result.reason);
                    eprintln!(
                        "\x1b[33m[Session] Conversation summarized and saved: {}\x1b[0m",
                        truncate(&split_result.summary, 80)
                    );
                    if !split_result.key_facts.is_empty() {
                        let facts_str = split_result
                            .key_facts
                            .iter()
                            .map(|f| format!("{}:{}", f.key, f.value))
                            .collect::<Vec<_>>()
                            .join(", ");
                        eprintln!("\x1b[33m[Session] Key facts: {}\x1b[0m", facts_str);
                    }
                    runtime_state.session.reset_session();
                    runtime_state.session.session_id = split_result.new_session_id;
                    eprintln!("\x1b[33m[Session] Starting new conversation.\x1b[0m");
                }
                Err(e) => {
                    if !matches!(e, ene_ai_core::AiCoreError::SplitNotNeeded) {
                        eprintln!("\x1b[31m[Session] Summary generation error: {}\x1b[0m", e);
                    }
                }
            }
        }
    }

    let mut drained_events = Vec::new();
    let mut clear_receiver = false;
    let mut disconnected = false;

    if let Ok(mut guard) = runtime_state.worker_rx.lock()
        && let Some(receiver) = guard.as_mut()
    {
        loop {
            match receiver.try_recv() {
                Ok(event) => drained_events.push(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    clear_receiver = true;
                    disconnected = true;
                    break;
                }
            }
        }

        if clear_receiver {
            *guard = None;
        }
    }

    if disconnected {
        runtime_state.processing = false;
    }

    for event in drained_events {
        match event {
            CoreAiStreamEvent::TextDelta(delta) => {
                let (text_deltas, special_tokens) = runtime_state.session.process_delta(&delta);

                for text_delta in text_deltas {
                    stream_writer.write(AiStreamEvent::TextDelta(text_delta));
                }
                for token in special_tokens {
                    stream_writer.write(AiStreamEvent::SpecialToken(token));
                }
            }
            CoreAiStreamEvent::ToolCallStart { name, arguments } => {
                stream_writer.write(AiStreamEvent::ToolCallStart { name, arguments });
            }
            CoreAiStreamEvent::ToolCallResult { name, result } => {
                stream_writer.write(AiStreamEvent::ToolCallResult { name, result });
            }
            CoreAiStreamEvent::Finished => {
                if let Some(tail) = runtime_state.session.finalize_response() {
                    stream_writer.write(AiStreamEvent::TextDelta(tail));
                }

                stream_writer.write(AiStreamEvent::Finished);
                runtime_state.processing = false;
                if let Ok(mut guard) = runtime_state.worker_rx.lock() {
                    *guard = None;
                }
            }
            CoreAiStreamEvent::Error(error) => {
                stream_writer.write(AiStreamEvent::Error(error));
                runtime_state.processing = false;
                if let Ok(mut guard) = runtime_state.worker_rx.lock() {
                    *guard = None;
                }
            }
            CoreAiStreamEvent::PermissionRequired {
                request_id,
                action,
                target,
                description,
            } => {
                stream_writer.write(AiStreamEvent::PermissionRequired {
                    request_id,
                    action,
                    target,
                    description,
                });
            }
            CoreAiStreamEvent::TaskProgress {
                task_id,
                step,
                total_steps,
                description,
            } => {
                stream_writer.write(AiStreamEvent::TaskProgress {
                    task_id,
                    step,
                    total_steps,
                    description,
                });
            }
            CoreAiStreamEvent::SpecialToken(_) => {}
            CoreAiStreamEvent::SessionSplit { summary, reason } => {
                eprintln!("[Session] {}: {}", reason, summary);
            }
        }
    }
}

async fn build_registry(settings: &ene_ai_core::config::AiSettings) -> Arc<dyn ToolRegistry> {
    let mut manager = match ToolHostManager::start(settings).await {
        Ok(m) => m,
        Err(e) => {
            eprintln!("[ToolHostManager] Warning: {}", e);
            ToolHostManager::start(&ene_ai_core::config::AiSettings {
                tools: ene_ai_core::config::AiToolSettings {
                    enabled: Vec::new(),
                },
                ..settings.clone()
            })
            .await
            .unwrap_or_else(|e2| {
                eprintln!("[ToolHostManager] Fatal: {}", e2);
                panic!("Failed to start tool host manager");
            })
        }
    };

    if !settings.mcp_servers.is_empty() {
        let mcp = McpToolRegistry::new();
        for server in &settings.mcp_servers {
            if !server.enabled {
                continue;
            }

            match &server.transport {
                ene_ai_core::config::McpTransport::Stdio { command, args } => {
                    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                    if let Err(err) = mcp.connect_stdio(&server.name, command, &args_ref).await {
                        eprintln!(
                            "Warning: MCP server '{}' failed to connect: {}",
                            server.name, err
                        );
                    }
                }
                ene_ai_core::config::McpTransport::Http { url } => {
                    eprintln!(
                        "Warning: MCP HTTP transport not supported yet for '{}': {}",
                        server.name, url
                    );
                }
            }
        }
        manager.add_registry(Arc::new(mcp));
    }

    manager.into_registry()
}
