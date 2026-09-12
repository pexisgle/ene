//! `ene-ctl` subcommands: wire-payload builders and rendering.
//!
//! Pure: builds `ene-api` DTOs and renders views to display strings; no I/O,
//! sockets, or environment. Argument syntax lives in the `clap` command at
//! the crate root; transport lives in [`crate::client`]; exit-code mapping
//! lives at the crate root.
//!
//! Wire-mapping decisions (all within the existing DTO shapes):
//!
//! * Setup intents use the shared setup-target grammar ([`credential_target`]
//!   and [`consent_target`], never a CLI-local mini-language). The credential
//!   key comes from the Host process environment over the Host-local path,
//!   never this wire; assignment parameters travel in the consent target,
//!   never in the rationale quote, and both rationales are provenance-only.
//! * Both setup intents carry the display-revision mark of a freshly fetched
//!   setup view as `base_view`, so staleness is checked against something the
//!   CLI actually saw, never defaulted to unconstrained.
//! * `watch --round ROUND` prints that round's items from a [`HistoryRequest`]
//!   (same fetch as `history`, filtered by round); true stream-following needs
//!   a live `send` in the same process because streams cannot resume, so that
//!   follow mode is deferred (see [`Command::Watch`]).

use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome, ManagementView,
    ManagementViewRequest, RationaleOrigin, consent_target, credential_target,
};
use ene_api::v1::refs::{
    BaseViewMark, ClientLocalId, CommandWireId, CompanionWireRef, ManagementTargetWire,
    RoundWireId, TextLangWire,
};
use ene_api::v1::round::{
    HistoryItem, HistoryRequest, HistoryRole, RoundIntakeOutcomeWire, SubmitTextInput, TextBodyWire,
};

/// Fallback companion reference sent until the first presence fact arrives.
/// The Host only resolves projections it issued itself, so this fallback
/// revalidates (rather than silently attributing) until the session learns
/// the current projection from presence and echoes it back.
pub const DEFAULT_COMPANION_REF: &str = "default";

pub const DEFAULT_HISTORY_LIMIT: u64 = 50;

/// The Host registers refs as `"<provider>:<label>"` and falls back to the
/// `"<provider>:main"` ref before any consent exists, so the setup flow
/// always uses this label: the consent step can then name the credential id
/// it just created (see [`credential_id_for`]).
pub const SETUP_CREDENTIAL_LABEL: &str = "main";

/// Mirrors the Host setup section set: `HostHandle::build_view` in
/// `apps/ene-core/src/setup.rs` renders exactly these for a setup or status
/// request (an empty request selects the same set). The contract test below
/// asserts these names against that documented Host set, so a Host rename
/// fails the test instead of silently fetching nothing.
pub const HOST_SETUP_SECTIONS: &[&str] =
    &["provider", "model", "consent", "credential", "learning"];

/// The read-only Memory section rendered by the same Host view builder.
pub const HOST_MEMORY_SECTION: &str = "memory";

/// Only provider the setup flow knows how to assign yet.
pub const SETUP_PROVIDER_OPENAI: &str = "openai";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Setup(SetupMode),
    Status,
    Send(SendArgs),
    /// A round-scoped history print, not a live stream follow: streams cannot
    /// resume across processes, so following a stream needs a live `send` in
    /// the same process (deferred). Viewing restored facts is not presenting a
    /// stream, so `watch` never sends a presentation confirmation.
    Watch {
        round: String,
    },
    History {
        limit: u64,
    },
    /// Read-only Memory view: current recognition, scope, temporal meaning,
    /// and importance. `after` continues the current list from the `next:` id
    /// of the previous page; `revisions` selects one Memory's paged revision
    /// history (with `after_revision` continuing it).
    Memory {
        after: Option<String>,
        revisions: Option<String>,
        after_revision: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupMode {
    Show,
    Assign {
        provider: String,
        /// Passed through to the assignment record verbatim.
        model: String,
        /// Assign the route to the learning capability instead of the
        /// dialogue capability. Consent is per capability, so the Owner makes
        /// this choice explicitly.
        learning: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendArgs {
    /// Target round wire ref, or [`None`] to join-or-mint; never combined
    /// with [`fresh`](Self::fresh) (the parser rejects `--new --round`).
    pub round: Option<String>,
    /// Force a fresh round: the Host mints instead of joining any open round.
    pub fresh: bool,
    pub text: String,
}

pub fn setup_view_request() -> ManagementViewRequest {
    ManagementViewRequest {
        sections: HOST_SETUP_SECTIONS
            .iter()
            .map(|section| (*section).to_string())
            .collect(),
        memory_after: None,
        memory_revisions_of: None,
        memory_revisions_after: None,
    }
}

/// Requests only the read-only Memory section. `after` continues the current
/// list from a previous page; `revisions_of` selects one Memory's paged
/// revision history, continued by `after_revision`.
pub fn memory_view_request(
    after: Option<&str>,
    revisions_of: Option<&str>,
    after_revision: Option<u64>,
) -> ManagementViewRequest {
    ManagementViewRequest {
        sections: vec![HOST_MEMORY_SECTION.to_string()],
        memory_after: after.map(str::to_owned),
        memory_revisions_of: revisions_of.map(str::to_owned),
        memory_revisions_after: after_revision,
    }
}

pub fn history_request(companion: &str, limit: u64) -> HistoryRequest {
    HistoryRequest {
        companion: CompanionWireRef(companion.to_string()),
        since: None,
        limit,
        round: None,
    }
}

/// Round-scoped history: the Host filters by the stored round projection, so
/// the round is addressable even after a Host restart dropped its transient
/// wire map, and the result does not depend on the overall recent window.
pub fn round_history_request(companion: &str, round: &str, limit: u64) -> HistoryRequest {
    HistoryRequest {
        companion: CompanionWireRef(companion.to_string()),
        since: None,
        limit,
        round: Some(RoundWireId(round.to_string())),
    }
}

/// `companion` is the caller-learned projection echoed from presence
/// ([`DEFAULT_COMPANION_REF`] until the first fact).
pub fn submit_input(
    companion: &str,
    round: Option<String>,
    fresh: bool,
    text: String,
    lang: String,
) -> SubmitTextInput {
    SubmitTextInput {
        companion: CompanionWireRef(companion.to_string()),
        round: round.map(RoundWireId),
        fresh,
        local_id: new_local_id(),
        body: TextBodyWire {
            text,
            lang: TextLangWire(lang),
        },
    }
}

/// Mints a client-local correspondence ID from a v4 UUID: unique per
/// connection for this process, which is all `local_id` needs (it matches
/// acks to sends within one Client and is never Host-canonical).
pub fn new_local_id() -> ClientLocalId {
    ClientLocalId(uuid::Uuid::new_v4().to_string())
}

/// `"credential:<provider>:main"` via the shared [`credential_target`]
/// grammar (validation and remainder rules are never re-invented here); see
/// [`SETUP_CREDENTIAL_LABEL`].
pub fn credential_target_for(provider: &str) -> ManagementTargetWire {
    credential_target(provider, SETUP_CREDENTIAL_LABEL)
}

/// `"<provider>:main"`, matching the Host registry naming
/// (`"<provider>:<label>"`) and its pre-consent default ref.
pub fn credential_id_for(provider: &str) -> String {
    format!("{provider}:{SETUP_CREDENTIAL_LABEL}")
}

/// Wire name of the dialogue capability in the shared consent grammar.
pub const CAPABILITY_DIALOGUE: &str = "dialogue";

/// Wire name of the learning capability in the shared consent grammar.
pub const CAPABILITY_LEARNING: &str = "learning";

/// `"consent:<capability>:<provider>:<model>:<credential-id>"` via the shared
/// [`consent_target`] grammar (never re-invented here).
pub fn consent_target_for(capability: &str, provider: &str, model: &str) -> ManagementTargetWire {
    consent_target(capability, provider, model, &credential_id_for(provider))
}

/// The Host sources the key from its own environment over the Host-local
/// path, so this payload carries no secret; the rationale is provenance-only
/// (origin, no quote).
pub fn credential_intent(
    intent_id: CommandWireId,
    base: &BaseViewMark,
    provider: &str,
) -> ManagementIntent {
    ManagementIntent {
        intent_id,
        kind: ManagementIntentKind::ConfigureCredentialIntent,
        target: credential_target_for(provider),
        base_view: base.clone(),
        rationale: IntentRationaleWire {
            origin: RationaleOrigin::ManagementSurface,
            quote: None,
        },
    }
}

/// Provenance-only rationale (origin, no quote): assignment parameters travel
/// in the consent target, never in the quote. The capability is explicit so a
/// dialogue assignment can never stand in for learning.
pub fn assignment_intent(
    intent_id: CommandWireId,
    base: &BaseViewMark,
    capability: &str,
    provider: &str,
    model: &str,
) -> ManagementIntent {
    ManagementIntent {
        intent_id,
        kind: ManagementIntentKind::ManageRuleConsentCap,
        target: consent_target_for(capability, provider, model),
        base_view: base.clone(),
        rationale: IntentRationaleWire {
            origin: RationaleOrigin::ManagementSurface,
            quote: None,
        },
    }
}

/// Renders one `kind: title – body` line per section, in Host order.
///
/// Prints exactly what the Host-filtered view contains and nothing else: no
/// revision marks, no envelope IDs, no `Debug` dumps. Body text is
/// Host-filtered display fact; secrecy is a Host property, and this adds no
/// secret-bearing surface of its own.
pub fn render_view(view: &ManagementView) -> String {
    view.sections
        .iter()
        .map(|section| format!("{}: {} – {}", section.kind, section.title, section.body))
        .collect::<Vec<String>>()
        .join("\n")
}

pub fn role_label(role: HistoryRole) -> &'static str {
    match role {
        HistoryRole::Owner => "owner",
        HistoryRole::Companion => "companion",
    }
}

/// One `[role] text` line per item, oldest first.
pub fn render_history(items: &[HistoryItem]) -> String {
    items
        .iter()
        .map(|item| format!("[{}] {}", role_label(item.role), item.text))
        .collect::<Vec<String>>()
        .join("\n")
}

/// Intake-routing decision for a [`RoundIntakeOutcomeWire`]; decline messages
/// carry refs and generations only, never body text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntakeAction {
    Accepted { round: String },
    Declined { message: String },
}

pub fn describe_intake(outcome: &RoundIntakeOutcomeWire) -> IntakeAction {
    match outcome {
        RoundIntakeOutcomeWire::AcceptedForRound { round } => IntakeAction::Accepted {
            round: round.0.clone(),
        },
        RoundIntakeOutcomeWire::StaleRound {
            current_round,
            current_generation,
        } => {
            let message = match current_round {
                Some(round) => format!(
                    "stale round; current round is {} at generation {current_generation}",
                    round.0
                ),
                None => format!("stale round; no round is open (generation {current_generation})"),
            };
            IntakeAction::Declined { message }
        }
        RoundIntakeOutcomeWire::HeldForTransition => IntakeAction::Declined {
            message: String::from(
                "held for a presence transition; retry after the transition settles",
            ),
        },
        RoundIntakeOutcomeWire::NeedsRevalidation { reason } => IntakeAction::Declined {
            message: format!("needs revalidation: {}", reason.0),
        },
    }
}

/// Management-routing decision for a [`ManagementOutcome`]; `detail`/`message`
/// lines carry operational facts only, never bodies or secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagementAction {
    Applied { detail: String },
    Retryable { message: String },
    Terminal { message: String },
}

pub fn describe_management(outcome: &ManagementOutcome) -> ManagementAction {
    match outcome {
        ManagementOutcome::AppliedAsOneTime => ManagementAction::Applied {
            detail: String::from("applied as a one-time approval"),
        },
        ManagementOutcome::StoredAsRuleView { revision } => ManagementAction::Applied {
            detail: format!("stored as a rule at revision {}", revision.0),
        },
        ManagementOutcome::NeedsClarification => ManagementAction::Terminal {
            message: String::from("needs clarification; refine the request and retry"),
        },
        ManagementOutcome::DeniedByBoundary => ManagementAction::Terminal {
            message: String::from("denied by the control boundary"),
        },
        ManagementOutcome::StaleBaseView { current } => ManagementAction::Retryable {
            message: format!("stale base view; current mark is {}", current.0),
        },
        ManagementOutcome::HeldByOperation => ManagementAction::Retryable {
            message: String::from("held by a concurrent operation; retry later"),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use ene_api::v1::management::{ManagementOutcome, ManagementView, ViewSection};
    use ene_api::v1::refs::{BaseViewMark, CommandWireId, ViewMarkWire};
    use ene_api::v1::refs::{RevalidationReasonWire, RoundWireId};
    use ene_api::v1::round::{HistoryItem, HistoryRole, RoundIntakeOutcomeWire};

    use super::{
        CAPABILITY_DIALOGUE, CAPABILITY_LEARNING, HOST_MEMORY_SECTION, HOST_SETUP_SECTIONS,
        SETUP_PROVIDER_OPENAI, assignment_intent, consent_target_for, credential_id_for,
        credential_intent, credential_target_for, describe_intake, describe_management,
        history_request, memory_view_request, new_local_id, render_history, render_view,
        round_history_request, setup_view_request, submit_input,
    };
    use super::{IntakeAction, ManagementAction};

    fn fixture_view() -> ManagementView {
        ManagementView {
            mark: ViewMarkWire(String::from("MARKER-MUST-NOT-APPEAR-7f3a")),
            sections: vec![
                ViewSection {
                    kind: String::from("setup"),
                    title: String::from("Setup status"),
                    body: String::from("provider openai ready"),
                },
                ViewSection {
                    kind: String::from("usage"),
                    title: String::from("Usage"),
                    body: String::from("3 rounds today"),
                },
            ],
        }
    }

    #[test]
    fn render_view_uses_kind_title_body_lines() {
        let rendered = render_view(&fixture_view());
        assert!(
            rendered
                == "setup: Setup status – provider openai ready\nusage: Usage – 3 rounds today",
            "view must render one `kind: title – body` line per section, got {rendered:?}"
        );
    }

    #[test]
    fn render_view_emits_nothing_but_sections() {
        let rendered = render_view(&fixture_view());
        assert!(
            !rendered.contains("MARKER-MUST-NOT-APPEAR-7f3a"),
            "the revision mark must not be echoed: {rendered:?}"
        );
        assert!(
            !rendered.contains("ManagementView"),
            "no Debug dumps may appear: {rendered:?}"
        );
    }

    #[test]
    fn render_view_of_no_sections_is_empty() {
        let view = ManagementView {
            mark: ViewMarkWire(String::from("mark-1")),
            sections: Vec::new(),
        };
        assert!(
            render_view(&view).is_empty(),
            "no sections must render to nothing"
        );
    }

    fn fixture_history() -> Vec<HistoryItem> {
        vec![
            HistoryItem {
                round: RoundWireId(String::from("round-1")),
                role: HistoryRole::Owner,
                text: String::from("first words"),
                at: String::from("2026-09-08T12:00:00+09:00"),
            },
            HistoryItem {
                round: RoundWireId(String::from("round-2")),
                role: HistoryRole::Companion,
                text: String::from("second words"),
                at: String::from("2026-09-08T12:01:00+09:00"),
            },
        ]
    }

    #[test]
    fn render_history_uses_role_text_lines() {
        let rendered = render_history(&fixture_history());
        assert!(
            rendered == "[owner] first words\n[companion] second words",
            "history must render `[role] text` lines, got {rendered:?}"
        );
    }

    #[test]
    fn describe_intake_accepted_carries_the_round() {
        let action = describe_intake(&RoundIntakeOutcomeWire::AcceptedForRound {
            round: RoundWireId(String::from("round-3")),
        });
        assert!(
            action
                == IntakeAction::Accepted {
                    round: String::from("round-3"),
                },
            "acceptance must carry the round, got {action:?}"
        );
    }

    #[test]
    fn describe_intake_decline_names_refs_not_bodies() {
        let stale = describe_intake(&RoundIntakeOutcomeWire::StaleRound {
            current_round: Some(RoundWireId(String::from("round-4"))),
            current_generation: 9,
        });
        let IntakeAction::Declined { message } = stale else {
            return;
        };
        assert!(
            message.contains("round-4") && message.contains('9'),
            "stale must name the current round and generation: {message:?}"
        );
        let empty = describe_intake(&RoundIntakeOutcomeWire::StaleRound {
            current_round: None,
            current_generation: 9,
        });
        assert!(
            matches!(empty, IntakeAction::Declined { .. }),
            "stale without a current round is still a decline"
        );
        let held = describe_intake(&RoundIntakeOutcomeWire::HeldForTransition);
        assert!(
            matches!(held, IntakeAction::Declined { .. }),
            "held must decline, got {held:?}"
        );
        let revalidation = describe_intake(&RoundIntakeOutcomeWire::NeedsRevalidation {
            reason: RevalidationReasonWire(String::from("reason-1")),
        });
        let IntakeAction::Declined { message } = revalidation else {
            return;
        };
        assert!(
            message.contains("reason-1"),
            "revalidation must name the reason code: {message:?}"
        );
    }

    #[test]
    fn describe_management_splits_applied_retryable_terminal() {
        assert!(
            matches!(
                describe_management(&ManagementOutcome::AppliedAsOneTime),
                ManagementAction::Applied { .. }
            ),
            "one-time approval is applied"
        );
        let stored = describe_management(&ManagementOutcome::StoredAsRuleView {
            revision: ViewMarkWire(String::from("rev-2")),
        });
        let ManagementAction::Applied { detail } = stored else {
            return;
        };
        assert!(
            detail.contains("rev-2"),
            "stored rule must name the revision: {detail:?}"
        );
        assert!(
            matches!(
                describe_management(&ManagementOutcome::StaleBaseView {
                    current: ViewMarkWire(String::from("rev-3")),
                }),
                ManagementAction::Retryable { .. }
            ),
            "stale base is retryable"
        );
        assert!(
            matches!(
                describe_management(&ManagementOutcome::HeldByOperation),
                ManagementAction::Retryable { .. }
            ),
            "held is retryable"
        );
        assert!(
            matches!(
                describe_management(&ManagementOutcome::NeedsClarification),
                ManagementAction::Terminal { .. }
            ),
            "clarification is terminal"
        );
        assert!(
            matches!(
                describe_management(&ManagementOutcome::DeniedByBoundary),
                ManagementAction::Terminal { .. }
            ),
            "denial is terminal"
        );
    }

    #[test]
    fn request_builders_use_the_bootstrap_companion() {
        let documented = ["provider", "model", "consent", "credential", "learning"];
        assert!(
            HOST_SETUP_SECTIONS == documented,
            "the requested sections must match the documented Host set: {HOST_SETUP_SECTIONS:?}"
        );
        let setup = setup_view_request();
        assert!(
            setup.sections
                == documented
                    .iter()
                    .map(|section| (*section).to_string())
                    .collect::<Vec<String>>(),
            "setup --show requests the Host sections: {setup:?}"
        );
        let memory = memory_view_request(None, None, None);
        assert!(
            memory.sections == vec![HOST_MEMORY_SECTION.to_string()]
                && memory.memory_after.is_none(),
            "memory requests exactly the read-only Memory section: {memory:?}"
        );
        let paged = memory_view_request(Some("memory-1"), None, None);
        assert_eq!(
            paged.memory_after,
            Some(String::from("memory-1")),
            "the page cursor rides the typed request field"
        );
        let revisions = memory_view_request(None, Some("memory-2"), Some(20));
        assert_eq!(
            revisions.memory_revisions_of,
            Some(String::from("memory-2")),
            "the revision selector rides its own typed field"
        );
        assert_eq!(
            revisions.memory_revisions_after,
            Some(20),
            "the revision cursor rides its own typed field"
        );
        assert!(
            revisions.memory_after.is_none(),
            "a revision request is not a list page: {revisions:?}"
        );
        let history = history_request("companion-1", 7);
        assert!(
            history.companion.0 == "companion-1" && history.limit == 7,
            "history echoes the learned companion: {history:?}"
        );
        assert!(
            history.round.is_none(),
            "plain history reads the whole timeline: {history:?}"
        );
        let scoped = round_history_request("companion-1", "round-9", 7);
        assert!(
            scoped.round == Some(RoundWireId(String::from("round-9"))),
            "round-scoped history carries the projection: {scoped:?}"
        );
        let input = submit_input(
            "companion-1",
            Some(String::from("round-1")),
            false,
            String::from("hello"),
            String::from("en"),
        );
        assert!(
            input.companion.0 == "companion-1",
            "input echoes the learned companion: {input:?}"
        );
        let round = input.round.as_ref().unwrap();
        assert!(
            round.0 == "round-1",
            "input keeps the premise round: {input:?}"
        );
        let fresh = submit_input(
            "companion-1",
            None,
            true,
            String::from("hello"),
            String::from("en"),
        );
        assert!(
            fresh.round.is_none(),
            "no premise round keeps no hint: {fresh:?}"
        );
        assert!(
            fresh.fresh,
            "the force flag must travel to the wire: {fresh:?}"
        );
    }

    #[test]
    fn setup_targets_use_the_shared_grammar() {
        assert!(
            credential_target_for("openai").0 == "credential:openai:main",
            "credential target spells the shared grammar"
        );
        assert!(
            credential_id_for("openai") == "openai:main",
            "credential id names the registry ref the register step creates"
        );
        assert!(
            consent_target_for(CAPABILITY_DIALOGUE, "openai", "gpt-x").0
                == "consent:dialogue:openai:gpt-x:openai:main",
            "dialogue consent target spells the shared grammar over that credential id"
        );
        assert!(
            consent_target_for(CAPABILITY_LEARNING, "openai", "gpt-x").0
                == "consent:learning:openai:gpt-x:openai:main",
            "learning consent target is capability-distinct"
        );
        // Roundtrip through the shared parsers: the builders never bypass
        // Host-side validation.
        assert!(
            ene_api::v1::management::parse_credential_target(&credential_target_for("openai"))
                == Some((String::from("openai"), String::from("main"))),
            "credential target must parse as (provider, label)"
        );
        assert!(
            ene_api::v1::management::parse_consent_target(&consent_target_for(
                CAPABILITY_DIALOGUE,
                "openai",
                "gpt-x"
            )) == Some((
                String::from("dialogue"),
                String::from("openai"),
                String::from("gpt-x"),
                String::from("openai:main"),
            )),
            "consent target must parse as (capability, provider, model, credential-id)"
        );
    }

    #[test]
    fn setup_intents_carry_grammar_targets_and_provenance_only_rationales() {
        use ene_api::v1::management::ManagementIntentKind;
        use ene_api::v1::management::RationaleOrigin;

        let base = BaseViewMark(String::from("mark-1"));
        let credential = credential_intent(
            CommandWireId(uuid::Uuid::new_v4()),
            &base,
            SETUP_PROVIDER_OPENAI,
        );
        assert!(
            credential.kind == ManagementIntentKind::ConfigureCredentialIntent
                && credential.target.0 == "credential:openai:main"
                && credential.base_view == base
                && credential.rationale.origin == RationaleOrigin::ManagementSurface
                && credential.rationale.quote.is_none(),
            "credential intent carries the grammar target and no quote: {credential:?}"
        );
        let assignment = assignment_intent(
            CommandWireId(uuid::Uuid::new_v4()),
            &base,
            CAPABILITY_DIALOGUE,
            SETUP_PROVIDER_OPENAI,
            "gpt-x",
        );
        assert!(
            assignment.kind == ManagementIntentKind::ManageRuleConsentCap
                && assignment.target.0 == "consent:dialogue:openai:gpt-x:openai:main"
                && assignment.base_view == base
                && assignment.rationale.origin == RationaleOrigin::ManagementSurface
                && assignment.rationale.quote.is_none(),
            "assignment intent carries the consent target and no quote: {assignment:?}"
        );
        let learning = assignment_intent(
            CommandWireId(uuid::Uuid::new_v4()),
            &base,
            CAPABILITY_LEARNING,
            SETUP_PROVIDER_OPENAI,
            "gpt-x",
        );
        assert!(
            learning.target.0 == "consent:learning:openai:gpt-x:openai:main",
            "learning assignment names its own capability: {learning:?}"
        );
    }

    #[test]
    fn local_ids_are_unique_across_many_draws() {
        let mut seen = HashSet::new();
        for _ in 0..1000 {
            seen.insert(new_local_id().0);
        }
        assert!(
            seen.len() == 1000,
            "1000 local IDs must all be distinct, got {}",
            seen.len()
        );
    }

    #[test]
    fn local_id_is_a_uuid() {
        let id = new_local_id().0;
        assert!(
            uuid::Uuid::parse_str(&id).is_ok(),
            "local ID must be a UUID: {id:?}"
        );
    }
}
