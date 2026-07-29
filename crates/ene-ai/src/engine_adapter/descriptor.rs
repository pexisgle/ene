//! Declared engine capability, concurrency, and resource metadata.
//!
//! Before this module, capability was discovered by trial and error:
//! `LocalLlamaCppProvider::create_chat_stream` returned a runtime error if
//! `tools` was non-empty, and `supports_vision()` was an inherent method not
//! on any trait, so callers had to know the concrete provider type. An
//! [`EngineDescriptor`] replaces that with an upfront declaration the blanket
//! adapters in this module can act on generically.

use std::collections::HashMap;

/// Stable identifier for one loaded engine instance (e.g. `"llama-cpp-chat"`,
/// `"whisper-base"`). Used in tracing spans and error messages; not
/// interpreted by this crate beyond that.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EngineId(String);

impl EngineId {
    /// Builds an id from any string-like value.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The id as a string slice.
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

/// One capability an engine may declare support for.
///
/// Kept as a fieldless enum (rather than separate `bool` flags on
/// [`EngineDescriptor`]) so [`CapabilitySet`] can store and query them
/// uniformly, and so adding a new capability later is one new variant, not a
/// new field threaded through every constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Capability {
    /// Text chat completion.
    Chat,
    /// Accepts image input in chat messages.
    Vision,
    /// Produces vector embeddings.
    Embed,
    /// Text-to-speech synthesis.
    Tts,
    /// Speech-to-text transcription.
    Stt,
    /// Grammar / JSON-schema constrained output.
    Grammar,
    /// Accepts tool/function-calling definitions.
    Tools,
    /// Delivers output incrementally rather than only as one final result.
    Streaming,
}

const CAPABILITY_COUNT: u32 = 8;

/// A small bitset of [`Capability`] values.
///
/// A hand-rolled `u16` bitset rather than a `bitflags` dependency: the
/// workspace does not otherwise depend on `bitflags`, and eight fixed,
/// crate-owned variants do not need an extensibility mechanism a macro would
/// provide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CapabilitySet(u16);

impl CapabilitySet {
    /// The empty set.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Returns a copy of this set with `cap` added.
    #[must_use]
    pub const fn with(self, cap: Capability) -> Self {
        Self(self.0 | (1 << cap as u16))
    }

    /// Whether `cap` is in this set.
    #[must_use]
    pub const fn contains(self, cap: Capability) -> bool {
        self.0 & (1 << cap as u16) != 0
    }

    /// Builds a set from an iterator of capabilities.
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
    /// Advisory: how many jobs for this engine should be allowed to run at
    /// once. Not enforced by a single [`ene_infer::EngineHandle`] today.
    pub max_in_flight: usize,
    /// Suggested [`ene_infer::EngineConfig::queue_depth`] for this engine.
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

/// The physical resource an engine contends on, and the key
/// [`crate::engine_adapter::resource::ResourceRegistry`] uses to share an
/// admission budget across engines that contend on the same one.
///
/// Two engines that declare `==` `ResourceClass` values share one semaphore:
/// adding a new local model that offloads to the same GPU device (for
/// example) automatically starts sharing that device's budget the moment it
/// declares `ResourceClass::Gpu { device: 0 }` — no change to callers or to
/// engines already using that class.
///
/// `Cpu` deliberately carries no field. An earlier revision used
/// `Cpu { threads: u32 }` and relied on two engines picking different
/// numbers to avoid sharing a semaphore — that made a capacity question
/// (how many CPU-bound jobs may run at once) masquerade as an identity
/// question (are these the same resource), which does not scale: every new
/// CPU engine had to pick an unused magic number, and retuning an existing
/// engine's thread count would silently change what it shares a budget
/// with. All CPU-bound local engines now declare the same `Cpu` value and
/// share one process-wide budget; how large that budget is (whether two
/// independent CPU engines may run concurrently at all) is controlled by
/// [`crate::engine_adapter::resource::default_permits`] /
/// [`crate::engine_adapter::resource::ResourceRegistry::configure_all`], not
/// by this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceClass {
    /// A specific GPU device index, as used by `with_main_gpu(n)` /
    /// CUDA/Vulkan device selection. A real, meaningful identity: two
    /// engines that declare the same device index genuinely do contend on
    /// that one physical device.
    Gpu {
        /// Device index.
        device: u32,
    },
    /// CPU-bound inference. Shared by every CPU-bound local engine — see the
    /// type-level docs for why this carries no field.
    Cpu,
    /// A network-attached engine (e.g. a local sidecar process reached over
    /// HTTP/gRPC) that does not contend on host GPU/CPU capacity the same
    /// way.
    Network,
}

/// Declared capability, concurrency, and resource metadata for one engine.
#[derive(Debug, Clone)]
pub struct EngineDescriptor {
    /// Stable identifier for this engine instance.
    pub id: EngineId,
    /// What this engine can do.
    pub capabilities: CapabilitySet,
    /// Advisory concurrency sizing.
    pub concurrency: ConcurrencyHint,
    /// The physical resource this engine's jobs contend on.
    pub resource: ResourceClass,
}

impl EngineDescriptor {
    /// Builds a descriptor with [`ConcurrencyHint::default`] (the
    /// conservative local-model default) and the given id, capabilities, and
    /// resource class.
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

    /// Sets an explicit [`ConcurrencyHint`], overriding the default.
    #[must_use]
    pub fn with_concurrency(mut self, concurrency: ConcurrencyHint) -> Self {
        self.concurrency = concurrency;
        self
    }
}

/// Per-[`ResourceClass`] semaphore permit overrides, applied at process
/// startup before any engine is spawned.
///
/// A thin, explicit alternative to hidden global mutable state: callers that
/// want non-default budgets build one of these and hand it to
/// [`crate::engine_adapter::resource::ResourceRegistry::configure_all`] once,
/// rather than reaching for ambient configuration.
#[derive(Debug, Clone, Default)]
pub struct ResourceBudgets(pub(crate) HashMap<ResourceClass, usize>);

impl ResourceBudgets {
    /// An empty override set — every class uses
    /// [`crate::engine_adapter::resource::default_permits`].
    #[must_use]
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    /// Overrides the permit count for `class`.
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
    fn resource_class_equality_is_by_value() {
        assert_eq!(
            ResourceClass::Gpu { device: 0 },
            ResourceClass::Gpu { device: 0 }
        );
        assert_ne!(
            ResourceClass::Gpu { device: 0 },
            ResourceClass::Gpu { device: 1 }
        );
        // `Cpu` carries no field: every CPU-bound engine shares this one
        // value on purpose (see the type docs) rather than picking a
        // distinguishing number, so there is nothing to assert `!=` here —
        // this instead pins that two `Cpu` values are always equal.
        assert_eq!(ResourceClass::Cpu, ResourceClass::Cpu);
        assert_ne!(ResourceClass::Cpu, ResourceClass::Gpu { device: 0 });
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
}
