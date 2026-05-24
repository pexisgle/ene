# セッション管理

`ConversationSession` は会話履歴・表示バッファ・長期記憶コンテキスト・分割状態をまとめて保持する。

## ConversationSession

```rust
pub struct ConversationSession {
    pub history: ConversationHistory,
    pub display: DisplayState,
    pub memory: MemoryContext,
    pub state: SessionState,
    pub character_card: Option<CharacterCardV3>,
    pub current_card_path: String,
}
```

### サブ構造体
```rust
pub struct ConversationHistory {
    pub conversation_history: Vec<(Role, String)>,
    pub max_history_turns: usize, // デフォルト 20
}

pub struct DisplayState {
    pub display_buffer: String,
    pub token_carry: String,
}

pub struct MemoryContext {
    pub memory_store: Option<Arc<MemoryStore>>,
    pub embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    pub session_id: String,
    pub session_started_at: DateTime<Utc>,
    pub pending_embedding: Option<Vec<f32>>,
}

pub struct SessionState {
    pub last_input_embedding: Option<Vec<f32>>,
    pub last_message_time: Option<DateTime<Utc>>,
    pub current_turn_count: usize,
}
```

## 主要メソッド

| メソッド | 説明 |
|---|---|
| `new()` | 空のセッション作成、`session_id` 自動生成 |
| `init_memory(store, embedder)` | メモリと埋め込みプロバイダをアタッチ |
| `load_card(path)` | キャラクターカード読み込み、履歴クリア、表情定義を解決 |
| `add_user_message(input)` | ユーザーメッセージ追加、自動トリム |
| `add_assistant_message(text)` | アシスタント応答追加、自動トリム |
| `process_delta(chunk)` | ストリームチャンクをテキスト/特殊トークンに分割 |
| `finalize_response()` | display_buffer を確定しアシスタントメッセージに登録 |
| `reset_display_buffer()` | 表示バッファをクリア |
| `reset_session()` | 全状態クリア、新 `session_id` 発行 |
| `set_pending_embedding(v)` | 検索用埋め込みを事前保持 |
| `set_last_input_embedding(v)` | 前回入力の埋め込みを保持 |
| `record_user_input()` / `record_assistant_response()` | ターン数・最終時刻更新 |
| `card_name()` | カード名（未設定時は `"default"`） |
| `session_elapsed_minutes()` | セッション開始からの経過分 |

### load_card の補足
`character_settings.json` に `expressions` がある場合はカード拡張にマージされる。  
`default_motion` があれば `extensions["ene"]` に反映される。

## CharacterCardV3

`ene_config::CharacterCardV3` で定義される V3 フォーマットのキャラクターカード。

```rust
pub struct CharacterCardV3 {
    pub spec: String,
    pub spec_version: String,
    pub data: CharacterCardData,
}
```

| CharacterCardData フィールド | 説明 |
|---|---|
| `name`, `nickname` | キャラクター名 |
| `description` | キャラクター説明文 |
| `personality` | 性格記述 |
| `scenario` | シナリオ設定 |
| `system_prompt` | システムプロンプト |
| `first_mes` | 初回メッセージ |
| `mes_example` | 会話例 |
| `post_history_instructions` | 履歴後指示（PHI） |
| `extensions` | 拡張フィールド（表情定義など） |
| `tags`, `creator`, `character_version`, etc. | メタデータ |
| `assets` | アセット参照（VRM等） |

## CBS 式展開
`expand_cbs_macros()` がカード内のテンプレート式を展開する。

| マクロ | 展開例 |
|---|---|
| `{{char}}` / `<char>` / `<bot>` | キャラクター名 |
| `{{user}}` | ユーザー名 |
| `{{random:a,b,c}}` / `{{pick:a,b,c}}` | ランダム選択 |
| `{{roll:d20}}` | ダイスロール |
| `{{//...}}` / `{{comment:...}}` | コメント（削除） |
| `{{reverse:...}}` | 文字列反転 |
