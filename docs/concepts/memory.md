# Memory

Ene's memory is explicit, typed, and inspectable. Nothing important lives
only inside the model's context window: conversation is logged, facts are
extracted into typed memories, and everything can be reviewed, edited, or
deleted by the user.

## Typed memories

Each memory item is a row with:

- **Kind** — `episodic` (events), `semantic` (facts), `user_profile`,
  `relationship`, `affective`, `commitment` (promises), `preference`,
  `procedure` (how-to), `reflection` (self-reflection), `world_state`
  (reserved).
- **Scope** — `character`, `user`, or `shared`.
- **Status** — `active`, `faded`, `archived`, `disputed`, `superseded`,
  `user_deleted`.
- **Source** — how it was created: `conversation`, `user_stated`,
  `llm_extracted`, `inferred`, `imported`, `ccv3`.
- **Confidence and salience** — how sure the system is and how important
  the item is; both influence recall.
- **Affect annotation** — valence/arousal captured with the memory.
- **Relationship impact** — how the memory shifts the character's feelings
  toward the user (-1..1).

## How memories are created

After each turn, the memory writer runs in the background:

1. **Deterministic extraction** captures structured signals (commitments,
   user-stated facts, tool outcomes).
2. **LLM extraction** (when a classifier model is configured) proposes
   candidate memories from the conversation.
3. The **memory arbiter** scores candidates, detects semantic duplicates
   and contradictions against existing memories, and decides: write,
   reject, or defer.

When `mind.memory_approval.require_approval` is enabled, deferred candidates
wait in a **pending queue** instead of being written. The host emits
lifecycle events (`pending_candidates_available`, `candidate_changed`) so
the UI can offer review. You can approve, reject, or edit candidates in the
desktop memory ledger or with the CLI `/memory` command.

## How memories are recalled

Every turn, the **recall planner** turns the conversation into a search
plan (what to look for, in which scope, with what budget), and a **hybrid
search** runs:

- vector similarity (embeddings, with a fallback when no embedder exists),
- lexical overlap and title matching,
- recency (exponential half-life decay),
- emotional-match and relationship scoring,
- contradiction penalties and stale-memory penalties,
- access boost: reading a memory raises its future relevance.

Results are diversified (MMR) so one topic does not crowd out the others,
and formatted into the prompt's memory sections with the reason each item
was recalled. Recalled memories also get their `access_count` bumped, which
feeds the forgetting policy.

## Forgetting and the lifecycle

Memories decay with a half-life (global default under
`mind.memory.*`):

```text
active ──(decay)──▶ faded ──(decay)──▶ archived
```

- `faded` memories still appear in search but rank lower.
- `archived` memories are preserved but excluded from normal recall.
- Pinned memories are exempt from decay.
- A contradictory new memory **supersedes** the old one (linked via
  `supersedes_id`); the old row becomes `superseded`.
- Users can mark memories `disputed` or delete them outright.

## The commitment ledger

Promises and follow-ups the companion makes ("I'll remind you tomorrow")
are tracked in a dedicated ledger rather than free-form memory. Each
commitment has a status lifecycle, is injected into the prompt while
active, and can be closed from the CLI (`/commitments`) or the desktop
memory ledger.

## Character-derived memory

Lorebook entries and card data are synced into the store as semantic
memories when a card loads (see
[Character cards → Lorebook](character-cards.md#lorebook)). They are
recalled like any other memory.

## Where memory lives

Everything is persisted in `memory.db` (SQLite + `sqlite-vec` for
embeddings). The store also keeps a full **audit log** of permission
decisions and destructive operations. Backups:

```sh
ene store backup
ene store list-backups
ene store restore <path>
```

## Inspecting memory

- Desktop: Memory page (browse / recall / pending / commitments tabs) and
  the memory ledger.
- CLI: `/memory <list|inspect|search|why|pin|archive|forget|dispute|restore|status|pending|retry|approval>`, `/commitments`,
  `/affect show`.

See the [Memory ledger guide](../guides/memory-ledger.md).
