# MCP サーバー

[Model Context Protocol](https://modelcontextprotocol.io) サーバーはベンダーしません。
手書きの各行は `mcp.<id>` ファイバーになり、`ene-tool-mcp` が
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
Streamable HTTP JSON-RPC POST）です。`tools/list` のあと、ブリッジは
`resources/list`（`<workspace>/mcp-context/` にスナップショットし、コンテキスト源へ注入）
と `prompts/list`（data-dir の skills 配下へ `SKILL.md`）も呼びます。
それらのメソッドが無いサーバーはそのままで構いません。
代表的なサーバーを選ぶマーケット UI はまだありません（後継、P-616）。
旧ホストの `tools.mcp_servers` ブロックをコピーしないでください。そのスキーマは
無くなっています。
