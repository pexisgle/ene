# MCP servers

[Model Context Protocol](https://modelcontextprotocol.io) servers expose
tools to AI applications. Ene can attach any MCP server and expose its
tools to the character, alongside the built-in plugins.

## Configuration

Add entries to `tools.mcp_servers` in `settings.json`:

```json
{
  "tools": {
    "mcp_servers": [
      {
        "name": "github",
        "enabled": true,
        "transport": {
          "type": "stdio",
          "command": "npx",
          "args": ["-y", "@modelcontextprotocol/server-github"]
        },
        "env_passthrough": ["GITHUB_TOKEN"]
      },
      {
        "name": "docs",
        "enabled": true,
        "transport": {
          "type": "http",
          "url": "https://mcp.example.com/docs"
        }
      }
    ]
  }
}
```

### Transports

| Transport | How it connects |
|---|---|
| `stdio` | Spawns `command` with `args` as a child process |
| `http` | Connects to a streamable HTTP endpoint |

## Security model

- The MCP child process inherits **no** environment variables except those
  listed in `env_passthrough` — API keys your server needs must be named
  explicitly.
- HTTP URLs are validated against SSRF before the host connects.
- MCP tools participate in the normal permission system: side-effecting
  calls require approval like any other tool.
- MCP servers can be disabled per entry (`enabled: false`) without
  removing the configuration.

## How MCP tools appear

Registered MCP tools join the same tool registry as plugin tools, so they
are listed by `/tool list`, participate in tool-RAG selection, and are
callable directly:

```sh
/tool list | grep github
```

Tool names are namespaced by the server name for routing.

## Troubleshooting

- **Server fails to start** — run the command manually to check it launches
  (many failures are missing `npx`/`uvx` or missing passthrough env).
- **Tool not listed** — check `enabled: true`, restart the app, and verify
  the server's own startup logs.
- **Permission prompts** — approve once/session/permanent as usual.
