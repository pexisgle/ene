#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CurrencyCode {
    Usd,
}

impl CurrencyCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Usd => "USD",
        }
    }

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
    #[must_use]
    pub const fn from_micros(currency: CurrencyCode, micros: u64) -> Self {
        Self { currency, micros }
    }

    #[must_use]
    pub const fn zero(currency: CurrencyCode) -> Self {
        Self::from_micros(currency, 0)
    }

    #[must_use]
    pub const fn currency(self) -> CurrencyCode {
        self.currency
    }

    #[must_use]
    pub const fn micros(self) -> u64 {
        self.micros
    }

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
