# プロンプト構築

`prompt_builder.rs` の `build_messages()` が各 LLM リクエストの完全なメッセージ配列を構築します。

## メッセージ組み立て順序

```
build_messages(
    ctx: &MessageBuildContext<'_>,
) -> Result<Vec<LlmMessage>, EneCoreError>

MessageBuildContext {
    card: &CharacterCardV3,
    user_input: &str,
    history: &[ConversationEntry],
    runtime_context: Option<&str>,
    runtime_rules: &str,
    user_name: &str,
    recalled_summaries: &[RecalledSummary],
    key_facts: &[KeyFact],
}
```

| # | 内容 | ソース | 条件 |
|---|------|--------|------|
| 1 | **システムプロンプト** | `build_system_prompt()` | 常時 |
| 2 | **例メッセージ** | カード `mes_example` | 初回ターンのみ |
| 3 | **呼び出された要約** | `format_summaries_for_prompt()` | メモリ有効時 |
| 4 | **キーファクト** | `[Known facts about {user_name}]` | メモリ有効時 |
| 5 | **会話履歴** | `session.history.conversation_history` | 常時 |
| 6 | **表情プロトコル** | `build_expression_phi()` | 常時 |
| 7 | **現在のユーザー入力** | `user_input` + `runtime_context` | 常時 |

## `build_system_prompt()`

`PromptLibrary`（`assets/prompts/en.json` 経由）を使用して、デスクトップマスコット向けに最適化されたMarkdownスタイルのプロンプトを構築します：

```
[マスコットコンテキストフレーム (You are a desktop AI companion...)]

## Behavior Rules
{runtime_rules}

## Character
{card.system_prompt}

### Personality
{card.personality}

### Background
{card.description}

## Current Scene
{card.scenario}
```

各セクションはコンテンツが存在する場合にのみ追加されます。プロンプト全体に `expand_cbs_macros()` が適用されます。

## `build_expression_phi()`

`PromptLibrary` のテンプレートを使用して、感情表現プロトコルブロックを生成します。性能の低いモデルでも `<|emo:name|>` トークンを正しく出力できるように、文頭に配置する具体的な例を提供します。

- プロンプトライブラリから `emotion.header` と `emotion.rule` を使用
- カード拡張 または デフォルト (neutral, happy, sad, angry, relaxed, surprised) から利用可能なトークン一覧を提示
- 具体的な成功例 (`Example Messages:`) を含める

## ツール渡し

ツール仕様 (`Vec<ToolSpec>`) は `select_relevant_tools()` (Tool RAG) で選択され、LLM プロバイダの `create_chat_stream()` に直接渡されます。各プロバイダは内部で `ToolSpec` を API 形式（例: OpenAI `ChatCompletionTools`）に変換します。

## CBS マクロ展開

`expand_cbs_macros()` がキャラクターカード内のテンプレート式を処理します:

| マクロ | 展開例 |
|-------|--------|
| `{{char}}` / `<char>` / `<bot>` | キャラクター名 |
| `{{user}}` | ユーザー名 |
| `{{random:a,b,c}}` / `{{pick:a,b,c}}` | ランダム選択 |
| `{{roll:d20}}` | ダイスロール |
| `{{//...}}` / `{{comment:...}}` | コメント (削除) |
| `{{reverse:...}}` | 文字列反転 |

## Cognitive Runtime PromptPacket（#87 / Phase 5–6）

`cognition.enabled` が true のとき、`ene-core::streaming_cognitive` は `build_messages` の代わりに `CognitionEngine::compose_prompt_packet` を使う。system 内容の組み立て順:

| # | セクション | ソース | truncate |
|---|-----------|--------|----------|
| 1 | **Identity Kernel** | `CharacterCompiler`（#82） | 不可（コアヘッダー保持） |
| 2 | **Style Examples** | `StyleExampleSelector`（#84） | drop 可。予算 `style_example_budget_tokens` |
| 3 | **Recalled Memories** | hybrid recall + lorebook boost（#83） | `max_prompt_tokens` 尾部予算 |
| 4 | **Active Commitments** | commitment ledger | 同上 |
| 5 | **Current Mood** | affect 要約 | 同上 |
| 6 | 履歴 | セッション履歴 | 別 LLM メッセージ |
| 7 | **Expression PHI** | `build_expression_phi`（レガシー互換） | 履歴の後の別 system メッセージ |
| 8 | 現在のユーザー入力 | ユーザーターン | 最後の LLM メッセージ |
