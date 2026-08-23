//! Effective context-window budget and prompt packing.

use crate::config::TokenEstimation;
use ene_plugin_ipc::{LlmMessage, LlmRole};
use std::collections::HashSet;

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

/// Drop oldest unpinned groups until the estimate fits `available_tokens`.
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

/// Pack provider messages without splitting a tool call from any result.
///
/// A malformed exchange is dropped entirely: a tool result with no preceding
/// matching call cannot be sent to OpenAI-style providers, and an unanswered
/// call must not wedge the following turn.
#[must_use]
pub fn fit_prompt_llm(
    messages: Vec<LlmMessage>,
    available_tokens: u32,
    tokens_of: impl Fn(&LlmMessage) -> u32,
) -> Vec<LlmMessage> {
    let groups = group_tool_exchanges(messages);
    let valid_groups = groups
        .into_iter()
        .filter(ToolExchangeGroup::is_complete)
        .map(|group| group.messages)
        .collect::<Vec<Vec<LlmMessage>>>();

    let group_tokens =
        |group: &[LlmMessage]| group.iter().map(&tokens_of).fold(0, u32::saturating_add);
    let mut pinned = Vec::new();
    let mut rest = Vec::new();
    for group in valid_groups {
        if group
            .first()
            .is_some_and(|message| message.role == LlmRole::System)
        {
            pinned.push(group);
        } else {
            rest.push(group);
        }
    }
    let mut remaining = available_tokens.saturating_sub(
        pinned
            .iter()
            .map(|group| group_tokens(group))
            .fold(0, u32::saturating_add),
    );
    let mut packed = Vec::new();
    for (position, group) in rest.into_iter().rev().enumerate() {
        let cost = group_tokens(&group);
        if cost > remaining && !(position == 0 && is_newest_ordinary_user(&group)) {
            continue;
        }
        remaining = remaining.saturating_sub(cost);
        packed.push(group);
    }
    packed.reverse();
    let mut messages = Vec::with_capacity(packed.iter().map(Vec::len).sum());
    for group in pinned.into_iter().chain(packed) {
        messages.extend(group);
    }
    messages
}

struct ToolExchangeGroup {
    messages: Vec<LlmMessage>,
}

impl ToolExchangeGroup {
    fn is_complete(&self) -> bool {
        let Some(call_message) = self.messages.first() else {
            return false;
        };
        if call_message.role != LlmRole::Assistant || call_message.tool_calls.is_empty() {
            return !self
                .messages
                .iter()
                .any(|message| message.role == LlmRole::Tool || !message.tool_calls.is_empty());
        }
        let expected_ids: HashSet<&str> = call_message
            .tool_calls
            .iter()
            .map(|call| call.id.as_str())
            .collect();
        if expected_ids.len() != call_message.tool_calls.len() {
            return false;
        }
        let mut answered_ids = HashSet::new();
        for result in self.messages.iter().skip(1) {
            if result.role != LlmRole::Tool {
                return false;
            }
            let Some(id) = &result.tool_call_id else {
                return false;
            };
            if !answered_ids.insert(id.clone()) {
                return false;
            }
            if !expected_ids.contains(id.as_str()) {
                return false;
            }
        }
        answered_ids.len() == expected_ids.len()
    }

    fn accepts_result(&self) -> bool {
        self.messages
            .first()
            .is_some_and(|first| first.role == LlmRole::Assistant)
            && self
                .messages
                .iter()
                .any(|message| !message.tool_calls.is_empty())
            && !self.is_complete()
    }

    fn accepts_message(&self, message: &LlmMessage) -> bool {
        message.role == LlmRole::Tool && self.accepts_result()
    }
}

fn is_newest_ordinary_user(messages: &[LlmMessage]) -> bool {
    messages.len() == 1
        && messages[0].role == LlmRole::User
        && messages[0].tool_calls.is_empty()
        && messages[0].tool_call_id.is_none()
}

fn group_tool_exchanges(messages: Vec<LlmMessage>) -> Vec<ToolExchangeGroup> {
    let mut groups: Vec<ToolExchangeGroup> = Vec::new();
    for message in messages {
        let starts_exchange = message.role == LlmRole::Assistant && !message.tool_calls.is_empty();
        if starts_exchange
            || groups
                .last()
                .is_none_or(|last| !last.accepts_message(&message))
        {
            groups.push(ToolExchangeGroup {
                messages: vec![message],
            });
        } else if let Some(last) = groups.last_mut() {
            last.messages.push(message);
        } else {
            unreachable!();
        }
    }
    groups.retain(|group| !group.messages.is_empty());
    groups
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_CONTEXT_WINDOW, ToolExchangeGroup, effective_window, estimate_tokens, fit_prompt,
        fit_prompt_llm, group_tool_exchanges,
    };
    use crate::config::TokenEstimation;
    use ene_plugin_ipc::{LlmMessage, LlmRole, LlmToolCall};

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

    fn tool_call(id: &str) -> LlmToolCall {
        LlmToolCall {
            id: id.to_owned(),
            name: "web__search".to_owned(),
            arguments: serde_json::json!({"query": "large"}),
        }
    }

    fn tool_message(id: &str, payload: &str) -> LlmMessage {
        let mut message = LlmMessage::new(LlmRole::Tool, payload);
        message.tool_call_id = Some(id.to_owned());
        message
    }

    fn message_tokens(message: &LlmMessage) -> u32 {
        u32::try_from(message.text.chars().count()).unwrap_or(u32::MAX)
    }

    #[test]
    fn llm_fit_keeps_large_tool_result_with_its_call() {
        let messages = vec![
            LlmMessage::new(LlmRole::User, "search"),
            LlmMessage {
                role: LlmRole::Assistant,
                text: String::new(),
                tool_calls: vec![tool_call("call-1")],
                tool_call_id: None,
                tool_name: None,
                images: Vec::new(),
            },
            tool_message("call-1", &"x".repeat(100)),
            LlmMessage::new(LlmRole::User, "latest"),
        ];
        let packed = fit_prompt_llm(messages, 1, message_tokens);
        let tool_results = packed
            .iter()
            .filter(|message| message.role == LlmRole::Tool)
            .count();
        let calls = packed
            .iter()
            .filter(|message| !message.tool_calls.is_empty())
            .count();

        assert_eq!(
            tool_results + calls,
            0,
            "oversized exchange must be dropped"
        );
        assert_eq!(packed.last().map(|m| m.text.clone()), Some("latest".into()));
    }

    #[test]
    fn llm_fit_drops_orphan_tool_result() {
        let messages = vec![
            LlmMessage::new(LlmRole::User, "before"),
            tool_message("missing", "orphan"),
            LlmMessage::new(LlmRole::User, "latest"),
        ];
        let packed = fit_prompt_llm(messages, 100, message_tokens);

        assert_eq!(
            packed
                .iter()
                .map(|message| message.role)
                .collect::<Vec<_>>(),
            vec![LlmRole::User, LlmRole::User]
        );
    }

    #[test]
    fn every_tool_group_is_atomic_under_random_budgets() {
        let messages = vec![
            LlmMessage::new(LlmRole::System, "contract"),
            LlmMessage::new(LlmRole::User, "old-turn-".repeat(20)),
            LlmMessage {
                role: LlmRole::Assistant,
                text: String::new(),
                tool_calls: vec![tool_call("old-call"), tool_call("old-call-2")],
                tool_call_id: None,
                tool_name: None,
                images: Vec::new(),
            },
            tool_message("old-call", "old-result"),
            tool_message("old-call-2", &"x".repeat(80)),
            LlmMessage::new(LlmRole::User, "latest"),
        ];

        for budget in 1..=120_u32 {
            let packed = fit_prompt_llm(messages.clone(), budget, message_tokens);
            let call_ids: std::collections::HashSet<_> = packed
                .iter()
                .flat_map(|message| message.tool_calls.iter().map(|call| call.id.clone()))
                .collect();
            for result in packed
                .iter()
                .filter(|message| message.role == LlmRole::Tool)
            {
                assert!(
                    result
                        .tool_call_id
                        .as_deref()
                        .is_some_and(|id| call_ids.contains(id)),
                    "budget {budget} orphaned {result:?}"
                );
            }
            for call in packed.iter().flat_map(|message| message.tool_calls.iter()) {
                assert!(
                    packed.iter().any(|message| {
                        message.role == LlmRole::Tool
                            && message.tool_call_id.as_deref() == Some(call.id.as_str())
                    }),
                    "budget {budget} left unanswered {call:?}"
                );
            }
        }
    }

    #[test]
    fn multiple_tool_calls_are_kept_or_dropped_as_one_group() {
        let messages = vec![
            LlmMessage::new(LlmRole::User, "search"),
            LlmMessage {
                role: LlmRole::Assistant,
                text: String::new(),
                tool_calls: vec![tool_call("call-1"), tool_call("call-2")],
                tool_call_id: None,
                tool_name: None,
                images: Vec::new(),
            },
            tool_message("call-1", "one"),
            tool_message("call-2", "two"),
            LlmMessage::new(LlmRole::User, "latest"),
        ];

        let packed = fit_prompt_llm(messages.clone(), 100, message_tokens);
        assert_eq!(packed.len(), messages.len());

        let packed = fit_prompt_llm(messages, 4, message_tokens);
        assert_eq!(
            packed
                .iter()
                .filter(|message| message.role == LlmRole::Tool)
                .count(),
            0
        );
        assert_eq!(
            packed.last().map(|message| message.text.as_str()),
            Some("latest")
        );
    }

    #[test]
    fn oversized_latest_tool_group_does_not_pin_an_older_user() {
        let messages = vec![
            LlmMessage::new(LlmRole::User, "old request"),
            LlmMessage {
                role: LlmRole::Assistant,
                text: String::new(),
                tool_calls: vec![tool_call("call-1")],
                tool_call_id: None,
                tool_name: None,
                images: Vec::new(),
            },
            tool_message("call-1", &"x".repeat(100)),
        ];

        let packed = fit_prompt_llm(messages, 1, message_tokens);
        assert!(packed.is_empty());
    }

    #[test]
    fn malformed_result_after_plain_assistant_is_invalid() {
        let group = group_tool_exchanges(vec![
            LlmMessage::new(LlmRole::Assistant, "plain"),
            tool_message("missing", "orphan"),
        ]);
        assert_eq!(group.len(), 2);
        assert!(!group[1].messages.is_empty());
        assert!(!ToolExchangeGroup::is_complete(&group[1]));
    }

    #[test]
    fn complete_exchange_does_not_absorb_following_user() {
        let groups = group_tool_exchanges(vec![
            LlmMessage::new(LlmRole::User, "before"),
            LlmMessage {
                role: LlmRole::Assistant,
                text: String::new(),
                tool_calls: vec![tool_call("call-1")],
                tool_call_id: None,
                tool_name: None,
                images: Vec::new(),
            },
            tool_message("call-1", "result"),
            LlmMessage::new(LlmRole::User, "after"),
        ]);

        assert_eq!(groups.len(), 3);
        assert_eq!(groups[1].messages.len(), 2);
        assert!(ToolExchangeGroup::is_complete(&groups[1]));
        assert_eq!(groups[2].messages[0].text, "after");
    }

    #[test]
    fn incomplete_exchange_does_not_hide_following_user() {
        let messages = vec![
            LlmMessage {
                role: LlmRole::Assistant,
                text: String::new(),
                tool_calls: vec![tool_call("call-1")],
                tool_call_id: None,
                tool_name: None,
                images: Vec::new(),
            },
            LlmMessage::new(LlmRole::User, "next"),
        ];
        let groups = group_tool_exchanges(messages.clone());
        assert_eq!(groups.len(), 2);
        assert!(!ToolExchangeGroup::is_complete(&groups[0]));

        let packed = fit_prompt_llm(messages, 100, message_tokens);
        assert_eq!(packed.len(), 1);
        assert_eq!(packed[0].text, "next");
    }

    #[test]
    fn ordinary_messages_are_not_absorbed_into_tool_groups() {
        let groups = group_tool_exchanges(vec![
            LlmMessage::new(LlmRole::User, "user"),
            LlmMessage::new(LlmRole::Assistant, "assistant"),
            LlmMessage::new(LlmRole::User, "next"),
        ]);

        assert_eq!(groups.len(), 3);
        assert!(groups.iter().all(|group| group.messages.len() == 1));
        assert!(groups.iter().all(ToolExchangeGroup::is_complete));
    }
}
