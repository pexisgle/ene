use std::path::Path;
use std::sync::Arc;

use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;

use crate::chat_state::{ChatMessage, ChatState, Role};
use crate::component::chat::ChatStateComponent;
use crate::core_session::CoreSession;

use super::dialogs::{render_permission_dialog, render_user_input_dialog};

#[derive(Default)]
pub struct ChatUi {
    settings_rx: Option<tokio::sync::oneshot::Receiver<Result<serde_json::Value, String>>>,
    chat_plugin: String,
    chat_model: String,
}

impl ChatUi {
    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        ai: Option<&Arc<CoreSession>>,
        world: &mut World,
        chat_entity: Entity,
        mic_handle: &mut crate::audio::MicCaptureHandle,
        assets_dir: &Path,
        card_path: Option<&str>,
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
        self.poll_chat_binding(ai);
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
        let caption = chat_binding_caption(&self.chat_plugin, &self.chat_model);
        if !caption.is_empty() {
            ui.weak(&caption);
            ui.add_space(4.0);
        }
        let message_area_height = (available.y - input_height - 12.0).max(120.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .stick_to_bottom(scroll_to_bottom)
            .max_height(message_area_height)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if messages.is_empty() {
                    render_greeting_picker(
                        ui,
                        ai,
                        world,
                        chat_entity,
                        processing,
                        assets_dir,
                        card_path,
                    );
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
                    && let Err(e) = crate::audio::toggle_mic_capture(world, ai, mic_handle)
                {
                    tracing::warn!(component = "Audio", error = %e, "mic toggle failed");
                    if let Some(mut chat) = world.get_mut::<ChatStateComponent>(chat_entity) {
                        chat.0.undo_status = Some(format!(
                            "{}: {e}",
                            i18n_embed_fl::fl!(crate::i18n::loader(), "audio-mic-error")
                        ));
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

    fn poll_chat_binding(&mut self, ai: &Arc<CoreSession>) {
        if self.settings_rx.is_none() && self.chat_plugin.is_empty() {
            self.settings_rx = Some(ai.fetch_core_settings());
        }
        let Some(receiver) = self.settings_rx.as_mut() else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(settings)) => {
                self.chat_plugin.clear();
                if let Some(plugin) = settings
                    .pointer("/effective/ai/tasks/chat/plugin")
                    .and_then(serde_json::Value::as_str)
                {
                    plugin.clone_into(&mut self.chat_plugin);
                }
                self.chat_model.clear();
                if let Some(model) = settings
                    .pointer("/effective/ai/tasks/chat/model")
                    .and_then(serde_json::Value::as_str)
                {
                    model.clone_into(&mut self.chat_model);
                }
                self.settings_rx = None;
            }
            Ok(Err(_)) | Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                self.settings_rx = None;
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
        }
    }
}

fn chat_binding_caption(plugin: &str, model: &str) -> String {
    if plugin.is_empty() {
        return String::new();
    }
    if plugin == "echo" {
        return i18n_embed_fl::fl!(crate::i18n::loader(), "chat-provider-echo");
    }
    let model = if model.is_empty() { "—" } else { model };
    i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "chat-provider-caption",
        plugin = plugin,
        model = model
    )
}

fn render_greeting_picker(
    ui: &mut egui::Ui,
    ai: &Arc<CoreSession>,
    world: &mut World,
    chat_entity: Entity,
    processing: bool,
    assets_dir: &Path,
    card_path: Option<&str>,
) {
    let greetings = ai.greetings(assets_dir, card_path);
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
    let Some((_, text)) = greetings.iter().find(|(item, _)| *item == index) else {
        return;
    };
    match ai.set_greeting_blocking(index, text) {
        Ok(greeting) => {
            if let Some(mut chat) = world.get_mut::<ChatStateComponent>(chat_entity) {
                chat.0.messages.push(ChatMessage {
                    role: Role::Assistant,
                    content: greeting,
                    is_streaming: false,
                });
                chat.0.scroll_to_bottom = true;
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

pub fn send_chat(ai: &Arc<CoreSession>, chat: &mut ChatState) {
    if ai.is_processing() {
        return;
    }
    let trimmed = chat.input_draft.trim().to_string();
    if !chat.prepare_send(&trimmed) {
        return;
    }
    ai.run(trimmed);
}

#[cfg(test)]
mod tests {
    use super::chat_binding_caption;

    #[test]
    fn caption_is_empty_until_settings_arrive() {
        assert!(chat_binding_caption("", "gpt").is_empty());
    }

    #[test]
    fn caption_warns_when_chat_is_echo() {
        let caption = chat_binding_caption("echo", "echo");
        assert!(caption.contains("Echo"));
    }

    #[test]
    fn caption_shows_plugin_and_model() {
        let caption = chat_binding_caption("provider.openai_compat", "openai/gpt-4o-mini");
        assert!(caption.contains("provider.openai_compat"));
        assert!(caption.contains("openai/gpt-4o-mini"));
    }
}
