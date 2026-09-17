//! Durable usage cap definitions, cap admission, and reservation accounting.
//!
//! `usage-cost-cap` §6-§10/§13: the cap rows and the reservation lifecycle
//! state live in the same SQLite master as the attempt claim, so the compare,
//! the reservation insert, and the attempt insert share one short `Immediate`
//! transaction. Cap accounting is reconstructed from durable reservation rows
//! and usage facts; no process-local counter is authority.
//!
//! A reservation's cap window is the UTC period containing `opened_at`, fixed
//! at admission: settlement never moves a row between windows, so a call
//! admitted at 23:59 on the last day of a month stays in that month even when
//! it settles after the boundary.

use std::sync::Arc;

use ene_inference::cost::{Money, UsageEstimate};
use ene_inference::pricing::PricingSnapshot;
use ene_inference::{InferenceTicketId, UsageReservation};
use ene_permission::{
    PermissionTechnicalError, SetUsageCapCommand, SetUsageCapOutcome, UsageCap,
    UsageCapConsumption, UsageCapId, UsageCapRef, UsageCapRepository, UsageCapRevision,
    UsageCapScope, UsageCapStatus, UsageCapStatusQuery, UsageCapWindow, UsageReservationRef,
    UsageReservationState,
};
use ene_primitive::{CurrencyCode, RawId, WallClockWithTz};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use crate::Store;
use crate::codec::{
    decode_currency, decode_id, decode_pricing_reference, decode_u64, decode_wall_clock, encode_id,
    encode_u64, encode_wall_clock, lock_shared, permission_unavailable,
};
use crate::run_blocking;

const SQL_SELECT_CAP: &str = "SELECT revision, currency, limit_micros FROM usage_cap WHERE scope = ?1 AND provider = ?2 AND window = ?3";

const SQL_INSERT_CAP: &str = "INSERT INTO usage_cap (scope, provider, window, revision, currency, limit_micros) VALUES (?1, ?2, ?3, ?4, ?5, ?6)";

const SQL_UPDATE_CAP: &str = "UPDATE usage_cap SET revision = ?4, currency = ?5, limit_micros = ?6 WHERE scope = ?1 AND provider = ?2 AND window = ?3";

/// Every current cap that applies to one route: the system scope and the
/// route's provider scope, in both windows.
const SQL_SELECT_APPLICABLE_CAPS: &str = "SELECT scope, provider, window, revision, currency, limit_micros FROM usage_cap WHERE scope = 'system' OR (scope = 'provider' AND provider = ?1)";

/// Every stored cap, deterministically ordered for the status read.
const SQL_SELECT_ALL_CAPS: &str = "SELECT scope, provider, window, revision, currency, limit_micros FROM usage_cap ORDER BY scope, provider, window";

/// The caps a provider-filtered status read reports: the system scope (it
/// budgets every provider) plus that provider's own scopes.
const SQL_SELECT_CAPS_FOR_PROVIDER: &str = "SELECT scope, provider, window, revision, currency, limit_micros FROM usage_cap WHERE scope = 'system' OR (scope = 'provider' AND provider = ?1) ORDER BY scope, provider, window";

/// Non-released reservations opened inside `[?1, ?2)`; the provider-scoped
/// sum additionally filters `provider = ?3`.
const SQL_SELECT_WINDOW_CONSUMPTION: &str = "SELECT state, currency, upper_bound_micros, committed_currency, committed_micros FROM usage_reservation WHERE state != 'released' AND opened_at >= ?1 AND opened_at < ?2";

const SQL_SELECT_WINDOW_CONSUMPTION_PROVIDER: &str = "SELECT state, currency, upper_bound_micros, committed_currency, committed_micros FROM usage_reservation WHERE state != 'released' AND opened_at >= ?1 AND opened_at < ?2 AND provider = ?3";

pub(crate) const SQL_SELECT_RESERVATION_BY_TICKET: &str = "SELECT ticket, reservation_id, provider, model, pricing_snapshot, currency, upper_bound_micros, state, committed_currency, committed_micros, opened_at FROM usage_reservation WHERE ticket = ?1";

/// Every non-terminal reservation, for Host-startup reconciliation.
pub(crate) const SQL_SELECT_ORPHANED_RESERVATIONS: &str = "SELECT ticket, reservation_id, provider, model, pricing_snapshot, currency, upper_bound_micros, state, committed_currency, committed_micros, opened_at FROM usage_reservation WHERE state = 'reserved'";

const SQL_INSERT_RESERVATION: &str = "INSERT INTO usage_reservation (reservation_id, ticket, provider, model, pricing_snapshot, currency, upper_bound_micros, state, opened_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'reserved', ?8)";

pub(crate) const SQL_SETTLE_RESERVATION: &str = "UPDATE usage_reservation SET state = ?2, committed_currency = ?3, committed_micros = ?4 WHERE ticket = ?1 AND state = 'reserved'";

/// One current cap row, decoded.
pub(crate) struct CapRow {
    id: UsageCapId,
    revision: UsageCapRevision,
    limit: Money,
}

impl CapRow {
    pub(crate) fn reference(&self) -> UsageCapRef {
        UsageCapRef::new(self.id.clone(), self.revision)
    }

    pub(crate) fn scope(&self) -> &UsageCapScope {
        self.id.scope()
    }

    pub(crate) fn window(&self) -> UsageCapWindow {
        self.id.window()
    }

    pub(crate) fn limit(&self) -> Money {
        self.limit
    }

    pub(crate) fn revision(&self) -> UsageCapRevision {
        self.revision
    }
}

fn decode_cap(
    scope: &str,
    provider: &str,
    window: &str,
    revision: i64,
    currency: &str,
    limit_micros: i64,
) -> Result<CapRow, String> {
    let scope = UsageCapScope::from_stored(scope, provider)
        .ok_or_else(|| String::from("unknown usage cap scope"))?;
    let window = UsageCapWindow::from_name(window)
        .ok_or_else(|| String::from("unknown usage cap window"))?;
    Ok(CapRow {
        id: UsageCapId::new(scope, window),
        revision: UsageCapRevision::from_u64(decode_u64(revision)?),
        limit: Money::from_micros(decode_currency(currency)?, decode_u64(limit_micros)?),
    })
}

/// Reads the one cap row for `(scope, window)`, if any.
fn select_cap(
    conn: &rusqlite::Connection,
    scope: &UsageCapScope,
    window: UsageCapWindow,
) -> Result<Option<CapRow>, String> {
    let provider = scope.provider().unwrap_or("");
    let found: Option<(i64, String, i64)> = conn
        .query_row(
            SQL_SELECT_CAP,
            params![scope.as_str(), provider, window.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    found
        .map(|(revision, currency, limit)| {
            decode_cap(
                scope.as_str(),
                provider,
                window.as_str(),
                revision,
                &currency,
                limit,
            )
        })
        .transpose()
}

/// Reads every current cap applying to the route. The rows are ordered
/// deterministically (system before provider, daily before monthly) so a
/// multi-cap violation always answers with the same cap reference.
fn select_applicable_caps(tx: &Transaction<'_>, provider: &str) -> Result<Vec<CapRow>, String> {
    let mut statement = tx
        .prepare(SQL_SELECT_APPLICABLE_CAPS)
        .map_err(|error| error.to_string())?;
    let rows: Vec<(String, String, String, i64, String, i64)> = statement
        .query_map(params![provider], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let mut caps = rows
        .iter()
        .map(|(scope, provider, window, revision, currency, limit)| {
            decode_cap(scope, provider, window, *revision, currency, *limit)
        })
        .collect::<Result<Vec<_>, _>>()?;
    caps.sort_by_key(cap_order);
    Ok(caps)
}

/// System before provider, daily before monthly.
fn cap_order(cap: &CapRow) -> (u8, u8) {
    let scope = match cap.scope() {
        UsageCapScope::System => 0,
        UsageCapScope::Provider(_) => 1,
    };
    let window = match cap.window() {
        UsageCapWindow::DailyUtc => 0,
        UsageCapWindow::MonthlyUtc => 1,
    };
    (scope, window)
}

/// The current-period consumption of one cap scope.
pub(crate) enum CapWindowConsumption {
    /// The exact sum of every non-released reservation's cap amount in the
    /// window: the reserved upper bound, or the actual committed cost when
    /// the provider reported usage.
    Known(Money),
    /// Safety cannot be confirmed from the durable rows (an unrepresentable
    /// period, a currency the cap cannot be compared in, or an overflowing
    /// sum). The caller must not start a cap-enabled send.
    Indeterminate,
}

/// One non-released reservation row's accounting columns.
struct ConsumptionRow {
    state: String,
    currency: String,
    upper_bound_micros: i64,
    committed_currency: Option<String>,
    committed_micros: Option<i64>,
}

fn consumption_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConsumptionRow> {
    Ok(ConsumptionRow {
        state: row.get(0)?,
        currency: row.get(1)?,
        upper_bound_micros: row.get(2)?,
        committed_currency: row.get(3)?,
        committed_micros: row.get(4)?,
    })
}

/// One cap window's consumption split by the reservation state that counts
/// it, in exact micro-currency units of one currency.
pub(crate) struct WindowConsumption {
    /// Upper bounds of still-non-terminal reservations.
    pub(crate) reserved: u64,
    /// Actual committed cost of reported settlements.
    pub(crate) committed_reported: u64,
    /// Upper bounds kept counted for unknown settlements.
    pub(crate) committed_unknown: u64,
}

/// Sums one scope's consumption breakdown over the UTC period containing
/// `at`.
///
/// Only rows whose state is not `released` count. `committed_reported`
/// contributes the actual committed cost (the reserved upper bound is
/// released); `reserved` and `committed_unknown` contribute the reserved
/// upper bound, so an unknown external consumption can never free a cap slot.
/// `Ok(None)` is indeterminate: an unrepresentable period, a currency the cap
/// cannot be compared in, or an unreadable state. A malformed stored amount
/// stays a technical error, never a guessed number.
pub(crate) fn consumption_breakdown(
    conn: &rusqlite::Connection,
    scope: &UsageCapScope,
    window: UsageCapWindow,
    currency: CurrencyCode,
    at: WallClockWithTz,
) -> Result<Option<WindowConsumption>, String> {
    let Some(period) = window.period_containing(at) else {
        return Ok(None);
    };
    let start = period.start().to_rfc3339_utc();
    let end = period.end().to_rfc3339_utc();
    let mut statement = conn
        .prepare(match scope {
            UsageCapScope::System => SQL_SELECT_WINDOW_CONSUMPTION,
            UsageCapScope::Provider(_) => SQL_SELECT_WINDOW_CONSUMPTION_PROVIDER,
        })
        .map_err(|error| error.to_string())?;
    let provider = scope.provider().unwrap_or("");
    let rows: Vec<ConsumptionRow> = match scope {
        UsageCapScope::System => statement
            .query_map(params![start, end], consumption_row)
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>(),
        UsageCapScope::Provider(_) => statement
            .query_map(params![start, end, provider], consumption_row)
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>(),
    }
    .map_err(|error| error.to_string())?;
    // Accumulate in u128: an overflowing u64 sum is an indeterminate cap
    // state, never a wrapped or saturated amount.
    let mut reserved: u128 = 0;
    let mut committed_reported: u128 = 0;
    let mut committed_unknown: u128 = 0;
    for row in rows {
        let state = UsageReservationState::from_name(&row.state)
            .ok_or_else(|| String::from("unknown usage reservation state"))?;
        let (amount_currency, amount, bucket) = match state {
            UsageReservationState::CommittedReported => (
                row.committed_currency
                    .ok_or_else(|| String::from("committed reservation missing currency"))?,
                row.committed_micros
                    .ok_or_else(|| String::from("committed reservation missing amount"))?,
                &mut committed_reported,
            ),
            UsageReservationState::Reserved => {
                (row.currency, row.upper_bound_micros, &mut reserved)
            }
            UsageReservationState::CommittedUnknown => {
                (row.currency, row.upper_bound_micros, &mut committed_unknown)
            }
            UsageReservationState::Released => continue,
        };
        if decode_currency(&amount_currency)? != currency {
            return Ok(None);
        }
        *bucket += u128::from(decode_u64(amount)?);
    }
    let (Ok(reserved), Ok(committed_reported), Ok(committed_unknown)) = (
        u64::try_from(reserved),
        u64::try_from(committed_reported),
        u64::try_from(committed_unknown),
    ) else {
        return Ok(None);
    };
    Ok(Some(WindowConsumption {
        reserved,
        committed_reported,
        committed_unknown,
    }))
}

/// Sums one scope's consumption over the UTC period containing `at`.
pub(crate) fn consumed_in_window(
    conn: &rusqlite::Connection,
    scope: &UsageCapScope,
    window: UsageCapWindow,
    currency: CurrencyCode,
    at: WallClockWithTz,
) -> Result<CapWindowConsumption, String> {
    let Some(breakdown) = consumption_breakdown(conn, scope, window, currency, at)? else {
        return Ok(CapWindowConsumption::Indeterminate);
    };
    let total = u128::from(breakdown.reserved)
        + u128::from(breakdown.committed_reported)
        + u128::from(breakdown.committed_unknown);
    let Ok(micros) = u64::try_from(total) else {
        return Ok(CapWindowConsumption::Indeterminate);
    };
    Ok(CapWindowConsumption::Known(Money::from_micros(
        currency, micros,
    )))
}

/// The cap-admission decision for one attempt claim.
pub(crate) enum ReservationAdmission {
    /// No cap applies to the route: the attempt claims without a reservation
    /// because there is no limit to protect.
    NoCap,
    /// The reservation is durable in this transaction; the caller may run
    /// provider I/O once the transaction commits.
    Reserved,
    /// A current cap would be exceeded: no attempt, no reservation, and no
    /// provider byte.
    Held(UsageCapRef),
    /// A cap applies but a finite safe upper bound cannot be established:
    /// no attempt, no reservation, and no provider byte.
    Indeterminate,
}

/// The premises the claim resolved before admission: the route, the immutable
/// pricing snapshot it published, and the provider adapter's safe bound.
pub(crate) struct ReservationPremise<'a> {
    pub(crate) provider: &'a str,
    pub(crate) model: &'a str,
    pub(crate) pricing: Option<&'a PricingSnapshot>,
    pub(crate) pricing_reference: Option<&'a str>,
    pub(crate) estimate: Option<&'a UsageEstimate>,
}

/// Reserves the conservative upper bound of one claim against every
/// applicable cap, inside the claim transaction.
///
/// Reads the current cap revisions, sums each cap's window consumption, and
/// inserts the reservation only when the new upper bound keeps every
/// applicable cap satisfied. Any missing premise (no reviewed rate, no
/// provider estimate) or unrepresentable amount answers
/// [`ReservationAdmission::Indeterminate`], never a guessed or zero bound.
pub(crate) fn admit_reservation(
    tx: &Transaction<'_>,
    ticket: &InferenceTicketId,
    premise: ReservationPremise<'_>,
    now: WallClockWithTz,
) -> Result<ReservationAdmission, String> {
    let caps = select_applicable_caps(tx, premise.provider)?;
    if caps.is_empty() {
        return Ok(ReservationAdmission::NoCap);
    }
    // A cap-enabled send needs both the immutable rate and a finite safe
    // bound: without either, the cap cannot be proven satisfied.
    let (Some(snapshot), Some(reference), Some(estimate)) =
        (premise.pricing, premise.pricing_reference, premise.estimate)
    else {
        return Ok(ReservationAdmission::Indeterminate);
    };
    let Some(upper_bound) = estimate.upper_bound_cost(snapshot) else {
        return Ok(ReservationAdmission::Indeterminate);
    };
    for cap in &caps {
        if cap.limit().currency() != upper_bound.currency() {
            // Comparing currencies by a guessed rate is forbidden; refusing
            // the send is the only safe answer.
            return Ok(ReservationAdmission::Indeterminate);
        }
        let consumed =
            consumed_in_window(tx, cap.scope(), cap.window(), upper_bound.currency(), now)?;
        let CapWindowConsumption::Known(consumed) = consumed else {
            return Ok(ReservationAdmission::Indeterminate);
        };
        let Some(total) = consumed.checked_add(upper_bound) else {
            return Ok(ReservationAdmission::Indeterminate);
        };
        if total.micros() > cap.limit().micros() {
            return Ok(ReservationAdmission::Held(cap.reference()));
        }
    }
    let reservation = UsageReservationRef(RawId::new());
    tx.execute(
        SQL_INSERT_RESERVATION,
        params![
            encode_id(reservation.0),
            encode_id(ticket.0),
            premise.provider,
            premise.model,
            reference,
            upper_bound.currency().as_str(),
            encode_u64(upper_bound.micros())?,
            encode_wall_clock(now),
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(ReservationAdmission::Reserved)
}

/// One stored reservation row, exactly as `usage_reservation` holds it.
pub(crate) struct ReservationRow {
    pub(crate) ticket: String,
    pub(crate) reservation_id: String,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) pricing_snapshot: String,
    pub(crate) currency: String,
    pub(crate) upper_bound_micros: i64,
    pub(crate) state: String,
    pub(crate) committed_currency: Option<String>,
    pub(crate) committed_micros: Option<i64>,
    pub(crate) opened_at: String,
}

pub(crate) fn reservation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReservationRow> {
    Ok(ReservationRow {
        ticket: row.get(0)?,
        reservation_id: row.get(1)?,
        provider: row.get(2)?,
        model: row.get(3)?,
        pricing_snapshot: row.get(4)?,
        currency: row.get(5)?,
        upper_bound_micros: row.get(6)?,
        state: row.get(7)?,
        committed_currency: row.get(8)?,
        committed_micros: row.get(9)?,
        opened_at: row.get(10)?,
    })
}

/// Decodes one stored reservation into the composed inference record.
///
/// The state and money columns must agree with the lifecycle: `reported`
/// carries an actual amount, every other state carries none, and the pricing
/// reference parses. Anything else is unreadable, never a guessed state.
pub(crate) fn decode_reservation(raw: &ReservationRow) -> Result<UsageReservation, String> {
    let state = UsageReservationState::from_name(&raw.state)
        .ok_or_else(|| String::from("unknown usage reservation state"))?;
    let upper_bound = Money::from_micros(
        decode_currency(&raw.currency)?,
        decode_u64(raw.upper_bound_micros)?,
    );
    let committed = match (state, &raw.committed_currency, raw.committed_micros) {
        (UsageReservationState::CommittedReported, Some(currency), Some(micros)) => Some(
            Money::from_micros(decode_currency(currency)?, decode_u64(micros)?),
        ),
        (UsageReservationState::CommittedReported, _, _) => {
            return Err(String::from("committed reservation is missing its amount"));
        }
        (_, None, None) => None,
        (_, _, _) => {
            return Err(String::from("uncommitted reservation carries an amount"));
        }
    };
    Ok(UsageReservation {
        reference: UsageReservationRef(decode_id(&raw.reservation_id)?),
        ticket: InferenceTicketId(decode_id(&raw.ticket)?),
        provider: raw.provider.clone(),
        model: raw.model.clone(),
        pricing: decode_pricing_reference(&raw.pricing_snapshot)?,
        upper_bound,
        state,
        committed,
        opened_at: decode_wall_clock(&raw.opened_at)?,
    })
}

impl UsageCapRepository for Store {
    async fn set_usage_cap(
        &self,
        command: SetUsageCapCommand,
    ) -> Result<SetUsageCapOutcome, PermissionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            if command.limit.micros() == 0 {
                return Ok(SetUsageCapOutcome::InvalidLimit);
            }
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| permission_unavailable(error.to_string()))?;
            // The compare and the write share the admission path's
            // serialization domain: an admission that committed first is
            // observed here (stale), and an update that commits first is what
            // the next admission reads.
            let current =
                select_cap(&tx, &command.scope, command.window).map_err(permission_unavailable)?;
            let matches = match (&current, &command.expected) {
                (None, None) => true,
                (Some(cap), Some(expected)) => &cap.reference() == expected,
                _ => false,
            };
            if !matches {
                return Ok(SetUsageCapOutcome::Stale {
                    current: current.as_ref().map(CapRow::reference),
                });
            }
            let revision = match &current {
                None => UsageCapRevision::first(),
                Some(cap) => cap.revision().checked_next().ok_or_else(|| {
                    permission_unavailable(String::from("usage cap revision exhausted"))
                })?,
            };
            let cap = UsageCap::new(
                UsageCapId::new(command.scope.clone(), command.window),
                revision,
                command.limit,
            );
            let scope_text = command.scope.as_str();
            let provider_text = command.scope.provider().unwrap_or("");
            let window_text = command.window.as_str();
            let revision_raw = encode_u64(revision.as_u64()).map_err(permission_unavailable)?;
            match &current {
                None => {
                    tx.execute(
                        SQL_INSERT_CAP,
                        params![
                            scope_text,
                            provider_text,
                            window_text,
                            revision_raw,
                            cap.limit().currency().as_str(),
                            encode_u64(cap.limit().micros()).map_err(permission_unavailable)?,
                        ],
                    )
                    .map_err(|error| permission_unavailable(error.to_string()))?;
                }
                Some(_) => {
                    tx.execute(
                        SQL_UPDATE_CAP,
                        params![
                            scope_text,
                            provider_text,
                            window_text,
                            revision_raw,
                            cap.limit().currency().as_str(),
                            encode_u64(cap.limit().micros()).map_err(permission_unavailable)?,
                        ],
                    )
                    .map_err(|error| permission_unavailable(error.to_string()))?;
                }
            }
            tx.commit()
                .map_err(|error| permission_unavailable(error.to_string()))?;
            Ok(SetUsageCapOutcome::StoredAs(cap.reference()))
        })
        .await
    }

    async fn load_usage_cap_status(
        &self,
        query: UsageCapStatusQuery,
    ) -> Result<Vec<UsageCapStatus>, PermissionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            // One read transaction is not needed: each cap's consumption is
            // computed from the durable rows under the shared connection
            // lock, and the read writes nothing (SELECT-only).
            let mut statement = guard
                .prepare(match query.provider {
                    None => SQL_SELECT_ALL_CAPS,
                    Some(_) => SQL_SELECT_CAPS_FOR_PROVIDER,
                })
                .map_err(|error| permission_unavailable(error.to_string()))?;
            let rows: Vec<(String, String, String, i64, String, i64)> = match &query.provider {
                None => statement
                    .query_map((), |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    })
                    .map_err(|error| permission_unavailable(error.to_string()))?
                    .collect::<Result<Vec<_>, _>>(),
                Some(provider) => statement
                    .query_map(params![provider], |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    })
                    .map_err(|error| permission_unavailable(error.to_string()))?
                    .collect::<Result<Vec<_>, _>>(),
            }
            .map_err(|error| permission_unavailable(error.to_string()))?;
            let mut statuses = Vec::with_capacity(rows.len());
            for (scope, provider, window, revision, currency, limit_micros) in &rows {
                let cap = decode_cap(scope, provider, window, *revision, currency, *limit_micros)
                    .map_err(permission_unavailable)?;
                let consumption =
                    cap_consumption(&guard, &cap, query.at).map_err(permission_unavailable)?;
                statuses.push(UsageCapStatus {
                    cap: UsageCap::new(cap.id.clone(), cap.revision(), cap.limit()),
                    consumption,
                });
            }
            Ok(statuses)
        })
        .await
    }
}

/// Builds one cap's window consumption from the same durable reservation rows
/// the send admission compares.
///
/// Indeterminate stays distinct from a zero total: a read must never display
/// "nothing consumed" for a row it cannot compare.
fn cap_consumption(
    conn: &rusqlite::Connection,
    cap: &CapRow,
    at: WallClockWithTz,
) -> Result<UsageCapConsumption, String> {
    let currency = cap.limit().currency();
    let Some(breakdown) = consumption_breakdown(conn, cap.scope(), cap.window(), currency, at)?
    else {
        return Ok(UsageCapConsumption::Indeterminate);
    };
    let total = u128::from(breakdown.reserved)
        + u128::from(breakdown.committed_reported)
        + u128::from(breakdown.committed_unknown);
    let Some(total) = u64::try_from(total).ok() else {
        return Ok(UsageCapConsumption::Indeterminate);
    };
    let consumed = Money::from_micros(currency, total);
    let held = consumed.micros() >= cap.limit().micros();
    let remaining = if held {
        Money::zero(currency)
    } else {
        Money::from_micros(currency, cap.limit().micros() - consumed.micros())
    };
    Ok(UsageCapConsumption::Known {
        reserved: Money::from_micros(currency, breakdown.reserved),
        committed_reported: Money::from_micros(currency, breakdown.committed_reported),
        committed_unknown: Money::from_micros(currency, breakdown.committed_unknown),
        consumed,
        remaining,
        held,
    })
}
