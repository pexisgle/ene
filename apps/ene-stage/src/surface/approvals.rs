//! Tool approval modal.

use crate::i18n;
use crate::surface::{SurfaceAction, SurfaceUiState};

pub fn show(ctx: &egui::Context, state: &mut SurfaceUiState) {
    let Some(pending) = state.pending_approval.clone() else {
        return;
    };
    egui::Window::new(i18n::fl("approval-title"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(format!("{}: {}", i18n::fl("approval-tool"), pending.tool));
            ui.label(format!(
                "{}: {}",
                i18n::fl("approval-target"),
                pending.target
            ));
            let pending_id = pending.id.clone();
            ui.horizontal(|ui| {
                if ui.button(i18n::fl("approval-allow")).clicked() {
                    state.push_action(SurfaceAction::Approval {
                        id: pending_id.clone(),
                        decision: "allow".to_owned(),
                    });
                }
                if ui.button(i18n::fl("approval-always")).clicked() {
                    state.push_action(SurfaceAction::Approval {
                        id: pending.id.clone(),
                        decision: "allow_and_remember".to_owned(),
                    });
                }
                if ui.button(i18n::fl("approval-deny")).clicked() {
                    state.push_action(SurfaceAction::Approval {
                        id: pending.id.clone(),
                        decision: "deny".to_owned(),
                    });
                }
            });
        });
}

#[derive(Debug, Clone)]
pub struct PendingApproval {
    pub id: String,
    pub tool: String,
    pub target: String,
}
