# セッション管理

`ConversationSession` は会話状態の中央コンテナです。

## 構造

```rust
pub struct ConversationSession {
    pub(crate) history: ConversationHistory,   // クレートプライベート
    pub display: DisplayState,
    pub memory: MemoryContext,
    pub(crate) state: SessionState,            // クレートプライベート
    pub character_card: Option<CharacterCardV3>,
    current_card_path: String,                 // プライベート
}
```

### サブ構造

```rust
pub struct ConversationHistory {
    pub conversation_history: Vec<(Role, String)>,
    pub max_history_turns: usize,  // デフォルト 20
}

pub struct DisplayState {
    pub display_buffer: String,   // 蓄積されたストリーミングテキスト
    pub token_carry: String,      // チャンクを跨ぐ部分トークン
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
|---------|------|
| `new()` | 空のセッションを作成、`session_id` を自動生成 |
| `init_memory(store, embedder)` | 記憶ストアと埋め込みプロバイダをアタッチ |
| `load_card(path)` | キャラクターカードを読み込み、`character_settings.json` をマージ、履歴をクリア |
| `add_user_message(input)` | ユーザーメッセージを追加、自動トリム |
| `add_assistant_message(text)` | アシスタント応答を追加、自動トリム |
| `process_delta(chunk)` | ストリームチャンクをテキスト/特殊トークンに分割 |
| `finalize_response()` | 表示バッファをアシスタントメッセージとして確定 |
| `reset_session()` | 全状態をクリア、新しい `session_id` を生成 |
| `card_name()` | キャラクター名または `"default"` を返す |
| `session_elapsed_minutes()` | セッション開始からの経過分数 |

## CharacterCardV3

`ene_config` で定義される V3 フォーマットのキャラクターカード:

| フィールド | 説明 |
|-----------|------|
| `spec` / `spec_version` | フォーマットバージョン識別子 |
| `data.name` | キャラクター名 |
| `data.nickname` | 代替表示名（空でない場合 `name` より優先） |
| `data.description` | キャラクター説明文 |
| `data.personality` | 性格記述 |
| `data.scenario` | シナリオ設定 |
| `data.system_prompt` | システムプロンプト上書き |
| `data.first_mes` | カード読み込み時の最初のメッセージ |
| `data.alternate_greetings` | 代替グリーティングメッセージ |
| `data.mes_example` | 会話例 |
| `data.post_history_instructions` | 履歴後指示 (PHI) |
| `data.creator_notes` | カード作成者のメモ |
| `data.tags` | タグ / カテゴリ |
| `data.creator` | カード作成者名 |
| `data.character_version` | このキャラクターのバージョン文字列 |
| `data.extensions` | 拡張フィールド (表情定義、設定) |
| `data.assets` | アセット参照 (VRM モデル、画像) |
| `data.character_book` | 世界観構築用のオプショナルなロアブック |
| `data.source` | 帰属情報 |
| `data.group_only_greetings` | グループチャット用の代替グリーティング |
| `data.creation_date` | 作成日の Unix タイムスタンプ |
| `data.modification_date` | 最終変更日の Unix タイムスタンプ |
