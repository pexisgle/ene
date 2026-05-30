# Prompt Construction

`build_messages()` in `prompt_builder.rs` constructs the full message array for each LLM request.

## Message Assembly Order

```
build_messages(
    ctx: &MessageBuildContext<'_>,
) -> Result<Vec<ChatCompletionRequestMessage>>

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

| # | Content | Source | Condition |
|---|---------|--------|-----------|
| 1 | **System prompt** | `build_system_prompt()` | Always |
| 2 | **Example messages** | Card `mes_example` | First turn only |
| 3 | **Recalled summaries** | `format_summaries_for_prompt()` | Memory enabled |
| 4 | **Key facts** | `[Known facts about {user_name}]` | Memory enabled |
| 5 | **Conversation history** | `session.history.conversation_history` | Always |
| 6 | **Expression protocol** | `build_expression_phi()` | Always |
| 7 | **Current user input** | `user_input` + `runtime_context` | Always |

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

Sections are separated by two newlines. The entire prompt is processed by `expand_cbs_macros()`.

## `build_expression_phi()`

Generates an Emotion Expression Protocol block from the character card's `post_history_instructions` and resolved expression definitions.

- Lists available `<|emo:name|>` tokens
- Merges card extensions `expressions` with defaults (neutral, happy, sad, angry, relaxed, surprised)
- Disabled expressions (`disabled: true`) are excluded

## `build_tools()`

Converts `ToolDefinition` lists into OpenAI `ChatCompletionTools` (function calling format):

- `name` → function name
- `description` → function description
- `parameters` → JSON Schema

## CBS Macro Expansion

`expand_cbs_macros()` processes template expressions in character card text:

| Macro | Expansion |
|-------|-----------|
| `{{char}}` / `<char>` / `<bot>` | Character name |
| `{{user}}` | User name |
| `{{random:a,b,c}}` / `{{pick:a,b,c}}` | Random selection |
| `{{roll:d20}}` | Dice roll |
| `{{//...}}` / `{{comment:...}}` | Comment (removed) |
| `{{reverse:...}}` | String reversal |
