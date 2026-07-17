# ツールカタログ

ツールは別バイナリとして動き、IPC で ene と通信します。名前は名前空間付き: `<namespace>.<action>`。

## 組み込み名前空間

| 名前空間 | アクション（要約） | バイナリ | ガイド |
|----------|--------------------|----------|--------|
| `filesystem` | `read`, `write`, `edit`, `delete`, `glob`, `grep`, `patch` | `ene-tool-fs` | [ファイルシステム](fs.md) |
| `shell` | `execute` | `ene-tool-fs` | [ファイルシステム](fs.md) |
| `app` | クリップボード、ウィンドウ、キーボード、マウス、スクリーンショットなど | `ene-tool-app` | [GUI 自動化](app.md) |
| `browser` | `navigate`, `click`, `type`, `wait`, … | `ene-tool-browser` | [ブラウザ](browser.md) |
| `web` | `fetch`, `search` | `ene-tool-web` | [Web](web.md) |
| `utility` | `question`、todo、時刻、システム情報、`undo` | `ene-tool-utility` / `ene-tool-fs` | [ユーティリティ](utility.md) |

## 安全

パス制限、禁止シェルコマンド、undo: [セキュリティサンドボックス](sandbox.md)。

## 自分で追加する

手順: [ツールを書く](write-a-tool.md)。

## リファレンス（IPC、ホスト、RAG、SDK）

- [ツールシステム（IPC / ホスト）](../../reference/tools/overview.md)
- [Tool RAG](../../reference/tools/tool-rag.md)
- [SDK](../../reference/tools/sdk.md)
- [Derive マクロ](../../reference/tools/derive-macro.md)
