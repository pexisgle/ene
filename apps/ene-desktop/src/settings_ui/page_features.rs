//! Features settings page — mind / tools toggles and proactive policy knobs.
//!
//! Provider / embedding settings stay on the AI tab. This page owns the
//! public-schema switches and proactive timing / source policy.

use crate::ai_bridge::AiBridge;
use crate::settings::CharacterSettings;
use crate::settings_ui::components::{
    BadgeTone, section_card, setting_row, slider_row, status_badge, toggle_row, warning_box,
};
use crate::settings_ui::draft::SettingsDraft;
use crate::settings_ui::input::SettingsInputState;
use crate::settings_ui::widgets::editable_combo;
use bevy_ecs::world::World;
use ene_mind::WindowTitleLevel;
use ene_plugin_host::PluginConfig;
use ene_rag::ToolRagConfig;
use std::sync::Arc;

/// Known tool binary names shown even when absent from the saved map.
const DEFAULT_TOOL_NAMES: &[&str] = &[
    "app",
    "browser",
    "calc",
    "calendar",
    "counter",
    "fs",
    "geo",
    "git",
    "homeassistant",
    "random",
    "utility",
    "web",
];

pub fn render(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    draft: &mut SettingsDraft,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
) {
    ui.vertical(|ui| {
        ui.weak(i18n_embed_fl::fl!(crate::i18n::loader(), "features-hint"));
        ui.add_space(6.0);
        section_card(
            ui,
            "features-mind",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "features-mind"),
            |ui| render_mind(ui, draft),
        );
        section_card(
            ui,
            "features-tools",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "features-tools"),
            |ui| render_tools(ui, draft),
        );
        section_card(
            ui,
            "features-capabilities",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "section-features-capabilities"),
            |ui| render_audio(ui, settings, draft, ai, input, world),
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

/// Window-title capture level combo for the activity source.
///
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

fn render_tools(ui: &mut egui::Ui, draft: &mut SettingsDraft) {
    let mut tools = draft.section::<PluginConfig>();
    let mut rag = draft.section::<ToolRagConfig>();

    let mut tools_enabled = tools.enabled;
    if toggle_row(
        ui,
        "features_tools_enabled",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "enable-tools"),
        "",
        &mut tools_enabled,
    ) {
        tools.enabled = tools_enabled;
        draft.set_section(&tools);
    }

    ui.add_enabled_ui(tools.enabled, |ui| {
        let mut rag_enabled = rag.enabled;
        if toggle_row(
            ui,
            "features_tool_rag",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "enable-tool-rag"),
            "",
            &mut rag_enabled,
        ) {
            rag.enabled = rag_enabled;
            draft.set_section(&rag);
        }

        ui.add_space(6.0);
        ui.label(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "features-per-tool"
        ));
        let mut names: Vec<String> = tools.list.keys().cloned().collect();
        for name in DEFAULT_TOOL_NAMES {
            if !names.iter().any(|n| n == *name) {
                names.push((*name).to_string());
            }
        }
        names.sort();

        let mut list_changed = false;
        for name in names {
            let mut enable = tools.list.get(&name).is_none_or(|entry| entry.enable);
            let label = format!(
                "{} ({})",
                i18n_embed_fl::fl!(crate::i18n::loader(), "enable-tool"),
                tool_display_name(&name)
            );
            if ui.checkbox(&mut enable, label).changed() {
                tools.list.entry(name).or_default().enable = enable;
                list_changed = true;
            }
        }
        if list_changed {
            draft.set_section(&tools);
        }
    });
}

fn render_audio(
    ui: &mut egui::Ui,
    settings: &mut CharacterSettings,
    draft: &mut SettingsDraft,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
) {
    #[cfg(not(feature = "voice"))]
    let _ = world;
    #[cfg(feature = "voice")]
    if input.mic_devices.is_empty() {
        input.mic_devices = crate::audio::capture::list_input_device_names();
    }

    let ai_cfg = draft.section::<ene_ai::AiConfig>();
    let mut changed = false;
    let mut mic_device_changed = false;

    // Microphone device selection (stored on the desktop section).
    setting_row(
        ui,
        "features_mic_device_row",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "audio-mic-device"),
        "",
        |ui| {
            let mut device = settings.mic_device().unwrap_or_default();
            let mut choices: Vec<(String, String)> = vec![(
                String::new(),
                i18n_embed_fl::fl!(crate::i18n::loader(), "audio-mic-default"),
            )];
            choices.extend(
                input
                    .mic_devices
                    .iter()
                    .map(|name| (name.clone(), name.clone())),
            );
            if !device.is_empty() && !choices.iter().any(|(value, _)| value == &device) {
                choices.insert(0, (device.clone(), device.clone()));
            }
            let combo = editable_combo(
                ui,
                "features_mic_device_combo",
                &mut device,
                &choices,
                200.0,
            );
            if ui
                .button(i18n_embed_fl::fl!(
                    crate::i18n::loader(),
                    "audio-mic-refresh"
                ))
                .clicked()
            {
                #[cfg(feature = "voice")]
                {
                    input.mic_devices = crate::audio::capture::list_input_device_names();
                }
            }
            if combo.commit_requested() {
                settings.set_mic_device(if device.trim().is_empty() {
                    None
                } else {
                    Some(device.trim().to_string())
                });
                mic_device_changed = true;
            }
        },
    );

    let mut vad_threshold = settings.vad_threshold();
    if slider_row(
        ui,
        "features_vad_threshold_row",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "audio-vad-threshold"),
        "",
        &mut vad_threshold,
        0.0..=1.0,
        0.05,
        |v| format!("{v:.2}"),
    ) {
        settings.set_vad_threshold(vad_threshold);
        changed = true;
    }

    setting_row(
        ui,
        "features_stt_provider_row",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "audio-stt-provider"),
        "",
        |ui| {
            ui.weak(provider_display(&ai_cfg.stt.provider));
        },
    );
    setting_row(
        ui,
        "features_tts_provider_row",
        &i18n_embed_fl::fl!(crate::i18n::loader(), "audio-tts-provider"),
        "",
        |ui| {
            ui.weak(provider_display(&ai_cfg.tts.provider));
        },
    );
    #[cfg(feature = "voice")]
    {
        // Display the live capture state (a dead thread after a device
        // unplug shows as disabled even though the config flag is set);
        // writes still go to the persisted config.
        let mut enabled = world
            .get_resource::<crate::resource::beat_sync::BeatSyncRuntime>()
            .is_some_and(crate::resource::beat_sync::BeatSyncRuntime::is_running);
        if toggle_row(
            ui,
            "features_beat_sync",
            &i18n_embed_fl::fl!(crate::i18n::loader(), "beat-sync-enabled"),
            &i18n_embed_fl::fl!(crate::i18n::loader(), "beat-sync-hint"),
            &mut enabled,
        ) {
            settings.set_beat_sync_enabled(enabled);
            settings.mark_dirty();
            if let Err(e) =
                crate::audio::set_beat_sync_enabled(world, ai, enabled, settings.beat_sync_device())
            {
                tracing::warn!(
                    component = "BeatSync",
                    error = %e,
                    "beat sync toggle failed"
                );
                // Roll the persisted setting back so a failed start (no
                // loopback device, unsupported format) does not leave the
                // feature enabled forever.
                settings.set_beat_sync_enabled(!enabled);
                settings.mark_dirty();
            }
        }
    }
    ui.weak(i18n_embed_fl::fl!(
        crate::i18n::loader(),
        "audio-open-voice-settings"
    ));

    if changed {
        draft.set_section(&ai_cfg);
    }

    // Propagate audio settings into the shared `AudioState` so the next
    // mic (re)start picks them up. The capture path snapshots
    // `AudioState.config` / `mic_device` at start, so without this the
    // Features page edits would never reach `start_mic_capture`.
    #[cfg(feature = "voice")]
    if (changed || mic_device_changed)
        && let Some(mut audio) = world.get_resource_mut::<crate::audio::AudioState>()
    {
        audio.config = settings.config_clone();
        audio.mic_device = settings.mic_device();
    }
    #[cfg(not(feature = "voice"))]
    {
        let _ = mic_device_changed;
    }
}

fn provider_display(provider: &str) -> String {
    if provider.is_empty() || provider == "none" {
        i18n_embed_fl::fl!(crate::i18n::loader(), "audio-provider-none")
    } else {
        provider.to_string()
    }
}

fn tool_display_name(name: &str) -> &str {
    match name {
        "app" => "App",
        "browser" => "Browser",
        "calc" => "Calculator",
        "calendar" => "Calendar",
        "fs" => "Filesystem",
        "random" => "Random",
        "geo" => "Geo",
        "git" => "Git",
        "homeassistant" => "Home Assistant",
        "utility" => "Utility",
        "web" => "Web",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_tool_names_cover_builtin_set() {
        let defaults = PluginConfig::default();
        for name in DEFAULT_TOOL_NAMES {
            assert!(
                defaults.list.contains_key(*name),
                "missing default tool `{name}`"
            );
        }
    }

    #[test]
    fn tool_display_name_covers_builtins() {
        assert_eq!(tool_display_name("calc"), "Calculator");
        assert_eq!(tool_display_name("fs"), "Filesystem");
        assert_eq!(tool_display_name("random"), "Random");
        assert_eq!(tool_display_name("geo"), "Geo");
        assert_eq!(tool_display_name("git"), "Git");
        assert_eq!(tool_display_name("homeassistant"), "Home Assistant");
        assert_eq!(tool_display_name("unknown"), "unknown");
    }
}
