//! Motion layer state — wraps the [`ene_vrm::layer_composer::LayerComposer`]
//! so ECS systems can route motion cues into it and the render path
//! can tick/compose per frame (#133).

use bevy_ecs::prelude::*;
use ene_config::MotionLayer;
use ene_vrm::RepeatMode;
use ene_vrm::layer_composer::{ComposedFrame, LayerComposer};

#[derive(Resource, Debug, Default)]
pub struct MotionLayerState(LayerComposer);

impl MotionLayerState {
    pub fn tick(&mut self, dt: f32) {
        self.0.tick(dt);
    }

    pub fn compose(&self) -> ComposedFrame {
        self.0.compose()
    }

    pub fn accept_motion(
        &mut self,
        name: String,
        layer: MotionLayer,
        priority: u8,
        duration: f32,
        repeat: RepeatMode,
    ) {
        self.0
            .accept_motion(name, layer, priority, duration, repeat);
    }

    pub fn cancel_motion(&mut self, layer: MotionLayer) {
        self.0.cancel_motion(layer);
    }

    pub fn cancel_all_motions(&mut self) {
        self.0.cancel_all_motions();
    }
}
