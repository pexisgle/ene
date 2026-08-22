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
| Character overlay (wgpu) | `surface` | VRM, VRMA, spring bones, look-at, visemes. Space toggles the window frame. Click-through is System → Overlay click-through (on by default). Esc quits. Up to two VRM bodies stay on the overlay at once (`body.render.max_concurrent`, default 2). A/D switch which soul the chat session targets; W/S change motion on the active body. F3 toggles spring-bone collider wireframes. The same Space/A/D/W/S shortcuts work from Chat or Detail when no text field is focused. |
| Chat (Alt+F2) | `surface` | Prompt / Steer / Follow-up (hover for meaning), approvals (Allow / Always / Deny), ask-user (`question.asked` → `POST /jobs/{id}/answer`), mic PCM relay, Detail button (same as the tray). Status wraps on its own line so setup errors stay readable in a narrow window. |
| Caption | `surface` | Spoken captions. Voice → Caption position (`top` / `bottom` / `left` / `right`) places the window; Pin caption stops it being dragged. Provider and HTTP errors stay on the chat status line (wrapped), not as spoken captions. The overlay closes when the turn ends. Long spoken lines wrap inside the overlay. |
| Spotlight (Alt+Space) | local | Jump to detail sections, mic, quit. Choosing a command closes the palette. If the OS keeps Alt+Space, use Voice → Open Spotlight |
| Detail (tray or Chat → Detail; Alt+F1 Companion, Alt+F4 Log) | `detail` | Settings IA, inner/thinking/tool/PAD log. Search filters sections; clicking a tab, Home shortcut, Spotlight, or Alt+F1/Alt+F4 clears the filter so it cannot pin you on the current section. An empty log shows a next-step hint instead of a blank pane. |

Stage does not use a WebView. Overlay drawing is wgpu; chrome windows are egui
on winit. The process talks to the core only through `ene-api`
(`client_id = stage`). It does not link `ene-core`, `ene-companion`, or
`ene-card`.

Characters are `.enechar` packages. `GET /characters` is install inventory;
playable companions are souls (`GET /souls` / `GET /stage` occupants).
`body_ref` is a body UUID. Stage imports the bundled Alicia VRM as
`char.alicia@1.0.0` and a second copy as `char.alicia-b@1.0.0` so two
occupants can render at once, then activates them over HTTP
(`soul_from_install`). CCv3/PNG/CHARX
are conversion inputs only — there is no CCv3 editor. Companion export and Work
session export open a save dialog in Documents or Downloads with a typed file name
(`.enechar` / `.json`).

Local `desktop.*` keys (theme, language, mic, captions, beat sync, graphics
quality, core lifetime, overlay placement) stay on the client. Theme (`light` /
`dark` / `system`) applies to both the wgpu window clear color and egui widget
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

Audio device relay, approval popups, tray, and OS notifications (`notify.hint`)
are stage's client-side jobs; the core owns policy and the live bus.

## Two companions

Core boot seeds two souls. Stage then installs the shipped Alicia VRM
(`assets/characters/Alicia/AliciaSolid.vrm`) twice under distinct package ids
so the overlay can draw two bodies. Each soul keeps its own session; history
does not leak across them. A/D retargets chat to the other soul without
unloading the other mesh.

### Automated vs manual

CI and the Cloud Agent use software Vulkan (lavapipe):
`DISPLAY=:1 WGPU_BACKEND=vulkan`. Automated coverage is:

- `ene-vrm` parses and, when a wgpu adapter exists, GPU-loads the shipped Alicia VRM
- HTTP: two souls keep isolated sessions; importing Alicia exposes `avatar_path`
- Overlay layout places two slots apart; `ene-stage` writes the minimal GLB fixture

Manual check: run `ene-stage`, confirm two VRM bodies on the overlay, and talk
to each via A/D. That GUI walkthrough is not part of CI.
