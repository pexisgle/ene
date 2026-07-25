# プラグインシステム関連クレート — API リファレンス

> **クレート**: `ene-plugin-proto` (Protocol v4 ワイヤー定義) | `ene-plugin` (開発用 SDK) | `ene-plugin-host` (プロセススーパーバイザ)

これらのクレート群は、Ene の統一されたプロセス外 IPC プラグイン基盤を形成します。

---

## 1. `ene-plugin-proto` (ワイヤープロトコル v4)

`ene-plugin-proto` は IPC ワイヤー型、メッセージ enum、パケットフレーミング、およびハンドシェイクデータ構造を定義します：

- **`PluginIpcRequest` / `PluginIpcResponse`**: Protocol v4 の全リクエスト/レスポンス。
- **`VersionRange`**: ハンドシェイクネゴシエーション用バージョン範囲 (`min: u32, max: u32`)。
- **`PluginCapabilities`**: プラグインの機能宣言 (`tools`, `llm_providers`, `stt_providers`, `tts_providers`)。
- **`ToolSpec`**: ツールおよびその引数の JSON スキーマ定義。

---

## 2. `ene-plugin` (プラグイン開発 SDK)

`ene-plugin` は新しいツールやプロバイダプラグインを構築するためのファサードクレートです：

- **`ToolPluginAdapter`**: `ActionSetProvider` や `ToolProvider` を IPC プラグインにラップします。
- **`run_plugin_server`**: `stdin`/`stdout` 上で IPC リクエストを処理する非同期エントリポイント。
- **`prelude`**: 便利 re-export (`ToolAction`, `ToolError`, `ActionSetProvider`, `run_plugin_server`)。

---

## 3. `ene-plugin-host` (プロセススーパーバイザ)

`ene-plugin-host` はホスト側で子プラグインプロセスの監視を実行します：

- **`PluginHostManager`**: プラグインバイナリの起動、Protocol v4 ハンドシェイクネゴシエーションの実行、ライフサイクル管理。
- **サーキットブレーカー**: 失敗したプラグインプロセスを検知し、バックオフ再起動を適用。
- **`CompositeToolRegistry`**: 組み込みプラグイン、プロセス外プラグイン、および MCP サーバーからのツール仕様を単一の検索レジストリに統合。

---

## 関連ドキュメント
- [プラグインと MCP システム](../concepts/plugins-and-mcp.md)
- [ツール SDK リファレンス](tool-sdk.md)
