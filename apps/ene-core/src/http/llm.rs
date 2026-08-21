use ene_kernel::{is_retryable_provider_failure, retry_call};
use ene_plugin_ipc::{LlmGenerateRequest, LlmGeneration};

use crate::CoreDaemon;

pub(crate) async fn generate_llm(
    core: &CoreDaemon,
    row_id: &str,
    request: LlmGenerateRequest,
) -> Result<LlmGeneration, String> {
    let policy = crate::boot::load_harness_settings(core.data_dir()).retry;
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
                if generation.finish_reason == "error" {
                    let msg = if generation.model_id.is_empty() {
                        "provider failed".to_owned()
                    } else {
                        generation.model_id
                    };
                    return Err(msg);
                }
                Ok(generation)
            }
        },
    )
    .await
}
