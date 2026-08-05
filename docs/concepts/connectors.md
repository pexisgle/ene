# Connectors

Connectors are the framework for **external-service integrations**:
services like calendars, GitHub, or Discord that the character can act on.
The framework lives in `ene-connector`; individual integrations are
plugins that implement the `Connector` trait.

## What the framework provides

- **`Connector` trait + registry** — a common lifecycle: registration,
  connectivity checks, connect/disconnect, per-action permission grants,
  and status snapshots. Every operation runs under the connector's timeout
  policy and a deny-by-default permission gate.
- **Connector identity** — stable `namespace.name` ids (`ConnectorId`),
  display metadata for configuration UIs (`ConnectorIdentity`), and OAuth
  scopes (`PermissionScope`).
- **Secure credential storage** (`CredentialStore`) — OAuth2 tokens and
  API keys whose secrets are redacted from `Debug`/`Serialize` and zeroed
  on drop. The only raw-material escape hatch is the audited
  `expose_for_persistence` path.
- **`PermissionGate`** — fail-closed, per-action approval with turn-scoped
  grants and conversation-scoped action patterns.
- **Transport policies** — timeout, exponential-backoff retry, rate
  limiting, and pagination helpers for HTTP integrations.
- **Webhook validation** — HMAC signature + replay-window checks for
  incoming webhooks.
- **Credential declarations** — plugins declare which credentials they need
  (`x-ene-credentials`), and `ene-plugin-host` parses/resolves those
  declarations into scoped access at startup.
- **Redaction** — structural secret scrubbing at event, audit, and error
  boundaries.

## Relationship to plugins

`ene-connector` deliberately does **not** depend on `ene-config` or
`ene-plugin-proto`, so plugin binaries can use its credential types without
dragging in the config/protocol stack. `ene-plugin-host` sits between the
two: it knows both the connector world and the plugin world.

## How you interact with connectors

- **Desktop** — the Settings → Connectors page lists connector state and
  lets you connect/disconnect accounts.
- **CLI** — `/connector <list|status|check|connect|disconnect|grant|revoke|permissions>`.
- **Runtime** — `EneHandle::connectors()` returns a `ConnectorHandle`;
  connector state changes are broadcast as `LifecycleEvent::ConnectorChanged`.

## Example: the calendar tool

`plugins/tool/calendar` is the reference connector consumer: it manages
multiple calendar accounts, requests scoped permissions, and exposes
actions (list/create/update/cancel events, free-slot search) that run
through approval gates before touching anything.

Connector state changes surface on the lifecycle bus so the UI can refresh
without polling.
