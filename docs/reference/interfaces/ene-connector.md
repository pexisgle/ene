# `ene-connector` interface

## Role

Secure connector framework for external-service integrations (calendar,
GitHub, Discord, …): connector lifecycle, credentials, permission gates,
transport policies, and webhook validation. It deliberately stays free of
`ene-config` / `ene-plugin-proto` so plugin binaries can use its credential
types without the config/protocol stack.

## Public modules

| Module | Contents |
|---|---|
| `connector` | `Connector` trait, `ConnectorAction`, `ConnectorStatus`, `ConnectorSummary`, `HealthStatus`, `ConnectionState`, `AuthenticatedAccount`, `AccountAuthKind`, `PermissionGrant`, `actions` |
| `registry` | `ConnectorRegistry`, `ConnectorEvent(Kind)`, `AccountRef` |
| `credential` | `CredentialStore`, `CredentialData`, `AccountCredentials` (secrets redacted from Debug/Serialize, zeroized on drop) |
| `identity` | `ConnectorId` (`namespace.name`), `CredentialId`, `PermissionScope`, `ConnectorIdentity` |
| `gate` | `PermissionGate` (fail-closed, per-action) |
| `policy` | `ConnectorPolicy`, `RetryPolicy`, `RateLimitPolicy`, `PaginationPolicy`, `Page`, `RateLimiter`, backoff/retry/pagination helpers |
| `webhook` | `WebhookValidator` (HMAC + replay window) |
| `redaction` | `redact_json`, `scrub_secrets` |
| `declaration` | `CredentialDeclaration`, `CredentialKind`, `parse_credentials`, `resolve_scope`, `ScopeDecision`, rejected/degraded credential types |
| `error` | `ConnectorError` |

## Dependencies

- Depends on: nothing internal.
- Used by: `ene-plugin-host` (credential identity/resolution), `ene-runtime`
  (`ConnectorHandle`), `ene-cli`, `ene-desktop`; connector consumer plugins
  such as `plugins/tool/calendar`.

## Refactoring notes

- The no-dependency rule is deliberate (see the crate docs): adding
  `ene-config` or `ene-plugin-proto` here propagates their weight to every
  plugin that sees the credential types.
- Secrets discipline is part of the interface: credential values never
  implement `Debug`/`Serialize` with raw material; the only escape hatch is
  the audited `expose_for_persistence` path.
- `PermissionGate` semantics (deny-by-default, turn-scoped grants,
  conversation-scoped patterns) are the safety contract for destructive
  external actions — change with the audit log in mind.
