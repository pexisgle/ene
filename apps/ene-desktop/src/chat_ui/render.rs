//! Chat window egui rendering.

use std::sync::Arc;

use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use ene_runtime::Role;

use crate::ai_bridge::AiBridge;
use crate::chat_state::ChatState;
use crate::component::chat::ChatStateComponent;

use super::dialogs::{render_permission_dialog, render_user_input_dialog};

#[derive(Default)]
pub struct ChatUi {
    /// Active microphone capture handle. Lives here (not in the ECS
    /// world) because `cpal::Stream` is `!Send + !Sync`.
    #[cfg(feature = "voice")]
    mic_handle: crate::audio::MicCaptureHandle,
    /// Placeholder so the struct has a field in text-only builds too.
    #[cfg(not(feature = "voice"))]
    mic_handle: Option<()>,
}

impl std::fmt::Debug for ChatUi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatUi")
            .field("mic_active", &self.mic_handle.is_some())
            .finish()
    }
}

impl ChatUi {
    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        ai: Option<&Arc<AiBridge>>,
        world: &mut World,
        chat_entity: Entity,
    ) {
        let Some(ai) = ai else {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                i18n_embed_fl::fl!(crate::i18n::loader(), "runtime-unavailable"),
            );
            return;
        };
        let Some(mut chat_state) = world.get_mut::<ChatStateComponent>(chat_entity) else {
            return;
        };
        let processing = ai.is_processing();
        let can_cancel = processing || ai.has_active_turn();
        let scroll_to_bottom = chat_state.0.scroll_to_bottom;
        let messages = chat_state.0.messages.clone();
        chat_state.0.scroll_to_bottom = false;

        // Mic state read once per frame so the toggle button can reflect it
        // without holding a world borrow inside the egui closure. The
        // `mic_active` flag is the authoritative "is capture live" state:
        // the error callback clears it on device unplug even though
        // the `!Send` handle may still exist on the chat UI.
        #[cfg(feature = "voice")]
        let mic_active = world
            .get_resource::<crate::audio::AudioState>()
            .is_some_and(crate::audio::AudioState::is_mic_active);

        let available = ui.available_size();
        let input_height = 88.0;
        let message_area_height = (available.y - input_height - 12.0).max(120.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .stick_to_bottom(scroll_to_bottom)
            .max_height(message_area_height)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if messages.is_empty() {
                    render_greeting_picker(ui, ai, world, chat_entity, processing);
                } else {
                    for message in &messages {
                        render_message_bubble(ui, message);
                    }
                }
            });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "chat-input"));
            ui.add_enabled_ui(!processing, |ui| {
                let mut draft = world
                    .get_mut::<ChatStateComponent>(chat_entity)
                    .map(|mut c| std::mem::take(&mut c.0.input_draft))
                    .unwrap_or_default();

                let response = ui.add(
                    egui::TextEdit::multiline(&mut draft)
                        .desired_width(f32::INFINITY)
                        .desired_rows(3)
                        .hint_text(if processing {
                            i18n_embed_fl::fl!(crate::i18n::loader(), "waiting-for-ai")
                        } else {
                            i18n_embed_fl::fl!(crate::i18n::loader(), "message-to-ai")
                        }),
                );

                let send_clicked = ui
                    .add_enabled(
                        !processing,
                        egui::Button::new(i18n_embed_fl::fl!(crate::i18n::loader(), "send")),
                    )
                    .clicked();
                let cancel_clicked = ui
                    .add_enabled(
                        can_cancel,
                        egui::Button::new(i18n_embed_fl::fl!(crate::i18n::loader(), "cancel")),
                    )
                    .clicked();
                let undo_clicked = ui
                    .add_enabled(
                        !processing,
                        egui::Button::new(i18n_embed_fl::fl!(crate::i18n::loader(), "chat-undo")),
                    )
                    .clicked();

                // Microphone toggle with a live active indicator. The
                // button stays enabled while the AI is processing so the
                // user can stop capture at any time.
                #[cfg(feature = "voice")]
                let mic_clicked = {
                    let mic_button = if mic_active {
                        egui::Button::new(
                            egui::RichText::new(format!(
                                "● {}",
                                i18n_embed_fl::fl!(crate::i18n::loader(), "audio-mic-active")
                            ))
                            .color(egui::Color32::from_rgb(220, 80, 80)),
                        )
                        .fill(egui::Color32::from_rgb(60, 30, 30))
                    } else {
                        egui::Button::new(i18n_embed_fl::fl!(crate::i18n::loader(), "audio-mic"))
                    };
                    ui.add(mic_button)
                        .on_hover_text(if mic_active {
                            i18n_embed_fl::fl!(crate::i18n::loader(), "audio-mic-toggle-off")
                        } else {
                            i18n_embed_fl::fl!(crate::i18n::loader(), "audio-mic-toggle-on")
                        })
                        .clicked()
                };
                #[cfg(not(feature = "voice"))]
                let mic_clicked = false;

                // Multiline TextEdit inserts a newline on Enter; detect send
                // while focused instead of waiting for lost_focus.
                let enter_send = !processing
                    && response.has_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                let escape_cancel = can_cancel && ui.input(|i| i.key_pressed(egui::Key::Escape));

                if cancel_clicked || escape_cancel {
                    ai.cancel();
                }

                if mic_clicked
                    && let Err(e) =
                        crate::audio::toggle_mic_capture(world, ai, &mut self.mic_handle)
                {
                    tracing::warn!(component = "Audio", error = %e, "mic toggle failed");
                    if let Some(mut chat) = world.get_mut::<ChatStateComponent>(chat_entity) {
                        chat.0.undo_status = Some(format!(
                            "{}: {e}",
                            i18n_embed_fl::fl!(crate::i18n::loader(), "audio-mic-error")
                        ));
                    }
                }

                if undo_clicked {
                    let status = match ai.undo_blocking() {
                        Ok(ene_runtime::UndoReport::NothingToUndo) => {
                            i18n_embed_fl::fl!(crate::i18n::loader(), "chat-undo-nothing")
                        }
                        Ok(ene_runtime::UndoReport::Irreversible { metadata }) => format!(
                            "{} ({})",
                            i18n_embed_fl::fl!(crate::i18n::loader(), "chat-undo-irreversible"),
                            metadata.tool_name
                        ),
                        Ok(ene_runtime::UndoReport::Reverted { metadata, .. }) => format!(
                            "{} ({})",
                            i18n_embed_fl::fl!(crate::i18n::loader(), "chat-undo-reverted"),
                            metadata.tool_name
                        ),
                        Ok(ene_runtime::UndoReport::Failed { metadata, error }) => format!(
                            "{} ({}: {})",
                            i18n_embed_fl::fl!(crate::i18n::loader(), "chat-undo-failed"),
                            metadata.tool_name,
                            error
                        ),
                        Err(e) => {
                            format!(
                                "{} ({e})",
                                i18n_embed_fl::fl!(crate::i18n::loader(), "chat-undo-failed")
                            )
                        }
                    };
                    if let Some(mut chat) = world.get_mut::<ChatStateComponent>(chat_entity) {
                        chat.0.undo_status = Some(status);
                    }
                }

                if let Some(mut chat) = world.get_mut::<ChatStateComponent>(chat_entity) {
                    chat.0.input_draft = draft;
                    if send_clicked || enter_send {
                        while chat.0.input_draft.ends_with('\n') {
                            chat.0.input_draft.pop();
                        }
                        send_chat(ai, &mut chat.0);
                    }
                }
            });
        });

        if let Some(chat) = world.get::<ChatStateComponent>(chat_entity)
            && let Some(status) = chat.0.undo_status.as_deref()
        {
            ui.weak(status);
        }
        if let Some(chat) = world.get::<ChatStateComponent>(chat_entity)
            && let Some(status) = chat.0.greeting_status.as_deref()
        {
            ui.weak(status);
        }

        render_permission_dialog(ui, world, chat_entity, ai);
        render_user_input_dialog(ui, world, chat_entity, ai);
    }
}

fn render_greeting_picker(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    world: &mut World,
    chat_entity: Entity,
    processing: bool,
) {
    let Some(card) = ai.character_card() else {
        ui.weak(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "chat-empty-history"
        ));
        return;
    };
    let greetings = card.data.greeting_options();
    if greetings.is_empty() {
        ui.weak(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "chat-empty-history"
        ));
        return;
    }

    let mut selected: Option<u32> = None;
    ui.label(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "chat-greeting-prompt"
    ));
    for (index, text) in &greetings {
        let label = first_line(text);
        let label: String = if label.chars().count() > 48 {
            label.chars().take(48).collect()
        } else {
            label.to_string()
        };
        if ui
            .add_enabled(!processing, egui::Button::new(format!("[{index}] {label}")))
            .clicked()
        {
            selected = Some(*index);
        }
    }

    let Some(index) = selected else {
        return;
    };
    match ai.set_greeting_blocking(index) {
        Ok(_) => {
            if let Some(mut chat) = world.get_mut::<ChatStateComponent>(chat_entity) {
                chat.0.needs_history_reconcile = true;
                chat.0.greeting_status = None;
            }
        }
        Err(e) => {
            if let Some(mut chat) = world.get_mut::<ChatStateComponent>(chat_entity) {
                chat.0.greeting_status = Some(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "chat-greeting-failed",
                    error = e.to_string()
                ));
            }
        }
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("")
}

fn render_message_bubble(ui: &mut egui::Ui, message: &crate::chat_state::ChatMessage) {
    let is_user = message.role == Role::User;
    let label = if is_user {
        i18n_embed_fl::fl!(crate::i18n::loader(), "chat-you")
    } else {
        i18n_embed_fl::fl!(crate::i18n::loader(), "chat-ene")
    };

    let row_width = ui.available_width();
    let bubble_max_width = (row_width * 0.82).max(120.0);
    let frame = egui::Frame::new()
        .fill(if is_user {
            egui::Color32::from_rgb(52, 90, 130)
        } else {
            egui::Color32::from_rgb(38, 42, 50)
        })
        .inner_margin(egui::Margin::symmetric(10, 8))
        .corner_radius(8.0);

    let align = if is_user {
        egui::Align::Max
    } else {
        egui::Align::Min
    };

    ui.with_layout(egui::Layout::top_down(align), |ui| {
        ui.set_width(row_width);
        frame.show(ui, |ui| {
            ui.set_max_width(bubble_max_width);
            ui.label(egui::RichText::new(label).small().weak());
            let mut text = message.content.clone();
            if message.is_streaming {
                text.push('▌');
            }
            if text.is_empty() && message.is_streaming {
                ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "waiting-for-ai"));
            } else {
                ui.add(egui::Label::new(text).wrap().selectable(true));
            }
        });
    });
    ui.add_space(6.0);
}

pub fn send_chat(ai: &Arc<AiBridge>, chat: &mut ChatState) {
    if ai.is_processing() {
        return;
    }
    let trimmed = chat.input_draft.trim().to_string();
    if !chat.prepare_send(&trimmed) {
        return;
    }
    ai.run(trimmed);
}
