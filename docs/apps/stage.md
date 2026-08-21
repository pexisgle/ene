# Stage user guide

`ene-stage` is the product GUI for the new harness core. It starts `ene-core`
when needed, draws companions with `ene-vrm` into a **transparent wgpu overlay**,
keeps chat on the **surface** WebSocket, and opens a **separate detail window**
for Home, Companion, Conversation, Voice, Memory, Work, Connections, System, and
the session log.

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
| Character overlay (wgpu) | `surface` | VRM, VRMA, spring bones, look-at, visemes. Space toggles the window frame. Click-through is System → Overlay click-through (on by default). Esc quits. A/D switch avatar bodies; W/S change motion. F3 toggles spring-bone collider wireframes. The same Space/A/D/W/S shortcuts work from Chat or Detail when no text field is focused. |
| Chat (F2) | `surface` | Prompt / Steer / Follow-up (hover for meaning), approvals (Allow / Always / Deny), ask-user, mic PCM relay, Detail button (same as the tray). Status wraps on its own line so setup errors stay readable in a narrow window. |
| Caption | `surface` | Spoken captions. Voice → Caption position (`top` / `bottom` / `left` / `right`) places the window; Pin caption stops it being dragged. Provider and HTTP errors stay on the chat status line (wrapped), not as spoken captions. The overlay closes when the turn ends. Long spoken lines wrap inside the overlay. |
| Spotlight (Alt+Space) | local | Jump to detail sections, mic, quit. Choosing a command closes the palette. If the OS keeps Alt+Space, use Voice → Open Spotlight |
| Detail (tray or Chat → Detail; F1 Companion, F4 Log) | `detail` | Settings IA, inner/thinking/tool/PAD log. Search filters sections; clicking a tab, Home shortcut, Spotlight, or F1/F4 clears the filter so it cannot pin you on the current section. An empty log shows a next-step hint instead of a blank pane. |

Stage does not use a WebView. Overlay drawing is wgpu; chrome windows are egui
on winit. The process talks to the core only through `ene-api`
(`client_id = stage`). It does not link `ene-core`, `ene-companion`, or
`ene-card`.

Characters are `.enechar` packages. `GET /characters` is install inventory;
playable companions are souls (`GET /souls` / `GET /stage` occupants).
`body_ref` is a body UUID. Stage imports the bundled Alicia VRM as
`char.alicia@1.0.0` and activates it over HTTP (`soul_from_install`). CCv3/PNG/CHARX
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
not a `plugins.list` map. API keys stay in the vault. Attach to an
already-running core with `ENE_API_URL` / `ENE_API_TOKEN`.

Core lifetime defaults to `desktop.core_lifetime = app` (stop the child core
when stage exits). Set `detached` to leave the core running.

Chat starts **unconfigured**. Open Detail → Conversation and bind
`ai.tasks.chat` from installed plugins. List models stays in a scrollable,
filterable list so Apply core fields remains on screen. Engines and GGUF weights live under
Connections → provider assets. TTS/STT are `ai.tasks.tts` / `ai.tasks.stt`.

VAD/ASR/TTS belong to core. Stage relays microphone PCM on
`POST /sessions/{id}/listen` and plays `audio.chunk`. Barge-in is core
`voice.state`, not client RMS. Stage claims speaker/notify exclusive by default.

Audio device relay, approval popups, tray, and OS notifications (`notify.hint`)
are stage's client-side jobs; the core owns policy and the live bus.
