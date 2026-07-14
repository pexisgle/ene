//! Motion layer state — wraps the [`ene_vrm::layer_composer::LayerComposer`]
//! so ECS systems can route motion cues into it and the render path
//! can tick/compose per frame (#133).

use bevy_ecs::prelude::*;
use ene_vrm::layer_composer::LayerComposer;

#[derive(Resource, Debug, Default)]
pub struct MotionLayerState(pub LayerComposer);
