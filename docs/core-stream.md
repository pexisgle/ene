# ストリーミングエンジン

`run_ai_with_tools()` が LLM とのストリーミング対話とツール呼び出しループを実行する。

## AiStreamEvent

```rust
pub enum AiStreamEvent {
    TextDelta(String),                                    // テキスト断片
    SpecialToken(String),                                 // <|emo:name|> 等
    ToolCallStart { name: String, arguments: String },    // ツール呼び出し開始
    ToolCallResult { name: String, result: String },      // ツール実行結果
    PermissionRequired { request_id, action, target, description }, // Phase2
    TaskProgress { task_id, step, total_steps, description },       // Phase2
    SessionSplit { summary: String, reason: String },               // Phase2
    Finished,                                             // 応答完了
    Error(String),                                        // エラー
}
```

## run_ai_with_tools 処理フロー

```rust
pub async fn run_ai_with_tools(
    settings: &EneSettings,
    session: &ConversationSession,
    user_input: &str,
    registry: Arc<dyn ToolRegistry>,
) -> Result<impl Stream<Item = AiStreamEvent>, AiCoreError>
```

### フロー詳細

1. **事前準備**
   - base_url / api_key を解決
   - キャラクターカードのロード確認
   - メモリ有効時: ユーザー入力を `conversation_logs` に非同期保存

2. **記憶検索**
   - `get_all_keyfacts()` で既存の重要事実を取得
   - Tool RAG 有効時: ユーザー入力を埋め込み → `store.search_tools()` で関連ツール抽出
   - `search_summaries()` で関連要約をベクトル検索

3. **メッセージ構築**
   - `build_messages()` で完全なメッセージ配列を組み立て
   - `build_tools()` で `ToolDefinition` → OpenAI 形式に変換

4. **メインループ**（最大 `max_tool_call_rounds` 回）

   ```
   POST chat/completions (stream)
       ↓
   TextDelta イベント配信
       ↓
   ツール呼び出し蓄積（ToolCallChunk）
       ↓
   ストリーム終了後、tool_calls が存在すれば:
     ├─ ToolCallStart イベント
     ├─ registry.call_tool(name, args)  # 30秒タイムアウト
     ├─ スクリーンショット結果の場合は画像メッセージに変換
     ├─ ToolCallResult イベント
     └─ ループ継続
   存在しなければ:
     └─ アシスタントログ保存、Finished イベント
   ```

5. **事後処理**
   - `AiStreamEvent::Finished` を送出
   - **履歴確定は呼び出し側で実行**（CLI では `session.finalize_response()`）

## ツール呼び出しの蓄積と確定

ストリーミング応答中のツール呼び出しは `ToolCallChunk` として蓄積される。

```rust
fn accumulate_tool_calls(chunks: &mut Vec<ToolCallChunk>, delta: &[ToolCallChunk])
fn finalize_tool_calls(chunks: Vec<ToolCallChunk>) -> Vec<ToolCalls>
```

各チャンクの `index` で同一呼び出しを識別し、`function.arguments` を連結する。

## スクリーンショット結果処理

ツール実行結果が `{"type":"screenshot","data":"data:image/png;base64,..."}` 形式の場合、base64 画像データを抽出し、`ChatCompletionRequestMessage::UserMessage` として画像URLを含むメッセージを生成する。
