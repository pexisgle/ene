# `ene-plugin-host` インターフェース

## 役割

ホスト側のプラグイン監視と capability ルーティング: 発見・起動・
ハンドシェイク・ヘルス・サーキットブレーカー・IPC プロバイダーブリッジ・
MCP クライアント・単一のプロバイダーレジストリ。

## 公開モジュール

| モジュール | 内容 |
|---|---|
| `manager` | `PluginHostManager`（ライフサイクル・レジストリ・`ProviderHost` 実装）・ファクトリハンドル型 |
| `tool_registry` | `ToolRegistry` トレイト・`CompositeToolRegistry`・`DeferredCallResult`・`compute_tool_version_hash` |
| `ipc_plugin` | `IpcPluginConnection`（1 プラグインバイナリへのクライアント）・`SetConfigOutcome` |
| `ipc_provider` / `ipc_stt` / `ipc_tts` / `ipc_vad` / `embedding` | `ene-ai` トレイトを実装する IPC プロバイダーブリッジ |
| `factory` / `stt_factory` / `tts_factory` | プロバイダーファクトリアダプタ |
| `capability_registry` | `CapabilityRegistry`・`CapabilityDeclaration`・`evaluate_capability_gate` |
| `capability_service` | `CapabilityMediator`・`CapabilityCallHandler`（プラグイン間仲介） |
| `mcp_config` / `mcp_registry` | `McpServerConfig`・`McpTransport`・`McpToolRegistry` |
| `config` | `PluginConfig`・`PluginEntry`（プラグインごとの enable/config/profiles） |
| `credential_registry` | `CredentialRegistry`（x-ene-credentials 解決） |
| `circuit_breaker` | `CircuitBreaker`・`BreakerState` |
| `health` | `PluginHealthEvent`・`DisabledReason` |
| `redact` | `redact_config`・`redact_config_unschematized` |
| `wav` | プロバイダー音声向け共有 WAV エンコード/デコード |
| `admission`・`error` | リソースクラス受付・`PluginHostError`・`ToolHostError` |

## 依存関係

- 依存: `ene-plugin-proto`・`ene-ai`・`ene-config`・`ene-connector`。
- 利用: `ene-runtime`・`ene-cli`・`ene-desktop`。

## リファクタリングの注目点

- `PluginHostManager` は `ene_ai::ProviderHost` を実装します — **唯一の
  プロバイダーレジストリ**（LLM/埋め込み/TTS/STT/VAD）です。タスク結合と
  フェイルオーバーは `ene-ai` に残し、ここやコンシューマにレジストリを
  複製しないでください。
- ヘルスプローブは各プロバイダープラグイン**経由**（最小チャット ping）で
  行われ、ホスト側の HTTP プローブではありません。エンドポイント知識は
  プラグインが持ちます。
- `BUILTIN_PLUGIN_NAMES` とデフォルトプラグイン一覧が発見の契約です。
  プラグイン追加時は両方とパッケージングスクリプトを更新してください。
- ホストはプラグイン設定値のリダクション境界かつ資格情報解決点です。
  シークレットが未リダクションで越えてはいけません。
- MCP 子プロセスは `env_passthrough` 以外の環境変数を継承せず、HTTP URL は
  接続前に SSRF 検証されます。レジストリ経路にこれらの検査を残してください。
