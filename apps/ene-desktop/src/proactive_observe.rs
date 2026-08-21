//! Privacy-aware activity observation is owned by `ene-core`.
//! Desktop keeps a no-op control so settings can still toggle the
//! local capture helpers without linking mind/runtime crates.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "kept as a local enable stub until core exposes observation controls over the API"
    )
)]

use std::sync::Arc;

use parking_lot::Mutex;

#[derive(Debug, Clone)]
pub struct ProactiveObserveControl {
    inner: Arc<Mutex<ObserveConfig>>,
}

#[derive(Debug, Clone)]
struct ObserveConfig {
    enabled: bool,
}

impl Default for ProactiveObserveControl {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ObserveConfig { enabled: false })),
        }
    }
}

impl ProactiveObserveControl {
    #[must_use]
    pub fn new(enabled: bool) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ObserveConfig { enabled })),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.inner.lock().enabled = enabled;
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.inner.lock().enabled
    }
}

pub fn spawn_proactive_observer(enabled: bool) -> ProactiveObserveControl {
    ProactiveObserveControl::new(enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_toggles_without_core() {
        let control = spawn_proactive_observer(false);
        assert!(!control.enabled());
        control.set_enabled(true);
        assert!(control.enabled());
    }
}
