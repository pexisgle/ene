use crate::config::{AutonomySettings, FallbackSettings};
use crate::error::BodyError;
use crate::map::BodyCatalog;
use crate::queue::{EmotionCue, LookTarget, PerformanceCommand, Posture, Vitality};
use ene_session::{BodyId, SoulId};
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

const MAX_QUEUED: usize = 64;

#[derive(Debug, Clone)]
pub struct IssuedCommand {
    pub soul: SoulId,
    pub body: Option<BodyId>,
    pub command: PerformanceCommand,
    pub warning: Option<String>,
}

struct Slot {
    body: Option<BodyId>,
    catalog: BodyCatalog,
    queue: VecDeque<PerformanceCommand>,
    vitality: Vitality,
    generation: u64,
}

/// Per-soul performance bus. Text-only (`body = None`) still receives cues (D-19).
pub struct PerformanceBus {
    slots: Mutex<HashMap<SoulId, Slot>>,
    fallback: FallbackSettings,
    autonomy: AutonomySettings,
}

impl PerformanceBus {
    #[must_use]
    pub fn new(fallback: FallbackSettings, autonomy: AutonomySettings) -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
            fallback,
            autonomy,
        }
    }

    pub fn attach(
        &self,
        soul: SoulId,
        body: Option<BodyId>,
        catalog: BodyCatalog,
    ) -> Result<(), BodyError> {
        let mut slots = self.slots.lock();
        slots.insert(
            soul,
            Slot {
                body,
                catalog,
                queue: VecDeque::new(),
                vitality: Vitality::Neutral,
                generation: 0,
            },
        );
        Ok(())
    }

    /// Session-live body swap: drop pending cues, keep the soul (P-406).
    pub fn hot_swap(
        &self,
        soul: SoulId,
        body: Option<BodyId>,
        catalog: BodyCatalog,
    ) -> Result<u64, BodyError> {
        let mut slots = self.slots.lock();
        let slot = slots
            .get_mut(&soul)
            .ok_or_else(|| BodyError::UnknownBody(soul.to_string()))?;
        slot.body = body;
        slot.catalog = catalog;
        slot.queue.clear();
        slot.generation = slot.generation.saturating_add(1);
        Ok(slot.generation)
    }

    pub fn apply_emotion(
        &self,
        soul: SoulId,
        cue: &EmotionCue,
    ) -> Result<IssuedCommand, BodyError> {
        let mut slots = self.slots.lock();
        let slot = slots
            .get_mut(&soul)
            .ok_or_else(|| BodyError::UnknownBody(soul.to_string()))?;
        let (command, warning) = slot.catalog.map_emotion(cue, &self.fallback)?;
        push_cmd(&mut slot.queue, command.clone());
        Ok(IssuedCommand {
            soul,
            body: slot.body,
            command,
            warning,
        })
    }

    pub fn push(
        &self,
        soul: SoulId,
        command: PerformanceCommand,
    ) -> Result<IssuedCommand, BodyError> {
        let mut slots = self.slots.lock();
        let slot = slots
            .get_mut(&soul)
            .ok_or_else(|| BodyError::UnknownBody(soul.to_string()))?;
        if let PerformanceCommand::Motion { name, .. } = &command {
            slot.catalog.validate_motion(name)?;
        }
        push_cmd(&mut slot.queue, command.clone());
        Ok(IssuedCommand {
            soul,
            body: slot.body,
            command,
            warning: None,
        })
    }

    pub fn set_vitality(&self, soul: SoulId, vitality: Vitality) -> Result<(), BodyError> {
        let mut slots = self.slots.lock();
        let slot = slots
            .get_mut(&soul)
            .ok_or_else(|| BodyError::UnknownBody(soul.to_string()))?;
        slot.vitality = vitality;
        Ok(())
    }

    /// Idle life signs. Does not start a turn (P-106).
    pub fn autonomy_tick(&self, soul: SoulId) -> Result<Vec<PerformanceCommand>, BodyError> {
        if !self.autonomy.enabled {
            return Ok(Vec::new());
        }
        let mut slots = self.slots.lock();
        let slot = slots
            .get_mut(&soul)
            .ok_or_else(|| BodyError::UnknownBody(soul.to_string()))?;
        let look_weight = match slot.vitality {
            Vitality::Exhausted => 0.15,
            Vitality::Tired => 0.3,
            Vitality::Neutral => 0.5,
            Vitality::Lively => 0.7,
            Vitality::Wired => 0.9,
        };
        let pose = match slot.vitality {
            Vitality::Exhausted | Vitality::Tired => Posture::Relax,
            Vitality::Neutral | Vitality::Lively => Posture::Alert,
            Vitality::Wired => Posture::Thinking,
        };
        let cmds = vec![
            PerformanceCommand::LookAt {
                target: LookTarget::User,
                weight: look_weight,
            },
            PerformanceCommand::Posture {
                pose,
                blend: look_weight,
            },
        ];
        for cmd in &cmds {
            push_cmd(&mut slot.queue, cmd.clone());
        }
        Ok(cmds)
    }

    pub fn drain(&self, soul: SoulId) -> Result<Vec<PerformanceCommand>, BodyError> {
        let mut slots = self.slots.lock();
        let slot = slots
            .get_mut(&soul)
            .ok_or_else(|| BodyError::UnknownBody(soul.to_string()))?;
        Ok(slot.queue.drain(..).collect())
    }

    #[must_use]
    pub fn body_of(&self, soul: SoulId) -> Option<BodyId> {
        self.slots.lock().get(&soul).and_then(|s| s.body)
    }

    #[must_use]
    pub fn has_subscriber(&self, soul: SoulId) -> bool {
        self.slots
            .lock()
            .get(&soul)
            .is_some_and(|s| s.body.is_some())
    }
}

impl Default for PerformanceBus {
    fn default() -> Self {
        Self::new(FallbackSettings::default(), AutonomySettings::default())
    }
}

fn push_cmd(queue: &mut VecDeque<PerformanceCommand>, command: PerformanceCommand) {
    if queue.len() >= MAX_QUEUED {
        queue.pop_front();
    }
    queue.push_back(command);
}

/// Shared handle for the daemon and tests.
pub type SharedBus = Arc<PerformanceBus>;
