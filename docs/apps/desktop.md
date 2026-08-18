# Stage user guide

`ene-stage` is the native client. It starts `ene-core` when needed, draws
companions with `ene-vrm`, and keeps a **separate detail window**.

```sh
cargo run -p ene-stage
```

| Window | Depth | Contents |
|---|---|---|
| Main (stage) | `surface` | Companions and speech. No inner / thinking / tool args |
| Detail | `detail` | Session log (including inner), thinking, tools, PAD, tasks |

Stage does not use a WebView. UI is egui; VRM is wgpu.

Without a conversation provider plugin, replies are Echo-only. Audio device
relay and approval popups are stage's client-side jobs; the daemon still owns
policy and the live bus.
