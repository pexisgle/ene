//! `ene-ctl` subcommands: argument parsing, wire-payload builders, rendering.
//!
//! This module is pure: it parses `argv` words, builds `ene-api` DTOs, and
//! renders Host-filtered views to display strings. It performs no I/O, opens
//! no sockets, and reads no environment. Transport lives in
//! [`crate::client`]; exit-code mapping lives at the crate root.
//!
//! Wire-mapping decisions (all within the existing DTO shapes):
//!
//! * Companion reference: every request echoes the projection the session
//!   learned from presence ([`DEFAULT_COMPANION_REF`] (`"default"`) until
//!   the first fact). The Host resolves inbound refs through its own
//!   mapping — exact match against the issued projection, never derived —
//!   so a rotated or guessed ref revalidates instead of attributing.
//! * `setup --provider openai --model MODEL` performs two intents in the
//!   shared setup-target grammar
//!   ([`credential_target`] and
//!   [`consent_target`], never a
//!   CLI-local mini-language). The credential step uses
//!   [`ManagementIntentKind::ConfigureCredentialIntent`]
//!   with [`credential_target_for`] (`"credential:<provider>:main"`; the key
//!   itself comes from the Host process environment over the Host-local path,
//!   never this wire). Provider/model assignment uses
//!   [`ManagementIntentKind::ManageRuleConsentCap`] with
//!   [`consent_target_for`] (`"consent:<provider>:<model>:<credential-id>"`,
//!   where the credential id is the `"<provider>:main"` ref the register step
//!   created). Both rationales are provenance-only
//!   ([`RationaleOrigin::ManagementSurface`], `quote` [`None`]): assignment
//!   parameters travel in the consent target, never in the quote.
//! * Both setup intents carry the display-revision mark of a freshly fetched
//!   setup view as their `base_view`; staleness is therefore checked against
//!   something the CLI actually saw, never defaulted to unconstrained.
//! * `setup --show` and `status` both request [`HOST_SETUP_SECTIONS`] — the
//!   four Host sections (`provider`, `model`, `consent`, `credential`).
//! * `watch --round ROUND` prints that round's items from a
//!   [`HistoryRequest`] (same fetch as
//!   `history`, filtered by round). True stream-following needs a live `send`
//!   in the same process because streams cannot resume; that follow mode is
//!   deferred, and this limitation is documented on [`Command::Watch`].
//!   Viewing restored facts is not presenting a stream, so `watch` never
//!   sends [`ConfirmPresentation`](ene_api::v1::round::ConfirmPresentationWire).
//!
//! [`ManagementIntentKind::ConfigureCredentialIntent`]: ene_api::v1::management::ManagementIntentKind::ConfigureCredentialIntent
//! [`ManagementIntentKind::ManageRuleConsentCap`]: ene_api::v1::management::ManagementIntentKind::ManageRuleConsentCap

use std::sync::atomic::{AtomicU64, Ordering};

use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome, ManagementView,
    ManagementViewRequest, RationaleOrigin, consent_target, credential_target,
};
use ene_api::v1::refs::{
    BaseViewMark, ClientLocalId, CommandWireId, CompanionWireRef, ManagementTargetWire,
    RoundWireId, TextLangWire,
};
use ene_api::v1::round::{
    HistoryRequest, HistoryRole, HistoryView, RoundIntakeOutcomeWire, SubmitTextInput, TextBodyWire,
};

use crate::errors::{CliError, USAGE};

/// Fallback companion reference sent until the first presence fact arrives.
/// The Host only resolves projections it issued itself, so this fallback
/// revalidates (rather than silently attributing) until the session learns
/// the current projection from presence and echoes it back.
pub const DEFAULT_COMPANION_REF: &str = "default";

/// Default item cap for `history` when `--limit` is absent.
pub const DEFAULT_HISTORY_LIMIT: u64 = 50;

/// Label of the conventional main credential the setup flow registers.
///
/// The Host registers refs as `"<provider>:<label>"` and falls back to the
/// `"<provider>:main"` ref before any consent exists, so the setup flow
/// always uses this label: the consent step can then name the credential id
/// it just created (see [`credential_id_for`]).
pub const SETUP_CREDENTIAL_LABEL: &str = "main";

/// Management-view sections the setup and status flows request.
///
/// This mirrors the Host section set: `HostHandle::build_view` in
/// `apps/ene-core/src/setup.rs` renders exactly `provider`, `model`,
/// `consent`, and `credential` (an empty request selects the same four).
/// The contract test below asserts these names against that documented Host
/// set, so a Host rename fails the test instead of silently fetching nothing.
pub const HOST_SETUP_SECTIONS: &[&str] = &["provider", "model", "consent", "credential"];

/// Only provider the setup flow knows how to assign yet.
pub const SETUP_PROVIDER_OPENAI: &str = "openai";

/// Counter backing [`new_local_id`]: process-local, monotonically increasing.
static LOCAL_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Parsed subcommand with its operands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Setup flow: show the filtered setup view, or register-then-assign.
    Setup(SetupMode),
    /// Render the `provider`/`model`/`consent`/`credential` view sections.
    Status,
    /// Submit text and stream the answering round.
    Send(SendArgs),
    /// Print one round's items from history.
    ///
    /// Honest limitation: this is a round-scoped history print, not a live
    /// stream follow. Streams cannot resume across processes, so following a
    /// stream needs a live `send` in the same process; that follow mode is
    /// deferred. Viewing restored facts is not presenting a stream, so
    /// `watch` never sends a presentation confirmation.
    Watch {
        /// Round whose items to print.
        round: String,
    },
    /// Print recent timeline items, newest request bounded by `limit`.
    History {
        /// Maximum items to request.
        limit: u64,
    },
}

/// `setup` mode: filtered-view display, or credential-plus-assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupMode {
    /// Fetch and render the `setup` view section.
    Show,
    /// Register the credential (Host-sourced key), then assign provider/model.
    Assign {
        /// Provider name; only `"openai"` is accepted.
        provider: String,
        /// Model name, passed through to the assignment record verbatim.
        model: String,
    },
}

/// `send` operands: optional premise round plus message text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendArgs {
    /// Target round, or [`None`] for a new round (`--new` forces [`None`]).
    pub round: Option<String>,
    /// Message text: remaining words joined with single spaces.
    pub text: String,
}

/// Parses the words after the global flags: subcommand name plus operands.
///
/// An empty slice, an unknown name, or malformed operands are all
/// [`CliError::Usage`] whose message ends with the usage text.
pub fn parse_command(words: &[String]) -> Result<Command, CliError> {
    let Some((name, rest)) = words.split_first() else {
        return Err(CliError::Usage(format!("missing command\n{USAGE}")));
    };
    match name.as_str() {
        "setup" => parse_setup(rest).map(Command::Setup),
        "status" => parse_status(rest),
        "send" => parse_send(rest).map(Command::Send),
        "watch" => parse_watch(rest),
        "history" => parse_history(rest),
        other => Err(CliError::Usage(format!(
            "unknown command: {other}\n{USAGE}"
        ))),
    }
}

/// Parses `setup` operands: bare words are rejected, `--show` stands alone,
/// and `--provider`/`--model` must appear together.
fn parse_setup(args: &[String]) -> Result<SetupMode, CliError> {
    let mut show = false;
    let mut provider: Option<String> = None;
    let mut model: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--show" => {
                show = true;
                index += 1;
            }
            "--provider" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(CliError::Usage(format!(
                        "missing value for --provider\n{USAGE}"
                    )));
                };
                provider = Some(value.clone());
                index += 1;
            }
            "--model" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(CliError::Usage(format!(
                        "missing value for --model\n{USAGE}"
                    )));
                };
                model = Some(value.clone());
                index += 1;
            }
            other => {
                return Err(CliError::Usage(format!(
                    "unknown argument: {other}\n{USAGE}"
                )));
            }
        }
    }
    if show {
        if provider.is_some() || model.is_some() {
            return Err(CliError::Usage(format!(
                "setup --show takes no other flags\n{USAGE}"
            )));
        }
        return Ok(SetupMode::Show);
    }
    match (provider, model) {
        (Some(name), Some(model_name)) => {
            if name != SETUP_PROVIDER_OPENAI {
                return Err(CliError::Usage(format!(
                    "unsupported provider: {name} (only openai)\n{USAGE}"
                )));
            }
            if model_name.is_empty() {
                return Err(CliError::Usage(format!("model must not be empty\n{USAGE}")));
            }
            Ok(SetupMode::Assign {
                provider: name,
                model: model_name,
            })
        }
        (None, None) => Err(CliError::Usage(format!(
            "setup requires --show or --provider openai --model MODEL\n{USAGE}"
        ))),
        _ => Err(CliError::Usage(format!(
            "--provider and --model must be given together\n{USAGE}"
        ))),
    }
}

/// Parses `status`: takes no operands.
fn parse_status(args: &[String]) -> Result<Command, CliError> {
    if let Some(extra) = args.first() {
        return Err(CliError::Usage(format!(
            "unknown argument: {extra}\n{USAGE}"
        )));
    }
    Ok(Command::Status)
}

/// Parses `send [--new | --round ROUND] TEXT...`: flags first in any order,
/// then one or more text words. A word starting with `--` is never treated
/// as text; such input is a usage error.
fn parse_send(args: &[String]) -> Result<SendArgs, CliError> {
    let mut fresh = false;
    let mut round: Option<String> = None;
    let mut text: Vec<&str> = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--new" => {
                fresh = true;
                index += 1;
            }
            "--round" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(CliError::Usage(format!(
                        "missing value for --round\n{USAGE}"
                    )));
                };
                round = Some(value.clone());
                index += 1;
            }
            word if word.starts_with("--") => {
                return Err(CliError::Usage(format!(
                    "unknown argument: {word}\n{USAGE}"
                )));
            }
            word => {
                text.push(word);
                index += 1;
            }
        }
    }
    if fresh && round.is_some() {
        return Err(CliError::Usage(format!(
            "--new and --round must not be combined\n{USAGE}"
        )));
    }
    if text.is_empty() {
        return Err(CliError::Usage(format!(
            "send requires message text\n{USAGE}"
        )));
    }
    Ok(SendArgs {
        round: if fresh { None } else { round },
        text: text.join(" "),
    })
}

/// Parses `watch --round ROUND`: the round flag is required, nothing else.
fn parse_watch(args: &[String]) -> Result<Command, CliError> {
    let mut round: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--round" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(CliError::Usage(format!(
                        "missing value for --round\n{USAGE}"
                    )));
                };
                round = Some(value.clone());
                index += 1;
            }
            other => {
                return Err(CliError::Usage(format!(
                    "unknown argument: {other}\n{USAGE}"
                )));
            }
        }
    }
    let Some(round) = round else {
        return Err(CliError::Usage(format!(
            "watch requires --round ROUND\n{USAGE}"
        )));
    };
    Ok(Command::Watch { round })
}

/// Parses `history [--limit N]`: the limit must be a non-negative integer.
fn parse_history(args: &[String]) -> Result<Command, CliError> {
    let mut limit = DEFAULT_HISTORY_LIMIT;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--limit" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(CliError::Usage(format!(
                        "missing value for --limit\n{USAGE}"
                    )));
                };
                let Some(parsed) = value.parse::<u64>().ok() else {
                    return Err(CliError::Usage(format!(
                        "invalid --limit: {value}\n{USAGE}"
                    )));
                };
                limit = parsed;
                index += 1;
            }
            other => {
                return Err(CliError::Usage(format!(
                    "unknown argument: {other}\n{USAGE}"
                )));
            }
        }
    }
    Ok(Command::History { limit })
}

/// Builds the `setup --show` view request: the four Host sections.
pub fn setup_view_request() -> ManagementViewRequest {
    ManagementViewRequest {
        sections: HOST_SETUP_SECTIONS
            .iter()
            .map(|section| (*section).to_string())
            .collect(),
    }
}

/// Builds the `status` view request: the same four Host sections as
/// [`setup_view_request`]. There are no `setup`- or `usage`-named sections
/// Host-side, so neither name is requested.
pub fn status_view_request() -> ManagementViewRequest {
    ManagementViewRequest {
        sections: HOST_SETUP_SECTIONS
            .iter()
            .map(|section| (*section).to_string())
            .collect(),
    }
}

/// Builds a timeline request against the bootstrap companion reference.
pub fn history_request(companion: &str, limit: u64) -> HistoryRequest {
    HistoryRequest {
        companion: CompanionWireRef(companion.to_string()),
        since: None,
        limit,
    }
}

/// Builds a text-input candidate: the caller-learned companion projection
/// (echoed from presence; [`DEFAULT_COMPANION_REF`] until the first fact),
/// optional premise round, a fresh [`new_local_id`], and the given body.
pub fn submit_input(
    companion: &str,
    round: Option<String>,
    text: String,
    lang: String,
) -> SubmitTextInput {
    SubmitTextInput {
        companion: CompanionWireRef(companion.to_string()),
        round: round.map(RoundWireId),
        local_id: new_local_id(),
        body: TextBodyWire {
            text,
            lang: TextLangWire(lang),
        },
    }
}

/// Mints a process-unique client-local correspondence ID.
///
/// The `uuid` crate is unavailable to this binary, so uniqueness is
/// process-id plus a process-local monotonic counter
/// (`ctl-<pid>-<counter>`). That is unique per connection for this
/// process, which is all `local_id` needs: it matches acks to sends
/// within one Client and is never Host-canonical.
pub fn new_local_id() -> ClientLocalId {
    let counter = LOCAL_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    ClientLocalId(format!("ctl-{}-{counter}", std::process::id()))
}

/// Credential-registration target for a provider in the shared setup
/// grammar: `"credential:<provider>:main"` (see [`SETUP_CREDENTIAL_LABEL`]).
///
/// This spells [`credential_target`]
/// once for the setup flow; the grammar itself (validation, remainder rules)
/// stays shared, never re-invented here.
pub fn credential_target_for(provider: &str) -> ManagementTargetWire {
    credential_target(provider, SETUP_CREDENTIAL_LABEL)
}

/// Credential id the register step creates for a provider:
/// `"<provider>:main"`, matching the Host registry naming
/// (`"<provider>:<label>"`) and its pre-consent default ref.
pub fn credential_id_for(provider: &str) -> String {
    format!("{provider}:{SETUP_CREDENTIAL_LABEL}")
}

/// Consent-assignment target for a provider/model pair in the shared setup
/// grammar: `"consent:<provider>:<model>:<credential-id>"` (see
/// [`consent_target`]).
///
/// This spells [`consent_target`]
/// once for the setup flow; the grammar itself stays shared.
pub fn consent_target_for(provider: &str, model: &str) -> ManagementTargetWire {
    consent_target(provider, model, &credential_id_for(provider))
}

/// Builds the credential-registration intent: the Host sources the key from
/// its own environment over the Host-local path, so this payload carries no
/// secret, only the intent with a provenance-only rationale (origin, no
/// quote).
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

/// Builds the provider/model assignment intent per the module-docs mapping:
/// [`ManagementIntentKind::ManageRuleConsentCap`] with
/// [`consent_target_for`] and a provenance-only rationale (origin, no
/// quote). Assignment parameters travel in the target, never in the quote.
pub fn assignment_intent(
    intent_id: CommandWireId,
    base: &BaseViewMark,
    provider: &str,
    model: &str,
) -> ManagementIntent {
    ManagementIntent {
        intent_id,
        kind: ManagementIntentKind::ManageRuleConsentCap,
        target: consent_target_for(provider, model),
        base_view: base.clone(),
        rationale: IntentRationaleWire {
            origin: RationaleOrigin::ManagementSurface,
            quote: None,
        },
    }
}

/// Renders a filtered management view as one `kind: title – body` line per
/// section, in Host order.
///
/// The renderer prints exactly what the Host-filtered view contains and
/// nothing else: no revision marks, no envelope IDs, no `Debug` dumps. Body
/// text shown here is Host-filtered display fact by contract; secrecy of
/// what reaches the CLI is a Host property, and this function adds no
/// secret-bearing surface of its own.
pub fn render_view(view: &ManagementView) -> String {
    view.sections
        .iter()
        .map(|section| format!("{}: {} – {}", section.kind, section.title, section.body))
        .collect::<Vec<String>>()
        .join("\n")
}

/// Owner/Companion label used by the history renderers.
pub fn role_label(role: HistoryRole) -> &'static str {
    match role {
        HistoryRole::Owner => "owner",
        HistoryRole::Companion => "companion",
    }
}

/// Renders timeline items as one `[role] text` line each, oldest first.
pub fn render_history(view: &HistoryView) -> String {
    view.items
        .iter()
        .map(|item| format!("[{}] {}", role_label(item.role), item.text))
        .collect::<Vec<String>>()
        .join("\n")
}

/// Renders only the items belonging to `round`, same line shape as
/// [`render_history`].
pub fn render_round_history(view: &HistoryView, round: &str) -> String {
    view.items
        .iter()
        .filter(|item| item.round.0 == round)
        .map(|item| format!("[{}] {}", role_label(item.role), item.text))
        .collect::<Vec<String>>()
        .join("\n")
}

/// Intake-routing decision for a [`RoundIntakeOutcomeWire`]: either the
/// accepted round, or a decline message carrying refs and generations only,
/// never body text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntakeAction {
    /// Accepted into this Host-issued round.
    Accepted {
        /// Round the input joined.
        round: String,
    },
    /// Declined with a display message (exit-code 2 at the crate root).
    Declined {
        /// Operational message: refs/generations only, no bodies.
        message: String,
    },
}

/// Maps an intake outcome to [`IntakeAction`].
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

/// Management-routing decision for a [`ManagementOutcome`]: either an
/// applied line, or a decline split by retryability. Stale/held outcomes
/// are retryable (exit-code 2); clarification/denial are terminal (the
/// crate root maps them to [`CliError::ServerRejected`], exit-code 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagementAction {
    /// Applied with a display line (refs/marks only, no secrets).
    Applied {
        /// Operational line describing what was recorded.
        detail: String,
    },
    /// Retryable decline: refetch the view and retry later.
    Retryable {
        /// Operational message: marks only, no bodies or secrets.
        message: String,
    },
    /// Terminal decline: retrying the same intent will not help.
    Terminal {
        /// Operational message: no bodies or secrets.
        message: String,
    },
}

/// Maps a management outcome to [`ManagementAction`].
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
    //! Parsing matrix, renderer fixtures, builder shapes, and ID uniqueness.
    //!
    //! No sockets, no environment mutation, no network: parsing is pure over
    //! the input slice, rendering is pure over fixtures, and frame tests go
    //! through the in-memory codec only.

    use std::collections::HashSet;

    use ene_api::v1::management::{ManagementOutcome, ManagementView, ViewSection};
    use ene_api::v1::refs::{BaseViewMark, CommandWireId, ViewMarkWire};
    use ene_api::v1::refs::{RevalidationReasonWire, RoundWireId};
    use ene_api::v1::round::{HistoryItem, HistoryRole, HistoryView, RoundIntakeOutcomeWire};

    use super::{Command, IntakeAction, ManagementAction, SendArgs, SetupMode};
    use super::{
        DEFAULT_HISTORY_LIMIT, HOST_SETUP_SECTIONS, SETUP_PROVIDER_OPENAI, assignment_intent,
        consent_target_for, credential_id_for, credential_intent, credential_target_for,
        describe_intake, describe_management, history_request, new_local_id, parse_command,
        render_history, render_round_history, render_view, role_label, setup_view_request,
        status_view_request, submit_input,
    };

    /// Builds owned arguments from plain words.
    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    /// Yields `Ok` values without `unwrap`/`expect` (both denied): the
    /// `assert!` fails the test first, so the `else` branch is only a
    /// type-level fallback, never a silent pass.
    fn require_ok<T: core::fmt::Debug, E: core::fmt::Debug>(
        result: Result<T, E>,
        what: &str,
    ) -> Option<T> {
        assert!(result.is_ok(), "{what} unexpectedly failed: {result:?}");
        result.ok()
    }

    /// Yields `Err` values; see [`require_ok`] for the pattern.
    fn require_err<T: core::fmt::Debug, E: core::fmt::Debug>(
        result: Result<T, E>,
        what: &str,
    ) -> Option<E> {
        assert!(result.is_err(), "{what} unexpectedly succeeded: {result:?}");
        result.err()
    }

    /// Asserts a usage error whose message ends with the usage text.
    fn assert_usage(result: Result<Command, crate::errors::CliError>, what: &str) {
        let Some(error) = require_err(result, what) else {
            return;
        };
        assert!(
            matches!(error, crate::errors::CliError::Usage(_)),
            "{what} must be a usage error, got {error:?}"
        );
        let crate::errors::CliError::Usage(message) = error else {
            return;
        };
        assert!(
            message.ends_with(crate::errors::USAGE),
            "{what} must end with the usage text: {message:?}"
        );
    }

    #[test]
    fn empty_words_need_a_command() {
        assert_usage(parse_command(&args(&[])), "no words");
    }

    #[test]
    fn unknown_command_reports_usage() {
        assert_usage(parse_command(&args(&["frobnicate"])), "unknown command");
    }

    #[test]
    fn setup_show_parses() {
        let Some(command) = require_ok(parse_command(&args(&["setup", "--show"])), "setup --show")
        else {
            return;
        };
        assert!(
            command == Command::Setup(SetupMode::Show),
            "setup --show must select Show, got {command:?}"
        );
    }

    #[test]
    fn setup_assign_parses_in_either_flag_order() {
        for words in [
            ["setup", "--provider", "openai", "--model", "gpt-x"].as_slice(),
            ["setup", "--model", "gpt-x", "--provider", "openai"].as_slice(),
        ] {
            let Some(command) = require_ok(parse_command(&args(words)), "setup assign") else {
                return;
            };
            assert!(
                command
                    == Command::Setup(SetupMode::Assign {
                        provider: String::from("openai"),
                        model: String::from("gpt-x"),
                    }),
                "flag order must not matter, got {command:?}"
            );
        }
    }

    #[test]
    fn setup_without_args_reports_usage_explicitly() {
        let Some(error) = require_err(parse_command(&args(&["setup"])), "bare setup") else {
            return;
        };
        let crate::errors::CliError::Usage(message) = error else {
            return;
        };
        assert!(
            message.contains("setup requires --show"),
            "bare setup must name the required flags: {message:?}"
        );
    }

    #[test]
    fn setup_show_with_provider_reports_usage() {
        assert_usage(
            parse_command(&args(&["setup", "--show", "--provider", "openai"])),
            "show plus provider",
        );
    }

    #[test]
    fn setup_provider_without_model_reports_usage() {
        assert_usage(
            parse_command(&args(&["setup", "--provider", "openai"])),
            "provider without model",
        );
    }

    #[test]
    fn setup_model_without_provider_reports_usage() {
        assert_usage(
            parse_command(&args(&["setup", "--model", "gpt-x"])),
            "model without provider",
        );
    }

    #[test]
    fn setup_unknown_provider_reports_usage() {
        assert_usage(
            parse_command(&args(&["setup", "--provider", "other", "--model", "m"])),
            "unknown provider",
        );
    }

    #[test]
    fn setup_empty_model_reports_usage() {
        assert_usage(
            parse_command(&args(&["setup", "--provider", "openai", "--model", ""])),
            "empty model",
        );
    }

    #[test]
    fn setup_missing_provider_value_reports_usage() {
        assert_usage(
            parse_command(&args(&["setup", "--provider"])),
            "missing provider value",
        );
    }

    #[test]
    fn setup_unknown_flag_reports_usage() {
        assert_usage(
            parse_command(&args(&["setup", "--register-key"])),
            "unknown setup flag",
        );
    }

    #[test]
    fn status_parses_without_operands() {
        let Some(command) = require_ok(parse_command(&args(&["status"])), "status") else {
            return;
        };
        assert!(
            command == Command::Status,
            "status must parse, got {command:?}"
        );
    }

    #[test]
    fn status_with_operands_reports_usage() {
        assert_usage(
            parse_command(&args(&["status", "extra"])),
            "status with operand",
        );
    }

    #[test]
    fn send_with_new_parses_and_joins_text() {
        let Some(command) = require_ok(
            parse_command(&args(&["send", "--new", "hello", "there"])),
            "send --new",
        ) else {
            return;
        };
        assert!(
            command
                == Command::Send(SendArgs {
                    round: None,
                    text: String::from("hello there"),
                }),
            "--new must force no round and join text, got {command:?}"
        );
    }

    #[test]
    fn send_with_round_parses() {
        let Some(command) = require_ok(
            parse_command(&args(&["send", "--round", "round-1", "hi"])),
            "send --round",
        ) else {
            return;
        };
        assert!(
            command
                == Command::Send(SendArgs {
                    round: Some(String::from("round-1")),
                    text: String::from("hi"),
                }),
            "--round must set the premise round, got {command:?}"
        );
    }

    #[test]
    fn send_without_flags_requests_a_new_round() {
        let Some(command) = require_ok(parse_command(&args(&["send", "hi"])), "bare send") else {
            return;
        };
        assert!(
            command
                == Command::Send(SendArgs {
                    round: None,
                    text: String::from("hi"),
                }),
            "bare send must request a new round, got {command:?}"
        );
    }

    #[test]
    fn send_new_with_round_reports_usage() {
        assert_usage(
            parse_command(&args(&["send", "--new", "--round", "r", "hi"])),
            "--new with --round",
        );
    }

    #[test]
    fn send_without_text_reports_usage() {
        assert_usage(parse_command(&args(&["send"])), "send without text");
        assert_usage(
            parse_command(&args(&["send", "--new"])),
            "send --new without text",
        );
    }

    #[test]
    fn send_missing_round_value_reports_usage() {
        assert_usage(
            parse_command(&args(&["send", "--round"])),
            "missing round value",
        );
    }

    #[test]
    fn send_dash_word_reports_usage() {
        assert_usage(
            parse_command(&args(&["send", "--verbose", "hi"])),
            "unknown send flag",
        );
    }

    #[test]
    fn watch_parses_with_round() {
        let Some(command) = require_ok(
            parse_command(&args(&["watch", "--round", "round-7"])),
            "watch --round",
        ) else {
            return;
        };
        assert!(
            command
                == Command::Watch {
                    round: String::from("round-7"),
                },
            "watch must carry the round, got {command:?}"
        );
    }

    #[test]
    fn watch_without_round_reports_usage() {
        assert_usage(parse_command(&args(&["watch"])), "watch without round");
    }

    #[test]
    fn watch_with_extra_flags_reports_usage() {
        assert_usage(
            parse_command(&args(&["watch", "--round", "r", "--limit", "3"])),
            "watch with extra flags",
        );
    }

    #[test]
    fn history_defaults_the_limit() {
        let Some(command) = require_ok(parse_command(&args(&["history"])), "history") else {
            return;
        };
        assert!(
            command
                == Command::History {
                    limit: DEFAULT_HISTORY_LIMIT,
                },
            "history must default the limit, got {command:?}"
        );
    }

    #[test]
    fn history_limit_parses() {
        let Some(command) = require_ok(
            parse_command(&args(&["history", "--limit", "3"])),
            "history --limit",
        ) else {
            return;
        };
        assert!(
            command == Command::History { limit: 3 },
            "history must carry the limit, got {command:?}"
        );
    }

    #[test]
    fn history_bad_limit_reports_usage() {
        assert_usage(
            parse_command(&args(&["history", "--limit", "many"])),
            "non-numeric limit",
        );
        assert_usage(
            parse_command(&args(&["history", "--limit", "-2"])),
            "negative limit",
        );
        assert_usage(
            parse_command(&args(&["history", "--limit"])),
            "missing limit",
        );
    }

    #[test]
    fn history_unknown_flag_reports_usage() {
        assert_usage(
            parse_command(&args(&["history", "--round", "r"])),
            "unknown history flag",
        );
    }

    /// Builds a two-section fixture view; the mark is planted with a marker
    /// the renderer must never echo.
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

    /// Builds a two-round fixture; item text carries display facts the
    /// renderer must preserve verbatim.
    fn fixture_history() -> HistoryView {
        HistoryView {
            items: vec![
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
            ],
        }
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
    fn render_round_history_filters_by_round() {
        let rendered = render_round_history(&fixture_history(), "round-2");
        assert!(
            rendered == "[companion] second words",
            "round history must keep only that round, got {rendered:?}"
        );
        assert!(
            render_round_history(&fixture_history(), "round-9").is_empty(),
            "an unknown round must render to nothing"
        );
    }

    #[test]
    fn role_labels_cover_both_roles() {
        assert!(role_label(HistoryRole::Owner) == "owner", "owner label");
        assert!(
            role_label(HistoryRole::Companion) == "companion",
            "companion label"
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
        // Documented Host section set: `HostHandle::build_view` in
        // `apps/ene-core/src/setup.rs` renders exactly these four (an empty
        // request selects the same four). This contract test pins the wire
        // names, so a Host rename fails here instead of fetching nothing.
        let documented = ["provider", "model", "consent", "credential"];
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
            "setup --show requests the four Host sections: {setup:?}"
        );
        let status = status_view_request();
        assert!(
            status.sections == setup.sections,
            "status requests the same four Host sections: {status:?}"
        );
        let history = history_request("companion-1", 7);
        assert!(
            history.companion.0 == "companion-1" && history.limit == 7,
            "history echoes the learned companion: {history:?}"
        );
        let input = submit_input(
            "companion-1",
            Some(String::from("round-1")),
            String::from("hello"),
            String::from("en"),
        );
        assert!(
            input.companion.0 == "companion-1",
            "input echoes the learned companion: {input:?}"
        );
        let Some(round) = input.round.as_ref() else {
            return;
        };
        assert!(
            round.0 == "round-1",
            "input keeps the premise round: {input:?}"
        );
        let fresh = submit_input(
            "companion-1",
            None,
            String::from("hello"),
            String::from("en"),
        );
        assert!(
            fresh.round.is_none(),
            "no premise round must request a new round: {fresh:?}"
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
            consent_target_for("openai", "gpt-x").0 == "consent:openai:gpt-x:openai:main",
            "consent target spells the shared grammar over that credential id"
        );
        // Roundtrip through the shared parsers: the builders never bypass
        // Host-side validation.
        assert!(
            ene_api::v1::management::parse_credential_target(&credential_target_for("openai"))
                == Some((String::from("openai"), String::from("main"))),
            "credential target must parse as (provider, label)"
        );
        assert!(
            ene_api::v1::management::parse_consent_target(&consent_target_for("openai", "gpt-x"))
                == Some((
                    String::from("openai"),
                    String::from("gpt-x"),
                    String::from("openai:main"),
                )),
            "consent target must parse as (provider, model, credential-id)"
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
            SETUP_PROVIDER_OPENAI,
            "gpt-x",
        );
        assert!(
            assignment.kind == ManagementIntentKind::ManageRuleConsentCap
                && assignment.target.0 == "consent:openai:gpt-x:openai:main"
                && assignment.base_view == base
                && assignment.rationale.origin == RationaleOrigin::ManagementSurface
                && assignment.rationale.quote.is_none(),
            "assignment intent carries the consent target and no quote: {assignment:?}"
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
    fn local_id_names_this_process_counter() {
        let id = new_local_id().0;
        assert!(
            id.starts_with(&format!("ctl-{}-", std::process::id())),
            "local ID must name this process: {id:?}"
        );
    }
}
