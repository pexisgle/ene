# Memory Ledger: Interactive Memory & Commitment Management

The Memory Ledger is a desktop settings page for inspecting and managing the
companion's long-term memory and commitment ledger from a GUI. It shows every
stored memory item — episodic, semantic, user profile, relationship,
affective, commitment, preference, procedure, world state, and reflection —
together with the companion's commitments in every lifecycle state.

Open **Settings → Memory Ledger** (the tab right after *Memory*).

## Searching and filtering

The toolbar offers:

- **Search** — free-text match against memory titles and content.
- **Kind** — filter by memory category (the kind acts as the category tag).
- **Status** — filter by lifecycle status (active, faded, archived, disputed,
  superseded, deleted).
- **Created** — only memories created within the last 7 / 30 / 90 days, or
  any time.

Above the table, a *Memories by kind* strip shows the distribution of the
currently loaded rows.

## Managing memories

Each table row shows the kind, title, content preview, creation date, status,
scope, and an importance bar with an inline slider:

- **Importance slider** — adjusts the memory's salience score (how strongly
  it competes in hybrid recall). On Preference memories the same control is
  labeled *Preference weight*.
- **Edit** — opens a dialog to change the title, content, kind, and
  confidence. Editing a memory re-derives its ownership scope from the kind
  (e.g. changing a memory to *Preference* moves it to user scope) and
  refreshes its embeddings in the background so vector recall stays accurate.
- **Delete** — marks the memory as deleted (`Confirm delete` is required).
  Deleted memories are removed from recall; the Memory Journal page can
  restore them.

## Managing commitments

The Commitments table lists commitments in every status (active, done,
cancelled, stale) with their due date and creation date. Active commitments
offer **Complete** and **Cancel** buttons; the status change takes effect
immediately in the ledger and in prompt injection.

## Notes

- The ledger requires the memory store (`store.enabled = true`); without it
  the page reports an error on refresh.
- Mutation actions go through the runtime's actor mailbox and emit
  `MemoryLedgerChanged` audit events on the lifecycle bus, so external
  consumers (CLI, API v1) observe manual edits and salience adjustments.
