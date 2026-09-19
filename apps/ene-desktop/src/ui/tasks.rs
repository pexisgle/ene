//! Task / Workspace management view-model.
//!
//! Host is durable authority. This module talks existing Client wire DTOs
//! only ([`ListTasks`], [`GetTaskReport`], [`GetReportSource`], [`SelectTask`],
//! [`ResumeTask`], [`ManagementIntentKind::CancelTask`],
//! [`ManagementIntentKind::SelectWorkspace`]). It does not know the DB schema
//! and does not mint Tasks: creation stays on the companion delegation path
//! (chat). Resume is explicit and bound to the Task revision / purpose the
//! panel currently displays; a stale premise is shown, never rewritten to the
//! latest behind the Owner's back.
//!
//! Presentation ACK for undelivered Task facts is issued only after this
//! panel has copied a receipt into its displayed state. Conversation stream
//! ACK already lives in [`crate::session::submit_and_collect`] (slice E can
//! keep that as the chat hook).

use std::path::Path;
use std::time::Duration;

use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome,
    RationaleOrigin, task_target, workspace_target,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{BaseViewMark, CommandWireId, RoundWireId};
use ene_api::v1::round::PresentationStatus;
use ene_api::v1::undelivered::{
    GetReportSource, GetTaskReport, ListTasks, ReportSourceResponse, ReportSourceWireRef,
    ResumeTask, ResumeTaskOutcomeWire, SelectTask, SelectTaskResponse, TaskListItem,
    TaskListResponse, TaskReportPage, TaskReportResponse, TaskWireRef, UndeliveredAck,
    UndeliveredAckOutcome, UndeliveredRequest, UndeliveredResponse, UndeliveredSummary,
};
use ene_client::Client;

use crate::ui::DesktopError;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Connection-scoped Task / Workspace projection. Wire refs die on reconnect.
#[derive(Debug, Default)]
pub(crate) struct TaskPanel {
    items: Vec<TaskListItem>,
    displayed: Option<DisplayedTask>,
    purpose_text: String,
    result_text: String,
    result_adopted: Option<u64>,
    action_lines: Vec<String>,
    workspace_path: Option<String>,
    undelivered_lines: Vec<String>,
    /// Set only after a receipt's items were copied into this panel.
    presented: Option<PresentedReceipt>,
    last_resume: Option<String>,
    last_cancel: Option<String>,
    last_ack: Option<String>,
}

/// Premise the Owner is looking at. Resume echoes this, never a fresher list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DisplayedTask {
    pub task: String,
    pub revision: u64,
    pub purpose: String,
    pub progress: String,
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PresentedReceipt {
    receipt: String,
    round: RoundWireId,
}

impl TaskPanel {
    #[must_use]
    pub(crate) fn list_lines(&self) -> Vec<String> {
        self.items.iter().map(list_line).collect()
    }

    #[must_use]
    pub(crate) fn detail_text(&self) -> String {
        let mut lines = Vec::new();
        match &self.displayed {
            None => lines.push(String::from("no task selected")),
            Some(shown) => {
                let running = if shown.running { "yes" } else { "no" };
                lines.push(format!(
                    "selected {} rev {} {} running={running}",
                    shown.task, shown.revision, shown.progress
                ));
                lines.push(format!("purpose-id {}", shown.purpose));
                lines.push(format!("interrupted={}", is_interrupted(shown)));
            }
        }
        if let Some(path) = &self.workspace_path {
            lines.push(format!("workspace {path}"));
        }
        if !self.purpose_text.is_empty() {
            lines.push(format!("purpose-text {}", self.purpose_text.trim()));
        }
        if self.result_text.is_empty() {
            lines.push(String::from("result none"));
        } else {
            let adopted = self
                .result_adopted
                .map(|rev| format!("adopted-rev {rev}"))
                .unwrap_or_else(|| String::from("recorded-not-adopted"));
            lines.push(format!("result ({adopted}) {}", self.result_text.trim()));
        }
        if self.action_lines.is_empty() {
            lines.push(String::from("actions none"));
        } else {
            lines.extend(self.action_lines.iter().cloned());
        }
        if self.undelivered_lines.is_empty() {
            lines.push(String::from("undelivered none"));
        } else {
            lines.push(String::from("undelivered:"));
            lines.extend(self.undelivered_lines.iter().cloned());
        }
        if self.presented.is_some() {
            lines.push(String::from("presentation ready-to-ack"));
        } else {
            lines.push(String::from("presentation not-presented"));
        }
        if let Some(cancel) = &self.last_cancel {
            lines.push(format!("cancel {cancel}"));
        }
        if let Some(resume) = &self.last_resume {
            lines.push(format!("resume {resume}"));
        }
        if let Some(ack) = &self.last_ack {
            lines.push(format!("ack {ack}"));
        }
        lines.join("\n")
    }

    #[must_use]
    pub(crate) fn displayed(&self) -> Option<&DisplayedTask> {
        self.displayed.as_ref()
    }

    #[must_use]
    pub(crate) fn has_presented_receipt(&self) -> bool {
        self.presented.is_some()
    }

    /// Drops connection-scoped refs. Workspace path is the last Owner-sent
    /// folder (display of our own intent), not a Host master.
    pub(crate) fn reset_connection_state(&mut self) {
        let workspace_path = self.workspace_path.clone();
        *self = Self {
            workspace_path,
            ..Self::default()
        };
    }

    pub(crate) async fn refresh_list(&mut self, client: &mut Client) -> Result<(), DesktopError> {
        let answer = request(
            client,
            WirePayload::ListTasks(ListTasks {
                cursor: None,
                limit: None,
            }),
        )
        .await?;
        let WirePayload::TaskListResponse(TaskListResponse::Page(page)) = answer else {
            return Err(DesktopError::Protocol(format!(
                "list tasks answered {}",
                answer.message_type()
            )));
        };
        self.items = page.tasks;
        if let Some(shown) = &self.displayed {
            let still = self.items.iter().any(|item| same_listed_task(item, shown));
            if !still {
                self.clear_selection_body();
            } else {
                self.sync_lifecycle_from_list();
            }
        }
        Ok(())
    }

    pub(crate) async fn select(
        &mut self,
        client: &mut Client,
        index: usize,
    ) -> Result<(), DesktopError> {
        let item =
            self.items.get(index).cloned().ok_or_else(|| {
                DesktopError::Protocol(String::from("no listed task at that index"))
            })?;
        let answer = request(
            client,
            WirePayload::SelectTask(SelectTask {
                task: item.task.clone(),
            }),
        )
        .await?;
        match answer {
            WirePayload::SelectTaskResponse(SelectTaskResponse::Selected(selected)) => {
                self.displayed = Some(DisplayedTask {
                    task: selected.task.0,
                    revision: selected.revision,
                    purpose: selected.purpose,
                    progress: selected.progress,
                    running: item.running,
                });
            }
            WirePayload::SelectTaskResponse(SelectTaskResponse::UnknownRef) => {
                return Err(DesktopError::Protocol(String::from(
                    "task ref is unknown on this connection",
                )));
            }
            other => {
                return Err(DesktopError::Protocol(format!(
                    "select task answered {}",
                    other.message_type()
                )));
            }
        }
        self.load_report(client).await
    }

    pub(crate) async fn select_workspace(
        &mut self,
        client: &mut Client,
        mark: &str,
        path: &Path,
    ) -> Result<ManagementOutcome, DesktopError> {
        let path_text = path.to_string_lossy().into_owned();
        let answer = request(
            client,
            WirePayload::ManagementIntent(ManagementIntent {
                intent_id: CommandWireId(uuid::Uuid::new_v4()),
                kind: ManagementIntentKind::SelectWorkspace,
                target: workspace_target(&path_text),
                base_view: BaseViewMark(mark.to_string()),
                rationale: IntentRationaleWire {
                    origin: RationaleOrigin::ManagementSurface,
                    quote: None,
                },
                confirmed: false,
            }),
        )
        .await?;
        let WirePayload::ManagementOutcome(outcome) = answer else {
            return Err(DesktopError::Protocol(format!(
                "select workspace answered {}",
                answer.message_type()
            )));
        };
        if matches!(outcome, ManagementOutcome::AppliedAsOneTime) {
            self.workspace_path = Some(path_text);
        }
        Ok(outcome)
    }

    /// Cancel the displayed Task through first-party [`CancelTask`].
    ///
    /// The management target uses the Host-published purpose identity
    /// (`{task}:{adopted_revision}`), not a SQLite read. Acceptance
    /// (`AppliedAsOneTime`) is not stop-complete.
    pub(crate) async fn cancel_displayed(
        &mut self,
        client: &mut Client,
        mark: &str,
    ) -> Result<ManagementOutcome, DesktopError> {
        let shown = self
            .displayed
            .clone()
            .ok_or_else(|| DesktopError::Protocol(String::from("cancel needs a displayed task")))?;
        let task = task_id_from_purpose(&shown.purpose)?;
        let answer = request(
            client,
            WirePayload::ManagementIntent(ManagementIntent {
                intent_id: CommandWireId(uuid::Uuid::new_v4()),
                kind: ManagementIntentKind::CancelTask,
                target: task_target(task),
                base_view: BaseViewMark(mark.to_string()),
                rationale: IntentRationaleWire {
                    origin: RationaleOrigin::ManagementSurface,
                    quote: None,
                },
                confirmed: false,
            }),
        )
        .await?;
        let WirePayload::ManagementOutcome(outcome) = answer else {
            return Err(DesktopError::Protocol(format!(
                "cancel answered {}",
                answer.message_type()
            )));
        };
        self.last_cancel = Some(cancel_label(&outcome, shown.running));
        Ok(outcome)
    }

    /// Resume using the displayed revision / purpose. Does not refresh the
    /// list first, so a stale view cannot be silently replaced with latest.
    pub(crate) async fn resume_displayed(
        &mut self,
        client: &mut Client,
        instruction: String,
    ) -> Result<ResumeTaskOutcomeWire, DesktopError> {
        let shown = self
            .displayed
            .clone()
            .ok_or_else(|| DesktopError::Protocol(String::from("resume needs a displayed task")))?;
        if instruction.trim().is_empty() {
            return Err(DesktopError::Protocol(String::from(
                "resume needs an instruction",
            )));
        }
        let command = resume_from_displayed(&shown, instruction);
        let prepared = client.prepare(WirePayload::ResumeTask(command));
        let answer = tokio::time::timeout(REQUEST_TIMEOUT, client.execute(&prepared))
            .await
            .map_err(|_| DesktopError::Transport(String::from("client request timed out")))?
            .map_err(DesktopError::Client)?;
        let WirePayload::ResumeTaskOutcome(outcome) = answer else {
            return Err(DesktopError::Protocol(format!(
                "resume answered {}",
                answer.message_type()
            )));
        };
        self.last_resume = Some(resume_label(&outcome, shown.revision, &shown.purpose));
        Ok(outcome)
    }

    /// Copies one undelivered page into the panel. ACK is a separate step.
    pub(crate) async fn present_undelivered(
        &mut self,
        client: &mut Client,
    ) -> Result<(), DesktopError> {
        let mut summary = take_pushed_summary(client);
        if summary.is_none() {
            let answer = request(
                client,
                WirePayload::UndeliveredRequest(UndeliveredRequest {
                    companion: None,
                    cursor: None,
                    limit: None,
                    redisplay: false,
                }),
            )
            .await?;
            match answer {
                WirePayload::UndeliveredResponse(UndeliveredResponse::Summary(page)) => {
                    summary = Some(page);
                }
                WirePayload::UndeliveredResponse(other) => {
                    return Err(DesktopError::Protocol(format!(
                        "undelivered was not a summary: {other:?}"
                    )));
                }
                other => {
                    return Err(DesktopError::Protocol(format!(
                        "undelivered answered {}",
                        other.message_type()
                    )));
                }
            }
        }
        let Some(summary) = summary else {
            self.undelivered_lines.clear();
            self.presented = None;
            return Ok(());
        };
        self.undelivered_lines = summary
            .items
            .iter()
            .map(|item| {
                let certainty = item.source.certainty.as_deref().unwrap_or("unset");
                format!(
                    "{} {} certainty={certainty}",
                    item.source.kind, item.source.subject
                )
            })
            .collect();
        for report in &summary.reports {
            self.undelivered_lines.push(format!(
                "headline {} rev {} {}",
                report.task.0, report.revision, report.progress
            ));
        }
        merge_action_certainty(&mut self.action_lines, &summary);
        self.presented = Some(PresentedReceipt {
            receipt: summary.receipt.0,
            round: summary.round,
        });
        Ok(())
    }

    /// ACK only after [`Self::present_undelivered`]. Receiving a frame is not
    /// presentation.
    pub(crate) async fn ack_presented(
        &mut self,
        client: &mut Client,
    ) -> Result<UndeliveredAckOutcome, DesktopError> {
        let Some(presented) = self.presented.clone() else {
            return Err(DesktopError::Protocol(String::from(
                "ack requires a presented receipt",
            )));
        };
        let answer = tokio::time::timeout(
            REQUEST_TIMEOUT,
            client.request_observed(
                WirePayload::UndeliveredAck(UndeliveredAck {
                    receipt: ene_api::v1::undelivered::PresentationReceiptWireRef(
                        presented.receipt,
                    ),
                    status: PresentationStatus::Presented,
                }),
                Some(presented.round),
            ),
        )
        .await
        .map_err(|_| DesktopError::Transport(String::from("client request timed out")))?
        .map_err(DesktopError::Client)?;
        let WirePayload::UndeliveredAckOutcome(outcome) = answer else {
            return Err(DesktopError::Protocol(format!(
                "ack answered {}",
                answer.message_type()
            )));
        };
        self.last_ack = Some(ack_label(&outcome));
        self.presented = None;
        Ok(outcome)
    }

    async fn load_report(&mut self, client: &mut Client) -> Result<(), DesktopError> {
        let shown = self
            .displayed
            .clone()
            .ok_or_else(|| DesktopError::Protocol(String::from("report needs a displayed task")))?;
        let answer = request(
            client,
            WirePayload::GetTaskReport(GetTaskReport {
                task: TaskWireRef(shown.task.clone()),
                cursor: None,
                limit: None,
            }),
        )
        .await?;
        let page = match answer {
            WirePayload::TaskReportResponse(TaskReportResponse::Page(page)) => page,
            WirePayload::TaskReportResponse(TaskReportResponse::UnknownRef) => {
                return Err(DesktopError::Protocol(String::from(
                    "task report ref is unknown",
                )));
            }
            WirePayload::TaskReportResponse(TaskReportResponse::StaleBaseView { .. }) => {
                return Err(DesktopError::Protocol(String::from(
                    "task report cursor is stale",
                )));
            }
            other => {
                return Err(DesktopError::Protocol(format!(
                    "task report answered {}",
                    other.message_type()
                )));
            }
        };
        self.apply_report(client, page).await
    }

    async fn apply_report(
        &mut self,
        client: &mut Client,
        page: TaskReportPage,
    ) -> Result<(), DesktopError> {
        if let Some(shown) = &mut self.displayed {
            shown.revision = page.revision;
            shown.progress = page.progress.clone();
            shown.purpose = page.purpose.clone();
        }
        self.purpose_text = load_source(client, &page.purpose_source).await?;
        self.result_text.clear();
        self.result_adopted = None;
        self.action_lines.clear();
        for row in &page.rows {
            match row.kind.as_str() {
                "task_result" => {
                    self.result_adopted = row.adopted_revision;
                    if let Some(source) = &row.source {
                        self.result_text = load_source(client, source).await?;
                    }
                    let adopted = row
                        .adopted_revision
                        .map(|rev| format!("adopted-rev {rev}"))
                        .unwrap_or_else(|| String::from("recorded-not-adopted"));
                    self.action_lines
                        .push(format!("task_result {} {adopted}", row.id));
                }
                "action_attempt" => {
                    self.action_lines
                        .push(format!("action_attempt {} certainty=unset", row.id));
                }
                other => {
                    self.action_lines.push(format!("{other} {}", row.id));
                }
            }
        }
        Ok(())
    }

    fn clear_selection_body(&mut self) {
        self.displayed = None;
        self.purpose_text.clear();
        self.result_text.clear();
        self.result_adopted = None;
        self.action_lines.clear();
        self.undelivered_lines.clear();
        self.presented = None;
    }

    /// Lifecycle flags may move (cancel admission, runner stop). Revision and
    /// purpose stay as the Owner-selected resume premise.
    fn sync_lifecycle_from_list(&mut self) {
        let Some(shown) = &mut self.displayed else {
            return;
        };
        if let Some(item) = self.items.iter().find(|item| same_listed_task(item, shown)) {
            shown.progress = item.progress.clone();
            shown.running = item.running;
        }
    }
}

fn list_line(item: &TaskListItem) -> String {
    let running = if item.running { " running" } else { "" };
    format!(
        "{} rev {} {}{}",
        item.task.0, item.revision, item.progress, running
    )
}

fn is_interrupted(shown: &DisplayedTask) -> bool {
    shown.progress == "in_progress" && !shown.running
}

fn same_listed_task(item: &TaskListItem, shown: &DisplayedTask) -> bool {
    item.task.0 == shown.task || task_key(&item.purpose) == task_key(&shown.purpose)
}

fn task_key(purpose: &str) -> Option<&str> {
    purpose.split_once(':').map(|(task, _)| task)
}

fn resume_from_displayed(shown: &DisplayedTask, instruction: String) -> ResumeTask {
    ResumeTask {
        task: TaskWireRef(shown.task.clone()),
        expected_revision: shown.revision,
        expected_purpose: shown.purpose.clone(),
        instruction,
    }
}

fn task_id_from_purpose(purpose: &str) -> Result<uuid::Uuid, DesktopError> {
    let (task, _) = purpose.split_once(':').ok_or_else(|| {
        DesktopError::Protocol(String::from("purpose identity is missing a task id"))
    })?;
    uuid::Uuid::parse_str(task)
        .map_err(|_| DesktopError::Protocol(String::from("purpose identity is not a task id")))
}

fn cancel_label(outcome: &ManagementOutcome, was_running: bool) -> String {
    match outcome {
        ManagementOutcome::AppliedAsOneTime if was_running => {
            String::from("accepted (stop not yet complete)")
        }
        ManagementOutcome::AppliedAsOneTime => String::from("accepted"),
        other => format!("{other:?}"),
    }
}

fn resume_label(outcome: &ResumeTaskOutcomeWire, expected: u64, purpose: &str) -> String {
    match outcome {
        ResumeTaskOutcomeWire::Resumed { revision, .. } => {
            format!("resumed revision {revision}")
        }
        ResumeTaskOutcomeWire::StalePremise { current_revision } => {
            format!("stale displayed-rev {expected} purpose {purpose} current {current_revision}")
        }
        ResumeTaskOutcomeWire::TaskTerminal { progress } => {
            format!("terminal {progress}")
        }
        other => format!("{other:?}"),
    }
}

fn ack_label(outcome: &UndeliveredAckOutcome) -> String {
    match outcome {
        UndeliveredAckOutcome::Presented { presented } => format!("presented {presented}"),
        UndeliveredAckOutcome::AlreadyPresented => String::from("already-presented"),
        other => format!("{other:?}"),
    }
}

fn take_pushed_summary(client: &mut Client) -> Option<UndeliveredSummary> {
    let mut found = None;
    for frame in client.take_undelivered() {
        if found.is_none()
            && let WirePayload::UndeliveredResponse(UndeliveredResponse::Summary(summary)) =
                frame.payload
        {
            found = Some(summary);
        }
    }
    found
}

fn merge_action_certainty(action_lines: &mut [String], summary: &UndeliveredSummary) {
    for item in &summary.items {
        if item.source.kind != "action_attempt" {
            continue;
        }
        let Some(certainty) = &item.source.certainty else {
            continue;
        };
        for line in action_lines.iter_mut() {
            if line.contains(&item.source.subject) && line.contains("certainty=unset") {
                *line = line.replace("certainty=unset", &format!("certainty={certainty}"));
            }
        }
    }
}

async fn load_source(
    client: &mut Client,
    source: &ReportSourceWireRef,
) -> Result<String, DesktopError> {
    let mut cursor = None;
    let mut text = String::new();
    loop {
        let answer = request(
            client,
            WirePayload::GetReportSource(GetReportSource {
                source: source.clone(),
                cursor,
                limit_bytes: None,
            }),
        )
        .await?;
        match answer {
            WirePayload::ReportSourceResponse(ReportSourceResponse::Page(page)) => {
                text.push_str(&page.text);
                match page.next {
                    Some(next) => cursor = Some(next),
                    None => break,
                }
            }
            WirePayload::ReportSourceResponse(ReportSourceResponse::InputUnavailable) => {
                return Ok(String::from("(unavailable)"));
            }
            WirePayload::ReportSourceResponse(ReportSourceResponse::UnknownRef) => {
                return Ok(String::from("(unknown-source)"));
            }
            other => {
                return Err(DesktopError::Protocol(format!(
                    "report source answered {}",
                    other.message_type()
                )));
            }
        }
    }
    Ok(text)
}

async fn request(client: &mut Client, payload: WirePayload) -> Result<WirePayload, DesktopError> {
    tokio::time::timeout(REQUEST_TIMEOUT, client.request(payload))
        .await
        .map_err(|_| DesktopError::Transport(String::from("client request timed out")))?
        .map_err(DesktopError::Client)
}

#[cfg(test)]
mod tests {
    use super::{
        DisplayedTask, ResumeTaskOutcomeWire, is_interrupted, resume_from_displayed, resume_label,
        same_listed_task, task_id_from_purpose,
    };
    use ene_api::v1::undelivered::{TaskListItem, TaskWireRef};

    fn shown(revision: u64) -> DisplayedTask {
        DisplayedTask {
            task: String::from("wire-1"),
            revision,
            purpose: String::from("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa:1"),
            progress: String::from("in_progress"),
            running: false,
        }
    }

    #[test]
    fn interrupted_is_in_progress_without_execution_registration() {
        let mut item = shown(1);
        assert!(is_interrupted(&item));
        item.running = true;
        assert!(!is_interrupted(&item));
        item.running = false;
        item.progress = String::from("cancelled");
        assert!(!is_interrupted(&item));
        item.progress = String::from("failed");
        assert!(!is_interrupted(&item));
        item.progress = String::from("completed");
        assert!(!is_interrupted(&item));
    }

    #[test]
    fn resume_command_echoes_the_displayed_premise_not_a_later_list() {
        let displayed = shown(1);
        let mut latest = displayed.clone();
        latest.revision = 4;
        latest.purpose = String::from("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb:4");
        let command = resume_from_displayed(&displayed, String::from("continue remaining work"));
        assert_eq!(command.task, TaskWireRef(String::from("wire-1")));
        assert_eq!(command.expected_revision, 1);
        assert_eq!(
            command.expected_purpose,
            "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa:1"
        );
        assert_ne!(command.expected_revision, latest.revision);
    }

    #[test]
    fn stale_resume_label_keeps_the_displayed_revision() {
        let label = resume_label(
            &ResumeTaskOutcomeWire::StalePremise {
                current_revision: 2,
            },
            1,
            "purpose-1",
        );
        assert!(label.contains("displayed-rev 1"), "{label}");
        assert!(label.contains("current 2"), "{label}");
        assert!(!label.contains("auto"));
    }

    #[test]
    fn purpose_identity_yields_the_management_task_target() {
        let id = task_id_from_purpose("01234567-89ab-cdef-0123-456789abcdef:3")
            .expect("purpose identity parses");
        assert_eq!(id.to_string(), "01234567-89ab-cdef-0123-456789abcdef");
    }

    #[test]
    fn list_refresh_keeps_the_displayed_premise_when_wire_refs_rotate() {
        let shown = shown(1);
        let mut rotated = TaskListItem {
            task: TaskWireRef(String::from("wire-2")),
            revision: 2,
            progress: String::from("in_progress"),
            running: true,
            purpose: shown.purpose.clone(),
        };
        assert!(same_listed_task(&rotated, &shown));
        rotated.purpose = String::from("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb:4");
        assert!(!same_listed_task(&rotated, &shown));
    }

    #[test]
    fn resume_debug_does_not_carry_the_instruction() {
        let command = resume_from_displayed(&shown(1), String::from("sk-should-not-leak"));
        let rendered = format!("{command:?}");
        assert!(
            !rendered.contains("sk-should-not-leak"),
            "instruction redacted: {rendered}"
        );
    }
}
