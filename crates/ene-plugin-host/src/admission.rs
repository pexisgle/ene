//! Host-side admission control keyed by [`ResourceClass`].
//!
//! Each plugin declares the physical resource its provider jobs contend on
//! (`LlmProviderSpec.resource_class`); [`ResourceClassAdmission`] maps that
//! declaration to one shared limiter per class, so providers from *different*
//! plugin processes that declare the same class (e.g. two local LLM plugins
//! offloading to GPU device 0) share one budget instead of each running
//! unbounded against the same device. The permit is held by the host for the
//! duration of a request — never by the plugin — so a crashed or restarted
//! plugin releases it through the same drop glue as every other
//! `OwnedSemaphorePermit` on the IPC path.
//!
//! GPU classes are gated by default (one concurrent job per device). `Cpu`
//! and `Network` classes are only gated when explicitly budgeted in
//! `plugins.resource_classes`, so cloud providers keep their declared
//! per-plugin concurrency untouched on machines where the class defaults
//! would be lower than what the provider declared.

use std::collections::HashMap;
use std::sync::Arc;

use ene_plugin_proto::{ConcurrencyHint, ResourceClass};
use parking_lot::Mutex;

use crate::config::ResourceClassBudget;
use crate::ipc_provider::ConcurrencyLimiter;

/// Default number of callers that may wait for a class permit before further
/// requests fail fast with `Busy`.
const DEFAULT_CLASS_QUEUE_DEPTH: usize = 8;

/// Default permits for a class with no configured budget, mirroring
/// `ene_ai::engine_adapter::resource::default_permits`.
fn default_permits(class: ResourceClass) -> usize {
    match class {
        ResourceClass::Gpu { .. } => 1,
        ResourceClass::Cpu => {
            std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
        }
        ResourceClass::Network => 4,
    }
}

fn gated_by_default(class: ResourceClass) -> bool {
    matches!(class, ResourceClass::Gpu { .. })
}

/// Process-wide admission registry: one shared [`ConcurrencyLimiter`] per
/// [`ResourceClass`], built from the `plugins.resource_classes` config.
pub struct ResourceClassAdmission {
    limiters: Mutex<HashMap<ResourceClass, Arc<ConcurrencyLimiter>>>,
    budgets: HashMap<ResourceClass, (usize, usize)>,
}

impl ResourceClassAdmission {
    /// Builds the registry from the configured budget entries. Classes not
    /// named by any entry fall back to [`gated_by_default`] plus
    /// [`default_permits`].
    #[must_use]
    pub fn new(budgets: &[ResourceClassBudget]) -> Self {
        let budgets = budgets
            .iter()
            .map(|b| {
                let permits = b.permits.unwrap_or_else(|| default_permits(b.class));
                let queue_depth = b.queue_depth.unwrap_or(DEFAULT_CLASS_QUEUE_DEPTH);
                // A zero-permit class would make every request wait forever
                // (or fail Busy outright with queue_depth 0) — clamping keeps
                // the failure mode at "serialized", matching
                // `ConcurrencyLimiter`'s max_in_flight clamp.
                (b.class, (permits.max(1), queue_depth))
            })
            .collect();
        Self {
            limiters: Mutex::new(HashMap::new()),
            budgets,
        }
    }

    /// The shared limiter for `class`, or `None` when the class is not gated.
    pub(crate) fn limiter(&self, class: ResourceClass) -> Option<Arc<ConcurrencyLimiter>> {
        let (permits, queue_depth) = if let Some(&budget) = self.budgets.get(&class) {
            budget
        } else if gated_by_default(class) {
            (default_permits(class), DEFAULT_CLASS_QUEUE_DEPTH)
        } else {
            return None;
        };
        let mut limiters = self.limiters.lock();
        if let Some(limiter) = limiters.get(&class) {
            return Some(Arc::clone(limiter));
        }
        let limiter = Arc::new(ConcurrencyLimiter::new(ConcurrencyHint {
            max_in_flight: u32::try_from(permits).unwrap_or(u32::MAX),
            queue_depth: u32::try_from(queue_depth).unwrap_or(u32::MAX),
        }));
        limiters.insert(class, Arc::clone(&limiter));
        Some(limiter)
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "unit tests use unwrap/expect/panic for concise assertions"
)]
mod tests {
    use super::{ResourceClassAdmission, ResourceClassBudget, default_permits, gated_by_default};
    use ene_plugin_proto::ResourceClass;
    use std::sync::Arc;

    #[test]
    fn gpu_classes_are_gated_by_default_and_cpu_network_are_not() {
        assert!(gated_by_default(ResourceClass::Gpu { device: 0 }));
        assert!(gated_by_default(ResourceClass::Gpu { device: 7 }));
        assert!(!gated_by_default(ResourceClass::Cpu));
        assert!(!gated_by_default(ResourceClass::Network));
    }

    #[test]
    fn default_permits_match_the_documented_values() {
        assert_eq!(default_permits(ResourceClass::Gpu { device: 0 }), 1);
        let cpu = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        assert_eq!(default_permits(ResourceClass::Cpu), cpu);
        assert_eq!(default_permits(ResourceClass::Network), 4);
    }

    #[tokio::test]
    async fn unconfigured_cpu_class_has_no_limiter() {
        let admission = ResourceClassAdmission::new(&[]);
        assert!(admission.limiter(ResourceClass::Cpu).is_none());
        assert!(admission.limiter(ResourceClass::Network).is_none());
    }

    #[tokio::test]
    async fn same_class_shares_one_limiter_across_lookups() {
        let admission = ResourceClassAdmission::new(&[]);
        let class = ResourceClass::Gpu { device: 301 };
        let a = admission.limiter(class).expect("gpu class is gated");
        let b = admission.limiter(class).expect("gpu class is gated");
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[tokio::test]
    async fn default_gpu_budget_holds_one_job_and_queues_the_next() {
        let admission = ResourceClassAdmission::new(&[]);
        let class = ResourceClass::Gpu { device: 302 };
        let limiter = admission.limiter(class).expect("gpu class is gated");
        let permit = limiter.acquire("test").await.expect("first permit");

        // The default queue depth admits waiters rather than failing fast.
        let waiter = tokio::spawn({
            let limiter = Arc::clone(&limiter);
            async move { limiter.acquire("test").await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(
            !waiter.is_finished(),
            "second job must wait on the single GPU permit"
        );

        drop(permit);
        assert!(
            waiter.await.unwrap().is_ok(),
            "queued job must run once the permit is released"
        );
    }

    #[tokio::test]
    async fn configured_budget_raises_permits() {
        let admission = ResourceClassAdmission::new(&[ResourceClassBudget {
            class: ResourceClass::Gpu { device: 303 },
            permits: Some(2),
            queue_depth: Some(0),
        }]);
        let limiter = admission
            .limiter(ResourceClass::Gpu { device: 303 })
            .expect("configured class is gated");
        let _a = limiter.acquire("test").await.expect("permit one");
        let _b = limiter.acquire("test").await.expect("permit two");
        let err = limiter
            .acquire("test")
            .await
            .expect_err("third job must be rejected: two permits, no queue");
        assert!(matches!(err, ene_ai::error::LlmProviderError::Busy { .. }));
    }

    #[tokio::test]
    async fn zero_permit_budget_is_clamped_to_one() {
        let admission = ResourceClassAdmission::new(&[ResourceClassBudget {
            class: ResourceClass::Gpu { device: 306 },
            permits: Some(0),
            queue_depth: Some(0),
        }]);
        let limiter = admission
            .limiter(ResourceClass::Gpu { device: 306 })
            .expect("configured class is gated");
        let _permit = limiter
            .acquire("test")
            .await
            .expect("clamped permit admits one");
        assert!(limiter.acquire("test").await.is_err());
    }

    #[tokio::test]
    async fn distinct_devices_are_independent() {
        let admission = ResourceClassAdmission::new(&[]);
        let a = admission
            .limiter(ResourceClass::Gpu { device: 304 })
            .expect("gated");
        let b = admission
            .limiter(ResourceClass::Gpu { device: 305 })
            .expect("gated");
        let _permit_a = a.acquire("test").await.expect("device a permit");
        assert!(b.acquire("test").await.is_ok());
    }
}
