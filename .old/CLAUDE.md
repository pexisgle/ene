# CLAUDE.md

Agent instructions for this repository live in a single source of truth: **[AGENTS.md](./AGENTS.md)**.

If your tool supports file imports, this line pulls it in:

@AGENTS.md

Otherwise, read `./AGENTS.md` before making changes. It covers the Nix build environment,
the `--workspace` command gotcha, the workspace-wide clippy contract, crate boundaries,
configuration, the plugin/IPC protocol, and repo etiquette.
