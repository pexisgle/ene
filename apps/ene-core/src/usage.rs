//! First-party usage / cost / cap surface (`usage-cost-cap` §16/§17).
//!
//! Two halves, kept apart on purpose:
//!
//! - The bounded usage summary read is SELECT-only composition over the
//!   inference-owned query ([`ene_inference::UsageSummaryRepository`]) and
//!   the permission-owned cap status ([`ene_permission::UsageCapRepository`]).
//!   It settles no reservation, refreshes no pricing, mutates no cap, and
//!   reconciles no usage; a read observes the durable state and changes
//!   nothing. The Host clamps the limit and the period before calling the
//!   owner, which clamps again at the SQL boundary.
//! - Cap set/update is a management intent (`ManageRuleConsentCap` with the
//!   shared `cap:` target grammar). The Host parses the intent's opaque
//!   `base_view` mark into the expected cap revision and reaches the
//!   permission-owned [`ene_permission::SetUsageCapCommand`]; the command's
//!   compare-and-set is the only mutation authority. A stale mark answers
//!   `StaleBaseView` with the current mark and stores nothing; a malformed
//!   mark is face-stale, never "expects no cap". LLM output and provider
//!   responses never construct an intent, and only the authenticated
//!   first-party connection reaches this handler.
//!
//! No body text, prompt, output, or credential value crosses either half:
//! the read rows carry attribution and accounting only, and the cap target
//! carries the intended limit.

use ene_api::v1::management::{
    ManagementIntent, ManagementOutcome, USAGE_CAP_TARGET_PREFIX, parse_usage_cap_target,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::UsageCursorWire;
use ene_api::v1::usage::{
    USAGE_PAGE_LIMIT_DEFAULT, USAGE_PAGE_LIMIT_MAX, UsageCapConsumptionView, UsageCapStoredView,
    UsageCapView, UsageCostView, UsageMoneyView, UsageSummaryPage, UsageSummaryRequest,
    UsageSummaryResponse, UsageSummaryRowView, UsageTokenUsageView,
};
use ene_inference::cost::UsageCostFact;
use ene_inference::{
    USAGE_SUMMARY_RANGE_DEFAULT_DAYS, UsageSummaryCursor, UsageSummaryQuery,
    UsageSummaryRepository as _, UsageSummaryRow, UsageSummaryStatus,
};
use ene_permission::{
    ConsumerKind, IntentOutcome, PurposeKind, SetUsageCapCommand, SetUsageCapOutcome,
    UsageCapConsumption, UsageCapId, UsageCapRef, UsageCapRepository as _, UsageCapRevision,
    UsageCapScope, UsageCapStatus, UsageCapStatusQuery, UsageCapWindow, parse_usage_cap_mark,
    usage_cap_mark,
};
use ene_plugin_ipc::WireFrame;
use ene_primitive::{CurrencyCode, Money, WallClockWithTz};

use crate::presentation::{StoredCursor, field_reject, stale_operation};
use crate::serve::{HostHandle, LiveInput, connection_key, outgoing_frame};
use crate::setup::outcome_frame;

/// Replay-fingerprint discriminator for one cap update; distinct from every
/// consent/setup kind so a reused intent id is different content by
/// construction.
pub(crate) const INTENT_KIND_USAGE_CAP: &str = "usage-cap";

/// One stored usage cursor's page state: the effective period bounds the
/// walk keeps and the keyset position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UsageCursorPage {
    pub(crate) from: WallClockWithTz,
    pub(crate) to: WallClockWithTz,
    pub(crate) after: Option<UsageSummaryCursor>,
}

/// The exact caller-declared filter premise one usage cursor is bound to.
/// Reusing a cursor under any other filter set answers `StaleBaseView` instead
/// of reading a different result set with an old position.
///
/// `from`/`to` are the declared bounds (canonical UTC text) or `None` when the
/// caller left the period to the Host: a cursor minted with the default period
/// keeps the same effective bounds on every page, so the Host stores those
/// bounds with the cursor rather than re-deriving them from a later clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UsageQueryPremise {
    pub(crate) from: Option<String>,
    pub(crate) to: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) consumer: Option<ConsumerKind>,
    pub(crate) purpose: Option<PurposeKind>,
    pub(crate) status: Option<UsageSummaryStatus>,
}

impl HostHandle {
    /// Dispatch entry: one bounded usage summary read.
    ///
    /// Malformed filters answer `UnsupportedFieldValue` (nothing is read); a
    /// foreign or premise-mismatched cursor answers `StaleBaseView`; a store
    /// failure answers `Unavailable` rather than an empty page.
    pub(crate) async fn usage_summary_wire(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        request: &UsageSummaryRequest,
    ) -> Vec<WireFrame> {
        let Some(limit) = checked_usage_limit(request.limit) else {
            return vec![field_reject(frame, live, "usage limit must be 1..=50")];
        };
        let Some(consumer) = parse_consumer(request.consumer.as_deref()) else {
            return vec![field_reject(frame, live, "unknown usage consumer filter")];
        };
        let Some(purpose) = parse_purpose(request.purpose.as_deref()) else {
            return vec![field_reject(frame, live, "unknown usage purpose filter")];
        };
        let Some(status) = parse_status(request.status.as_deref()) else {
            return vec![field_reject(frame, live, "unknown usage status filter")];
        };
        let declared_to = match request.to.as_deref() {
            None => None,
            Some(text) => match WallClockWithTz::parse_rfc3339(text) {
                Ok(at) => Some(at),
                Err(_) => {
                    return vec![field_reject(
                        frame,
                        live,
                        "usage period bound is not RFC 3339",
                    )];
                }
            },
        };
        let declared_from = match request.from.as_deref() {
            None => None,
            Some(text) => match WallClockWithTz::parse_rfc3339(text) {
                Ok(at) => Some(at),
                Err(_) => {
                    return vec![field_reject(
                        frame,
                        live,
                        "usage period bound is not RFC 3339",
                    )];
                }
            },
        };
        let premise = UsageQueryPremise {
            from: declared_from.map(|at| at.to_rfc3339_utc()),
            to: declared_to.map(|at| at.to_rfc3339_utc()),
            provider: request.provider.clone(),
            model: request.model.clone(),
            consumer,
            purpose,
            status,
        };
        let now = WallClockWithTz::now();
        let conn = connection_key(&live.connection_id);
        // The first page derives (and clamps) the effective bounds; a cursor
        // page reuses the bounds its cursor was minted with, so every page of
        // one walk covers the same window even though the wall clock moved.
        let (from, to, after) = match request.cursor.as_ref() {
            None => {
                let to = declared_to.unwrap_or(now);
                // A bound in the future is clamped to now: the read never
                // claims to have observed a later instant than it did.
                let to = if to.as_datetime() > now.as_datetime() {
                    now
                } else {
                    to
                };
                let from = declared_from.unwrap_or_else(|| default_from(to));
                (from, to, None)
            }
            Some(cursor) => {
                let state = crate::lock_unpoison(&self.presentations);
                match state.usage_cursor_page(&conn, &cursor.0, &premise) {
                    Some(page) => (page.from, page.to, page.after),
                    None => return vec![stale_usage_cursor(frame, live)],
                }
            }
        };
        let query = UsageSummaryQuery {
            from,
            to,
            provider: request.provider.clone(),
            model: request.model.clone(),
            consumer,
            purpose,
            status,
            after,
            limit,
        };
        // The owner query clamps again and applies LIMIT in SQL: the bound is
        // on the rows read, never a full materialization with a late cutoff.
        let rows = match self.store.query_usage_summary(query).await {
            Ok(rows) => rows,
            Err(_) => {
                return vec![outgoing_frame(
                    frame,
                    live,
                    WirePayload::UsageSummaryResponse(UsageSummaryResponse::Unavailable),
                )];
            }
        };
        // The cap section is the current state at the same instant the query
        // was bounded by; it is a read like the rows above.
        let caps = match self
            .store
            .load_usage_cap_status(UsageCapStatusQuery {
                provider: request.provider.clone(),
                at: now,
            })
            .await
        {
            Ok(statuses) => statuses,
            Err(_) => {
                return vec![outgoing_frame(
                    frame,
                    live,
                    WirePayload::UsageSummaryResponse(UsageSummaryResponse::Unavailable),
                )];
            }
        };
        let cap_views = build_cap_views(request.provider.as_deref(), &caps);
        let row_views: Vec<UsageSummaryRowView> = rows.iter().map(usage_row_view).collect();
        // Test-only race gate: pause after the durable read and before the
        // guarded mint (S5-05-style ownership check).
        #[cfg(test)]
        if let Some(gate) = self.ref_mint_gate() {
            gate.pause().await;
        }
        // The cursor mint runs under the ownership section: a connection
        // superseded while the page was read creates no cursor.
        let minted = self.with_presentation_state(live, |state| {
            Self::take_cursor(
                state,
                &conn,
                request.cursor.as_ref().map(|cursor| cursor.0.as_str()),
            );
            if rows.len() as u32 == limit
                && let Some(last) = rows.last()
            {
                Some(UsageCursorWire(
                    Self::mint_cursor(
                        state,
                        &conn,
                        StoredCursor::UsageSummary {
                            premise: premise.clone(),
                            from,
                            to,
                            after: Some(UsageSummaryCursor {
                                started_at: last.started_at,
                                ticket: last.ticket,
                            }),
                        },
                    )
                    .0,
                ))
            } else {
                None
            }
        });
        let Some(next_cursor) = minted else {
            return vec![stale_operation(
                frame,
                live,
                "usage summary on a superseded connection",
            )];
        };
        vec![outgoing_frame(
            frame,
            live,
            WirePayload::UsageSummaryResponse(UsageSummaryResponse::Page(UsageSummaryPage {
                rows: row_views,
                next_cursor,
                caps: cap_views,
                evaluated_at: now.to_rfc3339_utc(),
            })),
        )]
    }

    /// Maps one `ManageRuleConsentCap` intent whose target carries the shared
    /// `cap:` grammar onto the permission-owned command (`usage-cost-cap`
    /// §13/§17).
    ///
    /// Currentness is re-checked here, not trusted from the Client: the
    /// `base_view` mark names exactly `(scope, window)` at one revision (or
    /// the none state), and the command serializes the compare with the send
    /// admission. A face-stale mark, an unknown scope/window/currency, or an
    /// unrepresentable limit clarifies with zero writes; a store failure
    /// holds (nothing decided, a retry is safe).
    pub(crate) async fn set_usage_cap_intent(
        &self,
        frame: &WireFrame,
        intent: &ManagementIntent,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        if let Some(answer) = self
            .replay_or_hold(
                frame,
                live,
                intent,
                Self::intent_fingerprint(intent, INTENT_KIND_USAGE_CAP),
            )
            .await
        {
            return answer;
        }
        let Some(target) = parse_usage_cap_target(&intent.target) else {
            return vec![self.cap_clarify(frame, intent, live).await];
        };
        let scope = match (target.scope.as_str(), target.provider) {
            ("system", None) => UsageCapScope::System,
            ("provider", Some(provider)) => UsageCapScope::Provider(provider),
            _ => return vec![self.cap_clarify(frame, intent, live).await],
        };
        let Some(window) = UsageCapWindow::from_name(&target.window) else {
            return vec![self.cap_clarify(frame, intent, live).await];
        };
        let Some(currency) = CurrencyCode::from_code(&target.currency) else {
            return vec![self.cap_clarify(frame, intent, live).await];
        };
        let expected = match parse_usage_cap_mark(&intent.base_view.0, &scope, window) {
            Some(None) => None,
            Some(Some(revision)) => Some(UsageCapRef::new(
                UsageCapId::new(scope.clone(), window),
                UsageCapRevision::from_u64(revision),
            )),
            // A mark that names another cap or another shape is stale on its
            // face: it is never read as "expects no cap". Nothing is written;
            // the answer carries the rebuilt current mark.
            None => {
                return vec![self.cap_stale(frame, intent, live, &scope, window).await];
            }
        };
        let limit = Money::from_micros(currency, target.limit_micros);
        match self
            .store
            .set_usage_cap(SetUsageCapCommand {
                expected,
                scope: scope.clone(),
                window,
                limit,
            })
            .await
        {
            Ok(SetUsageCapOutcome::StoredAs(reference)) => {
                vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    self.record_decided(
                        Self::intent_fingerprint(intent, INTENT_KIND_USAGE_CAP),
                        IntentOutcome::StoredAsRuleView {
                            revision: cap_mark_of(&reference),
                        },
                    )
                    .await,
                )]
            }
            Ok(SetUsageCapOutcome::Stale { current }) => {
                vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    self.record_decided(
                        Self::intent_fingerprint(intent, INTENT_KIND_USAGE_CAP),
                        IntentOutcome::StaleBaseView {
                            current: current
                                .as_ref()
                                .map_or_else(|| usage_cap_mark(&scope, window, None), cap_mark_of),
                        },
                    )
                    .await,
                )]
            }
            Ok(SetUsageCapOutcome::InvalidLimit) => {
                vec![outcome_frame(
                    frame,
                    live,
                    intent,
                    self.record_decided(
                        Self::intent_fingerprint(intent, INTENT_KIND_USAGE_CAP),
                        IntentOutcome::NeedsClarification,
                    )
                    .await,
                )]
            }
            // Nothing was decided, so a retry is safe.
            Err(_) => vec![outcome_frame(
                frame,
                live,
                intent,
                ManagementOutcome::HeldByOperation,
            )],
        }
    }

    /// Records one clarify answer for a cap intent that cannot reach the
    /// owner command.
    async fn cap_clarify(
        &self,
        frame: &WireFrame,
        intent: &ManagementIntent,
        live: &LiveInput,
    ) -> WireFrame {
        outcome_frame(
            frame,
            live,
            intent,
            self.record_decided(
                Self::intent_fingerprint(intent, INTENT_KIND_USAGE_CAP),
                IntentOutcome::NeedsClarification,
            )
            .await,
        )
    }

    /// Answers a face-stale base view with the rebuilt current mark, reading
    /// the current cap without mutating anything.
    async fn cap_stale(
        &self,
        frame: &WireFrame,
        intent: &ManagementIntent,
        live: &LiveInput,
        scope: &UsageCapScope,
        window: UsageCapWindow,
    ) -> WireFrame {
        let statuses = match self
            .store
            .load_usage_cap_status(UsageCapStatusQuery {
                provider: scope.provider().map(str::to_owned),
                at: WallClockWithTz::now(),
            })
            .await
        {
            Ok(statuses) => statuses,
            // The cap read could not answer: nothing is decided, and a retry
            // is safe.
            Err(_) => {
                return outcome_frame(frame, live, intent, ManagementOutcome::HeldByOperation);
            }
        };
        // A successful read answers with the rebuilt current mark; an absent
        // row answers the none-state mark, which is a real current state, not
        // a failure.
        let current = statuses
            .iter()
            .find(|status| status.cap.scope() == scope && status.cap.window() == window)
            .map_or_else(
                || usage_cap_mark(scope, window, None),
                |status| cap_mark_of(&status.cap.reference()),
            );
        outcome_frame(
            frame,
            live,
            intent,
            self.record_decided(
                Self::intent_fingerprint(intent, INTENT_KIND_USAGE_CAP),
                IntentOutcome::StaleBaseView { current },
            )
            .await,
        )
    }
}

fn stale_usage_cursor(frame: &WireFrame, live: &LiveInput) -> WireFrame {
    outgoing_frame(
        frame,
        live,
        WirePayload::UsageSummaryResponse(UsageSummaryResponse::StaleBaseView { current: None }),
    )
}

/// The clamped page bound: `1..=USAGE_PAGE_LIMIT_MAX`, default
/// [`USAGE_PAGE_LIMIT_DEFAULT`].
fn checked_usage_limit(limit: Option<u32>) -> Option<u32> {
    match limit {
        None => Some(USAGE_PAGE_LIMIT_DEFAULT),
        Some(value) if (1..=USAGE_PAGE_LIMIT_MAX).contains(&value) => Some(value),
        Some(_) => None,
    }
}

/// The default lower bound: [`USAGE_SUMMARY_RANGE_DEFAULT_DAYS`] before `to`.
/// A calendar extreme without a representable floor keeps `to` itself, so the
/// range stays empty rather than inventing an unbounded one.
fn default_from(to: WallClockWithTz) -> WallClockWithTz {
    to.as_datetime()
        .checked_sub_days(chrono::Days::new(USAGE_SUMMARY_RANGE_DEFAULT_DAYS))
        .map_or(to, WallClockWithTz::from_datetime)
}

fn parse_consumer(name: Option<&str>) -> Option<Option<ConsumerKind>> {
    match name {
        None => Some(None),
        Some(name) => ConsumerKind::from_name(name).map(Some),
    }
}

fn parse_purpose(name: Option<&str>) -> Option<Option<PurposeKind>> {
    match name {
        None => Some(None),
        Some(name) => PurposeKind::from_name(name).map(Some),
    }
}

fn parse_status(name: Option<&str>) -> Option<Option<UsageSummaryStatus>> {
    match name {
        None => Some(None),
        Some(name) => UsageSummaryStatus::from_name(name).map(Some),
    }
}

/// The cap slots the response reports: the system scope always (it budgets
/// every provider), plus the provider the query names (its slots appear even
/// without a stored cap, so the Client receives the none-state mark it must
/// echo to create the first cap). Stored caps of other providers are included
/// with their slots.
fn build_cap_views(
    provider_filter: Option<&str>,
    statuses: &[UsageCapStatus],
) -> Vec<UsageCapView> {
    let mut providers: Vec<String> = statuses
        .iter()
        .filter_map(|status| status.cap.scope().provider().map(str::to_owned))
        .collect();
    if let Some(provider) = provider_filter
        && !providers.iter().any(|known| known == provider)
    {
        providers.push(provider.to_owned());
    }
    providers.sort();
    providers.dedup();
    let mut views = Vec::with_capacity(2 + providers.len() * 2);
    for window in [UsageCapWindow::DailyUtc, UsageCapWindow::MonthlyUtc] {
        views.push(cap_view(
            &UsageCapScope::System,
            window,
            statuses.iter().find(|status| {
                status.cap.scope() == &UsageCapScope::System && status.cap.window() == window
            }),
        ));
    }
    for provider in providers {
        let scope = UsageCapScope::Provider(provider);
        for window in [UsageCapWindow::DailyUtc, UsageCapWindow::MonthlyUtc] {
            views.push(cap_view(
                &scope,
                window,
                statuses
                    .iter()
                    .find(|status| status.cap.scope() == &scope && status.cap.window() == window),
            ));
        }
    }
    views
}

fn cap_view(
    scope: &UsageCapScope,
    window: UsageCapWindow,
    status: Option<&UsageCapStatus>,
) -> UsageCapView {
    let provider = scope.provider().map(str::to_owned);
    let (mark, stored) = match status {
        Some(status) => (
            cap_mark_of(&status.cap.reference()),
            Some(UsageCapStoredView {
                limit: money_view(status.cap.limit()),
                consumption: cap_consumption_view(&status.consumption),
            }),
        ),
        None => (usage_cap_mark(scope, window, None), None),
    };
    UsageCapView {
        mark,
        scope: scope.as_str().to_string(),
        provider,
        window: window.as_str().to_string(),
        stored,
    }
}

/// The owner rendering of one cap reference; the single mark grammar owner is
/// [`ene_permission::usage_cap_mark`].
fn cap_mark_of(reference: &UsageCapRef) -> String {
    usage_cap_mark(
        reference.scope(),
        reference.window(),
        Some(reference.revision()),
    )
}

fn cap_consumption_view(consumption: &UsageCapConsumption) -> UsageCapConsumptionView {
    match consumption {
        UsageCapConsumption::Known {
            reserved,
            committed_reported,
            committed_unknown,
            consumed,
            remaining,
            held,
        } => UsageCapConsumptionView::Known {
            reserved: money_view(*reserved),
            committed_reported: money_view(*committed_reported),
            committed_unknown: money_view(*committed_unknown),
            consumed: money_view(*consumed),
            remaining: money_view(*remaining),
            held: *held,
        },
        UsageCapConsumption::Indeterminate => UsageCapConsumptionView::Indeterminate,
    }
}

fn usage_row_view(row: &UsageSummaryRow) -> UsageSummaryRowView {
    UsageSummaryRowView {
        provider: row.provider.clone(),
        model: row.model.clone(),
        consumer: row.consumer.as_str().to_string(),
        purpose: row.purpose.as_str().to_string(),
        status: row.status.as_str().to_string(),
        tokens: row.tokens.map(|tokens| UsageTokenUsageView {
            input_tokens: tokens.input_tokens,
            cached_input_tokens: tokens.cached_input_tokens,
            output_tokens: tokens.output_tokens,
        }),
        cost: row.cost.as_ref().and_then(cost_view),
        reserved: row.reserved.map(money_view),
        started_at: row.started_at.to_rfc3339_utc(),
    }
}

fn cost_view(cost: &UsageCostFact) -> Option<UsageCostView> {
    match cost {
        UsageCostFact::Reported(cost) => Some(UsageCostView {
            input: money_view(cost.input),
            cached_input: money_view(cost.cached_input),
            output: money_view(cost.output),
            total: money_view(cost.total),
        }),
        UsageCostFact::Unknown { .. } => None,
    }
}

fn money_view(money: Money) -> UsageMoneyView {
    UsageMoneyView {
        currency: money.currency().as_str().to_string(),
        micros: money.micros(),
    }
}

/// Whether one `ManageRuleConsentCap` target belongs to the usage-cap grammar
/// rather than the consent/setup grammar; the Host branch keys on this and
/// then parses it authoritatively.
pub(crate) fn is_usage_cap_target(target: &str) -> bool {
    target.starts_with(USAGE_CAP_TARGET_PREFIX)
}
