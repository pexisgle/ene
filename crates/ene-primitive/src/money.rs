//! Exact currency amounts shared by cost facts and usage caps.
//!
//! The amount type is a primitive because two owners need the same meaning:
//! inference projects a provider call's cost from its pricing snapshot, and
//! permission / constraint stores the cap limit that call is admitted under.
//! A second amount type (or a floating-point conversion between them) would
//! let the two sides compare different values.
//!
//! Floating point is never the canonical value: [`Money`] is an exact count of
//! micro-currency units. Amounts of different currencies are never combined or
//! converted by a guess; [`Money::checked_add`] answers `None` instead.

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
