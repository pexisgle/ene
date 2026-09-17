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

use ene_api::v1::deletion::{
    DeletionParticipantReportWire, DeletionPurposeWire, DeletionStatusResponse, deletion_target,
};
use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome, ManagementView,
    ManagementViewRequest, RationaleOrigin, consent_target, credential_target,
};
use ene_api::v1::refs::{
    BaseViewMark, ClientLocalId, CommandWireId, CompanionWireRef, ManagementTargetWire,
    RoundWireId, TextLangWire,
};
use ene_api::v1::round::{
    HistoryItem, HistoryRequest, HistoryRole, PresentationStatus, RoundIntakeOutcomeWire,
    SubmitTextInput, TextBodyWire,
};
use ene_api::v1::undelivered::{
    GetReportSource, GetTaskReport, ListTasks, PageCursorWire, ReportSourcePageView,
    ReportSourceWireRef, ResumeTask, ResumeTaskOutcomeWire, SelectTask, TaskListPage,
    TaskReportPage, TaskReportResponse, TaskWireRef, UndeliveredAck, UndeliveredAckOutcome,
    UndeliveredRequest, UndeliveredResponse, UndeliveredSummary,
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
    /// First-party Task list (stored lifecycle + execution flag, paged).
    /// `cursor` continues from a previous page's `next:` line.
    Tasks {
        cursor: Option<String>,
        limit: Option<u32>,
    },
    /// One Task's paged report (attempt rows before result rows, no bodies).
    Report {
        task: String,
        cursor: Option<String>,
        limit: Option<u32>,
    },
    /// One bounded body page of a report source named by a report page.
    Source {
        source: String,
        cursor: Option<u64>,
        limit_bytes: Option<u32>,
    },
    /// Select the Owner-confirmed Task for this conversation (in-memory
    /// display selection; no execution starts).
    SelectTask {
        task: String,
    },
    /// Explicitly resume one interrupted Task (new revision + delegation on
    /// acceptance; refusals stay Ok-side with zero writes).
    ResumeTask {
        task: String,
        revision: u64,
        purpose: String,
        instruction: String,
    },
    /// Fetch the undelivered backlog, paint it, and ACK what was painted.
    /// `redisplay` forces an explicit head pass including failed rows.
    Undelivered {
        cursor: Option<String>,
        limit: Option<u32>,
        redisplay: bool,
    },
    /// Request one Targeted Deletion (`Stage 6` A1b, lifecycle §15). Advisory:
    /// the Host re-validates the typed target against its live deletion
    /// surface, stages the request, and the Owner confirms it on the Host PC
    /// (IPC §18.1).
    Deletion {
        text: String,
        purpose: DeletionPurposeWire,
    },
    /// Read the bounded Targeted Deletion operation status page (no target
    /// body, no search material).
    DeletionStatus {
        cursor: Option<String>,
        limit: Option<u32>,
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

/// Subscription / paging request for the undelivered backlog. [`None`]
/// cursor catches up (arrivals first, else an explicit head pass).
pub fn undelivered_request(
    cursor: Option<String>,
    limit: Option<u32>,
    redisplay: bool,
) -> UndeliveredRequest {
    UndeliveredRequest {
        companion: None,
        cursor: cursor.map(PageCursorWire),
        limit,
        redisplay,
    }
}

/// Presentation observation for one receipt: only ever `Presented` after the
/// batch fully painted. A partial batch sends nothing, so the Host keeps it
/// `Unknown` instead of recording a presentation the operator never saw.
pub fn undelivered_ack(receipt: &str, status: PresentationStatus) -> UndeliveredAck {
    UndeliveredAck {
        receipt: ene_api::v1::undelivered::PresentationReceiptWireRef(receipt.to_string()),
        status,
    }
}

pub fn list_tasks_request(cursor: Option<String>, limit: Option<u32>) -> ListTasks {
    ListTasks {
        cursor: cursor.map(PageCursorWire),
        limit,
    }
}

pub fn task_report_request(
    task: &str,
    cursor: Option<String>,
    limit: Option<u32>,
) -> GetTaskReport {
    GetTaskReport {
        task: TaskWireRef(task.to_string()),
        cursor: cursor.map(PageCursorWire),
        limit,
    }
}

pub fn report_source_request(
    source: &str,
    cursor: Option<u64>,
    limit_bytes: Option<u32>,
) -> GetReportSource {
    GetReportSource {
        source: ReportSourceWireRef(source.to_string()),
        cursor,
        limit_bytes,
    }
}

pub fn select_task_request(task: &str) -> SelectTask {
    SelectTask {
        task: TaskWireRef(task.to_string()),
    }
}

pub fn resume_task_request(
    task: &str,
    expected_revision: u64,
    expected_purpose: &str,
    instruction: String,
) -> ResumeTask {
    ResumeTask {
        task: TaskWireRef(task.to_string()),
        expected_revision,
        expected_purpose: expected_purpose.to_string(),
        instruction,
    }
}

/// One `kind subject: excerpt` line per item (truncation marked), then one
/// headline line per Task. Excerpts are Host-scrubbed display facts.
pub fn render_summary(summary: &UndeliveredSummary) -> String {
    let mut lines = Vec::new();
    for item in &summary.items {
        let mark = if item.truncated { "…" } else { "" };
        lines.push(format!(
            "{} {}: {}{}",
            item.source.kind, item.source.subject, item.excerpt, mark
        ));
    }
    for report in &summary.reports {
        lines.push(format!(
            "task {} rev {} {}",
            report.task.0, report.revision, report.progress
        ));
    }
    if summary.has_more {
        lines.push(String::from("(more)"));
    }
    if let Some(cursor) = &summary.next_cursor {
        lines.push(format!("next: {}", cursor.0));
    }
    lines.join("\n")
}

/// One `task rev progress` line per entry (`running` marked), plus the
/// `next:` continuation while a page remains.
pub fn render_task_list(page: &TaskListPage) -> String {
    let mut lines: Vec<String> = page
        .tasks
        .iter()
        .map(|task| {
            let running = if task.running { " running" } else { "" };
            format!(
                "{} rev {} {}{}",
                task.task.0, task.revision, task.progress, running
            )
        })
        .collect();
    if let Some(cursor) = &page.next_cursor {
        lines.push(format!("next: {}", cursor.0));
    }
    lines.join("\n")
}

/// Headline plus one `kind id` line per detail row, plus the `next:`
/// continuation while rows remain. Bodies page through `source`.
pub fn render_report_page(page: &TaskReportPage) -> String {
    let mut lines = vec![format!(
        "{} rev {} {}",
        page.task.0, page.revision, page.progress
    )];
    for row in &page.rows {
        lines.push(format!("{} {}", row.kind, row.id));
    }
    if let Some(cursor) = &page.next_cursor {
        lines.push(format!("next: {}", cursor.0));
    }
    lines.join("\n")
}

/// The body page text verbatim (Host-bounded, UTF-8 cut), plus the `next:`
/// byte cursor while the body continues.
pub fn render_source_page(page: &ReportSourcePageView) -> String {
    if let Some(next) = page.next {
        format!("{}\nnext: {next}", page.text)
    } else {
        page.text.clone()
    }
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

/// One Targeted Deletion request intent (`Stage 6` A1b, lifecycle §15).
///
/// Advisory by construction: the target grammar is the shared one from
/// `ene-api`, the rationale is provenance-only (the exact text travels in the
/// target, never in the quote, so the Host's intent journal can redact it),
/// and nothing here can confirm the destructive operation. `base` must be the
/// current deletion surface mark the status page returned.
#[must_use]
pub fn deletion_intent(
    intent_id: CommandWireId,
    base: &str,
    purpose: DeletionPurposeWire,
    exact_text: &str,
) -> ManagementIntent {
    ManagementIntent {
        intent_id,
        kind: ManagementIntentKind::RequestDeletionBackupRestoreReset,
        target: deletion_target(purpose, exact_text),
        base_view: BaseViewMark(base.to_string()),
        rationale: IntentRationaleWire {
            origin: RationaleOrigin::ManagementSurface,
            quote: None,
        },
    }
}

/// Parses one `--purpose` token from the closed wire set.
#[must_use]
pub fn deletion_purpose(token: &str) -> Option<DeletionPurposeWire> {
    DeletionPurposeWire::from_name(token)
}

/// Renders the bounded deletion status page: the surface mark an intent builds
/// on, one line per operation, and the `next:` cursor while a later page
/// exists. No target body, search material, or credential is in this page.
#[must_use]
pub fn render_deletion_status(response: &DeletionStatusResponse) -> String {
    let DeletionStatusResponse::Page(page) = response else {
        return String::from("deletion status is unavailable; retry later");
    };
    let mut lines = vec![format!("mark {}", page.mark.0)];
    for operation in &page.operations {
        let hold = operation
            .hold
            .map_or_else(|| String::from("-"), |hold| hold.as_str().to_string());
        let participants = match &operation.participants {
            DeletionParticipantReportWire::NotReported => String::from("not-reported"),
            DeletionParticipantReportWire::Reported(entries) => entries.len().to_string(),
        };
        lines.push(format!(
            "{} {} {} sweep={} started={} hold={} participants={}",
            operation.operation.0,
            operation.phase.as_str(),
            operation.purpose.as_str(),
            operation.sweep,
            operation.started_at,
            hold,
            participants
        ));
    }
    if let Some(next) = &page.next_cursor {
        lines.push(format!("next {}", next.0));
    }
    lines.join("\n")
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

/// ACK-routing decision for an [`UndeliveredAckOutcome`]; retryable answers
/// keep their meaning instead of being shown as presented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AckAction {
    Confirmed { detail: String },
    Retryable { message: String },
}

pub fn describe_ack(outcome: &UndeliveredAckOutcome) -> AckAction {
    match outcome {
        UndeliveredAckOutcome::Presented { presented } => AckAction::Confirmed {
            detail: format!("presented {presented} item(s)"),
        },
        UndeliveredAckOutcome::AlreadyPresented => AckAction::Confirmed {
            detail: String::from("already presented; nothing was written"),
        },
        UndeliveredAckOutcome::ReturnedToPending { count } => AckAction::Confirmed {
            detail: format!("returned {count} item(s) to pending"),
        },
        UndeliveredAckOutcome::KeptUnknown => AckAction::Confirmed {
            detail: String::from("kept as unknown; a later pass re-presents"),
        },
        UndeliveredAckOutcome::UnknownRef => AckAction::Retryable {
            message: String::from("unknown receipt; re-query for a new receipt and retry"),
        },
        UndeliveredAckOutcome::StalePresentation => AckAction::Retryable {
            message: String::from("stale presentation; re-query for a new receipt and retry"),
        },
        UndeliveredAckOutcome::StaleConnection => AckAction::Retryable {
            message: String::from("stale connection; re-query on this connection and retry"),
        },
    }
}

/// Resume-routing decision for a [`ResumeTaskOutcomeWire`]; only `Resumed`
/// is applied, refusals stay Ok-side with zero Task writes, and `InFlight`
/// / `Unavailable` are retryable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeAction {
    Resumed { detail: String },
    Refused { message: String },
    Retryable { message: String },
}

pub fn describe_resume(outcome: &ResumeTaskOutcomeWire) -> ResumeAction {
    match outcome {
        ResumeTaskOutcomeWire::Resumed {
            revision,
            delegation,
            ..
        } => ResumeAction::Resumed {
            detail: format!("resumed at revision {revision} (delegation {delegation})"),
        },
        ResumeTaskOutcomeWire::StalePremise { current_revision } => ResumeAction::Refused {
            message: format!("stale premise; current revision is {current_revision}"),
        },
        ResumeTaskOutcomeWire::Superseded => ResumeAction::Refused {
            message: String::from("superseded by a newer Owner input"),
        },
        ResumeTaskOutcomeWire::TaskTerminal { progress } => ResumeAction::Refused {
            message: format!("task is already {progress}"),
        },
        ResumeTaskOutcomeWire::AlreadyRunning => ResumeAction::Refused {
            message: String::from("task is already running"),
        },
        ResumeTaskOutcomeWire::HeldByUnknownEffects => ResumeAction::Refused {
            message: String::from("held by unknown effects; settle them first"),
        },
        ResumeTaskOutcomeWire::ResultAvailable => ResumeAction::Refused {
            message: String::from("a sealed result is available to review first"),
        },
        ResumeTaskOutcomeWire::NeedsRevalidation { hold } => ResumeAction::Refused {
            message: format!("needs revalidation: {hold}"),
        },
        ResumeTaskOutcomeWire::MissingTask => ResumeAction::Refused {
            message: String::from("no such task"),
        },
        ResumeTaskOutcomeWire::RevisionExhausted => ResumeAction::Refused {
            message: String::from("the task cannot take another change"),
        },
        ResumeTaskOutcomeWire::InFlight => ResumeAction::Retryable {
            message: String::from("resume already in flight; retry for its outcome"),
        },
        ResumeTaskOutcomeWire::UnknownRef => ResumeAction::Retryable {
            message: String::from("unknown task reference; re-list and retry"),
        },
        ResumeTaskOutcomeWire::StaleConnection => ResumeAction::Retryable {
            message: String::from("stale sender epoch; re-prepare on this connection and retry"),
        },
        ResumeTaskOutcomeWire::Unavailable => ResumeAction::Retryable {
            message: String::from("host unavailable; retry later"),
        },
    }
}

/// Undelivered-fetch routing for an [`UndeliveredResponse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchAction {
    Paint,
    Retryable { message: String },
}

pub fn describe_fetch(response: &UndeliveredResponse) -> FetchAction {
    match response {
        UndeliveredResponse::Summary(_) => FetchAction::Paint,
        UndeliveredResponse::FrameTooLarge => FetchAction::Retryable {
            message: String::from("frame too large; retry with a smaller limit"),
        },
        UndeliveredResponse::NoCurrentPresence => FetchAction::Retryable {
            message: String::from("no current presence; summon first, then retry"),
        },
        UndeliveredResponse::UnknownCompanion => FetchAction::Retryable {
            message: String::from("unknown companion; re-sync presence and retry"),
        },
        UndeliveredResponse::StaleBaseView { .. } => FetchAction::Retryable {
            message: String::from("stale base view; re-query from the head"),
        },
    }
}

/// Report-fetch routing for a [`TaskReportResponse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportAction {
    Show,
    Retryable { message: String },
}

pub fn describe_report(response: &TaskReportResponse) -> ReportAction {
    match response {
        TaskReportResponse::Page(_) => ReportAction::Show,
        TaskReportResponse::UnknownRef => ReportAction::Retryable {
            message: String::from("unknown task reference; re-list and retry"),
        },
        TaskReportResponse::StaleBaseView { .. } => ReportAction::Retryable {
            message: String::from("stale cursor; re-query from the head"),
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

    #[test]
    fn presentation_builders_carry_refs_limits_and_flags() {
        use ene_api::v1::round::PresentationStatus;

        let fetch = super::undelivered_request(Some(String::from("cursor-1")), Some(7), true);
        assert!(fetch.companion.is_none());
        assert_eq!(
            fetch.cursor.map(|cursor| cursor.0),
            Some(String::from("cursor-1"))
        );
        assert_eq!(fetch.limit, Some(7));
        assert!(fetch.redisplay);
        let head = super::undelivered_request(None, None, false);
        assert!(head.cursor.is_none() && head.limit.is_none() && !head.redisplay);

        let ack = super::undelivered_ack("receipt-1", PresentationStatus::Presented);
        assert!(ack.receipt.0 == "receipt-1" && ack.status == PresentationStatus::Presented);

        let list = super::list_tasks_request(None, None);
        assert!(list.cursor.is_none() && list.limit.is_none());
        let report = super::task_report_request("task-1", Some(String::from("c")), Some(3));
        assert!(report.task.0 == "task-1");
        assert_eq!(report.limit, Some(3));
        let source = super::report_source_request("source-1", Some(9), Some(128));
        assert!(source.source.0 == "source-1" && source.cursor == Some(9));
        assert_eq!(source.limit_bytes, Some(128));
        assert!(super::select_task_request("task-2").task.0 == "task-2");
        let resume = super::resume_task_request("task-3", 4, "task-3:4", String::from("go on"));
        assert!(resume.expected_revision == 4 && resume.expected_purpose == "task-3:4");
    }

    #[test]
    fn renders_use_item_headline_and_continuation_lines() {
        use ene_api::v1::refs::RoundWireId;
        use ene_api::v1::undelivered::{
            PageCursorWire, PresentationReceiptWireRef, ReportSourcePageView, TaskListItem,
            TaskListPage, TaskReportPage, TaskReportRowView, TaskReportView, TaskWireRef,
            UndeliveredItemView, UndeliveredSourceView, UndeliveredSummary, UndeliveredWireRef,
        };

        let summary = UndeliveredSummary {
            receipt: PresentationReceiptWireRef(String::from("receipt-1")),
            round: RoundWireId(String::from("round-1")),
            presence_generation: 2,
            items: vec![UndeliveredItemView {
                reference: UndeliveredWireRef(String::from("und-1")),
                source: UndeliveredSourceView {
                    kind: String::from("task_revision"),
                    subject: String::from("subject-1"),
                    certainty: None,
                },
                excerpt: String::from("first words"),
                truncated: true,
            }],
            reports: vec![TaskReportView {
                task: TaskWireRef(String::from("task-1")),
                revision: 2,
                progress: String::from("in_progress"),
                details_available: true,
            }],
            has_more: true,
            next_cursor: Some(PageCursorWire(String::from("cursor-9"))),
        };
        let rendered = super::render_summary(&summary);
        for wanted in [
            "task_revision subject-1: first words…",
            "task task-1 rev 2 in_progress",
            "(more)",
            "next: cursor-9",
        ] {
            assert!(
                rendered.contains(wanted),
                "summary must carry {wanted:?}, got {rendered:?}"
            );
        }
        assert!(
            !rendered.contains("receipt-1"),
            "receipt refs stay off the display: {rendered:?}"
        );

        let list = super::render_task_list(&TaskListPage {
            tasks: vec![TaskListItem {
                task: TaskWireRef(String::from("task-7")),
                revision: 2,
                progress: String::from("in_progress"),
                running: true,
                purpose: String::from("task-7:2"),
            }],
            next_cursor: Some(PageCursorWire(String::from("cursor-2"))),
        });
        assert!(
            list.contains("rev 2 in_progress running") && list.contains("next: cursor-2"),
            "task list renders entries plus continuation, got {list:?}"
        );
        let report = super::render_report_page(&TaskReportPage {
            task: TaskWireRef(String::from("task-7")),
            revision: 2,
            progress: String::from("in_progress"),
            purpose: String::from("task-7:2"),
            purpose_source: ene_api::v1::undelivered::ReportSourceWireRef(String::from("source-1")),
            rows: vec![
                TaskReportRowView {
                    kind: String::from("action_attempt"),
                    id: String::from("attempt-1"),
                    adopted_revision: None,
                    source: None,
                },
                TaskReportRowView {
                    kind: String::from("task_result"),
                    id: String::from("result-1"),
                    adopted_revision: Some(2),
                    source: None,
                },
            ],
            next_cursor: None,
        });
        assert!(
            report.contains("action_attempt") && report.contains("task_result"),
            "report renders both row kinds, got {report:?}"
        );
        let source = super::render_source_page(&ReportSourcePageView {
            text: String::from("body bytes"),
            total_bytes: 10,
            next: Some(4),
        });
        assert!(
            source.contains("body bytes") && source.contains("next: 4"),
            "source renders text plus byte cursor, got {source:?}"
        );
    }

    #[test]
    fn ack_resume_fetch_and_report_describes_split_applied_retryable() {
        use ene_api::v1::round::PresentationStatus;
        use ene_api::v1::undelivered::{
            ResumeTaskOutcomeWire, TaskReportResponse, UndeliveredAckOutcome, UndeliveredResponse,
        };

        assert!(matches!(
            super::describe_ack(&UndeliveredAckOutcome::Presented { presented: 2 }),
            super::AckAction::Confirmed { .. }
        ));
        assert!(matches!(
            super::describe_ack(&UndeliveredAckOutcome::AlreadyPresented),
            super::AckAction::Confirmed { .. }
        ));
        assert!(matches!(
            super::describe_ack(&UndeliveredAckOutcome::StaleConnection),
            super::AckAction::Retryable { .. }
        ));
        let acked = super::undelivered_ack("r", PresentationStatus::Unknown);
        assert!(acked.status == PresentationStatus::Unknown);

        assert!(matches!(
            super::describe_resume(&ResumeTaskOutcomeWire::Resumed {
                task: ene_api::v1::undelivered::TaskWireRef(String::from("t")),
                revision: 3,
                delegation: String::from("d"),
            }),
            super::ResumeAction::Resumed { .. }
        ));
        assert!(matches!(
            super::describe_resume(&ResumeTaskOutcomeWire::StalePremise {
                current_revision: 4
            }),
            super::ResumeAction::Refused { .. }
        ));
        assert!(matches!(
            super::describe_resume(&ResumeTaskOutcomeWire::InFlight),
            super::ResumeAction::Retryable { .. }
        ));
        assert!(matches!(
            super::describe_resume(&ResumeTaskOutcomeWire::Unavailable),
            super::ResumeAction::Retryable { .. }
        ));

        assert!(matches!(
            super::describe_fetch(&UndeliveredResponse::FrameTooLarge),
            super::FetchAction::Retryable { .. }
        ));
        assert!(matches!(
            super::describe_fetch(&UndeliveredResponse::NoCurrentPresence),
            super::FetchAction::Retryable { .. }
        ));
        assert!(matches!(
            super::describe_report(&TaskReportResponse::UnknownRef),
            super::ReportAction::Retryable { .. }
        ));
    }

    #[test]
    fn deletion_intent_spells_the_shared_grammar_without_the_quote() {
        use ene_api::v1::deletion::{DeletionPurposeWire, parse_deletion_target};

        let intent = super::deletion_intent(
            CommandWireId(uuid::Uuid::new_v4()),
            "deletion-view/0/-/0/-",
            DeletionPurposeWire::Security,
            "leaked key",
        );
        assert_eq!(
            intent.kind,
            ene_api::v1::management::ManagementIntentKind::RequestDeletionBackupRestoreReset
        );
        assert_eq!(intent.base_view.0, "deletion-view/0/-/0/-");
        assert!(
            intent.rationale.quote.is_none(),
            "the exact text travels in the target, never the rationale"
        );
        let parsed = parse_deletion_target(&intent.target).expect("the target must parse");
        assert_eq!(parsed.purpose(), DeletionPurposeWire::Security);
        assert_eq!(parsed.exact_text(), "leaked key");
        assert!(super::deletion_purpose("privacy").is_some());
        assert!(super::deletion_purpose("everything").is_none());
    }

    #[test]
    fn render_deletion_status_is_body_free_and_names_the_mark() {
        use ene_api::v1::deletion::{
            DeletionOperationStatusView, DeletionParticipantReportWire, DeletionPhaseWire,
            DeletionPurposeWire, DeletionStatusPage, DeletionStatusResponse,
        };
        use ene_api::v1::refs::{DeletionOperationWireRef, ViewMarkWire};

        let page = DeletionStatusResponse::Page(DeletionStatusPage {
            mark: ViewMarkWire(String::from("deletion-view/1/-/1/op-1")),
            operations: vec![DeletionOperationStatusView {
                operation: DeletionOperationWireRef(String::from("op-1")),
                phase: DeletionPhaseWire::Finalizing,
                purpose: DeletionPurposeWire::Privacy,
                started_at: String::from("2026-09-17T00:00:00+00:00"),
                sweep: 2,
                hold: None,
                participants: DeletionParticipantReportWire::NotReported,
            }],
            next_cursor: Some(ene_api::v1::refs::DeletionStatusCursorWire(String::from(
                "deletion-status:op-1",
            ))),
        });
        let rendered = super::render_deletion_status(&page);
        assert!(rendered.contains("mark deletion-view/1/-/1/op-1"));
        assert!(rendered.contains("op-1 finalizing privacy sweep=2"));
        assert!(rendered.contains("participants=not-reported"));
        assert!(rendered.contains("next deletion-status:op-1"));
        assert!(!rendered.contains("Debug"));
        assert_eq!(
            super::render_deletion_status(&DeletionStatusResponse::Unavailable),
            "deletion status is unavailable; retry later"
        );
    }
}
