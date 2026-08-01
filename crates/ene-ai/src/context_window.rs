//! Effective context-window computation.
//!
//! An LLM's context window is the hard limit on `prompt + completion`
//! tokens. Two independent sources can name it:
//!
//! 1. the provider itself, via [`ene_plugin_proto::LlmProviderSpec::context_window`]
//!    (or, for local models, `LocalModelDef.context_size`), and
//! 2. the operator, via an explicit `ai.providers.<name>.context_window`
//!    config override.
//!
//! Neither source is authoritative on its own: a plugin may not advertise a
//! limit at all (older binaries omit the field), and an operator may want to
//! run a large-window model on a smaller budget. [`effective_window`]
//! reconciles them and then reserves headroom for the response and for
//! token-estimation error, yielding the number of tokens a prompt may
//! actually occupy.

/// Conservative context window assumed when neither the provider nor the
/// operator names one.
///
/// Deliberately small: an unknown window is treated as constrained so prompt
/// packing errs toward truncating early rather than overflowing a model whose
/// real limit we never learned. Large-window cloud models are expected to
/// advertise their limit (or be configured explicitly), which lifts this
/// floor.
pub const DEFAULT_CONTEXT_WINDOW: u32 = 8192;

/// Fraction of the effective window held back as a safety margin against
/// token-estimation error, when no measured usage is available.
///
/// Prompt budgets are computed from a character-based token *estimate*, which
/// is off by a language-dependent factor (Japanese packs ~2–3× more meaning
/// per token than English). Reserving one eighth of the window absorbs that
/// error so an over-estimate does not push the prompt past the model's real
/// limit. When a provider reports measured usage the caller can pass a
/// zero margin, since the estimate is then anchored to a real count.
pub const DEFAULT_SAFETY_MARGIN_FRACTION: u32 = 8;

/// A fully computed context-window budget for one task.
///
/// `available` is the number of tokens a prompt may occupy once the response
/// reserve and safety margin have been set aside; it is the value prompt
/// packing should budget against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveWindow {
    /// `min(model-advertised, user-configured)` window in tokens, or
    /// [`DEFAULT_CONTEXT_WINDOW`] when neither source names a limit.
    pub effective: u32,
    /// Tokens set aside for the model's completion (`tasks.<task>.max_tokens`).
    pub response_reserve: u32,
    /// Tokens set aside to absorb token-estimation error.
    pub safety_margin: u32,
    /// Tokens a prompt may actually occupy: `effective − response_reserve −
    /// safety_margin`, saturated at zero.
    pub available: u32,
}

/// Compute the effective context-window budget for a task.
///
/// Resolution priority for the window itself:
///
/// 1. `provider_advertised` — what the plugin (or local model) reports, and
/// 2. `user_configured` — an explicit operator override,
///
/// combined as `min(advertised, configured)` so an operator can only ever
/// *shrink* a model's stated limit, never exceed it. When both are `None`,
/// [`DEFAULT_CONTEXT_WINDOW`] is assumed.
///
/// From that effective window two reserves are subtracted:
///
/// - `response_reserve` — the completion cap (`tasks.<task>.max_tokens`),
///   reserved so a prompt can never fill the whole window and leave the model
///   no room to reply; and
/// - a safety margin of `effective / safety_margin_divisor` (pass
///   [`DEFAULT_SAFETY_MARGIN_FRACTION`] for the heuristic default, or `0`
///   when measured usage is available and the margin is unneeded).
///
/// All subtractions saturate, so a misconfigured reserve larger than the
/// window yields `available == 0` rather than underflowing.
#[must_use]
pub fn effective_window(
    provider_advertised: Option<u32>,
    user_configured: Option<u32>,
    response_reserve: Option<u32>,
    safety_margin_divisor: u32,
) -> EffectiveWindow {
    let effective = match (provider_advertised, user_configured) {
        (Some(advertised), Some(configured)) => advertised.min(configured),
        (Some(advertised), None) => advertised,
        (None, Some(configured)) => configured,
        (None, None) => DEFAULT_CONTEXT_WINDOW,
    };

    let response_reserve = response_reserve.unwrap_or(0).min(effective);
    let safety_margin = effective
        .checked_div(safety_margin_divisor)
        .unwrap_or(0)
        .min(effective.saturating_sub(response_reserve));
    let available = effective
        .saturating_sub(response_reserve)
        .saturating_sub(safety_margin);

    EffectiveWindow {
        effective,
        response_reserve,
        safety_margin,
        available,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_window_falls_back_to_conservative_default() {
        let w = effective_window(None, None, None, DEFAULT_SAFETY_MARGIN_FRACTION);
        assert_eq!(w.effective, DEFAULT_CONTEXT_WINDOW);
        assert_eq!(w.response_reserve, 0);
        assert_eq!(
            w.safety_margin,
            DEFAULT_CONTEXT_WINDOW / DEFAULT_SAFETY_MARGIN_FRACTION
        );
        assert_eq!(
            w.available,
            DEFAULT_CONTEXT_WINDOW - DEFAULT_CONTEXT_WINDOW / DEFAULT_SAFETY_MARGIN_FRACTION
        );
    }

    #[test]
    fn advertised_window_is_used_when_no_override() {
        let w = effective_window(Some(200_000), None, None, DEFAULT_SAFETY_MARGIN_FRACTION);
        assert_eq!(w.effective, 200_000);
    }

    #[test]
    fn user_override_caps_advertised_window() {
        let w = effective_window(
            Some(200_000),
            Some(32_000),
            None,
            DEFAULT_SAFETY_MARGIN_FRACTION,
        );
        assert_eq!(w.effective, 32_000);
    }

    #[test]
    fn advertised_caps_larger_user_override() {
        let w = effective_window(
            Some(8_000),
            Some(200_000),
            None,
            DEFAULT_SAFETY_MARGIN_FRACTION,
        );
        assert_eq!(w.effective, 8_000);
    }

    #[test]
    fn response_reserve_is_subtracted() {
        let w = effective_window(Some(16_000), None, Some(4_096), 0);
        assert_eq!(w.effective, 16_000);
        assert_eq!(w.response_reserve, 4_096);
        assert_eq!(w.safety_margin, 0);
        assert_eq!(w.available, 16_000 - 4_096);
    }

    #[test]
    fn safety_margin_is_fraction_of_effective() {
        let w = effective_window(Some(16_000), None, None, DEFAULT_SAFETY_MARGIN_FRACTION);
        assert_eq!(w.safety_margin, 16_000 / DEFAULT_SAFETY_MARGIN_FRACTION);
        assert_eq!(
            w.available,
            16_000 - 16_000 / DEFAULT_SAFETY_MARGIN_FRACTION
        );
    }

    #[test]
    fn zero_divisor_disables_safety_margin() {
        let w = effective_window(Some(16_000), None, Some(4_096), 0);
        assert_eq!(w.safety_margin, 0);
        assert_eq!(w.available, 16_000 - 4_096);
    }

    #[test]
    fn oversized_reserve_saturates_available_to_zero() {
        let w = effective_window(
            Some(4_000),
            None,
            Some(8_000),
            DEFAULT_SAFETY_MARGIN_FRACTION,
        );
        assert_eq!(w.response_reserve, 4_000);
        assert_eq!(w.available, 0);
    }

    #[test]
    fn margin_does_not_eat_into_reserve() {
        let w = effective_window(
            Some(8_000),
            None,
            Some(7_500),
            DEFAULT_SAFETY_MARGIN_FRACTION,
        );
        assert_eq!(w.response_reserve, 7_500);
        assert_eq!(w.safety_margin, 500);
        assert_eq!(w.available, 0);
    }
}
