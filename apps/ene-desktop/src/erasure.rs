//! GUI erasure participant: every copy this process actually holds.
//!
//! Inventory (Stage 7 E): timeline, Memory grounds/history, Task report,
//! search/input draft, IME composition, undo, deferred frames (Client),
//! usage/deletion panel bodies, and presented chat receipts. Registered
//! secrets are not user content and are not wiped through this path; C1
//! secret intake is zeroized by [`crate::secret::SecretIntake`].
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
pub struct GuiOwned<'a> {
    pub timeline: &'a mut Vec<String>,
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
pub fn apply_demand(demand: &DeletionDemand, copies: GuiOwned<'_>) -> LocalErasureResult {
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

#[cfg(test)]
mod tests {
    use super::{GuiOwned, apply_demand};
    use crate::ui::deletion::DeletionPanel;
    use crate::ui::tasks::TaskPanel;
    use crate::ui::usage::UsagePanel;
    use crate::ui::{Composer, MemoryPage};
    use ene_api::v1::deletion::{
        ClientTempClass, DeletionDemand, DeletionDemandWireId, DeletionTargetWire,
    };
    use ene_api::v1::refs::DeletionOperationWireRef;

    fn empty_copies<'a>(
        timeline: &'a mut Vec<String>,
        history: &'a mut Vec<ene_api::v1::round::HistoryItem>,
        composer: &'a mut Composer,
        search_draft: &'a mut String,
        memory: &'a mut MemoryPage,
        tasks: &'a mut TaskPanel,
        usage: &'a mut UsagePanel,
        deletion: &'a mut DeletionPanel,
        chat_receipt: &'a mut Option<(String, Option<ene_api::v1::refs::StreamWireId>)>,
    ) -> GuiOwned<'a> {
        GuiOwned {
            timeline,
            history,
            composer,
            search_draft,
            memory,
            tasks,
            usage,
            deletion,
            chat_receipt,
        }
    }

    #[test]
    fn draft_wipe_does_not_touch_a_secret_field() {
        let demand = DeletionDemand {
            demand: DeletionDemandWireId(String::from("demand-1")),
            operation: DeletionOperationWireRef(String::from("op-1")),
            sweep: 1,
            targets: vec![DeletionTargetWire::WipeClass {
                class: ClientTempClass::InputDraft,
            }],
        };
        let mut timeline = vec![String::from("hello")];
        let mut history = Vec::new();
        let mut composer = Composer::default();
        composer.set_draft(String::from("draft text"));
        let mut search_draft = String::from("search");
        let mut memory = MemoryPage::default();
        let mut tasks = TaskPanel::default();
        let mut usage = UsagePanel::default();
        let mut deletion = DeletionPanel::default();
        let mut chat_receipt = None;
        let result = apply_demand(
            &demand,
            empty_copies(
                &mut timeline,
                &mut history,
                &mut composer,
                &mut search_draft,
                &mut memory,
                &mut tasks,
                &mut usage,
                &mut deletion,
                &mut chat_receipt,
            ),
        );
        assert!(result.wiped.contains(&ClientTempClass::InputDraft));
        assert!(result.unverified.is_empty());
        assert!(composer.draft().is_empty());
        assert_eq!(composer.undo_len(), 0);
        assert!(search_draft.is_empty());
        assert_eq!(timeline, vec![String::from("hello")]);
    }

    #[test]
    fn presentation_wipe_reports_wiped_only_when_copies_are_empty() {
        let demand = DeletionDemand {
            demand: DeletionDemandWireId(String::from("demand-2")),
            operation: DeletionOperationWireRef(String::from("op-2")),
            sweep: 1,
            targets: vec![DeletionTargetWire::WipeClass {
                class: ClientTempClass::PresentationBuffer,
            }],
        };
        let mut timeline = vec![String::from("keep-this-keyword")];
        let mut history = Vec::new();
        let mut composer = Composer::default();
        let mut search_draft = String::new();
        let mut memory = MemoryPage::default();
        let mut tasks = TaskPanel::default();
        let mut usage = UsagePanel::default();
        let mut deletion = DeletionPanel::default();
        let mut chat_receipt = Some((String::from("round-old"), None));
        let result = apply_demand(
            &demand,
            empty_copies(
                &mut timeline,
                &mut history,
                &mut composer,
                &mut search_draft,
                &mut memory,
                &mut tasks,
                &mut usage,
                &mut deletion,
                &mut chat_receipt,
            ),
        );
        assert!(result.wiped.contains(&ClientTempClass::PresentationBuffer));
        assert!(result.unverified.is_empty());
        assert!(timeline.is_empty());
        assert!(chat_receipt.is_none());
    }
}
