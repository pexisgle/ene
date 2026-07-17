# ADR: API v2 — 機能再設計

- **Status:** Accepted
- **Date:** 2026-07-13
- **Epic:** [#111](https://github.com/pexisgle/ene/issues/111)

## 背景

ライブラリ表面に二重ストリーミング（legacy + cognitive）、未準備の `EneHandle::new` ライフサイクル、チャット必須イベントと診断の混在、並行する設定ノブ（`memory.*` と `mind.memory.*`）、HyDE/rerank まで抱えた `EmbeddingProvider` が積み上がっていた。API v2 は最小ホスト契約と明確なクレート所有へ畳み込む。

## ロック済み決定

### ホスト / ターン識別

1. **`TurnId` は必須。** `run(input) -> Result<TurnId, Busy | ActorDead>`。ターンスコープのイベントと `cancel(turn)` はその id を運ぶ。
2. **並行性:** single-flight。ターン実行中の二度目の `run` は `Busy` — 暗黙 abort や broadcast だけの相関はしない。
3. **ライフサイクル:** `EneHandle::open(config, card) -> Result<ReadyHandle, _>`。公開の未準備 `new` + 多段 `load_config` / `load_character` は置かない。設定ファイル I/O は `ConfigStore` / `ene-config` 側。
4. **`Terminal` はチャット経路のターン完了**を意味する（返信をブロックする memory write / forgetting / affect persist の後）。ポストターンの LLM affect **分類**は `Terminal` 後の fire-and-forget であり、Done を遅らせたりターンゲートを占有してはならない。

### イベント

5. **チャット `EneEvent` は最小:** `TextDelta`、`Performance`、対話ゲート（`PermissionRequired` / `UserInputRequired`）、必要なら tool start/result、`Terminal`、`StatusChanged`、任意の薄い `ContextCompressed`。
6. **診断はオプトイン:** `PipelinePhase`、`PipelineMetrics`、arbiter/compression 詳細は `handle.diagnostics()`。
7. **`EneDiagnostics`** はハンドル上の具象 facade — UI がトレイトを実装しない。

### プロバイダ（`ene-ai`）

8. **`EmbeddingProvider`:** トレイト上は batch メソッド一つ。単一テキスト / query embed は convenience。HyDE / rerank は mind または tool-host の pipeline ステップ。

### クレートマップ

| ターゲット | 吸収元 |
|---|---|
| `ene-runtime` | 旧 `ene-core` ホスト / アクターファサード |
| `ene-mind` | 認知エンジン + セッション（旧 `ene-session` を吸収） |
| `ene-store` | 永続化（旧 memory ストア表面） |
| `ene-ai` | `ene-provider` + `ene-embedding` |
| `ene-tool` | `ene-tool-proto` + `ene-tool-common` + `ene-tool-derive` |
| `ene-tool-db` | ツールバイナリ用 IPC CRUD クライアント → `ene-runtime` の `DbIpcServer`；依存は `ene-tool-proto` のみ |
| `ene-tool-host` / `ene-tool-rag` / `ene-config` / `ene-vrm` | 同様（LayerComposer は vrm/desktop 内） |

### 依存ルール

- `ene-mind` ↛ `ene-runtime` / `ene-tool-host`
- `ene-store` ↛ `ene-ai` / `ene-mind`（LLM・埋め込みプロバイダなし）
- `ene-tool` ↛ runtime / mind / store
- `ene-tool-host` ↛ `ene-ai` / `ene-store` / `ene-mind` — Tool RAG は `ene-tool-rag` に配置
- `ene-tool-rag` は `ene-ai`（埋め込み、HyDE、rerank）+ `ene-store`（永続ツール埋め込み）に依存
- **`PerformanceCue` は `ene-mind` 所有**；runtime が再エクスポート；**`ene-vrm` は mind/runtime に依存しない**
- `ene-common` は `ene-config`（`Truncate` / `TruncateResult` は `ene_config::truncate`）と `ene-tool-common`（再エクスポート）へ吸収；`schema_link` 削除（runtime が mind を通常依存）

### 設定所有

- Store 側: `store.enabled` + `store.db_path` のみ
- recall / write / decay / MMR / emotion / performance はすべて `mind.*`
- JSON 破損: CLI は fail-hard（`ConfigStore::try_load`）；desktop のみ必要なら `ConfigStore::load` の soft-fallback

### 関連 epic

- [#119](https://github.com/pexisgle/ene/issues/119) Memory — ledger が唯一の SoT；store に embedder なし
- [#126](https://github.com/pexisgle/ene/issues/126) Performance — `PerformanceCue` は mind；明示 `perform` なしに `CueSource::Host` は置かない
- [#135](https://github.com/pexisgle/ene/issues/135) Tools — name 衝突は全レジストリ層で hard error；wire / host トレイト分離；`ToolSpec` は LLM 向けのみ（`name`, `description`, `parameters`）、内部 RAG フィールドは `#[doc(hidden)]` + `#[serde(skip)]`
- [#138](https://github.com/pexisgle/ene/issues/138) IPC — 8 request / 6 response バリアント（v3）；`UserInput` は `ToolError` 経由で送出
- [#158](https://github.com/pexisgle/ene/issues/158) ABI 整合 — #135 契約を実装に一致：`CompositeToolRegistry` を hard error 化、dead IPC バリアントを削除、`SingleActionProvider` を追加

## 目標依存グラフ

```mermaid
flowchart TD
  cli["ene-cli"] --> runtime["ene-runtime"]
  desktop["ene-desktop"] --> runtime
  desktop --> vrm["ene-vrm"]
  runtime --> mind["ene-mind"]
  runtime --> store["ene-store"]
  runtime --> ai["ene-ai"]
  runtime --> toolHost["ene-tool-host"]
  runtime --> toolRag["ene-tool-rag"]
  mind --> store
  mind --> ai
  ai --> toolProto["ene-tool-proto"]
  toolHost --> tool["ene-tool"]
  toolRag --> ai
  toolRag --> store
  toolRag --> toolProto
  store --> config["ene-config"]
  ai --> config
  mind --> config
```

チャットは `store.enabled=false` でも動作する（SQLite メモリなし）。メモリ機能（recall / spans / 型付きメモリ）には `store.enabled=true` と設定済み embedder が必要。

## ホスト契約（要約）

```rust
EneHandle::open(config, card) -> Result<EneHandle, EneRuntimeError>;
handle.run(input) -> Result<TurnId, RunError>; // Busy | ActorDead
handle.cancel(turn: &TurnId) -> Result<(), CancelError>;
handle.subscribe() -> EneEventReceiver;
handle.decide_permission(...);
handle.submit_user_input(...);
handle.shutdown(timeout).await;

handle.diagnostics() -> &EneDiagnostics;
```

### チャット `EneEvent`

| バリアント | 備考 |
|---|---|
| `TextDelta { turn, delta }` | marker 除去済み |
| `Performance { turn, cues, source }` | SpecialToken + Expression を置換 |
| `ToolCallStart` / `ToolCallResult` | UI 用に任意 |
| `PermissionRequired` / `UserInputRequired` | ゲート |
| `ContextCompressed { turn, … }` | 薄い信号；詳細は diagnostics |
| `Terminal { turn, reason }` | ターン完了 |
| `StatusChanged { status }` | Idle / Running / Error |

チャットから除去: `SpecialToken`、単独の `Expression`、`SessionSplit`、`PipelinePhase`、`PipelineMetrics`。

## エラーと非同期の規約

- I/O および oneshot actor 応答: tokio 上の `async fn`
- fire-and-forget（`run`、`cancel`、ゲート）: sync channel send
- クレートごとに公開 `thiserror` 列挙を一つ；ライブラリ境界に `anyhow` / 生 `String` / `Box<dyn Error>` なし
- テスト以外で `unwrap` / `expect` なし

## 移行メモ

- 二重パイプライン fallback なし: 埋め込み未初期化で memory 機能が必要な場合は fail closed
- **文脈境界は compression-only:** `mind.context.compression_enabled` は `EneHandle::open` 時点で `true` 必須（さもなくば validation 失敗）。hard session-ID 発行 / hard-split は製品経路ではなく、`ene-runtime` は hard-split タスクを起動しない
- **Cancel:** ストリームタスクを即 abort し、進行中の session 更新は破棄する。`Terminal::Cancelled` は最大一度
- 単一の `HistoryEntry { role: Role, content: String }` を mind + runtime snapshot で共有
- 公開互換 alias や移行用 feature/config は作らない — 呼び出し側を同一変更で更新
- ストア無しチャットはサポート；メモリ有効化時に store + embedder が無ければ fail closed
## 受入

- [ ] サブ issue #112–#118 がクローズまたは明示延期
- [ ] ロック事項が EN+JA docs に反映
- [ ] docs EN+JA が目標契約と一致
- [ ] `direnv exec .` 下で workspace clippy/tests が緑

## 参照

- Epic #111 とサブ issue #112–#118
- [認知ランタイム ADR](cognitive-runtime.md)
- [API Index](../api/index.md)
- [ストリーミングイベント](../runtime/streaming-events.md)
