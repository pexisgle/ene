# Developer Guide

Practical docs for people running and extending ene.

[← Docs home](../index.md) · [Reference (contracts & APIs)](../reference/index.md) · [日本語](../ja/guide/index.md)

## Recommended order

1. [Getting started](getting-started.md) — build, run, first provider settings
2. [System overview](system-overview.md) — crates and one turn
3. [Configure](configure.md) — settings you will touch often
4. Concepts: [Turns](concepts/turns.md) · [Sessions](concepts/sessions.md) · [Memory](concepts/memory.md) · [Emotions](concepts/emotions.md)
5. Apps: [CLI](apps/cli.md) · [Desktop](apps/desktop.md)
6. [Voice pipeline](voice-pipeline.md) — microphone, STT, TTS, lip-sync
7. [Release](release.md) — tag-driven Linux releases
8. Tools: [Catalog](tools/overview.md) · [Write a tool](tools/write-a-tool.md)

## Tools

| Document | Topic |
|----------|-------|
| [Catalog](tools/overview.md) | Namespaces and built-in binaries |
| [Filesystem](tools/fs.md) | read / write / edit / shell / … |
| [Web](tools/web.md) | fetch / search |
| [Utility](tools/utility.md) | question, todos, time, … |
| [GUI automation](tools/app.md) | `app` actions |
| [Browser](tools/browser.md) | CDP browser actions |
| [Sandbox](tools/sandbox.md) | Path and command restrictions |
| [Write a tool](tools/write-a-tool.md) | Add a custom binary |

## When you need contracts

Jump to [Reference](../reference/index.md) for ADRs, `ene-*` APIs, streaming internals, and the full settings schema.
