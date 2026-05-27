use bevy::prelude::*;
use std::collections::VecDeque;
use std::sync::Mutex;
use tokio::sync::mpsc::{self, UnboundedReceiver, error::TryRecvError};
use tokio_stream::StreamExt;

use crate::app_config::CharacterSettings;
use crate::character::ResolvedExpressionMap;
use ene_core::{
    EneRuntime,
    stream::{EneStreamEvent as CoreEneStreamEvent, run_ene_with_tools},
    truncate,
};

pub struct EnePlugin;

impl Plugin for EnePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<EneRequestEvent>()
            .add_message::<EneStreamEvent>()
            .init_resource::<EneRuntimeState>()
            .init_resource::<EneTokioRuntime>()
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
pub struct EneRequestEvent {
    pub user_input: String,
}

#[derive(Message, Debug, Clone)]
#[allow(dead_code)]
pub enum EneStreamEvent {
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
pub struct EneTokioRuntime(pub tokio::runtime::Runtime);

impl FromWorld for EneTokioRuntime {
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
pub struct EneRuntimeState {
    pub processing: bool,
    pub pending: VecDeque<String>,
    pub runtime: Option<EneRuntime>,
    pub embedding_in_progress: bool,
    worker_rx: Mutex<Option<UnboundedReceiver<CoreEneStreamEvent>>>,
}

impl Default for EneRuntimeState {
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
    mut requests: MessageReader<EneRequestEvent>,
    mut runtime_state: ResMut<EneRuntimeState>,
) {
    for request in requests.read() {
        if !request.user_input.trim().is_empty() {
            runtime_state.pending.push_back(request.user_input.clone());
        }
    }
}

/// Poll for asynchronous embedding task completion without blocking the game loop.
fn process_embedding(
    mut runtime_state: ResMut<EneRuntimeState>,
    settings: Res<CharacterSettings>,
    rt: Res<EneTokioRuntime>,
    mut stream_writer: MessageWriter<EneStreamEvent>,
) {
    if !runtime_state.embedding_in_progress {
        return;
    }

    runtime_state.embedding_in_progress = false;
    launch_ai_request(&mut runtime_state, &settings, &rt, &mut stream_writer);
}

fn launch_ai_request(
    runtime_state: &mut EneRuntimeState,
    settings: &CharacterSettings,
    rt: &EneTokioRuntime,
    stream_writer: &mut MessageWriter<EneStreamEvent>,
) {
    let Some(user_input) = runtime_state.pending.front().cloned() else {
        return;
    };

    let (tx, rx) = mpsc::unbounded_channel();
    {
        let Ok(mut guard) = runtime_state.worker_rx.lock() else {
            runtime_state.pending.pop_front();
            stream_writer.write(EneStreamEvent::Error(
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
        match run_ene_with_tools(&ai_settings, &session_clone, &user_input, registry_clone).await {
            Ok(stream) => {
                tokio::pin!(stream);
                while let Some(event) = stream.next().await {
                    if tx.send(event).is_err() {
                        return;
                    }
                }
            }
            Err(err) => {
                let _ = tx.send(CoreEneStreamEvent::Error(err.to_string()));
            }
        }
    });

    runtime_state.processing = true;
}

fn start_next_ai_request(
    mut runtime_state: ResMut<EneRuntimeState>,
    settings: Res<CharacterSettings>,
    rt: Res<EneTokioRuntime>,
    mut stream_writer: MessageWriter<EneStreamEvent>,
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
        match rt.0.block_on(async {
            let mut runtime = EneRuntime::init().await?;
            runtime.apply_settings(ai_settings).await?;
            Ok::<_, ene_core::EneCoreError>(runtime)
        }) {
            Ok(runtime) => {
                runtime_state.runtime = Some(runtime);
                info!("[Runtime] Unified AI Runtime initialized successfully.");
            }
            Err(e) => {
                runtime_state.pending.pop_front();
                stream_writer.write(EneStreamEvent::Error(format!(
                    "Failed to initialize AI runtime: {}",
                    e
                )));
                return;
            }
        }
    }

    let runtime = runtime_state.runtime.as_mut().unwrap();

    // Character card loading if card path changed
    if runtime.session.current_card_path != settings.ai.ai.character {
        match runtime.session.load_card(&settings.ai.ai.character) {
            Ok(resolved) => {
                expression_map.map = resolved.into_iter().map(|e| (e.name, e.vrm)).collect();
            }
            Err(e) => {
                runtime_state.pending.pop_front();
                stream_writer.write(EneStreamEvent::Error(e.to_string()));
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
                error!("[Embedding] Error: {}", e);
            }
            Ok(Err(_)) => {
                warn!("[Embedding] Channel closed without value");
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
    mut runtime_state: ResMut<EneRuntimeState>,
    mut stream_writer: MessageWriter<EneStreamEvent>,
    _settings: Res<CharacterSettings>,
) {
    if let Some(runtime) = runtime_state.runtime.as_mut() {
        match runtime.session.apply_pending_split(&mut runtime.pending_split) {
            Some(Ok(split_result)) => {
                info!("[Session] {}", split_result.reason);
                info!(
                    "[Session] Conversation summarized and saved: {}",
                    truncate(&split_result.summary, 80)
                );
                if !split_result.key_facts.is_empty() {
                    let facts_str = split_result
                        .key_facts
                        .iter()
                        .map(|f| format!("{}:{}", f.key, f.value))
                        .collect::<Vec<_>>()
                        .join(", ");
                    info!("[Session] Key facts: {}", facts_str);
                }
                info!("[Session] Starting new conversation.");
            }
            Some(Err(e)) => {
                if !matches!(e, ene_core::SessionError::SplitNotNeeded) {
                    error!("[Session] Summary generation error: {}", e);
                }
            }
            None => {}
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
            CoreEneStreamEvent::TextDelta(delta) => {
                let runtime = runtime_state.runtime.as_mut().unwrap();
                let (text_deltas, special_tokens) = runtime.session.process_delta(&delta);

                for text_delta in text_deltas {
                    stream_writer.write(EneStreamEvent::TextDelta(text_delta));
                }
                for token in special_tokens {
                    stream_writer.write(EneStreamEvent::SpecialToken(token));
                }
            }
            CoreEneStreamEvent::ToolCallStart { name, arguments } => {
                stream_writer.write(EneStreamEvent::ToolCallStart { name, arguments });
            }
            CoreEneStreamEvent::ToolCallResult { name, result } => {
                stream_writer.write(EneStreamEvent::ToolCallResult { name, result });
            }
            CoreEneStreamEvent::Finished => {
                let runtime = runtime_state.runtime.as_mut().unwrap();
                if let Some(tail) = runtime.session.finalize_response() {
                    stream_writer.write(EneStreamEvent::TextDelta(tail));
                }

                stream_writer.write(EneStreamEvent::Finished);
                runtime_state.processing = false;
                if let Ok(mut guard) = runtime_state.worker_rx.lock() {
                    *guard = None;
                }
            }
            CoreEneStreamEvent::Error(error) => {
                stream_writer.write(EneStreamEvent::Error(error));
                runtime_state.processing = false;
                if let Ok(mut guard) = runtime_state.worker_rx.lock() {
                    *guard = None;
                }
            }
            CoreEneStreamEvent::PermissionRequired {
                request_id,
                action,
                target,
                description,
            } => {
                stream_writer.write(EneStreamEvent::PermissionRequired {
                    request_id,
                    action,
                    target,
                    description,
                });
            }
            CoreEneStreamEvent::TaskProgress {
                task_id,
                step,
                total_steps,
                description,
            } => {
                stream_writer.write(EneStreamEvent::TaskProgress {
                    task_id,
                    step,
                    total_steps,
                    description,
                });
            }
            CoreEneStreamEvent::SpecialToken(_) => {}
            CoreEneStreamEvent::SessionSplit { summary, reason } => {
                info!("[Session] {}: {}", reason, summary);
            }
        }
    }
}
