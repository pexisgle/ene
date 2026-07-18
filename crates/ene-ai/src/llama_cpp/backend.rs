//! Process-wide `LlamaBackend` initialization.

use crate::error::LlmProviderError;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::{LogOptions, send_logs_to_tracing};
use parking_lot::Mutex;
use std::sync::OnceLock;

static BACKEND: OnceLock<Result<LlamaBackend, String>> = OnceLock::new();
static INFERENCE: Mutex<()> = Mutex::new(());

/// Run `f` with the process-global backend (initialized once).
///
/// Holds a process-wide inference lock so embedding and decision paths never
/// enter llama.cpp concurrently from different threads.
pub(crate) fn with_backend<T, F>(f: F) -> Result<T, LlmProviderError>
where
    F: FnOnce(&LlamaBackend) -> Result<T, LlmProviderError>,
{
    let _guard = INFERENCE.lock();
    let backend = BACKEND.get_or_init(|| {
        send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));
        LlamaBackend::init().map_err(|e| format!("LlamaBackend::init failed: {e}"))
    });
    match backend {
        Ok(b) => f(b),
        Err(msg) => Err(LlmProviderError::LocalLlm(msg.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn inference_lock_serializes_concurrent_callers() {
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let active = Arc::clone(&active);
                let max_active = Arc::clone(&max_active);
                std::thread::spawn(move || {
                    let _guard = INFERENCE.lock();
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    max_active.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(5));
                    active.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("thread join");
        }

        assert_eq!(max_active.load(Ordering::SeqCst), 1);
    }
}
