# アーキテクチャ概要

ene は API v1 ホスト契約（`ene-runtime`）と `ene-mind` 認知ターンパイプラインを中心としたモジュール型 Rust ワークスペースです。

## ランタイムアーキテクチャ

実行シェルはアクターモデル（`EneHandle` / actor）のまま、ターン知能は `ene-mind` が所有します。

### コアターンフロー

```text
User input
  -> before_turn (recall planning + affect update; Tool RAG / style / scene prefetch と並列)
  -> compose_prompt_packet (sectioned context + budgeting; pre-turn affect persist と並列)
  -> LLM streaming
  -> output arbitration (Performance cues)
  -> finalize_turn (affect persist; 同期)
  -> セッション履歴をコミット
  -> Terminal（チャットイベント）
  -> 遅延 write_memories + forgetting + affect 分類（バックグラウンド）
```

`ene-runtime` がこのフローを統合し、**最小**のチャットイベントバスを発行します。診断は別経路です。

## 目標クレートマップ（API v1）

| クレート | 役割 |
|---|---|
| `ene-runtime` | Ready `EneHandle::open`、`TurnId`、single-flight Busy、チャットイベント、diagnostics facade |
| `ene-mind` | Identity、型付きメモリ方針、affect、Performance 調停、compression、セッション状態 |
| `ene-store` | SQLite-vec 永続化のみ（`store.enabled` / `store.db_path`） |
| `ene-ai` | LLM + batch-only 埋め込みプロバイダ |
| `ene-plugin-proto` / `ene-plugin` / `ene-plugin-host` | wire プラグイン ABI (v3)、作成ファサード、プロセス/レジストリ管理 |
| `ene-config` | 設定、キャラクターカード、パス |
| `ene-vrm` | VRM レンダリング（mind/runtime 依存なし） |

ロック事項と依存グラフは [API v1](api-v1.md) を参照。

## メモリモデル

型付きメモリ（`episodic`、`semantic`、`preference`、`commitment` など）とライフサイクル状態。コミットメントは ledger が唯一の SoT。ハイブリッド recall は **mind** が実行し、**store** はテキスト / 任意の事前計算ベクトル / フィルタのみを受け取る。

## プロンプトモデル

`PromptPacket` によるセクション分割と明示予算。予算圧下でも Identity / output-contract は保護される。

## 感情と Performance

- Affect 状態はエンジン側で永続化。
- 最終的な提示 cue は `EneEvent::Performance`（単独の `SpecialToken` / `Expression` ではない）。
- `PerformanceCue` は `ene-mind` 所有；desktop が VRM 再生へ変換し、`ene-vrm` に mind 型を持ち込まない。

## アプリケーション

- `ene-cli`: `ConfigStore::try_load` → card → `EneHandle::open`；REPL + diagnostics。
- `ene-desktop`: 必要時 soft config load → `open`；VRM + Performance 消費。

## プラグインシステム

統合プラグインシステムは唯一のプロセス外拡張機構です。全ツールバイナリ（`plugins/tool/*`）、LLM プロバイダプラグイン（`plugins/ene-plugin-*`）、MCP サーバーは、IPC プロトコル v3 上で単一の `PluginHostManager` が管理します。

### クレート

| クレート | 役割 |
|---|---|
| `ene-plugin-proto` | ワイヤプロトコル v3: `PluginCapabilities`、`LlmProviderSpec`、ツール型（`tool_types`）、ストリーミング IPC メッセージ |
| `ene-plugin` | 作成ファサード: `Plugin` trait、`ToolPluginAdapter`（`ToolProvider` をラップ）、`run_plugin_server` エントリポイント |
| `ene-plugin-host` | ホスト側: `PluginHostManager`（プロセス管理、MCP、サーキットブレーカー、ヘルス）、`ToolRegistry`/`CompositeToolRegistry`、`IpcLlmProvider`、`IpcLlmProviderFactory` |

### IPC プロトコル v3

プラグイン IPC はレガシーのツール IPC と同じ 4 バイトリトルエンディアンの長さプレフィックス JSON フレーミングを使用し、ストリーミングとリッチなハンドシェイクで拡張します:

- **ハンドシェイク**: ホストが `Handshake { version: 3, plugin_config }` を送信し、プラグインが `HandshakeAck { version, capabilities }` で応答。
- **ストリーミング**: 1 リクエスト → N 個の `StreamChunk` → 終端 `StreamEnd` または `StreamError`、`request_id` で相関。
- **ケーパビリティ**: `PluginCapabilities` が `tools`、`llm_providers`、将来の `tts_providers` / `stt_providers` を宣言。

ツールバイナリもプラグインです: `ToolProvider` は `ToolPluginAdapter` でラップされ `run_plugin_server` 経由でサーブされ、`capabilities.tools` でツールスペックを通知します。

### プロセス管理

`PluginHostManager` が全プラグインを管理する唯一のマネージャです:

- `builtin_plugins_dir()` + `user_plugins_dir()` からプラグインバイナリを探索（命名: `ene-plugin-{name}`）
- 各バイナリを `ENE_PLUGIN_SOCKET` 環境変数付きで子プロセスとして起動
- v3 ハンドシェイクを実行しケーパビリティを検査
- `capabilities.tools` → `ToolRegistry` アダプタ、`capabilities.llm_providers` → `IpcLlmProviderFactory` にルーティング
- 設定された MCP サーバー（`plugins.mcp_servers`）に接続し、統合ツールレジストリにマージ
- サーキットブレーカーと指数バックオフ再起動付きの定期ヘルスプローブ（最大 5 回）

### LLM プロバイダ統合

プラグイン提供の LLM プロバイダはグローバル `LlmProviderRegistry` 経由で統合:

1. `PluginHostManager::start` が各 `llm_providers` kind に対して `IpcLlmProviderFactory` を登録。
2. `EneHandle::open` がこれらのファクトリを `LlmProviderRegistry` にマージ。
3. `resolve.rs` が `openai_compatible` 以外のプロバイダ kind をレジストリ経由でルーティング。
4. `IpcLlmProvider` が IPC ストリーミングプロトコルを `LlmProvider` trait にブリッジ。

### 設定

`plugins` 設定セクション（`plugins.enabled`、`plugins.list.<name>.enable`）がシステムを制御します。[設定](../configuration/settings.md#plugins--プラグインシステム)を参照。

## 参照

- [API v1 ADR](api-v1.md)
- [認知ランタイム ADR](cognitive-runtime.md)
- [アバター Performance ADR](avatar-performance.md)
- [能動発話 ADR](proactive-speech.md)
