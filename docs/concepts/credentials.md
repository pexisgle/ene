# Credentials

Ene hosts the credentials its plugins need to talk to external services.
Plugins declare what they need through `x-ene-credentials` (see
[Plugins & MCP](plugins-and-mcp.md)); the host resolves, scopes, audits,
stores, refreshes, and revokes the underlying values — a plugin never
handles a raw secret itself.

## Kinds

- **API key** — a static secret the host resolves from configuration
  (`plugins.list.<id>.config.api_key`, `ai.providers.<alias>.api_key`, or
  the `{ID}_API_KEY` environment variable) and injects into a header the
  plugin declared.
- **OAuth2** — a token set acquired through the authorization flow below.
  Requires a public `client_id` (no client secret: a desktop app cannot keep
  one secret).

## Authorization flow (Authorization Code + PKCE)

OAuth2 credentials are obtained with the [Authorization Code + PKCE]
(https://datatracker.ietf.org/doc/html/rfc7636) flow, driven entirely by the
host:

1. A plugin calls `request_authorization("google.calendar")`, or the user
   presses **Authorize** on the desktop Credentials settings page.
2. The host mints a 256-bit PKCE verifier and its S256 challenge, binds an
   ephemeral listener on `127.0.0.1` (a random port), and opens the
   authorization URL in the system browser.
3. The user consents; the authorization server redirects to the loopback
   callback with a `code` and the flow's `state`.
4. The host verifies `state` (128-bit CSPRNG, single-use) and exchanges the
   code — with the PKCE verifier — for an access token (and, when issued, a
   refresh token).
5. The token set is stored in the credential vault and the persistence file,
   and the requesting plugin is notified via an invalidation push so it
   re-resolves.

The flow runs out-of-band: the wire protocol is unchanged, and a plugin sees
`AuthorizationPending` from `request_authorization`, then an invalidation,
then the resolved credential. A failed or timed-out flow (browser could not
open, state mismatch, server refused) also invalidates, so the plugin's next
resolve reports the credential as missing.

## Automatic refresh

When a resolve finds the access token expired — or within 60 seconds of
expiring — the host refreshes it through the credential's declared
`token_url` (`grant_type=refresh_token`). Concurrent resolves for the same
credential are coalesced onto a single HTTP refresh. If the server issues a
new refresh token, it is stored (rotation); otherwise the current one is
kept. A server that does not report `expires_in` yields a token with no
known expiry, which is never auto-refreshed.

A failed refresh is answered with `RefreshRequired`, which the credential
service maps to `PluginError::authorization_required` — the UI asks the user
to re-authorize — and further refresh attempts for that credential are
refused for 60 seconds, so a revoked refresh token cannot hammer the token
endpoint.

## Storage

Tokens are **never** written to `settings.json`. They live in
`<app_data_dir>/credentials.json`, a JSON map of storage key to raw
credential, written with owner-only permissions (`0600`) on Unix via an
atomic temp-file-and-rename. The persistence backend is behind a trait so an
OS keychain can replace the plaintext file later.

Notes:

- **Windows** does not get owner-only permissions from the file mode; the
  file is protected only by the user's own account. A future keychain
  backend closes this gap.
- A corrupt file is moved to `credentials.json.bak` and the store starts
  empty; one unreadable entry is skipped, not fatal.
- Desktop and CLI sharing the same profile use last-writer-wins on the file.
- A credential whose plugin was uninstalled stays in the file until revoked;
  it is unreachable (scope enforcement denies undeclared ids), so this is
  cosmetic.

## Management

- **Desktop**: the Credentials settings page lists every stored credential
  and every declared OAuth2 credential (id, kind, expiry, status) with
  **Authorize** and **Revoke** actions. It is an interim page until the
  schema-driven settings UI absorbs it.
- **CLI**: the authorization flow is unavailable in the CLI (it needs a
  browser); `/auth list`, `/auth status <id>`, and `/auth revoke <id>`
  manage stored credentials from the terminal.

Revoking a credential drops the vault entry, removes it from the persistence
file, and pushes an invalidation so clients drop cached copies.

## Limitations and future work

- **Dynamic-port redirect URIs** require an RFC 8252 §8.3-compliant
  authorization server (Google, GitHub, Microsoft, and most others qualify).
  Servers that only accept a pre-registered fixed redirect URI are not
  supported; a future `redirect_uri` field on the declaration could fix
  this.
- **Device-code flow** is not implemented (no grant-type concept in the
  current declarations). The CLI therefore cannot authorize headlessly.
- **OS keychain** storage is a future replacement for the plaintext file.
- Priorities for these follow-ups are settled when the parent credential
  epic closes.
