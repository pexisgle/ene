# Architecture (current implementation)

> This page describes the code that exists today. It is **not** the product requirements document. Desired behavior is defined under [Requirements](../requirements/README.md).

Ene currently uses one core process, several clients, out-of-process tool/provider plugins, and in-process domain libraries.

```text
ene-stage   ─┐
ene-desktop ─┤
ene-ctl     ─┼── HTTP/WS (ene-api) ──► ene-core
Web         ─┘                              │
                                            ├── ene-session / ene-kernel
                                            ├── ene-companion / ene-body / ene-work
                                            ├── ene-access-control
                                            ├── ene-tool-registry
                                            └── ene-plugin-host ──► plugins/*
```

- `ene-core` owns process-level state and serves HTTP/WS.
- Clients use `ene-api`; they do not embed the kernel.
- `ene-session` owns the append-only conversation log and usage ledger.
- `ene-kernel` owns the dialogue lane.
- `ene-companion` owns soul, affect, memory, inner state, and proactive behavior.
- `ene-work` owns delegation, jobs, schedules, skills, and MCP bindings.
- `ene-access-control` owns approval, audit, and the credential vault.
- `ene-tool-registry` owns the unified tool registry/pipeline.
- `ene-plugin-host` supervises plugin processes and reversible host-side composition.
- Tool and provider plugins run out of process.

For the complete current crate map and dependency rules, see [Crate reference](../reference/crates.md).
