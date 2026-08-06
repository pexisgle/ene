# プラグイン IPC プロトコル

このページでは、ホストとプラグインバイナリ間のワイヤプロトコルを説明します。
正規実装は `crates/ene-plugin-proto/src/ipc.rs` で、このページは読みやすい
要約です。

## バージョン

現在のプロトコルバージョン: **7**（`PLUGIN_IPC_PROTOCOL_VERSION`）。

- ホストは `VersionRange { min: N-1, max: N }`（N-1 後方互換）を広告します。
  プラグインは自分がビルドされた単一バージョンを宣言します。
- ネゴシエーションされるバージョンは共通の最高値です。範囲が重ならないと
  ハンドシェイクは失敗します。
- v7 でプロセス外 VAD（`ProcessVadChunk`）が追加され、v6 でハンドシェイク後の
  フレームが JSON から MessagePack に変わりました。ホストは新しいリクエスト
  種別をネゴシエーション済みバージョンでゲートするため、古いプラグインに
  届くことはありません。

## フレーミング

すべてのフレームは:

```text
4 バイト・リトルエンディアン長さプレフィックス | ペイロード
```

- ハンドシェイク交換: 常に JSON。
- ハンドシェイク後のフレーム: ネゴシエーション済み `WireFormat`
  （v6 以降は MessagePack、v5 以前は JSON）。
- 最大メッセージサイズ: 64 MiB。
- すべてのメッセージ（ストリーミング含む）は相関用の `request_id`（UUID）
  を持ちます。

## ハンドシェイク

```text
ホスト  ── サポート範囲 + ハンドシェイクリクエスト ──▶ プラグイン
プラグイン ── ネゴシエーション済みバージョン + PluginCapabilities + ack ──▶ ホスト
```

`PluginCapabilities` は次を広告します:

- ツール（数 + RAG プロファイル）、
- LLM プロバイダー（`LlmProviderSpec`: kind・モデル・ストリーミング・ビジョン・
  コンテキスト窓・並行性）、
- TTS/STT/VAD プロバイダー仕様（ボイス・形式・サンプルレート・フレームサイズ）、
- capability 宣言（`provides` / `requires`）、
- リソースクラスと受付ヒント。

## メッセージ

| クラス | 例 |
|---|---|
| ツール IPC（v2 系） | `ToolCall` / `ToolResult` / `ToolError`（構造化・IPC 直列化可能）、`ToolSpec` |
| プラグイン IPC（v7） | `CreateChatStream`・`StreamChunk`・`StreamEnd`・`StreamError`・埋め込み・`SynthesizeTts`・`Transcribe`・`ProcessVadChunk` |
| 遅延タスク | `DeferredStatus` — バックグラウンドツール完了の非同期通知 |
| ホストサービス | `HostServiceRequest` / `HostServiceResponse` — 共有ソケット上の多重化パッセンジャー |
| capability 呼び出し | `CapabilityCall` — ホスト経由のプラグイン間仲介 |

## トランスポート

- Unix は Unix ドメインソケット、Windows は名前付きパイプ
  （`IpcListener` / `IpcStream` / `cleanup_path`）。
- ホストはサンドボックス設定（`SandboxConfigData`）も渡します。プラグインの
  作業ディレクトリ・権限コンテキスト・リソース制限を記述します。

## ホストサービス（`db` と `capability`）

状態保持プラグインは **`db` パッセンジャー**経由でホストのデータベースに
到達します。`ene-plugin-db` が共有ホストサービスソケット上で型付き CRUD
（list/insert/update/delete/search）を提供します。認証はプラグインごとの
トークンで、各プラグインのテーブルはプレフィックス分離されます。

**`capability` パッセンジャー**はプラグイン間の capability 呼び出しを仲介
します。呼び出し側の宣言済み `requires` がリクエストを許可し、ホストが
capability レジストリからプロバイダーを解決し、プロバイダーの接続へ転送
します。

## ホスト側のバージョンゲート

ホストは新機能を `negotiated_version()` チェックでガードします
（例: `supports_vad()`）。v6 と v7 の混在したプラグインフリートが 1 つの
ホストで動作します。プロトコルを上げるときは最小サポートバージョンも同じ
量だけ上げ、最も古いバージョンのサポートを落とします。

## 作成

このプロトコルを手書きする必要はありません。プラグインバイナリは
`ene_plugin::run_plugin_server` と `PluginDispatch`（ツール + プロバイダー
トレイト）を使い、ホストは `ene-plugin-host` の `PluginHostManager` を使います。
[ツール SDK](tools/sdk.md) と [derive マクロ](tools/derive-macro.md) を参照。
