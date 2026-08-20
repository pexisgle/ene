use crate::bus::PerformanceBus;
use crate::config::BodySettings;
use crate::error::BodyError;
use crate::map::BodyCatalog;
use crate::queue::EmotionCue;
use crate::voice::VoiceRuntime;
use ene_session::{BodyId, SoulId};
use parking_lot::Mutex;
use std::sync::Arc;

/// Stage: several (soul, body) pairs, one speaker.
pub struct Stage {
    bus: Arc<PerformanceBus>,
    voice: Mutex<VoiceRuntime>,
    settings: BodySettings,
    occupants: Mutex<Vec<(SoulId, Option<BodyId>)>>,
}

impl Stage {
    #[must_use]
    pub fn new(bus: Arc<PerformanceBus>, voice: VoiceRuntime, settings: BodySettings) -> Self {
        Self {
            bus,
            voice: Mutex::new(voice),
            settings,
            occupants: Mutex::new(Vec::new()),
        }
    }

    pub fn present(
        &self,
        soul: SoulId,
        body: Option<BodyId>,
        catalog: BodyCatalog,
    ) -> Result<(), BodyError> {
        let mut occupants = self.occupants.lock();
        if body.is_some() {
            let rendered = occupants.iter().filter(|(_, b)| b.is_some()).count();
            let already = occupants.iter().any(|(s, _)| *s == soul);
            if u32::try_from(rendered).unwrap_or(u32::MAX) >= self.settings.render.max_concurrent
                && !already
            {
                occupants.retain(|(s, _)| *s != soul);
                occupants.push((soul, None));
                return self.bus.attach(soul, None, catalog);
            }
        }
        occupants.retain(|(s, _)| *s != soul);
        occupants.push((soul, body));
        self.bus.attach(soul, body, catalog)
    }

    pub fn apply_emotion(&self, soul: SoulId, cue: &EmotionCue) -> Result<(), BodyError> {
        self.bus.apply_emotion(soul, cue).map(|_| ())
    }

    pub fn voice(&self) -> &Mutex<VoiceRuntime> {
        &self.voice
    }

    #[must_use]
    pub fn bus(&self) -> Arc<PerformanceBus> {
        Arc::clone(&self.bus)
    }

    #[must_use]
    pub fn occupant_count(&self) -> usize {
        self.occupants.lock().len()
    }

    #[must_use]
    pub fn occupants(&self) -> Vec<(SoulId, Option<BodyId>)> {
        self.occupants.lock().clone()
    }

    #[must_use]
    pub fn render_enabled(&self) -> bool {
        self.settings.render.enabled
    }
}
