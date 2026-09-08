//! Waterfall (intercept/rewrite via `next`) vs emit (notify only).

use parking_lot::Mutex;
use std::sync::Arc;

/// Continuation a waterfall listener must call to pass the event down the chain.
pub type WaterfallNext<T> = Box<dyn FnOnce(T) -> T + Send>;

type WaterfallFn<T> = Arc<dyn Fn(T, WaterfallNext<T>) -> T + Send + Sync>;

struct WaterfallInner<T> {
    listeners: Vec<(u64, WaterfallFn<T>)>,
    next_id: u64,
}

/// Onion-style hook chain. A listener that skips `next` intercepts the event.
pub struct Waterfall<T> {
    inner: Arc<Mutex<WaterfallInner<T>>>,
}

/// Drops the matching listener. Host/fiber code must retain this for the
/// registration lifetime (unload pops LIFO).
#[must_use = "dropping unregisters the waterfall listener"]
pub struct WaterfallGuard<T> {
    inner: Arc<Mutex<WaterfallInner<T>>>,
    id: u64,
}

impl<T> Drop for WaterfallGuard<T> {
    fn drop(&mut self) {
        self.inner.lock().listeners.retain(|(id, _)| *id != self.id);
    }
}

impl<T> Clone for Waterfall<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
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
            inner: Arc::new(Mutex::new(WaterfallInner {
                listeners: Vec::new(),
                next_id: 1,
            })),
        }
    }
}

impl<T: Send + 'static> Waterfall<T> {
    pub fn listen<F>(&self, listener: F) -> WaterfallGuard<T>
    where
        F: Fn(T, WaterfallNext<T>) -> T + Send + Sync + 'static,
    {
        let mut inner = self.inner.lock();
        let id = inner.next_id;
        inner.next_id = inner.next_id.saturating_add(1);
        inner.listeners.push((id, Arc::new(listener)));
        WaterfallGuard {
            inner: Arc::clone(&self.inner),
            id,
        }
    }

    #[must_use]
    pub fn run(&self, value: T) -> T {
        let listeners = self.inner.lock().listeners.clone();
        apply(&listeners, 0, value)
    }
}

fn apply<T: Send + 'static>(listeners: &[(u64, WaterfallFn<T>)], index: usize, value: T) -> T {
    let Some((_, listener)) = listeners.get(index) else {
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

/// Shared waterfall points. The host and plugin supervisor subscribe here;
/// dropping the returned guard unregisters the listener.
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
