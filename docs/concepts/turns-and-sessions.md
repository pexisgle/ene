# Turns & sessions

## Turns

A **turn** is one unit of conversation: a user message (or a proactive /
scheduled / delegated trigger) plus everything the daemon does in response.
Every turn has a `TurnId`. The dialogue lane lives in `ene-kernel`.

Clients start a turn with `POST /api/v1/sessions/{id}/messages` (`prompt`,
`steer`, or `follow_up`). The kernel returns a `TurnId` and streams on the
live bus. A second `prompt` while a turn is running fails with `lane_busy`.
`steer` queues a correction without cutting generation; `abort` cancels the
running turn; `compact` compresses history.

### Turn origins

| Origin | Trigger |
|---|---|
| `user` | A message from a client |
| `proactive` | The companion decides to speak (`ene-companion`) |
| `scheduled` | A persistent schedule fired (`ene-work`) |
| `delegation` | A back-harness job reporting in |
| `subagent` | A nested work turn |

### What happens inside a turn

1. Recall and affect tick in `ene-companion`.
2. The kernel composes the model-visible prompt from the session log.
3. The configured conversation model streams text through its bound `provider.*`
   plugin.
4. Surface-eligible tools run through `ene-registry` / `ene-plane`.
5. Events are committed to `ene-session` (model-visible equals logged).
6. Live events go out at `surface` or `detail` depth.

Signatures live in rustdoc for `ene-kernel` and `ene-session`.

## Events

The daemon exposes HTTP plus a WebSocket live bus. `ene-kernel::LiveEvent`
is depth-filtered on the server: `surface` gets speech, `detail` also gets
inner / thinking / tool args. Stage's main window is surface; the separate
detail window (and `ene-ctl --verbose`) is detail.

Conversation history is the append-only log in `sessions.db`, not a
client-side buffer.

## Sessions

A **session** is a contiguous conversation with one soul, identified by a
`SessionId`. `ene-ctl` can list, show, create, fork, export, compact, search,
split, and end sessions against the HTTP API.

Idle end and topic-boundary split are server-side. Compaction writes a
summary into the log so later turns stay in budget.
