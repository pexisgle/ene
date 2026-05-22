# セッション管理

`ConversationSession` は会話の状態全体を保持する。1 セッション＝1 LLM コンテキストウィンドウに相当し、タイムアウトやトピック変化で自動分割される。

## ConversationSession

```rust
pub struct ConversationSession {
    pub conversation_history: Vec<(Role, String)>,   // 会話履歴
    pub max_history_turns: usize,                    // 最大ターン数（デフォルト20）
    pub character_card: Option<CharacterCardV3>,     // キャラクターカード
    pub current_card_path: String,
    pub display_buffer: String,                      // ストリーミングテキスト蓄積バッファ
    pub token_carry: String,                         // 分割トークンのキャリーオーバー

    // 長期記憶
    pub memory_store: Option<Arc<MemoryStore>>,
    pub embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    pub session_id: String,                          // UUID形式
    pub session_started_at: DateTime<Utc>,
    pub pending_embedding: Option<Vec<f32>>,         // ストリーム開始前に計算済みの埋め込み

    // セッション境界検出
    pub last_input_embedding: Option<Vec<f32>>,
    pub last_message_time: Option<DateTime<Utc>>,
    pub current_turn_count: usize,
}
```

## 主要メソッド

| メソッド | 説明 |
|----------|------|
| `new()` | 空のセッション作成、`session_id` 自動生成 |
| `init_memory(store, embedder)` | メモリと埋め込みプロバイダをアタッチ |
| `load_card(path)` | キャラクターカード読み込み、履歴クリア、表情定義解決 |
| `add_user_message(input)` | ユーザーメッセージ追加、自動トリム |
| `add_assistant_message(text)` | アシスタント応答追加、自動トリム |
| `process_delta(chunk)` | ストリームチャンクをテキスト/スペシャルトークンに分割 |
| `finalize_response()` | display_buffer を確定しアシスタントメッセージとして登録 |
| `reset_session()` | 全状態クリア、新 `session_id` 発行 |
| `set_pending_embedding(v)` | 検索用埋め込みを事前保持 |

## CharacterCardV3

`crates/ene-ai-core/src/character_card.rs` で定義される V3 フォーマットのキャラクターカード。

```rust
pub struct CharacterCardV3 {
    pub spec: String,
    pub spec_version: String,
    pub data: CharacterCardData,
}
```

| CharacterCardData フィールド | 説明 |
|------------------------------|------|
| `name`, `nickname` | キャラクター名 |
| `description` | キャラクター説明文 |
| `personality` | 性格記述 |
| `scenario` | シナリオ設定 |
| `system_prompt` | システムプロンプト |
| `first_mes` | 初回メッセージ |
| `mes_example` | 会話例 |
| `post_history_instructions` | 履歴後指示（PHI） |
| `extensions` | 拡張フィールド（表情定義などを含む） |
| `tags`, `creator`, `character_version`, etc. | メタデータ |
| `assets` | アセット参照（VRM等） |

## CBS 式展開

`expand_cbs_macros()` がキャラクターカード内のテンプレート式を展開する。

| マクロ | 展開例 |
|--------|--------|
| `{{char}}` / `<char>` / `<bot>` | キャラクター名 |
| `{{user}}` | ユーザー名 |
| `{{random:a,b,c}}` / `{{pick:a,b,c}}` | ランダム選択 |
| `{{roll:d20}}` | ダイスロール |
| `{{//...}}` / `{{comment:...}}` | コメント（削除） |
| `{{reverse:...}}` | 文字列反転 |
