# クレートインターフェース — 概要

このセクションでは、各クレートの**公開インターフェース**（公開モジュール・
型・トレイト・関数）を一覧し、リファクタリング時に「変更が何に波及するか」を
一目で把握できるようにします。正確なシグネチャは rustdoc
（`cargo doc -p <crate> --open`）が正とし、このページ群はインターフェースの
存在理由と全体像を示します。

## レイヤー

```text
apps             ene-desktop, ene-cli
host facade      ene-runtime
cognitive        ene-mind
persistence      ene-store ──（唯一の DB 所有者）
vocabulary       ene-core（型 + ポートトレイト）
policy           ene-rag, ene-util
provider layer   ene-ai, ene-infer, ene-voice
plugin stack     ene-plugin-proto, ene-plugin, ene-plugin-macros,
                 ene-plugin-host, ene-plugin-db
external         ene-connector, ene-vrm
```

## インターフェースの種類

| 種類 | 場所 | リファクタリングの影響 |
|---|---|---|
| **ホスト契約（API v1）** | `ene-runtime::public_api` | 方針として安定。変更にはバージョンアップが必要（[API v1](../architecture/api-v1.md)参照） |
| **ポートトレイト** | `ene-core`（`MemoryPort`・`EmbeddingStorePort`・`WorkspaceDocumentPort`・`ToolFailureSignalPort`） | mind/runtime と store の継ぎ目。ポートのメソッド変更は両側に波及 |
| **ワイヤ ABI** | `ene-plugin-proto`、`ene-plugin-db` のメッセージ | 追加のみ。フレームやバリアントの破壊的変更は不可 |
| **作成ファサード** | `ene-plugin`（トレイト・`prelude`・`run_plugin_server`） | 全プラグインバイナリがコンパイル対象とする契約 |
| **プロバイダートレイト** | `ene-ai`（`LlmProvider`・`EmbeddingProvider`・`TtsProvider`・`SttProvider`・`VadEngine`・`ProviderHost`） | プラグインホストとプロバイダープラグインが実装 |
| **ローカルモデル基盤** | `ene-infer`（`LocalModel`・`EngineHandle`） | ローカルエンジン（llama.cpp・whisper・ONNX）が実装 |
| **設定スキーマ** | `ene-config`（`define_config!`） | キー追加は安全。削除/リネームはマイグレーションが必要 |
| **純粋ポリシー/ユーティリティ** | `ene-rag`・`ene-util` | I/O なし。純粋関数のシグネチャの背後で安全に変更可能 |

## インターフェースを守る依存ルール

- `ene-core` は内部に依存しない — `ene-store` と `ene-mind` の両方が互いに
  依存せずに利用できます。
- `ene-mind` は `ene-runtime`・`ene-plugin-host`・`ene-store`（本番）に依存
  せず、永続化には `MemoryPort` 経由でのみアクセスします。
- `ene-store` は `ene-ai`・`ene-mind` に依存しません。
- `ene-rag` のスコアリング核は `ene-core` のみに依存し、ツールパイプラインは
  `tool` フィーチャーでゲートされます（`ene-ai` を導入）。
- `ene-connector` は意図的に `ene-config`・`ene-plugin-proto` に依存せず、
  プラグインバイナリが設定/プロトコルの重みを引きずらずに資格情報型を
  使えるようにします。
- `ene-plugin-proto` はワイヤ ABI のみ。ビジネスロジックは一切置きません。
- `ene-vrm` は独立（mind/runtime/store の型なし）。

## リファクタリングの指針

1. **依存エッジに従う。** 型を依存エッジをまたいで移動すると全コンシューマが
   影響を受けます。まず所有クレート内での変更を検討してください。
2. **ポートが継ぎ目。** `ene-core` のトレイトは永続化実装を差し替え可能に
   するためにあります（テストダブル・新 DB）。呼び出し側ではなくトレイトを
   拡張してください。
3. **ワイヤ変更は追加のみ。** 新しい `#[serde(default)]` フィールドと列挙型
   バリアントの追加は安全。リネーム・削除は破壊的変更です。
4. **コンパイル時契約は重要。** `public_dto_fields_are_primitive_only` や
   capability 文法検証などのテストはインターフェース漏れを防ぐ要です。
   維持してください。
5. **`doc(hidden)` は意思表示。** `#[doc(hidden)]` の項目
   （`ene-runtime::streaming`・`message_builder`・一部の VRM ローダーなど）は
   意図的にサポート対象外です。自由にリファクタリングできますが、利用範囲を
   広げないでください。

## ページ一覧

| クレート | ページ |
|---|---|
| `ene-core` | [ene-core](ene-core.md) |
| `ene-config` | [ene-config](ene-config.md) |
| `ene-mind` | [ene-mind](ene-mind.md) |
| `ene-store` | [ene-store](ene-store.md) |
| `ene-ai` | [ene-ai](ene-ai.md) |
| `ene-infer` | [ene-infer](ene-infer.md) |
| `ene-rag` | [ene-rag](ene-rag.md) |
| `ene-connector` | [ene-connector](ene-connector.md) |
| `ene-plugin-proto` | [ene-plugin-proto](ene-plugin-proto.md) |
| `ene-plugin` | [ene-plugin](ene-plugin.md) |
| `ene-plugin-macros` | [ene-plugin-macros](ene-plugin-macros.md) |
| `ene-plugin-host` | [ene-plugin-host](ene-plugin-host.md) |
| `ene-plugin-db` | [ene-plugin-db](ene-plugin-db.md) |
| `ene-runtime` | [ene-runtime](ene-runtime.md) |
| `ene-voice` | [ene-voice](ene-voice.md) |
| `ene-vrm` | [ene-vrm](ene-vrm.md) |
| `ene-util` | [ene-util](ene-util.md) |
