//! Declared engine capability, concurrency, and resource metadata.
//!
//! Capability is declared up front by each engine rather than discovered by
//! trial and error at runtime (a runtime error when `tools` is non-empty, or
//! inherent methods that force callers to know the concrete provider type).
//! An [`EngineDescriptor`] lets the blanket adapters in this module act on
//! capability generically.

use std::collections::HashMap;

// Canonical in `ene-plugin-proto`; re-exported so `descriptor::ResourceClass`
// and the `ene_ai::*` paths stay stable while the definition lives on the wire.
pub use ene_plugin_proto::ResourceClass;

/// Stable identifier for one loaded engine instance (e.g. `"llama-cpp-chat"`,
/// `"whisper-base"`). Used in tracing spans and error messages; not
/// interpreted by this crate beyond that.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EngineId(String);

impl EngineId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for EngineId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for EngineId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for EngineId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

/// Kept as a fieldless enum (rather than separate `bool` flags on
/// [`EngineDescriptor`]) so [`CapabilitySet`] can store and query them
/// uniformly, and so adding a new capability later is one new variant, not a
/// new field threaded through every constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Capability {
    Chat,
    Vision,
    Embed,
    Tts,
    Stt,
    /// Grammar / JSON-schema constrained output.
    Grammar,
    Tools,
    Streaming,
}

const CAPABILITY_COUNT: u32 = 8;

/// A hand-rolled `u16` bitset rather than a `bitflags` dependency: the
/// workspace does not otherwise depend on `bitflags`, and eight fixed,
/// crate-owned variants do not need an extensibility mechanism a macro would
/// provide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CapabilitySet(u16);

impl CapabilitySet {
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn with(self, cap: Capability) -> Self {
        Self(self.0 | (1 << cap as u16))
    }

    #[must_use]
    pub const fn contains(self, cap: Capability) -> bool {
        self.0 & (1 << cap as u16) != 0
    }

    #[must_use]
    pub fn from_capabilities(caps: impl IntoIterator<Item = Capability>) -> Self {
        caps.into_iter().fold(Self::empty(), Self::with)
    }
}

// `CAPABILITY_COUNT` documents the bitset's occupancy bound; referenced here
// so it participates in dead-code analysis without needing `#[allow]`.
const _: () = assert!(
    CAPABILITY_COUNT <= 16,
    "CapabilitySet's u16 backing store overflows"
);

/// How eagerly an engine's worker should be sized.
///
/// Advisory metadata, not itself enforced by this crate: a single
/// [`ene_infer::EngineHandle`] is architecturally single-flight (exactly one
/// dedicated worker thread), so `max_in_flight` above 1 is a signal for a
/// *future* orchestration layer that would spawn multiple worker handles for
/// the same engine and route between them — no such layer exists yet.
/// `queue_depth` maps directly onto [`ene_infer::EngineConfig::queue_depth`]
/// when a caller constructs the underlying handle.
#[derive(Debug, Clone, Copy)]
pub struct ConcurrencyHint {
    /// Not enforced by a single [`ene_infer::EngineHandle`] today.
    pub max_in_flight: usize,
    pub queue_depth: usize,
}

impl Default for ConcurrencyHint {
    /// `max_in_flight: 1, queue_depth: 2` — the conservative default for a
    /// local model whose author has not thought about concurrency at all.
    fn default() -> Self {
        Self {
            max_in_flight: 1,
            queue_depth: 2,
        }
    }
}

impl From<ene_plugin_proto::ConcurrencyHint> for ConcurrencyHint {
    fn from(value: ene_plugin_proto::ConcurrencyHint) -> Self {
        Self {
            max_in_flight: value.max_in_flight as usize,
            queue_depth: value.queue_depth as usize,
        }
    }
}

impl TryFrom<ConcurrencyHint> for ene_plugin_proto::ConcurrencyHint {
    type Error = std::num::TryFromIntError;

    fn try_from(value: ConcurrencyHint) -> Result<Self, Self::Error> {
        Ok(Self {
            max_in_flight: value.max_in_flight.try_into()?,
            queue_depth: value.queue_depth.try_into()?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct EngineDescriptor {
    pub id: EngineId,
    pub capabilities: CapabilitySet,
    pub concurrency: ConcurrencyHint,
    /// The physical resource this engine's jobs contend on — the key
    /// [`crate::engine_adapter::resource::ResourceRegistry`] uses to share an
    /// admission budget across engines that contend on the same one.
    pub resource: ResourceClass,
}

impl EngineDescriptor {
    #[must_use]
    pub fn new(
        id: impl Into<EngineId>,
        capabilities: CapabilitySet,
        resource: ResourceClass,
    ) -> Self {
        Self {
            id: id.into(),
            capabilities,
            concurrency: ConcurrencyHint::default(),
            resource,
        }
    }

    #[must_use]
    pub fn with_concurrency(mut self, concurrency: ConcurrencyHint) -> Self {
        self.concurrency = concurrency;
        self
    }
}

/// A thin, explicit alternative to hidden global mutable state: callers that
/// want non-default budgets build one of these and hand it to
/// [`crate::engine_adapter::resource::ResourceRegistry::configure_all`] once,
/// rather than reaching for ambient configuration.
#[derive(Debug, Clone, Default)]
pub struct ResourceBudgets(pub(crate) HashMap<ResourceClass, usize>);

impl ResourceBudgets {
    #[must_use]
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    #[must_use]
    pub fn with_permits(mut self, class: ResourceClass, permits: usize) -> Self {
        self.0.insert(class, permits.max(1));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Capability, CapabilitySet, ConcurrencyHint, EngineDescriptor, EngineId, ResourceClass,
    };

    #[test]
    fn capability_set_contains_only_added() {
        let set = CapabilitySet::empty()
            .with(Capability::Chat)
            .with(Capability::Tools);
        assert!(set.contains(Capability::Chat));
        assert!(set.contains(Capability::Tools));
        assert!(!set.contains(Capability::Vision));
        assert!(!set.contains(Capability::Streaming));
    }

    #[test]
    fn capability_set_from_iter() {
        let set = CapabilitySet::from_capabilities([Capability::Stt, Capability::Streaming]);
        assert!(set.contains(Capability::Stt));
        assert!(set.contains(Capability::Streaming));
        assert!(!set.contains(Capability::Tts));
    }

    #[test]
    fn concurrency_hint_default_is_conservative() {
        let hint = ConcurrencyHint::default();
        assert_eq!(hint.max_in_flight, 1);
        assert_eq!(hint.queue_depth, 2);
    }

    #[test]
    fn descriptor_new_uses_default_concurrency() {
        let descriptor = EngineDescriptor::new(
            EngineId::new("test-engine"),
            CapabilitySet::empty().with(Capability::Chat),
            ResourceClass::Gpu { device: 0 },
        );
        assert_eq!(descriptor.id.as_str(), "test-engine");
        assert_eq!(descriptor.concurrency.max_in_flight, 1);
    }

    #[test]
    fn wire_concurrency_conversion_rejects_unrepresentable_values() {
        let hint = ConcurrencyHint {
            max_in_flight: usize::MAX,
            queue_depth: 2,
        };

        if usize::MAX > u32::MAX as usize {
            assert!(ene_plugin_proto::ConcurrencyHint::try_from(hint).is_err());
        }
    }
}
