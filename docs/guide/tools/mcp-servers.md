# MCP Server Setup Guide

Ene connects to any Model Context Protocol (MCP) server — local processes over
`stdio`, or remote endpoints over HTTP — and exposes their tools to the
character. This guide walks through configuring MCP servers for Calendar,
Mail/Chat, Notes, Map, and RSS services. The underlying mechanism is generic;
if you already have an MCP server for a service, the configuration shape below
applies unchanged.

> The examples reference third-party MCP servers by their npm/registry
> package names. They are community- or vendor-maintained projects, not Ene
> code — check each project's README for the current setup steps, since
> package names, environment variables, and auth flows change over time.

---

## 1. How MCP servers are declared

MCP servers live in `plugins.mcp_servers` inside `settings.json` (see
[Settings & Configuration Reference](../../configuration.md) for where the
file lives and the config precedence rules):

```jsonc
{
  "plugins": {
    "mcp_servers": [
      {
        "name": "my-server",
        "enabled": true,
        "transport": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "@some/mcp-server"]
        },
        "env_passthrough": ["SOME_API_KEY"]
      }
    ]
  }
}
```

| Field | Type | Notes |
|---|---|---|
| `name` | string | Used verbatim for routing and tool namespacing — hyphens and other characters are fine. |
| `enabled` | bool | **Required.** `false` skips the server without touching the entry. |
| `transport.type` | `"stdio"` \| `"http"` | **Required.** Stdio spawns a child process; http connects to a remote endpoint. |
| `transport.command` / `transport.args` | string / string[] | Stdio only: the process to spawn, e.g. `npx` with `-y <package>`. |
| `transport.url` | string | HTTP only: the server endpoint. |
| `transport.auth_header` | string | HTTP only, optional: sent as the `Authorization` header, e.g. `"Bearer <token>"`. A malformed value fails the connection (never a silent downgrade to unauthenticated). |
| `env_passthrough` | string[] | Stdio only: names of host-process environment variables forwarded to the server child process (API keys, etc.). |

Two behaviors of the stdio transport are worth knowing before wiring up
secrets:

- **The child's environment is cleared.** Only `PATH`, `HOME`, `TMPDIR`,
  `LANG`, `TZ`, `LD_LIBRARY_PATH` (Linux) and a few Windows essentials are
  forwarded automatically. Anything else — in particular API keys — must be
  (a) exported in the environment Ene was launched from, and (b) whitelisted
  in `env_passthrough`.
- **There is no `env` map** (unlike some other MCP clients): Ene does not
  define inline environment variables per server. Export the variables before
  launching Ene, then list them in `env_passthrough`.

The HTTP transport is HTTPS-only by default, and loopback addresses
(`127.0.0.0/8`, `::1`) are refused as an SSRF safeguard. For a server running
on your own machine (e.g. Obsidian's built-in MCP endpoint), opt in with
`"mcp_allow_insecure_urls": true` inside `plugins` — this permits plain
`http://` and loopback, while link-local addresses stay blocked. See
[Plugins & MCP](../../concepts/plugins-and-mcp.md) for the full security
model.

Because `mcp_servers` is an array, its entries are declared in `settings.json`
— `ENE_` environment variables cannot add array elements. Scalar plugin
options can still be overridden per launch, e.g.
`ENE_PLUGINS__MCP_ALLOW_INSECURE_URLS=true`.

After adding or editing servers, restart Ene. Verify that a server's tools
are live with the CLI command `/tool list` (each MCP tool appears under its
server name). A server that fails to connect is logged and skipped, so its tools silently
disappear from the list; check the log output rather than assuming the
server loaded. The message differs by transport: stdio children log "MCP
server failed to connect", HTTP endpoints "MCP HTTP connection failed".

## 2. Prerequisites

- **Node.js + npm** for `npx`-launched servers (the majority below).
- **Python + uv** for `uvx`-launched servers (e.g. the CalDAV variant).
- The accounts/API keys for the services you want to connect.
- OAuth-based servers need a one-time interactive authorization. Run the
  server's `auth` step manually in a terminal **before** configuring Ene —
  Ene spawns servers over stdio pipes, so a browser-based consent flow is
  much easier to complete outside Ene.

---

## 3. Calendar

### 3.1 Google Calendar — `@cocal/google-calendar-mcp`

A stdio server backed by the Google Calendar API.

1. Create a Google Cloud project, enable the **Calendar API**, and create
   OAuth 2.0 credentials of type **Desktop app**; download the client JSON
   (the `gcp-oauth.keys.json` file).
2. Run the one-time authorization:
   ```bash
   export GOOGLE_OAUTH_CREDENTIALS="/path/to/gcp-oauth.keys.json"
   npx @cocal/google-calendar-mcp auth
   ```
3. Declare the server, forwarding the credentials path:

```jsonc
{
  "plugins": {
    "mcp_servers": [
      {
        "name": "google-calendar",
        "enabled": true,
        "transport": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "@cocal/google-calendar-mcp"]
        },
        "env_passthrough": ["GOOGLE_OAUTH_CREDENTIALS"]
      }
    ]
  }
}
```

```bash
export GOOGLE_OAUTH_CREDENTIALS="/path/to/gcp-oauth.keys.json"
```

**Authentication:** OAuth 2.0 via the credentials file. **Troubleshooting:**
`ENOENT ... gcp-oauth.keys.json` means the variable is missing or the path is
wrong — the file is *not* auto-discovered under npx, so the environment
variable is mandatory. Re-run the `auth` step if Ene cannot refresh the
token. Google also offers a hosted Calendar MCP endpoint
(`https://calendarmcp.googleapis.com/mcp/v1`); it requires the OAuth2 client
flow, which Ene's HTTP transport (static `Authorization` header) cannot
perform, so the stdio server above is the supported route.

### 3.2 CalDAV — `caldav-mcp`

For self-hosted or provider calendars that speak CalDAV (Nextcloud,
ownCloud, Yandex Calendar, iCloud, FastMail, …):

```jsonc
{
  "plugins": {
    "mcp_servers": [
      {
        "name": "caldav",
        "enabled": true,
        "transport": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "caldav-mcp"]
        },
        "env_passthrough": ["CALDAV_BASE_URL", "CALDAV_USERNAME", "CALDAV_PASSWORD"]
      }
    ]
  }
}
```

```bash
export CALDAV_BASE_URL="https://your-domain.com/remote.php/dav/calendars/yourname/"
export CALDAV_USERNAME="yourname"
export CALDAV_PASSWORD="your-password"
```

**Authentication:** Basic auth with username/password. **Troubleshooting:**
several providers require an **app-specific password** (iCloud, Yandex) or
your full account URL (Nextcloud: `/remote.php/dav/calendars/<user>/`).
A Python-based alternative is `uvx mcp-caldav` (uses `CALDAV_URL` instead of
`CALDAV_BASE_URL`).

---

## 4. Mail & Chat

### 4.1 Gmail — `@franciscpd/gmail-mcp-server`

1. In Google Cloud Console, enable the **Gmail API**, configure the OAuth
   consent screen (scope `https://mail.google.com/`), and create an OAuth
   2.0 **Web application** client.
2. Exchange an authorization code for a refresh token (e.g. via the OAuth
   2.0 Playground with your own client ID/secret).
3. Declare the server:

```jsonc
{
  "plugins": {
    "mcp_servers": [
      {
        "name": "gmail",
        "enabled": true,
        "transport": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "@franciscpd/gmail-mcp-server"]
        },
        "env_passthrough": ["GMAIL_CLIENT_ID", "GMAIL_CLIENT_SECRET", "GMAIL_REFRESH_TOKEN"]
      }
    ]
  }
}
```

```bash
export GMAIL_CLIENT_ID="...apps.googleusercontent.com"
export GMAIL_CLIENT_SECRET="GOCSPX-..."
export GMAIL_REFRESH_TOKEN="1//0..."
```

**Authentication:** OAuth 2.0 with a refresh token (no interactive flow after
setup). **Troubleshooting:** `Error 403: access_denied` during consent means
the account is not listed as a test user on the consent screen. Like
Calendar, Google's hosted Gmail MCP endpoint
(`https://gmailmcp.googleapis.com/mcp/v1`) requires the OAuth2 client flow
and is therefore not reachable through Ene's HTTP transport.

### 4.2 Slack — `@modelcontextprotocol/server-slack`

1. Create a Slack app in your workspace, add the Bot Token Scopes
   (`channels:history`, `channels:read`, `chat:write`, `reactions:write`,
   `users:read`, `users.profile:read`), and install it.
2. Copy the **Bot User OAuth Token** (`xoxb-…`) and the workspace/team ID.

```jsonc
{
  "plugins": {
    "mcp_servers": [
      {
        "name": "slack",
        "enabled": true,
        "transport": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "@modelcontextprotocol/server-slack"]
        },
        "env_passthrough": ["SLACK_BOT_TOKEN", "SLACK_TEAM_ID"]
      }
    ]
  }
}
```

```bash
export SLACK_BOT_TOKEN="xoxb-..."
export SLACK_TEAM_ID="T01234567"
# optional: comma-separated allowlist of channel IDs
export SLACK_CHANNEL_IDS="C01234567,C76543210"
```

**Authentication:** bot token. **Troubleshooting:** without `SLACK_TEAM_ID`
the server cannot resolve your workspace; to restrict which channels are
reachable, set `SLACK_CHANNEL_IDS` (unset means all public channels).

---

## 5. Notes

### 5.1 Obsidian — built-in MCP endpoint of the Local REST API plugin

Install the **Local REST API** community plugin in Obsidian and copy the API
key from its settings. The plugin serves an MCP endpoint over HTTP — a good
fit for Ene's HTTP transport, including the loopback opt-in:

1. In the plugin settings, enable the **HTTP server** (plain `http://` mode).
2. Declare the server with the API key as the `Authorization` header:

```jsonc
{
  "plugins": {
    "mcp_allow_insecure_urls": true,
    "mcp_servers": [
      {
        "name": "obsidian",
        "enabled": true,
        "transport": {
          "type": "http",
          "url": "http://127.0.0.1:27123/mcp/",
          "auth_header": "Bearer YOUR_API_KEY"
        }
      }
    ]
  }
}
```

**Authentication:** bearer API key, sent as the `Authorization` header —
which is exactly how Ene's `auth_header` is transmitted. **Troubleshooting:**
the HTTPS endpoint (`https://127.0.0.1:27124/mcp/`) uses a self-signed
certificate that Ene's HTTP client rejects, so use the plain-HTTP endpoint
with `mcp_allow_insecure_urls: true` (loopback stays isolated from the
network; the opt-in never relaxes link-local). If no tools appear, confirm
the plugin's HTTP server toggle is on and Obsidian is running. Alternatively,
a stdio bridge (`npx -y @connorbritain/obsidian-mcp-server` with
`OBSIDIAN_API_KEY` in `env_passthrough`) avoids the HTTP opt-in entirely.

### 5.2 Notion — `@notionhq/notion-mcp-server`

1. Create a Notion integration and copy its token (`ntn_…`).
2. Share the pages/databases with the integration.

```jsonc
{
  "plugins": {
    "mcp_servers": [
      {
        "name": "notion",
        "enabled": true,
        "transport": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "@notionhq/notion-mcp-server"]
        },
        "env_passthrough": ["NOTION_TOKEN"]
      }
    ]
  }
}
```

```bash
export NOTION_TOKEN="ntn_..."
```

**Authentication:** integration token. **Troubleshooting:** Notion's hosted
MCP (`https://mcp.notion.com/mcp`) is OAuth-only and does not accept bearer
tokens, so it cannot be reached through Ene's HTTP transport — the stdio
server with a token is the supported route. Tools cannot see pages the
integration has not been shared with.

---

## 6. Map

### Google Maps — `@modelcontextprotocol/server-google-maps`

1. Create a Google Maps Platform API key with the relevant APIs enabled
   (Geocoding, Places, Routes, Distance Matrix, …) and billing set up.
2. Restrict the key to those APIs where possible.

```jsonc
{
  "plugins": {
    "mcp_servers": [
      {
        "name": "google-maps",
        "enabled": true,
        "transport": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "@modelcontextprotocol/server-google-maps"]
        },
        "env_passthrough": ["GOOGLE_MAPS_API_KEY"]
      }
    ]
  }
}
```

```bash
export GOOGLE_MAPS_API_KEY="AIza..."
```

**Authentication:** API key. **Troubleshooting:** a `REQUEST_DENIED` tool
result usually means the key is restricted to APIs that are not enabled on
the project. Note this package comes from the reference MCP servers
collection, which is archived upstream; the package remains published on npm.
Actively maintained alternatives exist (e.g. `mcp-server-google-maps`, an
HTTP-streamable server with `GOOGLE_MAPS_API_KEY`); those that require a
custom `X-Api-Key` header cannot be authenticated through Ene's HTTP
transport, which only sends an `Authorization` header — prefer stdio servers
or omit the server-side auth token for local-only use.

---

## 7. RSS

### RSS — `rss-mcp`

An RSSHub-based reader (supports RSS and Atom, plus RSSHub routes for
services without native feeds). No API key required.

```jsonc
{
  "plugins": {
    "mcp_servers": [
      {
        "name": "rss",
        "enabled": true,
        "transport": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "rss-mcp"]
        },
        "env_passthrough": ["PRIORITY_RSSHUB_INSTANCE"]
      }
    ]
  }
}
```

```bash
# optional: prefer a private/stable RSSHub instance
export PRIORITY_RSSHUB_INSTANCE="https://my-rsshub.example.com"
```

**Authentication:** none. **Troubleshooting:** without
`PRIORITY_RSSHUB_INSTANCE` the server picks a public RSSHub instance, which
may rate-limit; pointing it at your own instance is the fix. Since this
server accepts feed URLs per tool call, the character can subscribe to
arbitrary feeds without configuration changes.

---

## 8. Common troubleshooting

| Symptom | Cause / fix |
|---|---|
| No MCP tools in `/tool list` | The server failed to connect and was skipped. Look for the "MCP server failed to connect" log entry (stdio) or "MCP HTTP connection failed" (HTTP). For stdio servers, run the declared `command`/`args` in a terminal by hand to see the server's own error. |
| `command not found: npx` | The child's environment only forwards a small allowlist from the host — `PATH` among them — so npx must be installed and findable. Pre-warm a slow first run (`npx -y <package>`) once manually so the package download is not part of Ene's startup. |
| Server starts but rejects requests ("missing API key") | The key was not forwarded: export it in the host environment **and** list it in `env_passthrough`. Ene defines no inline `env` map. |
| OAuth server needs a browser | Complete the server's interactive `auth` step in a terminal first; Ene spawns the server over stdio and cannot drive consent screens. |
| HTTP connection refused for a local server | Loopback and plain-`http://` are refused by default (SSRF guard). Set `plugins.mcp_allow_insecure_urls: true` for local development. |
| "auth header contains invalid characters" | `auth_header` must be a valid HTTP header value (e.g. `Bearer <token>`); the connection fails rather than silently dropping auth. |
| Server uses a non-`Authorization` header (e.g. `X-Api-Key`) | Ene's HTTP transport can only send the `Authorization` header — use a stdio variant of the server, or run it without its custom header for local-only use. |
| Tools appear but calls fail | The remote service rejects the credential (restricted key, missing test-user grant, unshared Notion page). Check the service's console/API docs. |
