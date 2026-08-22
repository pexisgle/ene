use std::time::Duration;

use ene_kernel::{TextDeltaSink, is_retryable_provider_failure, retry_call};
use ene_plugin_ipc::{LlmGenerateRequest, LlmGeneration};

use crate::CoreDaemon;

pub(crate) async fn generate_llm(
    core: &CoreDaemon,
    row_id: &str,
    request: LlmGenerateRequest,
) -> Result<LlmGeneration, String> {
    let policy = core.harness().retry;
    retry_call(
        &policy,
        |err: &String| is_retryable_provider_failure(err),
        || {
            let request = request.clone();
            async move {
                let generation = core
                    .supervisor()
                    .generate_llm(row_id, request)
                    .await
                    .map_err(|err| err.to_string())?;
                validate_generation(generation)
            }
        },
    )
    .await
}

/// Stream one provider generation while preserving safe retry semantics.
///
/// A transient failure is retried only before any text/thinking delta has been
/// delivered. Once the caller has observed a chunk, replaying the request would
/// duplicate visible output, so the original failure is returned immediately.
pub(crate) async fn generate_llm_streaming(
    core: &CoreDaemon,
    row_id: &str,
    request: LlmGenerateRequest,
    sink: &mut dyn TextDeltaSink,
) -> Result<LlmGeneration, String> {
    let policy = core.harness().retry;
    let max_attempts = policy.max_attempts.max(1);
    let mut attempt = 0_u32;

    loop {
        let mut emitted = false;
        let result = core
            .supervisor()
            .generate_llm_streaming(row_id, request.clone(), |chunk| {
                if !chunk.text.is_empty() {
                    emitted = true;
                    sink.on_text(&chunk.text);
                }
                if let Some(thinking) = chunk.thinking.as_deref()
                    && !thinking.is_empty()
                {
                    emitted = true;
                    sink.on_thinking(thinking);
                }
            })
            .await
            .map_err(|err| err.to_string())
            .and_then(validate_generation);

        match result {
            Ok(generation) => return Ok(generation),
            Err(err) => {
                let next = attempt.saturating_add(1);
                if emitted || !is_retryable_provider_failure(&err) || next >= max_attempts {
                    return Err(err);
                }
                let idx = usize::try_from(attempt).unwrap_or(usize::MAX);
                let delay_ms = policy
                    .backoff_ms
                    .get(idx)
                    .copied()
                    .or_else(|| policy.backoff_ms.last().copied())
                    .unwrap_or(0);
                tracing::warn!(
                    attempt = next,
                    delay_ms,
                    error = %err,
                    "retryable streaming provider error before first chunk; backing off"
                );
                tokio::time::sleep(Duration::from_millis(u64::from(delay_ms))).await;
                attempt = next;
            }
        }
    }
}

fn validate_generation(generation: LlmGeneration) -> Result<LlmGeneration, String> {
    if generation.finish_reason != "error" {
        return Ok(generation);
    }
    Err(if generation.model_id.is_empty() {
        "provider failed".to_owned()
    } else {
        generation.model_id
    })
}
