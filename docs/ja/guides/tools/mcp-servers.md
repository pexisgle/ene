# MCP サーバー

[Model Context Protocol](https://modelcontextprotocol.io) サーバーはベンダーしません。
手書きの各行は `mcp.<id>` ファイバーになり、`ene-tool-mcp` が
`mcp:<id>.<tool>` として内製ツールと同じ `ene-registry` パイプラインに載せます。

`<data-dir>/mcp.json` を書くか、stage の **接続**（名前 / コマンドまたは URL のフォームと
`GET`/`PUT /api/v1/mcp`）を使います。引数は 1 行が 1 引数なので、空白を含む
パスも分割されません。JSON の取り込み / 書き出しは詳細用に残しています。

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
`resources/list`（`<workspace>/mcp-context/` にスナップショットし、`mcp.resources`
として注入）
と `prompts/list`（data-dir の skills 配下へ `SKILL.md`）も呼びます。
それらのメソッドが無いサーバーはそのままで構いません。
代表的なサーバーを選ぶマーケット UI はまだありません（後継、P-616）。
旧ホストの `tools.mcp_servers` ブロックをコピーしないでください。そのスキーマは
無くなっています。

## カタログ

stage の **接続** には、公式サーバー（git / fetch / memory / リモート GitHub MCP）の
厳選カタログも出ます。各エントリは接続前にコマンド・引数・URL・必要な認証・副作用・
上流ソース URL を表示し、**追加** で同じ手書きドキュメントへ通常行として書き込みます。
スキーマは一つのままです。

カタログは `ene-work` に組み込まれた静的許可リスト（`GET /api/v1/mcp/catalog`）です。
署名はまだなく、配布と署名は別設計です。GitHub リモートはサーバー自身の OAuth 2.0
フローを使い、ホストはプロバイダー固有のクライアント秘密を持ちません。保存後、
プラグイン設定のシークレット（`vault.plugin.config.mcp.<id>.token`）にトークンを一度
貼り付けると、ブリッジが `Authorization` ヘッダーとして注入します。生 JSON やログに
トークンは出ません。
