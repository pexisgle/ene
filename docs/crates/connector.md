# `ene-connector` — API Reference

> **Crate**: `ene-connector` | **Role**: Shared connector framework for external integrations & MCP client/server bridge

`ene-connector` provides a reusable framework for building connectors to external services (e.g. Calendar, MCP, GitHub) and bridging Model Context Protocol (MCP) servers into Ene.

---

## Key Components

### 1. Framework Abstractions
- **`Connector` Trait**: Unified lifecycle trait implemented by all external service connectors.
- **`ConnectorRegistry`**: Thread-safe registry for managing active connector instances.
- **`CredentialStore`**: Secure OAuth2 token and API key storage with memory-zeroing on drop.
- **Policies**: Composable `RetryPolicy`, `RateLimiter`, `TimeoutPolicy`, and `PaginationCursor`.

### 2. MCP (Model Context Protocol) Bridge
- Connects external MCP servers running over `stdio` or HTTP SSE.
- Translates MCP tool schemas into Ene `ToolSpec` definitions.
- Forwards LLM tool calls to target MCP servers and returns results.

---

## Related Links
- [Plugins & MCP System](../concepts/plugins-and-mcp.md)
- [System Architecture](../architecture.md)
