//! Main companion overlay viewport.

mod approvals;
mod caption;
mod chat;
mod spotlight;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Ui, Vec2};
use ene_api::HistoryResponse;

use crate::avatar::VrmPane;
use crate::detail::DetailTab;
use crate::i18n;
use crate::settings::DesktopSettings;

pub use approvals::PendingApproval;
pub use spotlight::SpotlightAction;

#[derive(Debug, Clone)]
pub enum SurfaceAction {
    SendChat,
    BargeIn,
    CancelTurn,
    ToggleMic,
    Approval { decision: String },
    OpenDetail(DetailTab),
    Quit,
    PersistCharacterPos,
}

#[derive(Debug, Clone)]
pub struct SurfaceUiState {
    pub chat_draft: String,
    pub focus_chat: bool,
    pub history: HistoryResponse,
    pub streaming_text: String,
    pub caption: String,
    pub pending_approval: Option<PendingApproval>,
    pub spotlight_open: bool,
    pub status: String,
    pub quit: bool,
    pub character_pos: [f32; 2],
    pub dragging_character: bool,
    pub pending_actions: Vec<SurfaceAction>,
}

impl Default for SurfaceUiState {
    fn default() -> Self {
        Self {
            chat_draft: String::new(),
            focus_chat: false,
            history: HistoryResponse {
                messages: Vec::new(),
                depth: "surface".to_owned(),
            },
            streaming_text: String::new(),
            caption: String::new(),
            pending_approval: None,
            spotlight_open: false,
            status: i18n::fl("status-ready"),
            quit: false,
            character_pos: [0.7, 0.15],
            dragging_character: false,
            pending_actions: Vec::new(),
        }
    }
}

impl SurfaceUiState {
    fn push_action(&mut self, action: SurfaceAction) {
        self.pending_actions.push(action);
    }
}

pub fn show(ui: &mut Ui, state: &mut SurfaceUiState, settings: &DesktopSettings, vrm: Option<&VrmPane>, mic_active: bool) {
    state.character_pos[0] = settings.character_x;
    state.character_pos[1] = settings.character_y;
    let ctx = ui.ctx().clone();

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(Color32::from_rgba_unmultiplied(0, 0, 0, 0)))
        .show(ui, |ui| {
            let rect = ui.max_rect();
            paint_avatar(ui, rect, vrm, settings, state);
            chat::show(ui, state, mic_active);
        });

    if settings.caption_enabled {
        caption::show(&ctx, state, settings.caption_font_size);
    }

    if state.pending_approval.is_some() {
        approvals::show(&ctx, state);
    }

    if state.spotlight_open && settings.spotlight_enabled {
        if let Some(action) = spotlight::show(&ctx) {
            state.spotlight_open = false;
            match action {
                SpotlightAction::OpenDetail(tab) => state.push_action(SurfaceAction::OpenDetail(tab)),
                SpotlightAction::ToggleMic => state.push_action(SurfaceAction::ToggleMic),
                SpotlightAction::Quit => state.push_action(SurfaceAction::Quit),
                SpotlightAction::Close => {}
            }
        }
    }

    if state.focus_chat {
        state.focus_chat = false;
        ctx.memory_mut(|mem| mem.request_focus(egui::Id::new("stage-chat-input")));
    }
}

fn paint_avatar(
    ui: &mut Ui,
    full: Rect,
    vrm: Option<&VrmPane>,
    settings: &DesktopSettings,
    state: &mut SurfaceUiState,
) {
    let size = Vec2::new(full.width() * 0.45, full.height() * 0.65) * settings.model_scale;
    let center = Pos2::new(
        full.left() + full.width() * state.character_pos[0],
        full.top() + full.height() * state.character_pos[1],
    );
    let avatar_rect = Rect::from_center_size(center, size);

    let response = ui.allocate_rect(avatar_rect, Sense::click_and_drag());
    if response.dragged() {
        state.dragging_character = true;
        if let Some(pos) = response.interact_pointer_pos() {
            state.character_pos[0] = ((pos.x - full.left()) / full.width()).clamp(0.05, 0.95);
            state.character_pos[1] = ((pos.y - full.top()) / full.height()).clamp(0.05, 0.95);
        }
    }
    if state.dragging_character && response.drag_stopped() {
        state.dragging_character = false;
        state.push_action(SurfaceAction::PersistCharacterPos);
    }

    if let Some(vrm) = vrm {
        if let Some(info) = vrm.paint_info() {
            ui.put(
                avatar_rect,
                egui::Image::new((info.texture_id, Vec2::new(info.size[0], info.size[1])))
                    .fit_to_exact_size(avatar_rect.size()),
            );
        }
    } else {
        ui.painter().rect_stroke(
            avatar_rect,
            8.0,
            egui::Stroke::new(1.0, Color32::from_gray(120)),
            egui::StrokeKind::Inside,
        );
        ui.put(
            avatar_rect,
            egui::Label::new(i18n::fl("surface-title")).wrap(),
        );
    }
}
