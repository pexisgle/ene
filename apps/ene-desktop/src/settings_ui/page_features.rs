//! Provider, embedding, voice, and tool settings live on their own pages;
//! this page owns the mind switches and proactive timing / source policy.

use crate::settings_ui::components::{
    BadgeTone, section_card, setting_row, status_badge, toggle_row, warning_box,
};
use crate::settings_ui::draft::SettingsDraft;
use crate::settings_ui::widgets::editable_combo;
use ene_mind::WindowTitleLevel;

pub fn render(ui: &mut egui::Ui, draft: &mut SettingsDraft) {
    ui.vertical(|ui| {
        ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "features-hint"));
        ui.add_space(6.0);
        section_card(
            ui,
            "features-mind",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "features-mind"),
            |ui| render_mind(ui, draft),
        );
    });
}

fn render_mind(ui: &mut egui::Ui, draft: &mut SettingsDraft) {
    let mut memory = draft.section::<ene_store::StoreConfig>();
    let mut mind = draft.section::<ene_mind::MindConfig>();

    let mut memory_enabled = memory.enabled;
    if toggle_row(
        ui,
        "features_memory",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "enable-long-term-memory"),
        "",
        &mut memory_enabled,
    ) {
        memory.enabled = memory_enabled;
        draft.set_section(&memory);
    }

    let mut emotion_enabled = mind.emotion.enabled;
    if toggle_row(
        ui,
        "features_emotion",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "enable-emotion"),
        "",
        &mut emotion_enabled,
    ) {
        mind.emotion.enabled = emotion_enabled;
        persist_mind(draft, &mind);
    }

    ui.add_space(6.0);
    ui.label(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "proactive-speech"
    ));
    ui.add_space(2.0);

    let mut proactive_enabled = mind.proactive.enabled;
    if toggle_row(
        ui,
        "features_proactive",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-enabled"),
        "",
        &mut proactive_enabled,
    ) {
        mind.proactive.enabled = proactive_enabled;
        persist_mind(draft, &mind);
    }

    ui.add_enabled_ui(mind.proactive.enabled, |ui| {
        let mut paused = mind.proactive.paused;
        if toggle_row(
            ui,
            "features_proactive_pause",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-pause"),
            &i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-pause-hint"),
            &mut paused,
        ) {
            mind.proactive.paused = paused;
            persist_mind(draft, &mind);
        }

        setting_row(
            ui,
            "proactive_interval_row",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-interval"),
            "",
            |ui| {
                let mut value = mind.proactive.interval_seconds as i32;
                if ui
                    .add(egui::DragValue::new(&mut value).range(1..=3600))
                    .changed()
                {
                    mind.proactive.interval_seconds = value.max(1) as u64;
                    persist_mind(draft, &mind);
                }
            },
        );

        setting_row(
            ui,
            "proactive_cooldown_row",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-cooldown"),
            "",
            |ui| {
                let mut value = mind.proactive.cooldown_seconds as i32;
                if ui
                    .add(egui::DragValue::new(&mut value).range(0..=86_400))
                    .changed()
                {
                    mind.proactive.cooldown_seconds = value.max(0) as u64;
                    persist_mind(draft, &mind);
                }
            },
        );

        setting_row(
            ui,
            "proactive_min_idle_row",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-min-idle"),
            "",
            |ui| {
                let mut value = mind.proactive.min_idle_seconds as i32;
                if ui
                    .add(egui::DragValue::new(&mut value).range(0..=86_400))
                    .changed()
                {
                    mind.proactive.min_idle_seconds = value.max(0) as u64;
                    persist_mind(draft, &mind);
                }
            },
        );

        setting_row(
            ui,
            "proactive_fatigue_row",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-fatigue-threshold"),
            &i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-fatigue-threshold-hint"),
            |ui| {
                let mut value = mind.proactive.fatigue_suppression_threshold;
                if ui
                    .add(
                        egui::DragValue::new(&mut value)
                            .range(0.0..=1.0)
                            .speed(0.01),
                    )
                    .changed()
                {
                    mind.proactive.fatigue_suppression_threshold = value.clamp(0.0, 1.0);
                    persist_mind(draft, &mind);
                }
            },
        );

        ui.add_space(6.0);
        ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "quiet-hours"));
        ui.add_space(2.0);
        let mut quiet_enabled = mind.proactive.quiet_hours.enabled;
        if toggle_row(
            ui,
            "features_quiet_hours",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "quiet-hours-enabled"),
            "",
            &mut quiet_enabled,
        ) {
            mind.proactive.quiet_hours.enabled = quiet_enabled;
            persist_mind(draft, &mind);
        }

        ui.add_enabled_ui(quiet_enabled, |ui| {
            // Probe with `enabled` forced on so timezone validity is reported
            // while the block is being edited, not only after it is saved.
            let mut probe = mind.proactive.quiet_hours.clone();
            probe.enabled = true;
            let eval = ene_mind::evaluate_quiet_hours(&probe, chrono::Utc::now());

            let status = if mind.proactive.paused {
                Some((
                    i18n_embed_fl::fl!(crate::i18n::loader(), "quiet-hours-paused-override"),
                    BadgeTone::Warn,
                ))
            } else if eval.active {
                Some((
                    format!(
                        "{} ({})",
                        i18n_embed_fl::fl!(crate::i18n::loader(), "quiet-hours-active"),
                        eval.local_time
                    ),
                    BadgeTone::Warn,
                ))
            } else {
                None
            };
            if let Some((text, tone)) = status {
                status_badge(ui, &text, tone);
            } else {
                ui.weak(format!(
                    "{} ({})",
                    i18n_embed_fl::fl!(crate::i18n::loader(), "quiet-hours-inactive"),
                    eval.local_time
                ));
            }

            setting_row(
                ui,
                "quiet_hours_timezone_row",
                &i18n_embed_fl::fl!(crate::i18n::loader(), "quiet-hours-timezone"),
                "",
                |ui| {
                    let mut timezone = mind.proactive.quiet_hours.timezone.clone();
                    let mut choices: Vec<(String, String)> = vec![(
                        String::new(),
                        i18n_embed_fl::fl!(crate::i18n::loader(), "quiet-hours-timezone-system"),
                    )];
                    let mut zones: Vec<String> = chrono_tz::TZ_VARIANTS
                        .iter()
                        .map(|zone| zone.name().to_string())
                        .collect();
                    zones.sort();
                    choices.extend(zones.into_iter().map(|name| (name.clone(), name.clone())));
                    if !timezone.is_empty() && !choices.iter().any(|(value, _)| value == &timezone)
                    {
                        choices.insert(0, (timezone.clone(), timezone.clone()));
                    }
                    let combo = editable_combo(
                        ui,
                        "quiet_hours_timezone_combo",
                        &mut timezone,
                        &choices,
                        180.0,
                    );
                    if combo.commit_requested() {
                        mind.proactive.quiet_hours.timezone = timezone.trim().to_string();
                        persist_mind(draft, &mind);
                    }
                },
            );
            if !eval.timezone_valid {
                warning_box(
                    ui,
                    &i18n_embed_fl::fl!(crate::i18n::loader(), "quiet-hours-timezone-invalid"),
                );
            }
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "quiet-hours-timezone-hint"
            ));

            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "quiet-hours-days"
            ));
            let mut days_changed = false;
            ui.horizontal_wrapped(|ui| {
                let days = &mut mind.proactive.quiet_hours.days;
                for (field, key) in [
                    (&mut days.monday, "quiet-hours-day-monday"),
                    (&mut days.tuesday, "quiet-hours-day-tuesday"),
                    (&mut days.wednesday, "quiet-hours-day-wednesday"),
                    (&mut days.thursday, "quiet-hours-day-thursday"),
                    (&mut days.friday, "quiet-hours-day-friday"),
                    (&mut days.saturday, "quiet-hours-day-saturday"),
                    (&mut days.sunday, "quiet-hours-day-sunday"),
                ] {
                    if ui.checkbox(field, crate::i18n::loader().get(key)).changed() {
                        days_changed = true;
                    }
                }
            });
            if days_changed {
                persist_mind(draft, &mind);
            }

            setting_row(
                ui,
                "quiet_hours_time_row",
                &i18n_embed_fl::fl!(crate::i18n::loader(), "quiet-hours-start"),
                "",
                |ui| {
                    if render_time(ui, &mut mind.proactive.quiet_hours.start) {
                        persist_mind(draft, &mind);
                    }
                    ui.label(i18n_embed_fl::fl!(crate::i18n::loader(), "quiet-hours-end"));
                    if render_time(ui, &mut mind.proactive.quiet_hours.end) {
                        persist_mind(draft, &mind);
                    }
                },
            );

            ui.label(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "quiet-hours-suppress"
            ));
            let mut suppress_changed = false;
            {
                let suppress = &mut mind.proactive.quiet_hours.suppress;
                for (field, key) in [
                    (
                        &mut suppress.notifications,
                        "quiet-hours-suppress-notifications",
                    ),
                    (&mut suppress.decisions, "quiet-hours-suppress-decisions"),
                    (&mut suppress.tts, "quiet-hours-suppress-tts"),
                ] {
                    if ui.checkbox(field, crate::i18n::loader().get(key)).changed() {
                        suppress_changed = true;
                    }
                }
            }
            if suppress_changed {
                persist_mind(draft, &mind);
            }

            setting_row(
                ui,
                "quiet_hours_policy_row",
                &i18n_embed_fl::fl!(crate::i18n::loader(), "quiet-hours-policy"),
                "",
                |ui| {
                    let mut selected = mind.proactive.quiet_hours.policy;
                    egui::ComboBox::from_id_salt("quiet_hours_policy")
                        .selected_text(quiet_hours_policy_label(selected))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut selected,
                                ene_mind::QuietHoursPolicy::Discard,
                                i18n_embed_fl::fl!(
                                    crate::i18n::loader(),
                                    "quiet-hours-policy-discard"
                                ),
                            );
                            ui.selectable_value(
                                &mut selected,
                                ene_mind::QuietHoursPolicy::Queue,
                                i18n_embed_fl::fl!(
                                    crate::i18n::loader(),
                                    "quiet-hours-policy-queue"
                                ),
                            );
                            ui.selectable_value(
                                &mut selected,
                                ene_mind::QuietHoursPolicy::Summary,
                                i18n_embed_fl::fl!(
                                    crate::i18n::loader(),
                                    "quiet-hours-policy-summary"
                                ),
                            );
                        });
                    if selected != mind.proactive.quiet_hours.policy {
                        mind.proactive.quiet_hours.policy = selected;
                        persist_mind(draft, &mind);
                    }
                },
            );
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "quiet-hours-policy-hint"
            ));
            ui.weak(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "quiet-hours-hint"
            ));
        });

        ui.add_space(6.0);
        ui.label(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "proactive-sources"
        ));
        ui.add_space(2.0);

        let mut conversation = mind.proactive.sources.conversation;
        if toggle_row(
            ui,
            "proactive_source_conversation",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-source-conversation"),
            "",
            &mut conversation,
        ) {
            mind.proactive.sources.conversation = conversation;
            persist_mind(draft, &mind);
        }

        let mut activity = mind.proactive.sources.activity;
        if toggle_row(
            ui,
            "proactive_source_activity",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-source-activity"),
            "",
            &mut activity,
        ) {
            mind.proactive.sources.activity = activity;
            persist_mind(draft, &mind);
        }

        ui.add_enabled_ui(mind.proactive.sources.activity, |ui| {
            render_window_title_level(ui, draft, &mut mind);
        });

        let mut screen = mind.proactive.sources.screen_summary;
        if toggle_row(
            ui,
            "proactive_source_screen",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-source-screen"),
            &i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-source-screen-hint"),
            &mut screen,
        ) {
            mind.proactive.sources.screen_summary = screen;
            persist_mind(draft, &mind);
        }

        let mut memory = mind.proactive.sources.memory;
        if toggle_row(
            ui,
            "proactive_source_memory",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-source-memory"),
            &i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-source-memory-hint"),
            &mut memory,
        ) {
            mind.proactive.sources.memory = memory;
            persist_mind(draft, &mind);
        }
    });
}

/// Raising the level lets the proactive observer read the focused window's
/// title; the hint warns that title text is sent to the decision LLM (and to
/// a cloud provider when one is configured).
fn render_window_title_level(
    ui: &mut egui::Ui,
    draft: &mut SettingsDraft,
    mind: &mut ene_mind::MindConfig,
) {
    setting_row(
        ui,
        "proactive_window_title_row",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-window-title-level"),
        &i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-window-title-hint"),
        |ui| {
            let mut selected = mind.proactive.sources.window_title_level;
            egui::ComboBox::from_id_salt("proactive_window_title_level")
                .selected_text(window_title_level_label(selected))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut selected,
                        WindowTitleLevel::AppOnly,
                        i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-title-app-only"),
                    );
                    ui.selectable_value(
                        &mut selected,
                        WindowTitleLevel::RedactedTitle,
                        i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-title-redacted"),
                    );
                    ui.selectable_value(
                        &mut selected,
                        WindowTitleLevel::FullTitle,
                        i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-title-full"),
                    );
                });
            if selected != mind.proactive.sources.window_title_level {
                mind.proactive.sources.window_title_level = selected;
                persist_mind(draft, mind);
            }
        },
    );
}

fn window_title_level_label(level: WindowTitleLevel) -> String {
    match level {
        WindowTitleLevel::AppOnly => {
            i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-title-app-only")
        }
        WindowTitleLevel::RedactedTitle => {
            i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-title-redacted")
        }
        WindowTitleLevel::FullTitle => {
            i18n_embed_fl::fl!(crate::i18n::loader(), "proactive-title-full")
        }
    }
}

fn quiet_hours_policy_label(policy: ene_mind::QuietHoursPolicy) -> String {
    match policy {
        ene_mind::QuietHoursPolicy::Discard => {
            i18n_embed_fl::fl!(crate::i18n::loader(), "quiet-hours-policy-discard")
        }
        ene_mind::QuietHoursPolicy::Queue => {
            i18n_embed_fl::fl!(crate::i18n::loader(), "quiet-hours-policy-queue")
        }
        ene_mind::QuietHoursPolicy::Summary => {
            i18n_embed_fl::fl!(crate::i18n::loader(), "quiet-hours-policy-summary")
        }
    }
}

/// Hour/minute drag row for a quiet-hours wall-clock time; returns true when
/// the caller should persist the change.
fn render_time(ui: &mut egui::Ui, time: &mut ene_mind::QuietHoursTimeConfig) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut hour = i32::from(time.hour);
        if ui
            .add(egui::DragValue::new(&mut hour).range(0..=23).prefix(" "))
            .changed()
        {
            time.hour = hour.clamp(0, 23) as u8;
            changed = true;
        }
        ui.label(":");
        let mut minute = i32::from(time.minute);
        if ui
            .add(egui::DragValue::new(&mut minute).range(0..=59))
            .changed()
        {
            time.minute = minute.clamp(0, 59) as u8;
            changed = true;
        }
    });
    changed
}

fn persist_mind(draft: &mut SettingsDraft, mind: &ene_mind::MindConfig) {
    draft.set_section(mind);
}
