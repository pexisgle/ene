//! GUI erasure participant: timeline, draft, IME composition.
//!
//! Registered secrets are not user content and are not wiped through this
//! path. C1 secret intake is zeroized by crate-private `SecretIntake`, not
//! by Targeted Deletion.

use ene_api::v1::deletion::{
    ClientTempClass, DeletionDemand, DeletionTargetWire, LocalErasureResult,
};
use ene_api::v1::round::HistoryItem;

use crate::ui::Composer;

/// Wipes GUI copies named by a Host deletion demand. Returns `wiped` only
/// for copies this process actually cleared.
pub fn apply_demand(
    demand: &DeletionDemand,
    timeline: &mut Vec<String>,
    history: &mut Vec<HistoryItem>,
    composer: &mut Composer,
) -> LocalErasureResult {
    let mut wiped = Vec::new();
    let mut unverified = Vec::new();
    for target in &demand.targets {
        match target {
            DeletionTargetWire::WipeClass {
                class: ClientTempClass::InputDraft,
            } => {
                composer.wipe();
                wiped.push(ClientTempClass::InputDraft);
            }
            DeletionTargetWire::WipeClass {
                class: ClientTempClass::PresentationBuffer,
            } => {
                timeline.clear();
                history.clear();
                wiped.push(ClientTempClass::PresentationBuffer);
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

#[cfg(test)]
mod tests {
    use super::apply_demand;
    use crate::ui::Composer;
    use ene_api::v1::deletion::{
        ClientTempClass, DeletionDemand, DeletionDemandWireId, DeletionTargetWire,
    };
    use ene_api::v1::refs::DeletionOperationWireRef;

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
        let result = apply_demand(&demand, &mut timeline, &mut history, &mut composer);
        assert!(result.wiped.contains(&ClientTempClass::InputDraft));
        assert!(composer.draft().is_empty());
        assert_eq!(timeline, vec![String::from("hello")]);
    }
}
