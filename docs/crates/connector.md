# `ene-connector`

> **Crate**: `ene-connector` | **Role**: Secure connector framework for external-service integrations

`ene-connector` owns the connector lifecycle and its shared plumbing: the
`Connector` trait and `ConnectorRegistry` (register / check / connect /
disconnect / per-action permission grants / status), common transport
policies (timeout, backoff retry, rate limiting, pagination), webhook
validation, structural secret scrubbing, a fail-closed per-action
`PermissionGate`, and secure OAuth2 / API-key storage (secrets redacted from
`Debug`/`Serialize`, zeroed on drop, never logged). It implements no specific
external service; concrete connectors are separate follow-up features that
register through the runtime's `EneHandle::connectors()`.

---

## Architectural boundaries

- `ene-connector` defines `CredentialStore` / `CredentialData` / `AccountCredentials`, `ConnectorId`, `PermissionScope`, `ConnectorIdentity`, `ConnectorError`, the `Connector` trait, `ConnectorRegistry`, `PermissionGate`, policy helpers, and `WebhookValidator`. It has no MCP-specific, tool-translation, or process-supervision logic of its own.
- Concrete integrations live in the consuming crate (`ene-plugin-host`), keeping this framework decoupled from any single external protocol.
- **Dependency direction**: `ene-connector` deliberately does **not** depend on `ene-config` or `ene-plugin-proto`. Exposing that weight through credential types would propagate it to every plugin that sees them; instead `ene-plugin-host` is the crate that knows both connector and proto.

## Design rationale

- **Why credentials are redacted and zeroed on drop**: `CredentialStore` holds OAuth tokens and API keys, which must not linger in process memory, logs, or accidental serializations. Raw material is reachable only through the explicit, audited `expose_for_persistence()` path.
- **Why the lifecycle layer is back**: an earlier revision shipped a `Connector` trait and `ConnectorRegistry` that were never wired into a live path and were deleted. The framework was reintroduced wired end-to-end: the runtime handle, CLI, desktop status page, and tests consume it, and the permission/audit integration is real.
- **Where the policy types live**: `policy.rs` (retry / rate limiting / timeouts / pagination) is part of the framework again; a credential vault and OAuth flow are planned as follow-ups.

## API reference

Struct and method signatures are not duplicated here — they drift. Generate rustdoc for the authoritative, current API:

```sh
cargo doc -p ene-connector --open
```

Start at `Connector`, `ConnectorRegistry`, and `CredentialStore`.

## Developer guide

See [Connectors](../concepts/connectors.md) for the writing-connectors guide
and the secret-handling contract.

---

## Related
- [Plugins & MCP System](../concepts/plugins-and-mcp.md)
- [System Architecture](../architecture.md)
