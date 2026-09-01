# Product boundaries

This page is the judgment table for **which client is the product GUI**,
what happened to pre-redesign tools and desktop features, and which gaps
are v1.0 versus post-v1.0.

Source of truth: the code on `main`, plus the architecture boundaries in
[Architecture](architecture.md) and
[`plans/harness-redesign/`](../../plans/harness-redesign/README.md).
Existence in the old tree is not a restore reason. v1.0 tracking stays on
[#717](https://github.com/pexisgle/ene/issues/717); this page must not
widen that definition.

## Judgment rules

- **Code and crate boundaries win** over restored UI copy.
- **`ene-stage` is the product GUI.** Grow it. Do not require feature
  parity with `ene-desktop`.
- **`ene-desktop` is frozen legacy.** Restored in
  [PR #794](https://github.com/pexisgle/ene/pull/794) so old UX is not
  lost while stage grows. Do not add features. Do not treat it as a
  second product. When stage is judged to cover the product capabilities
  that still matter, **delete `ene-desktop`**. Until that judgment,
  it stays in-tree only as a reference.
- Port a desktop capability into stage only when the current API and
  architecture still want it. Existence on desktop is not that judgment.
- **MCP-owned domains stay MCP.** Do not reintroduce host-owned OAuth or
  service-specific API clients for git, browser, calendar, Home Assistant,
  or geo ([D-23](../../plans/harness-redesign/tools/capabilities.md)).
- **Restored is not completed.** A page that lives only on desktop, or a
  no-op stub, is not a product completion. Product gaps are closed on
  stage (or in core), not by treating desktop as the shipping UI.
- **Still-missing items** name an open issue or an explicit non-goal.

Status values in the tables:

| Status | Meaning |
|---|---|
| Current | Implemented and on the current API / plugin pipeline |
| Unconnected | Restored or present in the client, but not completing against current core |
| MCP | Intentionally delegated; v1.0 is a handwritten `mcp.json` row |
| Dropped | Will not return as an in-tree tool or host connector |
| Missing | Needed; tracked by the linked issue |
| Post-v1.0 | Successor milestone; not required to close [#717](https://github.com/pexisgle/ene/issues/717) |

## 1. Client roles

Every client is a peer on `ene-api`. There is no desktop-only HTTP
surface. Exclusive resources (mic, speaker, approval response, OS
notify) are mediated by `ene-core`.

| Client | Product status | Owns | Does not own | Verification |
|---|---|---|---|---|
| `ene-stage` | **Product GUI** (`client_id = stage`) | Start/stop local `ene-core`; wgpu overlay + visemes; surface chat; 9-section detail IA (Home, Companion, Conversation, Voice, Memory, Work, Connections, System, Log); tray, captions, spotlight, hotkeys; audio device relay; approval popups; `notify.hint` | Kernel, companion persistence, approval policy, vault, plugin supervision; CCv3 in-app editor; restored desktop observation | Product path: overlay, Conversation bind, settings apply, approvals, exclusive resources. Linux and native Windows CI cover `-p ene-stage`. |
| `ene-desktop` | **Frozen legacy GUI** (`client_id = desktop`) | Pre-redesign 18-page settings/management IA, CCv3 editor, overlay/tray helpers restored in #794 | Feature work; remaining the shipping client; being required for v1.0 E2E | Catalogue of old UX until removal. Do not add features. Linux CI still tests `-p ene-desktop`; native Windows clippy currently targets stage. Delete the crate when stage is judged to replace it. |
| `ene-ctl` | CLI | Text chat, session/tool/plugin/job/memory/schedule/core control, `ene debug` at detail depth | Overlay, OS notify, mic/speaker | Anything on the wire that stage can do. Default `cargo test` member. |
| Web (`apps/ene-core/web`) | LAN / tunnel client | Surface chat + read-only detail (inner, thinking, tools, PAD, memories, jobs) | Settings mutation, memory delete, character admin, VRM | Text path + D-31 (no settings/memory writes). Detail UX is still log-like ([#717](https://github.com/pexisgle/ene/issues/717)). |
| Mobile | Post-v1.0 (M1) | — | — | Not in tree |

`ene-stage` and `ene-desktop` both speak the same API, both draw with
egui + wgpu, and neither uses a WebView.
[PR #794](https://github.com/pexisgle/ene/pull/794) restored the old
desktop binary so that UX is not lost while stage grows, but **the
product seat is `ene-stage`**. Plans say `desktop(stage)` for that role.
Desktop is freeze-only. When stage is judged to substitute for the
product-relevant desktop capabilities, remove `apps/ene-desktop`. A
second connected client for exclusive resources is CLI or Web; do not
keep desktop around for that.

## 2. Tool migration

Old builtins lived under `plugins/tool/` before the harness rewrite.
Current builtins are `fs`, `exec`, `web`, `utility`, and `app`, on the
same IPC as third-party tools. Mature external services are MCP, not
host connectors.

### `fs` (in-tree)

| Old action | Current | Status | Notes |
|---|---|---|---|
| `read` | `fs.read` | Current | Workspace-confined; parent canonicalization |
| `write` | `fs.write` | Current / Missing | Surface hidden (`side_effects: ["fs.write"]`). Atomic replace, line-ending preserve, job-scoped undo: [#797](https://github.com/pexisgle/ene/issues/797) |
| `edit` | `fs.edit` | Current / Missing | Tolerant fallback matching restored. Precondition hash, ambiguity error, CRLF/BOM: [#797](https://github.com/pexisgle/ene/issues/797) |
| `patch` | `fs.patch` | Current / Missing | Hunk context match exists. Same atomic/precondition work as edit: [#797](https://github.com/pexisgle/ene/issues/797) |
| `search` (grep / regex) | `fs.search` | Current | Host-`rg` backed; literal unless `regex` is set, with old grep options |
| `search` glob / path enumerate | — | Missing | [`fs.glob` + FileBroker list](https://github.com/pexisgle/ene/issues/813) |
| `delete` | — | Missing | Approval-gated delete on host FileBroker: [#813](https://github.com/pexisgle/ene/issues/813) |
| `undo` | `fs.undo` | Current / Missing | Same-job only (`job_id` / `ENE_JOB_ID`). Journal atomicity and secret exclusion: [#797](https://github.com/pexisgle/ene/issues/797) |
| Escape-normalized and boundary-only edit strategies | — | Dropped | Indent, line-trimmed, and block-anchor fallbacks are retained; multi-match must error |
| Regex playground | — | Dropped | Out of scope for [#813](https://github.com/pexisgle/ene/issues/813) |
| Direct plugin FS (beyond workspace env) | — | Missing | Host FileBroker as the single confinement: [#813](https://github.com/pexisgle/ene/issues/813) |

### `exec` (split from `fs.shell`, D-24)

| Old action | Current | Status | Notes |
|---|---|---|---|
| `fs.shell` | `exec.run` | Current / Missing | Separate plugin and approval axis. Output caps, process-tree kill, cwd/env policy: [#798](https://github.com/pexisgle/ene/issues/798) |
| Command-string blocklist | — | Dropped | Not a security boundary |
| `exec.pty` | — | Post-v1.0 | Designed in capabilities.md; not in current specs. One persistent PTY is the design cap |

### `web` (in-tree)

| Old action | Current | Status | Notes |
|---|---|---|---|
| `webfetch` | `web.fetch` | Current | `format` is `markdown` (default), `text`, or `html`. HTML keeps headings, paragraphs, and links. Binary types error with `binary_content`. Byte and converted-char caps apply |
| HTML → readable Markdown | `web.fetch` `format=markdown` | Current | Script/style/nav dropped; title and source URL kept |
| `websearch` | `web.search` | Current | `backend` is `duckduckgo` (default, no credential), `arxiv` (domain, same result shape), or `tavily`/`exa` (`credential_missing` until vault). `web.search_backends` lists availability |
| Paid search backends | `web.search_backends` | Current | Declared; not selected without a vault credential |
| Browser automation | — | MCP / Dropped as builtin | Playwright-class MCP, not `tool.web` |

### `app` (in-tree)

| Old action | Current | Status | Notes |
|---|---|---|---|
| `screenshot` / `capture_window` | `app.screenshot` | Current | Portal-first on Wayland, CLI fallback, GDI on Windows. Capture JSON includes size/scale/permission. Model-called shots log `ImageRef` + spill blob (not inline base64) and fold into `LlmImage` only when `ai.tasks.<task>.supports_images` is set. See [App platform matrix](../guides/tools/app-platform.md) |
| `list_windows` | `app.window_list` | Current | wmctrl / hyprctl / sway. GNOME/KDE Wayland reports unsupported via `app.capabilities` |
| `get_active_window` | `app.active_window` | Current | Observation source when proactive screen is enabled |
| `list_monitors` | `app.list_monitors` | Current | Scale/size aligned with capture when the compositor exposes layout |
| `clipboard_read` / `write` | `app.clipboard_get` / `app.clipboard_set` | Current | Native (`arboard`) first, CLI fallback flagged in the payload |
| `mouse_click` / `type_text` / `press_key` / `key_combo` | `app.click` / `app.type` / `app.key` | Current | Advertised only on X11/Windows. `side_effects: ["input"]`; not on the surface schema |
| `mouse_move` / `drag` / `scroll` / `focus_window` | — | Dropped / platform-limited | GNOME/KDE Wayland input is not advertised |
| Portal session lifecycle | `app.screenshot` error `code` | Current | `waiting` / `denied` / `cancelled` / `unsupported` / `unavailable` |

### `utility` / `calc` / `random` (reclassified, D-25)

| Old action | Current | Status | Notes |
|---|---|---|---|
| `calc.evaluate` | `utility.calc` (`expr`) | Current / Missing | `+ - * / ^` and parentheses. `sin` / `max` / `pi` / bindings: [#814](https://github.com/pexisgle/ene/issues/814) |
| `calc.unit` | `utility.calc` (`value`+`from`+`to`) | Current / Missing | Length, mass, time, data, temperature. Volume / speed / area: [#814](https://github.com/pexisgle/ene/issues/814) |
| `calc.color` | — | Missing | sRGB hex/rgb/hsl/alpha: [#814](https://github.com/pexisgle/ene/issues/814) |
| `calc.currency` | `utility.calc` FX snapshot | Current / Missing | `as_of` / `source` exist; `stale` and live feed are not. Live FX is a separate issue |
| `random.number` / `pick` / `uuid` | `utility.random` | Current / Missing | Float, pick, UUID v7. Integer range without modulo bias: [#814](https://github.com/pexisgle/ene/issues/814) |
| `random.color` | — | Missing | [#814](https://github.com/pexisgle/ene/issues/814) |
| `utility.time` / `system_info` / `hash` / `text` | same names | Current | Hash, encode/decode, regex |
| `utility.question` | harness `ask-user` | Dropped as tool | Core lane, not a plugin |
| `utility.notify` | client `notify.hint` | Dropped as tool | Desktop/stage OS notify; CLI does not notify |
| `utility.timer` | schedules | Dropped as tool | `ene-work` schedules; quiet hours / `important` |
| `utility.todo` | jobs | Dropped as tool | Public delegation / task list |
| `counter.*` | — | Dropped | Stateful sample; no product demand |

### MCP-delegated (D-23) — not restored as builtins

v1.0 connection is a handwritten `mcp.json` row on the same registry
pipeline; stage now adds a curated official-server catalog on top of the
same rows ([#812](https://github.com/pexisgle/ene/issues/812), P-616).

| Old plugin / action | Delegate | Status |
|---|---|---|
| `git.status` / `log` / `diff` / `branch` / `blame` / `remote` | git MCP | MCP (stdio git fixture is the process acceptance) |
| `browser.navigate` / `click` / `type_text` / `get_content` / `screenshot` / `scroll` / `wait` / `close` | Playwright-class MCP | MCP |
| `calendar.list_*` / `create_event` / `update_event` / `cancel_event` / `find_free_slots` / accounts | calendar MCP | MCP — no host OAuth client |
| `homeassistant.state` / `turn` / `climate` | Home Assistant MCP | MCP |
| `geo.weather` / `location` / `timezone` / `sun` | geo/weather MCP | MCP |

## 3. Legacy desktop feature inventory

Restored in [PR #794](https://github.com/pexisgle/ene/pull/794). These
rows describe the **old GUI**. Product work ports a capability into
`ene-stage` (or into core) when the current API still wants it. A desktop
page that already talks to `ene-api` is *available on the frozen client*,
not a reason to keep the crate.

### Settings and management pages

| Legacy page | On desktop | Stage destination | Status |
|---|---|---|---|
| Overview | Health / needs-config | Home | Grow on stage if Home is thinner |
| General (graphics, accessibility, language, theme, captions, hotkeys) | Local `desktop.*` | System + Voice (captions) | Stage already has theme/language/captions/overlay. Accessibility/hotkey depth stays on desktop until ported |
| Character | Occupants / bodies over HTTP; local placement | Companion | Current on both |
| Character editor (CCv3) | Local `character.json` via `ene-card` | None | Do not port. v1.0 is package import (`P-803`). Not a reason to keep desktop |
| AI / Voice / Engines | `GET/PATCH /settings`, `providers`, `provider.assets` | Conversation, Voice, Connections | Current on stage; grow stage if a desktop control is still missing |
| Features (proactive toggles) | `mind.proactive.*` PATCH | Conversation | Observation privacy (`title_mode`, `ocr_hint`, send scope) is current on stage. Other proactive toggles may still grow |
| Memory config + Memories ledger | Memory HTTP | Memory | Current on stage for list/edit/delete; auxiliary LLM scope remains [#717](https://github.com/pexisgle/ene/issues/717) |
| Sessions | Session HTTP | Log | Current on stage |
| Permissions / Approvals | Plane HTTP | System | Current on stage |
| Connectors (MCP form + curated catalog) | `GET/PUT /mcp`, `GET /mcp/catalog`, `POST /mcp/probe` | Connections | Handwritten form plus curated official-server picker, probe-based tool preview before enable, and status/error display are current on stage ([#812](https://github.com/pexisgle/ene/issues/812)) |
| Schedules | Schedule HTTP | Work | Current on stage |
| Plugins / Advanced / Diagnostics | Plugin profile + schema leaves | System | Current on stage. Plugin config schema: [#819](https://github.com/pexisgle/ene/issues/819) |

### Overlay and platform

| Feature | Where | Status |
|---|---|---|
| wgpu VRM overlay, look-at, spring bones, visemes | stage + desktop | Current on stage (product) |
| Click-through, input region, Wayland layer-shell / mask | stage + desktop | Current on stage; extra desktop gizmos stay legacy unless ported |
| Tray, captions, spotlight, hotkeys | stage + desktop | Current on stage |
| Audio PCM relay, exclusive speaker/notify | stage + desktop | Current on stage |
| Beat sync, graphics quality | both (client-local) | Stage has quality/placement. Beat-sync depth on desktop is legacy until ported |

### Observation (old `proactive_observe`)

The pre-redesign desktop owned ROI crop, luma fingerprint, OCR, and
screen summary. The restored desktop control is a **no-op stub**. Do not
rebuild that pipeline inside desktop; put it in `ene-work` / `ene-companion`
and surface privacy controls on **stage**.

| Piece | Owner | Status |
|---|---|---|
| Screenshot capability | `app` tool / client | Current CLI path; portal: [#800](https://github.com/pexisgle/ene/issues/800) |
| ROI, luma fingerprint, changed-cell gate, caret suppression | `ene-work` observation pipeline | Current |
| Title redaction (AppOnly / RedactedTitle / FullTitle) | `ene-companion` settings + `ene-work` send label | Current (`mind.proactive.world_state.title_mode`) |
| Proactive speak / world-state | `ene-companion` + core tick | Current: every open session, interval from `mind.proactive.observation_interval_seconds`; unchanged frames reuse the last summary |
| Raw pixels in session / memory / audit | Forbidden | Current; digest and text summary only |
| Desktop `ProactiveObserveControl` | Client stub | Unconnected |

## 4. Security delta

| Layer | Current | Gap |
|---|---|---|
| OS sandbox (`ene-sandbox`) | Landlock + seccomp + rlimits on Linux | Windows AppContainer is still the design target, not a claimed current path |
| Host FileBroker (`ene-plugin-host`) | `confine_path` for read/write; registry also rewrites `fs.*` args | Plugin still receives `ENE_WORKSPACE` and can touch files. List/glob/delete + TOCTOU: [#813](https://github.com/pexisgle/ene/issues/813) |
| Host net broker | Private/loopback/link-local deny, DNS pin, no redirects, 1 MiB body | `web` plugin bypasses it with reqwest + up to 4 redirects: [#799](https://github.com/pexisgle/ene/issues/799) |
| Credentials | Vault (`vault.bin` + `vault.key`); plugins do not get raw keys in env | Keep vault refs for search backends ([#818](https://github.com/pexisgle/ene/issues/818)) and plugin config ([#819](https://github.com/pexisgle/ene/issues/819)). No host OAuth for MCP services |
| Approval (`ene-access-control`) | Deny-by-default, hash chain, popup, “don’t ask next time” | AI auto-approve production model still unset ([#717](https://github.com/pexisgle/ene/issues/717)) |
| `exec` | SIGTERM then SIGKILL on the direct child | Process-tree ownership, output byte caps, cwd/env allowlist: [#798](https://github.com/pexisgle/ene/issues/798) |
| Raw pixels | Observation summarizes off the session log | Current: session / memory / audit store digest and summary, not PNG |

## 5. v1.0 versus post-v1.0

Aligned with [`product/done.md`](../../plans/harness-redesign/product/done.md)
and [`product/features.md`](../../plans/harness-redesign/product/features.md).
Closing a child issue does not close [#717](https://github.com/pexisgle/ene/issues/717).

### v1.0 (must complete or explicitly defer inside #717)

- Product GUI is `ene-stage`; grow it. CLI and Web are peers. `ene-desktop` is the legacy GUI, not the v1.0 E2E client.
- Bundled `fs` / `exec` / `web` / `utility` / `app` on the shared registry.
- Handwritten MCP (one real stdio server, e.g. git).
- Security boundaries in §4 that are already claimed in `done.md`, plus
  the high-priority tool hardening issues: [#797](https://github.com/pexisgle/ene/issues/797),
  [#798](https://github.com/pexisgle/ene/issues/798),
  [#799](https://github.com/pexisgle/ene/issues/799),
  [#813](https://github.com/pexisgle/ene/issues/813).
- Observation that does not flood the model or persist raw pixels:
  current (`ene-work` gate + stage privacy controls).
- Remaining `done.md` unchecked items (real provider chat, production
  ASR/TTS, job runner speech, GUI E2E) stay on #717.

### Post-v1.0 (do not block #717)

| Item | Issue / ID | Why later |
|---|---|---|
| MCP catalog, install preview, health, auth UX | [#812](https://github.com/pexisgle/ene/issues/812), P-616, M8 | Shipped post-v1.0: curated static catalog, one-shot probe preview with per-tool side effects before enable, fiber error surfacing with auth-required state, and manual bearer-token injection via the vault |
| Tool discovery index | [#817](https://github.com/pexisgle/ene/issues/817) (epic [#796](https://github.com/pexisgle/ene/issues/796)) | Scoring stays in `ene-tool-registry`; not a v1.0 `done.md` box |
| Background tool start/cancel/completion | [#816](https://github.com/pexisgle/ene/issues/816) (epic #796) | Persist on `ene-work` jobs; no second task store |
| Plugin config schema / dynamic options | [#819](https://github.com/pexisgle/ene/issues/819) (epic #796) | Unreleased: no legacy config shim |
| Readable Markdown + search backends | [#818](https://github.com/pexisgle/ene/issues/818) | Shipped: markdown/text/html fetch, DDG+ArXiv, paid backends declared unconfigured |
| Portal-first capture/clipboard | [#800](https://github.com/pexisgle/ene/issues/800) | CLI path is the current v1.0 capability |
| Utility math/units/color/random fill | [#814](https://github.com/pexisgle/ene/issues/814) | Low priority; no eval/code execution |
| `exec.pty`, desktop-pet mode, camera, Live2D, mobile | features.md successor IDs | Form-compatible, not v1.0 |
| Host OAuth / service API clients for MCP domains | — | Non-goal (D-23) |
| In-tree git/browser/calendar/HA/geo/counter | — | Dropped |
| Desktop feature parity on stage | — | Non-goal. Port selected UX into stage; freeze desktop |
| Keep shipping `ene-desktop` | — | Non-goal. Delete it once stage is judged to replace the product-relevant capabilities |

### Epic #796

[#796](https://github.com/pexisgle/ene/issues/796) is a tracker only.
Implement [#817](https://github.com/pexisgle/ene/issues/817),
[#816](https://github.com/pexisgle/ene/issues/816), and
[#819](https://github.com/pexisgle/ene/issues/819). Do not add a second
job store, leak scoring into the wire ABI, or put secrets in plugin
schema responses.

## See also

- [Stage user guide](../apps/stage.md)
- [Desktop user guide](../apps/desktop.md) (legacy)
- [CLI user guide](../apps/cli.md)
- [Built-in tools](../guides/tools/builtin-tools.md)
- [Sandbox and approvals](sandbox-and-approvals.md)
- [Plugins and MCP](plugins-and-mcp.md)
