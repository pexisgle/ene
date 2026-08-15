use crate::ai_bridge::AiBridge;
use crate::component::ui::UiStateComponent;
use crate::memory_journal::MemoryJournalAction;
use crate::memory_ledger::{CreatedWithinFilter, MemoryLedgerPresenter};
use crate::settings::MemoryLedgerDraft;
use crate::settings_ui::input::SettingsInputState;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use ene_store::MemoryKind;
use i18n_embed_fl::fl;
use std::sync::Arc;

use crate::settings_ui::components::{
    BadgeTone, danger_button, empty_state, section_card, status_badge,
};

/// Deferred ledger mutation collected during a render pass, applied after the
/// table loop so blocking bridge calls never borrow the world mid-iteration.
enum PendingLedgerAction {
    Edit { id: i64, draft: MemoryLedgerDraft },
    Salience { id: i64, salience: f32 },
    Forget { id: i64 },
    CompleteCommitment { id: i64 },
    CancelCommitment { id: i64 },
}

pub fn render(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
) {
    poll_ledger_feedback(ai, input, world, ui_entity);
    let mut do_refresh = false;
    if let Some(state) = world.get::<UiStateComponent>(ui_entity)
        && !state.0.memory_ledger_loaded
    {
        do_refresh = true;
    }

    ui.horizontal(|ui| {
        if ui
            .button(fl!(crate::i18n::loader(), "memory-ledger-refresh"))
            .clicked()
        {
            do_refresh = true;
        }
        if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
            ui.add(
                egui::TextEdit::singleline(&mut state.0.memory_ledger_query)
                    .hint_text(fl!(crate::i18n::loader(), "memory-ledger-search"))
                    .desired_width(180.0),
            );
            let kind_label = fl!(crate::i18n::loader(), "memory-ledger-filter-kind");
            egui::ComboBox::from_label(kind_label)
                .selected_text(kind_filter_label(state.0.memory_ledger_kind_filter))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut state.0.memory_ledger_kind_filter,
                        None,
                        fl!(crate::i18n::loader(), "memory-ledger-filter-kind-all"),
                    );
                    for kind in MemoryLedgerPresenter::ALL_KINDS {
                        ui.selectable_value(
                            &mut state.0.memory_ledger_kind_filter,
                            Some(kind),
                            label_from_key(MemoryLedgerPresenter::kind_label_key(kind)),
                        );
                    }
                });
            let status_label = fl!(crate::i18n::loader(), "memory-ledger-filter-status");
            egui::ComboBox::from_label(status_label)
                .selected_text(status_filter_label(state.0.memory_ledger_status_filter))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut state.0.memory_ledger_status_filter,
                        None,
                        fl!(crate::i18n::loader(), "memory-ledger-filter-status-all"),
                    );
                    for status in ALL_STATUSES {
                        ui.selectable_value(
                            &mut state.0.memory_ledger_status_filter,
                            Some(status),
                            label_from_key(MemoryLedgerPresenter::status_label_key(status)),
                        );
                    }
                });
            let created_label = fl!(crate::i18n::loader(), "memory-ledger-filter-created");
            egui::ComboBox::from_label(created_label)
                .selected_text(created_filter_label(state.0.memory_ledger_created_within))
                .show_ui(ui, |ui| {
                    for filter in [
                        CreatedWithinFilter::Any,
                        CreatedWithinFilter::Days7,
                        CreatedWithinFilter::Days30,
                        CreatedWithinFilter::Days90,
                    ] {
                        ui.selectable_value(
                            &mut state.0.memory_ledger_created_within,
                            filter,
                            created_filter_label(filter),
                        );
                    }
                });
        }
    });

    if do_refresh {
        refresh_ledger(ai, world, ui_entity, None);
    }

    let snapshot = world
        .get::<UiStateComponent>(ui_entity)
        .map(|s| s.0.clone());
    let Some(snapshot) = snapshot else {
        return;
    };
    if !snapshot.memory_ledger_loaded {
        empty_state(ui, &fl!(crate::i18n::loader(), "memory-ledger-empty"), "");
        return;
    }

    let mut pending: Vec<PendingLedgerAction> = Vec::new();
    section_card(
        ui,
        "ledger-browse",
        &fl!(crate::i18n::loader(), "memory-ledger-title"),
        |ui| {
            render_kind_distribution(ui, &snapshot);
            render_memory_table(ui, world, ui_entity, &snapshot, &mut pending);
        },
    );
    section_card(
        ui,
        "ledger-commitments",
        &fl!(crate::i18n::loader(), "memory-ledger-commitments-title"),
        |ui| render_commitment_table(ui, &snapshot, &mut pending),
    );
    apply_pending(ai, input, pending);

    if let Some(message) = snapshot.memory_ledger_message.as_deref() {
        ui.separator();
        ui.label(message);
    }

    render_edit_dialog(ui, ai, input, world, ui_entity);
}

fn render_kind_distribution(ui: &mut egui::Ui, snapshot: &crate::settings::UiState) {
    let distribution = MemoryLedgerPresenter::kind_distribution(&snapshot.memory_ledger_rows);
    if distribution.is_empty() {
        return;
    }
    egui::Frame::group(ui.style())
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.label(fl!(
                crate::i18n::loader(),
                "memory-ledger-kind-distribution"
            ));
            ui.horizontal_wrapped(|ui| {
                let total = distribution.iter().map(|(_, n)| n).sum::<usize>();
                for (kind, count) in &distribution {
                    let label = label_from_key(MemoryLedgerPresenter::kind_label_key(*kind));
                    let fraction = *count as f32 / total as f32;
                    ui.colored_label(kind_color(*kind), format!("{label}: {count}"));
                    ui.add(
                        egui::ProgressBar::new(fraction)
                            .desired_width(50.0)
                            .text(""),
                    );
                }
            });
        });
}

fn render_memory_table(
    ui: &mut egui::Ui,
    world: &mut World,
    ui_entity: Entity,
    snapshot: &crate::settings::UiState,
    pending: &mut Vec<PendingLedgerAction>,
) {
    let query = snapshot.memory_ledger_query.clone();
    let kind = snapshot.memory_ledger_kind_filter;
    let status = snapshot.memory_ledger_status_filter;
    let created_within = snapshot.memory_ledger_created_within;
    let rows = MemoryLedgerPresenter::filter_rows(
        &snapshot.memory_ledger_rows,
        &query,
        kind,
        status,
        created_within,
        chrono::Utc::now(),
    );

    ui.separator();
    if rows.is_empty() {
        empty_state(ui, &fl!(crate::i18n::loader(), "memory-ledger-empty"), "");
        return;
    }

    let pending_delete = world
        .get::<UiStateComponent>(ui_entity)
        .and_then(|s| s.0.memory_ledger_pending_delete);

    if ui.available_width() < 680.0 {
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .show(ui, |ui| {
                for row in &rows {
                    egui::Frame::group(ui.style())
                        .corner_radius(egui::CornerRadius::same(6))
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.colored_label(
                                    kind_color(row.kind),
                                    label_from_key(MemoryLedgerPresenter::kind_label_key(row.kind)),
                                );
                                status_badge(
                                    ui,
                                    &label_from_key(MemoryLedgerPresenter::status_label_key(
                                        row.status,
                                    )),
                                    status_tone(row.status),
                                );
                                if row.pinned {
                                    status_badge(
                                        ui,
                                        &fl!(crate::i18n::loader(), "memory-journal-pinned"),
                                        BadgeTone::Neutral,
                                    );
                                }
                            });
                            ui.strong(&row.title);
                            ui.weak(&row.content_preview);
                            ui.weak(format!(
                                "{}: {} · {}",
                                fl!(crate::i18n::loader(), "memory-journal-scope"),
                                row.scope,
                                row.created_at.format("%Y-%m-%d")
                            ));
                            render_salience_slider(ui, row, pending);
                            render_memory_actions(
                                ui,
                                world,
                                ui_entity,
                                row,
                                pending_delete,
                                pending,
                            );
                        });
                    ui.add_space(4.0);
                }
            });
        return;
    }

    egui::ScrollArea::vertical()
        .max_height(360.0)
        .show(ui, |ui| {
            egui::Grid::new("memory_ledger_memories")
                .striped(true)
                .min_col_width(72.0)
                .show(ui, |ui| {
                    ui.strong(fl!(crate::i18n::loader(), "memory-ledger-column-kind"));
                    ui.strong(fl!(crate::i18n::loader(), "memory-ledger-column-title"));
                    ui.strong(fl!(crate::i18n::loader(), "memory-ledger-column-created"));
                    ui.strong(fl!(crate::i18n::loader(), "memory-ledger-column-status"));
                    ui.strong(fl!(crate::i18n::loader(), "memory-ledger-column-salience"));
                    ui.strong(fl!(crate::i18n::loader(), "memory-ledger-column-actions"));
                    ui.end_row();

                    for row in &rows {
                        let kind_label =
                            label_from_key(MemoryLedgerPresenter::kind_label_key(row.kind));
                        ui.colored_label(kind_color(row.kind), kind_label);
                        ui.vertical(|ui| {
                            ui.strong(&row.title);
                            ui.weak(&row.content_preview);
                            if row.pinned {
                                ui.weak(fl!(crate::i18n::loader(), "memory-journal-pinned"));
                            }
                            ui.weak(format!(
                                "{}: {}",
                                fl!(crate::i18n::loader(), "memory-journal-scope"),
                                row.scope
                            ));
                        });
                        ui.label(row.created_at.format("%Y-%m-%d").to_string());
                        status_badge(
                            ui,
                            &label_from_key(MemoryLedgerPresenter::status_label_key(row.status)),
                            status_tone(row.status),
                        );
                        render_salience_slider(ui, row, pending);
                        render_memory_actions(ui, world, ui_entity, row, pending_delete, pending);
                        ui.end_row();
                    }
                });
        });
}

fn render_memory_actions(
    ui: &mut egui::Ui,
    world: &mut World,
    ui_entity: Entity,
    row: &crate::memory_ledger::MemoryLedgerRow,
    pending_delete: Option<i64>,
    pending: &mut Vec<PendingLedgerAction>,
) {
    ui.horizontal_wrapped(|ui| {
        if ui
            .button(fl!(crate::i18n::loader(), "memory-ledger-action-edit"))
            .clicked()
        {
            open_edit_draft(world, ui_entity, row);
        }
        if row.status != ene_store::MemoryStatus::UserDeleted {
            if pending_delete == Some(row.id) {
                if ui
                    .button(fl!(crate::i18n::loader(), "memory-ledger-action-confirm"))
                    .clicked()
                {
                    pending.push(PendingLedgerAction::Forget { id: row.id });
                    clear_pending_delete(world, ui_entity);
                }
            } else if danger_button(
                ui,
                &fl!(crate::i18n::loader(), "memory-ledger-action-delete"),
            )
            .clicked()
            {
                set_pending_delete(world, ui_entity, row.id);
            }
        }
    });
}

fn render_salience_slider(
    ui: &mut egui::Ui,
    row: &crate::memory_ledger::MemoryLedgerRow,
    pending: &mut Vec<PendingLedgerAction>,
) {
    let weight_label = if row.kind == MemoryKind::Preference {
        fl!(crate::i18n::loader(), "memory-ledger-preference-weight")
    } else {
        fl!(crate::i18n::loader(), "memory-ledger-importance")
    };
    let mut salience = row.salience;
    let response = ui.add_sized(
        [80.0, 20.0],
        egui::Slider::new(&mut salience, 0.0..=1.0).show_value(false),
    );
    ui.label(format!("{weight_label}: {salience:.2}"));
    if response.drag_stopped() && (salience - row.salience).abs() > f32::EPSILON {
        pending.push(PendingLedgerAction::Salience {
            id: row.id,
            salience,
        });
    }
}

fn render_commitment_table(
    ui: &mut egui::Ui,
    snapshot: &crate::settings::UiState,
    pending: &mut Vec<PendingLedgerAction>,
) {
    ui.separator();
    let commitments = &snapshot.memory_ledger_commitments;
    if commitments.is_empty() {
        empty_state(
            ui,
            &fl!(crate::i18n::loader(), "memory-ledger-commitments-empty"),
            "",
        );
        return;
    }
    if ui.available_width() < 680.0 {
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(ui, |ui| {
                for commitment in commitments {
                    egui::Frame::group(ui.style())
                        .corner_radius(egui::CornerRadius::same(6))
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.strong(&commitment.title);
                                status_badge(
                                    ui,
                                    &label_from_key(commitment_status_key(commitment.status)),
                                    commitment_status_tone(commitment.status),
                                );
                            });
                            ui.weak(format!(
                                "{}: {} · {}: {}",
                                fl!(crate::i18n::loader(), "memory-ledger-commitment-due"),
                                commitment.due_at.map_or_else(
                                    || "-".to_string(),
                                    |ts| ts.format("%Y-%m-%d").to_string(),
                                ),
                                fl!(crate::i18n::loader(), "memory-ledger-column-created"),
                                commitment.created_at.format("%Y-%m-%d")
                            ));
                            render_commitment_actions(ui, commitment, pending);
                        });
                    ui.add_space(4.0);
                }
            });
        return;
    }
    egui::ScrollArea::vertical()
        .max_height(220.0)
        .show(ui, |ui| {
            egui::Grid::new("memory_ledger_commitments")
                .striped(true)
                .min_col_width(72.0)
                .show(ui, |ui| {
                    ui.strong(fl!(crate::i18n::loader(), "memory-ledger-column-title"));
                    ui.strong(fl!(crate::i18n::loader(), "memory-ledger-column-status"));
                    ui.strong(fl!(crate::i18n::loader(), "memory-ledger-commitment-due"));
                    ui.strong(fl!(crate::i18n::loader(), "memory-ledger-column-created"));
                    ui.strong(fl!(crate::i18n::loader(), "memory-ledger-column-actions"));
                    ui.end_row();
                    for commitment in commitments {
                        ui.strong(&commitment.title);
                        status_badge(
                            ui,
                            &label_from_key(commitment_status_key(commitment.status)),
                            commitment_status_tone(commitment.status),
                        );
                        ui.label(commitment.due_at.map_or_else(
                            || "-".to_string(),
                            |ts| ts.format("%Y-%m-%d").to_string(),
                        ));
                        ui.label(commitment.created_at.format("%Y-%m-%d").to_string());
                        render_commitment_actions(ui, commitment, pending);
                        ui.end_row();
                    }
                });
        });
}

fn render_commitment_actions(
    ui: &mut egui::Ui,
    commitment: &ene_store::Commitment,
    pending: &mut Vec<PendingLedgerAction>,
) {
    ui.horizontal_wrapped(|ui| {
        if commitment.status == ene_store::CommitmentStatus::Active {
            if ui
                .button(fl!(
                    crate::i18n::loader(),
                    "memory-ledger-commitment-complete"
                ))
                .clicked()
            {
                pending.push(PendingLedgerAction::CompleteCommitment {
                    id: commitment.id.unwrap_or_default(),
                });
            }
            if ui
                .button(fl!(
                    crate::i18n::loader(),
                    "memory-ledger-commitment-cancel"
                ))
                .clicked()
            {
                pending.push(PendingLedgerAction::CancelCommitment {
                    id: commitment.id.unwrap_or_default(),
                });
            }
        }
    });
}

fn apply_pending(
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    pending: Vec<PendingLedgerAction>,
) {
    for action in pending {
        let receiver = match action {
            PendingLedgerAction::Edit { id, draft } => {
                let edit = ene_core::MemoryEdit {
                    title: draft.title,
                    content: draft.content,
                    kind: MemoryKind::from_db_str(&draft.kind),
                    confidence: ene_store::MemoryConfidence::new(draft.confidence),
                };
                let ok_label = fl!(crate::i18n::loader(), "memory-ledger-message-saved");
                let error_label = fl!(crate::i18n::loader(), "memory-ledger-error");
                let receiver = ai.apply_memory_edit(id, edit);
                ai.spawn_fetch(async move {
                    match receiver.await {
                        Ok(Ok(())) => ok_label,
                        Ok(Err(error)) => format!("{error_label}: {error}"),
                        Err(_) => error_label,
                    }
                })
            }
            PendingLedgerAction::Salience { id, salience } => {
                let ok_label = fl!(crate::i18n::loader(), "memory-ledger-message-salience");
                let error_label = fl!(crate::i18n::loader(), "memory-ledger-error");
                let receiver = ai.apply_memory_salience(id, salience);
                ai.spawn_fetch(async move {
                    match receiver.await {
                        Ok(Ok(())) => ok_label,
                        Ok(Err(error)) => format!("{error_label}: {error}"),
                        Err(_) => error_label,
                    }
                })
            }
            PendingLedgerAction::Forget { id } => {
                let ok_label = fl!(crate::i18n::loader(), "memory-ledger-message-deleted");
                let error_label = fl!(crate::i18n::loader(), "memory-ledger-error");
                let receiver = ai.apply_journal_action(id, MemoryJournalAction::Forget);
                ai.spawn_fetch(async move {
                    match receiver.await {
                        Ok(Ok(true)) => ok_label,
                        Ok(Ok(false) | Err(_)) | Err(_) => error_label,
                    }
                })
            }
            PendingLedgerAction::CompleteCommitment { id } => {
                let ok_label = fl!(
                    crate::i18n::loader(),
                    "memory-ledger-message-commitment-done"
                );
                let error_label = fl!(crate::i18n::loader(), "memory-ledger-error");
                let receiver = ai.apply_complete_commitment(id);
                ai.spawn_fetch(async move {
                    match receiver.await {
                        Ok(Ok(true)) => ok_label,
                        Ok(Ok(false) | Err(_)) | Err(_) => error_label,
                    }
                })
            }
            PendingLedgerAction::CancelCommitment { id } => {
                let ok_label = fl!(
                    crate::i18n::loader(),
                    "memory-ledger-message-commitment-cancelled"
                );
                let error_label = fl!(crate::i18n::loader(), "memory-ledger-error");
                let receiver = ai.apply_cancel_commitment(id);
                ai.spawn_fetch(async move {
                    match receiver.await {
                        Ok(Ok(true)) => ok_label,
                        Ok(Ok(false) | Err(_)) | Err(_) => error_label,
                    }
                })
            }
        };
        input.ledger_pending.push(receiver);
    }
}

fn poll_ledger_feedback(
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
) {
    let mut done = None;
    input
        .ledger_pending
        .retain_mut(|receiver| match receiver.try_recv() {
            Ok(message) => {
                done = Some(message);
                false
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => true,
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                done = Some(fl!(crate::i18n::loader(), "memory-ledger-error"));
                false
            }
        });
    if let Some(message) = done {
        refresh_ledger(ai, world, ui_entity, Some(message));
    }
}

fn render_edit_dialog(
    ui: &mut egui::Ui,
    ai: &Arc<AiBridge>,
    input: &mut SettingsInputState,
    world: &mut World,
    ui_entity: Entity,
) {
    let draft = world
        .get::<UiStateComponent>(ui_entity)
        .and_then(|s| s.0.memory_ledger_edit_draft.clone());
    let Some(mut draft) = draft else {
        return;
    };

    let mut save = false;
    let mut cancel = false;
    egui::Window::new(fl!(crate::i18n::loader(), "memory-ledger-edit-title"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            ui.label(fl!(crate::i18n::loader(), "memory-journal-title-field"));
            ui.text_edit_singleline(&mut draft.title);
            ui.label(fl!(crate::i18n::loader(), "memory-journal-content"));
            ui.add(
                egui::TextEdit::multiline(&mut draft.content)
                    .desired_rows(4)
                    .desired_width(320.0),
            );
            ui.label(fl!(crate::i18n::loader(), "memory-journal-kind"));
            egui::ComboBox::from_id_salt("memory_ledger_edit_kind")
                .selected_text(kind_name_label(&draft.kind))
                .show_ui(ui, |ui| {
                    for kind in MemoryLedgerPresenter::ALL_KINDS {
                        ui.selectable_value(
                            &mut draft.kind,
                            kind.as_str().to_string(),
                            label_from_key(MemoryLedgerPresenter::kind_label_key(kind)),
                        );
                    }
                });
            ui.horizontal(|ui| {
                ui.label(fl!(crate::i18n::loader(), "memory-journal-confidence"));
                ui.add(egui::Slider::new(&mut draft.confidence, 0.0..=1.0));
                ui.label(format!("{:.2}", draft.confidence));
            });
            ui.horizontal(|ui| {
                if ui
                    .button(fl!(crate::i18n::loader(), "memory-ledger-edit-save"))
                    .clicked()
                {
                    save = true;
                }
                if ui
                    .button(fl!(crate::i18n::loader(), "memory-ledger-edit-cancel"))
                    .clicked()
                {
                    cancel = true;
                }
            });
        });

    if cancel && let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.memory_ledger_edit_draft = None;
    }
    if save && let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.memory_ledger_edit_draft = None;
        let pending = PendingLedgerAction::Edit {
            id: draft.id,
            draft,
        };
        apply_pending(ai, input, vec![pending]);
    }
}

fn refresh_ledger(
    ai: &Arc<AiBridge>,
    world: &mut World,
    ui_entity: Entity,
    feedback: Option<String>,
) {
    match ai.refresh_memory_ledger(200) {
        Ok(payload) => {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.memory_ledger_rows = payload
                    .memories
                    .iter()
                    .map(MemoryLedgerPresenter::row_from_item)
                    .collect();
                state.0.memory_ledger_commitments = payload.commitments;
                state.0.memory_ledger_loaded = true;
                state.0.memory_ledger_message = Some(
                    feedback
                        .unwrap_or_else(|| fl!(crate::i18n::loader(), "memory-ledger-refresh-ok")),
                );
            }
        }
        Err(error) => {
            if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
                state.0.memory_ledger_message = Some(format!(
                    "{}: {error}",
                    fl!(crate::i18n::loader(), "memory-ledger-error")
                ));
            }
        }
    }
}

fn open_edit_draft(
    world: &mut World,
    ui_entity: Entity,
    row: &crate::memory_ledger::MemoryLedgerRow,
) {
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.memory_ledger_edit_draft = Some(MemoryLedgerDraft {
            id: row.id,
            title: row.title.clone(),
            content: row.content.clone(),
            kind: row.kind.as_str().to_string(),
            confidence: row.confidence,
        });
        state.0.memory_ledger_pending_delete = None;
    }
}

fn set_pending_delete(world: &mut World, ui_entity: Entity, id: i64) {
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.memory_ledger_pending_delete = Some(id);
    }
}

fn clear_pending_delete(world: &mut World, ui_entity: Entity) {
    if let Some(mut state) = world.get_mut::<UiStateComponent>(ui_entity) {
        state.0.memory_ledger_pending_delete = None;
    }
}

fn kind_filter_label(kind: Option<MemoryKind>) -> String {
    match kind {
        Some(kind) => label_from_key(MemoryLedgerPresenter::kind_label_key(kind)),
        None => fl!(crate::i18n::loader(), "memory-ledger-filter-kind-all"),
    }
}

fn status_filter_label(status: Option<ene_store::MemoryStatus>) -> String {
    match status {
        Some(status) => label_from_key(MemoryLedgerPresenter::status_label_key(status)),
        None => fl!(crate::i18n::loader(), "memory-ledger-filter-status-all"),
    }
}

fn created_filter_label(filter: CreatedWithinFilter) -> String {
    let key = match filter {
        CreatedWithinFilter::Any => "memory-ledger-filter-created-any",
        CreatedWithinFilter::Days7 => "memory-ledger-filter-created-7",
        CreatedWithinFilter::Days30 => "memory-ledger-filter-created-30",
        CreatedWithinFilter::Days90 => "memory-ledger-filter-created-90",
    };
    label_from_key(key)
}

fn kind_name_label(kind: &str) -> String {
    let known = MemoryLedgerPresenter::ALL_KINDS
        .iter()
        .find(|k| k.as_str() == kind);
    match known {
        Some(kind) => label_from_key(MemoryLedgerPresenter::kind_label_key(*kind)),
        None => kind.to_string(),
    }
}

fn commitment_status_key(status: ene_store::CommitmentStatus) -> &'static str {
    match status {
        ene_store::CommitmentStatus::Active => "memory-commitment-status-active",
        ene_store::CommitmentStatus::Done => "memory-commitment-status-done",
        ene_store::CommitmentStatus::Cancelled => "memory-commitment-status-cancelled",
        ene_store::CommitmentStatus::Stale => "memory-commitment-status-stale",
        _ => "memory-ledger-status-unknown",
    }
}

fn kind_color(kind: MemoryKind) -> egui::Color32 {
    match kind {
        MemoryKind::Episodic => egui::Color32::from_rgb(90, 160, 220),
        MemoryKind::Semantic => egui::Color32::from_rgb(110, 190, 130),
        MemoryKind::UserProfile => egui::Color32::from_rgb(230, 170, 90),
        MemoryKind::Relationship => egui::Color32::from_rgb(220, 130, 160),
        MemoryKind::Affective => egui::Color32::from_rgb(200, 110, 110),
        MemoryKind::Commitment => egui::Color32::from_rgb(160, 140, 220),
        MemoryKind::Preference => egui::Color32::from_rgb(90, 200, 190),
        MemoryKind::Procedure => egui::Color32::from_rgb(150, 170, 190),
        MemoryKind::WorldState => egui::Color32::from_rgb(120, 140, 120),
        MemoryKind::Reflection => egui::Color32::from_rgb(190, 180, 120),
        _ => egui::Color32::GRAY,
    }
}

fn status_tone(status: ene_store::MemoryStatus) -> BadgeTone {
    match status {
        ene_store::MemoryStatus::Active => BadgeTone::Ok,
        ene_store::MemoryStatus::Faded => BadgeTone::Warn,
        ene_store::MemoryStatus::Archived => BadgeTone::Neutral,
        ene_store::MemoryStatus::Disputed => BadgeTone::Error,
        ene_store::MemoryStatus::Superseded => BadgeTone::Neutral,
        ene_store::MemoryStatus::UserDeleted => BadgeTone::Error,
        _ => BadgeTone::Neutral,
    }
}

fn commitment_status_tone(status: ene_store::CommitmentStatus) -> BadgeTone {
    match status {
        ene_store::CommitmentStatus::Active => BadgeTone::Ok,
        ene_store::CommitmentStatus::Done => BadgeTone::Neutral,
        ene_store::CommitmentStatus::Cancelled => BadgeTone::Warn,
        _ => BadgeTone::Neutral,
    }
}

/// `fl!` requires a literal `message_id`, so keys that depend on a row's
/// kind/status go through here.
fn label_from_key(key: &str) -> String {
    match key {
        "memory-kind-episodic" => fl!(crate::i18n::loader(), "memory-kind-episodic"),
        "memory-kind-semantic" => fl!(crate::i18n::loader(), "memory-kind-semantic"),
        "memory-kind-user-profile" => {
            fl!(crate::i18n::loader(), "memory-kind-user-profile")
        }
        "memory-kind-relationship" => {
            fl!(crate::i18n::loader(), "memory-kind-relationship")
        }
        "memory-kind-affective" => fl!(crate::i18n::loader(), "memory-kind-affective"),
        "memory-kind-commitment" => fl!(crate::i18n::loader(), "memory-kind-commitment"),
        "memory-kind-preference" => fl!(crate::i18n::loader(), "memory-kind-preference"),
        "memory-kind-procedure" => fl!(crate::i18n::loader(), "memory-kind-procedure"),
        "memory-kind-world-state" => fl!(crate::i18n::loader(), "memory-kind-world-state"),
        "memory-kind-reflection" => fl!(crate::i18n::loader(), "memory-kind-reflection"),
        "memory-kind-unknown" => fl!(crate::i18n::loader(), "memory-kind-unknown"),
        "memory-ledger-status-active" => {
            fl!(crate::i18n::loader(), "memory-ledger-status-active")
        }
        "memory-ledger-status-faded" => {
            fl!(crate::i18n::loader(), "memory-ledger-status-faded")
        }
        "memory-ledger-status-archived" => {
            fl!(crate::i18n::loader(), "memory-ledger-status-archived")
        }
        "memory-ledger-status-disputed" => {
            fl!(crate::i18n::loader(), "memory-ledger-status-disputed")
        }
        "memory-ledger-status-superseded" => {
            fl!(crate::i18n::loader(), "memory-ledger-status-superseded")
        }
        "memory-ledger-status-user-deleted" => {
            fl!(crate::i18n::loader(), "memory-ledger-status-user-deleted")
        }
        "memory-ledger-status-unknown" => {
            fl!(crate::i18n::loader(), "memory-ledger-status-unknown")
        }
        "memory-commitment-status-active" => {
            fl!(crate::i18n::loader(), "memory-commitment-status-active")
        }
        "memory-commitment-status-done" => {
            fl!(crate::i18n::loader(), "memory-commitment-status-done")
        }
        "memory-commitment-status-cancelled" => {
            fl!(crate::i18n::loader(), "memory-commitment-status-cancelled")
        }
        "memory-commitment-status-stale" => {
            fl!(crate::i18n::loader(), "memory-commitment-status-stale")
        }
        "memory-ledger-filter-created-any" => {
            fl!(crate::i18n::loader(), "memory-ledger-filter-created-any")
        }
        "memory-ledger-filter-created-7" => {
            fl!(crate::i18n::loader(), "memory-ledger-filter-created-7")
        }
        "memory-ledger-filter-created-30" => {
            fl!(crate::i18n::loader(), "memory-ledger-filter-created-30")
        }
        "memory-ledger-filter-created-90" => {
            fl!(crate::i18n::loader(), "memory-ledger-filter-created-90")
        }
        _ => key.to_string(),
    }
}

const ALL_STATUSES: [ene_store::MemoryStatus; 6] = [
    ene_store::MemoryStatus::Active,
    ene_store::MemoryStatus::Faded,
    ene_store::MemoryStatus::Archived,
    ene_store::MemoryStatus::Disputed,
    ene_store::MemoryStatus::Superseded,
    ene_store::MemoryStatus::UserDeleted,
];
