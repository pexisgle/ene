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
            ui.colored_label(egui::Color32::LIGHT_RED, crate::i18n::runtime_unavailable());
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
        // without holding a world borrow inside the egui closure.
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
                    ui.weak(crate::i18n::chat_empty_history());
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
            ui.label(crate::i18n::chat_input());
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
                            crate::i18n::waiting_for_ai()
                        } else {
                            crate::i18n::message_to_ai()
                        }),
                );

                let send_clicked = ui
                    .add_enabled(!processing, egui::Button::new(crate::i18n::send()))
                    .clicked();
                let cancel_clicked = ui
                    .add_enabled(can_cancel, egui::Button::new(crate::i18n::cancel()))
                    .clicked();
                let undo_clicked = ui
                    .add_enabled(!processing, egui::Button::new(crate::i18n::chat_undo()))
                    .clicked();

                // Microphone toggle with a live active indicator. The
                // button stays enabled while the AI is processing so the
                // user can stop capture at any time.
                #[cfg(feature = "voice")]
                let mic_clicked = {
                    let mic_button = if mic_active {
                        egui::Button::new(
                            egui::RichText::new(format!("● {}", crate::i18n::audio_mic_active()))
                                .color(egui::Color32::from_rgb(220, 80, 80)),
                        )
                        .fill(egui::Color32::from_rgb(60, 30, 30))
                    } else {
                        egui::Button::new(crate::i18n::audio_mic())
                    };
                    ui.add(mic_button)
                        .on_hover_text(if mic_active {
                            crate::i18n::audio_mic_toggle_off()
                        } else {
                            crate::i18n::audio_mic_toggle_on()
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
                        chat.0.undo_status =
                            Some(format!("{}: {e}", crate::i18n::audio_mic_error()));
                    }
                }

                if undo_clicked {
                    let status = match ai.undo_blocking() {
                        Ok(ene_runtime::UndoReport::NothingToUndo) => {
                            crate::i18n::chat_undo_nothing()
                        }
                        Ok(ene_runtime::UndoReport::Irreversible { metadata }) => format!(
                            "{} ({})",
                            crate::i18n::chat_undo_irreversible(),
                            metadata.tool_name
                        ),
                        Ok(ene_runtime::UndoReport::Reverted { metadata, .. }) => format!(
                            "{} ({})",
                            crate::i18n::chat_undo_reverted(),
                            metadata.tool_name
                        ),
                        Ok(ene_runtime::UndoReport::Failed { metadata, error }) => format!(
                            "{} ({}: {})",
                            crate::i18n::chat_undo_failed(),
                            metadata.tool_name,
                            error
                        ),
                        Err(e) => {
                            format!("{} ({e})", crate::i18n::chat_undo_failed())
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

        render_permission_dialog(ui, world, chat_entity, ai);
        render_user_input_dialog(ui, world, chat_entity, ai);
    }
}

fn render_message_bubble(ui: &mut egui::Ui, message: &crate::chat_state::ChatMessage) {
    let is_user = message.role == Role::User;
    let label = if is_user {
        crate::i18n::chat_you()
    } else {
        crate::i18n::chat_ene()
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
                ui.weak(crate::i18n::waiting_for_ai());
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
