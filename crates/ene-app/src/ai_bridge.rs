use bevy::prelude::*;
use std::collections::VecDeque;
use std::sync::Mutex;
use tokio::sync::mpsc::{self, UnboundedReceiver, error::TryRecvError};
use tokio_stream::StreamExt;

use crate::app_config::CharacterSettings;
use crate::character::ResolvedExpressionMap;
use ene_ai_core::{
    AiRuntime,
    poll_split_result,
    stream::{AiStreamEvent as CoreAiStreamEvent, run_ai_with_tools},
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
    pub runtime: Option<AiRuntime>,
    pub embedding_in_progress: bool,
    worker_rx: Mutex<Option<UnboundedReceiver<CoreAiStreamEvent>>>,
}

impl Default for AiRuntimeState {
    fn default() -> Self {
        Self {
            processing: false,
            pending: VecDeque::new(),
            runtime: None,
            embedding_in_progress: false,
            worker_rx: Mutex::new(None),
        }
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
    let runtime = runtime_state.runtime.as_mut().unwrap();
    runtime.session.add_user_message(&user_input);

    let ai_settings = settings.ai.ai.clone();
    let session_clone = runtime.session.clone();
    let registry_clone = runtime.registry.clone();

    rt.0.spawn(async move {
        match run_ai_with_tools(&ai_settings, &session_clone, &user_input, registry_clone).await {
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

    // Initialize unified runtime if not yet initialized
    if runtime_state.runtime.is_none() {
        let ai_settings = settings.ai.ai.clone();
        match rt.0.block_on(AiRuntime::init(ai_settings)) {
            Ok(runtime) => {
                runtime_state.runtime = Some(runtime);
                eprintln!("\x1b[36m[Runtime] Unified AI Runtime initialized successfully.\x1b[0m");
            }
            Err(e) => {
                runtime_state.pending.pop_front();
                stream_writer.write(AiStreamEvent::Error(format!("Failed to initialize AI runtime: {}", e)));
                return;
            }
        }
    }

    let runtime = runtime_state.runtime.as_mut().unwrap();

    // Character card loading if card path changed
    if runtime.session.current_card_path != settings.ai.ai.character_card_path {
        match runtime.session.load_card(&settings.ai.ai.character_card_path) {
            Ok(resolved) => {
                expression_map.map = resolved.into_iter().map(|e| (e.name, e.vrm)).collect();
            }
            Err(e) => {
                runtime_state.pending.pop_front();
                stream_writer.write(AiStreamEvent::Error(e.to_string()));
                return;
            }
        }
    }

    // Unify Session splitting boundary check
    runtime.check_and_perform_split(&user_input, &settings.ai.ai.user_name);

    // Embed asynchronously without blocking the game loop.
    // If the embedding takes longer than 50ms, we defer to the next frame
    // via the embedding_in_progress flag and process_embedding system.
    let embedder = runtime.session.memory.embedding_provider.clone();
    if let Some(embedder) = embedder {
        let user_input_clone = user_input.clone();
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
                runtime.session.set_pending_embedding(embedding.clone());
                runtime.session.set_last_input_embedding(embedding);
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

    runtime.session.record_user_input();

    launch_ai_request(&mut runtime_state, &settings, &rt, &mut stream_writer);
}

fn poll_ai_worker(
    mut runtime_state: ResMut<AiRuntimeState>,
    mut stream_writer: MessageWriter<AiStreamEvent>,
    settings: Res<CharacterSettings>,
) {
    if settings.ai.ai.memory.enabled && settings.ai.ai.memory.auto_session_split {
        if let Some(runtime) = runtime_state.runtime.as_mut() {
            if let Some(result) = poll_split_result(&mut runtime.pending_split) {
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
                        runtime.session.reset_session();
                        runtime.session.memory.session_id = split_result.new_session_id;
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
                let runtime = runtime_state.runtime.as_mut().unwrap();
                let (text_deltas, special_tokens) = runtime.session.process_delta(&delta);

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
                let runtime = runtime_state.runtime.as_mut().unwrap();
                if let Some(tail) = runtime.session.finalize_response() {
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
