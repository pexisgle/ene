//! The schedule list and run history are fetched asynchronously (never per
//! frame on the render thread); mutations go through the actor's validated
//! commands and refetch on completion. All labels are localized.

use crate::ai_bridge::AiBridge;

use super::components;
use super::input::{AsyncData, SettingsInputState};
use std::sync::Arc;

pub fn render(ui: &mut egui::Ui, ai: &Arc<AiBridge>, input: &mut SettingsInputState) {
    input.schedules.poll();
    if !input.schedules.started() {
        input.schedules.start(ai.fetch_schedules());
    }
    let schedules = input.schedules.data.clone().unwrap_or_default();
    input.pending_runs.poll();
    if !input.pending_runs.started() {
        input.pending_runs.start(ai.fetch_all_schedule_runs(10));
    }
    let mut delete: Option<i64> = None;
    let mut toggle: Option<(i64, bool)> = None;
    let mut confirm: Option<(i64, i64, bool)> = None;

    components::section_card(
        ui,
        "schedules-list",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "schedules-list-title"),
        |ui| {
            if input.schedules.loading() {
                ui.weak(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "schedules-loading"
                ));
                return;
            }
            if let Some(error) = input.schedules.error.clone() {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
                if ui
                    .small_button(i18n_embed_fl::fl!(crate::i18n::loader(), "schedules-retry"))
                    .clicked()
                {
                    input.schedules = AsyncData::new();
                }
                return;
            }
            if schedules.is_empty() {
                ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "schedules-empty"));
                return;
            }
            for schedule in &schedules {
                ui.horizontal(|ui| {
                    let mut enabled = schedule.enabled;
                    if ui.checkbox(&mut enabled, "").changed() {
                        toggle = Some((schedule.id, enabled));
                    }
                    ui.label(&schedule.name);
                    ui.weak(schedule.kind.as_str());
                    if let Some(next) = schedule.next_run_at {
                        ui.weak(next.format("%Y-%m-%d %H:%M").to_string());
                    } else {
                        ui.weak("—");
                    }
                    if ui
                        .small_button(i18n_embed_fl::fl!(crate::i18n::loader(), "schedules-edit"))
                        .clicked()
                    {
                        input.schedule_editing = Some(schedule.id);
                        load_schedule_into_form(ui, schedule);
                    }
                    if ui
                        .small_button(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "schedules-delete"
                        ))
                        .clicked()
                    {
                        delete = Some(schedule.id);
                    }
                });
                ui.weak(action_label(&schedule.action));
                if schedule.pending_retry_of_run_id.is_some() {
                    ui.colored_label(
                        egui::Color32::from_rgb(0xff, 0x98, 0x00),
                        i18n_embed_fl::fl!(crate::i18n::loader(), "schedules-pending-retry"),
                    );
                }
            }
        },
    );

    components::section_card(
        ui,
        "schedules-pending",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "schedules-pending-title"),
        |ui| {
            let mut pending = 0;
            let runs_by_schedule = input.pending_runs.data.clone().unwrap_or_default();
            for (schedule_id, runs) in &runs_by_schedule {
                for run in runs {
                    if run.status == ene_core::ScheduleRunStatus::AwaitingApproval {
                        pending += 1;
                        let schedule_name = schedules
                            .iter()
                            .find(|schedule| schedule.id == *schedule_id)
                            .map_or_else(
                                || schedule_id.to_string(),
                                |schedule| schedule.name.clone(),
                            );
                        ui.horizontal(|ui| {
                            ui.label(format!(
                                "{} · {}",
                                schedule_name,
                                run.scheduled_at.format("%Y-%m-%d %H:%M")
                            ));
                            if ui
                                .small_button(i18n_embed_fl::fl!(
                                    crate::i18n::loader(),
                                    "schedules-approve"
                                ))
                                .clicked()
                            {
                                confirm = Some((*schedule_id, run.id, true));
                            }
                            if ui
                                .small_button(i18n_embed_fl::fl!(
                                    crate::i18n::loader(),
                                    "schedules-deny"
                                ))
                                .clicked()
                            {
                                confirm = Some((*schedule_id, run.id, false));
                            }
                        });
                    }
                }
            }
            if input.pending_runs.loading() {
                ui.weak(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "schedules-loading"
                ));
            }
            if pending == 0 {
                ui.weak(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "schedules-no-pending"
                ));
            }
        },
    );

    let mut selected = ui.data_mut(|data| {
        data.get_temp::<i64>(egui::Id::new("schedules_selected"))
            .unwrap_or(-1)
    });
    input.schedule_runs.poll();
    components::section_card(
        ui,
        "schedules-history",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "schedules-history-title"),
        |ui| {
            for schedule in &schedules {
                if ui
                    .selectable_label(selected == schedule.id, &schedule.name)
                    .clicked()
                {
                    selected = schedule.id;
                    ui.data_mut(|data| {
                        data.insert_temp(egui::Id::new("schedules_selected"), selected);
                    });
                    input.schedule_runs = AsyncData::new();
                }
            }
            if selected > 0 {
                ui.separator();
                if !input.schedule_runs.started() {
                    input
                        .schedule_runs
                        .start(ai.fetch_schedule_runs(selected, 20));
                }
                if input.schedule_runs.loading() {
                    ui.weak(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "schedules-loading"
                    ));
                } else if let Some(runs) = &input.schedule_runs.data {
                    for run in runs {
                        ui.label(format!(
                            "{} · {} · {}",
                            run.scheduled_at.format("%Y-%m-%d %H:%M"),
                            run.status.as_str(),
                            run.finished_at
                                .map_or_else(|| "—".to_string(), |t| t.format("%H:%M").to_string())
                        ));
                    }
                }
            }
        },
    );

    // Queue click-driven mutations as async requests; the list refreshes
    // when each in-flight receiver resolves.
    if let Some((id, enabled)) = toggle
        && input.schedule_toggle_rx.is_none()
    {
        input.schedule_toggle_rx = Some(ai.apply_schedule_enabled(id, enabled));
    }
    if let Some(id) = delete
        && input.schedule_delete_rx.is_none()
    {
        input.schedule_delete_rx = Some(ai.apply_schedule_delete(id));
    }
    if let Some((schedule_id, run_id, approve)) = confirm
        && input.schedule_confirm_rx.is_none()
    {
        input.schedule_confirm_rx =
            Some(ai.apply_schedule_confirmation(schedule_id, run_id, approve));
    }
    if let Some(receiver) = &mut input.schedule_toggle_rx
        && let Ok(result) = receiver.try_recv()
    {
        input.schedule_toggle_rx = None;
        if let Err(e) = result {
            tracing::warn!(component = "SchedulesPage", error = %e, "failed to toggle schedule");
        }
        input.schedules = AsyncData::new();
    }
    if let Some(receiver) = &mut input.schedule_delete_rx
        && let Ok(result) = receiver.try_recv()
    {
        input.schedule_delete_rx = None;
        if let Err(e) = result {
            tracing::warn!(component = "SchedulesPage", error = %e, "failed to delete schedule");
        }
        input.schedules = AsyncData::new();
    }
    if let Some(receiver) = &mut input.schedule_confirm_rx
        && let Ok(result) = receiver.try_recv()
    {
        input.schedule_confirm_rx = None;
        if let Err(e) = result {
            tracing::warn!(
                component = "SchedulesPage",
                error = %e,
                "failed to resolve schedule confirmation"
            );
        }
        input.schedule_runs = AsyncData::new();
        input.pending_runs = AsyncData::new();
    }
    if let Some(receiver) = &mut input.schedule_add_rx
        && let Ok(result) = receiver.try_recv()
    {
        input.schedule_add_rx = None;
        match result {
            Ok(_) => {
                input.schedules = AsyncData::new();
            }
            Err(e) => {
                ui.ctx().data_mut(|data| {
                    data.insert_temp(egui::Id::new("schedules_add_error"), e.to_string());
                });
            }
        }
    }
    if let Some(receiver) = &mut input.schedule_update_rx
        && let Ok(result) = receiver.try_recv()
    {
        input.schedule_update_rx = None;
        input.schedule_editing = None;
        match result {
            Ok(_) => {
                input.schedules = AsyncData::new();
            }
            Err(e) => {
                ui.ctx().data_mut(|data| {
                    data.insert_temp(egui::Id::new("schedules_add_error"), e);
                });
            }
        }
    }
    render_add_form(ui, ai, input);
}

fn load_schedule_into_form(ui: &mut egui::Ui, schedule: &ene_core::Schedule) {
    ui.data_mut(|data| {
        data.insert_temp(egui::Id::new("schedules_add_name"), schedule.name.clone());
        data.insert_temp(
            egui::Id::new("schedules_add_kind"),
            schedule.kind.as_str().to_string(),
        );
        data.insert_temp(
            egui::Id::new("schedules_add_interval"),
            schedule
                .interval_secs
                .map_or_else(|| "3600".to_string(), |secs| secs.to_string()),
        );
        data.insert_temp(
            egui::Id::new("schedules_add_cron"),
            schedule.cron_expr.clone().unwrap_or_default(),
        );
        data.insert_temp(
            egui::Id::new("schedules_add_timezone"),
            schedule.timezone.clone(),
        );
        match &schedule.action {
            ene_core::ScheduleAction::Tool { name, arguments } => {
                data.insert_temp(egui::Id::new("schedules_add_is_prompt"), false);
                data.insert_temp(egui::Id::new("schedules_add_prompt"), String::new());
                data.insert_temp(egui::Id::new("schedules_add_tool"), name.clone());
                data.insert_temp(
                    egui::Id::new("schedules_add_arguments"),
                    serde_json::to_string_pretty(arguments).unwrap_or_else(|_| "{}".to_string()),
                );
            }
            ene_core::ScheduleAction::Prompt { text, allow_tools } => {
                data.insert_temp(egui::Id::new("schedules_add_is_prompt"), true);
                data.insert_temp(egui::Id::new("schedules_add_prompt"), text.clone());
                data.insert_temp(egui::Id::new("schedules_add_allow_tools"), *allow_tools);
                data.insert_temp(egui::Id::new("schedules_add_tool"), String::new());
                data.insert_temp(egui::Id::new("schedules_add_arguments"), "{}".to_string());
            }
        }
        data.insert_temp(
            egui::Id::new("schedules_add_confirmation"),
            schedule.confirmation == ene_core::ScheduleConfirmation::Confirm,
        );
        data.insert_temp(egui::Id::new("schedules_add_error"), String::new());
    });
}

fn render_add_form(ui: &mut egui::Ui, ai: &Arc<AiBridge>, input: &mut SettingsInputState) {
    components::section_card(
        ui,
        "schedules-add",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "schedules-add-title"),
        |ui| {
            let mut name = ui.data_mut(|data| {
                data.get_temp::<String>(egui::Id::new("schedules_add_name"))
                    .unwrap_or_default()
            });
            let mut kind = ui.data_mut(|data| {
                data.get_temp::<String>(egui::Id::new("schedules_add_kind"))
                    .unwrap_or_else(|| "interval".to_string())
            });
            let mut interval = ui.data_mut(|data| {
                data.get_temp::<String>(egui::Id::new("schedules_add_interval"))
                    .unwrap_or_else(|| "3600".to_string())
            });
            let mut cron = ui.data_mut(|data| {
                data.get_temp::<String>(egui::Id::new("schedules_add_cron"))
                    .unwrap_or_default()
            });
            let mut tool = ui.data_mut(|data| {
                data.get_temp::<String>(egui::Id::new("schedules_add_tool"))
                    .unwrap_or_default()
            });
            let mut prompt = ui.data_mut(|data| {
                data.get_temp::<String>(egui::Id::new("schedules_add_prompt"))
                    .unwrap_or_default()
            });
            let prompt_mode = ui.data_mut(|data| {
                data.get_temp::<bool>(egui::Id::new("schedules_add_is_prompt"))
                    .unwrap_or(false)
            });
            let mut allow_tools = ui.data_mut(|data| {
                data.get_temp::<bool>(egui::Id::new("schedules_add_allow_tools"))
                    .unwrap_or(false)
            });
            let mut arguments = ui.data_mut(|data| {
                data.get_temp::<String>(egui::Id::new("schedules_add_arguments"))
                    .unwrap_or_else(|| "{}".to_string())
            });
            let mut timezone = ui.data_mut(|data| {
                data.get_temp::<String>(egui::Id::new("schedules_add_timezone"))
                    .unwrap_or_else(local_timezone_name)
            });
            let mut confirmation = ui.data_mut(|data| {
                data.get_temp::<bool>(egui::Id::new("schedules_add_confirmation"))
                    .unwrap_or(false)
            });
            let mut error: Option<String> = ui.data_mut(|data| {
                data.get_temp::<String>(egui::Id::new("schedules_add_error"))
                    .filter(|value| !value.is_empty())
            });

            ui.horizontal(|ui| {
                ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "schedules-name"));
                if ui
                    .add(egui::TextEdit::singleline(&mut name).desired_width(160.0))
                    .changed()
                {
                    ui.data_mut(|data| {
                        data.insert_temp(egui::Id::new("schedules_add_name"), name.clone());
                    });
                }
            });
            ui.horizontal(|ui| {
                ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "schedules-kind"));
                egui::ComboBox::from_id_salt("schedules_add_kind_combo")
                    .selected_text(kind.as_str())
                    .show_ui(ui, |ui| {
                        for candidate in ["interval", "cron", "one_shot", "startup"] {
                            if ui.selectable_label(kind == candidate, candidate).clicked() {
                                kind = candidate.to_string();
                                ui.data_mut(|data| {
                                    data.insert_temp(
                                        egui::Id::new("schedules_add_kind"),
                                        kind.clone(),
                                    );
                                });
                            }
                        }
                    });
            });
            match kind.as_str() {
                "cron" => {
                    ui.horizontal(|ui| {
                        ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "schedules-cron"));
                        if ui
                            .add(egui::TextEdit::singleline(&mut cron).desired_width(200.0))
                            .changed()
                        {
                            ui.data_mut(|data| {
                                data.insert_temp(egui::Id::new("schedules_add_cron"), cron.clone());
                            });
                        }
                    });
                }
                "interval" => {
                    ui.horizontal(|ui| {
                        ui.label(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "schedules-interval"
                        ));
                        if ui
                            .add(egui::TextEdit::singleline(&mut interval).desired_width(120.0))
                            .changed()
                        {
                            ui.data_mut(|data| {
                                data.insert_temp(
                                    egui::Id::new("schedules_add_interval"),
                                    interval.clone(),
                                );
                            });
                        }
                    });
                }
                _ => {}
            }
            ui.horizontal(|ui| {
                ui.label(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "schedules-timezone"
                ));
                if ui
                    .add(egui::TextEdit::singleline(&mut timezone).desired_width(160.0))
                    .changed()
                {
                    ui.data_mut(|data| {
                        data.insert_temp(egui::Id::new("schedules_add_timezone"), timezone.clone());
                    });
                }
            });
            if prompt_mode {
                ui.label(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "schedules-prompt"
                ));
                if ui
                    .add(
                        egui::TextEdit::multiline(&mut prompt)
                            .desired_rows(4)
                            .desired_width(280.0),
                    )
                    .changed()
                {
                    ui.data_mut(|data| {
                        data.insert_temp(egui::Id::new("schedules_add_prompt"), prompt.clone());
                    });
                }
                if ui
                    .checkbox(
                        &mut allow_tools,
                        i18n_embed_fl::fl!(crate::i18n::loader(), "schedules-allow-tools"),
                    )
                    .changed()
                {
                    ui.data_mut(|data| {
                        data.insert_temp(egui::Id::new("schedules_add_allow_tools"), allow_tools);
                    });
                }
            } else {
                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "schedules-tool"));
                    if ui
                        .add(egui::TextEdit::singleline(&mut tool).desired_width(200.0))
                        .changed()
                    {
                        ui.data_mut(|data| {
                            data.insert_temp(egui::Id::new("schedules_add_tool"), tool.clone());
                        });
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "schedules-arguments"
                    ));
                    if ui
                        .add(egui::TextEdit::singleline(&mut arguments).desired_width(240.0))
                        .changed()
                    {
                        ui.data_mut(|data| {
                            data.insert_temp(
                                egui::Id::new("schedules_add_arguments"),
                                arguments.clone(),
                            );
                        });
                    }
                });
            }
            if ui
                .checkbox(
                    &mut confirmation,
                    i18n_embed_fl::fl!(crate::i18n::loader(), "schedules-confirmation"),
                )
                .changed()
            {
                ui.data_mut(|data| {
                    data.insert_temp(egui::Id::new("schedules_add_confirmation"), confirmation);
                });
            }

            ui.horizontal(|ui| {
                if input.schedule_editing.is_some()
                    && ui
                        .small_button(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "schedules-edit-cancel"
                        ))
                        .clicked()
                {
                    input.schedule_editing = None;
                    ui.data_mut(|data| {
                        data.insert_temp(egui::Id::new("schedules_add_name"), String::new());
                        data.insert_temp(egui::Id::new("schedules_add_error"), String::new());
                    });
                }
                let submit_label = if input.schedule_editing.is_some() {
                    i18n_embed_fl::fl!(crate::i18n::loader(), "schedules-update")
                } else {
                    i18n_embed_fl::fl!(crate::i18n::loader(), "schedules-add")
                };
                if ui
                    .add_enabled(
                        input.schedule_add_rx.is_none() && input.schedule_update_rx.is_none(),
                        egui::Button::new(submit_label),
                    )
                    .on_hover_text(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "schedules-add-hint"
                    ))
                    .clicked()
                {
                    match parse_schedule_input(
                        &name,
                        &kind,
                        &cron,
                        &interval,
                        &arguments,
                        &tool,
                        &prompt,
                        prompt_mode,
                        allow_tools,
                        &timezone,
                        confirmation,
                    ) {
                        Ok(new) => {
                            if let Some(id) = input.schedule_editing {
                                input.schedule_update_rx = Some(ai.apply_schedule_update(id, new));
                            } else {
                                input.schedule_add_rx = Some(ai.apply_schedule_add(new));
                            }
                            ui.data_mut(|data| {
                                data.insert_temp(
                                    egui::Id::new("schedules_add_name"),
                                    String::new(),
                                );
                                data.insert_temp(
                                    egui::Id::new("schedules_add_error"),
                                    String::new(),
                                );
                            });
                            error = None;
                        }
                        Err(message) => {
                            error = Some(message);
                            ui.data_mut(|data| {
                                data.insert_temp(
                                    egui::Id::new("schedules_add_error"),
                                    error.clone().unwrap_or_default(),
                                );
                            });
                        }
                    }
                }
            });
            if let Some(error) = error {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
        },
    );
}

fn action_label(action: &ene_core::ScheduleAction) -> String {
    match action {
        ene_core::ScheduleAction::Tool { name, .. } => {
            format!(
                "{}: {name}",
                i18n_embed_fl::fl!(crate::i18n::loader(), "schedules-tool")
            )
        }
        ene_core::ScheduleAction::Prompt { text, .. } => {
            let preview: String = text.chars().take(60).collect();
            format!(
                "{}: {preview}",
                i18n_embed_fl::fl!(crate::i18n::loader(), "schedules-prompt")
            )
        }
    }
}

/// Nothing is silently coerced: invalid JSON, empty names, unknown kinds,
/// non-positive intervals, and missing cron expressions are rejected with a
/// message.
fn parse_schedule_input(
    name: &str,
    kind: &str,
    cron: &str,
    interval: &str,
    arguments: &str,
    tool: &str,
    prompt: &str,
    prompt_mode: bool,
    allow_tools: bool,
    timezone: &str,
    confirmation: bool,
) -> Result<ene_core::NewSchedule, String> {
    if name.trim().is_empty() {
        return Err(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "schedules-empty-name"
        ));
    }
    let action = if prompt_mode {
        if prompt.trim().is_empty() {
            return Err(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "schedules-prompt-required"
            ));
        }
        ene_core::ScheduleAction::Prompt {
            text: prompt.trim().to_string(),
            allow_tools,
        }
    } else {
        if tool.trim().is_empty() {
            return Err(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "schedules-tool-required"
            ));
        }
        ene_core::ScheduleAction::Tool {
            name: tool.trim().to_string(),
            arguments: serde_json::from_str(arguments.trim()).map_err(|e| {
                format!(
                    "{}: {e}",
                    i18n_embed_fl::fl!(crate::i18n::loader(), "schedules-invalid-json")
                )
            })?,
        }
    };
    let interval_secs = if kind == "interval" {
        match interval.trim().parse::<i64>() {
            Ok(value) if value > 0 => Some(value),
            _ => {
                return Err(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "schedules-invalid-interval"
                ));
            }
        }
    } else {
        None
    };
    let (schedule_kind, cron_expr, start_at) = match kind {
        "cron" => {
            if cron.trim().is_empty() {
                return Err(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "schedules-cron-required"
                ));
            }
            (
                ene_core::ScheduleKind::Cron,
                Some(cron.trim().to_string()),
                None,
            )
        }
        "one_shot" => (
            ene_core::ScheduleKind::OneShot,
            None,
            Some(chrono::Utc::now() + chrono::Duration::minutes(1)),
        ),
        "startup" => (ene_core::ScheduleKind::Startup, None, None),
        "interval" => (
            ene_core::ScheduleKind::Interval,
            None,
            Some(chrono::Utc::now()),
        ),
        other => {
            return Err(format!(
                "{}: {other}",
                i18n_embed_fl::fl!(crate::i18n::loader(), "schedules-invalid-kind")
            ));
        }
    };
    let resolved_timezone = if timezone.trim().is_empty() {
        local_timezone_name()
    } else {
        timezone.trim().to_string()
    };
    Ok(ene_core::NewSchedule {
        name: name.trim().to_string(),
        kind: schedule_kind,
        timezone: resolved_timezone,
        cron_expr,
        interval_secs,
        start_at,
        action,
        confirmation: if confirmation {
            ene_core::ScheduleConfirmation::Confirm
        } else {
            ene_core::ScheduleConfirmation::None
        },
        max_retries: 0,
        retry_delay_secs: 0,
    })
}

/// IANA timezone of the host: `TZ` env, then `/etc/timezone`, else `UTC`.
fn local_timezone_name() -> String {
    if let Ok(tz) = std::env::var("TZ")
        && !tz.trim().is_empty()
    {
        return tz;
    }
    if let Ok(contents) = std::fs::read_to_string("/etc/timezone") {
        let zone = contents.trim();
        if !zone.is_empty() {
            return zone.to_string();
        }
    }
    "UTC".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_input() -> (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        bool,
    ) {
        (
            "daily", "interval", "", "3600", "{}", "fs.read", "UTC", false,
        )
    }

    #[test]
    fn invalid_json_is_rejected_not_coerced() {
        let (name, kind, cron, interval, _args, tool, tz, confirm) = valid_input();
        let result = parse_schedule_input(
            name,
            kind,
            cron,
            interval,
            "{not json",
            tool,
            "",
            false,
            false,
            tz,
            confirm,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("JSON"));
    }

    #[test]
    fn empty_name_is_rejected() {
        let (_, kind, cron, interval, args, tool, tz, confirm) = valid_input();
        assert!(
            parse_schedule_input(
                "   ", kind, cron, interval, args, tool, "", false, false, tz, confirm
            )
            .is_err()
        );
    }

    #[test]
    fn non_positive_interval_is_rejected() {
        let (name, _, _, _, args, tool, tz, confirm) = valid_input();
        assert!(
            parse_schedule_input(
                name, "interval", "", "0", args, tool, "", false, false, tz, confirm
            )
            .is_err()
        );
        assert!(
            parse_schedule_input(
                name, "interval", "", "-5", args, tool, "", false, false, tz, confirm
            )
            .is_err()
        );
    }

    #[test]
    fn unknown_kind_and_missing_cron_are_rejected() {
        let (name, _, _, _, args, tool, tz, confirm) = valid_input();
        assert!(
            parse_schedule_input(
                name, "weekly", "", "3600", args, tool, "", false, false, tz, confirm
            )
            .is_err()
        );
        assert!(
            parse_schedule_input(
                name, "cron", "", "", args, tool, "", false, false, tz, confirm
            )
            .is_err()
        );
        assert!(
            parse_schedule_input(
                name,
                "cron",
                "* * * * *",
                "",
                args,
                tool,
                "",
                false,
                false,
                tz,
                confirm
            )
            .is_ok()
        );
    }

    #[test]
    fn valid_interval_input_parses_with_local_timezone_fallback() {
        let (name, _, _, _, args, tool, _, confirm) = valid_input();
        let schedule = parse_schedule_input(
            name, "interval", "", "60", args, tool, "", false, false, "", confirm,
        )
        .expect("valid input");
        assert_eq!(schedule.name, "daily");
        assert_eq!(schedule.kind, ene_core::ScheduleKind::Interval);
        assert_eq!(schedule.interval_secs, Some(60));
        assert!(
            !schedule.timezone.is_empty(),
            "empty timezone falls back to the local IANA zone"
        );
    }

    #[test]
    fn prompt_mode_requires_text_and_preserves_allow_tools() {
        let (name, _, _, _, args, _tool, tz, confirm) = valid_input();
        let rejected = parse_schedule_input(
            name, "interval", "", "60", args, "", "   ", true, true, tz, confirm,
        );
        assert!(rejected.is_err(), "an empty prompt must be rejected");

        let schedule = parse_schedule_input(
            name,
            "interval",
            "",
            "60",
            args,
            "",
            "summarize the day",
            true,
            true,
            tz,
            confirm,
        )
        .expect("valid prompt input");
        assert_eq!(
            schedule.action,
            ene_core::ScheduleAction::Prompt {
                text: "summarize the day".to_string(),
                allow_tools: true,
            }
        );
    }
}
