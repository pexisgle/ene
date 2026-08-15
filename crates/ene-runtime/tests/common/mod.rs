#![expect(
    dead_code,
    clippy::expect_used,
    clippy::field_reassign_with_default,
    reason = "shared harness: each test target uses only a subset of it"
)]

use chrono::{DateTime, Utc};
use ene_card::CharacterCardV3;
use ene_config::EneConfig;
use std::sync::Arc;
use std::time::Duration;

pub fn test_card(name: &str) -> CharacterCardV3 {
    let mut card = CharacterCardV3::default();
    card.data.name = name.into();
    card.data.system_prompt = "Be brief.".into();
    card
}

/// Config with the memory store and plugin host disabled, so tests exercise
/// the actor without `SQLite` or plugin processes.
pub fn test_config_memory_off() -> EneConfig {
    let mut config = EneConfig::default();
    let mut store = ene_store::StoreConfig::default();
    store.enabled = false;
    config.set_section(&store).expect("store config merges");
    let mut tools = ene_plugin_host::PluginConfig::default();
    tools.enabled = false;
    drop(config.set_section(&tools));
    let ai = ene_ai::AiConfig::default();
    drop(config.set_section(&ai));
    config
}

/// Virtual wall clock shared with the injected scheduler clock.
///
/// The scheduler task derives its sleep from the injected clock, so
/// [`Self::advance`] moves the scheduler's wall clock without touching the
/// tokio time driver (which sqlx pool timeouts depend on). The timer task
/// is woken explicitly via `EneHandle::wake_scheduler` after each advance,
/// so due fires never depend on wall-clock sleep durations.
///
/// The elapsed offset lives in a shared `Mutex` rather than
/// `tokio::time::Instant` because the actor runs on a different worker
/// than the test and tokio's test-util clock is not epoch-comparable
/// across workers of a multi-thread runtime.
#[derive(Clone)]
pub struct VirtualClock {
    anchor_wall: DateTime<Utc>,
    elapsed: Arc<parking_lot::Mutex<Duration>>,
}

impl VirtualClock {
    /// Anchors the virtual clock at the current wall time with zero
    /// elapsed virtual time.
    pub fn new() -> Self {
        Self {
            anchor_wall: Utc::now(),
            elapsed: Arc::new(parking_lot::Mutex::new(Duration::ZERO)),
        }
    }

    /// The clock's current wall time: the anchor plus virtual elapsed.
    pub fn now(&self) -> DateTime<Utc> {
        let elapsed = *self.elapsed.lock();
        self.anchor_wall + chrono::Duration::from_std(elapsed).unwrap_or_default()
    }

    /// Advances virtual time by `step`.
    ///
    /// Call [`EneHandle::wake_scheduler`] afterwards so the scheduler timer
    /// task re-derives due schedules against the new clock value.
    pub fn advance(&self, step: Duration) {
        *self.elapsed.lock() += step;
    }
}

impl Default for VirtualClock {
    fn default() -> Self {
        Self::new()
    }
}
