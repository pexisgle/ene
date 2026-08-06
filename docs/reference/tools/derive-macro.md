# Derive macros

`ene-plugin-macros` provides the derive/attribute macros that turn plain
structs into plugin surface. This page is the attribute reference.

## `#[derive(ToolAction)]`

Generates the `ToolAction` implementation (spec, argument parsing, dispatch)
for an action struct. Reads `#[tool(...)]` on the struct and `#[arg(...)]`
on fields.

## `#[derive(ToolSpec)]`

Generates just the `ToolSpec` (schema + metadata) for a struct, without
action behaviour — useful when a type is shared as a sub-schema.

## `#[tool_action]` (attribute macro)

Alternative form that attaches `#[tool(...)]` metadata to a function-style
action. See `crates/ene-plugin-macros/src/lib.rs` for the exact expansion
contract.

## `#[tool(...)]` container attribute

Applied to a `ToolAction`/`ToolSpec` struct:

| Key | Type | Meaning |
|---|---|---|
| `namespace` | string | Namespace prefix (required) |
| `name` | string | Action name (required) |
| `summary` | string | One-line summary for the model |
| `description` | string | Full description |
| `category` | string | Display category |
| `keywords_primary`, `keywords_secondary` | string | Comma-separated retrieval keywords |
| `side_effects` | string | Side-effect declaration (e.g. `"FileSystem { mutates: true }"`) |
| `background_capable` | flag | Deferred execution allowed |
| `internal` | flag | Hide from the tool registry/schema |

## `#[arg(...)]` field attribute

| Key | Type | Meaning |
|---|---|---|
| `internal` | flag | Exclude from the JSON schema |
| `enum_values` | string | Comma-separated allowed values |
| `default` | string | Schema default |
| `minimum`, `maximum` | int | Numeric bounds |
| `min_length`, `max_length` | int | String length bounds |
| `min_items`, `max_items` | int | Array bounds |
| `description` | string | Override the field doc comment |

## Provider derives

| Derive | Generates |
|---|---|
| `#[derive(LlmPlugin)]` | `llm_spec()` + `LLM_PROVIDER_KIND` |
| `#[derive(TtsPlugin)]` | `tts_spec()` + `TTS_PROVIDER_KIND` |
| `#[derive(SttPlugin)]` | `stt_spec()` + `STT_PROVIDER_KIND` |
| `#[derive(VadPlugin)]` | `vad_spec()` + `VAD_PROVIDER_KIND` |

All four share one `#[provider(...)]` attribute (a compound provider like
`#[derive(LlmPlugin, TtsPlugin)]` uses a single attribute):

| Key | Applies to | Meaning |
|---|---|---|
| `kind` | all | Provider kind string (e.g. `"openai"`, `"local"`) |
| `models` | LLM | Comma-separated model names |
| `voices`, `formats` | TTS | Voice/format lists |
| `streaming` | LLM | Supports streaming chat |
| `vision` | LLM | Supports image input |
| `context_window` | LLM | Advertised context size |
| `max_in_flight`, `queue_depth` | all | Admission hints |
| `frame_size`, `sample_rate` | STT/VAD | Audio parameters |
| `resource_class` | LLM | Resource class for admission |
| `provides`, `requires` | plugin-wide | Capability declarations (`"llm/chat@1, embed@1"`) |

The derive emits only the spec constructors; the `impl LlmPlugin { ... }`
block with your async handlers is written by hand. `provides`/`requires`
methods are emitted only from the `LlmPlugin` expansion — pair an
LLM derive with TTS/STT/VAD derives to declare capabilities.

Capability strings are validated against the grammar at compile time, so a
typo fails the build.

## `define_config!` / `define_tool_config!`

Not proc macros, but the config companion: `ene-config`'s declarative
macros define settings sections (registered into the JSON schema registry
at startup) and tool config schemas. See [Configuration](../../configuration.md).
