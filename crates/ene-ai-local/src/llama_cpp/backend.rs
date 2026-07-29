//! Process-wide `LlamaBackend` initialization.

use ene_ai::error::LlmProviderError;
use llama_cpp_4::llama_backend::LlamaBackend;
use llama_cpp_4::mtmd::MtmdContext;
use once_cell::sync::OnceCell;

/// Cache *success* only. A `std::sync::OnceLock<Result<LlamaBackend, String>>`
/// (the previous design) permanently remembers an initialization failure —
/// one transient GPU-init hiccup would disable local inference for the rest
/// of the process. `once_cell::sync::OnceCell::get_or_try_init` gives the
/// same "run the initializer exactly once, block concurrent callers until it
/// finishes" guarantee as `std::sync::OnceLock::get_or_init` (std's
/// equivalent fallible method, `get_or_try_init`, is nightly-only), but
/// leaves the cell empty on `Err` so the next call gets a fresh attempt
/// instead of a cached failure. No separate init-lock is needed:
/// `get_or_try_init` already serializes concurrent callers on its own.
static BACKEND: OnceCell<LlamaBackend> = OnceCell::new();

/// Run `f` with the process-global backend (initialized once, retried on a
/// prior failed attempt).
///
/// Each model instance (`LoadedModel`) maintains its own internal lock so separate
/// models (e.g. embedding vs decision vs vision) do not block each other globally.
pub(crate) fn with_backend<T, F>(f: F) -> Result<T, LlmProviderError>
where
    F: FnOnce(&LlamaBackend) -> Result<T, LlmProviderError>,
{
    let backend = BACKEND.get_or_try_init(|| -> Result<LlamaBackend, LlmProviderError> {
        let mut backend = LlamaBackend::init()
            .map_err(|e| LlmProviderError::LocalLlm(format!("LlamaBackend::init failed: {e}")))?;
        // Quiet llama.cpp / mtmd helper stderr; keep failures for tracing if needed.
        backend.void_logs();
        MtmdContext::void_helper_logs();
        Ok(backend)
    })?;
    f(backend)
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
