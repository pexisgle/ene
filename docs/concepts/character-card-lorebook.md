# Character Card Lorebook

A CCv3 character card may ship a **lorebook** (`character_book`) — a set of
world-information entries, each with trigger `keys`, `content`, and optional
flags. This page documents how Ene matches lorebook entries and how the CCv3
`@@` decorators are interpreted.

## Entry matching

An entry is selected when:

- it is `constant: true` (always selected), **or**
- at least one `keys` entry matches the recent conversation (within
  `scan_depth`), unless a `not_keys` entry matches;
- `selective` / `secondary_keys` behave per the CCv3 spec: with `selective`,
  at least one `secondary_keys` value must also match;
- and every active `@@` decorator condition passes (see below).

Matching is substring-based and **case-insensitive** by default
(`case_sensitive: true` opts in). Japanese (and other non-space-separated)
keys work with substring matching, so `ドラゴン` matches
`ドラゴンが現れた`. With `use_regex: true`, keys are regular expressions and
an entry whose keys fail to compile never matches.

> Note on `@@additional_keys`: the spec defines it as "at least one of these
> values must be present" — an AND-condition added on top of the main keys,
> not "all of them must match". Ene follows the spec.

## Injection

Decorator lines are stripped from an entry's `content` before anything is
stored or injected — raw `@@…` text never reaches the prompt.

Key-matched and constant entries are boosted into the recall results (the
hybrid-recall merge path). Placement control (`@@depth`, `@@position`,
`@@role`, `token_budget`, and guaranteed injection as a dedicated prompt
section) is tracked in the [lorebook injection issue #336](https://github.com/pexisgle/Ene/issues/336).

## Decorator reference

Decorators live on their own lines at the start of an entry's `content` (they
work anywhere in the content, and are stripped before injection).

### Activation

| Decorator | Ene behavior |
|---|---|
| `@@activate_only_after N` | Match only once the assistant (character) message count reaches `N`. `N = 0` disables the gate. |
| `@@activate_only_every N` | Match only when the assistant message count divides `N` evenly. `N <= 1` disables the gate. |
| `@@keep_activate_after_match` | Once matched, always matched (sticky). |
| `@@dont_activate_after_match` | Once matched, never matched again. |
| `@@activate` | Force a match "in any case" (overrides `@@dont_activate`). |
| `@@dont_activate` | Never match (unless `@@activate` is also present). |

The sticky decorators (`keep_activate_after_match` /
`dont_activate_after_match`) need previous-match state, which the recall-merge
path does not carry; the spec lets applications ignore them when previous
matches are unknowable, so they are inert there. The injection pipeline (#336)
supplies session-held state.

### Placement

| Decorator | Ene behavior |
|---|---|
| `@@depth N` | Parsed; message-depth injection lands with #336. `N < 1` means prefill, which Ene does not support — it falls back to a high-priority prompt position. |
| `@@reverse_depth N` | Parsed; counted from the oldest message. |
| `@@instruct_depth N` | **Parsed, ignored** — Ene is chat-based and the spec says to ignore this in chat contexts. |
| `@@position after_desc\|before_desc\|personality\|scenario` | Parsed; semantic-section placement lands with #336. |

### Scanning

| Decorator | Ene behavior |
|---|---|
| `@@scan_depth N` | Overrides the book's `scan_depth` for this entry. |
| `@@instruct_scan_depth N` | **Parsed, ignored** — chat context (see `@@instruct_depth`). |
| `@@is_greeting N` | **Parsed, ignored** — Ene does not track the active greeting index; the spec says to ignore the decorator when it cannot be checked. |

### Filters

| Decorator | Ene behavior |
|---|---|
| `@@additional_keys A,B,…` | At least one value must appear in the scan window (AND-condition on top of `keys`). Can be repeated; follows `use_regex`. |
| `@@exclude_keys X,Y,…` | Entry suppressed when any value appears. Ignored when `use_regex` is true (per spec). |
| `@@is_user_icon name` | **Parsed, ignored** — Ene has no user-icon prompt feature. |

### Meta

| Decorator | Ene behavior |
|---|---|
| `@@role assistant\|system\|user` | Parsed; applies to depth-injected messages with #336. |
| `@@ignore_on_max_context` | Parsed; consumed by the token-budget pass with #336. |
| `@@disable_ui_prompt post_history_instructions\|system_prompt` | **Parsed, not wired** — honoring it would let cards disable Ene's expression output contract, which is a core product guarantee. |

## Fallback chains (`@@@`)

An unknown decorator is skipped, but its **fallback** — the following
`@@@name` line — is consulted instead, top to bottom. Chains can be any
length:

```
@@risu_only_decorator 4
@@@agn_only_decorator 4
@@@activate_only_after 4
```

If no element of the chain is recognized, the whole group is ignored (and
still stripped from the content). Only the *first* decorator of a given name
counts — except `@@additional_keys`, which the spec allows to repeat.

## Reference

- [Character Card V3 specification (SPEC_V3.md, "Decorators")](https://github.com/kwaroran/character-card-spec-v3/blob/main/SPEC_V3.md)
