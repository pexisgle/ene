# Configure

Short tour of settings you will touch first. Full field tables live in the [settings reference](../reference/configuration/settings.md).

## Where settings live

Loading order (via `figment`):

1. Compile-time defaults
2. OS user config directory, or local `assets/settings.json` in development
3. `ENE_` environment variables

Schema JSON under `assets/` is auto-generated when you run the CLI — do not hand-edit or commit it.

## Sections you will meet often

| Section | Purpose |
|---------|---------|
| `provider` | Chat model, base URL, API key, embeddings |
| `character` / card path | Which character card to load |
| `store` | Persistence on/off and DB path |
| `mind.*` | Recall, compression, affect, performance policy |
| `tools` | Enable binaries, sandbox, MCP, Tool RAG |

There is no top-level `memory.*` policy block and no dual “cognition” switch — the mind path is the only streaming path. Persistence toggles are under `store`; recall/write policy under `mind.*`.

## Provider sketch

```json
{
  "provider": {
    "name": "openai-compatible",
    "model": "gpt-4o-mini",
    "base_url": "https://api.openai.com/v1",
    "api_key": { "source": "env", "env": "OPENAI_API_KEY" }
  }
}
```

## Character resolution

- Empty → default Alicia card under assets
- Bare name → `assets/characters/{name}/character.json`
- Path → used as-is

## Adding a config field (contributors)

1. Edit the struct in `crates/ene-config/src/config.rs` (`define_config!`)
2. Run `cargo run -p ene-cli` once to regenerate schemas
3. Document in [settings reference](../reference/configuration/settings.md) (EN + JA)

## Next

- [Getting started](getting-started.md)
- [CLI](apps/cli.md) / [Desktop](apps/desktop.md)
- [Full settings reference](../reference/configuration/settings.md)
