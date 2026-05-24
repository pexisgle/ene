# プロンプト構築

`build_messages()` が全メッセージ配列を構築する。`prompt_builder.rs` に実装。

## メッセージ組み立て順序

```
build_messages(
    card: &CharacterCardV3,
    user_input: &str,
    history: &[(Role, String)],
    runtime_context: &str,
    runtime_rules: &str,
    user_name: &str,
    recalled_summaries: &[RecalledSummary],
    key_facts: &[KeyFact],
) -> Vec<ChatCompletionRequestMessage>
```

| # | 内容 | ソース | 条件 |
|---|------|--------|------|
| 1 | **システムプロンプト** | `build_system_prompt()` | 常に |
| 2 | **例メッセージ** | カード `mes_example` | 初回ターンのみ |
| 3 | **想起された要約** | `format_summaries_for_prompt()` | メモリ有効時 |
| 4 | **重要事実** | `[Known facts about {user_name}]` | メモリ有効時 |
| 5 | **会話履歴** | `session.history.conversation_history` | 常に |
| 6 | **表現プロトコル** | `build_expression_phi()` | 常に |
| 7 | **現在のユーザー入力** | `user_input` + runtime_context | 常に |

## build_system_prompt

```
{runtime_rules}

{card.system_prompt}

Personality:
{card.personality}

Scenario:
{card.scenario}

Description:
{card.description}
```

各セクションは改行 2 つで区切られ、全体に `expand_cbs_macros()` が適用される。

## build_expression_phi

キャラクターカードの `post_history_instructions` と、解決された表情定義から Emotion Expression Protocol ブロックを生成する。

利用可能な `<|emo:name|>` トークンの一覧を提示し、LLM に感情表現の使用を指示する。`card.data.extensions["expressions"]` から `ExpressionDefinition` をパースし、デフォルト表情（neutral/happy/sad/angry/relaxed/surprised）とマージする。無効（`disabled: true`）の表情は除外される。

## build_tools

`ToolDefinition` のリストを OpenAI の `ChatCompletionTools`（Function Calling 形式）に変換する。各ツールの `name`、`description`、`parameters`（JSON Schema）がそのままマッピングされる。
