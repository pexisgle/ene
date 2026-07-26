//! Process-wide `LlamaBackend` initialization.

use ene_ai::error::LlmProviderError;
use llama_cpp_4::llama_backend::LlamaBackend;
use llama_cpp_4::mtmd::MtmdContext;
use parking_lot::Mutex;
use std::sync::OnceLock;

static BACKEND: OnceLock<Result<LlamaBackend, String>> = OnceLock::new();
static INIT_LOCK: Mutex<()> = Mutex::new(());

/// Run `f` with the process-global backend (initialized once).
///
/// Each model instance (`LoadedModel`) maintains its own internal lock so separate
/// models (e.g. embedding vs decision vs vision) do not block each other globally.
pub(crate) fn with_backend<T, F>(f: F) -> Result<T, LlmProviderError>
where
    F: FnOnce(&LlamaBackend) -> Result<T, LlmProviderError>,
{
    let backend = BACKEND.get_or_init(|| {
        let _guard = INIT_LOCK.lock();
        let mut backend =
            LlamaBackend::init().map_err(|e| format!("LlamaBackend::init failed: {e}"))?;
        // Quiet llama.cpp / mtmd helper stderr; keep failures for tracing if needed.
        backend.void_logs();
        MtmdContext::void_helper_logs();
        Ok(backend)
    });
    match backend {
        Ok(b) => f(b),
        Err(msg) => Err(LlmProviderError::LocalLlm(msg.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_init_once() {
        let res = with_backend(|_| Ok(42));
        assert!(res.is_ok());
    }
}
