# `ene-connector`

> **Crate**: `ene-connector` | **Role**: Generic external-service connector framework

`ene-connector` provides a reusable framework for building connectors to external services: connector identity and authenticated accounts, OAuth token/API-key storage (secrets zeroed on drop and never logged), and composable policies (retry with backoff, rate limiting, timeouts, pagination). It does not itself implement any specific external service — concrete connectors are built on top of it. The Model Context Protocol (MCP) bridge, for example, is implemented in `ene-plugin-host` (`McpConnector`, `McpToolRegistry`) as a consumer of this framework, not inside `ene-connector`.

Note: this crate is under active development (`#![expect(missing_docs, ...)]` in `lib.rs`), so its public surface is more likely to change than the other crates documented here.

---

## Architectural boundaries

- `ene-connector` defines the `Connector` lifecycle trait, `ConnectorRegistry`, `CredentialStore`, and policy types (`RetryPolicy`, `RateLimiter`, `TimeoutPolicy`, `PaginationCursor`). It has no MCP-specific or tool-translation logic of its own.
- Concrete integrations (currently the MCP bridge) live in the crate that consumes `ene-connector`'s trait, not in `ene-connector` itself — keeping the framework decoupled from any single external protocol.

## Design rationale

- **Why a shared connector framework instead of one-off client code per integration**: identity/credential management, retry/backoff, rate limiting, and pagination are the same shape across otherwise-unrelated external services; centralizing them here means a new connector only has to implement the `Connector` lifecycle trait, not reinvent policy handling.
- **Why credentials are zeroed on drop**: `CredentialStore` holds OAuth tokens and API keys, which must not linger in process memory or logs after use.

## API reference

Struct and method signatures are not duplicated here — they drift. Generate rustdoc for the authoritative, current API:

```sh
cargo doc -p ene-connector --open
```

Start at the `Connector` trait and `ConnectorRegistry`.

---

## Related
- [Plugins & MCP System](../concepts/plugins-and-mcp.md)
- [System Architecture](../architecture.md)
