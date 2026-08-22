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
| `proactive` | The companion decides to speak (`ene-companion`). Every open session is observed on `mind.proactive.observation_interval_seconds` (default 60). |
| `scheduled` | A persistent schedule fired (`ene-work`) |
| `delegation` | A back-harness job reporting in |
| `subagent` | A nested work turn |

### What happens inside a turn

1. Recall and affect tick in `ene-companion`. User turns raise `fatigue`;
   time decay pulls it back toward the card baseline.
2. The kernel composes the model-visible prompt from the session log.
   Installed skills appear as the `skills.catalog` System Context source.
   A matching `SKILL.md` body is injected as `skills.active`. Matching
   `ene.emotion_note` frontmatter is prepended to the affect classifier
   input and copied into that active block as a `Tone:` line.
   `ene.proactive_hint` values from enabled skills are passed into the
   proactive decision as `user_instructions`.
   `PATCH /api/v1/souls/{id}/skills` (or `ene-ctl soul skills`) sets the
   soul allowlist; an empty list means every installed skill is eligible.
3. The configured conversation model streams text through its bound `provider.*`
   plugin.
4. Surface-eligible tools run through `ene-registry` / `ene-plane`.
   `delegate.start` returns immediately; `ene-work` opens a **job lane**
   (`origin: delegation`) that uses job-layer tools and `delegation.send`.
   A bookmark request (`workflow.bookmark`) researches with `web.search` when
   that tool is registered, writes Markdown, and delivers it as a job artifact.
   `delegation.send kind=complete` (and the job runner's implicit complete)
   copies every registered artifact into
   `<data>/workspace/jobs/<soul_id>/artifacts/` and sets `delivered` on
   `GET /api/v1/artifacts`. Failed or cancelled jobs leave artifacts
   undelivered. The design doc's `<data>/workspaces/<soul_id>/` layout is
   not used: the live root is singular `workspace`, with per-soul job dirs
   under `jobs/<soul_id>/<job_id>/`.
5. Events are committed to `ene-session` (model-visible equals logged).
   Screenshot and other image tool results store bytes beside the log and keep
   an `ImageRef` in the projection; huge JSON results become `tool/spill`.
6. Live events go out at `surface` or `detail` depth.

Before generation, `agent/pre-step` runs as a waterfall on the shared
`LoopHooks` chain. The host and `ene-fiber` subscribe with a guard that
unregisters on fiber unload. A listener that skips `next` can rewrite or
stop the turn. `emit` buses notify only. Out-of-process plugins do not get
a raw intercept IPC — that would let a tool skip approval or quiet hours.

### System context

Each turn, `ene-kernel::ContextRegistry` assembles System Context in a fixed
order and logs every line as `context/system_message` (`source_key` is the
stable name). The dialogue lane keeps `platform_contract` and a fallback
`identity_kernel`; `ene-core` loads the rest for that turn:

| Key | What it carries |
|---|---|
| `platform_contract` | Output and safety rules |
| `identity_kernel` | Persona from the character package when present |
| `character_state` | Affect mood words (not PAD numbers) |
| `memory.semantic` | Ranked recall for this user text |
| `memory.user_profile` | Standing profile / preference notes |
| `memory.commitments` | Open (unexpired) commitments |
| `skills.catalog` | Installed skill names and descriptions (filtered by the soul allowlist) |
| `skills.active` | Matching `SKILL.md` bodies for this user text |
| `mcp.resources` | Snapshots under `<workspace>/mcp-context/` |
| `inner_recent` | Trailing model-visible inner thoughts |
| `interruption_note` | Only after an interrupted turn |
| `delegation.active` | Created / queued / running public jobs |

Empty loads omit the key. A failed load keeps the last good persona. Replaying
the session log rebuilds the same model-visible surface.

Signatures live in rustdoc for `ene-kernel` and `ene-session`.

## Events

The daemon exposes HTTP plus a WebSocket live bus. `ene-kernel::LiveEvent`
is depth-filtered on the server: `surface` gets speech, `detail` also gets
inner / thinking / tool args. Stage's main window is surface; the separate
detail window (and `ene-ctl --verbose`) is detail.

Conversation history is the append-only log in `sessions.db`, not a
client-side buffer. A provider failure ends the turn as `failed` and is not
written as assistant speech. History projects that failure as a `status`
message so reconnects still see the error.

## Sessions

A **session** is a contiguous conversation with one soul, identified by a
`SessionId`. `ene-ctl` can list, show, create, fork, export, compact, search,
split, and end sessions against the HTTP API.

Idle end and explicit split are server-side. Compaction writes a summary
into the log so later turns stay in budget. Ending a session (explicit
`POST /api/v1/sessions/{id}/end`, idle timeout, or split of a live
session) aborts any in-flight turn, waits until that turn has committed
(interrupted, not assistant speech), writes `session/end`, then drops the
session's dialogue-lane actor from the in-process hub. If the turn does not
go idle in time, the end request fails and `session/end` is not written. A
later prompt on the ended session fails with `closed`.

Session titles are whatever the client sends on create or `PATCH`. The daemon
does not auto-generate a title from conversation content.
