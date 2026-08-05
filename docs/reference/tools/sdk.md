# Tool SDK

This page is the reference for writing plugins with `ene-plugin`. For the
step-by-step guide, see [Write a tool](../../guides/tools/write-a-tool.md).

## The one-line imports

```rust
use ene_plugin::prelude::*;      // everything below, plus ene_infer re-exports
use ene_plugin::prelude::tool;   // tool authoring only
use ene_plugin::prelude::provider; // provider authoring only
```

## Tool actions

An action is a `#[derive(ToolAction)]` struct whose fields are the JSON
arguments and whose `run(&self) -> Result<..., ToolError>` implements the
behaviour:

```rust
#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "utility",
    name = "get_current_time",
    summary = "Get the current local time.",
    description = "...",
    category = "Utility",
    keywords_primary = "time, clock, now",
    side_effects = "...",      // optional, see below
    background_capable         // optional: runs as a deferred task
)]
struct GetTimeAction { /* fields */ }
```

### `#[tool(...)]` attributes

| Attribute | Meaning |
|---|---|
| `namespace` | Tool namespace (the `<namespace>.<name>` prefix) |
| `name` | Action name |
| `summary` | One-line summary shown to the model |
| `description` | Full description for tool selection |
| `category` | Display grouping |
| `keywords_primary` / `keywords_secondary` | Retrieval keywords (tool RAG) |
| `side_effects` | Declaration like `"FileSystem { mutates: true }"` — gates approval |
| `background_capable` | Action may run as a deferred background task |

### `#[arg(...)]` field attributes

Schema constraints for the argument fields: `internal` (hidden),
`enum_values`, `default`, `minimum`/`maximum`, `min_length`/`max_length`,
`min_items`/`max_items`, `description`.

### `ToolError`

Structured, IPC-serializable errors with a kind and message:
`ToolError::internal(...)`, provider errors, validation errors, etc.

## Providers

`ActionSetProvider` returns the action list:

```rust
impl ActionSetProvider for MyProvider {
    fn actions(&self) -> Vec<Box<dyn ToolAction>> { vec![...] }
}
```

`SingleActionProvider` wraps one action. The server entry point:

```rust
run_plugin_server(PluginDispatch::new(
    Some(Arc::new(MyToolProvider)),  // tool
    Some(Arc::new(MyLlm)),           // llm (optional)
    Some(Arc::new(MyEmbed)),         // embedding (optional)
    Some(Arc::new(MyTts)),           // tts (optional)
    Some(Arc::new(MyStt)),           // stt (optional)
)).await
```

`PluginDispatch::new` takes five positional implementations in this order:
tool, llm, embed, tts, stt. VAD arrived later and is attached with the
builder step `.with_vad(plugin)`, and capability mediation with
`.with_capability_provider(plugin)` / `.with_capability_declarations(...)`.

## Provider plugins

Provider plugins implement one or more of `LlmPlugin`, `EmbedPlugin`,
`TtsPlugin`, `SttPlugin`, `VadPlugin` plus `ConfigurablePlugin` (config
schema). The `#[provider(...)]` attribute declares the spec:

```rust
#[derive(LlmPlugin)]
#[provider(
    kind = "openai",
    models = "gpt-5.6-luna",
    streaming,
    vision,
    context_window = 128000,
    max_in_flight = 8,
    queue_depth = 32,
    resource_class = "cloud",
    provides = "llm/chat@1, embed@1",
    requires = "gguf-runner@1"
)]
struct MyLlm;
```

The derive emits the static spec constructor and kind const; you write the
async handlers (`chat_stream`, `chat_completion`, `embed_batch`, …) and the
`*_capabilities()` method yourself. Capability strings are validated at
compile time.

## Local inference discipline

If your plugin runs its own model in-process (llama.cpp, whisper.cpp,
ONNX), use the `ene-infer` framework re-exported through the prelude:

- Implement `LocalModel` (a plain synchronous `&mut self` trait).
- `EngineHandle::spawn(factory, config)` owns the model on a dedicated
  worker thread.
- `EngineHandle::submit(req, token)` gives you a bounded queue, cooperative
  cancellation, one timeout, and panic recovery — never hand-roll
  `spawn_blocking`/`block_in_place` around a shared model.

## Deferred (background) tasks

`background_capable` actions are invoked in deferred mode: the host
returns a task id immediately, the plugin works in the background, and
completion arrives as a `DeferredStatus` (see `utility.notify_send` for a
reference implementation). Lifecycle events
(`tool_background_completed`) surface completion to the UI.

## DB access for stateful tools

Use `ene-plugin-db` for persistent state:

```rust
let client = ene_plugin_db::client::connect().await?;   // host `db` passenger
client.insert(&table, &row).await?;
```

Tables are prefix-isolated per plugin and authenticated by token. The
`counter` plugin is the reference sample.

## Testing

- Unit tests run inside the bin crate (`#[cfg(test)]` modules) — plugin
  crates are binary-only by convention.
- The host side has contract tests (`tests/ipc_integration.rs` patterns in
  `ene-plugin-host`); stateful plugins should ship IPC integration tests
  like `plugins/tool/counter/tests/ipc.rs`.
- `ene-infer`'s `conformance` battery (feature `test-util`) exercises the
  queueing/cancellation/panic-recovery invariants of `LocalModel`
  implementations.

## Where to look for examples

- Simplest tool: `plugins/tool/random`
- Deferred/background: `plugins/tool/utility` (`notify_send`)
- Stateful + permission-gated + IPC tests: `plugins/tool/counter`
- Cloud provider: `plugins/provider/openai`
- Local model provider: `plugins/provider/local-llm`
- Template: `templates/tool/` (`new-tool.sh`)
