# Stage user guide

`ene-stage` is the product GUI. It starts `ene-core` when needed, draws
companions with `ene-vrm` into a **transparent wgpu overlay**, keeps chat
on the **surface** WebSocket, and opens a **separate detail window** for
Home, Companion, Conversation, Voice, Memory, Work, Connections, System, and
the session log.

`ene-desktop` is frozen pre-redesign GUI. Do not add features there; do
not require feature parity. When stage is judged to replace the
product-relevant desktop capabilities, delete desktop. Roles are recorded
in [Product boundaries](../concepts/product-boundaries.md).

```sh
cargo build -p ene-core -p ene-stage
cargo run -p ene-stage
```

The build step is required for stage's automatic local-core startup. Native
Windows development uses the same commands from PowerShell after installing
the stable MSVC Rust toolchain, Visual Studio C++ Build Tools, and the Windows
SDK.

| Window | Depth | Contents |
|---|---|---|
| Character overlay (wgpu) | `surface` | VRM, VRMA, spring bones, look-at, visemes. Space toggles the window frame. Left-drag a body silhouette to move it; each body keeps its own saved position and clicks on the background pass through to windows below. Click-through is System → Overlay click-through (on by default); with it on, dragging needs the cursor over a body (Windows/X11 open an input hole over the silhouette). On Wayland the overlay receives no pointer events while click-through is on, so turn it off first (Space shows the frame as a fallback) and then drag. Esc quits. Positions persist per soul in `desktop.character_positions` and survive restarts. Detail → Companion chooses which VRM bodies are displayed, in order, and saves that choice in `desktop.displayed_soul_ids`; up to two bodies are shown by the stage client. A/D switches the chat session's soul independently of the display list; W/S changes motion on the active body. F3 toggles spring-bone collider wireframes. The same Space/A/D/W/S shortcuts work from Chat or Detail when no text field is focused. |
| Chat | `surface` | Prompt / Steer / Follow-up (hover for meaning), New chat, approvals (Allow / Always / Deny), ask-user (`question.asked` opens the form; `POST /jobs/{id}/answer` or `question.resolved` closes it), mic PCM relay, Detail button (same as the tray). New chat ends the current lane and switches every stage subscription and pending prompt to a fresh session only after the core creates it; the old conversation remains in the log. Status wraps on its own line so setup errors stay readable in a narrow window. |
| Caption | `surface` | Spoken captions. Voice → Caption position (`top` / `bottom` / `left` / `right`) places the window; Pin caption stops it being dragged. Provider and HTTP errors stay on the chat status line (wrapped), not as spoken captions. The overlay closes when the turn ends. Long spoken lines wrap inside the overlay. |
| Spotlight (Alt+Space) | local | Searchable command palette: type to filter, ↑/↓ move, Enter runs, Esc closes (clicking an entry also runs it). Covers detail tabs, mic, chat, and quit. If Alt+Space fails to register, Voice shows a warning and a highlighted Open Spotlight button. |
| Detail (tray or Chat → Detail) | `detail` | Settings IA, inner/thinking/tool/PAD log. Search filters sections; clicking a tab, Home shortcut, or Spotlight clears the filter so it cannot pin you on the current section. An empty log shows a next-step hint instead of a blank pane. |

Direct avatar interaction is configured in Detail → System. A stationary press on a silhouette becomes a click, double-click, or long press; movement past the drag threshold remains a position drag. Each accepted gesture gives the hit soul a short local scale/expression cue. Background clicks stay pass-through. Touch uses the same classifier; pen input follows the platform's mouse or touch mapping. Agent handoff and switching the active chat target are opt-in, and agent handoff is rate-limited. Wayland users must turn off click-through before using touch or drag because the compositor may not deliver events to a transparent surface.

Stage does not use a WebView. Overlay drawing is wgpu with a Slint UI layer
composited on GPU; chrome windows (Chat, Detail, Caption, Spotlight) are
Slint on winit. The process talks to the core only through `ene-api`
(`client_id = stage`). It does not link `ene-core`, `ene-companion`, or
`ene-card`.

The overlay event loop uses `ControlFlow::WaitUntil`. Dirty frames (VRM
motion, look-at target change, visemes, blink, Slint dirty flags, resize,
collider debug, or an active body drag) wake about every 16 ms and
`request_redraw`. A static pose with a stable look-at target, no visemes,
and no dirty UI wakes about every 250 ms and skips the GPU pass. Hover
does not paint the cyan interaction outline; that outline is drag-only.
`CursorLeft` clears hover. Chrome still paints while its windows are open.
Performance gates for idle CPU / frame time are real-GPU only; Cloud Agent
lavapipe numbers are software reference and must not fail those gates.

Wayland click-through uses a coarse `wl_surface` input region from
interaction geometry. X11 uses coarse SHAPE Bounding/Input when the WM
accepts it, otherwise window-wide hit-test plus cursor poll. Windows maps
Passive to `set_cursor_hittest(false)` and keeps the existing DX12/DComp
path. Weston 13 is the Wayland compositor we verify; other compositors are
best-effort. X11 pixel-perfect regions are not a goal.

Characters are `.enechar` packages. `GET /characters` is install inventory;
playable companions are souls (`GET /souls` / `GET /stage` occupants).
`body_ref` is a body UUID. Stage imports the bundled Alicia VRM as
`char.alicia@1.0.0` during first-run setup. The Companion screen offers the
second bundled copy as `char.alicia-b@1.0.0`; activation happens over HTTP
(`soul_from_install`). CCv3/PNG/CHARX
are conversion inputs only — there is no CCv3 editor. Companion export and Work
session export open a save dialog in Documents or Downloads with a typed file name
(`.enechar` / `.json`).

Local `desktop.*` keys (theme, language, mic, captions, beat sync, graphics
quality, core lifetime, displayed companion order/selection in
`displayed_soul_ids`, and per-body overlay placement in `character_positions`)
stay on the client. Theme (`light` /
`dark` / `system`) applies to both the wgpu window clear color and Slint widget
colors, so light mode keeps readable contrast. Japanese UI text uses an OS CJK
font (Yu Gothic / Meiryo on Windows, Noto or Droid on Linux), or
`assets/fonts/NotoSansJP-Regular.ttf` when that file is packaged next to the
binary. Changing language updates open chrome window titles without a restart.
Core PATCH keys
are `core` / `harness` / `approval` / `theme` / `ai` / `mind` / `plugins`.
Plugin enablement is `plugins.profile` (`desktop` / `minimal` / `headless`),
not a `plugins.list` map. API keys stay in the vault. Connections edits MCP
servers as a form (name, local command or HTTP, args/URL) and loads plugin
config from `GET /api/v1/plugins/{id}/config` (same schema and validation as
the host API; secrets are names only). System applies
plugin profile and approval mode the same way. JSON import/export stays folded
for advanced use. Attach to an already-running core with `ENE_API_URL` /
`ENE_API_TOKEN`.

Core lifetime defaults to `desktop.core_lifetime = app` (stop the child core
when stage exits). Set `detached` to leave the core running.

Chat starts **unconfigured**. Open Detail → Conversation and pick a named
provider from the installed catalog (`GET /settings` → `effective.providers`),
then a model. Home shows **Chat is ready.** only when that binding has a model
and, if `effective.providers[].needs_key` is true, a vault API key
(`effective.ai_chat_key_set`). Provider failures such as HTTP 401 are
settings/status errors, not assistant replies. Apply sits above a scrollable,
filterable model list so listing models cannot push save off-screen. Observation
privacy (title mode, OCR hint, current send scope) is on the same Conversation
tab and patches `mind.proactive.world_state`. Engines
and GGUF weights live under Connections → provider assets. TTS/STT are
`ai.tasks.tts` / `ai.tasks.stt`.

VAD/ASR/TTS belong to core. Stage relays microphone PCM on
`GET /sessions/{id}/listen/stream` as packed `pcm_s16le` (not per-chunk JSON
POST) and plays `audio.chunk`. If the listen socket ends while the mic is
still claimed, stage drops the sender and reconnects with a short backoff.
A `Closed` send starts a new stream; a `Full` send drops that frame only. While local TTS is playing, stage raises the
mic RMS gate (`BARGE_IN_ENERGY_FACTOR = 2.0`) so speaker bleed does not look
like barge-in; loud user speech still reaches core VAD. Barge-in is decided in
core (`voice.state` plus an `audio.chunk` with `abort: true`). Stage stops the
playback sink and resets visemes when that abort chunk arrives; it does not use
client RMS as the source of truth. Stage claims speaker/notify exclusive by
default.

TTS utterances carry automatic expressions. The first `audio.chunk` of each
utterance includes the companion's current mood as an `expression` label plus
the owning `soul_id`; stage applies that expression to the matching avatar,
holds it for four seconds, fades it out over the last 0.3 seconds, and clears
it on an abort chunk. Explicit `body.expression` commands from the model keep
overriding this path unchanged.

Audio device relay, approval popups, tray, and OS notifications (`notify.hint`)
are stage's client-side jobs; the core owns policy and the live bus.

## Companion display management

An empty profile starts with one shipped Alicia avatar. Stage does not add a
second body until the user chooses Add companion in Detail → Companion. The
same screen lists each stage occupant with its name, thumbnail or text-only marker,
active chat target, soul, body, and session relationship.

Display selection is a client-side overlay choice, separate from installing a
package and from the active chat target. Add to display / Show loads a body;
Hide for now removes it until the stage restarts; Remove from display removes
it from the persistent overlay list but keeps the package, soul, and sessions.
The up/down controls change the overlay order. A package remains installed
until it is explicitly managed elsewhere, so removing an avatar from the
overlay is not an uninstall.

The stage client currently supports two visible VRM bodies. When the list is
full, the Companion screen explains that another body must be hidden or
removed before it can be shown. A text-only occupant or a failed VRM load is
reported as unavailable rather than silently appearing as an empty slot.
Each soul keeps its own session; history does not leak across them. A/D
retargets chat without changing which bodies are selected for display.

### Automated vs manual

CI and the Cloud Agent use software Vulkan (lavapipe):
`DISPLAY=:1 WGPU_BACKEND=vulkan`. Automated coverage is:

- `ene-vrm` parses and, when a wgpu adapter exists, GPU-loads the shipped Alicia VRM
- HTTP: souls keep isolated sessions; importing Alicia exposes `avatar_path`
- Overlay layout places two slots apart; `ene-stage` writes the minimal GLB fixture

Manual check: run `ene-stage`, confirm two VRM bodies on the overlay, and talk
to each via A/D. In Detail → System, leave Direct avatar reactions on, click and
hold one body without moving it, then drag it; verify only the hit body reacts
and the background still passes through. Enable the optional companion handoff
only with a configured chat provider. That GUI walkthrough is not part of CI.

Manual check: run `ene-stage`, confirm one VRM body on the overlay, then open
Detail → Companion and use the bundled-companion action to add the second body.
Verify the overlay
changes from one body to two, hide it and choose Show, remove it from display,
move the bodies up/down when two are present, and restart to confirm the
persistent selection. A/D should change the chat target independently of
which body is visible. That GUI walkthrough is not part of CI.
