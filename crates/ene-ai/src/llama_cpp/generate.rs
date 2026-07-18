//! Chat completion via llama-cpp-2 (decision path).

use super::backend::with_backend;
use super::load::LoadedModel;
use super::map_llama_err;
use crate::error::LlmProviderError;
use crate::message::{LlmMessage, UserMessagePart};
use llama_cpp_2::json_schema_to_grammar;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use std::time::{Duration, Instant};

const MAX_DECISION_TOKENS: i32 = 256;

/// Generate a completion for `messages`, optionally constrained by JSON schema grammar.
pub(crate) fn generate_chat(
    loaded: &LoadedModel,
    messages: &[LlmMessage],
    json_schema: Option<&serde_json::Value>,
    timeout: Duration,
) -> Result<String, LlmProviderError> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| LlmProviderError::LocalLlm("request timeout overflow".to_string()))?;

    with_backend(|backend| {
        let ctx_params = llama_cpp_2::context::params::LlamaContextParams::default()
            .with_n_ctx(Some(loaded.context_size));
        let mut ctx = loaded
            .model
            .new_context(backend, ctx_params)
            .map_err(|e| map_llama_err("failed to create llama context", e))?;

        let prompt = apply_messages_template(&loaded.model, messages)?;
        let tokens = loaded
            .model
            .str_to_token(&prompt, AddBos::Always)
            .map_err(|e| map_llama_err("tokenize prompt", e))?;

        if tokens.is_empty() {
            return Err(LlmProviderError::LocalLlm(
                "prompt tokenized to empty sequence".to_string(),
            ));
        }

        let n_ctx = i32::try_from(ctx.n_ctx())
            .map_err(|_| LlmProviderError::LocalLlm("n_ctx does not fit in i32".to_string()))?;
        let n_kv_req = i32::try_from(tokens.len())
            .map_err(|_| LlmProviderError::LocalLlm("token count overflow".to_string()))?
            .saturating_add(MAX_DECISION_TOKENS);
        if n_kv_req > n_ctx {
            return Err(LlmProviderError::LocalLlm(format!(
                "prompt too long for context ({n_kv_req} > {n_ctx})"
            )));
        }

        let mut batch = LlamaBatch::new(
            tokens
                .len()
                .saturating_add(MAX_DECISION_TOKENS as usize)
                .max(512),
            1,
        );
        let last_index = i32::try_from(tokens.len().saturating_sub(1)).unwrap_or(0);
        for (i, token) in (0_i32..).zip(tokens.iter().copied()) {
            batch
                .add(token, i, &[0], i == last_index)
                .map_err(|e| map_llama_err("batch.add prompt", e))?;
        }
        ctx.decode(&mut batch)
            .map_err(|e| map_llama_err("decode prompt", e))?;

        let mut sampler = build_sampler(&loaded.model, json_schema)?;
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut output = String::new();
        let mut n_cur = batch.n_tokens();
        let mut n_decode = 0_i32;

        while n_decode < MAX_DECISION_TOKENS {
            if Instant::now() >= deadline {
                return Err(LlmProviderError::LocalLlm(format!(
                    "decision generation timed out after {timeout:?}"
                )));
            }

            let idx = batch.n_tokens().saturating_sub(1);
            let token = sampler.sample(&ctx, idx);
            sampler.accept(token);

            if loaded.model.is_eog_token(token) {
                break;
            }

            let piece = loaded
                .model
                .token_to_piece(token, &mut decoder, true, None)
                .map_err(|e| map_llama_err("token_to_piece", e))?;
            output.push_str(&piece);

            batch.clear();
            batch
                .add(token, n_cur, &[0], true)
                .map_err(|e| map_llama_err("batch.add generated", e))?;
            ctx.decode(&mut batch)
                .map_err(|e| map_llama_err("decode generated", e))?;
            n_cur = n_cur.saturating_add(1);
            n_decode = n_decode.saturating_add(1);
        }

        Ok(output)
    })
}

fn build_sampler(
    model: &LlamaModel,
    json_schema: Option<&serde_json::Value>,
) -> Result<LlamaSampler, LlmProviderError> {
    let mut chain: Vec<LlamaSampler> = Vec::new();
    if let Some(schema) = json_schema {
        let grammar_schema = schema.get("schema").unwrap_or(schema);
        let schema_str = serde_json::to_string(grammar_schema)
            .map_err(|e| map_llama_err("serialize json schema", e))?;
        let grammar = json_schema_to_grammar(&schema_str)
            .map_err(|e| map_llama_err("json_schema_to_grammar", e))?;
        let grammar_sampler = LlamaSampler::grammar(model, &grammar, "root")
            .map_err(|e| map_llama_err("grammar sampler", e))?;
        chain.push(grammar_sampler);
    }
    chain.push(LlamaSampler::temp(0.0));
    chain.push(LlamaSampler::greedy());
    Ok(LlamaSampler::chain_simple(chain))
}

fn apply_messages_template(
    model: &LlamaModel,
    messages: &[LlmMessage],
) -> Result<String, LlmProviderError> {
    let chat: Vec<LlamaChatMessage> = messages
        .iter()
        .map(llm_message_to_chat)
        .collect::<Result<Vec<_>, _>>()?;

    match model.chat_template(None) {
        Ok(tmpl) => model
            .apply_chat_template(&tmpl, &chat, true)
            .map_err(|e| map_llama_err("apply_chat_template", e)),
        Err(_) => Ok(format_plain_prompt(messages)),
    }
}

fn llm_message_to_chat(msg: &LlmMessage) -> Result<LlamaChatMessage, LlmProviderError> {
    match msg {
        LlmMessage::System { content } => LlamaChatMessage::new("system".into(), content.clone())
            .map_err(|e| map_llama_err("chat message", e)),
        LlmMessage::User { parts } => {
            let text = parts
                .iter()
                .filter_map(|p| match p {
                    UserMessagePart::Text { text } => Some(text.as_str()),
                    UserMessagePart::Image { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            LlamaChatMessage::new("user".into(), text).map_err(|e| map_llama_err("chat message", e))
        }
        LlmMessage::Assistant { content, .. } => {
            LlamaChatMessage::new("assistant".into(), content.clone().unwrap_or_default())
                .map_err(|e| map_llama_err("chat message", e))
        }
        LlmMessage::Tool { content, .. } => {
            LlamaChatMessage::new("user".into(), format!("Tool result: {content}"))
                .map_err(|e| map_llama_err("chat message", e))
        }
    }
}

fn format_plain_prompt(messages: &[LlmMessage]) -> String {
    let mut out = String::new();
    for msg in messages {
        match msg {
            LlmMessage::System { content } => {
                out.push_str("System: ");
                out.push_str(content);
                out.push('\n');
            }
            LlmMessage::User { parts } => {
                out.push_str("User: ");
                for part in parts {
                    if let UserMessagePart::Text { text } = part {
                        out.push_str(text);
                    }
                }
                out.push('\n');
            }
            LlmMessage::Assistant { content, .. } => {
                out.push_str("Assistant: ");
                if let Some(c) = content {
                    out.push_str(c);
                }
                out.push('\n');
            }
            LlmMessage::Tool { content, .. } => {
                out.push_str("Tool: ");
                out.push_str(content);
                out.push('\n');
            }
        }
    }
    out.push_str("Assistant:");
    out
}
