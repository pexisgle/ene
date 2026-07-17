# ストリーミングエンジン

ene は **アクターベースのメッセージパッシングアーキテクチャ** で、ツール呼び出し付きのストリーミング LLM 対話を実行します。プロダクトパスは **API v2** です: 準備済み `EneHandle::open`、必須の `TurnId`、単一飛行の `Busy`、最小チャットイベントバス。

## アーキテクチャ

```
コンシューマー (CLI/デスクトップ)
    ↓ EneHandle::open(config, card)
EneHandle (mpsc チャンネル)
    ↓ run(input) → TurnId  (または Busy)
EneActor (バックグラウンド tokio タスク)
    ├── 所有: セッション, 設定, ツールレジストリ, 権限, mind エンジン
    ├── 生成: ストリームタスク (run_stream → mind 認知パス)
    │     ↓ EneEvent (broadcast、ターン範囲)
    └── Terminal { turn, … } までイベントを受信
```

## EneHandle

コンシューマー向け公開 API。スレッドセーフで clone 可能。

### 主要メソッド

| メソッド | 説明 |
|--------|-------------|
| `open(config, card)` | 非同期 — プロバイダ、ストア、ツール、mind、カードを **返却前に** 初期化 |
| `run(input)` | ターン開始。`TurnId` または `RunError::Busy` |
| `cancel(turn)` | 一致するターンのみキャンセル（不一致は `TurnMismatch`） |
| `decide_permission(request_id, decision)` | `PermissionRequired` を解決 |
| `submit_user_input(request_id, response)` | `UserInputRequired` を解決 |
| `subscribe()` | チャット `EneEvent` の broadcast 受信 |
| `diagnostics()` | スナップショット / ツール / 手動分割 / 診断ストリーム用の具象ファサード |
| `shutdown(timeout)` | アクターのドレインを待機 |

設定・キャラクターのファイル I/O は `ene-config` / ホスト（`ConfigStore`）側。公開の未準備 `new` + 多段 `load_config` / `load_character` はプロダクトパスにありません。

### ライフサイクル

- `EneHandle::open` がアクターを生成し、準備完了後にのみ返す
- clone は安い。早期イベントを逃したくない場合は `run` 前に subscribe
- `Drop`: 最後のハンドルが落ちたときだけ `Shutdown`
- アクターは `cmd_rx` が `None` で終了

## EneEvent（チャットバス）

```rust
pub enum EneEvent {
    TextDelta { turn: TurnId, delta: String },
    Performance { turn: TurnId, cues: Vec<PerformanceCue>, source: CueSource },
    ToolCallStart { turn: TurnId, name: String, arguments: String },
    ToolCallResult { turn: TurnId, name: String, result: String },
    PermissionRequired { turn: TurnId, request_id: RequestId, action: String, target: String, description: String },
    UserInputRequired { turn: TurnId, request_id: RequestId, prompt: UserInputPrompt },
    ContextCompressed { turn: TurnId, level: String },
    Terminal { turn: TurnId, reason: TerminalReason },
    StatusChanged { status: EneStatus },
}
```

**注意:**

- `TextDelta` はプレーンテキストのみ。マーカーは除去済み。
- 提示 cue は `Performance`。`SpecialToken` / 単独 `Expression` ではない。
- `Terminal` は `Run` ごとに正確に1回、チャット経路の `after_turn`（記憶書き込み / 忘却）の後。ポストターン LLM affect **分類**は `Terminal` 後に spawn され、Done を遅らせてはならない。
- `cancel(turn)` はストリームを即 abort し、進行中の session 更新は破棄する。キャンセルされたターンの部分アシスタント履歴のマージを期待してはならない。
- Desktop: broadcast `Lagged` 時はアクティブターンを cancel（ゲート解放）しつつ入力を開ける。`processing` がクリアされても `active_turn` がある間は Cancel を有効にする。
- パイプライン位相 / メトリクスは `diagnostics().subscribe()`。

詳細は [ストリーミングイベント](streaming-events.md) を参照。

## 内部ストリームフロー（`run_stream`）

アクターは mind 前提（ストア + 埋め込み）を検証し、認知パスを実行します:

```
Run { input, turn }
  ↓
1. before_turn（想起計画 + 感情）
2. compose_prompt_packet
3. 関連ツール選択（Tool RAG）
4. メインループ（max_tool_call_rounds まで）:
      ├── LLM ストリーミング → TextDelta / Performance
      ├── tool_calls あり → ToolCallStart / 実行 / ToolCallResult → 継続
      └── なし → after_turn（記憶書き込み、忘却）→ Terminal → affect 分類を spawn
5. Terminal { turn, Done | Failed | Cancelled }
```

ストア/埋め込み欠如 → `MindPrerequisite` + 失敗 `Terminal`。レガシーフォールバックなし。

## 権限処理

破壊的ツール操作はユーザー承認が必要です:

```
ツール実行 → PermissionRequired { turn, request_id, … }
  ↓
コンシューマー decide_permission(request_id, AllowOnce | …)
  ↓
ツール再開または中止
```

## 関連ドキュメント

- [ストリーミングイベント](streaming-events.md)
- [API v2](../architecture/api-v2.md)
- [`ene-runtime` API](../api/ene-runtime.md)
- [認知ランタイムADR](../architecture/cognitive-runtime.md)
