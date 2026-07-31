//! Chat completion via llama-cpp-4 (text + optional mtmd vision).
//!
//! Every function here takes an already-built `ctx: &mut LlamaContext<'_>`
//! (cached worker-local state owned by `crate::local_llm::model::LlamaChatModel`,
//! not created here) and a `job: &JobContext` for cooperative cancellation —
//! there is deliberately no `timeout: Duration` parameter and no internal
//! `Instant`-based deadline: the one timeout in this crate's design lives in
//! `ene_infer::EngineConfig::job_timeout`, checked via `job.should_stop()`.

use super::grammar::json_schema_to_grammar;
use super::load::LoadedModel;
use super::map_llama_err;
use ene_ai::error::LlmProviderError;
use ene_ai::message::{LlmMessage, UserMessagePart};
use ene_infer::{ChunkSink, JobContext};
use llama_cpp_4::context::LlamaContext;
use llama_cpp_4::llama_batch::LlamaBatch;
use llama_cpp_4::model::{AddBos, LlamaChatMessage, LlamaModel, Special};
use llama_cpp_4::mtmd::{MtmdBitmap, MtmdContext, MtmdInputChunks, MtmdInputText};
use llama_cpp_4::sampling::LlamaSampler;
use llama_cpp_4::token::detokenizer::StreamDetokenizer;

const MAX_DECISION_TOKENS: i32 = 320;
const MAX_VISION_TOKENS: i32 = 256;

/// The text a local llama.cpp generation produced, plus the exact token
/// counts the engine observed while producing it (#365).
///
/// llama.cpp knows both numbers precisely — the prompt length in tokens at
/// decode time and the number of tokens it sampled — so a local model reports
/// real usage instead of the character-based estimate remote callers fall back
/// to. `prompt_tokens` is the count fed into the context (for the vision path
/// this is the mtmd-evaluated chunk count, the closest available proxy);
/// `completion_tokens` is how many tokens `sample_tokens` emitted before the
/// end-of-generation token or the max-token cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Generated {
    /// The full detokenized assistant text.
    pub(crate) text: String,
    /// Tokens the prompt occupied in the context.
    pub(crate) prompt_tokens: u32,
    /// Tokens sampled for the completion.
    pub(crate) completion_tokens: u32,
}

impl Generated {
    /// The exact token usage this generation observed (#365).
    pub(crate) fn usage(&self) -> ene_ai::TokenUsage {
        ene_ai::TokenUsage::new(
            self.prompt_tokens,
            self.completion_tokens,
            self.prompt_tokens.saturating_add(self.completion_tokens),
        )
    }
}

/// One incremental unit a streaming llama.cpp chat job hands back (#365).
///
/// Mostly detokenized text pieces — exactly what [`sample_tokens`] already
/// produced per token before this type existed — plus a single trailing
/// [`LlamaStreamChunk::Usage`] carrying the generation's exact token counts,
/// so [`ene_ai::StreamingLocalLlmEngine`]'s stream ends on a chunk whose
/// `usage` is populated. That mirrors how remote providers report usage on
/// their final SSE chunk and is what lets a local model satisfy the "streaming
/// usage arrives on the final chunk" contract without a second round trip.
#[derive(Debug, Clone)]
pub(crate) enum LlamaStreamChunk {
    /// A detokenized text piece.
    Delta(String),
    /// Token usage for the whole completion, sent once as the final chunk.
    Usage(ene_ai::TokenUsage),
}

/// Generate a completion for `messages` on the caller's cached `ctx`,
/// optionally constrained by JSON schema grammar.
///
/// `sink`, when `Some`, receives each detokenized piece as it is sampled (see
/// [`sample_tokens`]) in addition to the full text still being returned —
/// `None` reproduces this function's pre-streaming behavior exactly, used by
/// [`crate::local_llm::model::LlamaChatModel::run`]'s one-shot path.
pub(crate) fn generate_chat(
    loaded: &LoadedModel,
    ctx: &mut LlamaContext<'_>,
    messages: &[LlmMessage],
    json_schema: Option<&serde_json::Value>,
    job: &JobContext,
    sink: Option<&ChunkSink<LlamaStreamChunk, LlmProviderError>>,
) -> Result<Generated, LlmProviderError> {
    if messages_have_images(messages) {
        return generate_chat_vision(loaded, ctx, messages, json_schema, job, sink);
    }
    generate_chat_text(loaded, ctx, messages, json_schema, job, sink)
}

/// Vision completion from raw RGB8 pixels (desktop screen summary), on the
/// caller's cached `ctx`. `sink` is threaded through exactly like
/// [`generate_chat`]'s.
pub(crate) fn generate_vision(
    loaded: &LoadedModel,
    ctx: &mut LlamaContext<'_>,
    system: &str,
    user: &str,
    width: u32,
    height: u32,
    rgb: &[u8],
    job: &JobContext,
    sink: Option<&ChunkSink<LlamaStreamChunk, LlmProviderError>>,
) -> Result<Generated, LlmProviderError> {
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
    generate_chat_vision_with_bitmaps(
        loaded,
        ctx,
        &messages,
        &[(width, height, rgb)],
        None,
        job,
        sink,
    )
}

fn generate_chat_text(
    loaded: &LoadedModel,
    ctx: &mut LlamaContext<'_>,
    messages: &[LlmMessage],
    json_schema: Option<&serde_json::Value>,
    job: &JobContext,
    sink: Option<&ChunkSink<LlamaStreamChunk, LlmProviderError>>,
) -> Result<Generated, LlmProviderError> {
    let prompt = apply_messages_template(loaded, messages, false)?;
    let tokens = loaded
        .model
        .str_to_token(&prompt, AddBos::Always)
        .map_err(|e| map_llama_err("tokenize prompt", e))?;

    if tokens.is_empty() {
        return Err(LlmProviderError::LocalLlm(
            "prompt tokenized to empty sequence".to_string(),
        ));
    }

    let prompt_tokens = u32::try_from(tokens.len()).unwrap_or(u32::MAX);

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
    if job.should_stop().is_some() {
        return Err(LlmProviderError::LocalLlm(
            "decision generation stopped before prompt decode".to_string(),
        ));
    }
    ctx.decode(&mut batch)
        .map_err(|e| map_llama_err("decode prompt", e))?;
    job.tick();

    let n_cur = batch.n_tokens();
    let Sampled {
        text,
        completion_tokens,
    } = sample_tokens(
        loaded,
        ctx,
        &mut batch,
        json_schema,
        job,
        MAX_DECISION_TOKENS,
        n_cur,
        sink,
    )?;
    let generated = Generated {
        text,
        prompt_tokens,
        completion_tokens,
    };
    send_usage_chunk(sink, &generated, job)?;
    Ok(generated)
}

/// Pushes the generation's exact token usage through `sink` as a single
/// trailing [`LlamaStreamChunk::Usage`] (#365), so a streaming consumer sees
/// usage on its final chunk exactly as it would from a remote provider. A
/// `None` sink (the one-shot `run` path) does nothing. A stop condition while
/// sending is surfaced like any other, so the caller returns promptly.
fn send_usage_chunk(
    sink: Option<&ChunkSink<LlamaStreamChunk, LlmProviderError>>,
    generated: &Generated,
    job: &JobContext,
) -> Result<(), LlmProviderError> {
    if let Some(sink) = sink
        && sink
            .send(LlamaStreamChunk::Usage(generated.usage()), job)
            .is_err()
    {
        return Err(LlmProviderError::LocalLlm(
            "generation stopped while streaming the usage chunk".to_string(),
        ));
    }
    Ok(())
}

fn generate_chat_vision(
    loaded: &LoadedModel,
    ctx: &mut LlamaContext<'_>,
    messages: &[LlmMessage],
    json_schema: Option<&serde_json::Value>,
    job: &JobContext,
    sink: Option<&ChunkSink<LlamaStreamChunk, LlmProviderError>>,
) -> Result<Generated, LlmProviderError> {
    let bitmaps = extract_rgb_images(messages)?;
    if bitmaps.is_empty() {
        return generate_chat_text(loaded, ctx, messages, json_schema, job, sink);
    }
    let refs: Vec<(u32, u32, &[u8])> = bitmaps
        .iter()
        .map(|(w, h, data)| (*w, *h, data.as_slice()))
        .collect();
    generate_chat_vision_with_bitmaps(loaded, ctx, messages, &refs, json_schema, job, sink)
}

fn generate_chat_vision_with_bitmaps(
    loaded: &LoadedModel,
    ctx: &mut LlamaContext<'_>,
    messages: &[LlmMessage],
    images: &[(u32, u32, &[u8])],
    json_schema: Option<&serde_json::Value>,
    job: &JobContext,
    sink: Option<&ChunkSink<LlamaStreamChunk, LlmProviderError>>,
) -> Result<Generated, LlmProviderError> {
    let mtmd = loaded.mtmd.as_ref().ok_or_else(|| {
        LlmProviderError::LocalLlm(
            "vision requested but mmproj is not loaded for this local model".to_string(),
        )
    })?;
    if !mtmd.supports_vision() {
        return Err(LlmProviderError::LocalLlm(
            "loaded mmproj does not support vision".to_string(),
        ));
    }

    let prompt = apply_messages_template(loaded, messages, true)?;
    let bitmaps: Vec<MtmdBitmap> = images
        .iter()
        .map(|(w, h, rgb)| {
            MtmdBitmap::from_rgb(*w, *h, rgb)
                .map_err(|e| LlmProviderError::LocalLlm(format!("mtmd bitmap: {e}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let bitmap_refs: Vec<&MtmdBitmap> = bitmaps.iter().collect();

    let text = MtmdInputText::new(&prompt, true, true);
    let mut chunks = MtmdInputChunks::new();
    mtmd.tokenize(&text, &bitmap_refs, &mut chunks)
        .map_err(|e| LlmProviderError::LocalLlm(format!("mtmd tokenize: {e}")))?;

    if job.should_stop().is_some() {
        return Err(LlmProviderError::LocalLlm(
            "vision generation stopped before mtmd eval".to_string(),
        ));
    }

    // `mtmd.eval_chunks` is a single opaque FFI call (no natural interruption
    // point, hence no `job.tick()` mid-call — see `CHAT_STALL_TIMEOUT`'s docs
    // in `local_llm::model` for why this engine sets no stall timeout).
    let mut n_past = 0_i32;
    mtmd.eval_chunks(ctx.as_ptr(), &chunks, 0, 0, 512, true, &mut n_past)
        .map_err(|e| LlmProviderError::LocalLlm(format!("mtmd eval: {e}")))?;
    job.tick();

    // `n_past` is how many positions the mtmd eval consumed in the context —
    // text plus expanded image patches — and is the closest available proxy
    // for the vision path's prompt token count.
    let prompt_tokens = u32::try_from(n_past).unwrap_or(u32::MAX);

    let mut batch = LlamaBatch::new(512, 1);
    let Sampled {
        text,
        completion_tokens,
    } = sample_tokens(
        loaded,
        ctx,
        &mut batch,
        json_schema,
        job,
        MAX_VISION_TOKENS,
        n_past,
        sink,
    )?;
    let generated = Generated {
        text,
        prompt_tokens,
        completion_tokens,
    };
    send_usage_chunk(sink, &generated, job)?;
    Ok(generated)
}

/// The output of [`sample_tokens`]: the accumulated text plus how many tokens
/// were sampled to produce it (#365).
struct Sampled {
    text: String,
    completion_tokens: u32,
}

/// Samples tokens one at a time until an end-of-generation token or
/// `max_tokens` is reached, always returning the full accumulated text.
///
/// `sink`, when `Some`, additionally receives each non-empty detokenized
/// piece as it is produced — this is the only change this stage makes to a
/// loop that already checked `should_stop`/called `tick` per token; a
/// caller passing `None` (every existing call site via
/// [`crate::local_llm::model::LlamaChatModel::run`]) sees no behavior change
/// at all. `ChunkSink::send` reports a stop condition (deadline,
/// cancellation, or a dropped consumer) the exact same way `should_stop`
/// does elsewhere in this loop, so it is handled identically: give up and
/// return promptly.
fn sample_tokens(
    loaded: &LoadedModel,
    ctx: &mut LlamaContext<'_>,
    batch: &mut LlamaBatch,
    json_schema: Option<&serde_json::Value>,
    job: &JobContext,
    max_tokens: i32,
    mut n_cur: i32,
    sink: Option<&ChunkSink<LlamaStreamChunk, LlmProviderError>>,
) -> Result<Sampled, LlmProviderError> {
    let sampler = build_sampler(&loaded.model, json_schema)?;
    let mut detok = StreamDetokenizer::new(&loaded.model, Special::Plaintext);
    let mut output = String::new();
    let mut n_decode = 0_i32;

    while n_decode < max_tokens {
        // Unlike a single opaque FFI call (mtmd eval, a huge prompt decode),
        // token-by-token sampling *is* a real interruption point: check and
        // tick every iteration, per this stage's cancellation contract.
        if job.should_stop().is_some() {
            return Err(LlmProviderError::LocalLlm(
                "generation stopped before completion".to_string(),
            ));
        }
        job.tick();

        // Always sample the last computed logits. After a multi-token prompt decode
        // only the final token requests logits, so `batch.n_tokens()-1` is out of range.
        // `llama_sampler_sample` already calls `llama_sampler_accept` — do not accept twice.
        // A second accept empties grammar stacks (e.g. after compound token `{"`) and the next
        // sample hits GGML_ASSERT(!stacks.empty()). That `abort()` cannot be caught by
        // `catch_unwind` (see `ene_infer`'s crate docs on this exact limit) — this code path
        // avoids the double-accept that would trigger it, but a genuinely malformed grammar
        // reaching llama.cpp by some other route is not something this crate can safely contain.
        let token = sampler.sample(ctx, -1);

        if loaded.model.is_eog_token(token) {
            break;
        }

        let piece = detok
            .push(token)
            .map_err(|e| map_llama_err("detokenize", e))?;
        if let Some(sink) = sink
            && !piece.is_empty()
            && sink
                .send(LlamaStreamChunk::Delta(piece.clone()), job)
                .is_err()
        {
            return Err(LlmProviderError::LocalLlm(
                "generation stopped while streaming a chunk".to_string(),
            ));
        }
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

    let tail = detok
        .finish()
        .map_err(|e| map_llama_err("detokenize finish", e))?;
    if let Some(sink) = sink
        && !tail.is_empty()
        && sink
            .send(LlamaStreamChunk::Delta(tail.clone()), job)
            .is_err()
    {
        return Err(LlmProviderError::LocalLlm(
            "generation stopped while streaming the final chunk".to_string(),
        ));
    }
    output.push_str(&tail);

    Ok(Sampled {
        text: output,
        completion_tokens: u32::try_from(n_decode).unwrap_or(u32::MAX),
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
        let grammar = json_schema_to_grammar(&schema_str)?;
        // llama-cpp-4's grammar constructor panics on invalid grammar / NUL; our shim
        // returns a validated GBNF string and root is fixed.
        let grammar_sampler = LlamaSampler::grammar(model, &grammar, "root");
        chain.push(grammar_sampler);
    }
    chain.push(LlamaSampler::greedy());
    Ok(LlamaSampler::chain_simple(chain))
}

fn apply_messages_template(
    loaded: &LoadedModel,
    messages: &[LlmMessage],
    with_media_markers: bool,
) -> Result<String, LlmProviderError> {
    let model = &loaded.model;
    let chat: Vec<LlamaChatMessage> = messages
        .iter()
        .map(|m| llm_message_to_chat(m, with_media_markers))
        .collect::<Result<Vec<_>, _>>()?;

    match model.apply_chat_template(None, &chat, true) {
        Ok(prompt) => Ok(prompt),
        Err(e) => {
            let model_lower = loaded.model_name.to_ascii_lowercase();
            let is_gemma = model_lower.contains("gemma");
            if is_gemma {
                tracing::debug!(
                    component = "LlamaCpp",
                    error = %e,
                    "apply_chat_template failed; using Gemma 4 turn fallback"
                );
            } else {
                tracing::warn!(
                    component = "LlamaCpp",
                    error = %e,
                    model = %loaded.model_name,
                    "apply_chat_template failed for non-Gemma model; using Gemma 4 fallback \
                     which may produce incorrect output"
                );
            }
            Ok(format_gemma4_prompt(messages, with_media_markers))
        }
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
    let marker = MtmdContext::default_marker();
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
        let marker = MtmdContext::default_marker();
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
        let marker = MtmdContext::default_marker();
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
