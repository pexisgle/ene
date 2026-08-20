//! Waterfall (intercept/rewrite via `next`) vs emit (notify only). P-1007.

use parking_lot::Mutex;
use std::sync::Arc;

/// Continuation a waterfall listener must call to pass the event down the chain.
pub type WaterfallNext<T> = Box<dyn FnOnce(T) -> T + Send>;

type WaterfallFn<T> = Arc<dyn Fn(T, WaterfallNext<T>) -> T + Send + Sync>;

/// Onion-style hook chain. A listener that skips `next` intercepts the event.
pub struct Waterfall<T> {
    listeners: Arc<Mutex<Vec<WaterfallFn<T>>>>,
}

impl<T> Clone for Waterfall<T> {
    fn clone(&self) -> Self {
        Self {
            listeners: Arc::clone(&self.listeners),
        }
    }
}

impl<T> Default for Waterfall<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Waterfall<T> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            listeners: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl<T: Send + 'static> Waterfall<T> {
    pub fn listen<F>(&self, listener: F)
    where
        F: Fn(T, WaterfallNext<T>) -> T + Send + Sync + 'static,
    {
        self.listeners.lock().push(Arc::new(listener));
    }

    #[must_use]
    pub fn run(&self, value: T) -> T {
        let listeners = self.listeners.lock().clone();
        apply(&listeners, 0, value)
    }
}

fn apply<T: Send + 'static>(listeners: &[WaterfallFn<T>], index: usize, value: T) -> T {
    let Some(listener) = listeners.get(index) else {
        return value;
    };
    let rest = listeners.to_vec();
    let listener = Arc::clone(listener);
    listener(
        value,
        Box::new(move |next_value| apply(&rest, index.saturating_add(1), next_value)),
    )
}

type EmitFn<T> = Arc<dyn Fn(&T) + Send + Sync>;

/// Notify-only bus. Listeners cannot replace the value.
pub struct EmitBus<T> {
    listeners: Arc<Mutex<Vec<EmitFn<T>>>>,
}

impl<T> Clone for EmitBus<T> {
    fn clone(&self) -> Self {
        Self {
            listeners: Arc::clone(&self.listeners),
        }
    }
}

impl<T> Default for EmitBus<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> EmitBus<T> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            listeners: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn listen<F>(&self, listener: F)
    where
        F: Fn(&T) + Send + Sync + 'static,
    {
        self.listeners.lock().push(Arc::new(listener));
    }

    pub fn emit(&self, value: &T) {
        let listeners = self.listeners.lock().clone();
        for listener in listeners {
            listener(value);
        }
    }
}

/// Shared payload for `agent/pre-step` and `agent/request`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookEvent {
    pub proceed: bool,
    pub note: String,
}

impl Default for HookEvent {
    fn default() -> Self {
        Self {
            proceed: true,
            note: String::new(),
        }
    }
}

/// Kernel-owned waterfall points. Third-party fibers cannot register here.
#[derive(Clone, Default)]
pub struct LoopHooks {
    pub pre_step: Waterfall<HookEvent>,
    pub request: Waterfall<HookEvent>,
}

impl LoopHooks {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}
