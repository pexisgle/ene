use crate::ai_bridge::AiBridge;
use crate::component::ui::UiStateComponent;
use crate::memory_journal::{MemoryJournalAction, MemoryJournalPresenter};
use crate::settings::{CommitmentFilter, MemoryPageMode};
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use ene_store::Commitment;
use i18n_embed_fl::fl;
use std::sync::Arc;

pub fn render(ui: &mut egui::Ui, ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    ui.heading(fl!(crate::i18n::loader(), "memory-journal-title"));

    // ── Tab bar ───────────────────────────────────────────────────────────────
    let mut do_refresh = false;
    let mut do_recall_search = false;
    let mut do_fetch_pending = false;
    let mut do_fetch_commitments = false;
    ui.horizontal(|ui| {
        if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
            ui.selectable_value(
                &mut state.0.memory_journal_mode,
                MemoryPageMode::Browse,
                fl!(crate::i18n::loader(), "memory-page-tab-browse"),
            );
            ui.selectable_value(
                &mut state.0.memory_journal_mode,
                MemoryPageMode::RecallSearch,
                fl!(crate::i18n::loader(), "memory-page-tab-recall"),
            );

            // Pending Approval tab with badge
            let pending_count = state.0.memory_journal_pending_count;
            let pending_label = if pending_count > 0 {
                format!(
                    "{} ({})",
                    fl!(crate::i18n::loader(), "memory-page-tab-pending"),
                    pending_count
                )
            } else {
                fl!(crate::i18n::loader(), "memory-page-tab-pending").to_string()
            };
            if ui
                .selectable_value(
                    &mut state.0.memory_journal_mode,
                    MemoryPageMode::PendingApproval,
                    pending_label,
                )
                .clicked()
            {
                do_fetch_pending = true;
            }

            ui.selectable_value(
                &mut state.0.memory_journal_mode,
                MemoryPageMode::Commitments,
                fl!(crate::i18n::loader(), "memory-page-tab-commitments"),
            );
        }
    });

    // ── Toolbar ───────────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        if ui
            .button(fl!(crate::i18n::loader(), "memory-journal-refresh"))
            .clicked()
        {
            let mode = world
                .get::<UiStateComponent>(ui_entity)
                .map(|s| s.0.memory_journal_mode)
                .unwrap_or_default();
            match mode {
                MemoryPageMode::Browse | MemoryPageMode::RecallSearch => do_refresh = true,
                MemoryPageMode::PendingApproval => do_fetch_pending = true,
                MemoryPageMode::Commitments => do_fetch_commitments = true,
            }
        }
        if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
            let before = state.0.memory_journal_recall_debug;
            ui.checkbox(
                &mut state.0.memory_journal_recall_debug,
                fl!(crate::i18n::loader(), "memory-journal-recall-debug"),
            );
            // Toggling the checkbox switches to RecallSearch mode.
            if state.0.memory_journal_recall_debug && !before {
                state.0.memory_journal_mode = MemoryPageMode::RecallSearch;
            }
        }
    });

    // ── Filter row (browse mode only) ────────────────────────────────────────
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity)
        && state.0.memory_journal_mode == MemoryPageMode::Browse
    {
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

    // ── Execute deferred actions ──────────────────────────────────────────────
    if do_refresh {
        refresh_journal(ai, world, ui_entity);
    }
    if do_fetch_pending {
        fetch_pending_candidates(ai, world, ui_entity);
    }
    if do_fetch_commitments {
        refresh_journal(ai, world, ui_entity);
    }

    let snapshot = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.clone());
    let Some(snapshot) = snapshot else {
        return;
    };

    // ── Mode-specific content ─────────────────────────────────────────────────
    match snapshot.memory_journal_mode {
        MemoryPageMode::Browse => {
            render_browse_rows(ui, ai, world, ui_entity, &snapshot);
        }
        MemoryPageMode::RecallSearch => {
            render_recall_search_ui(ui, ai, world, ui_entity, &snapshot, &mut do_recall_search);
        }
        MemoryPageMode::PendingApproval => {
            render_pending_approval(ui, ai, world, ui_entity);
        }
        MemoryPageMode::Commitments => {
            render_commitments(ui, ai, world, ui_entity);
        }
    }

    // ── Status message ────────────────────────────────────────────────────────
    if let Some(message) = snapshot.memory_journal_message.as_deref() {
        ui.separator();
        ui.label(message);
    }

    // ── Journal stats (always visible) ────────────────────────────────────────
    ui.group(|ui| {
        ui.label(fl!(crate::i18n::loader(), "memory-journal-pending-writes"));
        ui.label(format!(
            "{}: {}",
            fl!(crate::i18n::loader(), "memory-journal-pending-count"),
            snapshot.memory_journal_pending_writes
        ));
        ui.label(format!(
            "{}: {}",
            fl!(crate::i18n::loader(), "memory-journal-permanent-count"),
            snapshot.memory_journal_permanent_writes
        ));
    });

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
}

// ── Recall search UI ─────────────────────────────────────────────────────────

fn render_recall_search_ui(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    world: &mut World,
    ui_entity: Entity,
    snapshot: &crate::settings::UiState,
    do_recall_search: &mut bool,
) {
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
            *do_recall_search = true;
        }
    });

    if *do_recall_search {
        let query = world
            .get::<UiStateComponent>(ui_entity)
            .map(|s| s.0.memory_journal_recall_query.clone())
            .unwrap_or_default();
        run_recall_search(ai, world, ui_entity, &query);
        *do_recall_search = false;
    }

    render_recall_rows(ui, snapshot);
}

// ── Pending approval UI ───────────────────────────────────────────────

fn render_pending_approval(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    world: &mut World,
    ui_entity: Entity,
) {
    ui.separator();
    ui.heading(fl!(crate::i18n::loader(), "memory-pending-approval"));

    let candidates = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.memory_journal_pending_candidates.clone())
        .unwrap_or_default();

    if candidates.is_empty() {
        ui.weak(fl!(crate::i18n::loader(), "memory-pending-empty"));
        return;
    }

    ui.label(fl!(
        crate::i18n::loader(),
        "memory-pending-count",
        count = candidates.len()
    ));

    egui::ScrollArea::vertical()
        .max_height(400.0)
        .show(ui, |ui| {
            for candidate in &candidates {
                ui.group(|ui| {
                    ui.strong(&candidate.title);

                    ui.label(format!(
                        "{}: {}",
                        fl!(crate::i18n::loader(), "memory-journal-kind"),
                        candidate.kind
                    ));

                    ui.label(format!(
                        "{}: {}",
                        fl!(crate::i18n::loader(), "memory-journal-content"),
                        candidate.content
                    ));

                    ui.horizontal(|ui| {
                        ui.label(fl!(crate::i18n::loader(), "memory-journal-confidence"));
                        render_salience_bar(ui, candidate.confidence);
                        ui.label(format!("{:.1}%", candidate.confidence * 100.0));
                    });

                    ui.label(format!(
                        "{}: {}",
                        fl!(crate::i18n::loader(), "memory-journal-reason"),
                        candidate.reason_detail
                    ));

                    if !candidate.source_quote.is_empty() {
                        ui.label(format!(
                            "{}: \"{}\"",
                            fl!(crate::i18n::loader(), "memory-journal-source-quote"),
                            candidate.source_quote
                        ));
                    }

                    if let Some(ref existing_title) = candidate.existing_memory_title {
                        ui.label(format!(
                            "{}: {}",
                            fl!(crate::i18n::loader(), "memory-pending-conflict"),
                            existing_title
                        ));
                    }

                    ui.horizontal(|ui| {
                        let candidate_id = candidate.id;
                        if ui
                            .button(fl!(crate::i18n::loader(), "memory-approve"))
                            .clicked()
                        {
                            let result = ai.approve_candidate(candidate_id);
                            set_action_message(
                                world,
                                ui_entity,
                                result.map(|()| true),
                                "memory-approve",
                            );
                            fetch_pending_candidates(ai, world, ui_entity);
                        }
                        if ui
                            .button(fl!(crate::i18n::loader(), "memory-reject"))
                            .clicked()
                        {
                            let result = ai.reject_candidate(candidate_id);
                            set_action_message(
                                world,
                                ui_entity,
                                result.map(|()| true),
                                "memory-reject",
                            );
                            fetch_pending_candidates(ai, world, ui_entity);
                        }
                    });
                });
                ui.add_space(4.0);
            }
        });
}

// ── Commitment management UI ──────────────────────────────────────────

fn render_commitments(ui: &mut egui::Ui, ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    ui.separator();
    ui.heading(fl!(crate::i18n::loader(), "memory-commitments-title"));

    ui.horizontal(|ui| {
        if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
            ui.selectable_value(
                &mut state.0.memory_journal_commitment_filter,
                CommitmentFilter::Active,
                fl!(crate::i18n::loader(), "memory-commitment-filter-active"),
            );
            ui.selectable_value(
                &mut state.0.memory_journal_commitment_filter,
                CommitmentFilter::All,
                fl!(crate::i18n::loader(), "memory-commitment-filter-all"),
            );
            ui.selectable_value(
                &mut state.0.memory_journal_commitment_filter,
                CommitmentFilter::Completed,
                fl!(crate::i18n::loader(), "memory-commitment-filter-done"),
            );
            ui.selectable_value(
                &mut state.0.memory_journal_commitment_filter,
                CommitmentFilter::Cancelled,
                fl!(crate::i18n::loader(), "memory-commitment-filter-cancelled"),
            );
        }
    });

    let filter = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.memory_journal_commitment_filter)
        .unwrap_or_default();

    let commitments = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.memory_journal_commitments_cache.clone())
        .unwrap_or_default();

    if commitments.is_empty() {
        ui.weak(fl!(crate::i18n::loader(), "memory-commitments-empty"));
        return;
    }

    let filtered: Vec<&Commitment> = match filter {
        CommitmentFilter::Active => commitments
            .iter()
            .filter(|c| c.status == ene_store::CommitmentStatus::Active)
            .collect(),
        CommitmentFilter::All => commitments.iter().collect(),
        CommitmentFilter::Completed => commitments
            .iter()
            .filter(|c| c.status == ene_store::CommitmentStatus::Done)
            .collect(),
        CommitmentFilter::Cancelled => commitments
            .iter()
            .filter(|c| c.status == ene_store::CommitmentStatus::Cancelled)
            .collect(),
    };

    egui::ScrollArea::vertical()
        .max_height(400.0)
        .show(ui, |ui| {
            for commitment in &filtered {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.strong(&commitment.title);

                        let status_label = match commitment.status {
                            ene_store::CommitmentStatus::Active => {
                                fl!(crate::i18n::loader(), "memory-commitment-status-active")
                            }
                            ene_store::CommitmentStatus::Done => {
                                fl!(crate::i18n::loader(), "memory-commitment-status-done")
                            }
                            ene_store::CommitmentStatus::Cancelled => {
                                fl!(crate::i18n::loader(), "memory-commitment-status-cancelled")
                            }
                            ene_store::CommitmentStatus::Stale => {
                                fl!(crate::i18n::loader(), "memory-commitment-status-stale")
                            }
                            _ => "unknown".to_string(),
                        };
                        ui.label(format!("[{status_label}]"));
                    });

                    if !commitment.description.is_empty() {
                        ui.label(&commitment.description);
                    }

                    ui.label(format!(
                        "{}: {}",
                        fl!(crate::i18n::loader(), "memory-journal-created"),
                        commitment.created_at.format("%Y-%m-%d %H:%M")
                    ));
                    if let Some(ref due) = commitment.due_at {
                        ui.label(format!(
                            "{}: {}",
                            fl!(crate::i18n::loader(), "memory-commitment-due"),
                            due.format("%Y-%m-%d %H:%M")
                        ));
                    }
                    if let Some(ref due_label) = commitment.due_label {
                        ui.label(format!(
                            "{}: {}",
                            fl!(crate::i18n::loader(), "memory-commitment-due-label"),
                            due_label
                        ));
                    }

                    if commitment.status == ene_store::CommitmentStatus::Active
                        && let Some(c_id) = commitment.id
                    {
                        ui.horizontal(|ui| {
                            if ui
                                .button(fl!(crate::i18n::loader(), "memory-commitment-complete"))
                                .clicked()
                            {
                                let result = ai.complete_commitment(c_id);
                                set_action_message(
                                    world,
                                    ui_entity,
                                    result,
                                    "memory-commitment-complete",
                                );
                                refresh_journal(ai, world, ui_entity);
                            }
                            if ui
                                .button(fl!(crate::i18n::loader(), "memory-commitment-cancel"))
                                .clicked()
                            {
                                let result = ai.cancel_commitment(c_id);
                                set_action_message(
                                    world,
                                    ui_entity,
                                    result,
                                    "memory-commitment-cancel",
                                );
                                refresh_journal(ai, world, ui_entity);
                            }
                        });
                    }
                });
                ui.add_space(4.0);
            }
        });
}

// ── Salience bar visualization ────────────────────────────────────────

fn render_salience_bar(ui: &mut egui::Ui, salience: f32) {
    let color = if salience > 0.7 {
        egui::Color32::GREEN
    } else if salience > 0.3 {
        egui::Color32::YELLOW
    } else {
        egui::Color32::RED
    };

    let bar = egui::ProgressBar::new(salience)
        .fill(color)
        .desired_width(60.0);
    ui.add(bar);
}

// ── Browse rows ──────────────────────────────────────────────────────────────

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
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "{}: {:.2}  |",
                            fl!(crate::i18n::loader(), "memory-journal-confidence"),
                            row.confidence,
                        ));
                        ui.label(fl!(crate::i18n::loader(), "memory-salience"));
                        render_salience_bar(ui, row.salience);
                        ui.label(format!("{:.2}", row.salience));
                    });
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

// ── Recall rows ──────────────────────────────────────────────────────────────

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

// ── Helpers ──────────────────────────────────────────────────────────────────

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
        Ok(payload) => {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.memory_journal_rows = payload
                    .memories
                    .iter()
                    .map(MemoryJournalPresenter::row_from_item)
                    .collect();
                state.0.memory_journal_affect = crate::settings::MemoryJournalAffect {
                    mood: payload.affect.mood_label,
                    expression: payload.affect.last_expression,
                    affinity: payload.affect.affinity,
                    trust: payload.affect.trust,
                };
                state
                    .0
                    .memory_journal_commitments_cache
                    .clone_from(&payload.commitments);
                state.0.memory_journal_commitments = payload
                    .commitments
                    .into_iter()
                    .map(|c| format!("{} [{}]", c.title, c.status.as_str()))
                    .collect();
                state.0.memory_journal_pending_writes = payload.pending_writes;
                state.0.memory_journal_permanent_writes = payload.permanent_writes;
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

fn fetch_pending_candidates(ai: &Arc<AiBridge>, world: &mut World, ui_entity: Entity) {
    match ai.fetch_pending_candidates() {
        Ok(candidates) => {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.memory_journal_pending_candidates = candidates;
                state.0.memory_journal_pending_count =
                    state.0.memory_journal_pending_candidates.len();
                state.0.memory_journal_message =
                    Some(fl!(crate::i18n::loader(), "memory-pending-refresh-ok"));
            }
        }
        Err(error) => {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.memory_journal_message = Some(format!(
                    "{}: {error}",
                    fl!(crate::i18n::loader(), "memory-pending-refresh-error")
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
    result: Result<bool, crate::ai_bridge::AiBridgeError>,
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
