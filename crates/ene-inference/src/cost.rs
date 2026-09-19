//! Integer money arithmetic and reproducible cost projection.
//!
//! `usage-cost-cap` §4/§5: a cost fact is derived from the reported token
//! counts and the immutable pricing snapshot the provider call ran under.
//! Floating point is never the canonical value: [`Money`] is an exact count of
//! micro-currency units and [`TokenRate`] is an exact count of micro-currency
//! units per 1,000,000 tokens.
//!
//! Each component rounds *up* to the next micro-currency unit, so a fractional
//! micro-unit is never silently dropped (which would understate the billed
//! amount) and the rule is deterministic and recomputable. The total is the
//! exact sum of the three rounded components. An amount that does not fit the
//! money representation is [`CostProjectionError::AmountOverflow`]: it is
//! never wrapped, saturated, or replaced by zero.
//!
//! The projection never applies another model's rate: a snapshot that does
//! not match the usage attribution is a hard error. Missing token counts or a
//! missing reviewed rate produce [`UsageCostFact::Unknown`], never a zero
//! amount.

use thiserror::Error;

use crate::UsageFact;
use crate::pricing::{PricingSnapshot, PricingSnapshotRef};

/// Rates are stated per this many tokens, so the stored ratio is exact
/// integer arithmetic with no intermediate decimal expansion.
const RATE_DENOMINATOR: u128 = 1_000_000;

/// Currency of a money amount.
///
/// The reviewed first-party catalog is USD-only. Adding another currency is an
/// explicit catalog change; amounts of different currencies are never
/// combined or converted by a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CurrencyCode {
    Usd,
}

impl CurrencyCode {
    /// The code this currency stores and renders as.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Usd => "USD",
        }
    }

    /// Parses a stored code. An unknown code is `None`, never a default
    /// currency.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "USD" => Some(Self::Usd),
            _ => None,
        }
    }
}

/// Exact money amount in micro-currency units (1e-6 of the currency unit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Money {
    currency: CurrencyCode,
    micros: u64,
}

impl Money {
    /// Builds an amount from an exact micro-currency count.
    #[must_use]
    pub const fn from_micros(currency: CurrencyCode, micros: u64) -> Self {
        Self { currency, micros }
    }

    /// The zero amount of `currency`.
    #[must_use]
    pub const fn zero(currency: CurrencyCode) -> Self {
        Self::from_micros(currency, 0)
    }

    /// The currency this amount is denominated in.
    #[must_use]
    pub const fn currency(self) -> CurrencyCode {
        self.currency
    }

    /// The exact micro-currency count.
    #[must_use]
    pub const fn micros(self) -> u64 {
        self.micros
    }

    /// Adds two amounts of the same currency. Different currencies and
    /// amounts that do not fit [`u64`] micro-units answer `None`, never a
    /// converted, wrapped, or saturated value.
    #[must_use]
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        if !matches!(
            (self.currency, other.currency),
            (CurrencyCode::Usd, CurrencyCode::Usd)
        ) {
            return None;
        }
        match self.micros.checked_add(other.micros) {
            Some(micros) => Some(Self {
                currency: self.currency,
                micros,
            }),
            None => None,
        }
    }
}

/// Exact price of one token class: micro-currency units per 1,000,000 tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenRate {
    micros_per_million_tokens: u64,
}

impl TokenRate {
    /// Builds a rate from an exact micro-currency count per 1,000,000 tokens.
    #[must_use]
    pub const fn from_micros_per_million(micros_per_million_tokens: u64) -> Self {
        Self {
            micros_per_million_tokens,
        }
    }

    /// The exact micro-currency count per 1,000,000 tokens.
    #[must_use]
    pub const fn micros_per_million(self) -> u64 {
        self.micros_per_million_tokens
    }

    /// Charges `tokens` at this rate, rounding up to the next micro-currency
    /// unit. `None` when the amount does not fit [`Money`]: the caller must
    /// fail closed instead of truncating or saturating.
    #[must_use]
    pub fn checked_cost(self, currency: CurrencyCode, tokens: u64) -> Option<Money> {
        // The product of two u64 values fits u128 exactly; checked_mul keeps
        // the arithmetic total even if the widths change later.
        let scaled = u128::from(tokens).checked_mul(u128::from(self.micros_per_million_tokens))?;
        let rounded = scaled
            .checked_add(RATE_DENOMINATOR - 1)?
            .checked_div(RATE_DENOMINATOR)?;
        let micros = u64::try_from(rounded).ok()?;
        Some(Money::from_micros(currency, micros))
    }
}

/// Cost components of one reported token usage, in the currency of the
/// snapshot that priced it.
///
/// `input` charges only the non-cached input tokens; the cached subset is
/// charged once at `cached_input`, never again at the normal input rate.
/// `total` is the exact sum of the three components.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportedCost {
    /// `(input_tokens - cached_input_tokens) * input_rate`.
    pub input: Money,
    /// `cached_input_tokens * cached_input_rate`.
    pub cached_input: Money,
    /// `output_tokens * output_rate`.
    pub output: Money,
    /// Exact sum of the three components.
    pub total: Money,
    /// Immutable pricing snapshot the components were derived from.
    pub pricing: PricingSnapshotRef,
}

/// The durable cost fact of one settled ticket.
///
/// `Unknown` means no amount is known: either the token usage itself is
/// unknown, or no reviewed rate covered the route at admission. It carries
/// the pricing snapshot reference when one existed so the missing amount is
/// explainable, but it never carries a zero amount and is never rendered or
/// summed as zero. The absent reference means no reviewed rate existed at
/// admission; a fabricated reference would claim a rate basis that never did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageCostFact {
    Reported(ReportedCost),
    Unknown { pricing: Option<PricingSnapshotRef> },
}

/// Why a cost fact cannot be projected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CostProjectionError {
    /// The token counts disagree with their source: a Reported fact is
    /// missing counts or reports cached input above input, or an Unknown fact
    /// carries counts. Zero-filling such a fact would fabricate usage.
    #[error("usage counts are not internally consistent")]
    InconsistentUsage,
    /// The pricing snapshot names a different provider/model than the usage
    /// fact. Applying it would charge another model's rate.
    #[error("pricing snapshot does not match the usage attribution")]
    AttributionMismatch,
    /// A component or the total does not fit the money representation.
    /// Fail closed rather than wrap or saturate the amount.
    #[error("usage cost does not fit the money representation")]
    AmountOverflow,
}

/// Derives the cost fact for one token usage fact.
///
/// `pricing` is the snapshot bound to the ticket at admission, or `None` when
/// the reviewed catalog had no rate for the route. The projection applies the
/// `usage-cost-cap` §5 formula:
///
/// ```text
/// non_cached_input = input_tokens - cached_input_tokens
/// input_cost        = non_cached_input * input_rate
/// cached_input_cost = cached_input_tokens * cached_input_rate
/// output_cost       = output_tokens * output_rate
/// ```
///
/// A Reported usage with no snapshot settles [`UsageCostFact::Unknown`], never
/// a zero: the provider may have spent money that no reviewed rate can price.
///
/// # Errors
///
/// Returns [`CostProjectionError::InconsistentUsage`] for a malformed usage
/// fact, [`CostProjectionError::AttributionMismatch`] when the pricing
/// snapshot belongs to another route, and
/// [`CostProjectionError::AmountOverflow`] when a component or the total is
/// not representable.
pub fn project_cost(
    usage: &UsageFact,
    pricing: Option<&PricingSnapshot>,
) -> Result<UsageCostFact, CostProjectionError> {
    let counts = match usage.source {
        crate::UsageSource::Reported => {
            let input_tokens = usage
                .input_tokens
                .ok_or(CostProjectionError::InconsistentUsage)?;
            let cached_tokens = usage
                .cached_input_tokens
                .ok_or(CostProjectionError::InconsistentUsage)?;
            let output_tokens = usage
                .output_tokens
                .ok_or(CostProjectionError::InconsistentUsage)?;
            let non_cached_tokens = input_tokens
                .checked_sub(cached_tokens)
                .ok_or(CostProjectionError::InconsistentUsage)?;
            Some((non_cached_tokens, cached_tokens, output_tokens))
        }
        crate::UsageSource::Unknown => {
            if usage.input_tokens.is_some()
                || usage.cached_input_tokens.is_some()
                || usage.output_tokens.is_some()
            {
                return Err(CostProjectionError::InconsistentUsage);
            }
            None
        }
    };
    let Some(snapshot) = pricing else {
        return Ok(UsageCostFact::Unknown { pricing: None });
    };
    if snapshot.provider != usage.provider || snapshot.model != usage.model {
        return Err(CostProjectionError::AttributionMismatch);
    }
    let pricing = snapshot.reference();
    let Some((non_cached_tokens, cached_tokens, output_tokens)) = counts else {
        return Ok(UsageCostFact::Unknown {
            pricing: Some(pricing),
        });
    };
    let input = snapshot
        .input_rate
        .checked_cost(snapshot.currency, non_cached_tokens)
        .ok_or(CostProjectionError::AmountOverflow)?;
    let cached_input = snapshot
        .cached_input_rate
        .checked_cost(snapshot.currency, cached_tokens)
        .ok_or(CostProjectionError::AmountOverflow)?;
    let output = snapshot
        .output_rate
        .checked_cost(snapshot.currency, output_tokens)
        .ok_or(CostProjectionError::AmountOverflow)?;
    let total = input
        .checked_add(cached_input)
        .and_then(|sum| sum.checked_add(output))
        .ok_or(CostProjectionError::AmountOverflow)?;
    Ok(UsageCostFact::Reported(ReportedCost {
        input,
        cached_input,
        output,
        total,
        pricing,
    }))
}

#[cfg(test)]
mod tests {
    use super::{CostProjectionError, CurrencyCode, Money, TokenRate, UsageCostFact, project_cost};
    use crate::UsageSource;
    use crate::pricing::tests_support::snapshot;
    use crate::{InferenceTicketId, UsageFact};
    use ene_primitive::RawId;

    fn rate(micros_per_million: u64) -> TokenRate {
        TokenRate::from_micros_per_million(micros_per_million)
    }

    fn usage(provider: &str, model: &str, counts: Option<(u64, u64, u64)>) -> UsageFact {
        UsageFact {
            ticket: InferenceTicketId(RawId::new()),
            provider: provider.to_owned(),
            model: model.to_owned(),
            input_tokens: counts.map(|(input, _, _)| input),
            cached_input_tokens: counts.map(|(_, cached, _)| cached),
            output_tokens: counts.map(|(_, _, output)| output),
            source: if counts.is_some() {
                UsageSource::Reported
            } else {
                UsageSource::Unknown
            },
        }
    }

    #[test]
    fn components_and_total_do_not_double_count_cached_input() {
        let pricing = snapshot(
            "openai",
            "gpt-test",
            1,
            TokenRate::from_micros_per_million(2_000_000),
            TokenRate::from_micros_per_million(500_000),
            TokenRate::from_micros_per_million(8_000_000),
        );
        let fact = usage("openai", "gpt-test", Some((1_000, 200, 500)));
        let cost = project_cost(&fact, Some(&pricing)).expect("the projection must succeed");
        let UsageCostFact::Reported(cost) = cost else {
            panic!("a Reported usage with a rate must project Reported");
        };
        // non_cached = 800 * 2.0 = 1600 micros; cached = 200 * 0.5 = 100;
        // output = 500 * 8.0 = 4000.
        assert_eq!(cost.input, Money::from_micros(CurrencyCode::Usd, 1_600));
        assert_eq!(
            cost.cached_input,
            Money::from_micros(CurrencyCode::Usd, 100)
        );
        assert_eq!(cost.output, Money::from_micros(CurrencyCode::Usd, 4_000));
        assert_eq!(cost.total, Money::from_micros(CurrencyCode::Usd, 5_700));
        assert_eq!(
            cost.total,
            cost.input
                .checked_add(cost.cached_input)
                .and_then(|sum| sum.checked_add(cost.output))
                .expect("the components fit")
        );
        assert_eq!(cost.pricing, pricing.reference());
    }

    #[test]
    fn fractional_micro_units_round_up_exactly_once_per_component() {
        let pricing = snapshot(
            "openai",
            "gpt-test",
            1,
            TokenRate::from_micros_per_million(2_500_000),
            TokenRate::from_micros_per_million(2_500_000),
            TokenRate::from_micros_per_million(2_500_000),
        );
        for (tokens, expected_micros) in [(0_u64, 0_u64), (2, 5), (1, 3), (3, 8)] {
            let fact = usage("openai", "gpt-test", Some((tokens, 0, 0)));
            let cost = project_cost(&fact, Some(&pricing)).expect("the projection must succeed");
            let UsageCostFact::Reported(cost) = cost else {
                panic!("a Reported usage with a rate must project Reported");
            };
            assert_eq!(
                cost.input.micros(),
                expected_micros,
                "{tokens} tokens at 2.5 micros/million must round up, not truncate"
            );
            assert_eq!(cost.total, cost.input);
        }
    }

    #[test]
    fn large_counts_and_rates_fail_closed_instead_of_wrapping() {
        let pricing = snapshot(
            "openai",
            "gpt-test",
            1,
            TokenRate::from_micros_per_million(u64::MAX),
            TokenRate::from_micros_per_million(u64::MAX),
            TokenRate::from_micros_per_million(u64::MAX),
        );
        let fact = usage("openai", "gpt-test", Some((u64::MAX, 0, u64::MAX)));
        assert_eq!(
            project_cost(&fact, Some(&pricing)),
            Err(CostProjectionError::AmountOverflow),
            "an unrepresentable amount must fail closed, never wrap or saturate"
        );
    }

    #[test]
    fn total_overflow_fails_closed_even_when_each_component_fits() {
        // One micro per token: each component fits, their exact sum does not.
        let rate = TokenRate::from_micros_per_million(1_000_000);
        let half = u64::MAX / 2;
        let pricing = snapshot("openai", "gpt-test", 1, rate, rate, rate);
        let fact = usage("openai", "gpt-test", Some((half + 1, 1, half + 1)));
        assert_eq!(
            project_cost(&fact, Some(&pricing)),
            Err(CostProjectionError::AmountOverflow)
        );
    }

    #[test]
    fn unknown_usage_or_missing_rate_stays_unknown_never_zero() {
        let pricing = snapshot(
            "openai",
            "gpt-test",
            1,
            TokenRate::from_micros_per_million(2_500_000),
            TokenRate::from_micros_per_million(1_250_000),
            TokenRate::from_micros_per_million(10_000_000),
        );
        // Unknown token counts with a known rate: no amount is inferred.
        let unknown = usage("openai", "gpt-test", None);
        assert_eq!(
            project_cost(&unknown, Some(&pricing)),
            Ok(UsageCostFact::Unknown {
                pricing: Some(pricing.reference())
            })
        );
        // Reported counts with no reviewed rate: no rate is guessed.
        let reported = usage("openai", "gpt-test", Some((10, 1, 2)));
        assert_eq!(
            project_cost(&reported, None),
            Ok(UsageCostFact::Unknown { pricing: None })
        );
        // Unknown counts and no rate.
        assert_eq!(
            project_cost(&unknown, None),
            Ok(UsageCostFact::Unknown { pricing: None })
        );
    }

    #[test]
    fn another_models_rate_is_never_applied() {
        let pricing = snapshot(
            "openai",
            "gpt-a",
            1,
            TokenRate::from_micros_per_million(1_000),
            TokenRate::from_micros_per_million(1_000),
            TokenRate::from_micros_per_million(1_000),
        );
        let wrong_model = usage("openai", "gpt-b", Some((10, 0, 0)));
        assert_eq!(
            project_cost(&wrong_model, Some(&pricing)),
            Err(CostProjectionError::AttributionMismatch)
        );
        let wrong_provider = usage("acme", "gpt-a", Some((10, 0, 0)));
        assert_eq!(
            project_cost(&wrong_provider, Some(&pricing)),
            Err(CostProjectionError::AttributionMismatch)
        );
    }

    #[test]
    fn malformed_usage_facts_are_refused_not_zero_filled() {
        let mut reported = usage("openai", "gpt-test", Some((10, 0, 0)));
        reported.cached_input_tokens = None;
        assert_eq!(
            project_cost(&reported, None),
            Err(CostProjectionError::InconsistentUsage)
        );
        let mut above_input = usage("openai", "gpt-test", Some((10, 11, 0)));
        assert_eq!(
            project_cost(&above_input, None),
            Err(CostProjectionError::InconsistentUsage)
        );
        above_input.source = UsageSource::Unknown;
        assert_eq!(
            project_cost(&above_input, None),
            Err(CostProjectionError::InconsistentUsage)
        );
    }

    #[test]
    fn cost_follows_the_bound_snapshot_not_the_current_catalog() {
        use crate::pricing::PricingResolution;
        use crate::pricing::tests_support::catalog_of;
        use ene_primitive::WallClockWithTz;

        let at = |value: &str| WallClockWithTz::parse_rfc3339(value).expect("fixture instant");
        let fact = usage("openai", "gpt-test", Some((1_000, 200, 500)));
        // The call was admitted under revision 1.
        let first = catalog_of(&snapshot(
            "openai",
            "gpt-test",
            1,
            rate(2_000_000),
            rate(500_000),
            rate(8_000_000),
        ));
        let PricingResolution::Priced(bound) =
            first.resolve("openai", "gpt-test", at("2025-07-01T00:00:00Z"))
        else {
            panic!("revision 1 covers the call instant");
        };
        let recorded =
            project_cost(&fact, Some(&bound)).expect("the bound snapshot prices the fact");
        // The current catalog later moves to revision 2 with different rates.
        let second = catalog_of(&snapshot(
            "openai",
            "gpt-test",
            2,
            rate(100_000),
            rate(50_000),
            rate(400_000),
        ));
        let PricingResolution::Priced(current) =
            second.resolve("openai", "gpt-test", at("2025-10-01T00:00:00Z"))
        else {
            panic!("revision 2 covers the later instant");
        };
        assert_ne!(bound.reference(), current.reference());
        // The recorded fact still projects from the bound revision; the
        // current revision only prices calls admitted after it.
        assert_eq!(
            project_cost(&fact, Some(&bound)).expect("the bound snapshot still prices the fact"),
            recorded
        );
        assert_ne!(
            project_cost(&fact, Some(&current)).expect("the current snapshot prices new calls"),
            recorded
        );
    }

    #[test]
    fn money_addition_refuses_overflow_and_currency_mixing() {
        assert_eq!(
            Money::from_micros(CurrencyCode::Usd, u64::MAX)
                .checked_add(Money::from_micros(CurrencyCode::Usd, 1)),
            None
        );
        assert_eq!(Money::zero(CurrencyCode::Usd).currency().as_str(), "USD");
        assert_eq!(CurrencyCode::from_code("USD"), Some(CurrencyCode::Usd));
        assert_eq!(CurrencyCode::from_code("EUR"), None);
    }
}
