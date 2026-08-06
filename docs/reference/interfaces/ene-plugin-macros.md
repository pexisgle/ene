# `ene-plugin-macros` interface

## Role

Proc-macro crate: derives that turn plain structs into tool actions and
provider specs.

## Public macros

| Macro | Generates |
|---|---|
| `#[derive(ToolAction)]` | Full `ToolAction` impl (spec + argument parsing + dispatch) |
| `#[derive(ToolSpec)]` | `ToolSpec` (schema + metadata) only |
| `#[tool_action]` | Attribute-macro form of `ToolAction` |
| `#[derive(LlmPlugin)]` / `#[derive(TtsPlugin)]` / `#[derive(SttPlugin)]` / `#[derive(VadPlugin)]` | Static spec constructor + kind const per trait (e.g. `llm_spec()`, `LLM_PROVIDER_KIND`) |

## Attributes understood

| Attribute | Applies to | Keys |
|---|---|---|
| `#[tool(...)]` | tool structs | `namespace`, `name`, `summary`, `description`, `category`, `keywords_primary/secondary`, `side_effects`, `background_capable`, `internal` |
| `#[arg(...)]` | tool fields | `internal`, `enum_values`, `default`, `minimum`/`maximum`, `min_length`/`max_length`, `min_items`/`max_items`, `description` |
| `#[provider(...)]` | provider structs | `kind`, `models`, `voices`, `formats`, `streaming`, `vision`, `context_window`, `max_in_flight`, `queue_depth`, `frame_size`, `sample_rate`, `resource_class`, `provides`, `requires` |

See [Derive macros](../tools/derive-macro.md) for the full reference.

## Dependencies

- Depends on: `ene-plugin-proto` (capability grammar validation).
- Used by: `ene-plugin` (re-exported into the prelude), plugin binaries
  directly.

## Refactoring notes

- Capability strings (`provides`/`requires`) are validated at **compile
  time** against the wire grammar — a typo fails the build, not the
  handshake. Keep that property.
- One `#[provider(...)]` attribute feeds compound derives
  (`#[derive(LlmPlugin, TtsPlugin)]`); `provides`/`requires` methods are
  emitted only from the `LlmPlugin` expansion. Changing that split is a
  breaking change for compound providers.
- Async handlers are never generated; authors write the trait impls. The
  derive only produces inherent spec constructors (per-trait names, because
  two derives on one struct cannot share an inherent item name).
