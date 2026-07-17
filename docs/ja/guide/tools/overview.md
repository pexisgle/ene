# ツールカタログ

ツールは別バイナリとして動作し、IPC で ene と通信します。ツール名は `<namespace>.<action>` 形式の名前空間付きで表されます。

## 組み込み名前空間

| 名前空間 | アクション（要約） | バイナリ | ガイド |
|----------|--------------------|----------|--------|
| `filesystem` | `read`, `write`, `edit`, `delete`, `glob`, `grep`, `patch` | `ene-tool-fs` | [ファイルシステム](fs.md) |
| `shell` | `execute` | `ene-tool-fs` | [ファイルシステム](fs.md) |
| `app` | クリップボード、ウィンドウ、キーボード、マウス、スクリーンショットなど | `ene-tool-app` | [GUI 自動化](app.md) |
| `browser` | `navigate`, `click`, `type`, `wait`, … | `ene-tool-browser` | [ブラウザ](browser.md) |
| `web` | `fetch`, `search`（HTTP 経由の取得・検索） | `ene-tool-web` | [Web](web.md) |
| `utility` | `question`、todo、時刻、システム情報、`undo` | `ene-tool-utility` / `ene-tool-fs` | [ユーティリティ](utility.md) |

## 安全

パス制限、禁止シェルコマンド、undo: [セキュリティサンドボックス](sandbox.md)。

## 自分で追加する

手順: [ツールを書く](write-a-tool.md)。

## リファレンス（IPC、ホスト、RAG、SDK）

- [ツールシステム（IPC / ホスト）](../../reference/tools/overview.md)
- [Tool RAG（ツール検索）](../../reference/tools/tool-rag.md)
- [SDK](../../reference/tools/sdk.md)
- [Derive マクロ](../../reference/tools/derive-macro.md)
