# `ene-plugin` インターフェース

## 役割

プラグイン**作成ファサード**: プラグインバイナリが実装するトレイトと、ワイヤ
プロトコルを話すサーバー入口。プラグイン作者の 1 行インポートは
`use ene_plugin::prelude::*;` です。

## 公開モジュール

| モジュール | 内容 |
|---|---|
| `plugin` | トレイト: `ToolPlugin`・`LlmPlugin`・`EmbedPlugin`・`TtsPlugin`・`SttPlugin`・`VadPlugin`・`CapabilityProvider`・`ConfigurablePlugin`・ストリーミングチャンク型（`PluginStream`・`PluginStreamChunk`・`PluginCompletion`・`PluginTranscription`） |
| `action` | `ToolAction`・`ToolSpecArgs` |
| `tool_provider` | `ActionSetProvider`・`SingleActionProvider`・`ToolProviderPlugin`（レガシーアダプタ） |
| `server` | `PluginDispatch`・`run_plugin_server` |
| `capability` | `CapabilityClient`（ホストサービス `capability` パッセンジャークライアント） |
| `compat` | レガシー `ToolProvider` 用互換アダプタ |
| `prelude` | `prelude::tool`（アクション + マクロ）・`prelude::provider`（プロバイダートレイト + `ene-infer` 再エクスポート）・グロブ再エクスポート |

## 主要な再エクスポート（`ene-plugin-proto` と `ene-infer` から）

- ワイヤ型: `ToolSpec`・`ToolError`・`ToolResult`・`PluginError`・
  `PluginCapabilities`・プロバイダー仕様・`VersionRange`・`TokenUsage`・
  `DeferredStatus`・`SandboxConfigData`・`IpcListener`/`IpcStream`。
- ローカルモデル規律: `LocalModel`・`EngineHandle`・`EngineConfig`・
  `JobContext`・`StopReason`・`EngineError`（`prelude::provider` 経由）。

## 依存関係

- 依存: `ene-plugin-proto`・`ene-infer`・`ene-plugin-macros`。
- 利用: 全プラグインバイナリ（`plugins/tool/*`・`plugins/provider/*`）・
  `ene-plugin-host` テスト（dev）。

## リファクタリングの注目点

- **prelude が作者向け契約**です。そこでの再エクスポートがサポート対象面。
  追加は安全、削除は全プラグインを壊します。
- `PluginDispatch::new` は 5 つの位置引数（tool・llm・embed・tts・stt）。
  VAD と capability 仲介はビルダーステップ（`with_vad`・
  `with_capability_provider`・`with_capability_declarations`）。位置引数を
  増やさないでください。
- `ene-infer` の再エクスポートにより、ローカル推論プラグインはホスト自身の
  並行処理規律を使います（手書き `spawn_blocking` の防止）。
- プラグインクレートはバイナリのみ。ファサードトレイトを、プラグインコードが
  コンパイル対象とする唯一のライブラリ面に保ってください。
