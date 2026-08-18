use super::components::section_card;
use super::draft::SettingsDraft;

pub fn render(ui: &mut egui::Ui, draft: &mut SettingsDraft) {
    ui.weak(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "features-core-hint"
    ));
    ui.add_space(6.0);
    section_card(
        ui,
        "features-mind",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "features-mind"),
        |ui| {
            render_json_section(ui, draft, "mind");
            ui.separator();
            render_json_section(ui, draft, "body");
        },
    );
}

fn render_json_section(ui: &mut egui::Ui, draft: &mut SettingsDraft, key: &str) {
    ui.label(key);
    let mut text = draft
        .editing()
        .section_value(key)
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| "{}".to_owned());
    if ui
        .add(egui::TextEdit::multiline(&mut text).desired_width(f32::INFINITY))
        .changed()
        && let Ok(parsed) = serde_json::from_str(&text)
    {
        draft.set_section_value(key, parsed);
    }
}
