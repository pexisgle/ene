mod color;
mod currency;
mod evaluate;
mod unit;

pub use color::ColorConvertAction;
pub use currency::CurrencyConvertAction;
pub use evaluate::EvaluateAction;
pub use unit::UnitConvertAction;

/// Formats an f64 for display: scientific notation outside the
/// 1e-12..1e15 range, otherwise 12 decimals with trailing zeros
/// trimmed. The 12-decimal rounding absorbs the sub-1e-12 error of
/// affine conversions (e.g. 0 °C → 31.999999999999943 °F, and 32 °F →
/// 5.7e-14 °C which prints as 0).
pub(crate) fn format_number(value: f64) -> String {
    if value == 0.0 || value.abs() < 1.0e-12 {
        return "0".to_string();
    }
    if value.abs() >= 1.0e15 {
        return value.to_string();
    }
    format!("{value:.12}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}
