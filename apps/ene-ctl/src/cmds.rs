use ene_api::v1::deletion::{
    DeletionParticipantReportWire, DeletionPurposeWire, DeletionStatusResponse, deletion_target,
};
use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome, ManagementView,
    ManagementViewRequest, RationaleOrigin, consent_target, credential_target, usage_cap_target,
};
use ene_api::v1::refs::{
    BaseViewMark, ClientLocalId, CommandWireId, CompanionWireRef, ManagementTargetWire,
    RoundWireId, TextLangWire, UsageCursorWire,
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
use ene_api::v1::usage::{
    UsageCapConsumptionView, UsageMoneyView, UsageSummaryPage, UsageSummaryRequest,
    UsageSummaryResponse,
};

pub const DEFAULT_COMPANION_REF: &str = "default";

pub const DEFAULT_HISTORY_LIMIT: u64 = 50;

pub const SETUP_CREDENTIAL_LABEL: &str = "main";

pub const HOST_SETUP_SECTIONS: &[&str] =
    &["provider", "model", "consent", "credential", "learning"];

pub const HOST_MEMORY_SECTION: &str = "memory";

pub const SETUP_PROVIDER_OPENAI: &str = "openai";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Setup(SetupMode),
    Status,
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
    Usage(UsageArgs),
    UsageCap {
        scope: String,
        provider: Option<String>,
        window: String,
        currency: String,
        limit_micros: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageArgs {
    pub from: Option<String>,
    pub to: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub consumer: Option<String>,
    pub purpose: Option<String>,
    pub status: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
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

pub fn history_request(companion: &str, limit: u64) -> HistoryRequest {
    HistoryRequest {
        companion: CompanionWireRef(companion.to_string()),
        since: None,
        limit,
        round: None,
    }
}

pub fn round_history_request(companion: &str, round: &str, limit: u64) -> HistoryRequest {
    HistoryRequest {
        companion: CompanionWireRef(companion.to_string()),
        since: None,
        limit,
        round: Some(RoundWireId(round.to_string())),
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

pub fn new_local_id() -> ClientLocalId {
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
        confirmed: false,
    }
}

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
        confirmed: false,
    }
}

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
        confirmed: false,
    }
}

#[must_use]
pub fn deletion_purpose(token: &str) -> Option<DeletionPurposeWire> {
    DeletionPurposeWire::from_name(token)
}

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

#[must_use]
pub fn usage_request(args: &UsageArgs) -> UsageSummaryRequest {
    UsageSummaryRequest {
        from: args.from.clone(),
        to: args.to.clone(),
        provider: args.provider.clone(),
        model: args.model.clone(),
        consumer: args.consumer.clone(),
        purpose: args.purpose.clone(),
        status: args.status.clone(),
        cursor: args.cursor.clone().map(UsageCursorWire),
        limit: args.limit,
    }
}

#[must_use]
pub fn usage_cap_intent(
    intent_id: CommandWireId,
    base: &str,
    scope: &str,
    provider: Option<&str>,
    window: &str,
    currency: &str,
    limit_micros: u64,
) -> ManagementIntent {
    ManagementIntent {
        intent_id,
        kind: ManagementIntentKind::ManageRuleConsentCap,
        target: usage_cap_target(scope, provider, window, currency, limit_micros),
        base_view: BaseViewMark(base.to_string()),
        rationale: IntentRationaleWire {
            origin: RationaleOrigin::ManagementSurface,
            quote: None,
        },
        confirmed: false,
    }
}

#[must_use]
pub fn usage_cap_mark_for<'a>(
    page: &'a UsageSummaryPage,
    scope: &str,
    provider: Option<&str>,
    window: &str,
) -> Option<&'a str> {
    page.caps
        .iter()
        .find(|cap| {
            cap.scope == scope && cap.provider.as_deref() == provider && cap.window == window
        })
        .map(|cap| cap.mark.as_str())
}

#[must_use]
pub fn render_usage_page(response: &UsageSummaryResponse) -> String {
    match response {
        UsageSummaryResponse::Unavailable => String::from("usage is unavailable; retry later"),
        UsageSummaryResponse::StaleBaseView { current } => match current {
            Some(current) => format!(
                "usage cursor is stale; restart from the head (current {})",
                current.0
            ),
            None => String::from("usage cursor is stale; restart from the head"),
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
                        cap.mark, scope, cap.window
                    )),
                    Some(stored) => match &stored.consumption {
                        UsageCapConsumptionView::Indeterminate => lines.push(format!(
                            "cap {} {} {} limit={} indeterminate",
                            cap.mark,
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
                            cap.mark,
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
            lines.join("\n")
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

pub fn role_label(role: HistoryRole) -> &'static str {
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
        UndeliveredAckOutcome::HeldForErasure => AckAction::Retryable {
            message: String::from("items are under deletion; re-query after it settles"),
        },
    }
}

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
