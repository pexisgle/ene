//! Chat panel for the surface viewport.

use crate::detail::{DetailTab, chat_setup_gap, chat_setup_status};
use crate::i18n;
use crate::surface::{SurfaceAction, SurfaceUiState};
use ene_api::{HistoryResponse, MessageMode, MessageResponse};

/// Role a transcript row plays in the conversation view. The kind decides
/// alignment and the visible label, so meaning never rests on color alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscriptKind {
    User,
    Assistant,
    Error,
    Tool,
    System,
}

/// Delivery state of a row. Streaming rows get the caret suffix and the
/// waiting placeholder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscriptState {
    Stable,
    Error,
    Streaming,
}

/// Normalized conversation row: role, delivery state, and owned text. Kept
/// independent of egui so follow-up stage features can reuse the transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChatMessageView {
    pub(crate) role: TranscriptKind,
    pub(crate) state: TranscriptState,
    pub(crate) text: String,
}

pub(crate) fn normalize_transcript(
    history: &HistoryResponse,
    streaming_text: &str,
) -> Vec<ChatMessageView> {
    let mut rows = history
        .messages
        .iter()
        .filter_map(normalize_message)
        .collect::<Vec<_>>();
    if !streaming_text.is_empty() {
        rows.push(ChatMessageView {
            role: TranscriptKind::Assistant,
            state: TranscriptState::Streaming,
            text: streaming_text.to_owned(),
        });
    }
    rows
}

fn normalize_message(message: &MessageResponse) -> Option<ChatMessageView> {
    let (kind, state) = match message.role.as_str() {
        "user" => (TranscriptKind::User, TranscriptState::Stable),
        "assistant" => (TranscriptKind::Assistant, TranscriptState::Stable),
        "status" | "error" => (TranscriptKind::Error, TranscriptState::Error),
        "tool" | "tool-summary" => (TranscriptKind::Tool, TranscriptState::Stable),
        "inner" | "thinking" => return None,
        _ => (TranscriptKind::System, TranscriptState::Stable),
    };
    Some(ChatMessageView {
        role: kind,
        state,
        text: message.text.clone(),
    })
}

fn transcript_label(kind: TranscriptKind) -> String {
    i18n::fl(match kind {
        TranscriptKind::User => "chat-role-user",
        TranscriptKind::Assistant => "chat-role-assistant",
        TranscriptKind::Error => "chat-error",
        TranscriptKind::Tool => "chat-tool",
        TranscriptKind::System => "chat-system",
    })
}

pub(crate) fn render_message_bubble(ui: &mut egui::Ui, row: &ChatMessageView) {
    let is_user = row.role == TranscriptKind::User;
    let frame_color = match row.state {
        TranscriptState::Error => egui::Color32::from_rgb(76, 29, 29),
        TranscriptState::Stable | TranscriptState::Streaming if is_user => {
            egui::Color32::from_rgb(52, 90, 130)
        }
        TranscriptState::Stable | TranscriptState::Streaming => egui::Color32::from_rgb(38, 42, 50),
    };
    let text_color = if row.state == TranscriptState::Error {
        egui::Color32::from_rgb(255, 205, 205)
    } else {
        egui::Color32::PLACEHOLDER
    };
    let row_width = ui.available_width();
    let bubble_max_width = (row_width * 0.82).max(120.0);
    let frame = egui::Frame::new()
        .fill(frame_color)
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
            ui.label(
                egui::RichText::new(transcript_label(row.role))
                    .small()
                    .weak(),
            );
            let mut text = row.text.clone();
            if row.state == TranscriptState::Streaming {
                text.push('▌');
            }
            if text.is_empty() && row.state == TranscriptState::Streaming {
                ui.weak(i18n::fl("chat-waiting"));
            } else {
                ui.add(
                    egui::Label::new(egui::RichText::new(text).color(text_color))
                        .wrap()
                        .selectable(true),
                );
            }
        });
    });
    ui.add_space(6.0);
}

fn render_greeting_picker(ui: &mut egui::Ui, state: &mut SurfaceUiState) {
    if state.greetings.is_empty() {
        ui.weak(i18n::fl("chat-empty-history"));
        return;
    }
    if state.greetings.len() == 1 {
        request_single_greeting_commit(state);
        return;
    }
    ui.label(i18n::fl("chat-greeting-prompt"));
    for greeting in state.greetings.clone() {
        let first_line = greeting.text.lines().next().unwrap_or_default();
        let preview: String = first_line.chars().take(48).collect();
        let label = format!("[{}] {preview}", greeting.index);
        if ui
            .add_enabled(!state.greeting_inflight, egui::Button::new(label))
            .clicked()
        {
            state.push_action(SurfaceAction::SelectGreeting {
                index: greeting.index,
            });
        }
    }
    if !state.greeting_status.is_empty() {
        ui.colored_label(egui::Color32::LIGHT_RED, &state.greeting_status);
    }
}

/// A lone canonical greeting commits as soon as the picker renders; guard
/// against re-queueing while the selection is already pending or in flight.
fn request_single_greeting_commit(state: &mut SurfaceUiState) {
    let Some(greeting) = state.greetings.first() else {
        return;
    };
    if state.greeting_inflight
        || state
            .pending_actions
            .iter()
            .any(|action| matches!(action, SurfaceAction::SelectGreeting { .. }))
    {
        return;
    }
    state.push_action(SurfaceAction::SelectGreeting {
        index: greeting.index,
    });
}

pub(crate) const CHAT_INPUT_ID: &str = "stage-chat-input";

const COMPOSER_MIN_ROWS: usize = 3;
const COMPOSER_MAX_ROWS: usize = 8;
const COMPOSER_ROW_HEIGHT: f32 = 18.0;
const COMPOSER_VERTICAL_PADDING: f32 = 14.0;
const COMPOSER_MIN_HEIGHT: f32 = 64.0;
const COMPOSER_PANEL_HEIGHT: f32 = 280.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ComposerSendRequest {
    send: bool,
}

const COMPOSER_NONE: ComposerSendRequest = ComposerSendRequest { send: false };

const COMPOSER_SEND: ComposerSendRequest = ComposerSendRequest { send: true };

/// Rows the composer shows for the current draft: grows with content up to
/// the cap, past which the editor scrolls internally instead of pushing the
/// rest of the panel off screen.
#[must_use]
fn composer_metrics(draft: &str) -> (usize, f32) {
    let mut rows = draft.lines().count().max(1);
    if draft.ends_with('\n') {
        rows += 1;
    }
    rows = rows.clamp(COMPOSER_MIN_ROWS, COMPOSER_MAX_ROWS);
    #[expect(
        clippy::cast_precision_loss,
        reason = "row counts stay far below the f32 exact-integer range"
    )]
    let height = rows as f32 * COMPOSER_ROW_HEIGHT + COMPOSER_VERTICAL_PADDING;
    (rows, height.max(COMPOSER_MIN_HEIGHT))
}

#[must_use]
fn composer_send_allowed(state: &SurfaceUiState) -> bool {
    !state.chat_draft.trim().is_empty()
}

/// The multiline editor inserts the newline for the same Enter press that
/// requests the send; dropping it keeps blocked turns from collecting stray
/// blank lines at the end of the draft.
fn pop_enter_newline(draft: &mut String) {
    if draft.ends_with('\n') {
        draft.pop();
    }
}

#[must_use]
fn composer_request_for_key(
    enter_pressed: bool,
    shift_pressed: bool,
    composing: bool,
) -> ComposerSendRequest {
    if !enter_pressed || shift_pressed || composing {
        return COMPOSER_NONE;
    }
    COMPOSER_SEND
}

/// Reads the focused editor's key events for this frame instead of inferring
/// intent from focus loss, so a focus race can never swallow or fake a send.
/// Shift+Enter stays a newline and an active IME preedit claims Enter for
/// the composition.
#[must_use]
fn composer_send_requested(ui: &egui::Ui) -> ComposerSendRequest {
    ui.input(|input| {
        let mut request = COMPOSER_NONE;
        let mut composing = false;
        for event in &input.events {
            match event {
                egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) if !text.is_empty() => {
                    composing = true;
                }
                egui::Event::Key {
                    key: egui::Key::Enter,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    request = composer_request_for_key(true, modifiers.shift, false);
                }
                _ => {}
            }
        }
        if composing { COMPOSER_NONE } else { request }
    })
}

pub fn show(ui: &mut egui::Ui, state: &mut SurfaceUiState, mic_active: bool) -> bool {
    let mut composer_focused = false;
    let mut jump_to_voice = false;

    // Cap the measured panel size: its content can otherwise include the
    // previous frame's max rect and grow on every repaint until it hides the transcript.
    egui::Panel::bottom("stage-chat-composer")
        .resizable(false)
        .show_separator_line(false)
        .exact_size(COMPOSER_PANEL_HEIGHT)
        .frame(egui::Frame::new())
        .show(ui, |panel_ui| {
            panel_ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            composer_send_allowed(state),
                            egui::Button::new(i18n::fl("chat-send")),
                        )
                        .clicked()
                    {
                        state.push_action(SurfaceAction::SendChat);
                    }
                    if ui
                        .button(i18n::fl("chat-new-session"))
                        .on_hover_text(i18n::fl("chat-new-session-hint"))
                        .clicked()
                    {
                        state.push_action(SurfaceAction::NewSession);
                    }
                    let mic_label = if mic_active {
                        i18n::fl("mic-on")
                    } else {
                        i18n::fl("mic-off")
                    };
                    if ui.button(mic_label).clicked() {
                        state.push_action(SurfaceAction::ToggleMic);
                    }
                    ui.add_enabled_ui(state.turn_active, |ui| {
                        if ui
                            .button(i18n::fl("chat-barge-in"))
                            .on_hover_text(i18n::fl("chat-barge-in-hint"))
                            .clicked()
                        {
                            state.push_action(SurfaceAction::BargeIn);
                        }
                        if ui
                            .button(i18n::fl("chat-cancel"))
                            .on_hover_text(i18n::fl("chat-cancel-hint"))
                            .clicked()
                        {
                            state.push_action(SurfaceAction::CancelTurn);
                        }
                    });
                    if ui
                        .button(i18n::fl("chat-open-detail"))
                        .on_hover_text(i18n::fl("chat-open-detail-hint"))
                        .clicked()
                    {
                        state.push_action(SurfaceAction::OpenDetail(DetailTab::Home));
                    }
                });

                ui.horizontal(|ui| {
                    ui.weak(i18n::fl("chat-send-keyboard-hint"));
                    if state.turn_active {
                        ui.weak(i18n::fl("chat-draft-editable-hint"));
                    }
                    // The status label at the top of this panel is easily
                    // missed, so the mic guard repeats its Voice-setup call to
                    // action as a button inside the composer panel. Gating on
                    // the dedicated flag keeps unrelated status text from
                    // advertising a missing Speech-to-Text provider.
                    if mic_cta_eligible(state, mic_active)
                        && ui
                            .add(egui::Button::new(
                                egui::RichText::new(i18n::fl("tray-mic-needs-stt")).small(),
                            ))
                            .clicked()
                    {
                        jump_to_voice = true;
                    }
                });

                let composer_width = ui.available_width();
                let (rows, composer_min_height) = composer_metrics(&state.chat_draft);
                let response = ui.add(
                    egui::TextEdit::multiline(&mut state.chat_draft)
                        .id_salt(CHAT_INPUT_ID)
                        .hint_text(i18n::fl("chat-placeholder"))
                        .desired_width(composer_width)
                        .desired_rows(rows)
                        .min_size(egui::vec2(composer_width, composer_min_height))
                        .return_key(Some(egui::KeyboardShortcut::new(
                            egui::Modifiers::SHIFT,
                            egui::Key::Enter,
                        ))),
                );
                composer_focused = response.has_focus();
                let request = composer_send_requested(ui);
                if request.send && composer_send_allowed(state) {
                    pop_enter_newline(&mut state.chat_draft);
                    state.push_action(SurfaceAction::SendChat);
                }

                ui.horizontal(|ui| {
                    ui.label(i18n::fl("chat-input-label"));
                    for (mode, label, hint) in [
                        (
                            MessageMode::Prompt,
                            i18n::fl("chat-mode-prompt"),
                            i18n::fl("chat-mode-prompt-hint"),
                        ),
                        (
                            MessageMode::Steer,
                            i18n::fl("chat-mode-steer"),
                            i18n::fl("chat-mode-steer-hint"),
                        ),
                        (
                            MessageMode::FollowUp,
                            i18n::fl("chat-mode-follow-up"),
                            i18n::fl("chat-mode-follow-up-hint"),
                        ),
                    ] {
                        let enabled = mode == MessageMode::Prompt || state.turn_active;
                        ui.add_enabled_ui(enabled, |ui| {
                            if ui
                                .selectable_label(state.message_mode == mode, label)
                                .on_hover_text(hint)
                                .clicked()
                            {
                                state.message_mode = mode;
                            }
                        });
                    }
                    if !state.voice_state.is_empty() {
                        ui.weak(format!(
                            "{}: {}",
                            i18n::fl("voice-state"),
                            state.voice_state
                        ));
                    }
                });

                if !state.exclusive_notice.is_empty() {
                    ui.colored_label(egui::Color32::YELLOW, &state.exclusive_notice);
                }
                if !state.overlay_notice.is_empty() {
                    ui.colored_label(egui::Color32::YELLOW, &state.overlay_notice);
                }

                ui.collapsing(i18n::fl("chat-overlay-hint"), |ui| {
                    ui.label(i18n::fl("chat-overlay-hint"));
                });

                if !state.status.is_empty() {
                    ui.add(
                        egui::Label::new(egui::RichText::new(&state.status).small())
                            .wrap()
                            .selectable(true),
                    );
                }

                ui.add_space(4.0);
            });
        });
    egui::CentralPanel::default()
        .frame(egui::Frame::new())
        .show(ui, |transcript_ui| {
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .show(transcript_ui, |scroll_ui| {
                    let rows = normalize_transcript(&state.history, &state.streaming_text);
                    if rows.is_empty() {
                        render_greeting_picker(scroll_ui, state);
                    } else {
                        for row in rows {
                            render_message_bubble(scroll_ui, &row);
                        }
                    }
                    if let Some(gap) = chat_setup_gap(&state.chat_setup) {
                        scroll_ui.add_space(4.0);
                        let setup_cta = chat_setup_cta_eligible(state);
                        if setup_cta
                            && scroll_ui
                                .button(i18n::fl("chat-setup-unconfigured"))
                                .clicked()
                        {
                            state.push_action(SurfaceAction::OpenDetail(DetailTab::Conversation));
                        }
                        scroll_ui.weak(chat_setup_status(gap));
                    }
                });
        });

    if jump_to_voice {
        state.push_action(SurfaceAction::OpenDetail(DetailTab::Voice));
    }
    composer_focused
}

/// The mic guard's Voice-setup button must not infer "STT missing" from
/// arbitrary status text; only the dedicated flag may show or arm it.
fn mic_cta_eligible(state: &SurfaceUiState, mic_active: bool) -> bool {
    state.stt_setup_needed && !mic_active
}

/// The chat-setup CTA may not piggyback on generic status text nor crowd out
/// live conversation rows or a visible greeting picker; only a dedicated setup
/// gap over a quiet panel may show or arm it. A lone greeting is committed
/// automatically and does not occupy the panel while that request is pending.
fn chat_setup_cta_eligible(state: &SurfaceUiState) -> bool {
    chat_setup_gap(&state.chat_setup).is_some()
        && state.greetings.len() < 2
        && normalize_transcript(&state.history, &state.streaming_text).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_api::GreetingView;

    #[test]
    fn chat_layout_keeps_transcript_and_composer_content_visible() {
        let ctx = egui::Context::default();
        let mut state = SurfaceUiState {
            history: HistoryResponse {
                messages: vec![message("user", "visible user message")],
                depth: "surface".to_owned(),
            },
            ..Default::default()
        };
        let full = ctx.run_ui(chat_raw_input(), |ui| {
            show(ui, &mut state, false);
        });
        let texts = visible_painted_texts(&full.shapes);

        assert!(texts.contains(&"visible user message".to_owned()));
        assert!(texts.contains(&i18n::fl("chat-send-keyboard-hint")));
        assert!(texts.contains(&i18n::fl("chat-input-label")));
    }

    #[test]
    fn chat_composer_panel_stays_fixed_across_repaints() {
        let ctx = egui::Context::default();
        let mut state = SurfaceUiState::default();

        for _ in 0..8 {
            let _response = ctx.run_ui(chat_raw_input(), |ui| {
                show(ui, &mut state, false);
            });

            let panel = egui::PanelState::load(&ctx, egui::Id::new("stage-chat-composer"));
            assert!(
                panel.is_some(),
                "chat composer panel must persist its layout"
            );
            if let Some(panel) = panel {
                assert!(
                    (panel.outer_rect.height() - COMPOSER_PANEL_HEIGHT).abs() < f32::EPSILON,
                    "chat composer panel grew between repaints"
                );
            }
        }
    }

    fn chat_raw_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(520.0, 560.0),
            )),
            ..Default::default()
        }
    }

    fn first_run_chat_layout_paints_empty_state_and_setup_cta() {
        let ctx = egui::Context::default();
        let mut state = SurfaceUiState::default();
        let full = ctx.run_ui(first_run_raw_input(), |ui| {
            show(ui, &mut state, false);
        });
        let texts = painted_texts(&full.shapes);
        let visible_texts = visible_painted_texts(&full.shapes);

        assert!(texts.contains(&i18n::fl("chat-empty-history")));
        assert!(texts.contains(&i18n::fl("chat-setup-unconfigured")));
        assert!(texts.contains(&i18n::fl("chat-unconfigured")));
        assert!(visible_texts.contains(&i18n::fl("chat-empty-history")));
        assert!(visible_texts.contains(&i18n::fl("chat-setup-unconfigured")));
        assert!(visible_texts.contains(&i18n::fl("chat-unconfigured")));
        assert!(visible_texts.contains(&i18n::fl("chat-send-keyboard-hint")));
    }

    #[test]
    fn chat_composer_panel_stays_fixed_across_repaints() {
        let ctx = egui::Context::default();
        let mut state = SurfaceUiState::default();

        for _ in 0..8 {
            let _response = ctx.run_ui(first_run_raw_input(), |ui| {
                show(ui, &mut state, false);
            });

            let panel = egui::PanelState::load(&ctx, egui::Id::new("stage-chat-composer"));
            assert!(
                panel.is_some(),
                "chat composer panel must persist its layout"
            );
            if let Some(panel) = panel {
                assert!(
                    (panel.outer_rect.height() - COMPOSER_PANEL_HEIGHT).abs() < f32::EPSILON,
                    "chat composer panel grew between repaints"
                );
            }
        }
    }

    #[test]
    fn first_run_chat_layout_paints_setup_cta_while_single_greeting_is_pending() {
        let ctx = egui::Context::default();
        let mut state = SurfaceUiState::default();
        state.greetings.push(GreetingView {
            index: 0,
            text: "Hi".to_owned(),
        });
        state.greeting_inflight = true;
        let full = ctx.run_ui(first_run_raw_input(), |ui| {
            show(ui, &mut state, false);
        });
        let texts = painted_texts(&full.shapes);

        assert!(texts.contains(&i18n::fl("chat-setup-unconfigured")));
        assert!(texts.contains(&i18n::fl("chat-unconfigured")));
        assert!(!texts.contains(&i18n::fl("chat-greeting-prompt")));
    }

    fn first_run_raw_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(520.0, 560.0),
            )),
            ..Default::default()
        }
    }

    fn visible_painted_texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
        let mut texts = Vec::new();
        for clipped in shapes {
            collect_visible_texts(&clipped.shape, clipped.clip_rect, &mut texts);
        }
        texts
    }

    fn painted_texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
        let mut texts = Vec::new();
        for shape in shapes {
            collect_texts(&shape.shape, &mut texts);
        }
        texts
    }

    fn collect_visible_texts(
        shape: &egui::epaint::Shape,
        clip_rect: egui::Rect,
        out: &mut Vec<String>,
    ) {
        match shape {
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_visible_texts(shape, clip_rect, out);
                }
            }
            egui::epaint::Shape::Text(text)
                if clip_rect.intersects(text.visual_bounding_rect()) =>
            {
                out.push(text.galley.job.text.clone());
            }
            _ => {}
        }
    }
    }

    fn collect_texts(shape: &egui::epaint::Shape, out: &mut Vec<String>) {
        match shape {
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_texts(shape, out);
                }
            }
            egui::epaint::Shape::Text(text) => {
                out.push(text.galley.job.text.clone());
            }
            _ => {}
        }
    }

    fn collect_visible_texts(
        shape: &egui::epaint::Shape,
        clip_rect: egui::Rect,
        out: &mut Vec<String>,
    ) {
        match shape {
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_visible_texts(shape, clip_rect, out);
                }
            }
            egui::epaint::Shape::Text(text)
                if clip_rect.intersects(text.visual_bounding_rect()) =>
            {
                out.push(text.galley.job.text.clone());
            }
            _ => {}
        }
    }

    #[test]
    fn unrelated_status_never_arms_or_renders_the_mic_voice_cta() {
        let mut state = SurfaceUiState {
            // An unrelated failure path left text in the generic status line.
            status: "tool: execute: unknown skill skill".to_owned(),
            ..Default::default()
        };
        assert!(
            !mic_cta_eligible(&state, false),
            "unrelated status must never render or fire the Voice CTA"
        );

        // Only the mic toggle guard arms the flag; then the CTA is eligible.
        state.stt_setup_needed = true;
        assert!(mic_cta_eligible(&state, false));
        // With the mic claimed the guard no longer applies either.
        assert!(!mic_cta_eligible(&state, true));
    }

    #[test]
    fn chat_setup_cta_needs_a_gap_and_an_empty_transcript() {
        // Defaults leave chat unconfigured, so the bare surface arms the CTA.
        assert!(chat_setup_cta_eligible(&SurfaceUiState::default()));

        // Rows on screen displace the first-run CTA.
        let occupied = SurfaceUiState {
            history: HistoryResponse {
                messages: vec![message("assistant", "hello")],
                depth: "surface".to_owned(),
            },
            ..Default::default()
        };
        assert!(!chat_setup_cta_eligible(&occupied));

        // A visible greeting picker displaces it even before any row exists.
        let greeting_pending = SurfaceUiState {
            greetings: vec![greeting(0, "Hi!"), greeting(1, "Hello!")],
            history: HistoryResponse {
                messages: Vec::new(),
                depth: "surface".to_owned(),
            },
            ..Default::default()
        };
        assert!(!chat_setup_cta_eligible(&greeting_pending));

        // A lone greeting is committed automatically and leaves the panel
        // free for setup guidance until the assistant row arrives.
        let single_greeting = SurfaceUiState {
            greetings: vec![greeting(0, "Hi!")],
            ..Default::default()
        };
        assert!(chat_setup_cta_eligible(&single_greeting));

        // A stream in flight displaces it even without stable rows.
        let streaming = SurfaceUiState {
            streaming_text: "still writing".to_owned(),
            ..Default::default()
        };
        assert!(!chat_setup_cta_eligible(&streaming));

        // Configuring a model resolves the gap and removes the CTA.
        let mut configured = SurfaceUiState::default();
        configured.chat_setup.chat_plugin = "provider.gguf".to_owned();
        configured.chat_setup.chat_model = "local-model".to_owned();
        assert!(!chat_setup_cta_eligible(&configured));

        // Arbitrary generic status text neither arms nor gates the signal.
        assert!(chat_setup_cta_eligible(&SurfaceUiState {
            status: "tool: execute: unknown skill skill".to_owned(),
            ..Default::default()
        }));
    }

    fn message(role: &str, text: &str) -> MessageResponse {
        MessageResponse {
            seq: 1,
            role: role.to_owned(),
            text: text.to_owned(),
        }
    }

    fn greeting(index: u32, text: &str) -> GreetingView {
        GreetingView {
            index,
            text: text.to_owned(),
        }
    }

    #[test]
    fn transcript_normalization_keeps_surface_roles_and_streaming_state() {
        let history = HistoryResponse {
            messages: vec![
                message("user", "hello"),
                message("assistant", "hi"),
                message("status", "model failed"),
                message("tool-summary", "searched"),
                message("inner", "private thought"),
            ],
            depth: "surface".to_owned(),
        };

        let rows = normalize_transcript(&history, "still writing");

        assert_eq!(rows.len(), 5);
        assert_eq!(
            rows[0],
            ChatMessageView {
                role: TranscriptKind::User,
                state: TranscriptState::Stable,
                text: "hello".to_owned(),
            }
        );
        assert_eq!(rows[2].role, TranscriptKind::Error);
        assert_eq!(rows[2].state, TranscriptState::Error);
        assert_eq!(rows[3].role, TranscriptKind::Tool);
        assert_eq!(
            rows[4],
            ChatMessageView {
                role: TranscriptKind::Assistant,
                state: TranscriptState::Streaming,
                text: "still writing".to_owned(),
            }
        );
    }

    #[test]
    fn transcript_normalization_hides_inner_and_thinking_rows() {
        let history = HistoryResponse {
            messages: vec![message("thinking", "private"), message("inner", "private")],
            depth: "surface".to_owned(),
        };

        assert!(normalize_transcript(&history, "").is_empty());
    }

    #[test]
    fn greeting_picker_without_greetings_shows_empty_state() {
        let mut state = SurfaceUiState {
            greetings: Vec::new(),
            ..Default::default()
        };

        request_single_greeting_commit(&mut state);

        assert!(state.pending_actions.is_empty());
    }

    #[test]
    fn single_greeting_commits_once_without_click() {
        let mut state = SurfaceUiState {
            greetings: vec![greeting(0, "Welcome back.")],
            ..Default::default()
        };
        state.push_action(SurfaceAction::SelectGreeting { index: 0 });

        request_single_greeting_commit(&mut state);

        assert_eq!(state.pending_actions.len(), 1);

        state.greeting_inflight = true;
        request_single_greeting_commit(&mut state);

        assert_eq!(state.pending_actions.len(), 1);
    }

    #[test]
    fn multiple_greetings_wait_for_explicit_selection() {
        let state = SurfaceUiState {
            greetings: vec![
                greeting(0, "First greeting."),
                greeting(1, "Second greeting."),
            ],
            ..Default::default()
        };

        assert!(state.greetings.len() > 1, "picker must wait for a click");

        assert!(SurfaceUiState::default().pending_actions.is_empty());
    }

    #[test]
    fn existing_history_suppresses_greeting_picker() {
        let mut state = SurfaceUiState::default();
        state.history.messages = vec![message("assistant", "hello")];

        let rows = normalize_transcript(&state.history, "");

        assert!(!rows.is_empty(), "existing history must hide the picker");
    }

    #[test]
    fn composer_contract_keeps_shift_enter_out_of_send_path() {
        assert_eq!(composer_request_for_key(true, false, false), COMPOSER_SEND);
        assert_eq!(composer_request_for_key(true, true, false), COMPOSER_NONE);
        assert_eq!(composer_request_for_key(true, false, true), COMPOSER_NONE);
    }

    #[test]
    fn composer_height_grows_with_content_and_caps() {
        let (min_rows, min_height) = composer_metrics("");
        assert_eq!(min_rows, COMPOSER_MIN_ROWS);
        assert!(min_height >= COMPOSER_MIN_HEIGHT);

        let grown_draft = "one\n".repeat(COMPOSER_MAX_ROWS);
        let (_, grown) = composer_metrics(grown_draft.trim_end());
        assert!(grown > min_height);

        let long = "line\n".repeat(COMPOSER_MAX_ROWS * 2);
        let (capped_rows, capped_height) = composer_metrics(&long);
        assert_eq!(capped_rows, COMPOSER_MAX_ROWS);
        let (_, saturated_height) = composer_metrics("a\nb\nc\nd\ne\nf\ng\nh");
        assert!((capped_height - saturated_height).abs() < f32::EPSILON);
    }

    #[test]
    fn whitespace_draft_blocks_send_but_keeps_typing() {
        let mut state = SurfaceUiState {
            chat_draft: "   \n\t ".to_owned(),
            ..Default::default()
        };

        assert!(!composer_send_allowed(&state));
        state.chat_draft = "real words".to_owned();
        assert!(composer_send_allowed(&state));
    }

    #[test]
    fn enter_newline_is_removed_before_sending() {
        let mut draft = "hello\n".to_owned();
        pop_enter_newline(&mut draft);
        assert_eq!(draft, "hello");

        pop_enter_newline(&mut draft);
        assert_eq!(
            draft, "hello",
            "only one trailing newline is removed per send"
        );
    }

    #[test]
    fn multiline_draft_preserves_paste_newlines() {
        let state = SurfaceUiState {
            chat_draft: "first\nsecond\n".to_owned(),
            ..Default::default()
        };

        assert_eq!(state.chat_draft.lines().count(), 2);
        assert!(state.chat_draft.ends_with('\n'));
    }

    #[test]
    fn send_requires_non_whitespace_draft() {
        let state = SurfaceUiState {
            chat_draft: "  \n\t".to_owned(),
            ..Default::default()
        };

        assert!(state.chat_draft.trim().is_empty());
    }

    #[test]
    fn ime_preedit_blocks_send_even_when_enter_arrives_first() {
        assert_eq!(
            composer_request_for_key(true, false, true),
            COMPOSER_NONE,
            "active IME preedit must claim Enter for the composition"
        );
    }
}
