//! Chat completion via llama-cpp-2 (text + optional mtmd vision).

use super::backend::with_backend;
use super::load::LoadedModel;
use super::map_llama_err;
use crate::error::LlmProviderError;
use crate::message::{LlmMessage, UserMessagePart};
use llama_cpp_2::json_schema_to_grammar;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaModel};
use llama_cpp_2::mtmd::{MtmdBitmap, MtmdInputText, mtmd_default_marker};
use llama_cpp_2::sampling::LlamaSampler;
use std::time::{Duration, Instant};

const MAX_DECISION_TOKENS: i32 = 256;
const MAX_VISION_TOKENS: i32 = 128;

/// Generate a completion for `messages`, optionally constrained by JSON schema grammar.
pub(crate) fn generate_chat(
    loaded: &LoadedModel,
    messages: &[LlmMessage],
    json_schema: Option<&serde_json::Value>,
    timeout: Duration,
) -> Result<String, LlmProviderError> {
    if messages_have_images(messages) {
        return generate_chat_vision(loaded, messages, json_schema, timeout);
    }
    generate_chat_text(loaded, messages, json_schema, timeout)
}

/// Vision completion from raw RGB8 pixels (desktop screen summary).
pub(crate) fn generate_with_rgb_image(
    loaded: &LoadedModel,
    system: &str,
    user: &str,
    width: u32,
    height: u32,
    rgb: &[u8],
    timeout: Duration,
) -> Result<String, LlmProviderError> {
    // Fold system into a single user turn; Image part alone inserts the mtmd marker.
    let mut text = String::new();
    if !system.trim().is_empty() {
        text.push_str(system.trim());
        text.push_str("\n\n");
    }
    text.push_str(user);
    let messages = [LlmMessage::User {
        parts: vec![
            UserMessagePart::Text { text },
            // Placeholder so `messages_have_images` / template path stay consistent;
            // actual bitmap is passed separately.
            UserMessagePart::Image {
                base64_image_data: String::new(),
            },
        ],
    }];
    generate_chat_vision_with_bitmaps(loaded, &messages, &[(width, height, rgb)], None, timeout)
}

fn generate_chat_text(
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

        let prompt = apply_messages_template(&loaded.model, messages, false)?;
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

        let n_cur = batch.n_tokens();
        sample_tokens(
            loaded,
            &mut ctx,
            &mut batch,
            json_schema,
            deadline,
            MAX_DECISION_TOKENS,
            n_cur,
        )
    })
}

fn generate_chat_vision(
    loaded: &LoadedModel,
    messages: &[LlmMessage],
    json_schema: Option<&serde_json::Value>,
    timeout: Duration,
) -> Result<String, LlmProviderError> {
    let bitmaps = extract_rgb_images(messages)?;
    if bitmaps.is_empty() {
        return generate_chat_text(loaded, messages, json_schema, timeout);
    }
    let refs: Vec<(u32, u32, &[u8])> = bitmaps
        .iter()
        .map(|(w, h, data)| (*w, *h, data.as_slice()))
        .collect();
    generate_chat_vision_with_bitmaps(loaded, messages, &refs, json_schema, timeout)
}

fn generate_chat_vision_with_bitmaps(
    loaded: &LoadedModel,
    messages: &[LlmMessage],
    images: &[(u32, u32, &[u8])],
    json_schema: Option<&serde_json::Value>,
    timeout: Duration,
) -> Result<String, LlmProviderError> {
    let mtmd = loaded.mtmd.as_ref().ok_or_else(|| {
        LlmProviderError::LocalLlm(
            "vision requested but mmproj is not loaded for this local model".to_string(),
        )
    })?;
    if !mtmd.support_vision() {
        return Err(LlmProviderError::LocalLlm(
            "loaded mmproj does not support vision".to_string(),
        ));
    }

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

        let prompt = apply_messages_template(&loaded.model, messages, true)?;
        let bitmaps: Vec<MtmdBitmap> = images
            .iter()
            .map(|(w, h, rgb)| {
                MtmdBitmap::from_image_data(*w, *h, rgb)
                    .map_err(|e| LlmProviderError::LocalLlm(format!("mtmd bitmap: {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let bitmap_refs: Vec<&MtmdBitmap> = bitmaps.iter().collect();

        let chunks = mtmd
            .tokenize(
                MtmdInputText {
                    text: prompt,
                    add_special: true,
                    parse_special: true,
                },
                &bitmap_refs,
            )
            .map_err(|e| LlmProviderError::LocalLlm(format!("mtmd tokenize: {e}")))?;

        let n_past = chunks
            .eval_chunks(mtmd, &ctx, 0, 0, 512, true)
            .map_err(|e| LlmProviderError::LocalLlm(format!("mtmd eval: {e}")))?;

        let mut batch = LlamaBatch::new(512, 1);
        sample_tokens(
            loaded,
            &mut ctx,
            &mut batch,
            json_schema,
            deadline,
            MAX_VISION_TOKENS,
            n_past,
        )
    })
}

fn sample_tokens(
    loaded: &LoadedModel,
    ctx: &mut llama_cpp_2::context::LlamaContext<'_>,
    batch: &mut LlamaBatch,
    json_schema: Option<&serde_json::Value>,
    deadline: Instant,
    max_tokens: i32,
    mut n_cur: i32,
) -> Result<String, LlmProviderError> {
    let mut sampler = build_sampler(&loaded.model, json_schema)?;
    let mut decoder = encoding_rs::UTF_8.new_decoder();
    let mut output = String::new();
    let mut n_decode = 0_i32;

    // After mtmd eval, logits are already for the last position; first sample
    // uses the current context without an empty batch add when n_cur > 0 from text path.
    // Text path leaves the last prompt token in `batch`; vision path starts from n_past.
    let mut need_initial_from_ctx = batch.n_tokens() == 0;

    while n_decode < max_tokens {
        if Instant::now() >= deadline {
            return Err(LlmProviderError::LocalLlm(
                "decision generation timed out after deadline".to_string(),
            ));
        }

        let idx = if need_initial_from_ctx {
            need_initial_from_ctx = false;
            // Sampler idx is the logits *output* index from the last eval (-1 = last),
            // not the absolute KV position. Passing n_cur-1 yields nullptr logits.
            -1
        } else {
            batch.n_tokens().saturating_sub(1)
        };

        // `llama_sampler_sample` already calls `llama_sampler_accept` — do not accept twice.
        // A second accept empties grammar stacks (e.g. after compound token `{"`) and the next
        // sample hits GGML_ASSERT(!stacks.empty()).
        let token = sampler.sample(ctx, idx);

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
        ctx.decode(batch)
            .map_err(|e| map_llama_err("decode generated", e))?;
        n_cur = n_cur.saturating_add(1);
        n_decode = n_decode.saturating_add(1);
    }

    Ok(output)
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
    with_media_markers: bool,
) -> Result<String, LlmProviderError> {
    let chat: Vec<LlamaChatMessage> = messages
        .iter()
        .map(|m| llm_message_to_chat(m, with_media_markers))
        .collect::<Result<Vec<_>, _>>()?;

    match model.chat_template(None) {
        Ok(tmpl) => match model.apply_chat_template(&tmpl, &chat, true) {
            Ok(prompt) => Ok(prompt),
            Err(e) => {
                // Gemma 4 Jinja is too complex for llama_chat_apply_template (FFI -1).
                tracing::debug!(
                    component = "LlamaCpp",
                    error = %e,
                    "apply_chat_template failed; using Gemma 4 turn fallback"
                );
                Ok(format_gemma4_prompt(messages, with_media_markers))
            }
        },
        Err(_) => Ok(format_gemma4_prompt(messages, with_media_markers)),
    }
}

fn llm_message_to_chat(
    msg: &LlmMessage,
    with_media_markers: bool,
) -> Result<LlamaChatMessage, LlmProviderError> {
    match msg {
        LlmMessage::System { content } => LlamaChatMessage::new("system".into(), content.clone())
            .map_err(|e| map_llama_err("chat message", e)),
        LlmMessage::User { parts } => {
            let text = flatten_user_parts(parts, with_media_markers);
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

fn flatten_user_parts(parts: &[UserMessagePart], with_media_markers: bool) -> String {
    let marker = mtmd_default_marker();
    let mut text = String::new();
    let mut image_count = 0_usize;
    for part in parts {
        match part {
            UserMessagePart::Text { text: t } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(t);
            }
            UserMessagePart::Image { .. } if with_media_markers => {
                image_count = image_count.saturating_add(1);
            }
            UserMessagePart::Image { .. } => {}
        }
    }
    if with_media_markers && image_count > 0 {
        let already = text.matches(marker).count();
        let need = image_count.saturating_sub(already);
        if need > 0 {
            let mut prefix = String::new();
            for _ in 0..need {
                prefix.push_str(marker);
                prefix.push('\n');
            }
            if text.is_empty() {
                text = prefix;
            } else {
                prefix.push_str(&text);
                text = prefix;
            }
        }
    }
    text
}

/// Hand-rolled Gemma 4 turns when embedded Jinja cannot be applied via FFI.
fn format_gemma4_prompt(messages: &[LlmMessage], with_media_markers: bool) -> String {
    let mut out = String::new();
    let mut pending_system = String::new();

    for msg in messages {
        match msg {
            LlmMessage::System { content } => {
                if !pending_system.is_empty() {
                    pending_system.push_str("\n\n");
                }
                pending_system.push_str(content);
            }
            LlmMessage::User { parts } => {
                out.push_str("<|turn>user\n");
                if !pending_system.is_empty() {
                    out.push_str(&pending_system);
                    out.push_str("\n\n");
                    pending_system.clear();
                }
                out.push_str(&flatten_user_parts(parts, with_media_markers));
                out.push_str("\n<turn|>\n");
            }
            LlmMessage::Assistant { content, .. } => {
                out.push_str("<|turn>model\n");
                if let Some(c) = content {
                    out.push_str(c);
                }
                out.push_str("\n<turn|>\n");
            }
            LlmMessage::Tool { content, .. } => {
                out.push_str("<|turn>user\nTool result: ");
                out.push_str(content);
                out.push_str("\n<turn|>\n");
            }
        }
    }

    if !pending_system.is_empty() {
        out.push_str("<|turn>user\n");
        out.push_str(&pending_system);
        out.push_str("\n<turn|>\n");
    }

    out.push_str("<|turn>model\n");
    out
}

fn messages_have_images(messages: &[LlmMessage]) -> bool {
    messages.iter().any(|m| match m {
        LlmMessage::User { parts } => parts
            .iter()
            .any(|p| matches!(p, UserMessagePart::Image { .. })),
        _ => false,
    })
}

fn extract_rgb_images(
    messages: &[LlmMessage],
) -> Result<Vec<(u32, u32, Vec<u8>)>, LlmProviderError> {
    let mut out = Vec::new();
    for msg in messages {
        let LlmMessage::User { parts } = msg else {
            continue;
        };
        for part in parts {
            let UserMessagePart::Image { base64_image_data } = part else {
                continue;
            };
            if base64_image_data.is_empty() {
                continue;
            }
            out.push(decode_data_uri_to_rgb(base64_image_data)?);
        }
    }
    Ok(out)
}

fn decode_data_uri_to_rgb(data_uri: &str) -> Result<(u32, u32, Vec<u8>), LlmProviderError> {
    use base64::Engine;

    let b64 = data_uri.split_once(',').map_or(data_uri, |(_, data)| data);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| LlmProviderError::LocalLlm(format!("image base64 decode: {e}")))?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| LlmProviderError::LocalLlm(format!("image decode: {e}")))?
        .to_rgb8();
    let (w, h) = img.dimensions();
    Ok((w, h, img.into_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemma4_fallback_includes_turns_and_media_marker() {
        let marker = mtmd_default_marker();
        let messages = [
            LlmMessage::System {
                content: "Be brief.".into(),
            },
            LlmMessage::User {
                parts: vec![
                    UserMessagePart::Text {
                        text: "Summarize this screen.".into(),
                    },
                    UserMessagePart::Image {
                        base64_image_data: String::new(),
                    },
                ],
            },
        ];
        let prompt = format_gemma4_prompt(&messages, true);
        assert!(prompt.contains("<|turn>user\n"), "{prompt}");
        assert!(prompt.contains("Be brief."), "{prompt}");
        assert!(prompt.contains("Summarize this screen."), "{prompt}");
        assert!(prompt.contains(marker), "{prompt}");
        assert!(prompt.ends_with("<|turn>model\n"), "{prompt}");
        assert_eq!(prompt.matches(marker).count(), 1, "{prompt}");
    }

    #[test]
    fn flatten_user_parts_does_not_duplicate_marker() {
        let marker = mtmd_default_marker();
        let parts = [
            UserMessagePart::Text {
                text: format!("hello\n{marker}"),
            },
            UserMessagePart::Image {
                base64_image_data: String::new(),
            },
        ];
        let flat = flatten_user_parts(&parts, true);
        assert_eq!(flat.matches(marker).count(), 1, "{flat}");
    }
}
