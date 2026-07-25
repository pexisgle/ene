# ツール SDK 関連クレート — API リファレンス

> **クレート**: `ene-tool-common` | `ene-plugin-db` | `ene-tool-derive` | `ene-tool-rag`

ツールプラグインの開発、ステートフルなストレージ操作、Proc-macro derive、および RAG によるツール探索を支援するライブラリクレート群です。

---

## 1. `ene-tool-common` (共通アクション定義 & プロバイダ)

ツールバイナリ向けの標準アクション定義およびプロバイダを提供します：
- `ToolAction`: 個々のツールアクションを非同期実行するためのトレイト。
- `ActionSetProvider`: 単一のプラグインバイナリ内で複数の `ToolAction` 型を登録・保持するコンテナ。

---

## 2. `ene-plugin-db` (ステートフルツール用 DB IPC クライアント)

状態を保持するツールプラグイン (`ene-plugin-fs`, `ene-plugin-utility`) は `ene-plugin-db` を使用してホストの `DbServer` ソケットと通信します：
- `UndoManager`: `ene-plugin-fs` 向けにファイルの変更・元戻しスタックを管理します。
- `TodoStore`: `ene-plugin-utility` 向けにアクティブな TODO 項目の CRUD 操作を管理します。

---

## 3. `ene-tool-derive` (Proc-Macro Derives)

ツール定義を簡略化するプロシージャルマクロを提供します：
- `#[derive(ToolAction)]`: `ToolSpec` メタデータ、JSON 引数のデシリアライズ、および `execute()` のボイラープレートコードを自動生成します。

---

## 4. `ene-tool-rag` (ツール検索拡張生成 RAG)

多ベクトル意味検索および字句検索によるツール発見機能を提供します：
- ツールの説明文およびアクションパラメータをインデックス化します。
- LLM 再ランクと重み付きフィールド類似度を使用して、トークン予算内で関連するツールのみをプロンプトパケットに注入します。

---

## 関連ドキュメント
- [プラグインと MCP システム](../concepts/plugins-and-mcp.md)
- [プラグインシステム関連クレート](plugin-system.md)
