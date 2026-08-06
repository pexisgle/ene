# MCP サーバー

[Model Context Protocol](https://modelcontextprotocol.io) サーバーはツールを
AI アプリケーションに公開します。Ene は任意の MCP サーバーを接続し、その
ツールを組み込みプラグインと並べてキャラクターに公開できます。

## 設定

`settings.json` の `tools.mcp_servers` にエントリを追加します:

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

### トランスポート

| トランスポート | 接続方法 |
|---|---|
| `stdio` | `command` を `args` 付きで子プロセスとして起動 |
| `http` | ストリーミング HTTP エンドポイントに接続 |

## セキュリティモデル

- MCP 子プロセスは `env_passthrough` に列挙した環境変数**以外**を一切
  継承しません。サーバーが必要な API キーは明示的に列挙してください。
- HTTP URL はホストが接続する前に SSRF に対して検証されます。
- MCP ツールは通常の権限システムに参加します。副作用のある呼び出しは
  他のツールと同様に承認が必要です。
- MCP サーバーはエントリごとに無効化できます（`enabled: false`）。
  設定を削除する必要はありません。

## MCP ツールの見え方

登録された MCP ツールはプラグインツールと同じツールレジストリに入ります。
`/tool list` に表示され、ツール RAG 選択に参加し、直接呼び出せます:

```sh
/tool list | grep github
```

ツール名はルーティング用にサーバー名で名前空間化されます。

## トラブルシューティング

- **サーバーが起動しない** — コマンドを手動で実行して起動できるか確認
  （多くは `npx`/`uvx` がないか、パススルー環境変数の欠落が原因）。
- **ツールが表示されない** — `enabled: true` を確認し、アプリを再起動し、
  サーバー自身の起動ログを確認。
- **権限プロンプト** — 通常どおり一度/セッション/永続で承認します。
