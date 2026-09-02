# MCP servers

[Model Context Protocol](https://modelcontextprotocol.io) servers are not
vendored. Each handwritten row becomes a `mcp.<id>` fiber that runs
`ene-tool-mcp` and registers tools as `mcp:<id>.<tool>` on the same
`ene-tool-registry` pipeline as in-tree tools.

Write `<data-dir>/mcp.json` or use stage **Connections** (name / command-or-URL form +
`GET`/`PUT /api/v1/mcp`). Arguments are one argv entry per line so paths with
spaces stay a single argument. JSON import/export stays available for advanced
use.

```json
{
  "servers": [
    {
      "id": "git",
      "transport": "stdio",
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-git"],
      "enabled": true
    },
    {
      "id": "remote",
      "transport": "http",
      "url": "https://mcp.example.invalid/mcp",
      "enabled": true
    }
  ]
}
```

`transport` is `stdio` (spawn `command` + `args`) or `http` (Streamable HTTP
JSON-RPC POST to `url`). Stdio follows the MCP spec via `rmcp`: one JSON-RPC
message per line. After `tools/list`, the bridge also calls
`resources/list` (snapshots under `<workspace>/mcp-context/`, injected as
`mcp.resources`) and `prompts/list` (written as `SKILL.md` under the data-dir
skills home). Servers that omit those methods are fine. There is no
settings-page marketplace picker yet (successor, P-616). Do not add a
`tools.mcp_servers` block copied from the old in-process host — that schema
is gone.

## Curated catalog

Stage **Connections** also shows a curated catalog of official servers (git,
fetch, memory, and the remote GitHub MCP server). Each entry previews the
command, arguments, URL, required auth, side effects, and upstream source URL
before you connect. **Connect** first opens a one-shot probe connection that
never touches `mcp.json`: it lists the tools the server actually exposes, with
per-tool side effects. Adding from that preview writes a normal row into the same
handwritten document, disabled until you enable it — exactly one schema.

The catalog is a static allowlist compiled into `ene-work`
(`GET /api/v1/mcp/catalog`). It carries no signatures yet; distribution and
signing are a later design. The GitHub remote entry expects manual bearer-token
entry: you create your own personal access token (or another bearer token) and
paste it once via the plugin config secret field
(`vault.plugin.config.mcp.<id>.auth_token`). The bridge injects it as an
`Authorization` header and it never appears in raw JSON or logs. If the server
rejects the credential, stage surfaces an "auth required" connection state so
you know to update the token.
