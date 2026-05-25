# ストリーミングエンジン

`ene-core` の `run_ai_with_tools()` は、ツール呼び出しループを備えたストリーミング LLM 対話を実行します。

## `AiStreamEvent`

ストリーミングパイプラインが生成する主要なイベント型:

```rust
pub enum AiStreamEvent {
    TextDelta(String),                                    // テキスト断片
    SpecialToken(String),                                 // 例: <|emo:happy|>
    ToolCallStart { name: String, arguments: String },    // ツール呼び出し開始
    ToolCallResult { name: String, result: String },      // ツール実行結果
    PermissionRequired { request_id, action, target, description }, // Phase 2
    TaskProgress { task_id, step, total_steps, description },       // Phase 2
    SessionSplit { summary: String, reason: String },               // Phase 2
    Finished,                                             // 応答完了
    Error(String),                                        // エラー詳細
}
```

## `run_ai_with_tools()` フロー

```rust
pub async fn run_ai_with_tools(
    settings: &EneSettings,
    session: &ConversationSession,
    user_input: &str,
    registry: Arc<dyn ToolRegistry>,
) -> Result<impl Stream<Item = AiStreamEvent>, AiCoreError>
```

### ステップ詳細

1. **準備** — `base_url`/`api_key` 解決、カード読み込み確認、ユーザー入力を `conversation_logs` に保存 (非同期、メモリ有効時)

2. **記憶検索**
   - `get_all_keyfacts()` → 既存ユーザーファクト
   - Tool RAG 有効時: ユーザー入力を埋め込み → `store.search_tools()` → 関連ツール
   - `search_summaries()` → 呼び出された過去の会話要約

3. **メッセージ構築**
   - `build_messages()` → 完全なメッセージ配列 (システムプロンプト、例、要約、履歴、プロトコル、ユーザー入力)
   - `build_tools()` → `ToolDefinition` リスト → OpenAI 関数呼び出し形式

4. **メインループ** (最大 `max_tool_call_rounds` 回)

   ```
   POST chat/completions (ストリーム)
       ↓
   TextDelta イベント送出
       ↓
   ToolCallChunk 蓄積
       ↓
   ストリーム終了後、tool_calls が存在すれば:
     ├── ToolCallStart イベント
     ├── registry.call_tool(name, args) (30秒タイムアウト)
     ├── スクリーンショット結果 → 画像メッセージ変換
     ├── ToolCallResult イベント
     └── ループ継続
   存在しなければ:
     └── アシスタントログ保存、Finished イベント
   ```

5. **事後処理**
   - `AiStreamEvent::Finished` 送出
   - 履歴確定は呼び出し側の責任 (`session.finalize_response()`)

## ツール呼び出しの蓄積

ストリーミング中のツール呼び出しはチャンク単位で到着し、蓄積する必要があります:

```rust
fn accumulate_tool_calls(chunks: &mut Vec<ToolCallChunk>, delta: &[ToolCallChunk])
fn finalize_tool_calls(chunks: Vec<ToolCallChunk>) -> Vec<ToolCalls>
```

各チャンクは `index` フィールドで識別されます。`function.arguments` 文字列はチャンク間で連結されます。

## スクリーンショット処理

ツール結果が `{"type":"screenshot","data":"data:image/png;base64,..."}` 形式の場合、base64 データが抽出され、画像 URL を含む `ChatCompletionRequestMessage::UserMessage` に変換されて次の LLM API 呼び出しに送られます。
