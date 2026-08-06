# `ene-plugin-proto` インターフェース

## 役割

**ワイヤ ABI**: プラグイン IPC プロトコル v7・ツール型・capability 宣言・
ホストサービスのフレーミング・トランスポート。ビジネスロジックは決して
置かれません。

## 公開モジュール

| モジュール | 内容 |
|---|---|
| `ipc` | `PLUGIN_IPC_PROTOCOL_VERSION`（7）・`PLUGIN_IPC_MIN_SUPPORTED_VERSION`・`VersionRange`・`PluginIpcRequest`/`PluginIpcResponse`・フレーミングヘルパー（`read/write_plugin_request/response`）・`ConfigFieldError`・`ConfigOption`・`VadEvent` |
| `tool_ipc` | ツール IPC v2: `IpcRequest`/`IpcResponse`・`CallContext`・`DeferredStatus`・`ToolConfigAccessor`・`IPC_PROTOCOL_VERSION` |
| `tool_types` | `ToolSpec`・`ToolCategory`・`ToolName`・`ToolRagProfile`・`ToolResult` |
| `tool_error` | `ToolError`・`ErrorKind`・対話型プロンプト型（`UserInputPrompt`・`QuestionItem`・`MultiAnswer`） |
| `tool_provider` | `ToolProvider` トレイト（レガシーツールバイナリ契約） |
| `capabilities` | `PluginCapabilities`・`LlmProviderSpec`・`TtsProviderSpec`・`SttProviderSpec`・`VadProviderSpec`・`CapabilityRef`・`CapabilityRequirement`・`ConcurrencyHint`・`ResourceClass`・`DEFAULT_SAMPLE_RATE` |
| `capability_service` | `CapabilityCall(Result/Error)`・`CapabilityServiceHandler`・パッセンジャーフレーミングヘルパー |
| `host_service` | `HostServiceId`・`HostServiceRequest/Response`・`HOST_SERVICE_MAX_MESSAGE_SIZE`・フレーミングヘルパー |
| `sandbox` | `SandboxConfigData` |
| `transport` | `IpcStream`・`IpcListener`・`cleanup_path`（UDS / 名前付きパイプ） |
| `usage` | `TokenUsage` |
| `error` | `PluginError`・`ProviderErrorKind` |

## 依存関係

- 依存: 内部なし（serde・rmp-serde・tokio・thiserror など）。
- 利用: `ene-plugin`・`ene-plugin-host`・`ene-plugin-db`・`ene-store`
  （DB IPC）・`ene-ai`（共有 `TokenUsage`）・`ene-plugin-macros`・
  プロバイダー/ツールプラグイン。

## リファクタリングの注目点

- **追加のみ。** `#[serde(default)]` フィールドと新しい列挙バリアントを
  優先し、リネーム/削除は避けてください。挙動はネゴシエーション済み
  プロトコルバージョンでゲートします（パターン: `supports_vad()`）。前提に
  しないでください。
- ホストは `VersionRange { min: N-1, max: N }` を広告し、プラグインはビルド
  時の単一バージョンを宣言します。プロトコルを上げるときは最小サポート
  バージョンも同じ量だけ上げます。
- フレームは 4 バイト・リトルエンディアン長さプレフィックス。ハンドシェイクは
  JSON、以降はネゴシエーション済みワイヤ形式（v6 以降 MessagePack）。
  フレーミングとバージョンネゴシエーションはここに置き、プラグイン側で
  再実装させないでください。
- このクレートはビジネスロジックの居場所ではありません。意味的なもの
  （ルーティング・ワイヤ形状を超える検証）はホストかドメインクレートへ。
