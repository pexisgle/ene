# 自動セッション分割

LLM コンテキストウィンドウの制限に対応し、話題変化に適応するためにセッションは自動分割されます。

## 分割理由

```rust
pub enum SplitReason {
    Timeout { elapsed_minutes: u64 },
    TopicChange { similarity: f32 },
    Manual,
}
```

## トリガー条件

`check_boundary()` が 2 つの条件を評価します:

1. **タイムアウト**
   - `session_elapsed_minutes() >= session_timeout_minutes`
   - かつ `current_turn_count >= min_turns_before_split`

2. **話題変化**
   - 前回のユーザー入力埋め込みと今回の埋め込みのコサイン類似度 < `topic_change_threshold`
   - 有効な埋め込みを持つユーザー入力が最低 2 回必要
   - かつ `current_turn_count >= min_turns_before_split`

## 非同期ライフサイクル

```
ユーザー入力受信
    ↓
spawn_split_task() → バックグラウンド Tokio タスク
    ↓
                    check_boundary()
                        ↓
                    Continue | Split
                        ↓ (Split)
                    execute_split()
                        ↓
                    oneshot チャンネルで SplitResult を送信
    ↓
poll_split_result() → ノンブロッキング確認
    ↓ (完了時)
session.reset_session()
```

同時に実行される分割タスクは 1 つのみです。既に pending のタスクがある場合、`spawn_split_task()` の追加呼び出しは無視されます。

## `execute_split()` の手順

1. 会話履歴の全件を `conversation_logs` に保存
2. 既存のキーファクトを取得
3. `summarize_conversation()` を呼び出し → LLM が構造化要約 + トピック + キーファクトを生成
4. `embed_session_messages()` → メッセージごとの埋め込み → max-pooling → 単一ベクトル
5. `insert_summary()` → 要約 + キーファクトを `MemoryStore` に保存
6. 新しいセッション ID を返却

## Max-pooling 埋め込み

`embed_session_messages()` は各ユーザー/アシスタントメッセージを個別に埋め込み、次元ごとに最大値 (max-pooling) を適用します。これにより、挨拶や相槌などの情報量の少ないメッセージによるセマンティックシグナルの希釈を防ぎます。結果のベクトルは正規化されます。

## 結果のポーリング

`poll_split_result()` は oneshot 受信機をノンブロッキングで確認します。結果が到着すると、呼び出し側は新しいセッション ID でセッションリセットを実行します。次のメッセージサイクルで新しい `ConversationSession` が開始されます。
