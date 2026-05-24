# セッション自動分割

LLM コンテキストウィンドウ制約と話題変化に対応するため、会話を自動分割する。

## トリガー条件

```rust
pub enum SplitReason {
    Timeout { elapsed_minutes: u64 },
    TopicChange { similarity: f32 },
    Manual,
}
```

| トリガー | 条件 |
|----------|------|
| Timeout | `elapsed >= session_timeout_minutes` AND `current_turn_count >= min_turns_before_split` |
| TopicChange | 前回のユーザー入力埋め込みと今回の埋め込みのコサイン類似度 < `topic_change_threshold` |

`check_boundary()` が両条件を評価する:

1. 自動分割が無効なら即座に `Continue`
2. タイムアウトチェック
3. トピック変更チェック（前回の embedding が存在し、最低ターン数以上の場合）
4. いずれも該当しなければ `Continue`

## 非同期タスク起動

`spawn_split_task()` はバックグラウンド Tokio タスクで境界チェック＋分割を実行し、結果を `oneshot` チャンネルで通知する。同時に 1 タスクのみ実行可能（既に pending の場合は無視）。

## 分割実行

`execute_split()` の処理順:

1. 会話履歴全件を `conversation_logs` に保存
2. 既存のキーファクトを取得
3. LLM で `summarize_conversation()` を呼び出し要約＋キーファクト生成
4. `embed_session_messages()` で全メッセージを個別に埋め込み、Max-pooling で単一ベクトルに集約
5. `insert_summary()` で要約＋キーファクトを保存
6. 新しい `session_id` を生成して返却

## 結果ポーリング

`poll_split_result()` がノンブロッキングで oneshot 受信機を確認し、分割完了後にセッションリセットを実行する。

## Max-pooling 埋め込み

`embed_session_messages()` は会話履歴の全ユーザー/アシスタントメッセージを個別に埋め込み、各次元の最大値を採用する。これにより挨拶などの情報量ゼロのメッセージによる意味の希釈を防止する。最終ベクトルは正規化される。
