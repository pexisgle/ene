//! Sidecar artifact cards used to live in the in-process plugin host.
//! Engines and models are owned by `ene-core`; this module keeps the
//! page layout hook without linking that host.
#![expect(
    dead_code,
    reason = "artifact-card layout stays for engine pages that still share this chrome"
)]
use std::sync::Arc;

use crate::core_session::CoreSession;
use crate::settings_ui::input::SettingsInputState;

fn fl(key: &str) -> String {
    crate::i18n::loader().get(key)
}

pub fn render_artifact_card(
    ui: &mut egui::Ui,
    _ai: &Arc<CoreSession>,
    _input: &mut SettingsInputState,
    _artifacts: &[()],
    artifact_id: &str,
) {
    ui.weak(format!("{}: {artifact_id}", fl("engines-core-hint")));
}
