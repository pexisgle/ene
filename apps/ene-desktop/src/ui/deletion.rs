use std::time::Duration;

use ene_api::v1::deletion::{
    DeletionHoldWire, DeletionOperationStatusView, DeletionParticipantReportWire,
    DeletionPhaseWire, DeletionPurposeWire, DeletionStatusPage, DeletionStatusRequest,
    DeletionStatusResponse, deletion_target,
};
use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome, RationaleOrigin,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{BaseViewMark, CommandWireId};
use ene_client::Client;
use ene_local_control::{ControlOutcome, DeletionOutcome, FromConfirmation};
use zeroize::Zeroize as _;

use crate::control::ConfirmationClient;
use crate::ui::DesktopError;
use crate::ui::request_with_timeout;

pub struct DeletionPanel {
    exact_text: String,
    purpose: DeletionPurposeWire,
    mark: String,
    page: Option<DeletionStatusPage>,
    notice: String,
    pending: Vec<ene_local_control::PendingDeletionPreview>,
    selected: String,
}

impl core::fmt::Debug for DeletionPanel {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("DeletionPanel")
            .field("exact_text", &"[redacted]")
            .field("purpose", &self.purpose)
            .field("mark", &self.mark)
            .field("page", &self.page)
            .field("notice", &self.notice)
            .finish()
    }
}

impl Default for DeletionPanel {
    fn default() -> Self {
        Self {
            exact_text: String::new(),
            purpose: DeletionPurposeWire::Privacy,
            mark: String::new(),
            page: None,
            notice: String::new(),
            pending: Vec::new(),
            selected: String::new(),
        }
    }
}

fn request_key(id: &str) -> String {
    format!("request:{id}")
}

fn operation_key(operation: &str, sweep: u64) -> String {
    format!("operation:{operation}:{sweep}")
}

impl DeletionPanel {
    pub(crate) fn rows(&self, locale: crate::i18n::Locale) -> Vec<super::presentation::Row> {
        use super::presentation::{Row, state, tr};
        let mut rows: Vec<Row> = self
            .pending
            .iter()
            .enumerate()
            .map(|(index, p)| Row {
                key: request_key(&p.request_id),
                title: format!(
                    "{} {}",
                    tr(locale, "削除要求", "Deletion request"),
                    index + 1
                ),
                body: tr(
                    locale,
                    "本人確認後に削除を開始します",
                    "Deletion starts after owner confirmation",
                ),
                state: tr(locale, "確認待ち", "Awaiting confirmation"),
                ..Row::default()
            })
            .collect();
        if let Some(page) = &self.page {
            rows.extend(page.operations.iter().map(|op| Row {
                key: operation_key(&op.operation.0, op.sweep),
                title: tr(locale, "データ削除", "Data deletion"),
                state: state(locale, op.phase.as_str()),
                body: match &op.participants {
                    DeletionParticipantReportWire::NotReported => {
                        tr(locale, "進捗の報告を待っています", "Waiting for progress")
                    }
                    DeletionParticipantReportWire::Reported(entries) => format!(
                        "{} {}",
                        entries.len(),
                        tr(locale, "件の処理先から報告", "participants reported")
                    ),
                },
                meta: op.started_at.clone(),
            }));
        }
        rows
    }
    pub(crate) fn selected_key(&self) -> String {
        self.selected.clone()
    }
    pub(crate) fn select_key(&mut self, key: &str) -> Result<(), DesktopError> {
        let known = self
            .pending
            .iter()
            .any(|p| request_key(&p.request_id) == key)
            || self.page.as_ref().is_some_and(|page| {
                page.operations
                    .iter()
                    .any(|op| operation_key(&op.operation.0, op.sweep) == key)
            });
        if !known {
            return Err(DesktopError::Protocol(String::from(
                "stale deletion selection",
            )));
        }
        self.selected = key.into();
        Ok(())
    }
    pub(crate) fn can_resume(&self) -> bool {
        self.page.as_ref().is_some_and(|p| {
            p.operations.iter().any(|op| {
                self.selected == operation_key(&op.operation.0, op.sweep)
                    && op.phase == DeletionPhaseWire::Held
            })
        })
    }
    pub(crate) async fn refresh_pending(
        &mut self,
        seat: &ConfirmationClient,
    ) -> Result<(), DesktopError> {
        self.pending = seat.list_pending_deletions().await?;
        Ok(())
    }

    pub fn set_exact_text(&mut self, text: String) {
        self.exact_text.zeroize();
        self.exact_text = text;
    }

    pub fn set_purpose(&mut self, purpose: DeletionPurposeWire) {
        self.purpose = purpose;
    }

    pub fn wipe_exact_text(&mut self) {
        self.exact_text.zeroize();
        self.exact_text.clear();
    }

    #[must_use]
    pub fn exact_text_cleared(&self) -> bool {
        self.exact_text.is_empty()
    }

    #[must_use]
    pub fn render(&self) -> String {
        let mut lines = Vec::new();
        if !self.notice.is_empty() {
            lines.push(self.notice.clone());
        }
        lines.push(String::from(
            "targeted-deletion is distinct from conversational forget",
        ));
        let Some(page) = &self.page else {
            if self.mark.is_empty() {
                lines.push(String::from("deletion: (empty)"));
            } else {
                lines.push(format!("mark {}", self.mark));
            }
            return lines.join("\n");
        };
        lines.push(format!("mark {}", page.mark.0));
        for operation in &page.operations {
            lines.push(render_operation(operation));
        }
        if let Some(next) = &page.next_cursor {
            lines.push(format!("next {}", next.0));
        }
        lines.join("\n")
    }

    #[must_use]
    pub fn phase_of_first(&self) -> Option<DeletionPhaseWire> {
        self.page
            .as_ref()
            .and_then(|page| page.operations.first())
            .map(|operation| operation.phase)
    }

    #[must_use]
    pub fn has_operations(&self) -> bool {
        self.page
            .as_ref()
            .is_some_and(|page| !page.operations.is_empty())
    }

    pub async fn request(
        &mut self,
        client: &mut Client,
    ) -> Result<ManagementOutcome, DesktopError> {
        if self.exact_text.is_empty() {
            return Err(DesktopError::Protocol(String::from(
                "deletion target is empty",
            )));
        }
        self.refresh(client).await?;
        let intent = ManagementIntent {
            intent_id: CommandWireId(uuid::Uuid::new_v4()),
            kind: ManagementIntentKind::RequestDeletionBackupRestoreReset,
            target: deletion_target(self.purpose, &self.exact_text),
            base_view: BaseViewMark(self.mark.clone()),
            rationale: IntentRationaleWire {
                origin: RationaleOrigin::ManagementSurface,
                quote: None,
            },
            confirmed: false,
        };
        self.wipe_exact_text();
        match request_with_timeout(
            client,
            WirePayload::ManagementIntent(intent),
            Duration::from_secs(15),
        )
        .await?
        {
            WirePayload::ManagementOutcome(outcome) => {
                self.notice = format!("deletion-outcome={outcome:?}");
                Ok(outcome)
            }
            other => Err(DesktopError::Protocol(format!(
                "expected ManagementOutcome, got {}",
                other.message_type()
            ))),
        }
    }

    pub async fn request_confirmed_true(
        &mut self,
        client: &mut Client,
    ) -> Result<ManagementOutcome, DesktopError> {
        self.refresh(client).await?;
        let intent = ManagementIntent {
            intent_id: CommandWireId(uuid::Uuid::new_v4()),
            kind: ManagementIntentKind::RequestDeletionBackupRestoreReset,
            target: deletion_target(self.purpose, "must-not-apply"),
            base_view: BaseViewMark(self.mark.clone()),
            rationale: IntentRationaleWire {
                origin: RationaleOrigin::ManagementSurface,
                quote: None,
            },
            confirmed: true,
        };
        match request_with_timeout(
            client,
            WirePayload::ManagementIntent(intent),
            Duration::from_secs(15),
        )
        .await?
        {
            WirePayload::ManagementOutcome(outcome) => Ok(outcome),
            other => Err(DesktopError::Protocol(format!(
                "expected ManagementOutcome, got {}",
                other.message_type()
            ))),
        }
    }

    pub async fn refresh(&mut self, client: &mut Client) -> Result<(), DesktopError> {
        match request_with_timeout(
            client,
            WirePayload::DeletionStatusRequest(DeletionStatusRequest {
                cursor: None,
                limit: None,
            }),
            Duration::from_secs(15),
        )
        .await?
        {
            WirePayload::DeletionStatusResponse(DeletionStatusResponse::Page(page)) => {
                self.mark = page.mark.0.clone();
                self.page = Some(page);
                Ok(())
            }
            WirePayload::DeletionStatusResponse(DeletionStatusResponse::Unavailable) => {
                self.page = None;
                self.notice = String::from("deletion status is unavailable; retry later");
                Err(DesktopError::Unavailable(String::from(
                    "deletion status unavailable",
                )))
            }
            other => Err(DesktopError::Protocol(format!(
                "expected DeletionStatusResponse, got {}",
                other.message_type()
            ))),
        }
    }

    pub async fn begin_confirm(
        &mut self,
        seat: &mut ConfirmationClient,
    ) -> Result<(), DesktopError> {
        let pending = seat.list_pending_deletions().await?;
        let preview = pending
            .iter()
            .find(|p| self.selected == request_key(&p.request_id));
        let Some(preview) = preview else {
            return Err(DesktopError::Protocol(String::from(
                "no staged deletion request",
            )));
        };
        seat.request_deletion_confirm(&preview.request_id).await
    }

    pub(crate) fn resume_target(&self) -> Result<(String, u64), DesktopError> {
        let operation = self.page.as_ref().and_then(|page| {
            page.operations
                .iter()
                .find(|op| self.selected == operation_key(&op.operation.0, op.sweep))
        });
        let Some(operation) = operation else {
            return Err(DesktopError::Protocol(String::from(
                "no deletion operation to resume",
            )));
        };
        if operation.phase != DeletionPhaseWire::Held {
            return Err(DesktopError::Protocol(String::from(
                "resume applies to Held operations",
            )));
        }
        Ok((operation.operation.0.clone(), operation.sweep))
    }

    pub(crate) fn note_resume(&mut self, reply: &FromConfirmation) {
        self.notice =
            match reply {
                FromConfirmation::Outcome(ControlOutcome::Deletion(DeletionOutcome::Resumed {
                    operation,
                    sweep,
                })) => format!("resumed {operation} sweep {sweep}"),
                FromConfirmation::Outcome(ControlOutcome::Deletion(
                    DeletionOutcome::StaleSweep { operation, sweep },
                )) => format!("stale-sweep {operation} sweep {sweep}"),
                FromConfirmation::Outcome(ControlOutcome::Deletion(
                    DeletionOutcome::Completed { operation, sweep },
                )) => format!("completed {operation} sweep {sweep}"),
                FromConfirmation::Outcome(ControlOutcome::Deletion(
                    DeletionOutcome::Finalizing { operation, sweep },
                )) => format!("finalizing {operation} sweep {sweep}"),
                FromConfirmation::Outcome(ControlOutcome::Deletion(
                    DeletionOutcome::HeldByOperation { operation, sweep },
                )) => format!("held {operation} sweep {sweep}"),
                FromConfirmation::Outcome(ControlOutcome::Deletion(DeletionOutcome::Missing)) => {
                    String::from("missing")
                }
                other => format!("resume={other:?}"),
            };
    }
}

fn render_operation(operation: &DeletionOperationStatusView) -> String {
    let hold = operation.hold.map_or_else(
        || String::from("-"),
        |hold| match hold {
            DeletionHoldWire::Unavailable => String::from("unavailable"),
            DeletionHoldWire::GenerationExhausted => String::from("generation-exhausted"),
        },
    );
    let participants = match &operation.participants {
        DeletionParticipantReportWire::NotReported => String::from("not-reported"),
        DeletionParticipantReportWire::Reported(entries) => entries
            .iter()
            .map(|entry| format!("{}:{}", entry.owner, entry.progress))
            .collect::<Vec<_>>()
            .join(","),
    };
    format!(
        "{} {} {} sweep={} started={} hold={} participants={}",
        operation.operation.0,
        operation.phase.as_str(),
        operation.purpose.as_str(),
        operation.sweep,
        operation.started_at,
        hold,
        participants
    )
}

#[cfg(test)]
mod tests {
    use super::{DeletionPanel, render_operation};
    use ene_api::v1::deletion::{
        DeletionOperationStatusView, DeletionParticipantReportWire, DeletionPhaseWire,
        DeletionPurposeWire,
    };
    use ene_api::v1::refs::DeletionOperationWireRef;

    #[test]
    fn debug_redacts_exact_text() {
        let mut panel = DeletionPanel::default();
        panel.set_exact_text(String::from("raw-secret-keyword"));
        let rendered = format!("{panel:?}");
        assert!(
            !rendered.contains("raw-secret-keyword"),
            "exact text must not Debug: {rendered}"
        );
        assert!(rendered.contains("[redacted]"));
        let body = panel.render();
        assert!(
            !body.contains("raw-secret-keyword"),
            "projection must omit the body: {body}"
        );
    }

    #[test]
    fn phases_are_distinct_in_the_projection() {
        for phase in [
            DeletionPhaseWire::Held,
            DeletionPhaseWire::Finalizing,
            DeletionPhaseWire::Completed,
        ] {
            let line = render_operation(&DeletionOperationStatusView {
                operation: DeletionOperationWireRef(String::from("op-1")),
                phase,
                purpose: DeletionPurposeWire::Privacy,
                started_at: String::from("2026-09-19T00:00:00Z"),
                sweep: 1,
                hold: None,
                participants: DeletionParticipantReportWire::NotReported,
            });
            assert!(
                line.contains(phase.as_str()),
                "phase token must appear: {line}"
            );
        }
        assert_ne!(DeletionPhaseWire::Held, DeletionPhaseWire::Completed);
        assert_ne!(DeletionPhaseWire::Finalizing, DeletionPhaseWire::Completed);
    }
}
