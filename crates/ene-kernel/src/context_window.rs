//! Effective context-window budget and prompt packing.

use crate::config::TokenEstimation;

/// Conservative window when neither the provider nor the operator names one.
pub const DEFAULT_CONTEXT_WINDOW: u32 = 8192;

/// Prompt budget after response reserve and safety margin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveWindow {
    /// `min(advertised, configured)` or [`DEFAULT_CONTEXT_WINDOW`].
    pub effective: u32,
    /// Tokens held for the completion.
    pub response_reserve: u32,
    /// Tokens held against estimate error.
    pub safety_margin: u32,
    /// Tokens the prompt may occupy.
    pub available: u32,
}

/// Reconcile advertised, operator, and default windows, then reserve headroom.
///
/// The operator override can only shrink a named advertised limit. When both
/// sources are absent the conservative [`DEFAULT_CONTEXT_WINDOW`] is used.
#[must_use]
pub fn effective_window(
    provider_advertised: Option<u32>,
    user_configured: Option<u32>,
    response_reserve: Option<u32>,
    safety_margin_ratio: f32,
) -> EffectiveWindow {
    let effective = match (provider_advertised, user_configured) {
        (Some(advertised), Some(configured)) => advertised.min(configured),
        (Some(advertised), None) => advertised,
        (None, Some(configured)) => configured,
        (None, None) => DEFAULT_CONTEXT_WINDOW,
    };
    let response_reserve = response_reserve.unwrap_or(0).min(effective);
    let safety_margin = margin_tokens(effective, safety_margin_ratio)
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

fn margin_tokens(effective: u32, ratio: f32) -> u32 {
    let ratio = ratio.clamp(0.0, 1.0);
    if ratio <= 0.0 {
        return 0;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "ratio is clamped to [0, 1] so the product fits in u32"
    )]
    {
        (f64::from(effective) * f64::from(ratio)).floor() as u32
    }
}

/// Character-based token estimate for prompt packing.
#[must_use]
pub fn estimate_tokens(text: &str, mode: TokenEstimation) -> u32 {
    let chars = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
    if chars == 0 {
        return 0;
    }
    let resolved = match mode {
        TokenEstimation::Auto if cjk_ratio(text) >= 0.3 => TokenEstimation::Cjk15,
        TokenEstimation::Auto | TokenEstimation::Chars4 => TokenEstimation::Chars4,
        TokenEstimation::Cjk15 => TokenEstimation::Cjk15,
    };
    match resolved {
        TokenEstimation::Cjk15 => chars.saturating_mul(2).div_ceil(3).max(1),
        TokenEstimation::Auto | TokenEstimation::Chars4 => chars.div_ceil(4).max(1),
    }
}

fn cjk_ratio(text: &str) -> f32 {
    let total = text.chars().count();
    if total == 0 {
        return 0.0;
    }
    let cjk = text.chars().filter(|ch| is_cjk(*ch)).count();
    #[expect(
        clippy::cast_precision_loss,
        reason = "ratio is only used as a 0.3 threshold"
    )]
    {
        cjk as f32 / total as f32
    }
}

const fn is_cjk(ch: char) -> bool {
    matches!(
        ch,
        '\u{3040}'..='\u{30FF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{FF66}'..='\u{FF9D}'
    )
}

/// Drop oldest unpinned messages until the estimate fits `available_tokens`.
///
/// Pinned messages (system) are always kept. The newest unpinned message is
/// kept even when it alone exceeds the remaining budget.
#[must_use]
pub fn fit_prompt<T>(
    messages: Vec<T>,
    available_tokens: u32,
    tokens_of: impl Fn(&T) -> u32,
    pinned: impl Fn(&T) -> bool,
) -> Vec<T> {
    let total = messages.iter().map(&tokens_of).fold(0, u32::saturating_add);
    if total <= available_tokens {
        return messages;
    }
    let mut pinned_msgs = Vec::new();
    let mut rest = Vec::new();
    for message in messages {
        if pinned(&message) {
            pinned_msgs.push(message);
        } else {
            rest.push(message);
        }
    }
    let pinned_cost = pinned_msgs
        .iter()
        .map(&tokens_of)
        .fold(0, u32::saturating_add);
    let mut remaining = available_tokens.saturating_sub(pinned_cost);
    let mut kept_rest = Vec::new();
    for message in rest.into_iter().rev() {
        let cost = tokens_of(&message);
        if !kept_rest.is_empty() && cost > remaining {
            continue;
        }
        remaining = remaining.saturating_sub(cost);
        kept_rest.push(message);
    }
    kept_rest.reverse();
    pinned_msgs.extend(kept_rest);
    pinned_msgs
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_CONTEXT_WINDOW, effective_window, estimate_tokens, fit_prompt};
    use crate::config::TokenEstimation;

    #[test]
    fn unknown_window_uses_conservative_default() {
        let window = effective_window(None, None, None, 0.125);
        assert_eq!(window.effective, DEFAULT_CONTEXT_WINDOW);
        assert_eq!(window.response_reserve, 0);
        assert_eq!(window.safety_margin, DEFAULT_CONTEXT_WINDOW / 8);
        assert_eq!(
            window.available,
            DEFAULT_CONTEXT_WINDOW - DEFAULT_CONTEXT_WINDOW / 8
        );
    }

    #[test]
    fn operator_override_shrinks_advertised_window() {
        let window = effective_window(Some(200_000), Some(32_000), None, 0.0);
        assert_eq!(window.effective, 32_000);
    }

    #[test]
    fn advertised_caps_larger_override() {
        let window = effective_window(Some(8_000), Some(200_000), None, 0.0);
        assert_eq!(window.effective, 8_000);
    }

    #[test]
    fn response_reserve_is_consumed() {
        let window = effective_window(Some(16_000), None, Some(4_096), 0.0);
        assert_eq!(window.response_reserve, 4_096);
        assert_eq!(window.available, 16_000 - 4_096);
    }

    #[test]
    fn oversized_reserve_saturates_available() {
        let window = effective_window(Some(4_000), None, Some(8_000), 0.1);
        assert_eq!(window.response_reserve, 4_000);
        assert_eq!(window.available, 0);
    }

    #[test]
    fn chars4_and_cjk_estimates_differ() {
        assert_eq!(estimate_tokens("abcd", TokenEstimation::Chars4), 1);
        assert_eq!(estimate_tokens("日本語です", TokenEstimation::Cjk15), 4);
        assert_eq!(
            estimate_tokens("日本語の文章です", TokenEstimation::Auto),
            estimate_tokens("日本語の文章です", TokenEstimation::Cjk15)
        );
    }

    #[test]
    fn fit_prompt_keeps_pinned_and_latest() {
        let packed = fit_prompt(
            vec!["sys", "old-a", "old-b", "latest"],
            3,
            |text| u32::try_from(text.len()).unwrap(),
            |text| *text == "sys",
        );
        assert_eq!(packed, vec!["sys", "latest"]);
    }

    #[test]
    fn fit_prompt_under_budget_keeps_all() {
        let packed = fit_prompt(
            vec!["a", "b"],
            100,
            |text| u32::try_from(text.len()).unwrap(),
            |_| false,
        );
        assert_eq!(packed, vec!["a", "b"]);
    }
}
