use bevy::prelude::*;
use ene_core::{EneEvent, EneEventReceiver, EneHandle, EneStatus, RequestId};

pub struct EnePlugin;

impl Plugin for EnePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<EneRequestEvent>()
            .add_message::<EneStreamEvent>()
            .init_resource::<EneResource>()
            .add_systems(Update, (enqueue_ai_requests, poll_ene_events).chain());
    }
}

/// User input event from the settings UI.
#[derive(Message, Debug, Clone)]
pub struct EneRequestEvent {
    pub user_input: String,
}

/// Stream events for the settings UI and character system.
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
        request_id: RequestId,
        action: String,
        target: String,
        description: String,
    },
    TaskProgress {
        task_id: String,
        step: usize,
        total_steps: Option<usize>,
        description: String,
    },
    Finished,
    Error(String),
}

#[derive(Resource)]
pub struct EneResource {
    pub handle: EneHandle,
    pub receiver: EneEventReceiver,
    pub processing: bool,
}

impl Default for EneResource {
    fn default() -> Self {
        let handle = EneHandle::new();
        let receiver = handle.subscribe();
        Self {
            handle,
            receiver,
            processing: false,
        }
    }
}

fn enqueue_ai_requests(mut requests: MessageReader<EneRequestEvent>, mut ene: ResMut<EneResource>) {
    for request in requests.read() {
        if !request.user_input.trim().is_empty() {
            let _ = ene.handle.run(&request.user_input);
            ene.processing = true;
        }
    }
}

fn poll_ene_events(mut ene: ResMut<EneResource>, mut stream_writer: MessageWriter<EneStreamEvent>) {
    loop {
        match ene.receiver.try_recv() {
            Ok(EneEvent::TextDelta { delta }) => {
                stream_writer.write(EneStreamEvent::TextDelta(delta));
            }
            Ok(EneEvent::SpecialToken { token }) => {
                stream_writer.write(EneStreamEvent::SpecialToken(token));
            }
            Ok(EneEvent::ToolCallStart { name, arguments }) => {
                stream_writer.write(EneStreamEvent::ToolCallStart { name, arguments });
            }
            Ok(EneEvent::ToolCallResult { name, result }) => {
                stream_writer.write(EneStreamEvent::ToolCallResult { name, result });
            }
            Ok(EneEvent::Done) => {
                ene.processing = false;
                stream_writer.write(EneStreamEvent::Finished);
            }
            Ok(EneEvent::Failed { message }) => {
                ene.processing = false;
                stream_writer.write(EneStreamEvent::Error(message));
            }
            Ok(EneEvent::PermissionRequired {
                request_id,
                action,
                target,
                description,
            }) => {
                stream_writer.write(EneStreamEvent::PermissionRequired {
                    request_id,
                    action,
                    target,
                    description,
                });
            }
            Ok(EneEvent::TaskProgress {
                task_id,
                step,
                total_steps,
                description,
            }) => {
                stream_writer.write(EneStreamEvent::TaskProgress {
                    task_id,
                    step,
                    total_steps,
                    description,
                });
            }
            Ok(EneEvent::SessionSplit { .. }) => {}
            Ok(EneEvent::StatusChanged { status }) => {
                if status == EneStatus::Idle {
                    // Stream completed, processing state already handled by Done/Failed
                }
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                warn!("[Ene] Dropped {} events", n);
                // Continue reading to drain any remaining buffered events
                continue;
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
        }
    }
}
