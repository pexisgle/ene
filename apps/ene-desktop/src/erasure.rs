//! GUI erasure participant: every copy this process actually holds.
//!
//! Inventory (Stage 7 E): timeline, Memory grounds/history, Task report,
//! search/input draft, IME composition, undo, deferred frames (Client),
//! usage/deletion panel bodies, and presented chat receipts. Registered
//! secrets are not user content and are not wiped through this path; C1
//! secret intake is zeroized by crate-private `SecretIntake`.
//!
//! `wiped` is returned only after the named copies are confirmed empty.

use ene_api::v1::deletion::{
    ClientTempClass, DeletionDemand, DeletionTargetWire, LocalErasureResult,
};
use ene_api::v1::refs::StreamWireId;
use ene_api::v1::round::HistoryItem;

use crate::ui::deletion::DeletionPanel;
use crate::ui::tasks::TaskPanel;
use crate::ui::usage::UsagePanel;
use crate::ui::{Composer, MemoryPage};

/// Mutable GUI-owned copies one deletion demand may name.
pub(crate) struct GuiOwned<'a> {
    pub timeline: &'a mut Vec<crate::ui::presentation::Message>,
    pub history: &'a mut Vec<HistoryItem>,
    pub composer: &'a mut Composer,
    pub search_draft: &'a mut String,
    pub memory: &'a mut MemoryPage,
    pub tasks: &'a mut TaskPanel,
    pub usage: &'a mut UsagePanel,
    pub deletion: &'a mut DeletionPanel,
    pub chat_receipt: &'a mut Option<(String, Option<StreamWireId>)>,
}

/// Wipes GUI copies named by a Host deletion demand. Returns `wiped` only
/// for copies this process actually cleared and confirmed empty.
pub(crate) fn apply_demand(
    demand: &DeletionDemand,
    copies: &mut GuiOwned<'_>,
) -> LocalErasureResult {
    let mut wiped = Vec::new();
    let mut unverified = Vec::new();
    for target in &demand.targets {
        match target {
            DeletionTargetWire::WipeClass {
                class: ClientTempClass::InputDraft,
            } => {
                copies.composer.wipe();
                copies.search_draft.clear();
                if input_draft_gone(copies.composer, copies.search_draft) {
                    wiped.push(ClientTempClass::InputDraft);
                } else {
                    unverified.push(ClientTempClass::InputDraft);
                }
            }
            DeletionTargetWire::WipeClass {
                class: ClientTempClass::PresentationBuffer,
            } => {
                copies.timeline.clear();
                copies.history.clear();
                copies.memory.wipe();
                copies.tasks.wipe_owned_copies();
                copies.usage.wipe_body();
                copies.deletion.wipe_exact_text();
                *copies.chat_receipt = None;
                if presentation_gone(copies) {
                    wiped.push(ClientTempClass::PresentationBuffer);
                } else {
                    unverified.push(ClientTempClass::PresentationBuffer);
                }
            }
        }
    }
    if wiped.is_empty() && unverified.is_empty() {
        unverified.push(ClientTempClass::PresentationBuffer);
    }
    LocalErasureResult {
        demand: demand.demand.clone(),
        operation: demand.operation.clone(),
        sweep: demand.sweep,
        wiped,
        unverified,
    }
}

fn input_draft_gone(composer: &Composer, search_draft: &str) -> bool {
    composer.draft().is_empty()
        && !composer.composing()
        && composer.undo_len() == 0
        && search_draft.is_empty()
}

fn presentation_gone(copies: &GuiOwned<'_>) -> bool {
    copies.timeline.is_empty()
        && copies.history.is_empty()
        && copies.memory.is_empty()
        && copies.tasks.presentation_cleared()
        && copies.usage.body_cleared()
        && copies.deletion.exact_text_cleared()
        && copies.chat_receipt.is_none()
}
