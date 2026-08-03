//! Guaranteed lorebook injection at prompt composition.
//!
//! The `CCv3` contract is that a key-matched entry *must* be injected — it may
//! only fall when the book's `token_budget` is exceeded (in `priority` order).
//! The recall pipeline cannot honor that (entries compete with other memories
//! for scores and section survival), so this module evaluates entries directly
//! from the card and hands the prompt packer a separate, required section plus
//! optional depth-placed history messages.

#![expect(
    clippy::indexing_slicing,
    reason = "mind pipeline indexes into bounds-checked turn/selection buffers"
)]

use ene_config::{CharacterCardV3, LorebookEntry, expand_cbs_macros};
use ene_core::MemorySource;

use crate::lifecycle::HistoryEntry;
use crate::recall::RecallTurn;

use super::lorebook::{
    LOREBOOK_SOURCE_PREFIX, build_lorebook_scan_text, compile_lorebook_regex_cache,
    entry_keys_match_with_cache, stable_entry_id,
};
use super::lorebook_decorators::{
    ActivationContext, DecoratorRole, EntryDecorators, EntryPlacement, SemanticPosition,
    entry_decorators_accept,
};
use crate::context::estimate_tokens_language_aware;

/// Precompiled regex patterns keyed by `{entry_id}:{key}`.
type RegexCache = std::collections::HashMap<String, regex::Regex>;

/// Rendered lorebook content for the current turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LorebookInjection {
    /// Entries placed before the character description, in insertion order.
    pub before_char: Vec<String>,
    /// Entries placed after the character description, in insertion order.
    pub after_char: Vec<String>,
    /// Entries injected into the chat history at a message depth.
    pub messages: Vec<LorebookMessage>,
}

/// A depth-placed lorebook message (`@@depth` / `@@reverse_depth`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LorebookMessage {
    /// 1-based depth; counted from the most recent message when
    /// `from_oldest` is false and from the oldest message otherwise.
    pub depth: usize,
    /// Depth counting direction.
    pub from_oldest: bool,
    /// Message role (`@@role`, default `system`).
    pub role: DecoratorRole,
    /// Rendered content.
    pub content: String,
}

/// One selected entry before rendering, carrying the fields the placement and
/// budget passes need.
struct SelectedEntry<'a> {
    entry: &'a LorebookEntry,
    index: usize,
    decorators: EntryDecorators,
    content: String,
    placement: EntryPlacement,
}

/// Build the guaranteed-injection content for the current turn.
///
/// `history` is the session history *before* the current user message; the
/// last `scan_depth`-scaled window of it plus `user_input` forms the scan
/// text. Sticky decorator state is derived from `history`: an entry counts as
/// previously matched when a strictly earlier turn accepted it with that
/// turn's assistant count, so the state holds while the matching turn stays
/// within the retained history.
#[must_use]
pub fn build_lorebook_injection(
    card: &CharacterCardV3,
    user_name: &str,
    user_input: &str,
    history: &[HistoryEntry],
) -> LorebookInjection {
    let Some(book) = card.data.character_book.as_ref() else {
        return LorebookInjection::default();
    };
    if book.entries.is_empty() {
        return LorebookInjection::default();
    }

    let char_name = card.data.get_character_name();
    let scan_depth = book.scan_depth.unwrap_or(4);
    let regex_cache = compile_lorebook_regex_cache(book);
    let base_turns = history_turns(history);
    let base_scan = build_lorebook_scan_text(user_input, &base_turns, scan_depth);
    let ctx = ScanContext {
        base_turns: &base_turns,
        scan_depth,
        regex_cache: &regex_cache,
    };

    let mut selected: Vec<SelectedEntry<'_>> = Vec::new();
    for (index, entry) in book.entries.iter().enumerate().filter(|(_, e)| e.enabled) {
        if lorebook_entry_accepted(entry, index, user_input, &ctx, &base_scan) {
            selected.push(selected_entry(entry, index, char_name, user_name));
        }
    }

    if book.recursive_scanning.unwrap_or(false) && !selected.is_empty() {
        select_recursive(&mut selected, book, &ctx, char_name, user_name);
    }

    selected = apply_token_budget(selected, book.token_budget);

    // Sections and same-depth messages follow `insertion_order` (ties broken
    // by card position, keeping the ordering stable).
    selected.sort_by_key(|s| (s.entry.insertion_order, s.index));

    let mut injection = LorebookInjection::default();
    for item in selected {
        let before_char = match item.placement {
            EntryPlacement::MessageDepth(depth) => {
                injection.messages.push(LorebookMessage {
                    depth,
                    from_oldest: false,
                    role: item.decorators.role.unwrap_or(DecoratorRole::System),
                    content: item.content,
                });
                continue;
            }
            EntryPlacement::MessageDepthFromOldest(depth) => {
                injection.messages.push(LorebookMessage {
                    depth,
                    from_oldest: true,
                    role: item.decorators.role.unwrap_or(DecoratorRole::System),
                    content: item.content,
                });
                continue;
            }
            EntryPlacement::Semantic(SemanticPosition::BeforeDesc) => true,
            EntryPlacement::Semantic(SemanticPosition::AfterDesc) => false,
            // `personality` / `scenario` name sections Ene does not render, so
            // the decorator is ignored per spec and the `position` field
            // decides.
            EntryPlacement::Semantic(
                SemanticPosition::Personality | SemanticPosition::Scenario,
            )
            | EntryPlacement::SectionTop
            | EntryPlacement::Section => entry_position_before_char(item.entry),
        };
        let slot = if before_char {
            &mut injection.before_char
        } else {
            &mut injection.after_char
        };
        slot.push(item.content);
    }
    injection
}

fn selected_entry<'a>(
    entry: &'a LorebookEntry,
    index: usize,
    char_name: &str,
    user_name: &str,
) -> SelectedEntry<'a> {
    let (decorators, stripped) = EntryDecorators::parse(&entry.content);
    let placement = decorators.resolve_placement();
    SelectedEntry {
        entry,
        index,
        decorators,
        content: expand_cbs_macros(&stripped, char_name, user_name),
        placement,
    }
}

/// Selection state shared by the acceptance and sticky passes.
struct ScanContext<'a> {
    /// Turns before the current user input.
    base_turns: &'a [RecallTurn<'a>],
    /// Book-level scan depth.
    scan_depth: u32,
    /// Precompiled regex cache.
    regex_cache: &'a RegexCache,
}

/// Whether a lorebook entry belongs in this turn's prompt.
///
/// Constant entries and `@@activate` always match on keys; everything else
/// needs a key hit. All entries then pass the activation gates, evaluated
/// against the full-history assistant count and the history-derived sticky
/// state.
fn lorebook_entry_accepted(
    entry: &LorebookEntry,
    index: usize,
    user_input: &str,
    ctx: &ScanContext<'_>,
    base_scan: &str,
) -> bool {
    let (decorators, _) = EntryDecorators::parse(&entry.content);
    let entry_scan = entry_scan_text(
        user_input,
        ctx.base_turns,
        base_scan,
        &decorators,
        ctx.scan_depth,
    );
    entry_accepted_with_scan(entry, index, &decorators, &entry_scan, ctx)
}

/// Acceptance with an already-resolved scan text, shared by the recursive
/// scan pass (which evaluates keys against the extended window).
fn entry_accepted_with_scan(
    entry: &LorebookEntry,
    index: usize,
    decorators: &EntryDecorators,
    entry_scan: &str,
    ctx: &ScanContext<'_>,
) -> bool {
    let key_match = entry.constant.unwrap_or(false)
        || decorators.activate
        || entry_keys_match_with_cache(entry, index, entry_scan, Some(ctx.regex_cache));
    if !key_match {
        return false;
    }
    let previously_matched = sticky_decorators_active(decorators)
        && entry_previously_matched(entry, index, decorators, ctx);
    entry_decorators_accept(
        decorators,
        entry,
        entry_scan,
        &ActivationContext {
            assistant_message_count: assistant_message_count(ctx.base_turns),
            previously_matched,
            ..ActivationContext::default()
        },
    )
}

/// Whether the sticky gates can fire at all.
///
/// The sticky decorators are inert without a previous-match record; entries
/// without them skip the per-turn history scan entirely.
const fn sticky_decorators_active(decorators: &EntryDecorators) -> bool {
    decorators.keep_activate_after_match || decorators.dont_activate_after_match
}

/// Whether an earlier turn accepted `entry` (keys + filters + gates at that
/// turn's assistant count). A turn is "earlier" when it is strictly before
/// the last history entry, which is the one the current user input follows.
fn entry_previously_matched(
    entry: &LorebookEntry,
    index: usize,
    decorators: &EntryDecorators,
    ctx: &ScanContext<'_>,
) -> bool {
    let last = ctx.base_turns.len().saturating_sub(1);
    (0..last).any(|pos| {
        let prior = &ctx.base_turns[..pos];
        let scan = build_lorebook_scan_text(
            ctx.base_turns[pos].content,
            prior,
            decorators.effective_scan_depth(ctx.scan_depth),
        );
        let key_match = entry.constant.unwrap_or(false)
            || decorators.activate
            || entry_keys_match_with_cache(entry, index, &scan, Some(ctx.regex_cache));
        key_match
            && entry_decorators_accept(
                decorators,
                entry,
                &scan,
                &ActivationContext {
                    assistant_message_count: assistant_message_count(prior),
                    previously_matched: false,
                    ..ActivationContext::default()
                },
            )
    })
}

/// Extend the selection with entries whose keys match inside already-selected
/// content (`recursive_scanning`), per the spec's allow-list.
fn select_recursive<'a>(
    selected: &mut Vec<SelectedEntry<'a>>,
    book: &'a ene_config::Lorebook,
    ctx: &ScanContext<'_>,
    char_name: &str,
    user_name: &str,
) {
    let mut extended = String::new();
    for item in selected.iter() {
        if !extended.is_empty() {
            extended.push('\n');
        }
        extended.push_str(&item.content);
    }
    let selected_indices: std::collections::HashSet<usize> =
        selected.iter().map(|s| s.index).collect();
    let mut fresh: Vec<SelectedEntry<'_>> = Vec::new();
    for (index, entry) in book.entries.iter().enumerate().filter(|(_, e)| e.enabled) {
        if selected_indices.contains(&index) || entry.constant.unwrap_or(false) {
            continue;
        }
        let (decorators, _) = EntryDecorators::parse(&entry.content);
        let entry_scan =
            entry_scan_text("", ctx.base_turns, &extended, &decorators, ctx.scan_depth);
        if entry_accepted_with_scan(entry, index, &decorators, &entry_scan, ctx) {
            fresh.push(selected_entry(entry, index, char_name, user_name));
        }
    }
    selected.extend(fresh);
}

/// Per-entry scan text: the `@@scan_depth` override rebuilds the scan window,
/// entries without the decorator share the book-level (or recursively
/// extended) scan text.
fn entry_scan_text<'a>(
    user_input: &str,
    base_turns: &[RecallTurn<'_>],
    base_scan: &'a str,
    decorators: &EntryDecorators,
    book_scan_depth: u32,
) -> std::borrow::Cow<'a, str> {
    if decorators.scan_depth.is_none() {
        std::borrow::Cow::Borrowed(base_scan)
    } else {
        std::borrow::Cow::Owned(build_lorebook_scan_text(
            user_input,
            base_turns,
            decorators.effective_scan_depth(book_scan_depth),
        ))
    }
}

/// Assistant message count of the current history.
fn assistant_message_count(turns: &[RecallTurn<'_>]) -> u32 {
    turns
        .iter()
        .filter(|t| t.role == "assistant")
        .count()
        .try_into()
        .unwrap_or(u32::MAX)
}

/// Map history entries to recall turns (the scan-text API's input shape).
fn history_turns(history: &[HistoryEntry]) -> Vec<RecallTurn<'_>> {
    history
        .iter()
        .map(|entry| RecallTurn {
            role: entry.role_label(),
            content: entry.content.as_str(),
        })
        .collect()
}

/// Whether the entry's `position` field places it before the character
/// description; entries without the field go after it, where the previous
/// recall-merge path surfaced lorebook content.
fn entry_position_before_char(entry: &LorebookEntry) -> bool {
    entry.position.as_deref() == Some("before_char")
}

/// Drop entries until the rendered content fits the book's `token_budget`.
///
/// Per spec, `@@ignore_on_max_context` entries are trimmed first, then
/// ascending `priority` (falling back to `insertion_order`), then ascending
/// `insertion_order`. Constant entries are never dropped — they are the
/// always-injected contract of the card.
fn apply_token_budget(
    selected: Vec<SelectedEntry<'_>>,
    token_budget: Option<u32>,
) -> Vec<SelectedEntry<'_>> {
    let Some(budget) = token_budget else {
        return selected;
    };
    let budget = usize::try_from(budget).unwrap_or(usize::MAX);
    let total: usize = selected
        .iter()
        .map(|s| estimate_tokens_language_aware(&s.content))
        .sum();
    if total <= budget {
        return selected;
    }
    let mut drop_order: Vec<usize> = selected
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.entry.constant.unwrap_or(false))
        .map(|(idx, _)| idx)
        .collect();
    drop_order.sort_by_key(|&idx| {
        let s = &selected[idx];
        let priority = s.entry.priority.unwrap_or(s.entry.insertion_order);
        // Negated so `@@ignore_on_max_context` entries (true) sort first.
        (
            !s.decorators.ignore_on_max_context,
            priority,
            s.entry.insertion_order,
            s.index,
        )
    });
    let mut keep = vec![true; selected.len()];
    let mut dropped_tokens = 0usize;
    for idx in drop_order {
        if total.saturating_sub(dropped_tokens) <= budget {
            break;
        }
        let tokens = estimate_tokens_language_aware(&selected[idx].content);
        dropped_tokens = dropped_tokens.saturating_add(tokens);
        keep[idx] = false;
        tracing::debug!(
            component = "LorebookInjection",
            entry = %stable_entry_id(selected[idx].entry, selected[idx].index),
            reason = if selected[idx].decorators.ignore_on_max_context {
                "ignore_on_max_context"
            } else {
                "token_budget"
            },
            "Dropped lorebook entry over token budget"
        );
    }
    selected
        .into_iter()
        .enumerate()
        .filter_map(|(idx, s)| keep[idx].then_some(s))
        .collect()
}

/// Whether a memory row belongs to the lorebook index; the recall path filters
/// these out so lorebook content cannot double-inject through memory sections.
#[must_use]
pub fn is_lorebook_memory_row(source: MemorySource, source_ref: Option<&str>) -> bool {
    source == MemorySource::Ccv3
        && source_ref.is_some_and(|r| r.starts_with(LOREBOOK_SOURCE_PREFIX))
}

#[cfg(test)]
#[expect(
    clippy::default_trait_access,
    reason = "explicit Default for test fixture clarity"
)]
mod tests {
    use super::*;
    use ene_ai::Role;
    use ene_config::Lorebook;

    fn entry(keys: &[&str], content: &str, constant: bool, insertion_order: i32) -> LorebookEntry {
        LorebookEntry {
            keys: keys.iter().map(|k| (*k).to_string()).collect(),
            content: content.into(),
            extensions: Default::default(),
            enabled: true,
            insertion_order,
            case_sensitive: None,
            use_regex: false,
            constant: Some(constant),
            name: None,
            priority: None,
            id: None,
            comment: None,
            selective: None,
            secondary_keys: None,
            position: None,
        }
    }

    fn card_with(book: Lorebook) -> CharacterCardV3 {
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.character_book = Some(book);
        card
    }

    fn history_of(entries: &[(&str, Role)]) -> Vec<HistoryEntry> {
        entries
            .iter()
            .map(|(content, role)| HistoryEntry {
                role: *role,
                content: (*content).into(),
            })
            .collect()
    }

    #[test]
    fn japanese_key_matches_without_word_boundaries() {
        let book = Lorebook {
            entries: vec![entry(&["ドラゴン"], "竜の国は北にある。", false, 1)],
            ..Default::default()
        };
        let card = card_with(book);
        let injection = build_lorebook_injection(&card, "User", "ドラゴンが現れた", &[]);
        assert!(
            injection.after_char.iter().any(|c| c.contains("竜の国")),
            "a Japanese key must match inside a longer string"
        );
    }

    #[test]
    fn constant_entry_injected_without_keys() {
        let book = Lorebook {
            entries: vec![entry(&[], "The world is always sunny.", true, 1)],
            ..Default::default()
        };
        let card = card_with(book);
        let injection = build_lorebook_injection(&card, "User", "hi", &[]);
        assert_eq!(injection.after_char.len(), 1);
        assert!(injection.after_char[0].contains("always sunny"));
    }

    #[test]
    fn key_matched_entry_injected_and_unmatched_omitted() {
        let book = Lorebook {
            entries: vec![
                entry(&["dragon"], "Dragons guard the pass.", false, 1),
                entry(&["castle"], "The castle is empty.", false, 2),
            ],
            ..Default::default()
        };
        let card = card_with(book);
        let injection = build_lorebook_injection(&card, "User", "I met a dragon", &[]);
        assert!(injection.after_char.iter().any(|c| c.contains("pass")));
        assert!(
            injection.after_char.iter().all(|c| !c.contains("castle")),
            "an unmatched entry must not be injected"
        );
    }

    #[test]
    fn insertion_order_orders_section_entries() {
        let book = Lorebook {
            entries: vec![
                entry(&["b"], "second content", false, 20),
                entry(&["a"], "first content", false, 10),
            ],
            ..Default::default()
        };
        let card = card_with(book);
        let injection = build_lorebook_injection(&card, "User", "a b", &[]);
        let texts: Vec<&str> = injection.after_char.iter().map(String::as_str).collect();
        assert_eq!(texts, vec!["first content", "second content"]);
    }

    #[test]
    fn position_field_places_before_or_after_char() {
        let mut before = entry(&["dragon"], "before entry", false, 1);
        before.position = Some("before_char".into());
        let book = Lorebook {
            entries: vec![before, entry(&["castle"], "after entry", false, 2)],
            ..Default::default()
        };
        let card = card_with(book);
        let injection = build_lorebook_injection(&card, "User", "dragon castle", &[]);
        assert!(injection.before_char.iter().any(|c| c.contains("before")));
        assert!(injection.after_char.iter().any(|c| c.contains("after")));
    }

    #[test]
    fn depth_decorator_places_history_message() {
        let book = Lorebook {
            entries: vec![entry(
                &["dragon"],
                "@@depth 2\n@@role assistant\nThe dragon speaks.",
                false,
                1,
            )],
            ..Default::default()
        };
        let card = card_with(book);
        let history = history_of(&[("user one", Role::User), ("assistant one", Role::Assistant)]);
        let injection = build_lorebook_injection(&card, "User", "the dragon roars", &history);
        assert_eq!(injection.messages.len(), 1);
        assert_eq!(injection.messages[0].depth, 2);
        assert!(!injection.messages[0].from_oldest);
        assert_eq!(injection.messages[0].role, DecoratorRole::Assistant);
        assert!(injection.messages[0].content.contains("dragon speaks"));
        assert!(
            !injection.messages[0].content.contains("@@"),
            "decorator lines must never reach injected content"
        );
        assert!(injection.after_char.is_empty());
    }

    #[test]
    fn position_decorator_overrides_position_field() {
        let mut position = entry(&["dragon"], "@@position before_desc", false, 1);
        position.position = Some("after_char".into());
        let book = Lorebook {
            entries: vec![position],
            ..Default::default()
        };
        let card = card_with(book);
        let injection = build_lorebook_injection(&card, "User", "dragon", &[]);
        assert_eq!(injection.before_char.len(), 1);
        assert!(injection.after_char.is_empty());
    }

    #[test]
    fn token_budget_drops_lowest_priority_first() {
        let mut low = entry(&["a"], "low filler content", false, 10);
        low.priority = Some(1);
        let mut high = entry(&["b"], "keep", false, 20);
        high.priority = Some(9);
        let book = Lorebook {
            token_budget: Some(2),
            entries: vec![low, high],
            ..Default::default()
        };
        let card = card_with(book);
        let injection = build_lorebook_injection(&card, "User", "a b", &[]);
        assert!(injection.after_char.iter().any(|c| c.contains("keep")));
        assert!(
            injection
                .after_char
                .iter()
                .all(|c| !c.contains("low filler")),
            "the lowest-priority entry must be dropped first"
        );
    }

    #[test]
    fn token_budget_drops_ignore_on_max_context_first() {
        let mut flagged = entry(&["a"], "flagged", false, 10);
        flagged.priority = Some(9);
        flagged.content = "@@ignore_on_max_context\nflagged".into();
        let mut normal = entry(&["b"], "keep", false, 20);
        normal.priority = Some(1);
        let book = Lorebook {
            token_budget: Some(2),
            entries: vec![flagged, normal],
            ..Default::default()
        };
        let card = card_with(book);
        let injection = build_lorebook_injection(&card, "User", "a b", &[]);
        assert!(injection.after_char.iter().any(|c| c.contains("keep")));
        assert!(
            injection.after_char.iter().all(|c| !c.contains("flagged")),
            "an @@ignore_on_max_context entry must drop before any priority"
        );
    }

    #[test]
    fn constant_entries_survive_token_budget() {
        let book = Lorebook {
            token_budget: Some(2),
            entries: vec![
                entry(&[], "constant content always injected", true, 1),
                entry(&["a"], "keyed content", false, 2),
            ],
            ..Default::default()
        };
        let card = card_with(book);
        let injection = build_lorebook_injection(&card, "User", "a", &[]);
        assert!(
            injection
                .after_char
                .iter()
                .any(|c| c.contains("constant content")),
            "constant entries must survive the token budget"
        );
        assert!(
            !injection.after_char.iter().any(|c| c.contains("keyed")),
            "the keyed entry should be dropped under budget"
        );
    }

    #[test]
    fn keep_activate_after_match_sticks_from_earlier_turn() {
        let book = Lorebook {
            entries: vec![entry(
                &["sword"],
                "@@keep_activate_after_match\nThe sword remembers.",
                false,
                1,
            )],
            ..Default::default()
        };
        let card = card_with(book);
        // An earlier turn matched the key; the current turn does not.
        let history = history_of(&[
            ("my sword is here", Role::User),
            ("a reply", Role::Assistant),
        ]);
        let injection = build_lorebook_injection(&card, "User", "tell me about home", &history);
        assert!(
            injection
                .after_char
                .iter()
                .any(|c| c.contains("sword remembers")),
            "a once-matched entry with @@keep_activate_after_match stays injected"
        );
    }

    #[test]
    fn dont_activate_after_match_suppresses_after_first_match() {
        let book = Lorebook {
            entries: vec![entry(
                &["sword"],
                "@@dont_activate_after_match\nThe sword remembers.",
                false,
                1,
            )],
            ..Default::default()
        };
        let card = card_with(book);
        let history = history_of(&[
            ("my sword is here", Role::User),
            ("a reply", Role::Assistant),
        ]);
        let injection = build_lorebook_injection(&card, "User", "the sword again", &history);
        assert!(
            injection.after_char.is_empty(),
            "a @@dont_activate_after_match entry must not repeat"
        );
    }

    #[test]
    fn activate_only_after_counts_full_history_assistant_messages() {
        let book = Lorebook {
            entries: vec![entry(
                &["sword"],
                "@@activate_only_after 2\nThe sword remembers.",
                false,
                1,
            )],
            ..Default::default()
        };
        let card = card_with(book);
        let history = history_of(&[("sword", Role::User), ("one", Role::Assistant)]);
        let gated = build_lorebook_injection(&card, "User", "sword", &history);
        assert!(
            gated.after_char.is_empty(),
            "one assistant message must not satisfy @@activate_only_after 2"
        );
        let more = history_of(&[
            ("sword", Role::User),
            ("one", Role::Assistant),
            ("two", Role::User),
            ("three", Role::Assistant),
        ]);
        let open = build_lorebook_injection(&card, "User", "sword", &more);
        assert_eq!(open.after_char.len(), 1);
    }

    #[test]
    fn recursive_scanning_matches_against_selected_content() {
        let book = Lorebook {
            recursive_scanning: Some(true),
            entries: vec![
                entry(&["dragon"], "The dragon names the lost city.", false, 1),
                entry(&["lost city"], "Lost city lore.", false, 2),
            ],
            ..Default::default()
        };
        let card = card_with(book);
        let injection = build_lorebook_injection(&card, "User", "dragon", &[]);
        assert!(
            injection
                .after_char
                .iter()
                .any(|c| c.contains("Lost city lore")),
            "recursive scanning should match keys inside selected content"
        );
    }

    #[test]
    fn cbs_macros_expanded_in_injected_content() {
        let book = Lorebook {
            entries: vec![entry(&["dragon"], "{{char}} knows {{user}}.", false, 1)],
            ..Default::default()
        };
        let card = card_with(book);
        let injection = build_lorebook_injection(&card, "Alice", "dragon", &[]);
        assert!(injection.after_char[0].contains("Ene knows Alice."));
    }

    #[test]
    fn is_lorebook_memory_row_recognizes_indexed_rows() {
        assert!(is_lorebook_memory_row(
            MemorySource::Ccv3,
            Some("ccv3:lorebook:dragon")
        ));
        assert!(!is_lorebook_memory_row(
            MemorySource::Ccv3,
            Some("ccv3:style:greeting")
        ));
        assert!(!is_lorebook_memory_row(MemorySource::Conversation, None));
    }
}
