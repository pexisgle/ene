# Architecture

Ene is a **companion harness**: one core daemon process, several clients,
out-of-process tools, and an in-process cognitive layer.

The finished product is defined in
[`plans/harness-redesign/`](../../plans/harness-redesign/README.md).
This page describes the code that is in the tree today.

## Process model

```text
ene-stage   ─┐
ene-desktop ─┤
ene-ctl     ─┼── HTTP/WS (ene-api) ──► ene-core (ene-daemon)
Web         ─┘                              │
                                            ├── ene-session / ene-kernel
                                            ├── ene-companion / ene-body / ene-work
                                            ├── ene-plane (approval + audit + vault)
                                            └── ene-fiber ──► plugins/tool/*
```

- **One host.** Table-stakes state lives in `ene-core`. Clients do not embed
  the kernel.
- **Clients are peers** on the public API. Exclusive resources (mic, approval
  response) are mediated by the daemon. `ene-stage` is the product GUI;
  `ene-desktop` is the legacy GUI of the same API — see
  [Product boundaries](product-boundaries.md).
- **Tools are out of process.** Built-in tools (`fs`, `exec`, `web`, `utility`,
  `app`) use the same IPC as a third-party tool would. Harness functions that
  touch companion state stay in-process and go through `ene-registry`.

## Two layers, one companion

Each companion has a **surface soul** (the dialogue lane) and a **back harness**
(jobs, delegation, schedules). Users speak only to the surface. Complicated
work is delegated; a job lane runs the model with tools, and progress comes
back as companion speech, not a progress bar.

Display depth is `surface` or `detail`. The server decides what a connection
receives. Stage's character overlay and chat are surface; the separate detail
window is detail. The legacy desktop client uses the same depths.

## Where to read next

| Topic | Doc |
|---|---|
| Crate map and dependency rules | [Crate reference](../reference/crates.md) |
| Which client is the product GUI | [Product boundaries](product-boundaries.md) |
| Plugin IPC | rustdoc for `ene-plugin-ipc` |
| Character packages | [Character packages](character-cards.md) |
| Design decisions | [`plans/harness-redesign/decisions.md`](../../plans/harness-redesign/decisions.md) |
