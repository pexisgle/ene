use ene_api::v1::deletion::{
    ClientTempClass, DeletionDemand, DeletionTargetWire, LocalErasureResult,
};
use ene_api::v1::refs::StreamWireId;
use ene_api::v1::round::HistoryItem;

use crate::ui::deletion::DeletionPanel;
use crate::ui::tasks::TaskPanel;
use crate::ui::usage::UsagePanel;
use crate::ui::{Composer, MemoryPage};

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
                copies.deletion.wipe_exact_text();
                if input_draft_gone(copies) {
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
                copies.tasks.reset_connection_state();
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

fn input_draft_gone(copies: &GuiOwned<'_>) -> bool {
    copies.composer.draft().is_empty()
        && !copies.composer.composing()
        && copies.composer.undo_len() == 0
        && copies.search_draft.is_empty()
        && copies.deletion.exact_text_cleared()
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
