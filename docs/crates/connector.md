# `ene-connector`

> **Crate**: `ene-connector` | **Role**: Credential and identity authority for external-service connectors

`ene-connector` owns the *credential* side of connecting to external services: secure OAuth2 / API-key storage (secrets are redacted from `Debug`/`Serialize` and zeroed on drop, never logged), stable connector identifiers (`ConnectorId`), OAuth permission scopes, and display metadata (`ConnectorIdentity`). It does not implement any specific external service, and it does not own connection lifecycle — process supervision, restarts, and health probing live in `ene-plugin-host`, which is the only crate that consumes these types. The MCP bridge (including its SSRF URL validation) is implemented there, not here.

---

## Architectural boundaries

- `ene-connector` defines `CredentialStore` / `CredentialData` / `AccountCredentials`, `ConnectorId`, `PermissionScope`, `ConnectorIdentity`, and `ConnectorError`. It has no MCP-specific, tool-translation, or process-supervision logic of its own.
- Concrete integrations live in the consuming crate (`ene-plugin-host`), keeping this framework decoupled from any single external protocol.
- **Dependency direction**: `ene-connector` deliberately does **not** depend on `ene-config` or `ene-plugin-proto` (#308). Exposing that weight through credential types would propagate it to every plugin that sees them; instead `ene-plugin-host` is the crate that knows both connector and proto (#412).

## Design rationale

- **Why credentials are redacted and zeroed on drop**: `CredentialStore` holds OAuth tokens and API keys, which must not linger in process memory, logs, or accidental serializations. Raw material is reachable only through the explicit, audited `expose_for_persistence()` path.
- **Why the `Connector` lifecycle layer was removed (#416)**: an earlier revision shipped a `Connector` trait and `ConnectorRegistry` that were never wired into the live MCP path and duplicated supervision `ene-plugin-host` already provides. They were deleted; the MCP bridge's SSRF URL validation moved into `ene-plugin-host`'s `mcp_registry`.
- **Where the policy types went**: the former `policy.rs` (retry / rate limiting / timeouts) was deleted with the lifecycle layer. A client-side credential policy is reintroduced in `ene-plugin` under #413; a credential vault and OAuth flow land here in #415.

## API reference

Struct and method signatures are not duplicated here — they drift. Generate rustdoc for the authoritative, current API:

```sh
cargo doc -p ene-connector --open
```

Start at `CredentialStore` and `ConnectorId`.

---

## Related
- [Plugins & MCP System](../concepts/plugins-and-mcp.md)
- [System Architecture](../architecture.md)
