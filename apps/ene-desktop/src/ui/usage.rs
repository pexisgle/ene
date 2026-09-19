//! Usage / cost / cap management projection.
//!
//! Reads the Stage 6 first-party bounded query over the Client channel.
//! Cap mutation uses the existing revisioned intent; displayed remaining
//! is never the admit authority.

use std::time::Duration;

use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome,
    RationaleOrigin, usage_cap_target,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{BaseViewMark, CommandWireId, UsageCursorWire};
use ene_api::v1::usage::{
    UsageCapConsumptionView, UsageCapView, UsageCostView, UsageMoneyView, UsageSummaryPage,
    UsageSummaryRequest, UsageSummaryResponse, UsageSummaryRowView,
};
use ene_client::Client;

use crate::ui::DesktopError;

const UNKNOWN: &str = "unknown";

/// Token / cost / cap page the usage screen projects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsagePanel {
    request: UsageSummaryRequest,
    page: Option<UsageSummaryPage>,
    notice: String,
    cap_limit_micros: u64,
    cap_scope: String,
    cap_provider: Option<String>,
    cap_window: String,
    cap_currency: String,
}

impl Default for UsagePanel {
    fn default() -> Self {
        Self {
            request: UsageSummaryRequest {
                from: None,
                to: None,
                provider: None,
                model: None,
                consumer: None,
                purpose: None,
                status: None,
                cursor: None,
                limit: None,
            },
            page: None,
            notice: String::new(),
            cap_limit_micros: 1_000_000,
            cap_scope: String::from("system"),
            cap_provider: None,
            cap_window: String::from("daily_utc"),
            cap_currency: String::from("USD"),
        }
    }
}

impl UsagePanel {
    /// Display-only body. Unknown cost is never formatted as yen zero.
    #[must_use]
    pub fn render(&self) -> String {
        let mut lines = Vec::new();
        if !self.notice.is_empty() {
            lines.push(self.notice.clone());
        }
        let Some(page) = &self.page else {
            if lines.is_empty() {
                lines.push(String::from("usage: (empty)"));
            }
            return lines.join("\n");
        };
        lines.push(format!("evaluated-at {}", page.evaluated_at));
        lines.push(String::from(
            "remaining is Host display fact, not admit authority",
        ));
        for row in &page.rows {
            lines.push(render_row(row));
        }
        for cap in &page.caps {
            lines.push(render_cap(cap));
        }
        if let Some(next) = &page.next_cursor {
            lines.push(format!("next {}", next.0));
        }
        lines.join("\n")
    }

    #[must_use]
    pub fn has_unknown_cost(&self) -> bool {
        self.page.as_ref().is_some_and(|page| {
            page.rows.iter().any(|row| row.cost.is_none())
                || page.caps.iter().any(|cap| {
                    matches!(
                        cap.stored.as_ref().map(|stored| &stored.consumption),
                        Some(UsageCapConsumptionView::Indeterminate)
                    )
                })
        })
    }

    pub fn set_status_filter(&mut self, status: Option<String>) {
        self.request.status = status;
        self.request.cursor = None;
    }

    pub fn set_period(&mut self, from: Option<String>, to: Option<String>) {
        self.request.from = from;
        self.request.to = to;
        self.request.cursor = None;
    }

    pub fn set_attribution_filters(
        &mut self,
        provider: Option<String>,
        model: Option<String>,
        consumer: Option<String>,
        purpose: Option<String>,
    ) {
        self.request.provider = provider;
        self.request.model = model;
        self.request.consumer = consumer;
        self.request.purpose = purpose;
        self.request.cursor = None;
    }

    pub fn set_cap_limit_micros(&mut self, micros: u64) {
        self.cap_limit_micros = micros;
    }

    pub fn set_cap_slot(
        &mut self,
        scope: String,
        provider: Option<String>,
        window: String,
        currency: String,
    ) {
        self.cap_scope = scope;
        self.cap_provider = provider;
        self.cap_window = window;
        self.cap_currency = currency;
    }

    /// Drops the cached usage page. Filters stay; they are not target bodies.
    pub fn wipe_body(&mut self) {
        self.page = None;
        self.notice.clear();
    }

    #[must_use]
    pub fn body_cleared(&self) -> bool {
        self.page.is_none() && self.notice.is_empty()
    }

    pub async fn refresh(&mut self, client: &mut Client) -> Result<(), DesktopError> {
        self.request.cursor = None;
        self.load(client).await
    }

    pub async fn next_page(&mut self, client: &mut Client) -> Result<(), DesktopError> {
        let Some(next) = self.page.as_ref().and_then(|page| page.next_cursor.clone()) else {
            return Ok(());
        };
        self.request.cursor = Some(next);
        self.load(client).await
    }

    /// Cap mutation through the Host revision/intent. The panel's remaining
    /// figure is not consulted.
    pub async fn apply_cap(
        &mut self,
        client: &mut Client,
    ) -> Result<ManagementOutcome, DesktopError> {
        let mark = cap_mark_for(
            self.page.as_ref(),
            &self.cap_scope,
            self.cap_provider.as_deref(),
            &self.cap_window,
        )
        .ok_or_else(|| {
            DesktopError::Protocol(String::from("no cap mark from the last usage read"))
        })?;
        let intent = ManagementIntent {
            intent_id: CommandWireId(uuid::Uuid::new_v4()),
            kind: ManagementIntentKind::ManageRuleConsentCap,
            target: usage_cap_target(
                &self.cap_scope,
                self.cap_provider.as_deref(),
                &self.cap_window,
                &self.cap_currency,
                self.cap_limit_micros,
            ),
            base_view: BaseViewMark(mark),
            rationale: IntentRationaleWire {
                origin: RationaleOrigin::ManagementSurface,
                quote: None,
            },
            confirmed: false,
        };
        match ask(client, WirePayload::ManagementIntent(intent)).await? {
            WirePayload::ManagementOutcome(outcome) => {
                self.notice = format!("cap-outcome={outcome:?}");
                Ok(outcome)
            }
            other => Err(DesktopError::Protocol(format!(
                "expected ManagementOutcome, got {}",
                other.message_type()
            ))),
        }
    }

    async fn load(&mut self, client: &mut Client) -> Result<(), DesktopError> {
        match ask(
            client,
            WirePayload::UsageSummaryRequest(self.request.clone()),
        )
        .await?
        {
            WirePayload::UsageSummaryResponse(UsageSummaryResponse::Page(page)) => {
                self.notice.clear();
                self.page = Some(page);
                Ok(())
            }
            WirePayload::UsageSummaryResponse(UsageSummaryResponse::StaleBaseView { current }) => {
                self.page = None;
                self.notice = match current {
                    Some(UsageCursorWire(cursor)) => {
                        format!("usage cursor is stale; restart from the head (current {cursor})")
                    }
                    None => String::from("usage cursor is stale; restart from the head"),
                };
                Ok(())
            }
            WirePayload::UsageSummaryResponse(UsageSummaryResponse::Unavailable) => {
                self.page = None;
                self.notice = String::from("usage is unavailable; retry later");
                Ok(())
            }
            other => Err(DesktopError::Protocol(format!(
                "expected UsageSummaryResponse, got {}",
                other.message_type()
            ))),
        }
    }
}

fn cap_mark_for(
    page: Option<&UsageSummaryPage>,
    scope: &str,
    provider: Option<&str>,
    window: &str,
) -> Option<String> {
    page?
        .caps
        .iter()
        .find(|cap| {
            cap.scope == scope && cap.provider.as_deref() == provider && cap.window == window
        })
        .map(|cap| cap.mark.clone())
}

fn render_row(row: &UsageSummaryRowView) -> String {
    let tokens = row.tokens.as_ref().map_or_else(
        || String::from(UNKNOWN),
        |tokens| {
            format!(
                "{}/{}/{}",
                tokens.input_tokens, tokens.cached_input_tokens, tokens.output_tokens
            )
        },
    );
    format!(
        "{} {}/{} {} {} {} tokens={} cost={} reserved={}",
        row.started_at,
        row.provider,
        row.model,
        row.consumer,
        row.purpose,
        row.status,
        tokens,
        render_cost(row.cost.as_ref()),
        row.reserved
            .as_ref()
            .map_or_else(|| String::from("-"), render_money)
    )
}

fn render_cost(cost: Option<&UsageCostView>) -> String {
    match cost {
        None => String::from(UNKNOWN),
        Some(cost) => format!(
            "{}/{}/{}/{}",
            render_money(&cost.input),
            render_money(&cost.cached_input),
            render_money(&cost.output),
            render_money(&cost.total)
        ),
    }
}

fn render_money(money: &UsageMoneyView) -> String {
    format!("{}:{}", money.currency, money.micros)
}

fn render_cap(cap: &UsageCapView) -> String {
    let scope = cap.provider.as_deref().map_or_else(
        || String::from("system"),
        |provider| format!("provider={provider}"),
    );
    match &cap.stored {
        None => format!("cap {} {} {} no-cap", cap.mark, scope, cap.window),
        Some(stored) => match &stored.consumption {
            UsageCapConsumptionView::Indeterminate => format!(
                "cap {} {} {} limit={} indeterminate",
                cap.mark,
                scope,
                cap.window,
                render_money(&stored.limit)
            ),
            UsageCapConsumptionView::Known {
                reserved,
                committed_reported,
                committed_unknown,
                consumed,
                remaining,
                held,
            } => format!(
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
            ),
        },
    }
}

async fn ask(client: &mut Client, payload: WirePayload) -> Result<WirePayload, DesktopError> {
    tokio::time::timeout(Duration::from_secs(15), client.request(payload))
        .await
        .map_err(|_| DesktopError::Transport(String::from("client request timed out")))?
        .map_err(DesktopError::Client)
}

#[cfg(test)]
mod tests {
    use super::{UNKNOWN, render_cost, render_row};
    use ene_api::v1::usage::{UsageMoneyView, UsageSummaryRowView};

    fn money(micros: u64) -> UsageMoneyView {
        UsageMoneyView {
            currency: String::from("USD"),
            micros,
        }
    }

    #[test]
    fn unknown_cost_is_not_yen_zero() {
        let row = UsageSummaryRowView {
            provider: String::from("openai"),
            model: String::from("gpt-4o-mini"),
            consumer: String::from("companion_dialogue"),
            purpose: String::from("dialogue_response"),
            status: String::from("unknown"),
            tokens: None,
            cost: None,
            reserved: Some(money(100)),
            started_at: String::from("2026-09-19T00:00:00Z"),
        };
        let rendered = render_row(&row);
        assert!(
            rendered.contains(UNKNOWN),
            "unknown must stay visible: {rendered}"
        );
        assert!(
            !rendered.contains("¥0"),
            "unknown must never display as yen zero: {rendered}"
        );
        assert_eq!(render_cost(None), UNKNOWN);
        assert!(!render_cost(None).contains('¥'));
    }
}
