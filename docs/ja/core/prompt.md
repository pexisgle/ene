# プロンプト構築

`prompt_builder.rs` の `build_messages()` が各 LLM リクエストの完全なメッセージ配列を構築します。

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
| 1 | **システムプロンプト** | `build_system_prompt()` | 常時 |
| 2 | **例メッセージ** | カード `mes_example` | 初回ターンのみ |
| 3 | **呼び出された要約** | `format_summaries_for_prompt()` | メモリ有効時 |
| 4 | **キーファクト** | `[Known facts about {user_name}]` | メモリ有効時 |
| 5 | **会話履歴** | `session.history.conversation_history` | 常時 |
| 6 | **表情プロトコル** | `build_expression_phi()` | 常時 |
| 7 | **現在のユーザー入力** | `user_input` + `runtime_context` | 常時 |

## `build_system_prompt()`

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

各セクションは 2 つの改行で区切られます。プロンプト全体に `expand_cbs_macros()` が適用されます。

## `build_expression_phi()`

キャラクターカードの `post_history_instructions` と解決された表情定義から感情表現プロトコルブロックを生成します。

- 利用可能な `<|emo:name|>` トークンの一覧を提示
- カード拡張 `expressions` をデフォルト (neutral, happy, sad, angry, relaxed, surprised) とマージ
- 無効な表情 (`disabled: true`) は除外

## `build_tools()`

`ToolDefinition` リストを OpenAI `ChatCompletionTools` (関数呼び出し形式) に変換します:

- `name` → 関数名
- `description` → 関数説明
- `parameters` → JSON Schema

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
