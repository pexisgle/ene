# MCP サーバー

[Model Context Protocol](https://modelcontextprotocol.io) サーバーはベンダーしません。
手書きの各行は `mcp.<id>` ファイバーになり、`ene-harness-mcp` が
`mcp:<id>.<tool>` として内製ツールと同じ `ene-registry` パイプラインに載せます。

`<data-dir>/mcp.json` を書くか、**コネクター**（JSON エディタと
`GET`/`PUT /api/v1/mcp`）を使います。

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

`transport` は `stdio`（`command` + `args` を spawn）か `http`（`url` への
Streamable HTTP JSON-RPC POST）です。代表的なサーバーを選ぶマーケット UI は
まだありません（後継、P-616）。旧ホストの `tools.mcp_servers` ブロックを
コピーしないでください。そのスキーマは無くなっています。
