//! Process-wide admission budgets, one shared [`tokio::sync::Semaphore`] per
//! distinct [`ResourceClass`].
//!
//! # Why this exists
//!
//! Three llama.cpp models (decision LLM, vision mmproj, embedding) can all
//! offload to `with_main_gpu(0)` with no knowledge of each other. Each has
//! its own model-level lock, and per-model locks cannot prevent them
//! contending for the GPU — the locks are separate, so nothing stops two of
//! them running inference on the same device at the same time. Concurrent
//! GPU jobs sharing one device mean VRAM exhaustion or driver-level
//! serialization that shows up as latency rather than a clear error.
//!
//! [`ResourceRegistry`] fixes this by keying a shared semaphore on
//! [`ResourceClass`] equality: any engine that declares the same class as an
//! existing one automatically starts sharing its admission budget, with no
//! change to that existing engine or to callers.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use tokio::sync::{AcquireError, OwnedSemaphorePermit, Semaphore};

use super::descriptor::{ResourceBudgets, ResourceClass};

/// The default admission budget for a [`ResourceClass`] that has not been
/// explicitly overridden via [`ResourceRegistry::configure_all`].
///
/// - [`ResourceClass::Gpu`]: **1**. Distinct local models that offload to the
///   same physical device are the exact scenario this module exists to
///   serialize; a single in-flight job per device is the safe default until
///   an operator opts into more, having confirmed their VRAM budget allows
///   it.
/// - [`ResourceClass::Cpu`]: **the number of logical CPUs**, per
///   [`std::thread::available_parallelism`] (falling back to 1 if the
///   platform cannot report it). Every CPU-bound local engine shares this
///   one class (see the type docs on [`ResourceClass::Cpu`] for why), so
///   this is a genuine capacity budget rather than a per-engine identity:
///   independently-authored CPU engines (whisper, Kokoro, a future one) may
///   run concurrently up to this many at once before further submissions
///   wait on [`ResourceRegistry::acquire`]. This is a heuristic upper bound,
///   not a guarantee against CPU oversubscription — a single job may itself
///   use more than one thread internally (whisper.cpp, ONNX Runtime's own
///   intra-op parallelism), so this only bounds how many independent *engine
///   jobs* run at once, not total OS thread count. Operators with tighter
///   constraints should override via [`ResourceRegistry::configure_all`].
/// - [`ResourceClass::Network`]: **4**. A network-attached engine (e.g. a
///   local sidecar process) is not constrained by host GPU/CPU capacity the
///   same way; a small amount of headroom avoids needlessly serializing what
///   is really I/O-bound, while still bounding fan-out.
#[must_use]
pub fn default_permits(class: ResourceClass) -> usize {
    match class {
        ResourceClass::Gpu { .. } => 1,
        ResourceClass::Cpu => {
            std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
        }
        ResourceClass::Network => 4,
    }
}

#[derive(Default)]
struct RegistryState {
    /// Permit-count overrides recorded by [`ResourceRegistry::configure_all`]
    /// for classes whose semaphore does not exist yet.
    overrides: HashMap<ResourceClass, usize>,
    /// One shared semaphore per class that has been acquired at least once.
    /// `tokio::sync::Semaphore` cannot shrink its permit count, so once a
    /// class's semaphore is created here its size is fixed for the rest of
    /// the process — matching `configure_all`'s contract that overrides only
    /// take effect before first use.
    semaphores: HashMap<ResourceClass, Arc<Semaphore>>,
}

/// Process-wide registry mapping each distinct [`ResourceClass`] to one
/// shared [`tokio::sync::Semaphore`].
///
/// A zero-sized handle type; all state lives in a process-global
/// [`OnceLock`]. There is intentionally one registry per process, not one
/// per [`crate::local_engine::llm::LocalLlmEngine`]/etc. instance — sharing
/// the budget across independently-constructed adapters is the entire
/// point.
pub struct ResourceRegistry;

impl ResourceRegistry {
    fn state() -> &'static Mutex<RegistryState> {
        static STATE: OnceLock<Mutex<RegistryState>> = OnceLock::new();
        STATE.get_or_init(|| Mutex::new(RegistryState::default()))
    }

    /// Overrides the permit count for one or more resource classes.
    ///
    /// Only takes effect for a class whose semaphore has not been created
    /// yet (i.e. no engine using that class has called [`Self::acquire`]).
    /// Call this once at process startup, before spawning any engine, for
    /// every class you want a non-default budget for. A call that arrives
    /// too late is logged and ignored rather than silently resizing a
    /// semaphore other engines are already relying on.
    pub fn configure_all(budgets: &ResourceBudgets) {
        let mut state = Self::state().lock();
        for (&class, &permits) in &budgets.0 {
            if state.semaphores.contains_key(&class) {
                tracing::warn!(
                    ?class,
                    permits,
                    "ResourceRegistry::configure_all called after this class's semaphore \
                     already existed; the new permit count is ignored"
                );
                continue;
            }
            state.overrides.insert(class, permits.max(1));
        }
    }

    /// Returns the shared semaphore for `class`, creating it on first use
    /// with the configured override (see [`Self::configure_all`]) or
    /// [`default_permits`] otherwise.
    #[must_use]
    pub fn semaphore(class: ResourceClass) -> Arc<Semaphore> {
        let mut state = Self::state().lock();
        if let Some(sem) = state.semaphores.get(&class) {
            return Arc::clone(sem);
        }
        let permits = state
            .overrides
            .get(&class)
            .copied()
            .unwrap_or_else(|| default_permits(class));
        let sem = Arc::new(Semaphore::new(permits));
        state.semaphores.insert(class, Arc::clone(&sem));
        sem
    }

    /// Acquires one permit for `class`, waiting if the budget is currently
    /// exhausted.
    ///
    /// # Errors
    ///
    /// Returns [`AcquireError`] only if the semaphore were closed, which
    /// this registry never does — kept as a `Result` (rather than assuming
    /// infallibility with `unwrap`/`expect`) because
    /// [`Semaphore::acquire_owned`] itself is fallible.
    pub async fn acquire(class: ResourceClass) -> Result<OwnedSemaphorePermit, AcquireError> {
        Self::semaphore(class).acquire_owned().await
    }
}

#[cfg(test)]
mod tests {
    use super::{ResourceClass, ResourceRegistry, default_permits};
    use crate::local_engine::descriptor::ResourceBudgets;

    #[test]
    fn default_permits_matches_documented_values() {
        assert_eq!(default_permits(ResourceClass::Gpu { device: 0 }), 1);
        let expected_cpu =
            std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
        assert_eq!(default_permits(ResourceClass::Cpu), expected_cpu);
        assert_eq!(default_permits(ResourceClass::Network), 4);
    }

    #[tokio::test]
    async fn cpu_class_default_budget_matches_available_parallelism() {
        // `ResourceClass::Cpu` is a single, process-wide value shared by
        // every CPU-bound engine (see its type docs) — this is the one test
        // in this crate that touches its real semaphore, so no other test's
        // expectations about its permit count can collide with this one.
        let expected = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
        let sem = ResourceRegistry::semaphore(ResourceClass::Cpu);
        assert_eq!(sem.available_permits(), expected);
    }

    #[tokio::test]
    async fn distinct_classes_get_independent_semaphores() {
        // Unique device indices so this test cannot collide with any other
        // test's use of the process-global registry.
        let a = ResourceClass::Gpu { device: 101 };
        let b = ResourceClass::Gpu { device: 102 };
        let sem_a = ResourceRegistry::semaphore(a);
        let sem_b = ResourceRegistry::semaphore(b);
        assert_eq!(sem_a.available_permits(), 1);
        assert_eq!(sem_b.available_permits(), 1);
        // Acquiring from `a` must not affect `b`'s budget.
        let _permit_a = ResourceRegistry::acquire(a).await.expect("acquire a");
        assert_eq!(sem_a.available_permits(), 0);
        assert_eq!(sem_b.available_permits(), 1);
    }

    #[tokio::test]
    async fn same_class_shares_one_semaphore() {
        let class = ResourceClass::Gpu { device: 103 };
        let first = ResourceRegistry::semaphore(class);
        let second = ResourceRegistry::semaphore(class);
        assert!(std::sync::Arc::ptr_eq(&first, &second));
    }

    #[tokio::test]
    async fn configure_all_overrides_default_before_first_use() {
        // A unique `Gpu` device index, not `Cpu`, so this test's override
        // cannot collide with `ResourceClass::Cpu`'s own dedicated test
        // above or with any other test's expectations about the shared,
        // process-wide `Cpu` class.
        let class = ResourceClass::Gpu { device: 104 };
        ResourceRegistry::configure_all(&ResourceBudgets::new().with_permits(class, 3));
        let sem = ResourceRegistry::semaphore(class);
        assert_eq!(sem.available_permits(), 3);
    }

    #[tokio::test]
    async fn configure_all_after_first_use_is_ignored() {
        let class = ResourceClass::Gpu { device: 105 };
        // Establish the semaphore at the default (1 permit) first.
        let sem = ResourceRegistry::semaphore(class);
        assert_eq!(sem.available_permits(), 1);
        ResourceRegistry::configure_all(&ResourceBudgets::new().with_permits(class, 9));
        // Re-fetching returns the same, unresized semaphore.
        let sem_again = ResourceRegistry::semaphore(class);
        assert_eq!(sem_again.available_permits(), 1);
    }
}
