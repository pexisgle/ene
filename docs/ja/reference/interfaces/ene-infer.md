# `ene-infer` インターフェース

## 役割

**同期・シングルスレッドのローカルモデル**を統一された非同期 API の背後で
実行する基盤: 専用ワーカースレッド・有界キュー・協調キャンセル・単一
タイムアウト・パニック回復。

## 主要な公開項目

| 項目 | 契約 |
|---|---|
| `LocalModel` | 実装者が書く同期トレイト: `run(&mut self, req, ctx) -> Result<Response, Error>`（+ 任意の `reset`） |
| `StreamingLocalModel` | トークン/音声出力向けのストリーミング版 |
| `EngineHandle<M>` | `spawn(factory, config)` がワーカースレッドでモデルを所有。`submit(req, token)` が有界・非ブロッキングの非同期入口 |
| `EngineConfig` | `job_timeout`（ワーカー内部で協調的に強制） |
| `JobContext`・`StopReason` | ジョブ単位のコンテキストと停止シグナル |
| `EngineError` | `Busy`（キュー満杯）・`EngineDown`（パニック回復）・タイムアウト・キャンセルなど |
| `ChunkReceiver`・`ChunkSink` | ストリーミングチャンクの配管 |
| `conformance::run_all` | `LocalModel` 実装向け汎用回帰テスト一式（フィーチャー `test-util`） |

## 依存関係

- 依存: 内部なし（tokio・tokio-util・thiserror・tracing）。
- 利用: `ene-ai`（`engine_adapter`）・`ene-voice`（STT/TTS/VAD エンジン）・
  `ene_plugin::prelude` 経由のプラグイン作者。

## リファクタリングの注目点

- ワーカーの不変条件がこのクレートの存在理由です: モデルは 1 スレッドが所有・
  有界キューは即失敗・協調タイムアウトは 1 つ・`catch_unwind` とモデル再構築。
  `submit` の周囲に外側タイムアウトや `spawn_blocking` ラッパーを追加しないで
  ください — このクレートが排除するためにあるバグです。
- `catch_unwind` はネイティブの `abort()`（例: `GGML_ASSERT`）を捕捉できません。
  プロバイダーはネイティブコードに渡す前に入力を検証してください。
- `conformance` テスト一式がプロバイダー横断でエンジン挙動を固定します。
  ワーカー意味論を変更するときは必ず実行してください。
