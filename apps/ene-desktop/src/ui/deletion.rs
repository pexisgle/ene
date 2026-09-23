//! Targeted Deletion management projection.
//!
//! Distinct from conversational forget. Client intent only stages a request;
//! Host-local seated control confirms. Status is the bounded
//! `DeletionStatusRequest`. Exact target text is Owner body and stays out of
//! snapshots and Debug.

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

/// Targeted Deletion page. Exact text is never part of the projection.
pub struct DeletionPanel {
    exact_text: String,
    purpose: DeletionPurposeWire,
    mark: String,
    page: Option<DeletionStatusPage>,
    notice: String,
    pending_request_id: Option<String>,
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
            .field("pending_request_id", &self.pending_request_id)
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
            pending_request_id: None,
            pending: Vec::new(),
            selected: String::new(),
        }
    }
}

impl DeletionPanel {
    pub(crate) fn rows(&self, locale: crate::i18n::Locale) -> Vec<super::presentation::Row> {
        use super::presentation::{Row, state, tr};
        let mut rows: Vec<Row> = self
            .pending
            .iter()
            .enumerate()
            .map(|(index, p)| Row {
                key: format!("request:{}", p.request_id),
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
                key: format!("operation:{}:{}", op.operation.0, op.sweep),
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
        if !self
            .rows(crate::i18n::Locale::En)
            .iter()
            .any(|r| r.key == key)
        {
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
                self.selected == format!("operation:{}:{}", op.operation.0, op.sweep)
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

    /// Status body: phases, holds, participants. No target text.
    #[must_use]
    pub fn render(&self) -> String {
        let mut lines = Vec::new();
        if !self.notice.is_empty() {
            lines.push(self.notice.clone());
        }
        lines.push(String::from(
            "targeted-deletion is distinct from conversational forget",
        ));
        if let Some(pending) = &self.pending_request_id {
            lines.push(format!("pending-request {pending}"));
        }
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

    /// Advisory Client request. Destructive confirmation is Host-local.
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
        match ask(client, WirePayload::ManagementIntent(intent)).await? {
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

    /// Client `confirmed=true` is DeniedByBoundary and does not complete.
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
        match ask(client, WirePayload::ManagementIntent(intent)).await? {
            WirePayload::ManagementOutcome(outcome) => Ok(outcome),
            other => Err(DesktopError::Protocol(format!(
                "expected ManagementOutcome, got {}",
                other.message_type()
            ))),
        }
    }

    pub async fn refresh(&mut self, client: &mut Client) -> Result<(), DesktopError> {
        match ask(
            client,
            WirePayload::DeletionStatusRequest(DeletionStatusRequest {
                cursor: None,
                limit: None,
            }),
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
                Err(DesktopError::Protocol(String::from(
                    "deletion status unavailable",
                )))
            }
            other => Err(DesktopError::Protocol(format!(
                "expected DeletionStatusResponse, got {}",
                other.message_type()
            ))),
        }
    }

    /// Lists staged request identities on the seated control channel, then
    /// mints a confirmation session. Exact text does not travel this path.
    pub async fn begin_confirm(
        &mut self,
        seat: &mut ConfirmationClient,
    ) -> Result<(), DesktopError> {
        let pending = seat.list_pending_deletions().await?;
        let preview = pending
            .iter()
            .find(|p| self.selected == format!("request:{}", p.request_id))
            .or_else(|| {
                if self.selected.is_empty() && pending.len() == 1 {
                    pending.first()
                } else {
                    None
                }
            });
        let Some(preview) = preview else {
            return Err(DesktopError::Protocol(String::from(
                "no staged deletion request",
            )));
        };
        self.pending_request_id = Some(preview.request_id.clone());
        seat.request_deletion_confirm(&preview.request_id).await
    }

    /// Resolves the Held operation the Owner selected, as the identity the
    /// resume request names. Borrowing nothing keeps the request future free
    /// to run while this panel keeps serving its own erasure demand.
    ///
    /// # Errors
    ///
    /// [`DesktopError::Protocol`] when no Held operation is selected.
    pub(crate) fn resume_target(&self) -> Result<(String, u64), DesktopError> {
        let operation = self.page.as_ref().and_then(|page| {
            page.operations
                .iter()
                .find(|op| self.selected == format!("operation:{}:{}", op.operation.0, op.sweep))
                .or_else(|| {
                    if self.selected.is_empty() && page.operations.len() == 1 {
                        page.operations.first()
                    } else {
                        None
                    }
                })
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

    /// Records the resume outcome as this panel's notice.
    pub(crate) fn note_resume(&mut self, reply: &FromConfirmation) {
        self.notice = match reply {
            FromConfirmation::Outcome(ControlOutcome::Deletion(DeletionOutcome::Resumed {
                operation,
                sweep,
            })) => format!("resumed {operation} sweep {sweep}"),
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

async fn ask(client: &mut Client, payload: WirePayload) -> Result<WirePayload, DesktopError> {
    tokio::time::timeout(Duration::from_secs(15), client.request(payload))
        .await
        .map_err(|_| DesktopError::Transport(String::from("client request timed out")))?
        .map_err(DesktopError::Client)
}

#[cfg(test)]
mod tests {
    use super::DeletionPanel;

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
}
