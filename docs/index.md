# Ene Documentation

Ene is a **local AI companion harness** written in Rust. Companions live on
your device, speak and listen, remember, and run tools under an approval
plane. A core daemon (`ene-core`) owns that state; desktop, CLI, stage, and Web
are clients of the same API. `ene-stage` is the product GUI; `ene-desktop` is
frozen legacy and is deleted when stage is judged to replace it. See
[Product boundaries](concepts/product-boundaries.md).

The finished-product definition is
[`plans/harness-redesign/`](../plans/harness-redesign/README.md).

## Who this documentation is for

| If you are… | Start here |
|---|---|
| An end user who wants to run Ene | [Quickstart](quickstart.md) |
| Someone configuring companions and settings | [Configuration](configuration.md) → [Concepts](concepts/architecture.md) |
| A developer extending Ene with tools | [Write a tool](guides/tools/write-a-tool.md) |
| A contributor to this repository | [Architecture](concepts/architecture.md) → [Crate reference](reference/crates.md) |

## Documentation map

| Section | What it covers |
|---|---|
| [Quickstart](quickstart.md) | Build, configure, and run `ene-ctl` / `ene-stage` |
| [Configuration](configuration.md) | `settings.json`, environment variables, file locations |
| [Concepts](concepts/architecture.md) | How Ene works: process model, packages, memory, plugins |
| [Apps](apps/stage.md) | User guides for stage, CLI, and the legacy desktop |
| [Guides](guides/character-editor.md) | Task-oriented how-tos |
| [Experiments](experiments/stage-ui-poc.md) | Slint + shared wgpu Stage UI probe |
| [Reference](reference/crates.md) | Crate map and rendering API |

Japanese documentation is available at [日本語ドキュメント](ja/index.md).
Every page under `docs/` has a matching page under `docs/ja/`.

## Source of truth

If a page and the code disagree, the code wins — please report the discrepancy.

- Repository: <https://github.com/pexisgle/ene>
- Rust API docs: `cargo doc --workspace --no-deps`
