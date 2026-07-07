use crate::ai_bridge::AiBridge;
use crate::component::ui::UiStateComponent;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use i18n_embed_fl::fl;
use std::sync::Arc;

pub fn render(ui: &mut egui::Ui, ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    ui.heading(fl!(crate::i18n::loader(), "memory-journal-title"));

    let mut do_refresh = false;
    ui.horizontal(|ui| {
        if ui
            .button(fl!(crate::i18n::loader(), "memory-journal-refresh"))
            .clicked()
        {
            do_refresh = true;
        }
        if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
            ui.checkbox(
                &mut state.0.memory_journal_show_deleted,
                fl!(crate::i18n::loader(), "memory-journal-show-deleted"),
            );
        }
    });

    if do_refresh {
        refresh_journal(ai, world, ui_entity);
    }

    let snapshot = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.clone());
    let Some(snapshot) = snapshot else {
        return;
    };

    if let Some(message) = snapshot.memory_journal_message.as_deref() {
        ui.label(message);
    }

    ui.group(|ui| {
        ui.label(fl!(crate::i18n::loader(), "memory-journal-affect"));
        ui.label(format!(
            "{}: {}",
            fl!(crate::i18n::loader(), "memory-journal-mood"),
            snapshot.memory_journal_affect.mood
        ));
        ui.label(format!(
            "{}: {}",
            fl!(crate::i18n::loader(), "memory-journal-expression"),
            snapshot.memory_journal_affect.expression
        ));
        ui.label(format!(
            "{}: {:.2}",
            fl!(crate::i18n::loader(), "memory-journal-affinity"),
            snapshot.memory_journal_affect.affinity
        ));
        ui.label(format!(
            "{}: {:.2}",
            fl!(crate::i18n::loader(), "memory-journal-trust"),
            snapshot.memory_journal_affect.trust
        ));
    });

    if !snapshot.memory_journal_commitments.is_empty() {
        ui.separator();
        ui.label(fl!(crate::i18n::loader(), "memory-journal-commitments"));
        for line in &snapshot.memory_journal_commitments {
            ui.label(format!("  - {line}"));
        }
    }

    ui.separator();
    egui::ScrollArea::vertical()
        .max_height(420.0)
        .show(ui, |ui| {
            if snapshot.memory_journal_rows.is_empty() {
                ui.weak(fl!(crate::i18n::loader(), "memory-journal-empty"));
                return;
            }

            for row in &snapshot.memory_journal_rows {
                ui.group(|ui| {
                    ui.label(format!(
                        "{}: {}",
                        fl!(crate::i18n::loader(), "memory-journal-title-field"),
                        row.title
                    ));
                    ui.label(format!(
                        "{}: {}  |  {}: {}",
                        fl!(crate::i18n::loader(), "memory-journal-kind"),
                        row.kind,
                        fl!(crate::i18n::loader(), "memory-journal-status"),
                        row.status
                    ));
                    ui.label(format!(
                        "{}: {:.2}  |  {}: {:.2}",
                        fl!(crate::i18n::loader(), "memory-journal-confidence"),
                        row.confidence,
                        fl!(crate::i18n::loader(), "memory-journal-salience"),
                        row.salience
                    ));
                    ui.label(format!(
                        "{}: {}",
                        fl!(crate::i18n::loader(), "memory-journal-last-accessed"),
                        row.last_accessed.as_deref().unwrap_or("-")
                    ));
                    ui.label(format!(
                        "{}: {}",
                        fl!(crate::i18n::loader(), "memory-journal-why-recalled"),
                        row.why_recalled
                    ));
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .button(fl!(crate::i18n::loader(), "memory-journal-action-pin"))
                            .clicked()
                        {
                            set_action_message(
                                world,
                                ui_entity,
                                ai.pin_memory(row.id),
                                "memory-journal-action-pin",
                            );
                        }
                        if ui
                            .button(fl!(crate::i18n::loader(), "memory-journal-action-archive"))
                            .clicked()
                        {
                            set_action_message(
                                world,
                                ui_entity,
                                ai.update_memory_status(row.id, ene_memory::MemoryStatus::Archived),
                                "memory-journal-action-archive",
                            );
                        }
                        if ui
                            .button(fl!(crate::i18n::loader(), "memory-journal-action-forget"))
                            .clicked()
                        {
                            set_action_message(
                                world,
                                ui_entity,
                                ai.update_memory_status(
                                    row.id,
                                    ene_memory::MemoryStatus::UserDeleted,
                                ),
                                "memory-journal-action-forget",
                            );
                        }
                        if ui
                            .button(fl!(crate::i18n::loader(), "memory-journal-action-dispute"))
                            .clicked()
                        {
                            set_action_message(
                                world,
                                ui_entity,
                                ai.update_memory_status(row.id, ene_memory::MemoryStatus::Disputed),
                                "memory-journal-action-dispute",
                            );
                        }
                        if ui
                            .button(fl!(crate::i18n::loader(), "memory-journal-action-restore"))
                            .clicked()
                        {
                            set_action_message(
                                world,
                                ui_entity,
                                ai.update_memory_status(row.id, ene_memory::MemoryStatus::Active),
                                "memory-journal-action-restore",
                            );
                        }
                    });
                });
                ui.add_space(4.0);
            }
        });
}

fn refresh_journal(ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    match ai.refresh_memory_journal(48) {
        Ok((mut memories, affect, commitments)) => {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                if !state.0.memory_journal_show_deleted {
                    memories.retain(|m| m.status != ene_memory::MemoryStatus::UserDeleted);
                }
                state.0.memory_journal_rows = memories
                    .into_iter()
                    .map(|m| crate::settings::MemoryJournalRow {
                        id: m.id.unwrap_or_default(),
                        title: m.title,
                        kind: m.kind.as_str().to_string(),
                        status: m.status.as_str().to_string(),
                        confidence: m.confidence.get(),
                        salience: m.salience.get(),
                        last_accessed: m.last_accessed_at.map(|ts| ts.to_rfc3339()),
                        why_recalled: format!(
                            "source={} access_count={}",
                            m.source.as_str(),
                            m.access_count
                        ),
                    })
                    .collect();
                state.0.memory_journal_affect = crate::settings::MemoryJournalAffect {
                    mood: affect.mood_label,
                    expression: affect.last_expression,
                    affinity: affect.affinity,
                    trust: affect.trust,
                };
                state.0.memory_journal_commitments = commitments
                    .into_iter()
                    .map(|c| format!("{} [{}]", c.title, c.status.as_str()))
                    .collect();
                state.0.memory_journal_message =
                    Some(fl!(crate::i18n::loader(), "memory-journal-refresh-ok"));
            }
        }
        Err(error) => {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.memory_journal_message = Some(format!(
                    "{}: {error}",
                    fl!(crate::i18n::loader(), "memory-journal-refresh-error")
                ));
            }
        }
    }
}

fn set_action_message(
    world: &mut World,
    ui_entity: Entity,
    result: Result<bool, String>,
    action_key: &str,
) {
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        let action = action_label(action_key);
        state.0.memory_journal_message = Some(match result {
            Ok(true) => format!(
                "{}: {}",
                action,
                fl!(crate::i18n::loader(), "memory-journal-action-ok")
            ),
            Ok(false) => format!(
                "{}: {}",
                action,
                fl!(crate::i18n::loader(), "memory-journal-action-no-change")
            ),
            Err(error) => format!(
                "{}: {} ({error})",
                action,
                fl!(crate::i18n::loader(), "memory-journal-action-error")
            ),
        });
    }
}

fn action_label(action_key: &str) -> String {
    match action_key {
        "memory-journal-action-pin" => fl!(crate::i18n::loader(), "memory-journal-action-pin"),
        "memory-journal-action-archive" => {
            fl!(crate::i18n::loader(), "memory-journal-action-archive")
        }
        "memory-journal-action-forget" => {
            fl!(crate::i18n::loader(), "memory-journal-action-forget")
        }
        "memory-journal-action-dispute" => {
            fl!(crate::i18n::loader(), "memory-journal-action-dispute")
        }
        "memory-journal-action-restore" => {
            fl!(crate::i18n::loader(), "memory-journal-action-restore")
        }
        _ => action_key.to_string(),
    }
}
