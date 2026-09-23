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
use crate::ui::{request_observed_with_timeout, request_with_timeout};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Default)]
pub(crate) struct TaskPanel {
    items: Vec<TaskListItem>,
    displayed: Option<DisplayedTask>,
    purpose_text: String,
    result_text: String,
    result_adopted: Option<u64>,
    action_lines: Vec<String>,
    report_rows: Vec<super::presentation::Row>,
    certainty: std::collections::BTreeMap<String, String>,
    workspace_path: Option<String>,
    undelivered_lines: Vec<String>,
    presented: Option<PresentedReceipt>,
}

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
    generation: u64,
}

impl TaskPanel {
    pub(crate) fn rows(&self, locale: crate::i18n::Locale) -> Vec<super::presentation::Row> {
        use super::presentation::{Row, state, task_key, tr};
        self.items
            .iter()
            .enumerate()
            .map(|(index, task)| Row {
                key: task_key(&task.task.0, task.revision, &task.purpose),
                title: format!("{} {}", tr(locale, "作業", "Task"), index + 1),
                state: if task.running {
                    tr(locale, "実行中", "Running")
                } else {
                    state(locale, &task.progress)
                },
                ..Row::default()
            })
            .collect()
    }
    pub(crate) fn selected_key(&self) -> String {
        self.displayed
            .as_ref()
            .map(|t| super::presentation::task_key(&t.task, t.revision, &t.purpose))
            .unwrap_or_default()
    }
    pub(crate) fn index_for_key(&self, key: &str) -> Option<usize> {
        self.items
            .iter()
            .position(|t| super::presentation::task_key(&t.task.0, t.revision, &t.purpose) == key)
    }
    pub(crate) fn details(&self, locale: crate::i18n::Locale) -> Vec<super::presentation::Row> {
        use super::presentation::{Row, tr};
        if self.displayed.is_none() {
            return Vec::new();
        }
        let mut rows = vec![
            Row::text(&tr(locale, "目的", "Purpose"), &self.purpose_text),
            Row::text(
                &tr(locale, "結果", "Result"),
                if self.result_text.is_empty() {
                    tr(locale, "まだ結果はありません", "No result yet")
                } else {
                    self.result_text.clone()
                },
            ),
        ];
        if let Some(path) = &self.workspace_path {
            rows.push(Row::text("Workspace", path));
        }
        for (index, row) in self.report_rows.iter().enumerate() {
            rows.push(Row {
                title: format!(
                    "{} {}",
                    tr(locale, "成果物・操作", "Artifacts and actions"),
                    index + 1
                ),
                ..row.clone()
            });
        }
        rows
    }

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
        lines.join("\n")
    }

    #[must_use]
    pub(crate) fn has_presented_receipt(&self) -> bool {
        self.presented.is_some()
    }

    pub(crate) fn reset_connection_state(&mut self) {
        let workspace_path = self.workspace_path.clone();
        *self = Self {
            workspace_path,
            ..Self::default()
        };
    }

    #[must_use]
    pub(crate) fn presentation_cleared(&self) -> bool {
        self.items.is_empty()
            && self.displayed.is_none()
            && self.purpose_text.is_empty()
            && self.result_text.is_empty()
            && self.action_lines.is_empty()
            && self.report_rows.is_empty()
            && self.undelivered_lines.is_empty()
            && self.presented.is_none()
    }

    pub(crate) async fn refresh_list(&mut self, client: &mut Client) -> Result<(), DesktopError> {
        let mut cursor = None;
        let mut items = Vec::new();
        loop {
            let answer = request_with_timeout(
                client,
                WirePayload::ListTasks(ListTasks {
                    cursor,
                    limit: None,
                }),
                REQUEST_TIMEOUT,
            )
            .await?;
            let page = match answer {
                WirePayload::TaskListResponse(TaskListResponse::Page(page)) => page,
                WirePayload::TaskListResponse(TaskListResponse::StaleBaseView { .. }) => {
                    return Err(DesktopError::Stale(String::from(
                        "task list cursor is stale",
                    )));
                }
                WirePayload::TaskListResponse(TaskListResponse::Unavailable) => {
                    return Err(DesktopError::Unavailable(String::from(
                        "task list is unavailable; retry later",
                    )));
                }
                other => {
                    return Err(DesktopError::Protocol(format!(
                        "list tasks answered {}",
                        other.message_type()
                    )));
                }
            };
            let next = page.next_cursor;
            items.extend(page.tasks);
            match next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        self.items = items;
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
        let answer = request_with_timeout(
            client,
            WirePayload::SelectTask(SelectTask {
                task: item.task.clone(),
            }),
            REQUEST_TIMEOUT,
        )
        .await?;
        match answer {
            WirePayload::SelectTaskResponse(SelectTaskResponse::Selected(selected)) => {
                if selected.revision != item.revision || selected.purpose != item.purpose {
                    return Err(DesktopError::Protocol(String::from("stale task selection")));
                }
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
            WirePayload::SelectTaskResponse(SelectTaskResponse::Unavailable) => {
                return Err(DesktopError::Unavailable(String::from(
                    "task selection is unavailable; retry later",
                )));
            }
            other => {
                return Err(DesktopError::Protocol(format!(
                    "select task answered {}",
                    other.message_type()
                )));
            }
        }
        match self.load_report(client).await {
            Ok(()) => Ok(()),
            Err(error) => {
                self.clear_selection_body();
                Err(error)
            }
        }
    }

    pub(crate) async fn select_workspace(
        &mut self,
        client: &mut Client,
        mark: &str,
        path: &Path,
    ) -> Result<ManagementOutcome, DesktopError> {
        let path_text = path.to_string_lossy().into_owned();
        let answer = request_with_timeout(
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
            REQUEST_TIMEOUT,
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
        let answer = request_with_timeout(
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
            REQUEST_TIMEOUT,
        )
        .await?;
        let WirePayload::ManagementOutcome(outcome) = answer else {
            return Err(DesktopError::Protocol(format!(
                "cancel answered {}",
                answer.message_type()
            )));
        };
        Ok(outcome)
    }

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
        let answer =
            request_with_timeout(client, WirePayload::ResumeTask(command), REQUEST_TIMEOUT).await?;
        let WirePayload::ResumeTaskOutcome(outcome) = answer else {
            return Err(DesktopError::Protocol(format!(
                "resume answered {}",
                answer.message_type()
            )));
        };
        Ok(outcome)
    }

    pub(crate) async fn present_undelivered(
        &mut self,
        client: &mut Client,
    ) -> Result<(), DesktopError> {
        let summary = if let Some(summary) = take_pushed_summary(client) {
            summary
        } else {
            let answer = request_with_timeout(
                client,
                WirePayload::UndeliveredRequest(UndeliveredRequest {
                    companion: None,
                    cursor: None,
                    limit: None,
                    redisplay: false,
                }),
                REQUEST_TIMEOUT,
            )
            .await?;
            match answer {
                WirePayload::UndeliveredResponse(UndeliveredResponse::Summary(page)) => page,
                WirePayload::UndeliveredResponse(UndeliveredResponse::Unavailable) => {
                    return Err(DesktopError::Unavailable(String::from(
                        "the undelivered pass could not be read; retry later",
                    )));
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
        self.certainty = summary
            .items
            .iter()
            .filter_map(|item| {
                item.source
                    .certainty
                    .as_ref()
                    .map(|certainty| (item.source.subject.clone(), certainty.clone()))
            })
            .collect();
        self.presented = Some(PresentedReceipt {
            receipt: summary.receipt.0,
            round: summary.round,
            generation: summary.presence_generation,
        });
        Ok(())
    }

    pub(crate) async fn ack_presented(
        &mut self,
        client: &mut Client,
    ) -> Result<UndeliveredAckOutcome, DesktopError> {
        let Some(presented) = self.presented.clone() else {
            return Err(DesktopError::Protocol(String::from(
                "ack requires a presented receipt",
            )));
        };
        let answer = request_observed_with_timeout(
            client,
            WirePayload::UndeliveredAck(UndeliveredAck {
                receipt: ene_api::v1::undelivered::PresentationReceiptWireRef(presented.receipt),
                status: PresentationStatus::Presented,
            }),
            Some(presented.round),
            Some(presented.generation),
            REQUEST_TIMEOUT,
        )
        .await?;
        let WirePayload::UndeliveredAckOutcome(outcome) = answer else {
            return Err(DesktopError::Protocol(format!(
                "ack answered {}",
                answer.message_type()
            )));
        };
        if !matches!(outcome, UndeliveredAckOutcome::Unavailable) {
            self.presented = None;
        }
        Ok(outcome)
    }

    async fn load_report(&mut self, client: &mut Client) -> Result<(), DesktopError> {
        let shown = self
            .displayed
            .clone()
            .ok_or_else(|| DesktopError::Protocol(String::from("report needs a displayed task")))?;
        let mut cursor = None;
        let mut merged: Option<TaskReportPage> = None;
        loop {
            let answer = request_with_timeout(
                client,
                WirePayload::GetTaskReport(GetTaskReport {
                    task: TaskWireRef(shown.task.clone()),
                    cursor,
                    limit: None,
                }),
                REQUEST_TIMEOUT,
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
                    return Err(DesktopError::Stale(String::from(
                        "task report cursor is stale",
                    )));
                }
                WirePayload::TaskReportResponse(TaskReportResponse::Unavailable) => {
                    return Err(DesktopError::Unavailable(String::from(
                        "task report is unavailable; retry later",
                    )));
                }
                other => {
                    return Err(DesktopError::Protocol(format!(
                        "task report answered {}",
                        other.message_type()
                    )));
                }
            };
            let next = page.next_cursor.clone();
            if let Some(existing) = merged.as_mut() {
                existing.rows.extend(page.rows);
            } else {
                merged = Some(page);
            }
            match next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        let page = merged
            .ok_or_else(|| DesktopError::Protocol(String::from("task report returned no page")))?;
        self.apply_report(client, page).await
    }

    async fn apply_report(
        &mut self,
        client: &mut Client,
        page: TaskReportPage,
    ) -> Result<(), DesktopError> {
        if let Some(shown) = &mut self.displayed {
            if shown.revision != page.revision || shown.purpose != page.purpose {
                return Err(DesktopError::Protocol(String::from(
                    "task report premise changed",
                )));
            }
            shown.progress = page.progress.clone();
        }
        self.purpose_text = load_source(client, &page.purpose_source).await?;
        self.result_text.clear();
        self.result_adopted = None;
        self.action_lines.clear();
        self.report_rows.clear();
        for row in &page.rows {
            if row.kind != "task_result" {
                let body = if let Some(source) = &row.source {
                    load_source(client, source).await?
                } else {
                    String::new()
                };
                let meta = if row.kind == "action_attempt" {
                    let certainty = self.certainty.get(&row.id).map_or("unset", String::as_str);
                    format!("{} certainty={certainty}", row.id)
                } else {
                    String::new()
                };
                self.report_rows.push(super::presentation::Row {
                    body,
                    meta,
                    ..Default::default()
                });
            }
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
                    let certainty = self.certainty.get(&row.id).map_or("unset", String::as_str);
                    self.action_lines
                        .push(format!("action_attempt {} certainty={certainty}", row.id));
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
        self.report_rows.clear();
    }

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

async fn load_source(
    client: &mut Client,
    source: &ReportSourceWireRef,
) -> Result<String, DesktopError> {
    let mut cursor = None;
    let mut text = String::new();
    loop {
        let answer = request_with_timeout(
            client,
            WirePayload::GetReportSource(GetReportSource {
                source: source.clone(),
                cursor,
                limit_bytes: None,
            }),
            REQUEST_TIMEOUT,
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

#[cfg(test)]
mod tests {
    use super::{
        DisplayedTask, PresentedReceipt, TaskPanel, is_interrupted, resume_from_displayed,
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
    fn purpose_identity_yields_the_management_task_target() {
        let id = task_id_from_purpose("01234567-89ab-cdef-0123-456789abcdef:3")
            .expect("purpose identity parses");
        assert_eq!(id.to_string(), "01234567-89ab-cdef-0123-456789abcdef");
    }

    #[test]
    fn clearing_the_selection_body_keeps_the_presentation_receipt() {
        let mut panel = TaskPanel {
            presented: Some(PresentedReceipt {
                receipt: String::from("receipt-1"),
                round: ene_api::v1::refs::RoundWireId(String::from("round-1")),
                generation: 3,
            }),
            undelivered_lines: vec![String::from("undelivered 1")],
            ..TaskPanel::default()
        };
        panel.clear_selection_body();
        assert!(
            panel.has_presented_receipt(),
            "a receipt is a Host presentation fact, not part of the selection body"
        );
        assert_eq!(panel.undelivered_lines.len(), 1);
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
