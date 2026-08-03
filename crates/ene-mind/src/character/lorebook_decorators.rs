//! `CCv3` lorebook `@@` decorator parsing and evaluation.
//!
//! Decorators extend a lorebook entry without adding card fields: they live in
//! the entry's `content`, start with `@@`, and are stripped before the content
//! is injected into the prompt. Behavior follows
//! <https://github.com/kwaroran/character-card-spec-v3> (`SPEC_V3.md`, "Decorators").

use ene_config::LorebookEntry;

use super::lorebook::stable_entry_id;

/// Role for a decorator-injected message (`@@role`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoratorRole {
    /// `assistant`
    Assistant,
    /// `system`
    System,
    /// `user`
    User,
}

/// Semantic insertion position from `@@position`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticPosition {
    /// `after_desc` — after the character description.
    AfterDesc,
    /// `before_desc` — before the character description.
    BeforeDesc,
    /// `personality` — in the personality section.
    Personality,
    /// `scenario` — in the scenario section.
    Scenario,
}

/// Where a matched entry lands in the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryPlacement {
    /// `@@depth N` — insert as a message at depth `N` counted from the most
    /// recent message (1-based); depths beyond the history length land at the
    /// front (spec: "before the oldest message").
    MessageDepth(usize),
    /// `@@reverse_depth N` — insert as a message at depth `N` counted from the
    /// oldest message (1-based); depths beyond the history length land at the
    /// front.
    MessageDepthFromOldest(usize),
    /// `@@position` semantic section.
    Semantic(SemanticPosition),
    /// Depth-0 fallback slot: Ene does not support prefill, so the spec's
    /// high-priority-position fallback applies (front of the lorebook section).
    SectionTop,
    /// Default lorebook section slot (ordered by the `position` field and
    /// `insertion_order`).
    Section,
}

/// Conversation state the activation decorators evaluate against.
#[derive(Debug, Clone, Copy, Default)]
pub struct ActivationContext {
    /// Number of assistant (character) messages in the chat log, counted over
    /// the full history — never over a recent-turn window.
    pub assistant_message_count: u32,
    /// Whether this entry matched on an earlier turn (sticky decorators).
    pub previously_matched: bool,
    /// Index of the greeting this session started with (`0` = `first_mes`,
    /// `i+1` = `alternate_greetings[i]`); `None` when no greeting was chosen.
    /// Drives `@@is_greeting` (`CCv3` `SPEC_V3.md`); `None` means the
    /// decorator cannot be checked and is ignored.
    pub greeting_index: Option<u32>,
}

/// Parsed decorators of a single lorebook entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntryDecorators {
    /// `@@activate_only_after N` — only match once the assistant message count
    /// reaches `N`.
    pub activate_only_after: Option<u32>,
    /// `@@activate_only_every N` — only match when the assistant message count
    /// divides `N` evenly.
    pub activate_only_every: Option<u32>,
    /// `@@keep_activate_after_match` — once matched, always match.
    pub keep_activate_after_match: bool,
    /// `@@dont_activate_after_match` — once matched, never match again.
    pub dont_activate_after_match: bool,
    /// `@@activate` — force a match regardless of keys.
    pub activate: bool,
    /// `@@dont_activate` — never match (overridden by `activate`).
    pub dont_activate: bool,
    /// `@@depth N` — message insertion depth from the most recent message.
    pub depth: Option<i64>,
    /// `@@reverse_depth N` — message insertion depth from the oldest message.
    ///
    /// The spec says chat contexts should ignore `@@reverse_depth`
    /// (`SPEC_V3.md`, "Decorators"); Ene follows `SillyTavern`'s reading of it
    /// (count from the oldest) so cards relying on ST behavior keep working.
    pub reverse_depth: Option<i64>,
    /// `@@position` semantic insertion position.
    pub position: Option<SemanticPosition>,
    /// `@@scan_depth N` — per-entry scan depth override.
    pub scan_depth: Option<u32>,
    /// `@@is_greeting N` — only match when the session's active greeting
    /// index equals `N`. Resolved only while a greeting is active; without
    /// one, checking is not possible, so the decorator is ignored and its
    /// `@@@` fallback chain applies (spec).
    pub is_greeting: Option<u32>,
    /// `@@additional_keys` — extra keys, at least one must match (accumulates
    /// across multiple decorator lines, per spec).
    pub additional_keys: Vec<String>,
    /// `@@exclude_keys` — suppress when any key matches.
    pub exclude_keys: Vec<String>,
    /// `@@role` — role of the injected message.
    pub role: Option<DecoratorRole>,
    /// `@@ignore_on_max_context` — drop first when the prompt exceeds the
    /// lorebook token budget. Inert on cards without a `token_budget`.
    pub ignore_on_max_context: bool,
}

impl EntryDecorators {
    /// Parse the decorator lines of `content` and return them together with
    /// the content stripped of every decorator line and trimmed.
    ///
    /// A line starting with `@@` is a decorator; a line starting with `@@@` is
    /// a fallback for the preceding decorator (chains of any length are
    /// supported). Each `@@` line plus its immediately following `@@@` lines
    /// form one group; the first decorator in the chain that is recognized,
    /// valid, **and honored in Ene's context** wins. Decorators Ene ignores in
    /// its chat context (see [`is_context_ignored`]) never resolve, so their
    /// fallbacks are consulted per the spec's fallback rule. Unknown groups are
    /// ignored (but still stripped). Only the first decorator of a given name
    /// counts — except `@@additional_keys`, which the spec explicitly allows
    /// multiple times.
    #[must_use]
    pub fn parse(content: &str) -> (Self, String) {
        Self::parse_with_greeting(content, None)
    }

    /// [`Self::parse`] with the session's active greeting index.
    ///
    /// `@@is_greeting N` can only be honored while a greeting is active; with
    /// `greeting_index: None` the decorator is ignored per spec ("checking the
    /// active greeting is not possible") and its `@@@` fallback chain applies.
    #[must_use]
    pub fn parse_with_greeting(content: &str, greeting_index: Option<u32>) -> (Self, String) {
        let mut decorators = Self::default();
        let mut seen_names = Vec::new();
        let mut chain = ChainGroup::default();
        let mut kept = Vec::new();

        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("@@") {
                if let Some(fallback) = rest.strip_prefix('@') {
                    // Fallback line: appends to the current group (a group
                    // without a primary can never resolve and is ignored).
                    let (name, value) = split_name_value(fallback);
                    chain.fallbacks.push((name, value));
                } else {
                    // New decorator group; resolve the previous group first.
                    chain.resolve(&mut decorators, &mut seen_names, greeting_index);
                    let (name, value) = split_name_value(rest);
                    chain.primary = Some((name, value));
                }
            } else {
                chain.resolve(&mut decorators, &mut seen_names, greeting_index);
                kept.push(line);
            }
        }
        chain.resolve(&mut decorators, &mut seen_names, greeting_index);

        (decorators, kept.join("\n").trim().to_string())
    }

    /// Whether the activation decorators pass for `ctx`. All conditions are
    /// `AND`ed; `@@activate` short-circuits to `true` (the spec's "in any case").
    #[must_use]
    pub fn activation_passes(&self, ctx: &ActivationContext) -> bool {
        if self.activate {
            return true;
        }
        if self.dont_activate {
            return false;
        }
        if self.keep_activate_after_match && ctx.previously_matched {
            return true;
        }
        if self.dont_activate_after_match && ctx.previously_matched {
            return false;
        }
        if let Some(after) = self.activate_only_after
            && ctx.assistant_message_count < after
        {
            return false;
        }
        if let Some(every) = self.activate_only_every
            && every > 1
            && !ctx.assistant_message_count.is_multiple_of(every)
        {
            return false;
        }
        if let Some(index) = self.is_greeting
            && ctx.greeting_index != Some(index)
        {
            return false;
        }
        true
    }

    /// Effective scan depth: the `@@scan_depth` override or `book_default`.
    #[must_use]
    pub fn effective_scan_depth(&self, book_default: u32) -> u32 {
        self.scan_depth.unwrap_or(book_default)
    }

    /// Resolve the placement decorators. `@@position` wins over depth; depth-0
    /// (prefill) and reverse depth-0 fall back to [`EntryPlacement::SectionTop`]
    /// because Ene does not support prefill messages.
    #[must_use]
    pub fn resolve_placement(&self) -> EntryPlacement {
        if let Some(position) = self.position {
            return EntryPlacement::Semantic(position);
        }
        if let Some(depth) = self.depth {
            return if depth < 1 {
                EntryPlacement::SectionTop
            } else {
                EntryPlacement::MessageDepth(usize::try_from(depth).unwrap_or(usize::MAX))
            };
        }
        if let Some(depth) = self.reverse_depth {
            return if depth < 1 {
                EntryPlacement::SectionTop
            } else {
                EntryPlacement::MessageDepthFromOldest(usize::try_from(depth).unwrap_or(usize::MAX))
            };
        }
        EntryPlacement::Section
    }
}

/// One decorator group: the primary `@@name` line plus its `@@@name` fallback
/// lines. Resolution tries the primary first, then each fallback top-to-bottom;
/// a group without a primary never resolves.
#[derive(Default)]
struct ChainGroup {
    primary: Option<(String, Option<String>)>,
    fallbacks: Vec<(String, Option<String>)>,
}

impl ChainGroup {
    /// Resolve the group into `decorators` and reset it. The primary wins
    /// when recognized and honored; fallbacks are only consulted when the
    /// primary is unknown, invalid, or ignored in Ene's context (spec: "if the
    /// decorator is not recognized ... or decorator is ignored, check if the
    /// fallback decorator is present"). A group without a primary (`@@@` with
    /// no preceding `@@`) never resolves.
    fn resolve(
        &mut self,
        decorators: &mut EntryDecorators,
        seen_names: &mut Vec<String>,
        greeting_index: Option<u32>,
    ) {
        let Some((name, value)) = self.primary.take() else {
            // `@@@` lines without a preceding `@@` are not decorators.
            self.fallbacks.clear();
            return;
        };
        if seen_names.contains(&name) {
            // A repeated decorator name is ignored — except
            // `additional_keys`, which the spec allows multiple times.
            if name == "additional_keys"
                && let Some(values) = split_values(value.as_deref())
            {
                decorators.additional_keys.extend(values);
            }
            self.fallbacks.clear();
            return;
        }
        seen_names.push(name.clone());
        if let Some(resolved) = resolve_decorator(&name, value.as_deref(), greeting_index) {
            apply_resolved(decorators, resolved);
            self.fallbacks.clear();
            return;
        }
        for (fallback_name, fallback_value) in std::mem::take(&mut self.fallbacks) {
            if seen_names.contains(&fallback_name) {
                // A repeated decorator name is ignored — except
                // `additional_keys`, which the spec allows multiple times.
                if fallback_name == "additional_keys"
                    && let Some(values) = split_values(fallback_value.as_deref())
                {
                    decorators.additional_keys.extend(values);
                }
                continue;
            }
            seen_names.push(fallback_name.clone());
            if let Some(resolved) =
                resolve_decorator(&fallback_name, fallback_value.as_deref(), greeting_index)
            {
                apply_resolved(decorators, resolved);
                return;
            }
        }
    }
}

/// A single resolved decorator before it is folded into
/// [`EntryDecorators`]; keeps resolution and application separate so invalid
/// values fall through to fallback lines.
enum ResolvedDecorator {
    ActivateOnlyAfter(u32),
    ActivateOnlyEvery(u32),
    KeepActivateAfterMatch,
    DontActivateAfterMatch,
    Activate,
    DontActivate,
    Depth(i64),
    ReverseDepth(i64),
    Position(SemanticPosition),
    ScanDepth(u32),
    IsGreeting(u32),
    AdditionalKeys(Vec<String>),
    ExcludeKeys(Vec<String>),
    Role(DecoratorRole),
    IgnoreOnMaxContext,
}

/// Split a decorator line into its name and optional value.
fn split_name_value(rest: &str) -> (String, Option<String>) {
    let mut parts = rest.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or_default().trim().to_string();
    let value = parts.next().map(str::trim).map(str::to_string);
    (name, value.filter(|v| !v.is_empty()))
}

/// Split a comma-separated decorator value into trimmed pieces.
fn split_values(value: Option<&str>) -> Option<Vec<String>> {
    value.map(|v| {
        v.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    })
}

/// Decorators the spec defines but Ene's context forces it to ignore, so they
/// never resolve and their `@@@` fallbacks are consulted instead:
/// `instruct_depth` / `instruct_scan_depth` (Ene is always chat-based),
/// `is_user_icon` (no user-icon feature), and `disable_ui_prompt`
/// (deliberately not honored — cards must not be able to disable Ene's
/// expression output contract).
fn is_context_ignored(name: &str) -> bool {
    matches!(
        name,
        "instruct_depth" | "instruct_scan_depth" | "is_user_icon" | "disable_ui_prompt"
    )
}

/// Resolve one decorator chain element, or `None` when the name is unknown,
/// ignored in Ene's context (or uncheckable for `greeting_index`), or the
/// value is invalid (the spec's "ignore the decorator" case).
fn resolve_decorator(
    name: &str,
    value: Option<&str>,
    greeting_index: Option<u32>,
) -> Option<ResolvedDecorator> {
    if is_context_ignored(name) {
        return None;
    }
    match name {
        "activate_only_after" => {
            let parsed = value?.parse::<u32>().ok()?;
            Some(ResolvedDecorator::ActivateOnlyAfter(parsed))
        }
        "activate_only_every" => {
            let parsed = value?.parse::<u32>().ok()?;
            Some(ResolvedDecorator::ActivateOnlyEvery(parsed))
        }
        "keep_activate_after_match" if value.is_none() => {
            Some(ResolvedDecorator::KeepActivateAfterMatch)
        }
        "dont_activate_after_match" if value.is_none() => {
            Some(ResolvedDecorator::DontActivateAfterMatch)
        }
        "activate" if value.is_none() => Some(ResolvedDecorator::Activate),
        "dont_activate" if value.is_none() => Some(ResolvedDecorator::DontActivate),
        "depth" => Some(ResolvedDecorator::Depth(value?.parse::<i64>().ok()?)),
        "reverse_depth" => Some(ResolvedDecorator::ReverseDepth(value?.parse::<i64>().ok()?)),
        "position" => {
            let position = match value? {
                "after_desc" => SemanticPosition::AfterDesc,
                "before_desc" => SemanticPosition::BeforeDesc,
                "personality" => SemanticPosition::Personality,
                "scenario" => SemanticPosition::Scenario,
                _ => return None,
            };
            Some(ResolvedDecorator::Position(position))
        }
        "scan_depth" => Some(ResolvedDecorator::ScanDepth(value?.parse::<u32>().ok()?)),
        // Without an active greeting the index cannot be checked, so the
        // decorator is ignored and the `@@@` fallback chain applies.
        "is_greeting" if greeting_index.is_some() => {
            Some(ResolvedDecorator::IsGreeting(value?.parse::<u32>().ok()?))
        }
        "additional_keys" => Some(ResolvedDecorator::AdditionalKeys(split_values(value)?)),
        "exclude_keys" => Some(ResolvedDecorator::ExcludeKeys(split_values(value)?)),
        "role" => {
            let role = match value? {
                "assistant" => DecoratorRole::Assistant,
                "system" => DecoratorRole::System,
                "user" => DecoratorRole::User,
                _ => return None,
            };
            Some(ResolvedDecorator::Role(role))
        }
        "ignore_on_max_context" if value.is_none() => Some(ResolvedDecorator::IgnoreOnMaxContext),
        _ => None,
    }
}

/// Fold a resolved decorator into the parsed state.
fn apply_resolved(decorators: &mut EntryDecorators, resolved: ResolvedDecorator) {
    match resolved {
        ResolvedDecorator::ActivateOnlyAfter(value) => decorators.activate_only_after = Some(value),
        ResolvedDecorator::ActivateOnlyEvery(value) => decorators.activate_only_every = Some(value),
        ResolvedDecorator::KeepActivateAfterMatch => decorators.keep_activate_after_match = true,
        ResolvedDecorator::DontActivateAfterMatch => {
            decorators.dont_activate_after_match = true;
        }
        ResolvedDecorator::Activate => decorators.activate = true,
        ResolvedDecorator::DontActivate => decorators.dont_activate = true,
        ResolvedDecorator::Depth(value) => decorators.depth = Some(value),
        ResolvedDecorator::ReverseDepth(value) => decorators.reverse_depth = Some(value),
        ResolvedDecorator::Position(value) => decorators.position = Some(value),
        ResolvedDecorator::ScanDepth(value) => decorators.scan_depth = Some(value),
        ResolvedDecorator::IsGreeting(value) => decorators.is_greeting = Some(value),
        ResolvedDecorator::AdditionalKeys(values) => decorators.additional_keys.extend(values),
        ResolvedDecorator::ExcludeKeys(values) => decorators.exclude_keys.extend(values),
        ResolvedDecorator::Role(value) => decorators.role = Some(value),
        ResolvedDecorator::IgnoreOnMaxContext => decorators.ignore_on_max_context = true,
    }
}

/// Whether the `@@additional_keys` / `@@exclude_keys` filters pass for
/// `scan_text`. Key matching follows the entry's `use_regex` / `case_sensitive`
/// settings; per spec, `@@exclude_keys` is ignored when `use_regex` is set.
/// Regex patterns are looked up in `regex_cache` first (keyed like the entry
/// keys) and compiled on demand only on a cache miss.
#[expect(
    clippy::implicit_hasher,
    reason = "HashMap key type is fixed to String in lorebook API"
)]
#[must_use]
pub fn decorator_filters_pass(
    decorators: &EntryDecorators,
    entry: &LorebookEntry,
    entry_index: usize,
    scan_text: &str,
    regex_cache: Option<&std::collections::HashMap<String, regex::Regex>>,
) -> bool {
    let use_regex = entry.use_regex;
    let case_sensitive = entry.case_sensitive.unwrap_or(false);
    let entry_id = stable_entry_id(entry, entry_index);

    let key_matches = |key: &str| -> bool {
        if use_regex {
            let cache_key = format!("{entry_id}:{key}");
            if let Some(cache) = regex_cache
                && let Some(re) = cache.get(&cache_key)
            {
                return re.is_match(scan_text);
            }
            regex::RegexBuilder::new(key)
                .case_insensitive(!case_sensitive)
                .build()
                .is_ok_and(|re| re.is_match(scan_text))
        } else if case_sensitive {
            scan_text.contains(key)
        } else {
            let haystack = scan_text.to_lowercase();
            haystack.contains(&key.to_lowercase())
        }
    };

    if !use_regex
        && decorators
            .exclude_keys
            .iter()
            .any(|key| !key.is_empty() && key_matches(key))
    {
        return false;
    }

    if !decorators.additional_keys.is_empty()
        && !decorators
            .additional_keys
            .iter()
            .any(|key| !key.is_empty() && key_matches(key))
    {
        return false;
    }

    true
}

/// Whether a lorebook entry's decorator conditions accept the current turn.
///
/// Combines the activation gates and the additional/exclude key filters.
/// `previously_matched` feeds the sticky decorators.
#[expect(
    clippy::implicit_hasher,
    reason = "HashMap key type is fixed to String in lorebook API"
)]
#[must_use]
pub fn entry_decorators_accept(
    decorators: &EntryDecorators,
    entry: &LorebookEntry,
    entry_index: usize,
    scan_text: &str,
    regex_cache: Option<&std::collections::HashMap<String, regex::Regex>>,
    ctx: &ActivationContext,
) -> bool {
    if decorators.activate {
        return true;
    }
    decorator_filters_pass(decorators, entry, entry_index, scan_text, regex_cache)
        && decorators.activation_passes(ctx)
}

#[cfg(test)]
#[expect(
    clippy::default_trait_access,
    reason = "explicit Default for test fixture clarity"
)]
mod tests {
    use super::*;
    use ene_config::LorebookEntry;

    fn sample_entry() -> LorebookEntry {
        LorebookEntry {
            keys: vec!["dragon".into()],
            content: "Lore.".into(),
            extensions: Default::default(),
            enabled: true,
            insertion_order: 1,
            case_sensitive: None,
            use_regex: false,
            constant: None,
            name: None,
            priority: None,
            id: None,
            comment: None,
            selective: None,
            secondary_keys: None,
            position: None,
        }
    }

    fn parse(content: &str) -> (EntryDecorators, String) {
        EntryDecorators::parse(content)
    }

    #[test]
    fn strips_decorators_and_keeps_body() {
        let (decorators, body) = parse("@@depth 2\n@@role system\n\nThe castle looms.\n");
        assert_eq!(decorators.depth, Some(2));
        assert_eq!(decorators.role, Some(DecoratorRole::System));
        assert_eq!(body, "The castle looms.");
    }

    #[test]
    fn decorator_lines_mid_content_are_stripped() {
        let (decorators, body) = parse("First line.\n@@activate\nSecond line.");
        assert!(decorators.activate);
        assert_eq!(body, "First line.\nSecond line.");
    }

    #[test]
    fn unknown_decorator_is_stripped_and_ignored() {
        let (decorators, body) = parse("@@risu_only_decorator 4\nHello");
        assert_eq!(decorators, EntryDecorators::default());
        assert_eq!(body, "Hello");
    }

    #[test]
    fn fallback_chain_uses_first_recognized() {
        let (decorators, body) =
            parse("@@risu_only_decorator 4\n@@@agn_only 4\n@@@scan_depth 3\nHello");
        assert_eq!(decorators.scan_depth, Some(3));
        assert_eq!(body, "Hello");
    }

    #[test]
    fn recognized_primary_beats_fallback() {
        let (decorators, _) = parse("@@depth 2\n@@@scan_depth 9\nHello");
        assert_eq!(decorators.depth, Some(2));
        assert_eq!(decorators.scan_depth, None);
    }

    #[test]
    fn invalid_value_falls_through_to_fallback() {
        let (decorators, _) = parse("@@depth not-a-number\n@@@scan_depth 4\nHello");
        assert_eq!(decorators.depth, None);
        assert_eq!(decorators.scan_depth, Some(4));
    }

    #[test]
    fn repeated_name_first_wins() {
        let (decorators, _) = parse("@@depth 2\n@@depth 9\nHello");
        assert_eq!(decorators.depth, Some(2));
    }

    #[test]
    fn additional_keys_accumulate() {
        let (decorators, _) =
            parse("@@additional_keys sword,shield\n@@additional_keys crown\nHello");
        assert_eq!(decorators.additional_keys, vec!["sword", "shield", "crown"]);
    }

    #[test]
    fn lone_fallback_without_primary_is_ignored() {
        let (decorators, body) = parse("@@@scan_depth 2\nHello");
        assert_eq!(decorators.scan_depth, None);
        assert_eq!(body, "Hello");
    }

    #[test]
    fn value_less_decorators_reject_values() {
        let (decorators, _) = parse("@@activate 4\nHello");
        assert!(!decorators.activate);
    }

    #[test]
    fn no_value_decorator_without_value_parses() {
        let (decorators, _) =
            parse("@@activate\n@@keep_activate_after_match\n@@ignore_on_max_context\nHello");
        assert!(decorators.activate);
        assert!(decorators.keep_activate_after_match);
        assert!(decorators.ignore_on_max_context);
    }

    #[test]
    fn activate_short_circuits_activation_gates() {
        let decorators = EntryDecorators {
            activate: true,
            dont_activate: true,
            activate_only_after: Some(99),
            ..EntryDecorators::default()
        };
        let ctx = ActivationContext {
            assistant_message_count: 0,
            ..ActivationContext::default()
        };
        assert!(decorators.activation_passes(&ctx));
    }

    #[test]
    fn dont_activate_suppresses_unless_activate_present() {
        let off = EntryDecorators {
            dont_activate: true,
            ..EntryDecorators::default()
        };
        assert!(!off.activation_passes(&ActivationContext::default()));
    }

    #[test]
    fn activate_only_after_gates_on_assistant_count() {
        let decorators = EntryDecorators {
            activate_only_after: Some(3),
            ..EntryDecorators::default()
        };
        let ctx = |count: u32| ActivationContext {
            assistant_message_count: count,
            ..ActivationContext::default()
        };
        assert!(!decorators.activation_passes(&ctx(2)));
        assert!(decorators.activation_passes(&ctx(3)));
        // N = 0 disables the gate.
        let zero = EntryDecorators {
            activate_only_after: Some(0),
            ..EntryDecorators::default()
        };
        assert!(zero.activation_passes(&ctx(0)));
    }

    #[test]
    fn activate_only_every_gates_on_remainder() {
        let decorators = EntryDecorators {
            activate_only_every: Some(3),
            ..EntryDecorators::default()
        };
        let ctx = |count: u32| ActivationContext {
            assistant_message_count: count,
            ..ActivationContext::default()
        };
        assert!(decorators.activation_passes(&ctx(3)));
        assert!(decorators.activation_passes(&ctx(6)));
        assert!(!decorators.activation_passes(&ctx(4)));
        assert!(!decorators.activation_passes(&ctx(5)));
        // Zero assistant messages divides evenly, so the gate passes.
        assert!(decorators.activation_passes(&ctx(0)));
        // N = 1 disables the gate.
        let every_turn = EntryDecorators {
            activate_only_every: Some(1),
            ..EntryDecorators::default()
        };
        assert!(every_turn.activation_passes(&ctx(0)));
    }

    #[test]
    fn keep_activate_after_match_is_sticky() {
        let decorators = EntryDecorators {
            keep_activate_after_match: true,
            ..EntryDecorators::default()
        };
        let ctx = |matched: bool| ActivationContext {
            previously_matched: matched,
            ..ActivationContext::default()
        };
        assert!(decorators.activation_passes(&ctx(true)));
        // The decorator extends matches, it does not gate unmatching turns.
        assert!(decorators.activation_passes(&ctx(false)));
    }

    #[test]
    fn dont_activate_after_match_suppresses_once_matched() {
        let decorators = EntryDecorators {
            dont_activate_after_match: true,
            ..EntryDecorators::default()
        };
        let ctx = |matched: bool| ActivationContext {
            previously_matched: matched,
            ..ActivationContext::default()
        };
        assert!(!decorators.activation_passes(&ctx(true)));
        assert!(decorators.activation_passes(&ctx(false)));
    }

    #[test]
    fn additional_keys_require_one_match() {
        let entry = sample_entry();
        let decorators = EntryDecorators {
            additional_keys: vec!["sword".into(), "shield".into()],
            ..EntryDecorators::default()
        };
        assert!(decorator_filters_pass(
            &decorators,
            &entry,
            0,
            "a sword lies here",
            None
        ));
        assert!(!decorator_filters_pass(
            &decorators,
            &entry,
            0,
            "only a dagger",
            None
        ));
    }

    #[test]
    fn exclude_keys_suppress_on_match() {
        let entry = sample_entry();
        let decorators = EntryDecorators {
            exclude_keys: vec!["rusty".into()],
            ..EntryDecorators::default()
        };
        assert!(!decorator_filters_pass(
            &decorators,
            &entry,
            0,
            "a rusty sword",
            None
        ));
        assert!(decorator_filters_pass(
            &decorators,
            &entry,
            0,
            "a shiny sword",
            None
        ));
    }

    #[test]
    fn additional_keys_follow_use_regex() {
        let mut entry = sample_entry();
        entry.use_regex = true;
        let decorators = EntryDecorators {
            additional_keys: vec!["sword\\d+".into()],
            ..EntryDecorators::default()
        };
        assert!(decorator_filters_pass(
            &decorators,
            &entry,
            0,
            "sword42 here",
            None
        ));
        assert!(!decorator_filters_pass(
            &decorators,
            &entry,
            0,
            "sword here",
            None
        ));
        // Invalid regex means no match.
        let broken = EntryDecorators {
            additional_keys: vec!["(unclosed".into()],
            ..EntryDecorators::default()
        };
        assert!(!decorator_filters_pass(
            &broken, &entry, 0, "anything", None
        ));
    }

    #[test]
    fn exclude_keys_ignored_when_use_regex() {
        let mut entry = sample_entry();
        entry.use_regex = true;
        let decorators = EntryDecorators {
            exclude_keys: vec!["rusty".into()],
            ..EntryDecorators::default()
        };
        assert!(decorator_filters_pass(
            &decorators,
            &entry,
            0,
            "rusty",
            None
        ));
    }

    #[test]
    fn additional_keys_follow_case_sensitive() {
        let mut entry = sample_entry();
        entry.case_sensitive = Some(true);
        let decorators = EntryDecorators {
            additional_keys: vec!["Sword".into()],
            ..EntryDecorators::default()
        };
        assert!(decorator_filters_pass(
            &decorators,
            &entry,
            0,
            "a Sword lies here",
            None
        ));
        assert!(!decorator_filters_pass(
            &decorators,
            &entry,
            0,
            "a sword lies here",
            None
        ));
    }

    #[test]
    fn exclude_keys_follow_case_sensitive() {
        let mut entry = sample_entry();
        entry.case_sensitive = Some(true);
        let decorators = EntryDecorators {
            exclude_keys: vec!["Rusty".into()],
            ..EntryDecorators::default()
        };
        assert!(!decorator_filters_pass(
            &decorators,
            &entry,
            0,
            "a Rusty sword",
            None
        ));
        assert!(decorator_filters_pass(
            &decorators,
            &entry,
            0,
            "a rusty sword",
            None
        ));
    }

    #[test]
    fn activate_only_every_zero_disables_gate() {
        let decorators = EntryDecorators {
            activate_only_every: Some(0),
            ..EntryDecorators::default()
        };
        for count in [0, 5, 6] {
            let ctx = ActivationContext {
                assistant_message_count: count,
                ..ActivationContext::default()
            };
            assert!(
                decorators.activation_passes(&ctx),
                "every 0 must disable the gate at count {count}"
            );
        }
    }

    #[test]
    fn depth_placement_resolution() {
        let decorators = EntryDecorators {
            depth: Some(2),
            ..EntryDecorators::default()
        };
        assert_eq!(
            decorators.resolve_placement(),
            EntryPlacement::MessageDepth(2)
        );
    }

    #[test]
    fn depth_below_one_falls_back_to_section_top() {
        for depth in [0, -3] {
            let decorators = EntryDecorators {
                depth: Some(depth),
                ..EntryDecorators::default()
            };
            assert_eq!(
                decorators.resolve_placement(),
                EntryPlacement::SectionTop,
                "depth {depth}"
            );
        }
    }

    #[test]
    fn reverse_depth_placement_resolution() {
        let decorators = EntryDecorators {
            reverse_depth: Some(3),
            ..EntryDecorators::default()
        };
        assert_eq!(
            decorators.resolve_placement(),
            EntryPlacement::MessageDepthFromOldest(3)
        );
        let zero = EntryDecorators {
            reverse_depth: Some(0),
            ..EntryDecorators::default()
        };
        assert_eq!(zero.resolve_placement(), EntryPlacement::SectionTop);
    }

    #[test]
    fn position_wins_over_depth() {
        let decorators = EntryDecorators {
            depth: Some(2),
            position: Some(SemanticPosition::Scenario),
            ..EntryDecorators::default()
        };
        assert_eq!(
            decorators.resolve_placement(),
            EntryPlacement::Semantic(SemanticPosition::Scenario)
        );
    }

    #[test]
    fn default_placement_is_section() {
        assert_eq!(
            EntryDecorators::default().resolve_placement(),
            EntryPlacement::Section
        );
    }

    #[test]
    fn scan_depth_override_and_default() {
        let override_decorators = EntryDecorators {
            scan_depth: Some(7),
            ..EntryDecorators::default()
        };
        assert_eq!(override_decorators.effective_scan_depth(4), 7);
        assert_eq!(EntryDecorators::default().effective_scan_depth(4), 4);
    }

    #[test]
    fn disable_ui_prompt_is_ignored_and_falls_back() {
        // Ene does not support disabling its UI prompts (cards must not be
        // able to drop the expression output contract), so the decorator is
        // ignored and its `@@@` fallback is consulted per spec.
        let (decorators, _) =
            parse("@@disable_ui_prompt post_history_instructions\n@@@depth 2\nBody");
        assert_eq!(decorators.depth, Some(2));
    }

    #[test]
    fn user_icon_decorator_is_ignored_and_falls_back() {
        let (decorators, _) = parse("@@is_user_icon alice\n@@@activate\nBody");
        assert!(decorators.activate);
    }

    #[test]
    fn instruct_decorators_ignored_in_chat_context_fall_back() {
        // Ene is chat-based, so the token-based instruct decorators are ignored
        // and their fallback chains are consulted per the spec.
        let (decorators, _) = parse("@@instruct_depth 5\n@@@depth 2\nBody");
        assert_eq!(decorators.depth, Some(2));
        let (decorators, _) = parse("@@instruct_scan_depth 5\n@@@scan_depth 3\nBody");
        assert_eq!(decorators.scan_depth, Some(3));
        // Without a fallback the whole group is dropped.
        let (decorators, _) = parse("@@instruct_depth 5\nBody");
        assert_eq!(decorators, EntryDecorators::default());
    }

    #[test]
    fn greeting_decorator_parses_and_gates_on_active_index() {
        let (decorators, _) = EntryDecorators::parse_with_greeting(
            "@@is_greeting 2\n@@@activate_only_after 4\nBody",
            Some(2),
        );
        assert_eq!(decorators.is_greeting, Some(2));
        // The honored primary wins; the `@@@` fallback is not consulted.
        assert_eq!(decorators.activate_only_after, None);

        let matching = ActivationContext {
            greeting_index: Some(2),
            ..ActivationContext::default()
        };
        assert!(decorators.activation_passes(&matching));
        let other = ActivationContext {
            greeting_index: Some(1),
            ..ActivationContext::default()
        };
        assert!(!decorators.activation_passes(&other));
    }

    #[test]
    fn greeting_decorator_ignored_without_active_greeting_consults_fallback() {
        // No greeting chosen: checking the active greeting is not possible,
        // so `@@is_greeting` is ignored and the `@@@` fallback applies.
        let (decorators, _) = parse("@@is_greeting 0\n@@@activate_only_after 4\nBody");
        assert_eq!(decorators.is_greeting, None);
        assert_eq!(decorators.activate_only_after, Some(4));
        let ctx = |count: u32| ActivationContext {
            assistant_message_count: count,
            ..ActivationContext::default()
        };
        assert!(!decorators.activation_passes(&ctx(3)));
        assert!(decorators.activation_passes(&ctx(4)));
        // Without a fallback the gate is gone entirely.
        let (bare, _) = parse("@@is_greeting 0\nBody");
        assert!(bare.activation_passes(&ActivationContext::default()));
    }

    #[test]
    fn greeting_decorator_invalid_value_consults_fallback() {
        let (decorators, _) = parse("@@is_greeting abc\n@@@activate_only_after 4\nBody");
        assert_eq!(decorators.is_greeting, None);
        assert_eq!(decorators.activate_only_after, Some(4));
    }

    #[test]
    fn role_parses_all_spec_values() {
        let (decorators, _) = parse("@@role assistant\nBody");
        assert_eq!(decorators.role, Some(DecoratorRole::Assistant));
        let (decorators, _) = parse("@@role user\nBody");
        assert_eq!(decorators.role, Some(DecoratorRole::User));
        let (decorators, _) = parse("@@role narrators\nBody");
        assert_eq!(decorators.role, None);
    }

    #[test]
    fn entry_decorators_accept_combines_filters_and_activation() {
        let entry = sample_entry();
        let decorators = EntryDecorators {
            activate_only_after: Some(2),
            additional_keys: vec!["sword".into()],
            ..EntryDecorators::default()
        };
        let ctx = ActivationContext {
            assistant_message_count: 3,
            ..ActivationContext::default()
        };
        assert!(entry_decorators_accept(
            &decorators,
            &entry,
            0,
            "a sword",
            None,
            &ctx
        ));
        let early = ActivationContext {
            assistant_message_count: 1,
            ..ActivationContext::default()
        };
        assert!(!entry_decorators_accept(
            &decorators,
            &entry,
            0,
            "a sword",
            None,
            &early
        ));
        assert!(!entry_decorators_accept(
            &decorators,
            &entry,
            0,
            "no weapons",
            None,
            &ctx
        ));
    }

    #[test]
    fn activate_bypasses_filters_through_entry_decorators_accept() {
        let entry = sample_entry();
        let decorators = EntryDecorators {
            activate: true,
            exclude_keys: vec!["rusty".into()],
            additional_keys: vec!["sword".into()],
            ..EntryDecorators::default()
        };
        let ctx = ActivationContext {
            assistant_message_count: 0,
            ..ActivationContext::default()
        };
        assert!(entry_decorators_accept(
            &decorators,
            &entry,
            0,
            "a rusty sword",
            None,
            &ctx
        ));
    }
}
