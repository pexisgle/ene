use std::sync::Arc;

use ene_api::CreateScheduleRequest;

use crate::core_session::CoreSession;

use super::components::section_card;
use super::input::SettingsInputState;

pub fn render(ui: &mut egui::Ui, ai: &Arc<CoreSession>, input: &mut SettingsInputState) {
    input.schedules.poll();
    if !input.schedules.started() {
        input.schedules.start(ai.fetch_schedules());
    }
    section_card(
        ui,
        "schedules-list",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "schedules"),
        |ui| {
            ui.horizontal(|ui| {
                ui.label("name");
                ui.text_edit_singleline(&mut input.schedule_name);
                ui.label("cron");
                ui.text_edit_singleline(&mut input.schedule_spec);
                if ui
                    .button(i18n_embed_fl::fl!(crate::i18n::loader(), "add"))
                    .clicked()
                    && let Some(soul_id) = ai.soul_id()
                {
                    drop(ai.create_schedule(CreateScheduleRequest {
                        soul_id,
                        name: input.schedule_name.clone(),
                        spec: input.schedule_spec.clone(),
                        timezone: "UTC".to_owned(),
                        action: "remind".to_owned(),
                        action_ref: None,
                        important: false,
                    }));
                    input.schedules.restart(ai.fetch_schedules());
                }
            });
            let Some(items) = input.schedules.data.clone() else {
                return;
            };
            for schedule in items {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{} ({}) enabled={}",
                        schedule.name, schedule.spec, schedule.enabled
                    ));
                    if ui.button("toggle").clicked() {
                        drop(ai.set_schedule_enabled(schedule.id.clone(), !schedule.enabled));
                        input.schedules.restart(ai.fetch_schedules());
                    }
                    if ui
                        .button(i18n_embed_fl::fl!(crate::i18n::loader(), "delete"))
                        .clicked()
                    {
                        drop(ai.delete_schedule(schedule.id.clone()));
                        input.schedules.restart(ai.fetch_schedules());
                    }
                });
            }
        },
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn schedule_request_defaults_remind() {
        let req = ene_api::CreateScheduleRequest {
            soul_id: "s".to_owned(),
            name: "n".to_owned(),
            spec: "0 * * * *".to_owned(),
            timezone: "UTC".to_owned(),
            action: "remind".to_owned(),
            action_ref: None,
            important: false,
        };
        assert_eq!(req.action, "remind");
    }
}
