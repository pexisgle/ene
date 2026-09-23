use thiserror::Error;

use crate::UsageFact;
use crate::pricing::{PricingSnapshot, PricingSnapshotRef};
pub use ene_primitive::money::{CurrencyCode, Money};

const RATE_DENOMINATOR: u128 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenRate {
    micros_per_million_tokens: u64,
}

impl TokenRate {
    #[must_use]
    pub const fn from_micros_per_million(micros_per_million_tokens: u64) -> Self {
        Self {
            micros_per_million_tokens,
        }
    }

    #[must_use]
    pub const fn micros_per_million(self) -> u64 {
        self.micros_per_million_tokens
    }

    #[must_use]
    pub fn checked_cost(self, currency: CurrencyCode, tokens: u64) -> Option<Money> {
        let scaled = u128::from(tokens).checked_mul(u128::from(self.micros_per_million_tokens))?;
        let rounded = scaled
            .checked_add(RATE_DENOMINATOR - 1)?
            .checked_div(RATE_DENOMINATOR)?;
        let micros = u64::try_from(rounded).ok()?;
        Some(Money::from_micros(currency, micros))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageEstimate {
    pub input_tokens_upper_bound: u64,
    pub output_tokens_upper_bound: u64,
}

impl UsageEstimate {
    #[must_use]
    pub fn upper_bound_cost(self, snapshot: &PricingSnapshot) -> Option<Money> {
        let input_rate = if snapshot.input_rate.micros_per_million()
            >= snapshot.cached_input_rate.micros_per_million()
        {
            snapshot.input_rate
        } else {
            snapshot.cached_input_rate
        };
        let input = input_rate.checked_cost(snapshot.currency, self.input_tokens_upper_bound)?;
        let input = input.checked_add(Money::from_micros(snapshot.currency, 1))?;
        let output = snapshot
            .output_rate
            .checked_cost(snapshot.currency, self.output_tokens_upper_bound)?;
        input.checked_add(output)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportedCost {
    pub input: Money,
    pub cached_input: Money,
    pub output: Money,
    pub total: Money,
    pub pricing: PricingSnapshotRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageCostFact {
    Reported(ReportedCost),
    Unknown { pricing: Option<PricingSnapshotRef> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CostProjectionError {
    #[error("usage counts are not internally consistent")]
    InconsistentUsage,
    #[error("pricing snapshot does not match the usage attribution")]
    AttributionMismatch,
    #[error("usage cost does not fit the money representation")]
    AmountOverflow,
}

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
        let unknown = usage("openai", "gpt-test", None);
        assert_eq!(
            project_cost(&unknown, Some(&pricing)),
            Ok(UsageCostFact::Unknown {
                pricing: Some(pricing.reference())
            })
        );
        let reported = usage("openai", "gpt-test", Some((10, 1, 2)));
        assert_eq!(
            project_cost(&reported, None),
            Ok(UsageCostFact::Unknown { pricing: None })
        );
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
    fn estimate_upper_bound_charges_the_more_expensive_input_rate() {
        let pricing = snapshot(
            "openai",
            "gpt-test",
            1,
            TokenRate::from_micros_per_million(1_000_000),
            TokenRate::from_micros_per_million(3_000_000),
            TokenRate::from_micros_per_million(2_000_000),
        );
        let estimate = super::UsageEstimate {
            input_tokens_upper_bound: 10,
            output_tokens_upper_bound: 4,
        };
        assert_eq!(
            estimate.upper_bound_cost(&pricing),
            Some(Money::from_micros(CurrencyCode::Usd, 39))
        );
    }

    #[test]
    fn estimate_upper_bound_never_falls_below_the_settled_total() {
        let pricing = snapshot(
            "openai",
            "gpt-4o-mini",
            1,
            TokenRate::from_micros_per_million(150_000),
            TokenRate::from_micros_per_million(75_000),
            TokenRate::from_micros_per_million(600_000),
        );
        let estimate = super::UsageEstimate {
            input_tokens_upper_bound: 1_000,
            output_tokens_upper_bound: 4_096,
        };
        let bound = estimate
            .upper_bound_cost(&pricing)
            .expect("the bound is representable");
        for cached in [0, 1, 2, 999, 1_000] {
            let fact = usage("openai", "gpt-4o-mini", Some((1_000, cached, 4_096)));
            let UsageCostFact::Reported(cost) =
                project_cost(&fact, Some(&pricing)).expect("the projection must succeed")
            else {
                panic!("a Reported usage with a rate must project Reported");
            };
            assert!(
                cost.total.micros() <= bound.micros(),
                "cached {cached}: settled {} must not exceed the reserved {}",
                cost.total.micros(),
                bound.micros()
            );
        }
        let split = usage("openai", "gpt-4o-mini", Some((1_000, 1, 4_096)));
        let UsageCostFact::Reported(cost) =
            project_cost(&split, Some(&pricing)).expect("the projection must succeed")
        else {
            panic!("a Reported usage with a rate must project Reported");
        };
        assert_eq!(cost.total.micros(), 2_609);
        assert_eq!(bound.micros(), 2_609);
    }

    #[test]
    fn estimate_upper_bound_rounds_up_and_fails_closed_on_overflow() {
        let pricing = snapshot(
            "openai",
            "gpt-test",
            1,
            TokenRate::from_micros_per_million(2_500_000),
            TokenRate::from_micros_per_million(2_500_000),
            TokenRate::from_micros_per_million(2_500_000),
        );
        let estimate = super::UsageEstimate {
            input_tokens_upper_bound: 2,
            output_tokens_upper_bound: 2,
        };
        assert_eq!(
            estimate.upper_bound_cost(&pricing),
            Some(Money::from_micros(CurrencyCode::Usd, 11))
        );
        let maxed = super::UsageEstimate {
            input_tokens_upper_bound: u64::MAX,
            output_tokens_upper_bound: u64::MAX,
        };
        let expensive = snapshot(
            "openai",
            "gpt-test",
            1,
            TokenRate::from_micros_per_million(u64::MAX),
            TokenRate::from_micros_per_million(u64::MAX),
            TokenRate::from_micros_per_million(u64::MAX),
        );
        assert_eq!(
            maxed.upper_bound_cost(&expensive),
            None,
            "an unrepresentable bound must fail closed, never wrap or saturate"
        );
    }
}
