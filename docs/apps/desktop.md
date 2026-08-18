# Desktop user guide

`ene-desktop` is the product GUI. It starts `ene-core` when needed, draws
the character overlay with `ene-vrm`, keeps chat on the **surface** depth,
and opens a **separate detail window**.

```sh
cargo run -p ene-desktop
```

| Window | Depth | Contents |
|---|---|---|
| Character overlay + chat | `surface` | Companion and speech. No inner / thinking / tool args |
| Detail (F4 / tray) | `detail` | Session log (including inner), thinking, tools, PAD, tasks |
| Settings | local + API | `desktop.*` stays in the desktop process; other sections PATCH `/api/v1/settings` |

Desktop does not use a WebView. UI is egui; VRM is wgpu. The process talks to
the daemon only through `ene-api` (`client_id = desktop`). It does not link
`ene-daemon`, `ene-companion`, or the old runtime/mind/store crates.

Local `desktop.*` (graphics, theme, language, mic, captions, beat sync, core
lifetime) is persisted by the desktop process. Daemon settings live in the
data-dir `settings.json`. Attach to an already-running core with `ENE_API_URL`
/ `ENE_API_TOKEN`.

`ene-stage` remains an optional debug client for the same API.

Without a conversation provider plugin, replies are Echo-only. Audio device
relay and approval popups are the desktop's client-side jobs; the daemon still
owns policy and the live bus.
