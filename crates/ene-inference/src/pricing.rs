use ene_primitive::WallClockWithTz;
use thiserror::Error;
use uuid::Uuid;

use crate::cost::{CurrencyCode, TokenRate};

const PRICING_NAMESPACE: Uuid = Uuid::from_u128(0x0f0a_4b3a_7c2e_4d1b_9d5f_2a6c_8e0b_1f43);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PricingCatalogRevision(u64);

impl PricingCatalogRevision {
    #[must_use]
    pub const fn new(revision: u64) -> Self {
        Self(revision)
    }

    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PricingSnapshotRef(Uuid);

impl PricingSnapshotRef {
    #[must_use]
    pub fn to_text(self) -> String {
        self.0.as_hyphenated().to_string()
    }

    #[must_use]
    pub fn from_text(text: &str) -> Option<Self> {
        Uuid::parse_str(text).ok().map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricingSnapshot {
    pub provider: String,
    pub model: String,
    pub currency: CurrencyCode,
    pub input_rate: TokenRate,
    pub cached_input_rate: TokenRate,
    pub output_rate: TokenRate,
    pub effective_at: WallClockWithTz,
    pub source_revision: PricingCatalogRevision,
}

impl PricingSnapshot {
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

fn push_field(name: &mut String, value: &str) {
    name.push_str(&value.len().to_string());
    name.push(':');
    name.push_str(value);
    name.push(';');
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricingCatalogEntry {
    pub provider: String,
    pub model: String,
    pub currency: CurrencyCode,
    pub input_rate: TokenRate,
    pub cached_input_rate: TokenRate,
    pub output_rate: TokenRate,
    pub effective_at: WallClockWithTz,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricingCatalog {
    revision: PricingCatalogRevision,
    entries: Vec<PricingCatalogEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PricingResolution {
    Priced(PricingSnapshot),
    Unpriced,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PricingCatalogError {
    #[error("pricing catalog effective instant is invalid")]
    InvalidEffectiveInstant,
    #[error("pricing catalog has two entries for one route and effective instant")]
    DuplicateEffectiveInstant { provider: String, model: String },
}

impl PricingCatalog {
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

    #[must_use]
    pub const fn revision(&self) -> PricingCatalogRevision {
        self.revision
    }

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

pub const FIRST_PARTY_REVISION: PricingCatalogRevision = PricingCatalogRevision::new(2);

const FIRST_PARTY_EFFECTIVE_AT: &str = "2025-06-01T00:00:00Z";

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
