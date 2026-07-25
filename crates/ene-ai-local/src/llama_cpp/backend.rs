//! Process-wide `LlamaBackend` initialization.

use ene_ai::error::LlmProviderError;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::{LogOptions, send_logs_to_tracing};
use parking_lot::Mutex;
use std::os::raw::{c_char, c_int, c_uint, c_void};
use std::sync::OnceLock;

static BACKEND: OnceLock<Result<LlamaBackend, String>> = OnceLock::new();
static INFERENCE: Mutex<()> = Mutex::new(());

// Quiet llama.cpp `common/log` (LOG_INF / LOG_DBG spam).
// These bypass `llama_log_set` and go through `common_log_*` (C++ Itanium ABI on Unix).
#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "_Z30common_log_set_verbosity_tholdi"]
    fn common_log_set_verbosity_thold(verbosity: c_int);
}

/// Suppress common-log INFO/DEBUG (INFO=3, DEBUG=5).
#[cfg(unix)]
const COMMON_LOG_ERRORS_ONLY: c_int = 1;

// mtmd / clip log to their own callback (default: always fputs stderr).
// `add_text:` prompt dumps and "encoding image slice..." come from here —
// `common_log_set_verbosity_thold` does not affect them.
unsafe extern "C" {
    fn mtmd_helper_log_set(
        log_callback: Option<unsafe extern "C" fn(c_uint, *const c_char, *mut c_void)>,
        user_data: *mut c_void,
    );
}

/// `GGML_LOG_LEVEL_ERROR` — keep real failures, drop DEBUG/INFO/WARN spam.
const GGML_LOG_LEVEL_ERROR: c_uint = 4;

unsafe extern "C" fn mtmd_log_errors_only(
    level: c_uint,
    text: *const c_char,
    _user_data: *mut c_void,
) {
    if level < GGML_LOG_LEVEL_ERROR || text.is_null() {
        return;
    }
    // SAFETY: llama.cpp passes a NUL-terminated C string for the log line.
    let Ok(msg) = unsafe { std::ffi::CStr::from_ptr(text) }.to_str() else {
        return;
    };
    let msg = msg.trim_end();
    if !msg.is_empty() {
        tracing::error!(target: "mtmd", "{msg}");
    }
}

fn quiet_llama_logs() {
    #[cfg(unix)]
    {
        // SAFETY: sets a process-global llama.cpp log verbosity threshold; called once at init.
        unsafe {
            common_log_set_verbosity_thold(COMMON_LOG_ERRORS_ONLY);
        }
    }
    // SAFETY: installs a process-global mtmd/clip log callback; called once at init.
    // Also sets `mtmd_log_set` internally, covering `add_text` and encode timing.
    unsafe {
        mtmd_helper_log_set(Some(mtmd_log_errors_only), std::ptr::null_mut());
    }
}

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
        quiet_llama_logs();
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

        let handles: Vec<_> = std::iter::repeat_with(|| {
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
        .take(4)
        .collect();

        for handle in handles {
            handle.join().expect("thread join");
        }

        assert_eq!(max_active.load(Ordering::SeqCst), 1);
    }
}
