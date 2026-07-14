use crate::ai_bridge::AiBridge;
use crate::component::ui::UiStateComponent;
use crate::memory_journal::{MemoryJournalAction, MemoryJournalPresenter};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use i18n_embed_fl::fl;
use std::sync::Arc;

pub fn render(ui: &mut egui::Ui, ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    ui.heading(fl!(crate::i18n::loader(), "memory-journal-title"));

    let mut do_refresh = false;
    let mut do_recall_search = false;
    ui.horizontal(|ui| {
        if ui
            .button(fl!(crate::i18n::loader(), "memory-journal-refresh"))
            .clicked()
        {
            do_refresh = true;
        }
        if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
            ui.checkbox(
                &mut state.0.memory_journal_recall_debug,
                fl!(crate::i18n::loader(), "memory-journal-recall-debug"),
            );
        }
    });

    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        ui.horizontal(|ui| {
            ui.checkbox(
                &mut state.0.memory_journal_show_deleted,
                fl!(crate::i18n::loader(), "memory-journal-show-deleted"),
            );
            ui.checkbox(
                &mut state.0.memory_journal_show_archived,
                fl!(crate::i18n::loader(), "memory-journal-show-archived"),
            );
            ui.checkbox(
                &mut state.0.memory_journal_show_superseded,
                fl!(crate::i18n::loader(), "memory-journal-show-superseded"),
            );
        });
    }

    if do_refresh {
        refresh_journal(ai, world, ui_entity);
    }

    let recall_debug = world
        .get::<UiStateComponent>(ui_entity)
        .is_some_and(|s| s.0.memory_journal_recall_debug);

    if recall_debug {
        ui.separator();
        ui.label(fl!(crate::i18n::loader(), "memory-journal-recall-mode"));
        ui.horizontal(|ui| {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                ui.text_edit_singleline(&mut state.0.memory_journal_recall_query);
            }
            if ui
                .button(fl!(crate::i18n::loader(), "memory-journal-recall-search"))
                .clicked()
            {
                do_recall_search = true;
            }
        });
    }

    if do_recall_search {
        let query = world
            .get::<UiStateComponent>(ui_entity)
            .map(|s| s.0.memory_journal_recall_query.clone())
            .unwrap_or_default();
        run_recall_search(ai, world, ui_entity, &query);
    }

    let snapshot = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.clone());
    let Some(snapshot) = snapshot else {
        return;
    };

    if recall_debug {
        render_recall_rows(ui, &snapshot);
    } else {
        render_browse_rows(ui, ai, world, ui_entity, &snapshot);
    }

    if let Some(message) = snapshot.memory_journal_message.as_deref() {
        ui.separator();
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
}

fn render_browse_rows(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    world: &mut World,
    ui_entity: Entity,
    snapshot: &crate::settings::UiState,
) {
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
                    let pin_label = if row.pinned {
                        fl!(crate::i18n::loader(), "memory-journal-pinned")
                    } else {
                        String::new()
                    };
                    ui.label(format!(
                        "{}: {}{}",
                        fl!(crate::i18n::loader(), "memory-journal-title-field"),
                        row.title,
                        if pin_label.is_empty() {
                            String::new()
                        } else {
                            format!(" [{pin_label}]")
                        }
                    ));
                    ui.label(format!(
                        "{}: {}  |  {}: {}  |  {}: {}",
                        fl!(crate::i18n::loader(), "memory-journal-kind"),
                        row.kind,
                        fl!(crate::i18n::loader(), "memory-journal-status"),
                        row.status,
                        fl!(crate::i18n::loader(), "memory-journal-scope"),
                        row.scope
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
                        fl!(crate::i18n::loader(), "memory-journal-source"),
                        row.source_metadata
                    ));
                    ui.horizontal_wrapped(|ui| {
                        for action in &row.available_actions {
                            if ui.button(action_label(*action)).clicked() {
                                set_action_message(
                                    world,
                                    ui_entity,
                                    ai.execute_journal_action(row.id, *action),
                                    action.i18n_key(),
                                );
                            }
                        }
                    });
                });
                ui.add_space(4.0);
            }
        });
}

fn render_recall_rows(ui: &mut egui::Ui, snapshot: &crate::settings::UiState) {
    ui.separator();
    egui::ScrollArea::vertical()
        .max_height(320.0)
        .show(ui, |ui| {
            if snapshot.memory_journal_recall_rows.is_empty() {
                ui.weak(fl!(crate::i18n::loader(), "memory-journal-recall-empty"));
                return;
            }

            for row in &snapshot.memory_journal_recall_rows {
                ui.group(|ui| {
                    ui.label(format!("#{} {}", row.id, row.title));
                    ui.label(format!(
                        "{}: {}",
                        fl!(crate::i18n::loader(), "memory-journal-recall-reason"),
                        recall_reason_label(&row.reason)
                    ));
                    ui.label(format!(
                        "{}: {}",
                        fl!(crate::i18n::loader(), "memory-journal-recall-scores"),
                        row.score_summary
                    ));
                });
                ui.add_space(4.0);
            }
        });
}

fn action_label_from_key(action_key: &str) -> String {
    match action_key {
        "memory-journal-action-pin" => fl!(crate::i18n::loader(), "memory-journal-action-pin"),
        "memory-journal-action-unpin" => fl!(crate::i18n::loader(), "memory-journal-action-unpin"),
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
        other => other.to_string(),
    }
}

fn action_label(action: MemoryJournalAction) -> String {
    action_label_from_key(action.i18n_key())
}

fn recall_reason_label(reason_key: &str) -> String {
    match reason_key {
        "similar_topic" => fl!(
            crate::i18n::loader(),
            "memory-journal-recall-reason-similar_topic"
        ),
        "recent_conversation" => {
            fl!(
                crate::i18n::loader(),
                "memory-journal-recall-reason-recent_conversation"
            )
        }
        "active_promise" => {
            fl!(
                crate::i18n::loader(),
                "memory-journal-recall-reason-active_promise"
            )
        }
        "character_lore" => fl!(
            crate::i18n::loader(),
            "memory-journal-recall-reason-character_lore"
        ),
        "user_preference" => {
            fl!(
                crate::i18n::loader(),
                "memory-journal-recall-reason-user_preference"
            )
        }
        "emotional_continuity" => fl!(
            crate::i18n::loader(),
            "memory-journal-recall-reason-emotional_continuity"
        ),
        "pinned" => fl!(crate::i18n::loader(), "memory-journal-recall-reason-pinned"),
        other => other.to_string(),
    }
}

fn refresh_journal(ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    let filters = world
        .get::<UiStateComponent>(ui_entity)
        .map_or((false, false, false), |s| {
            (
                s.0.memory_journal_show_deleted,
                s.0.memory_journal_show_archived,
                s.0.memory_journal_show_superseded,
            )
        });

    match ai.refresh_memory_journal(48, filters.0, filters.1, filters.2) {
        Ok((memories, affect, commitments)) => {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.memory_journal_rows = memories
                    .iter()
                    .map(MemoryJournalPresenter::row_from_item)
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

fn run_recall_search(ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity, query: &str) {
    if query.trim().is_empty() {
        if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
            state.0.memory_journal_message = Some(fl!(
                crate::i18n::loader(),
                "memory-journal-recall-query-required"
            ));
        }
        return;
    }

    match ai.search_memory_journal_recall(query, 12) {
        Ok(rows) => {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.memory_journal_recall_rows = rows;
                state.0.memory_journal_message =
                    Some(fl!(crate::i18n::loader(), "memory-journal-recall-ok"));
            }
        }
        Err(error) => {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.memory_journal_message = Some(format!(
                    "{}: {error}",
                    fl!(crate::i18n::loader(), "memory-journal-recall-error")
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
        let action = action_label_from_key(action_key);
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
