//! Reviewed first-party pricing and immutable snapshot resolution.
//!
//! `usage-cost-cap` §4/§4.1: a provider call fixes the rate it ran under as an
//! immutable snapshot, and the current catalog revision only affects calls
//! admitted after the update. Stage 6 does not fetch pricing over the network:
//! the source of truth is the reviewed first-party table in
//! [`PricingCatalog::first_party`].
//!
//! A snapshot's identity is derived from its full reviewed content
//! (`provider`, `model`, `currency`, the three rates, `effective_at`, and the
//! catalog revision), so the same revision resolves to the same durable
//! identity in every process, and a stored row whose content changed under a
//! reference fails verification instead of silently repricing history.
//!
//! Resolution is exact and total for a well-formed catalog: for one route and
//! call instant it selects the latest entry whose `effective_at` is not after
//! the call. A route with no covering entry resolves [`PricingResolution::Unpriced`];
//! another model's rate is never substituted, and the caller settles the cost
//! as Unknown.

use ene_primitive::WallClockWithTz;
use thiserror::Error;
use uuid::Uuid;

use crate::cost::{CurrencyCode, TokenRate};

/// Frozen namespace for snapshot identity derivation. Changing it would
/// rebind every published identity, so it is a constant.
const PRICING_NAMESPACE: Uuid = Uuid::from_u128(0x0f0a_4b3a_7c2e_4d1b_9d5f_2a6c_8e0b_1f43);

/// Revision of the reviewed pricing catalog as a whole.
///
/// A revision is immutable: once published, its entries may not be edited.
/// A price change is a new revision, and only calls admitted after it resolve
/// the new snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PricingCatalogRevision(u64);

impl PricingCatalogRevision {
    /// Builds a revision value.
    #[must_use]
    pub const fn new(revision: u64) -> Self {
        Self(revision)
    }

    /// The stored revision number.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// Durable identity of one published [`PricingSnapshot`].
///
/// Opaque: the value is derived from the reviewed content, never chosen by a
/// caller. It is a text-parsable identity (unlike domain identities), because
/// the reference is what durable rows store and join on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PricingSnapshotRef(Uuid);

impl PricingSnapshotRef {
    /// Renders the reference for durable storage.
    #[must_use]
    pub fn to_text(self) -> String {
        self.0.as_hyphenated().to_string()
    }

    /// Parses a stored reference. Malformed text is `None`, never a fresh or
    /// default identity.
    #[must_use]
    pub fn from_text(text: &str) -> Option<Self> {
        Uuid::parse_str(text).ok().map(Self)
    }
}

/// One reviewed rate for one provider route at one effective instant.
///
/// This is the resolved, immutable content of a provider call's pricing, per
/// `usage-cost-cap` §4. It carries no prompt, output, or credential material:
/// only the route, the currency, the three token rates, the effective instant,
/// and the catalog revision they came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricingSnapshot {
    /// Provider the rate belongs to (exact match; never a family guess).
    pub provider: String,
    /// Model the rate belongs to (exact match; never a prefix or alias).
    pub model: String,
    /// Currency all three rates are denominated in.
    pub currency: CurrencyCode,
    /// Rate for non-cached input tokens.
    pub input_rate: TokenRate,
    /// Rate for the cached subset of input tokens.
    pub cached_input_rate: TokenRate,
    /// Rate for output tokens.
    pub output_rate: TokenRate,
    /// Instant the reviewed rate takes effect.
    pub effective_at: WallClockWithTz,
    /// Reviewed catalog revision the rate came from.
    pub source_revision: PricingCatalogRevision,
}

impl PricingSnapshot {
    /// The content-derived durable identity of this snapshot.
    #[must_use]
    pub fn reference(&self) -> PricingSnapshotRef {
        let mut name = String::new();
        push_field(&mut name, &self.provider);
        push_field(&mut name, &self.model);
        push_field(&mut name, self.currency.as_str());
        push_field(&mut name, &self.input_rate.micros_per_million().to_string());
        push_field(
            &mut name,
            &self.cached_input_rate.micros_per_million().to_string(),
        );
        push_field(
            &mut name,
            &self.output_rate.micros_per_million().to_string(),
        );
        push_field(&mut name, &self.effective_at.to_rfc3339_utc());
        push_field(&mut name, &self.source_revision.as_u64().to_string());
        PricingSnapshotRef(Uuid::new_v5(&PRICING_NAMESPACE, name.as_bytes()))
    }
}

/// Length-prefixed field framing: concatenating plain values could collide
/// (`("ab", "c")` vs `("a", "bc")`), so every field carries its own length.
fn push_field(name: &mut String, value: &str) {
    name.push_str(&value.len().to_string());
    name.push(':');
    name.push_str(value);
    name.push(';');
}

/// One reviewed catalog entry: an exact route rate effective from an instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricingCatalogEntry {
    /// Provider name the rate applies to.
    pub provider: String,
    /// Model name the rate applies to.
    pub model: String,
    /// Currency all three rates are denominated in.
    pub currency: CurrencyCode,
    /// Rate for non-cached input tokens.
    pub input_rate: TokenRate,
    /// Rate for the cached subset of input tokens.
    pub cached_input_rate: TokenRate,
    /// Rate for output tokens.
    pub output_rate: TokenRate,
    /// Instant this rate becomes effective; the next entry for the same
    /// route replaces it.
    pub effective_at: WallClockWithTz,
}

/// The reviewed pricing source of truth, at one revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricingCatalog {
    revision: PricingCatalogRevision,
    entries: Vec<PricingCatalogEntry>,
}

/// Resolution of one route at one instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PricingResolution {
    /// The reviewed rate covering the call instant. The provider call binds
    /// this snapshot before its attempt claim.
    Priced(PricingSnapshot),
    /// No reviewed rate covers this route at the call instant. The caller
    /// must not substitute another model's rate or zero: the cost fact
    /// settles Unknown.
    Unpriced,
}

/// Why a reviewed catalog is malformed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PricingCatalogError {
    /// A first-party effective instant literal is not a valid timestamp.
    #[error("pricing catalog effective instant is invalid")]
    InvalidEffectiveInstant,
    /// Two entries cover the same route from the same instant, so resolution
    /// would be order-dependent. Resolution never guesses between them.
    #[error("pricing catalog has two entries for one route and effective instant")]
    DuplicateEffectiveInstant {
        /// Provider of the ambiguous route.
        provider: String,
        /// Model of the ambiguous route.
        model: String,
    },
}

impl PricingCatalog {
    /// Builds a catalog, refusing ambiguous entries.
    ///
    /// # Errors
    ///
    /// Returns [`PricingCatalogError::DuplicateEffectiveInstant`] when two
    /// entries name the same `(provider, model)` and the same effective
    /// instant: picking either rate would be an arbitrary guess.
    pub fn new(
        revision: PricingCatalogRevision,
        entries: Vec<PricingCatalogEntry>,
    ) -> Result<Self, PricingCatalogError> {
        for (index, entry) in entries.iter().enumerate() {
            for other in &entries[index + 1..] {
                if entry.provider == other.provider
                    && entry.model == other.model
                    && entry.effective_at == other.effective_at
                {
                    return Err(PricingCatalogError::DuplicateEffectiveInstant {
                        provider: entry.provider.clone(),
                        model: entry.model.clone(),
                    });
                }
            }
        }
        Ok(Self { revision, entries })
    }

    /// The catalog revision.
    #[must_use]
    pub const fn revision(&self) -> PricingCatalogRevision {
        self.revision
    }

    /// Resolves the reviewed rate for `(provider, model)` at `at`.
    ///
    /// The provider and model match exactly; there is no alias, prefix, or
    /// family fallback. When several entries for the route are effective by
    /// `at`, the latest effective instant wins, so an updated revision only
    /// affects calls at or after it.
    #[must_use]
    pub fn resolve(&self, provider: &str, model: &str, at: WallClockWithTz) -> PricingResolution {
        let entry = self
            .entries
            .iter()
            .filter(|entry| {
                entry.provider == provider
                    && entry.model == model
                    && entry.effective_at.as_datetime() <= at.as_datetime()
            })
            .max_by_key(|entry| entry.effective_at.as_datetime());
        match entry {
            Some(entry) => PricingResolution::Priced(PricingSnapshot {
                provider: entry.provider.clone(),
                model: entry.model.clone(),
                currency: entry.currency,
                input_rate: entry.input_rate,
                cached_input_rate: entry.cached_input_rate,
                output_rate: entry.output_rate,
                effective_at: entry.effective_at,
                source_revision: self.revision,
            }),
            None => PricingResolution::Unpriced,
        }
    }

    /// The reviewed first-party Stage 6 catalog (OpenAI, USD).
    ///
    /// The rates are reviewed first-party data (micro-USD per 1,000,000
    /// tokens) and are not fetched from the network. A price update is a new
    /// catalog revision with a later effective instant; published revisions
    /// are never edited.
    ///
    /// # Errors
    ///
    /// Returns [`PricingCatalogError::InvalidEffectiveInstant`] if the
    /// compiled-in effective instant is malformed (a first-party data defect),
    /// or [`PricingCatalogError::DuplicateEffectiveInstant`] if the table
    /// became ambiguous.
    pub fn first_party() -> Result<Self, PricingCatalogError> {
        let effective_at = WallClockWithTz::parse_rfc3339(FIRST_PARTY_EFFECTIVE_AT)
            .map_err(|_| PricingCatalogError::InvalidEffectiveInstant)?;
        let entries = FIRST_PARTY_OPENAI_RATES
            .iter()
            .map(|(model, input, cached_input, output)| PricingCatalogEntry {
                provider: String::from("openai"),
                model: String::from(*model),
                currency: CurrencyCode::Usd,
                input_rate: TokenRate::from_micros_per_million(*input),
                cached_input_rate: TokenRate::from_micros_per_million(*cached_input),
                output_rate: TokenRate::from_micros_per_million(*output),
                effective_at,
            })
            .collect();
        Self::new(FIRST_PARTY_REVISION, entries)
    }
}

/// Revision of the compiled-in reviewed catalog. Bump it when any entry
/// changes; never edit a published revision in place.
pub const FIRST_PARTY_REVISION: PricingCatalogRevision = PricingCatalogRevision::new(2);

/// Effective instant of [`FIRST_PARTY_REVISION`].
const FIRST_PARTY_EFFECTIVE_AT: &str = "2025-06-01T00:00:00Z";

/// Reviewed OpenAI rates in micro-USD per 1,000,000 tokens:
/// `(model, input, cached input, output)`, Standard processing.
///
/// The reviewed rates are the published short-context rates (at most 272K
/// input tokens). OpenAI charges more above that threshold, and this
/// single-rate catalog does not model the long-context band; dispatch refuses
/// prompts over [`crate::MAX_INPUT_CHARS`], far below the boundary.
const FIRST_PARTY_OPENAI_RATES: &[(&str, u64, u64, u64)] = &[
    ("gpt-6-astra", 10_000_000, 1_000_000, 50_000_000),
    ("gpt-5.6-sol", 4_000_000, 400_000, 20_000_000),
    ("gpt-5.6-terra", 2_000_000, 200_000, 12_000_000),
    ("gpt-5.6-luna", 200_000, 20_000, 1_200_000),
    ("gpt-5.5", 5_000_000, 500_000, 30_000_000),
    ("gpt-4o", 2_500_000, 1_250_000, 10_000_000),
    ("gpt-4o-mini", 150_000, 75_000, 600_000),
    ("gpt-4.1", 2_000_000, 500_000, 8_000_000),
    ("gpt-4.1-mini", 400_000, 100_000, 1_600_000),
    ("o4-mini", 1_100_000, 275_000, 4_400_000),
];

#[cfg(test)]
pub(crate) mod tests_support {
    use super::{CurrencyCode, PricingCatalog, PricingCatalogRevision, PricingSnapshot, TokenRate};
    use ene_primitive::WallClockWithTz;

    /// Test fixture: one exactly-rated snapshot for `(provider, model)`.
    pub(crate) fn snapshot(
        provider: &str,
        model: &str,
        revision: u64,
        input_rate: TokenRate,
        cached_input_rate: TokenRate,
        output_rate: TokenRate,
    ) -> PricingSnapshot {
        PricingSnapshot {
            provider: provider.to_owned(),
            model: model.to_owned(),
            currency: CurrencyCode::Usd,
            input_rate,
            cached_input_rate,
            output_rate,
            effective_at: WallClockWithTz::parse_rfc3339("2025-06-01T00:00:00Z")
                .expect("the fixture instant parses"),
            source_revision: PricingCatalogRevision::new(revision),
        }
    }

    /// Test fixture: a one-entry catalog with the same rates as [`snapshot`].
    pub(crate) fn catalog_of(snapshot: &PricingSnapshot) -> PricingCatalog {
        PricingCatalog::new(
            snapshot.source_revision,
            vec![super::PricingCatalogEntry {
                provider: snapshot.provider.clone(),
                model: snapshot.model.clone(),
                currency: snapshot.currency,
                input_rate: snapshot.input_rate,
                cached_input_rate: snapshot.cached_input_rate,
                output_rate: snapshot.output_rate,
                effective_at: snapshot.effective_at,
            }],
        )
        .expect("the fixture catalog is unambiguous")
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::snapshot;
    use super::{
        FIRST_PARTY_REVISION, PricingCatalog, PricingCatalogEntry, PricingCatalogError,
        PricingCatalogRevision, PricingResolution,
    };
    use crate::cost::{CurrencyCode, TokenRate};
    use ene_primitive::WallClockWithTz;

    fn at(value: &str) -> WallClockWithTz {
        WallClockWithTz::parse_rfc3339(value).expect("the fixture instant parses")
    }

    fn rate(micros: u64) -> TokenRate {
        TokenRate::from_micros_per_million(micros)
    }

    fn entry(model: &str, at_value: &str) -> PricingCatalogEntry {
        PricingCatalogEntry {
            provider: String::from("openai"),
            model: String::from(model),
            currency: CurrencyCode::Usd,
            input_rate: rate(1_000_000),
            cached_input_rate: rate(500_000),
            output_rate: rate(2_000_000),
            effective_at: at(at_value),
        }
    }

    #[test]
    fn first_party_catalog_resolves_reviewed_openai_rates() {
        let catalog = PricingCatalog::first_party().expect("the reviewed table must be valid");
        assert_eq!(catalog.revision(), FIRST_PARTY_REVISION);
        let PricingResolution::Priced(snapshot) =
            catalog.resolve("openai", "gpt-4o", at("2026-09-17T12:00:00Z"))
        else {
            panic!("a reviewed OpenAI model must resolve");
        };
        assert_eq!(snapshot.provider, "openai");
        assert_eq!(snapshot.model, "gpt-4o");
        assert_eq!(snapshot.currency, CurrencyCode::Usd);
        assert_eq!(snapshot.input_rate, rate(2_500_000));
        assert_eq!(snapshot.cached_input_rate, rate(1_250_000));
        assert_eq!(snapshot.output_rate, rate(10_000_000));
        assert_eq!(snapshot.source_revision, FIRST_PARTY_REVISION);
        // The same content yields the same durable identity across catalogs
        // and processes.
        let rebuilt = PricingCatalog::first_party().expect("the reviewed table must be valid");
        let PricingResolution::Priced(rebuilt) =
            rebuilt.resolve("openai", "gpt-4o", at("2026-09-17T12:00:00Z"))
        else {
            panic!("a reviewed OpenAI model must resolve");
        };
        assert_eq!(snapshot.reference(), rebuilt.reference());
    }

    #[test]
    fn first_party_catalog_prices_the_current_gpt_families() {
        let catalog = PricingCatalog::first_party().expect("the reviewed table must be valid");
        let at_value = at("2026-09-21T12:00:00Z");
        for (model, input, cached_input, output) in [
            ("gpt-6-astra", 10_000_000, 1_000_000, 50_000_000),
            ("gpt-5.6-sol", 4_000_000, 400_000, 20_000_000),
            ("gpt-5.6-terra", 2_000_000, 200_000, 12_000_000),
            ("gpt-5.6-luna", 200_000, 20_000, 1_200_000),
            ("gpt-5.5", 5_000_000, 500_000, 30_000_000),
        ] {
            let PricingResolution::Priced(snapshot) = catalog.resolve("openai", model, at_value)
            else {
                panic!("{model} must resolve its reviewed rate");
            };
            assert_eq!(snapshot.input_rate, rate(input), "{model} input rate");
            assert_eq!(
                snapshot.cached_input_rate,
                rate(cached_input),
                "{model} cached input rate"
            );
            assert_eq!(snapshot.output_rate, rate(output), "{model} output rate");
            assert_eq!(snapshot.source_revision, FIRST_PARTY_REVISION);
        }
    }

    #[test]
    fn unknown_routes_and_other_providers_are_unpriced() {
        let catalog = PricingCatalog::first_party().expect("the reviewed table must be valid");
        let at_value = at("2026-09-17T12:00:00Z");
        for (provider, model) in [
            ("openai", "gpt-4o-2024-11-20"),
            ("openai", "gpt-4"),
            ("openai", ""),
            ("acme", "gpt-4o"),
        ] {
            assert_eq!(
                catalog.resolve(provider, model, at_value),
                PricingResolution::Unpriced,
                "{provider}/{model} must never borrow another route's rate"
            );
        }
    }

    #[test]
    fn resolution_picks_the_latest_effective_entry_for_the_route() {
        let older = snapshot(
            "openai",
            "gpt-test",
            1,
            rate(1_000_000),
            rate(500_000),
            rate(2_000_000),
        );
        let newer = snapshot(
            "openai",
            "gpt-test",
            2,
            rate(3_000_000),
            rate(1_500_000),
            rate(6_000_000),
        );
        let catalog = PricingCatalog::new(
            PricingCatalogRevision::new(2),
            vec![
                entry("gpt-test", "2025-06-01T00:00:00Z"),
                PricingCatalogEntry {
                    provider: newer.provider.clone(),
                    model: newer.model.clone(),
                    currency: newer.currency,
                    input_rate: newer.input_rate,
                    cached_input_rate: newer.cached_input_rate,
                    output_rate: newer.output_rate,
                    effective_at: at("2025-09-01T00:00:00Z"),
                },
            ],
        )
        .expect("the fixture catalog is unambiguous");
        let PricingResolution::Priced(before) =
            catalog.resolve("openai", "gpt-test", at("2025-07-01T00:00:00Z"))
        else {
            panic!("the older entry must cover July");
        };
        assert_eq!(before.input_rate, older.input_rate);
        assert_eq!(before.source_revision, PricingCatalogRevision::new(2));
        let PricingResolution::Priced(after) =
            catalog.resolve("openai", "gpt-test", at("2025-10-01T00:00:00Z"))
        else {
            panic!("the newer entry must cover October");
        };
        assert_eq!(after.input_rate, newer.input_rate);
        // A revision that is not effective yet does not reprice earlier calls.
        assert_eq!(
            catalog.resolve("openai", "gpt-test", at("2025-05-01T00:00:00Z")),
            PricingResolution::Unpriced
        );
    }

    #[test]
    fn duplicate_effective_instants_for_one_route_are_refused() {
        let result = PricingCatalog::new(
            PricingCatalogRevision::new(3),
            vec![
                entry("gpt-test", "2025-06-01T00:00:00Z"),
                entry("gpt-test", "2025-06-01T00:00:00Z"),
            ],
        );
        assert_eq!(
            result,
            Err(PricingCatalogError::DuplicateEffectiveInstant {
                provider: String::from("openai"),
                model: String::from("gpt-test"),
            })
        );
        // The same instant on different routes is not ambiguous.
        let other = PricingCatalog::new(
            PricingCatalogRevision::new(3),
            vec![
                entry("gpt-a", "2025-06-01T00:00:00Z"),
                entry("gpt-b", "2025-06-01T00:00:00Z"),
            ],
        );
        assert!(other.is_ok());
    }

    #[test]
    fn reference_distinguishes_every_reviewed_field() {
        let base = snapshot(
            "openai",
            "gpt-test",
            1,
            rate(1_000_000),
            rate(500_000),
            rate(2_000_000),
        );
        let reference = base.reference();
        assert_eq!(reference, base.clone().reference());
        let mut changed_rate = base.clone();
        changed_rate.output_rate = rate(2_000_001);
        assert_ne!(reference, changed_rate.reference());
        let mut changed_revision = base.clone();
        changed_revision.source_revision = PricingCatalogRevision::new(2);
        assert_ne!(reference, changed_revision.reference());
        let mut changed_model = base.clone();
        changed_model.model = String::from("gpt-other");
        assert_ne!(reference, changed_model.reference());
        // Field framing is length-prefixed, so concatenation collisions do
        // not alias two different snapshots.
        let mut split = base.clone();
        split.provider = String::from("openaiX");
        split.model = String::from("gpt-tes");
        assert_ne!(reference, split.reference());
        assert_eq!(
            reference.to_text(),
            base.reference().to_text(),
            "the rendered reference is stable"
        );
    }

    #[test]
    fn stored_reference_text_round_trips() {
        let pricing = snapshot(
            "openai",
            "gpt-test",
            1,
            rate(1_000_000),
            rate(500_000),
            rate(2_000_000),
        );
        let reference = pricing.reference();
        assert_eq!(
            super::PricingSnapshotRef::from_text(&reference.to_text()),
            Some(reference)
        );
        assert_eq!(
            super::PricingSnapshotRef::from_text("not-a-reference"),
            None
        );
    }
}
