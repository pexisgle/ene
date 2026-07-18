//! Process-wide `LlamaBackend` initialization.

use crate::error::LlmProviderError;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::{LogOptions, send_logs_to_tracing};
use std::sync::OnceLock;

static BACKEND: OnceLock<Result<LlamaBackend, String>> = OnceLock::new();

/// Run `f` with the process-global backend (initialized once).
pub(crate) fn with_backend<T, F>(f: F) -> Result<T, LlmProviderError>
where
    F: FnOnce(&LlamaBackend) -> Result<T, LlmProviderError>,
{
    let backend = BACKEND.get_or_init(|| {
        send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));
        LlamaBackend::init().map_err(|e| format!("LlamaBackend::init failed: {e}"))
    });
    match backend {
        Ok(b) => f(b),
        Err(msg) => Err(LlmProviderError::LocalLlm(msg.clone())),
    }
}
