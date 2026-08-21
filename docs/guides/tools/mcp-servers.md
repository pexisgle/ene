# MCP servers

[Model Context Protocol](https://modelcontextprotocol.io) servers are not
vendored. Each handwritten row becomes a `mcp.<id>` fiber that runs
`ene-harness-mcp` and registers tools as `mcp:<id>.<tool>` on the same
`ene-registry` pipeline as in-tree tools.

Write `<data-dir>/mcp.json` or use **Connectors** (JSON editor +
`GET`/`PUT /api/v1/mcp`):

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
JSON-RPC POST to `url`). After `tools/list`, the bridge also calls
`resources/list` (snapshots under `<workspace>/mcp-context/`, injected as
`mcp.resources`) and `prompts/list` (written as `SKILL.md` under the data-dir
skills home). Servers that omit those methods are fine. There is no
settings-page marketplace picker yet (successor, P-616). Do not add a
`tools.mcp_servers` block copied from the old in-process host — that schema
is gone.
