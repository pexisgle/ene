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
| `ene-tool` / `ene-tool-host` | wire/host ツール ABI とプロセス管理 |
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

## 参照

- [API v1 ADR](api-v1.md)
- [認知ランタイム ADR](cognitive-runtime.md)
- [アバター Performance ADR](avatar-performance.md)
- [能動発話 ADR](proactive-speech.md)
