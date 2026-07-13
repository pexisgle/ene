# Prompt Construction

`build_messages()` in `prompt_builder.rs` constructs the full message array for each LLM request.

## Message Assembly Order

```
build_messages(
    ctx: &MessageBuildContext<'_>,
) -> Result<Vec<LlmMessage>, EneRuntimeError>

MessageBuildContext {
    card: &CharacterCardV3,
    user_input: &str,
    history: &[HistoryEntry],
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
| 3 | **Recalled summaries** | `ene-runtime` message_builder | Memory enabled |
| 4 | **Key facts** | `[Known facts about {user_name}]` | Memory enabled |
| 5 | **Conversation history** | `session.history.conversation_history` | Always |
| 6 | **Expression protocol** | `build_expression_phi()` | Always |
| 7 | **Current user input** | `user_input` + `runtime_context` | Always |

## `build_system_prompt()`

Uses `PromptLibrary` (via `assets/prompts/en.json`) to construct a structured Markdown-style prompt optimized for desktop mascots:

```
[Mascot Context Frame (You are a desktop AI companion...)]

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

Sections are only appended if they contain content. The entire prompt is processed by `expand_cbs_macros()`.

## `build_expression_phi()`

Generates an Emotion Expression Protocol block using `PromptLibrary` templates. Provides concrete examples of how to format the `<|emo:name|>` tokens at the beginning of sentences to improve lower-capability model compliance.

- Uses `emotion.header` and `emotion.rule` from the prompt library
- Lists available tokens from card extensions or defaults (neutral, happy, sad, angry, relaxed, surprised)
- Includes strict positive examples `Example Messages:`

## Tool Passing

Tool specifications (`Vec<ToolSpec>`) are selected via `select_relevant_tools()` (Tool RAG) and passed directly to the LLM provider's `create_chat_stream()`. Each provider internally converts `ToolSpec` to its API format (e.g., OpenAI `ChatCompletionTools`).

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

## Cognitive Runtime PromptPacket (#87 / Phase 6)

The mind streaming path uses `CognitionEngine::compose_prompt_packet` with the Context Budget Manager (`ene-mind::context::pack_prompt`). System content is assembled in this deterministic order:

| # | Section | Source | Budget / Truncation |
|---|---------|--------|---------------------|
| 1 | **Platform Contract** | `PromptLibrary` mascot context | Required |
| 2 | **Identity Kernel** | `CharacterCompiler` (#82) | Required; never dropped |
| 3 | **Behavior Contract** | Card creator notes / runtime rules | Optional |
| 4 | **Current Mood** | `AffectState` summary | Droppable; `mind.context` budget |
| 5 | **Current Scene** | Active `memory_spans` scene summary (#79) | Droppable; `scene_summary_tokens` |
| 6 | **Semantic Context** | Lorebook / semantic recall (#83) | Droppable; `semantic_budget_tokens` |
| 7 | **User Profile** | Preference / relationship recall | Droppable; memory budget share |
| 8 | **Active Commitments** | Commitment ledger (#90) | Droppable |
| 9 | **Episodic Memories** | Hybrid typed recall | Droppable; low confidence dropped first |
| 10 | **Style Examples** | `StyleExampleSelector` (#84) | Droppable; `style_example_budget_tokens` |
| 11 | History | Recent raw turns (`recent_turns`) | Separate LLM messages |
| 12 | **Output Contract** | `build_expression_phi` | Required; separate system message after history |
| 13 | Current user input | User turn | Required; final LLM message |

Overflow policy: Identity Kernel, Platform Contract, Output Contract, and the current user input are never dropped. Style examples and low-confidence memories are dropped first.
