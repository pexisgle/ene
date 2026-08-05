# Connector Framework

The connector framework (`ene-connector`) provides the common plumbing for
external-service integrations so each connector implements only its service
specifics. Authentication storage, permission gating, retries, rate limits,
timeouts, pagination, webhook validation, redaction, audit rows, and status
surfaces are shared.

## Framework shape

- `crates/ene-connector` owns the framework:
  - `Connector` trait — identity, declared actions, transport policy, and the
    four lifecycle operations (`check_connectivity`, `connect`, `disconnect`,
    plus status snapshots maintained by the registry).
  - `ConnectorRegistry` — the common API: register / unregister / list /
    status / check / connect / disconnect / grant / revoke /
    permission-status. Every lifecycle operation is wrapped in the
    connector's per-operation timeout; status reads are I/O-free cached
    snapshots.
  - `PermissionGate` — fail-closed per-action permission model shared with
    the tool permission center: turn-scoped allow-once approvals and
    conversation-scoped `(action, target-prefix)` patterns.
  - `policy` — timeout, exponential backoff retry (with jitter), token-bucket
    rate limiting, and cursor-driven pagination helpers.
  - `webhook` — HMAC-SHA256 signature validation with a replay window.
  - `redaction` — structural secret scrubbing applied at event, audit, and
    error boundaries.
  - `CredentialStore` — OAuth2 / API-key storage whose secrets are redacted
    from `Debug`/`Serialize` and zeroed on drop.
- `ene-runtime` owns the wiring: `EneHandle::connectors()` exposes the
  `ConnectorHandle` (registration + lifecycle + queries), connector
  operations cross the actor mailbox so permission prompts resolve through
  the existing permission center and audit rows land in the same
  tool-permission audit trail (`connector.<id>.<op>` tool names). State
  changes broadcast as `LifecycleEvent::ConnectorChanged`.
- CLI: `/connector list|status|check|connect|disconnect|grant|revoke|permissions`.
- Desktop: the **Connectors** settings page shows cached status, health,
  accounts, and standing grants, with a connectivity-check button.

Concrete connectors (Discord, Slack, GitHub, …) are separate follow-up
features; the framework ships no built-in connector.

## Writing a connector

1. Implement `Connector`:

   ```rust
   struct MyConnector {
       identity: ConnectorIdentity,
   }

   #[async_trait]
   impl Connector for MyConnector {
       fn identity(&self) -> &ConnectorIdentity { &self.identity }

       fn actions(&self) -> &'static [ConnectorAction] {
           &[ConnectorAction::side_effecting("send_message", "Send a message")]
       }

       fn policy(&self) -> ConnectorPolicy {
           ConnectorPolicy::default()
               .with_timeout(Duration::from_secs(10))
               .with_retry(RetryPolicy::new(4, Duration::from_secs(1), Duration::from_secs(8)))
               .with_rate_limit(RateLimitPolicy::new(10, Duration::from_secs(1)))
       }

       async fn check_connectivity(&self) -> Result<HealthStatus, ConnectorError> { /* … */ }
       async fn connect(&self, credential: &AccountCredentials)
           -> Result<Vec<AuthenticatedAccount>, ConnectorError> { /* … */ }
       async fn disconnect(&self, account: &AuthenticatedAccount)
           -> Result<(), ConnectorError> { /* … */ }
   }
   ```

2. Register it with the runtime:

   ```rust
   handle.connectors().register(Arc::new(MyConnector::new()))?;
   ```

3. Use the policy helpers inside your HTTP calls: `retry_with_backoff`
   retries only transient failures (`Transport` / `Io` / `RateLimited`);
   `RateLimiter::acquire` bounds bursts; `collect_pages` walks cursor-based
   endpoints up to `PaginationPolicy::max_pages`. Wrap the whole sequence in
   your operation timeout so a stuck service can never run past the
   operation boundary.

4. Declare every user-visible action in `actions()` — undeclared actions are
   rejected by `grant`, and the permission center lists them for status
   display.

5. Custom actions (beyond `connect` / `disconnect`, which the framework
   gates itself) are enforced by your implementation: after registration,
   grab the connector's gate with `registry.gate(id)` and call
   `gate.check(action, target, description)` inside each action before
   touching the service. This makes per-action grants and revokes apply to
   custom actions too.

## Secret-handling contract

- Secrets are handled exclusively through `AccountCredentials` /
  `CredentialStore` (`SecretString`): redacted from `Debug`/`Serialize`,
  zeroed on drop. The CLI reads API keys from `ENE_CONNECTOR_<ID>_API_KEY`
  (id characters outside `[A-Za-z0-9]` become `_`); the value is never
  echoed or logged.
- **Never** place raw secret material in status messages, events, error
  strings, or descriptions. Errors are built from fixed strings and
  identifiers. `redaction::scrub_secrets` is applied at the registry event
  and audit boundaries as defense in depth — it is not a substitute for the
  contract.
- Audit rows carry no arguments by construction (`{}`) and the store
  redacts argument JSON as a second layer.

## Permissions and audit

- `connect` / `disconnect` are gated deny-by-default through the connector's
  `PermissionGate`. A denied operation returns
  `ConnectorError::PermissionRequired { request_id, action, target, description }`;
  the runtime prompts through the same permission center as tools
  (allow-once / allow-session / deny). Allow-once approvals expire at the
  turn boundary; session patterns expire at the conversation boundary and
  appear in `/permissions` where they can be revoked centrally.
- Explicit `grant` / `revoke` are user commands that record or remove a
  per-action pattern; `/connector permissions <id>` displays them.
- Every connector operation writes one audit row
  (`connector.<id>.<op>`) with the permission decision, action, target, and
  outcome.

## CLI reference

| Command | Effect |
|---|---|
| `/connector list` | Registered connectors with cached state |
| `/connector status <id>` | Health, connection state, accounts |
| `/connector check <id>` | Connectivity probe (read-only) |
| `/connector connect <id>` | Authenticate (reads `ENE_CONNECTOR_<ID>_API_KEY`) |
| `/connector disconnect <id> [account]` | Tear down an account session |
| `/connector grant <id> <action> <target>` | Record a per-action grant |
| `/connector revoke <id> <action> <target>` | Remove a per-action grant |
| `/connector permissions <id>` | List standing grants |

## Status

`list` and `status` read cached snapshots and never touch the network; only
`check` probes the service. Status is updated by the registry after every
check / connect / disconnect, and failures are surfaced as
`ConnectionState::Error` with scrubbed detail.
