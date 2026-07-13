# プロンプト構築

`prompt_builder.rs` の `build_messages()` が各 LLM リクエストの完全なメッセージ配列を構築します。

## メッセージ組み立て順序

```
build_messages(
    ctx: &MessageBuildContext<'_>,
) -> Result<Vec<LlmMessage>, EneRuntimeError>

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

## Cognitive Runtime PromptPacket（#87 / Phase 6）

mind ストリーミングパスでは、`ene-runtime::streaming_cognitive` が Context Budget Manager（`ene-mind::context::pack_prompt`）付きの `CognitionEngine::compose_prompt_packet` を使う。system 内容の組み立て順:

| # | セクション | ソース | 予算 / truncate |
|---|-----------|--------|-----------------|
| 1 | **Platform Contract** | `PromptLibrary` マスコット文脈 | 必須 |
| 2 | **Identity Kernel** | `CharacterCompiler`（#82） | 必須。drop 不可 |
| 3 | **Behavior Contract** | カード creator notes / ランタイムルール | 任意 |
| 4 | **Current Mood** | `AffectState` 要約 | drop 可 |
| 5 | **Current Scene** | アクティブな `memory_spans` シーン要約（#79） | drop 可。`scene_summary_tokens` |
| 6 | **Semantic Context** | lorebook / 意味記憶 recall（#83） | drop 可。`semantic_budget_tokens` |
| 7 | **User Profile** | 嗜好 / 関係性 recall | drop 可 |
| 8 | **Active Commitments** | commitment ledger（#90） | drop 可 |
| 9 | **Episodic Memories** | hybrid typed recall | drop 可。低信頼度を優先 drop |
| 10 | **Style Examples** | `StyleExampleSelector`（#84） | drop 可。`style_example_budget_tokens` |
| 11 | 履歴 | 直近 raw ターン（`recent_turns`） | 別 LLM メッセージ |
| 12 | **Output Contract** | `build_expression_phi` | 必須。履歴後の別 system メッセージ |
| 13 | 現在のユーザー入力 | ユーザーターン | 必須。最後の LLM メッセージ |

オーバーフロー時: Identity Kernel・Platform Contract・Output Contract・現在のユーザー入力は drop されない。Style Examples と低信頼度記憶が最初に drop される。
