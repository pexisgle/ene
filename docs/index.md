# Ene Documentation

Ene is a local AI companion harness written in Rust. This documentation is split deliberately into two kinds of material:

- **Requirements** define what Ene should become. They are being rebuilt interactively and are the product source of truth once confirmed.
- **Current implementation docs** describe the code and behavior that exist in the tree today. They must not silently become product requirements.

## Start here

| If you are… | Start here |
|---|---|
| Defining or reviewing the product | [Requirements](requirements/README.md) |
| An end user who wants to run Ene | [Quickstart](quickstart.md) |
| Someone configuring Ene | [Configuration](configuration.md) |
| A developer extending Ene with tools | [Write a tool](guides/tools/write-a-tool.md) |
| A contributor trying to understand the current code | [Architecture](concepts/architecture.md) → [Crate reference](reference/crates.md) |

## Documentation map

| Section | What it covers |
|---|---|
| [Requirements](requirements/README.md) | Product definition, use cases, requirements, invariants, glossary, decisions |
| [Quickstart](quickstart.md) | Build, configure, and run Ene |
| [Configuration](configuration.md) | Settings, environment variables, file locations |
| [Concepts](concepts/architecture.md) | Descriptive documentation of the current implementation |
| [Apps](apps/stage.md) | Current stage, CLI, and legacy desktop behavior |
| [Guides](guides/character-editor.md) | Current task-oriented how-tos |
| [Reference](reference/crates.md) | Current crate map and APIs |

Japanese documentation is available at [日本語ドキュメント](ja/index.md).

## Source of truth

For **desired product behavior**, confirmed documents under `docs/requirements/` win. For **current behavior and signatures**, code and rustdoc win. Legacy requirement material under `docs/requirements/legacy/` is historical input only.

- Repository: <https://github.com/pexisgle/ene>
- Rust API docs: `cargo doc --workspace --no-deps`
