use crate::errors::CliError;

use ene_api::v1::deletion::{
    DeletionParticipantReportWire, DeletionPurposeWire, DeletionStatusResponse, deletion_target,
};
use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome, ManagementView,
    ManagementViewRequest, RationaleOrigin, consent_target, credential_target, usage_cap_target,
};
use ene_api::v1::refs::{
    BaseViewMark, ClientLocalId, CommandWireId, CompanionWireRef, ManagementTargetWire,
    RoundWireId, TextLangWire,
};
use ene_api::v1::round::{
    HistoryItem, HistoryRequest, HistoryResponse, HistoryRole, PresentationStatus,
    RoundIntakeOutcomeWire, SubmitTextInput, TextBodyWire,
};
use ene_api::v1::undelivered::{
    GetReportSource, GetTaskReport, ListTasks, PageCursorWire, ReportSourcePageView,
    ReportSourceResponse, ReportSourceWireRef, ResumeTask, ResumeTaskOutcomeWire, SelectTask,
    SelectTaskResponse, TaskListPage, TaskListResponse, TaskReportPage, TaskReportResponse,
    TaskSelected, TaskWireRef, UndeliveredAck, UndeliveredAckOutcome, UndeliveredRequest,
    UndeliveredResponse, UndeliveredSummary,
};
use ene_api::v1::usage::{
    UsageCapConsumptionView, UsageMoneyView, UsageSummaryPage, UsageSummaryRequest,
    UsageSummaryResponse,
};
use ene_client::ClientError;

pub const DEFAULT_HISTORY_LIMIT: u64 = 50;

pub const SETUP_CREDENTIAL_LABEL: &str = "main";

pub const HOST_SETUP_SECTIONS: &[&str] =
    &["provider", "model", "consent", "credential", "learning"];

pub const HOST_MEMORY_SECTION: &str = "memory";

pub const SETUP_PROVIDER_OPENAI: &str = "openai";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Setup(SetupMode),
    Send(SendArgs),
    Watch {
        round: String,
    },
    History {
        limit: u64,
    },
    Memory {
        after: Option<String>,
        revisions: Option<String>,
        after_revision: Option<u64>,
    },
    Tasks {
        cursor: Option<String>,
        limit: Option<u32>,
    },
    Report {
        task: String,
        cursor: Option<String>,
        limit: Option<u32>,
    },
    Source {
        source: String,
        cursor: Option<u64>,
        limit_bytes: Option<u32>,
    },
    SelectTask {
        task: String,
    },
    ResumeTask {
        task: String,
        revision: u64,
        purpose: String,
        instruction: String,
    },
    Undelivered {
        cursor: Option<String>,
        limit: Option<u32>,
        redisplay: bool,
    },
    Deletion {
        text: String,
        purpose: DeletionPurposeWire,
    },
    DeletionStatus {
        cursor: Option<String>,
        limit: Option<u32>,
    },
    Usage(UsageSummaryRequest),
    UsageCap {
        provider: Option<String>,
        window: String,
        currency: String,
        limit_micros: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupMode {
    Show,
    Assign {
        provider: String,
        model: String,
        learning: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendArgs {
    pub round: Option<String>,
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

pub fn history_request(companion: &str, round: Option<&str>, limit: u64) -> HistoryRequest {
    HistoryRequest {
        companion: CompanionWireRef(companion.to_string()),
        since: None,
        limit,
        round: round.map(|round| RoundWireId(round.to_string())),
    }
}

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

fn new_local_id() -> ClientLocalId {
    ClientLocalId(uuid::Uuid::new_v4().to_string())
}

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

pub fn render_task_list(page: &TaskListPage) -> String {
    let mut lines: Vec<String> = page
        .tasks
        .iter()
        .map(|task| {
            let running = if task.running { " running" } else { "" };
            format!(
                "{} rev {} {}{} purpose {}",
                task.task.0, task.revision, task.progress, running, task.purpose
            )
        })
        .collect();
    if let Some(cursor) = &page.next_cursor {
        lines.push(format!("next: {}", cursor.0));
    }
    lines.join("\n")
}

pub fn render_report_page(page: &TaskReportPage) -> String {
    let mut lines = vec![format!(
        "{} rev {} {} purpose {} purpose-source {}",
        page.task.0, page.revision, page.progress, page.purpose, page.purpose_source.0
    )];
    for row in &page.rows {
        match &row.source {
            Some(source) => lines.push(format!("{} {} source {}", row.kind, row.id, source.0)),
            None => lines.push(format!("{} {}", row.kind, row.id)),
        }
    }
    if let Some(cursor) = &page.next_cursor {
        lines.push(format!("next: {}", cursor.0));
    }
    lines.join("\n")
}

pub fn render_source_page(page: &ReportSourcePageView) -> String {
    if let Some(next) = page.next {
        format!("{}\nnext: {next}", page.text)
    } else {
        page.text.clone()
    }
}

pub fn credential_target_for(provider: &str) -> ManagementTargetWire {
    credential_target(provider, SETUP_CREDENTIAL_LABEL)
}

pub fn credential_id_for(provider: &str) -> String {
    format!("{provider}:{SETUP_CREDENTIAL_LABEL}")
}

pub const CAPABILITY_DIALOGUE: &str = "dialogue";

pub const CAPABILITY_LEARNING: &str = "learning";

pub fn consent_target_for(capability: &str, provider: &str, model: &str) -> ManagementTargetWire {
    consent_target(capability, provider, model, &credential_id_for(provider))
}

fn intent(
    intent_id: CommandWireId,
    kind: ManagementIntentKind,
    target: ManagementTargetWire,
    base_view: BaseViewMark,
) -> ManagementIntent {
    ManagementIntent {
        intent_id,
        kind,
        target,
        base_view,
        rationale: IntentRationaleWire {
            origin: RationaleOrigin::ManagementSurface,
            quote: None,
        },
        confirmed: false,
    }
}

pub fn credential_intent(
    intent_id: CommandWireId,
    base: &BaseViewMark,
    provider: &str,
) -> ManagementIntent {
    intent(
        intent_id,
        ManagementIntentKind::ConfigureCredentialIntent,
        credential_target_for(provider),
        base.clone(),
    )
}

pub fn assignment_intent(
    intent_id: CommandWireId,
    base: &BaseViewMark,
    capability: &str,
    provider: &str,
    model: &str,
) -> ManagementIntent {
    intent(
        intent_id,
        ManagementIntentKind::ManageRuleConsentCap,
        consent_target_for(capability, provider, model),
        base.clone(),
    )
}

#[must_use]
pub fn deletion_intent(
    intent_id: CommandWireId,
    base: &str,
    purpose: DeletionPurposeWire,
    exact_text: &str,
) -> ManagementIntent {
    intent(
        intent_id,
        ManagementIntentKind::RequestDeletionBackupRestoreReset,
        deletion_target(purpose, exact_text),
        BaseViewMark(base.to_string()),
    )
}

pub fn render_deletion_status(response: &DeletionStatusResponse) -> Result<String, CliError> {
    let DeletionStatusResponse::Page(page) = response else {
        return Err(retryable("deletion status is unavailable; retry later"));
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
    Ok(lines.join("\n"))
}

#[must_use]
pub fn usage_cap_intent(
    intent_id: CommandWireId,
    base: &str,
    provider: Option<&str>,
    window: &str,
    currency: &str,
    limit_micros: u64,
) -> ManagementIntent {
    intent(
        intent_id,
        ManagementIntentKind::ManageRuleConsentCap,
        usage_cap_target(provider, window, currency, limit_micros),
        BaseViewMark(base.to_string()),
    )
}

#[must_use]
pub fn usage_cap_mark_for<'a>(
    page: &'a UsageSummaryPage,
    provider: Option<&str>,
    window: &str,
) -> Option<&'a str> {
    page.caps
        .iter()
        .find(|cap| cap.provider.as_deref() == provider && cap.window == window)
        .map(|cap| cap.mark.0.as_str())
}

pub fn render_usage_page(response: &UsageSummaryResponse) -> Result<String, CliError> {
    match response {
        UsageSummaryResponse::Unavailable => Err(retryable("usage is unavailable; retry later")),
        UsageSummaryResponse::StaleBaseView { current } => match current {
            Some(current) => Err(retryable(format!(
                "usage cursor is stale; restart from the head (current {})",
                current.0
            ))),
            None => Err(retryable("usage cursor is stale; restart from the head")),
        },
        UsageSummaryResponse::Page(page) => {
            let mut lines = vec![format!("evaluated-at {}", page.evaluated_at)];
            for row in &page.rows {
                let tokens = row.tokens.as_ref().map_or_else(
                    || String::from("-"),
                    |tokens| {
                        format!(
                            "{}/{}/{}",
                            tokens.input_tokens, tokens.cached_input_tokens, tokens.output_tokens
                        )
                    },
                );
                let cost = row.cost.as_ref().map_or_else(
                    || String::from("-"),
                    |cost| {
                        format!(
                            "{}/{}/{}/{}",
                            render_money(&cost.input),
                            render_money(&cost.cached_input),
                            render_money(&cost.output),
                            render_money(&cost.total)
                        )
                    },
                );
                let reserved = row
                    .reserved
                    .as_ref()
                    .map_or_else(|| String::from("-"), render_money);
                lines.push(format!(
                    "{} {}/{} {} {} {} tokens={} cost={} reserved={}",
                    row.started_at,
                    row.provider,
                    row.model,
                    row.consumer,
                    row.purpose,
                    row.status,
                    tokens,
                    cost,
                    reserved
                ));
            }
            for cap in &page.caps {
                let scope = cap.provider.as_deref().map_or_else(
                    || String::from("system"),
                    |provider| format!("provider={provider}"),
                );
                match &cap.stored {
                    None => lines.push(format!(
                        "cap {} {} {} no-cap",
                        cap.mark.0, scope, cap.window
                    )),
                    Some(stored) => match &stored.consumption {
                        UsageCapConsumptionView::Indeterminate => lines.push(format!(
                            "cap {} {} {} limit={} indeterminate",
                            cap.mark.0,
                            scope,
                            cap.window,
                            render_money(&stored.limit)
                        )),
                        UsageCapConsumptionView::Known {
                            reserved,
                            committed_reported,
                            committed_unknown,
                            consumed,
                            remaining,
                            held,
                        } => lines.push(format!(
                            "cap {} {} {} limit={} consumed={} reserved={} reported={} unknown={} remaining={} held={}",
                            cap.mark.0,
                            scope,
                            cap.window,
                            render_money(&stored.limit),
                            render_money(consumed),
                            render_money(reserved),
                            render_money(committed_reported),
                            render_money(committed_unknown),
                            render_money(remaining),
                            held
                        )),
                    },
                }
            }
            if let Some(next) = &page.next_cursor {
                lines.push(format!("next {}", next.0));
            }
            Ok(lines.join("\n"))
        }
    }
}

fn render_money(money: &UsageMoneyView) -> String {
    format!("{}:{}", money.currency, money.micros)
}

pub fn render_view(view: &ManagementView) -> String {
    view.sections
        .iter()
        .map(|section| format!("{}: {} – {}", section.kind, section.title, section.body))
        .collect::<Vec<String>>()
        .join("\n")
}

fn retryable(message: impl Into<String>) -> CliError {
    CliError::Client(ClientError::ServerOutcome(message.into()))
}

fn rejected(message: impl Into<String>) -> CliError {
    CliError::Client(ClientError::ServerRejected(message.into()))
}

fn role_label(role: HistoryRole) -> &'static str {
    match role {
        HistoryRole::Owner => "owner",
        HistoryRole::Companion => "companion",
    }
}

pub fn render_history(items: &[HistoryItem]) -> String {
    items
        .iter()
        .map(|item| format!("[{}] {}", role_label(item.role), item.text))
        .collect::<Vec<String>>()
        .join("\n")
}

pub fn describe_history(response: HistoryResponse) -> Result<Vec<HistoryItem>, CliError> {
    match response {
        HistoryResponse::Items(items) => Ok(items),
        HistoryResponse::InvalidRequest => Err(rejected(
            "invalid history request; correct the request fields and retry",
        )),
        HistoryResponse::Unavailable => Err(retryable("history is unavailable; retry later")),
        HistoryResponse::StaleCompanion => Err(retryable(
            "companion projection is stale; re-sync presence and retry",
        )),
    }
}

pub fn describe_intake(outcome: &RoundIntakeOutcomeWire) -> Result<String, CliError> {
    match outcome {
        RoundIntakeOutcomeWire::AcceptedForRound { round } => Ok(round.0.clone()),
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
            Err(retryable(message))
        }
        RoundIntakeOutcomeWire::HeldForTransition => Err(retryable(String::from(
            "held for a presence transition; retry after the transition settles",
        ))),
        RoundIntakeOutcomeWire::NeedsRevalidation { reason } => {
            Err(retryable(format!("needs revalidation: {}", reason.0)))
        }
    }
}

pub fn describe_ack(outcome: &UndeliveredAckOutcome) -> Result<(), CliError> {
    match outcome {
        UndeliveredAckOutcome::Presented { .. }
        | UndeliveredAckOutcome::AlreadyPresented
        | UndeliveredAckOutcome::ReturnedToPending { .. }
        | UndeliveredAckOutcome::KeptUnknown => Ok(()),
        UndeliveredAckOutcome::UnknownRef => Err(retryable(String::from(
            "unknown receipt; re-query for a new receipt and retry",
        ))),
        UndeliveredAckOutcome::StalePresentation => Err(retryable(String::from(
            "stale presentation; re-query for a new receipt and retry",
        ))),
        UndeliveredAckOutcome::StaleConnection => Err(retryable(String::from(
            "stale connection; re-query on this connection and retry",
        ))),
        UndeliveredAckOutcome::HeldForErasure => Err(retryable(String::from(
            "items are under deletion; re-query after it settles",
        ))),
        UndeliveredAckOutcome::Unavailable => Err(retryable(String::from(
            "presentation confirmation is unavailable; retry later",
        ))),
    }
}

pub fn describe_resume(outcome: &ResumeTaskOutcomeWire) -> Result<String, CliError> {
    match outcome {
        ResumeTaskOutcomeWire::Resumed {
            revision,
            delegation,
            ..
        } => Ok(format!(
            "resumed at revision {revision} (delegation {delegation})"
        )),
        ResumeTaskOutcomeWire::StalePremise { current_revision } => Err(rejected(format!(
            "stale premise; current revision is {current_revision}"
        ))),
        ResumeTaskOutcomeWire::Superseded => {
            Err(rejected(String::from("superseded by a newer Owner input")))
        }
        ResumeTaskOutcomeWire::TaskTerminal { progress } => {
            Err(rejected(format!("task is already {progress}")))
        }
        ResumeTaskOutcomeWire::AlreadyRunning => {
            Err(rejected(String::from("task is already running")))
        }
        ResumeTaskOutcomeWire::HeldByUnknownEffects => Err(rejected(String::from(
            "held by unknown effects; settle them first",
        ))),
        ResumeTaskOutcomeWire::ResultAvailable => Err(rejected(String::from(
            "a sealed result is available to review first",
        ))),
        ResumeTaskOutcomeWire::NeedsRevalidation { hold } => {
            Err(rejected(format!("needs revalidation: {hold}")))
        }
        ResumeTaskOutcomeWire::MissingTask => Err(rejected(String::from("no such task"))),
        ResumeTaskOutcomeWire::RevisionExhausted => Err(rejected(String::from(
            "the task cannot take another change",
        ))),
        ResumeTaskOutcomeWire::InFlight => Err(retryable(String::from(
            "resume already in flight; retry for its outcome",
        ))),
        ResumeTaskOutcomeWire::UnknownRef => Err(retryable(String::from(
            "unknown task reference; re-list and retry",
        ))),
        ResumeTaskOutcomeWire::StaleConnection => Err(retryable(String::from(
            "stale sender epoch; re-prepare on this connection and retry",
        ))),
        ResumeTaskOutcomeWire::Unavailable => {
            Err(retryable(String::from("host unavailable; retry later")))
        }
    }
}

pub fn describe_fetch(response: UndeliveredResponse) -> Result<UndeliveredSummary, CliError> {
    match response {
        UndeliveredResponse::Summary(summary) => Ok(summary),
        UndeliveredResponse::FrameTooLarge => Err(retryable(String::from(
            "frame too large; retry with a smaller limit",
        ))),
        UndeliveredResponse::NoCurrentPresence => Err(retryable(String::from(
            "no current presence; summon first, then retry",
        ))),
        UndeliveredResponse::UnknownCompanion => Err(retryable(String::from(
            "unknown companion; re-sync presence and retry",
        ))),
        UndeliveredResponse::StaleBaseView { .. } => Err(retryable(String::from(
            "stale base view; re-query from the head",
        ))),
        UndeliveredResponse::Unavailable => Err(retryable(String::from(
            "undelivered items are unavailable; retry later",
        ))),
    }
}

pub fn describe_report(response: TaskReportResponse) -> Result<TaskReportPage, CliError> {
    match response {
        TaskReportResponse::Page(page) => Ok(page),
        TaskReportResponse::UnknownRef => Err(retryable(String::from(
            "unknown task reference; re-list and retry",
        ))),
        TaskReportResponse::StaleBaseView { .. } => Err(retryable(String::from(
            "stale cursor; re-query from the head",
        ))),
        TaskReportResponse::Unavailable => {
            Err(retryable(String::from("host unavailable; retry later")))
        }
    }
}

pub fn describe_task_list(response: TaskListResponse) -> Result<TaskListPage, CliError> {
    match response {
        TaskListResponse::Page(page) => Ok(page),
        TaskListResponse::StaleBaseView { .. } => {
            Err(retryable("stale task-list cursor; re-query from the head"))
        }
        TaskListResponse::Unavailable => Err(retryable("task list is unavailable; retry later")),
    }
}

pub fn describe_report_source(
    response: ReportSourceResponse,
) -> Result<ReportSourcePageView, CliError> {
    match response {
        ReportSourceResponse::Page(page) => Ok(page),
        ReportSourceResponse::UnknownRef => Err(retryable(
            "unknown report source; re-read the report and retry",
        )),
        ReportSourceResponse::InputUnavailable => {
            Err(retryable("report source is unavailable; retry later"))
        }
    }
}

pub fn describe_select_task(response: SelectTaskResponse) -> Result<TaskSelected, CliError> {
    match response {
        SelectTaskResponse::Selected(selected) => Ok(selected),
        SelectTaskResponse::UnknownRef => {
            Err(retryable("unknown task reference; re-list and retry"))
        }
        SelectTaskResponse::Unavailable => {
            Err(retryable("task selection is unavailable; retry later"))
        }
    }
}

pub fn describe_management(outcome: &ManagementOutcome) -> Result<String, CliError> {
    match outcome {
        ManagementOutcome::AppliedAsOneTime => Ok(String::from("applied as a one-time approval")),
        ManagementOutcome::StoredAsRuleView { revision } => {
            Ok(format!("stored as a rule at revision {}", revision.0))
        }
        ManagementOutcome::NeedsClarification => Err(retryable(String::from(
            "nothing was applied; the request needs clarification",
        ))),
        ManagementOutcome::DeniedByBoundary => {
            Err(rejected(String::from("denied by the control boundary")))
        }
        ManagementOutcome::StaleBaseView { current } => Err(retryable(format!(
            "stale base view; current mark is {}",
            current.0
        ))),
        ManagementOutcome::HeldByOperation => Err(retryable(String::from(
            "held by a concurrent operation; retry later",
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use ene_api::v1::management::{ManagementOutcome, ManagementView, ViewSection};
    use ene_api::v1::refs::{BaseViewMark, CommandWireId, ViewMarkWire};
    use ene_api::v1::refs::{RevalidationReasonWire, RoundWireId};
    use ene_api::v1::round::{HistoryItem, HistoryRole, RoundIntakeOutcomeWire};

    use super::CliError;
    use super::{
        CAPABILITY_DIALOGUE, CAPABILITY_LEARNING, HOST_MEMORY_SECTION, HOST_SETUP_SECTIONS,
        SETUP_PROVIDER_OPENAI, assignment_intent, consent_target_for, credential_id_for,
        credential_intent, credential_target_for, describe_intake, describe_management,
        history_request, memory_view_request, new_local_id, render_history, render_view,
        setup_view_request, submit_input,
    };
    use ene_client::ClientError;

    fn expect_outcome(error: CliError) -> String {
        match error {
            CliError::Client(ClientError::ServerOutcome(message)) => message,
            other => panic!("expected a retryable server outcome, got {other:?}"),
        }
    }

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
        let round = describe_intake(&RoundIntakeOutcomeWire::AcceptedForRound {
            round: RoundWireId(String::from("round-3")),
        })
        .expect("acceptance must succeed");
        assert!(
            round == "round-3",
            "acceptance must carry the round, got {round:?}"
        );
    }

    #[test]
    fn describe_intake_decline_names_refs_not_bodies() {
        let stale = expect_outcome(
            describe_intake(&RoundIntakeOutcomeWire::StaleRound {
                current_round: Some(RoundWireId(String::from("round-4"))),
                current_generation: 9,
            })
            .expect_err("stale must decline"),
        );
        assert!(
            stale.contains("round-4") && stale.contains('9'),
            "stale must name the current round and generation: {stale:?}"
        );
        assert!(
            describe_intake(&RoundIntakeOutcomeWire::StaleRound {
                current_round: None,
                current_generation: 9,
            })
            .is_err(),
            "stale without a current round is still a decline"
        );
        assert!(
            describe_intake(&RoundIntakeOutcomeWire::HeldForTransition).is_err(),
            "held must decline"
        );
        let revalidation = expect_outcome(
            describe_intake(&RoundIntakeOutcomeWire::NeedsRevalidation {
                reason: RevalidationReasonWire(String::from("reason-1")),
            })
            .expect_err("revalidation must decline"),
        );
        assert!(
            revalidation.contains("reason-1"),
            "revalidation must name the reason code: {revalidation:?}"
        );
    }

    #[test]
    fn describe_management_splits_applied_retryable_terminal() {
        assert!(
            describe_management(&ManagementOutcome::AppliedAsOneTime).is_ok(),
            "one-time approval is applied"
        );
        let detail = describe_management(&ManagementOutcome::StoredAsRuleView {
            revision: ViewMarkWire(String::from("rev-2")),
        })
        .expect("stored rule must be applied");
        assert!(
            detail.contains("rev-2"),
            "stored rule must name the revision: {detail:?}"
        );
        for outcome in [
            ManagementOutcome::StaleBaseView {
                current: ViewMarkWire(String::from("rev-3")),
            },
            ManagementOutcome::HeldByOperation,
            ManagementOutcome::NeedsClarification,
        ] {
            let error = describe_management(&outcome).expect_err("retryable");
            assert!(
                error.exit_code() == std::process::ExitCode::from(2),
                "retryable outcomes must exit 2, got {error:?}"
            );
        }
        let denied = describe_management(&ManagementOutcome::DeniedByBoundary)
            .expect_err("denial is terminal");
        assert!(
            denied.exit_code() == std::process::ExitCode::FAILURE,
            "denial must exit 1, got {denied:?}"
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
        let history = history_request("companion-1", None, 7);
        assert!(
            history.companion.0 == "companion-1" && history.limit == 7,
            "history echoes the learned companion: {history:?}"
        );
        assert!(
            history.round.is_none(),
            "plain history reads the whole timeline: {history:?}"
        );
        let scoped = history_request("companion-1", Some("round-9"), 7);
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
            list.contains("rev 2 in_progress running")
                && list.contains("purpose task-7:2")
                && list.contains("next: cursor-2"),
            "task list renders entries plus purpose and continuation, got {list:?}"
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
            report.contains("purpose task-7:2")
                && report.contains("action_attempt")
                && report.contains("task_result"),
            "report renders purpose and both row kinds, got {report:?}"
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

        assert!(super::describe_ack(&UndeliveredAckOutcome::Presented { presented: 2 }).is_ok());
        assert!(super::describe_ack(&UndeliveredAckOutcome::AlreadyPresented).is_ok());
        let stale = super::describe_ack(&UndeliveredAckOutcome::StaleConnection)
            .expect_err("stale connection is retryable");
        assert!(
            stale.exit_code() == std::process::ExitCode::from(2),
            "retryable ack answers exit 2, got {stale:?}"
        );
        let acked = super::undelivered_ack("r", PresentationStatus::Unknown);
        assert!(acked.status == PresentationStatus::Unknown);

        assert!(
            super::describe_resume(&ResumeTaskOutcomeWire::Resumed {
                task: ene_api::v1::undelivered::TaskWireRef(String::from("t")),
                revision: 3,
                delegation: String::from("d"),
            })
            .is_ok()
        );
        let refused = super::describe_resume(&ResumeTaskOutcomeWire::StalePremise {
            current_revision: 4,
        })
        .expect_err("stale premise is refused");
        assert!(
            refused.exit_code() == std::process::ExitCode::FAILURE,
            "refusals exit 1, got {refused:?}"
        );
        assert!(
            super::describe_resume(&ResumeTaskOutcomeWire::InFlight)
                .expect_err("in flight is retryable")
                .exit_code()
                == std::process::ExitCode::from(2)
        );
        assert!(
            super::describe_resume(&ResumeTaskOutcomeWire::Unavailable)
                .expect_err("unavailable is retryable")
                .exit_code()
                == std::process::ExitCode::from(2)
        );

        assert!(
            super::describe_fetch(UndeliveredResponse::FrameTooLarge)
                .expect_err("frame too large is retryable")
                .exit_code()
                == std::process::ExitCode::from(2)
        );
        assert!(
            super::describe_fetch(UndeliveredResponse::NoCurrentPresence)
                .expect_err("no presence is retryable")
                .exit_code()
                == std::process::ExitCode::from(2)
        );
        assert!(
            super::describe_report(TaskReportResponse::UnknownRef)
                .expect_err("unknown ref is retryable")
                .exit_code()
                == std::process::ExitCode::from(2)
        );
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
        assert!(DeletionPurposeWire::from_name("privacy").is_some());
        assert!(DeletionPurposeWire::from_name("everything").is_none());
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
        let rendered = super::render_deletion_status(&page).expect("a page renders");
        assert!(rendered.contains("mark deletion-view/1/-/1/op-1"));
        assert!(rendered.contains("op-1 finalizing privacy sweep=2"));
        assert!(rendered.contains("participants=not-reported"));
        assert!(rendered.contains("next deletion-status:op-1"));
        assert!(!rendered.contains("Debug"));
        let unavailable = super::render_deletion_status(&DeletionStatusResponse::Unavailable)
            .expect_err("an unavailable read is not a rendered page");
        assert_eq!(
            expect_outcome(unavailable),
            "deletion status is unavailable; retry later"
        );
    }

    use ene_api::v1::management::{ManagementIntentKind, RationaleOrigin};
    use ene_api::v1::refs::UsageCursorWire;
    use ene_api::v1::usage::{
        UsageCapConsumptionView, UsageCapStoredView, UsageCapView, UsageCostView, UsageMoneyView,
        UsageSummaryPage, UsageSummaryResponse, UsageSummaryRowView, UsageTokenUsageView,
    };

    use super::{render_usage_page, usage_cap_intent, usage_cap_mark_for};

    fn fixture_usage_page() -> UsageSummaryPage {
        UsageSummaryPage {
            rows: vec![UsageSummaryRowView {
                provider: String::from("openai"),
                model: String::from("gpt-x"),
                consumer: String::from("companion_dialogue"),
                purpose: String::from("dialogue_response"),
                status: String::from("reported"),
                tokens: Some(UsageTokenUsageView {
                    input_tokens: 10,
                    cached_input_tokens: 4,
                    output_tokens: 2,
                }),
                cost: Some(UsageCostView {
                    input: UsageMoneyView {
                        currency: String::from("USD"),
                        micros: 6,
                    },
                    cached_input: UsageMoneyView {
                        currency: String::from("USD"),
                        micros: 1,
                    },
                    output: UsageMoneyView {
                        currency: String::from("USD"),
                        micros: 4,
                    },
                    total: UsageMoneyView {
                        currency: String::from("USD"),
                        micros: 11,
                    },
                }),
                reserved: None,
                started_at: String::from("2026-09-17T00:00:00.000000000Z"),
            }],
            next_cursor: Some(UsageCursorWire(String::from("cursor-2"))),
            caps: vec![
                UsageCapView {
                    mark: ViewMarkWire(String::from("usage-cap-system-daily_utc-rev-0")),
                    provider: None,
                    window: String::from("daily_utc"),
                    stored: Some(UsageCapStoredView {
                        limit: UsageMoneyView {
                            currency: String::from("USD"),
                            micros: 1_000,
                        },
                        consumption: UsageCapConsumptionView::Known {
                            reserved: UsageMoneyView {
                                currency: String::from("USD"),
                                micros: 200,
                            },
                            committed_reported: UsageMoneyView {
                                currency: String::from("USD"),
                                micros: 100,
                            },
                            committed_unknown: UsageMoneyView {
                                currency: String::from("USD"),
                                micros: 200,
                            },
                            consumed: UsageMoneyView {
                                currency: String::from("USD"),
                                micros: 500,
                            },
                            remaining: UsageMoneyView {
                                currency: String::from("USD"),
                                micros: 500,
                            },
                            held: false,
                        },
                    }),
                },
                UsageCapView {
                    mark: ViewMarkWire(String::from("usage-cap-provider-openai-daily_utc-none")),
                    provider: Some(String::from("openai")),
                    window: String::from("daily_utc"),
                    stored: None,
                },
            ],
            evaluated_at: String::from("2026-09-17T00:00:00.000000000Z"),
        }
    }

    #[test]
    fn usage_cap_intent_uses_the_shared_grammar_and_no_quote() {
        let intent = usage_cap_intent(
            CommandWireId(uuid::Uuid::nil()),
            "usage-cap-system-daily_utc-none",
            None,
            "daily_utc",
            "USD",
            1_000,
        );
        assert_eq!(intent.kind, ManagementIntentKind::ManageRuleConsentCap);
        assert_eq!(intent.target.0.as_str(), "cap:system:daily_utc:USD:1000");
        assert_eq!(
            intent.base_view,
            BaseViewMark(String::from("usage-cap-system-daily_utc-none"))
        );
        assert_eq!(intent.rationale.origin, RationaleOrigin::ManagementSurface);
        assert_eq!(intent.rationale.quote, None);
        let provider = usage_cap_intent(
            CommandWireId(uuid::Uuid::nil()),
            "usage-cap-provider-openai-monthly_utc-rev-2",
            Some("openai"),
            "monthly_utc",
            "USD",
            42,
        );
        assert_eq!(
            provider.target.0.as_str(),
            "cap:provider:openai:monthly_utc:USD:42"
        );
    }

    #[test]
    fn usage_cap_mark_for_selects_exactly_one_slot() {
        let page = fixture_usage_page();
        assert_eq!(
            usage_cap_mark_for(&page, None, "daily_utc"),
            Some("usage-cap-system-daily_utc-rev-0")
        );
        assert_eq!(
            usage_cap_mark_for(&page, Some("openai"), "daily_utc"),
            Some("usage-cap-provider-openai-daily_utc-none")
        );
        assert_eq!(
            usage_cap_mark_for(&page, None, "monthly_utc"),
            None,
            "an unnamed slot yields no mark instead of a guess"
        );
        assert_eq!(usage_cap_mark_for(&page, Some("other"), "daily_utc"), None);
    }

    #[test]
    fn render_usage_page_prints_rows_caps_and_the_cursor() {
        let response = UsageSummaryResponse::Page(fixture_usage_page());
        let rendered = render_usage_page(&response).expect("a page renders");
        for needle in [
            "evaluated-at 2026-09-17T00:00:00.000000000Z",
            "openai/gpt-x",
            "companion_dialogue",
            "dialogue_response",
            "reported",
            "tokens=10/4/2",
            "cost=USD:6/USD:1/USD:4/USD:11",
            "usage-cap-system-daily_utc-rev-0",
            "consumed=USD:500",
            "remaining=USD:500",
            "held=false",
            "usage-cap-provider-openai-daily_utc-none",
            "no-cap",
            "next cursor-2",
        ] {
            assert!(
                rendered.contains(needle),
                "the render must include {needle:?}: {rendered}"
            );
        }
        assert!(
            !rendered.contains("UsageSummaryPage"),
            "no Debug dumps: {rendered}"
        );
    }

    #[test]
    fn render_usage_page_keeps_stale_and_unavailable_distinct() {
        let stale = render_usage_page(&UsageSummaryResponse::StaleBaseView {
            current: Some(UsageCursorWire(String::from("cursor-9"))),
        })
        .expect_err("a stale page is not a rendered page");
        let stale = expect_outcome(stale);
        assert!(stale.contains("stale"), "stale must say so: {stale}");
        assert!(stale.contains("cursor-9"));
        let unavailable = render_usage_page(&UsageSummaryResponse::Unavailable)
            .expect_err("an unavailable read is not a rendered page");
        let unavailable = expect_outcome(unavailable);
        assert!(
            unavailable.contains("unavailable"),
            "unavailable must not read as an empty page: {unavailable}"
        );
        assert!(!unavailable.contains("next"));
    }
}
