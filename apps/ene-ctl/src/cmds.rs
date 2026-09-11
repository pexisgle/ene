//! `ene-ctl` subcommands: argument parsing, wire-payload builders, rendering.
//!
//! Pure: parses `argv` words, builds `ene-api` DTOs, renders views to display
//! strings; no I/O, sockets, or environment. Transport lives in
//! [`crate::client`]; exit-code mapping lives at the crate root.
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

pub const DEFAULT_HISTORY_LIMIT: u64 = 50;

/// The Host registers refs as `"<provider>:<label>"` and falls back to the
/// `"<provider>:main"` ref before any consent exists, so the setup flow
/// always uses this label: the consent step can then name the credential id
/// it just created (see [`credential_id_for`]).
pub const SETUP_CREDENTIAL_LABEL: &str = "main";

/// Mirrors the Host section set: `HostHandle::build_view` in
/// `apps/ene-core/src/setup.rs` renders exactly these four (an empty request
/// selects the same four). The contract test below asserts these names
/// against that documented Host set, so a Host rename fails the test instead
/// of silently fetching nothing.
pub const HOST_SETUP_SECTIONS: &[&str] =
    &["provider", "model", "consent", "credential", "learning"];

/// Only provider the setup flow knows how to assign yet.
pub const SETUP_PROVIDER_OPENAI: &str = "openai";

static LOCAL_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

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

/// An empty slice, an unknown name, or malformed operands are all
/// [`CliError::Usage`] whose message ends with [`USAGE`].
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

fn parse_setup(args: &[String]) -> Result<SetupMode, CliError> {
    let mut show = false;
    let mut learning = false;
    let mut provider: Option<String> = None;
    let mut model: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--show" => {
                show = true;
                index += 1;
            }
            "--learning" => {
                learning = true;
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
        if provider.is_some() || model.is_some() || learning {
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
                learning,
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

fn parse_status(args: &[String]) -> Result<Command, CliError> {
    if let Some(extra) = args.first() {
        return Err(CliError::Usage(format!(
            "unknown argument: {extra}\n{USAGE}"
        )));
    }
    Ok(Command::Status)
}

/// A word starting with `--` is never treated as text; such input is a usage
/// error.
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
        round,
        fresh,
        text: text.join(" "),
    })
}

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

pub fn setup_view_request() -> ManagementViewRequest {
    ManagementViewRequest {
        sections: HOST_SETUP_SECTIONS
            .iter()
            .map(|section| (*section).to_string())
            .collect(),
    }
}

/// There are no `setup`- or `usage`-named sections Host-side, so neither name
/// is requested.
pub fn status_view_request() -> ManagementViewRequest {
    ManagementViewRequest {
        sections: HOST_SETUP_SECTIONS
            .iter()
            .map(|section| (*section).to_string())
            .collect(),
    }
}

pub fn history_request(companion: &str, limit: u64) -> HistoryRequest {
    HistoryRequest {
        companion: CompanionWireRef(companion.to_string()),
        since: None,
        limit,
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

/// Mints a client-local correspondence ID: `ctl-<pid>-<counter>`.
///
/// The `uuid` crate is unavailable to this binary, so uniqueness rests on the
/// process id plus a process-local monotonic counter: unique per connection
/// for this process, which is all `local_id` needs (it matches acks to sends
/// within one Client and is never Host-canonical).
pub fn new_local_id() -> ClientLocalId {
    let counter = LOCAL_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    ClientLocalId(format!("ctl-{}-{counter}", std::process::id()))
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
pub fn render_history(view: &HistoryView) -> String {
    view.items
        .iter()
        .map(|item| format!("[{}] {}", role_label(item.role), item.text))
        .collect::<Vec<String>>()
        .join("\n")
}

pub fn render_round_history(view: &HistoryView, round: &str) -> String {
    view.items
        .iter()
        .filter(|item| item.round.0 == round)
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
    use ene_api::v1::round::{HistoryItem, HistoryRole, HistoryView, RoundIntakeOutcomeWire};

    use super::{
        CAPABILITY_DIALOGUE, CAPABILITY_LEARNING, DEFAULT_HISTORY_LIMIT, HOST_SETUP_SECTIONS,
        SETUP_PROVIDER_OPENAI, assignment_intent, consent_target_for, credential_id_for,
        credential_intent, credential_target_for, describe_intake, describe_management,
        history_request, new_local_id, parse_command, render_history, render_round_history,
        render_view, setup_view_request, status_view_request, submit_input,
    };
    use super::{Command, IntakeAction, ManagementAction, SendArgs, SetupMode};

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    fn assert_usage(result: Result<Command, crate::errors::CliError>, what: &str) {
        let error = result.expect_err(what);
        let crate::errors::CliError::Usage(message) = error else {
            panic!("{what} must be a usage error, got {error:?}");
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
        let command = (parse_command(&args(&["setup", "--show"]))).expect("setup --show");
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
            let command = (parse_command(&args(words))).expect("setup assign");
            assert!(
                command
                    == Command::Setup(SetupMode::Assign {
                        provider: String::from("openai"),
                        model: String::from("gpt-x"),
                        learning: false,
                    }),
                "flag order must not matter, got {command:?}"
            );
        }
    }

    #[test]
    fn setup_learning_assignment_parses_explicitly() {
        let command = (parse_command(&args(&[
            "setup",
            "--learning",
            "--provider",
            "openai",
            "--model",
            "gpt-x",
        ])))
        .expect("learning assign");
        let Command::Setup(SetupMode::Assign {
            provider,
            model,
            learning,
        }) = command
        else {
            panic!("setup --learning must select an assignment");
        };
        assert!(learning, "the learning capability must be explicit");
        assert_eq!(provider, "openai");
        assert_eq!(model, "gpt-x");
    }

    #[test]
    fn setup_without_args_reports_usage_explicitly() {
        let error = (parse_command(&args(&["setup"]))).expect_err("bare setup");
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
        let command = (parse_command(&args(&["status"]))).expect("status");
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
        let command =
            (parse_command(&args(&["send", "--new", "hello", "there"]))).expect("send --new");
        assert!(
            command
                == Command::Send(SendArgs {
                    round: None,
                    fresh: true,
                    text: String::from("hello there"),
                }),
            "--new must force a fresh round and join text, got {command:?}"
        );
    }

    #[test]
    fn send_with_round_parses() {
        let command =
            (parse_command(&args(&["send", "--round", "round-1", "hi"]))).expect("send --round");
        assert!(
            command
                == Command::Send(SendArgs {
                    round: Some(String::from("round-1")),
                    fresh: false,
                    text: String::from("hi"),
                }),
            "--round must set the premise round, got {command:?}"
        );
    }

    #[test]
    fn send_without_flags_requests_a_new_round() {
        let command = (parse_command(&args(&["send", "hi"]))).expect("bare send");
        assert!(
            command
                == Command::Send(SendArgs {
                    round: None,
                    fresh: false,
                    text: String::from("hi"),
                }),
            "bare send must join-or-mint, got {command:?}"
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
        let command =
            (parse_command(&args(&["watch", "--round", "round-7"]))).expect("watch --round");
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
        let command = (parse_command(&args(&["history"]))).expect("history");
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
        let command =
            (parse_command(&args(&["history", "--limit", "3"]))).expect("history --limit");
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

    /// Fixture view whose mark is a marker the renderer must never echo.
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
        let status = status_view_request();
        assert!(
            status.sections == setup.sections,
            "status requests the same Host sections: {status:?}"
        );
        let history = history_request("companion-1", 7);
        assert!(
            history.companion.0 == "companion-1" && history.limit == 7,
            "history echoes the learned companion: {history:?}"
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
    fn local_id_names_this_process_counter() {
        let id = new_local_id().0;
        assert!(
            id.starts_with(&format!("ctl-{}-", std::process::id())),
            "local ID must name this process: {id:?}"
        );
    }
}
