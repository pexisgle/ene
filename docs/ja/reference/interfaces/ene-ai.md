# `ene-ai` インターフェース

## 役割

AI プロバイダー層: 汎用メッセージ/ストリーミング型・プロバイダートレイト・
タスクルーティング・リトライ/フェイルオーバーポリシー・コンテキスト窓計算・
モデル取得。具象プロバイダーはプラグインとして出荷され、`ene-ai` は契約を
定義します。

## 公開モジュール

| モジュール | 内容 |
|---|---|
| `traits` | `LlmProvider`・`EmbeddingProvider`・`TtsProvider`・`SttProvider`・`VadEngine`・`ProviderHost`・ファクトリトレイト・`cosine_similarity`・`embed`/`embed_query` |
| `message` | `LlmMessage`・`LlmResponseChunk`・`LlmToolCall(Chunk)`・`LlmCompletion`・`UserMessagePart` |
| `config` | `AiConfig`・`AiProviderDef`・`AiTasksConfig`・`ApiKeyConfig`・`LocalModelDef`・`RetryConfig`・`FallbackConfig`・`SttConfig`・`TtsConfig`・`VadConfig`・`BUILTIN_PROVIDER_KINDS`・`canonical_provider_kind` |
| `resolve` | `ResolvedChat`・`ResolvedEmbedding`・`ResolvedTts/Stt/Vad`・`probe_provider_health`・`probe_chat_candidates`・`select_healthy_chat`・`validate_settings`・`validate_api_key`・`fetch_model_ids`・`needs_onboarding` |
| `routing` | `AiTaskKind`・`create_chat_provider_for_task`・`create_task_chat_provider` |
| `retry` | `RetryPolicy` |
| `context_window` | `effective_window`・`EffectiveWindow`・`DEFAULT_CONTEXT_WINDOW` |
| `model_fetch` | `ModelFetcher`・`ModelValidator` 各種・`validate_https_url` |
| `engine_adapter` | `LocalLlmEngine`・`LocalTtsEngine`・`LocalSttEngine`・`EngineDescriptor`・`ResourceRegistry`・`ResourceClass`・`CapabilitySet`（`ene-infer` へのブリッジ） |
| `plugin_config` | `plugins.list.<name>` に移設されたプロバイダー固有設定 |
| `error` / `role` | `AiError`・`LlmProviderError`・`Role`（User/Assistant/System/Tool） |

## 主要な再エクスポート

- `TokenUsage` を `ene-plugin-proto` から再エクスポート — プロセス内
  プロバイダー・IPC ブリッジ・ワイヤ形式がひとつの定義を共有します。

## 依存関係

- 依存: `ene-config`・`ene-infer`・`ene-plugin-proto`。
- 利用: `ene-mind`・`ene-runtime`・`ene-plugin-host`・`ene-voice`・
  プロバイダープラグイン・`ene-cli`・`ene-desktop`。

## リファクタリングの注目点

- `ProviderHost` が**レジストリの継ぎ目**です。`ene-plugin-host` が実装し、
  `resolve`/`routing` はプロバイダーを所有せず参照します。タスク→プロバイダー
  結合とフェイルオーバーポリシーはホストではなくここにあります。
- プロバイダー kind の追加 = プロバイダープラグイン + 組み込み kind エントリ
  （`BUILTIN_PROVIDER_KINDS`）+ kind 固有設定。タイポ提案も UX 契約の一部です。
- `engine_adapter` モジュールは `ene-infer` の同期ローカルモデル基盤を非同期
  プロバイダートレイトに橋渡しします。プロバイダーで手書きの並行処理を
  追加せず、必ずこの経路を通してください。
- `effective_window`（公表窓と設定窓・応答予約・安全マージン）は mind の
  予算計算と共有されます。テスト付きで変更してください。
