use serde::{Deserialize, Serialize};

use super::refs::{
    DeletionOperationWireRef, DeletionStatusCursorWire, ManagementTargetWire, ViewMarkWire,
    string_wire_ref,
};

pub const DELETION_TARGET_PREFIX: &str = "deletion:";
pub const DELETION_EXACT_TEXT_MAX_BYTES: usize = 1024;
pub const DELETION_STATUS_CURSOR_PREFIX: &str = "deletion-status:";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeletionPurposeWire {
    Privacy,
    Security,
}

impl DeletionPurposeWire {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Privacy => "privacy",
            Self::Security => "security",
        }
    }

    #[must_use]
    pub fn from_name(token: &str) -> Option<Self> {
        match token {
            "privacy" => Some(Self::Privacy),
            "security" => Some(Self::Security),
            _ => None,
        }
    }
}

#[must_use]
pub fn deletion_target(purpose: DeletionPurposeWire, exact_text: &str) -> ManagementTargetWire {
    ManagementTargetWire(format!(
        "{DELETION_TARGET_PREFIX}{}:{exact_text}",
        purpose.as_str()
    ))
}

#[must_use]
pub fn parse_deletion_target(target: &ManagementTargetWire) -> Option<DeletionTargetRequest> {
    let rest = target.0.strip_prefix(DELETION_TARGET_PREFIX)?;
    let (purpose, exact_text) = rest.split_once(':')?;
    let purpose = DeletionPurposeWire::from_name(purpose)?;
    if exact_text.is_empty() || exact_text.len() > DELETION_EXACT_TEXT_MAX_BYTES {
        return None;
    }
    Some(DeletionTargetRequest {
        purpose,
        exact_text: exact_text.to_owned(),
    })
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct DeletionTargetRequest {
    purpose: DeletionPurposeWire,
    exact_text: String,
}

impl DeletionTargetRequest {
    #[must_use]
    pub fn purpose(&self) -> DeletionPurposeWire {
        self.purpose
    }

    #[must_use]
    pub fn exact_text(&self) -> &str {
        &self.exact_text
    }
}

impl core::fmt::Debug for DeletionTargetRequest {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("DeletionTargetRequest")
            .field("purpose", &self.purpose)
            .field("exact_text", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletionStatusRequest {
    pub cursor: Option<DeletionStatusCursorWire>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeletionStatusResponse {
    Page(DeletionStatusPage),
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletionStatusPage {
    pub mark: ViewMarkWire,
    pub operations: Vec<DeletionOperationStatusView>,
    pub next_cursor: Option<DeletionStatusCursorWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletionOperationStatusView {
    pub operation: DeletionOperationWireRef,
    pub phase: DeletionPhaseWire,
    pub purpose: DeletionPurposeWire,
    pub started_at: String,
    pub sweep: u64,
    pub hold: Option<DeletionHoldWire>,
    pub participants: DeletionParticipantReportWire,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeletionPhaseWire {
    Active,
    Held,
    Finalizing,
    Completed,
}

impl DeletionPhaseWire {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Held => "held",
            Self::Finalizing => "finalizing",
            Self::Completed => "completed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeletionHoldWire {
    Unavailable,
    GenerationExhausted,
}

impl DeletionHoldWire {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::GenerationExhausted => "generation-exhausted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeletionParticipantReportWire {
    NotReported,
    Reported(Vec<DeletionParticipantStatusWire>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletionParticipantStatusWire {
    pub owner: String,
    pub progress: String,
    pub sweep: u64,
}

string_wire_ref!(DeletionDemandWireId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClientTempClass {
    PresentationBuffer,
    InputDraft,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeletionTargetWire {
    WipeClass { class: ClientTempClass },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletionDemand {
    pub demand: DeletionDemandWireId,
    pub operation: DeletionOperationWireRef,
    pub sweep: u64,
    pub targets: Vec<DeletionTargetWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalErasureResult {
    pub demand: DeletionDemandWireId,
    pub operation: DeletionOperationWireRef,
    pub sweep: u64,
    pub wiped: Vec<ClientTempClass>,
    pub unverified: Vec<ClientTempClass>,
}

#[cfg(test)]
mod tests {
    use super::super::refs::ManagementTargetWire;
    use super::{
        ClientTempClass, DELETION_EXACT_TEXT_MAX_BYTES, DELETION_TARGET_PREFIX, DeletionPhaseWire,
        DeletionPurposeWire, deletion_target, parse_deletion_target,
    };

    #[test]
    fn deletion_target_roundtrips_with_colons_and_unicode() {
        for text in [
            "secret",
            "with:colons:inside",
            "日本語のキーワード",
            " leading and trailing ",
        ] {
            let target = deletion_target(DeletionPurposeWire::Privacy, text);
            let parsed = parse_deletion_target(&target).expect("the built target must parse");
            assert_eq!(parsed.purpose(), DeletionPurposeWire::Privacy);
            assert_eq!(parsed.exact_text(), text);
        }
        let security = deletion_target(DeletionPurposeWire::Security, "leaked-key");
        assert_eq!(security.0.as_str(), "deletion:security:leaked-key");
        assert_eq!(
            parse_deletion_target(&security).map(|request| request.purpose()),
            Some(DeletionPurposeWire::Security)
        );
    }

    #[test]
    fn deletion_target_parser_rejects_other_families_and_shapes() {
        for raw in [
            "deletion:",
            "deletion:privacy",
            "deletion:privacy:",
            "deletion:unknown:secret",
            "deletion::secret",
            "credential:openai:personal",
            "setup:show",
            "task:not-a-uuid",
            "",
        ] {
            let target = ManagementTargetWire(String::from(raw));
            assert!(
                parse_deletion_target(&target).is_none(),
                "the deletion grammar rejects {raw:?}"
            );
        }
        assert_eq!(DELETION_TARGET_PREFIX, "deletion:");
    }

    #[test]
    fn deletion_target_bounds_the_exact_text() {
        let at_limit = "x".repeat(DELETION_EXACT_TEXT_MAX_BYTES);
        assert!(
            parse_deletion_target(&deletion_target(DeletionPurposeWire::Privacy, &at_limit))
                .is_some(),
            "a target at the bound is admissible"
        );
        let over = "x".repeat(DELETION_EXACT_TEXT_MAX_BYTES + 1);
        assert!(
            parse_deletion_target(&deletion_target(DeletionPurposeWire::Privacy, &over)).is_none(),
            "a target over the bound is refused, never truncated"
        );
    }

    #[test]
    fn parsed_request_debug_redacts_the_owner_body() {
        let request =
            parse_deletion_target(&deletion_target(DeletionPurposeWire::Privacy, "raw secret"))
                .expect("the built target must parse");
        let rendered = format!("{request:?}");
        assert!(
            !rendered.contains("raw secret"),
            "the exact text never renders through Debug: {rendered}"
        );
        assert!(
            rendered.contains("Privacy"),
            "the purpose stays visible: {rendered}"
        );
        assert!(
            !rendered.to_lowercase().contains("exact_text: \"raw"),
            "the body is redacted: {rendered}"
        );
    }

    #[test]
    fn phase_tokens_are_stable() {
        assert_eq!(DeletionPhaseWire::Active.as_str(), "active");
        assert_eq!(DeletionPhaseWire::Held.as_str(), "held");
        assert_eq!(DeletionPhaseWire::Finalizing.as_str(), "finalizing");
        assert_eq!(DeletionPhaseWire::Completed.as_str(), "completed");
    }

    #[test]
    fn demand_and_result_round_trip_without_a_target_body() {
        use super::super::refs::DeletionOperationWireRef;
        use super::{DeletionDemand, DeletionDemandWireId, DeletionTargetWire, LocalErasureResult};

        let demand = DeletionDemand {
            demand: DeletionDemandWireId(String::from("demand-1")),
            operation: DeletionOperationWireRef(String::from("operation-1")),
            sweep: 2,
            targets: vec![
                DeletionTargetWire::WipeClass {
                    class: ClientTempClass::PresentationBuffer,
                },
                DeletionTargetWire::WipeClass {
                    class: ClientTempClass::InputDraft,
                },
            ],
        };
        let payload = super::super::payload::WirePayload::DeletionDemand(demand.clone());
        assert_eq!(payload.message_type(), "DeletionDemand");
        let json = serde_json::to_string(&payload).expect("the demand must serialize");
        assert!(
            !json.to_lowercase().contains("deletion:"),
            "the wire never carries the mechanical target grammar: {json}"
        );
        assert!(
            !json.contains("target-body"),
            "no body can ride a demand: {json}"
        );
        let result = LocalErasureResult {
            demand: demand.demand.clone(),
            operation: demand.operation.clone(),
            sweep: demand.sweep,
            wiped: vec![ClientTempClass::PresentationBuffer],
            unverified: vec![ClientTempClass::InputDraft],
        };
        let payload = super::super::payload::WirePayload::LocalErasureResult(result.clone());
        assert_eq!(payload.message_type(), "LocalErasureResult");
        let round_trip: LocalErasureResult =
            serde_json::from_str(&serde_json::to_string(&result).expect("serializes"))
                .expect("round-trips");
        assert_eq!(round_trip, result);
        assert!(!format!("{demand:?}").contains("target-body"));
    }
}
