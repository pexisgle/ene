//! The [`AiBridge`] handle is held in a bevy `Resource` so
//! per-action UI systems can call `ai.run(input)` via
//! `Res<AiBridgeResource>`.
//!
//! `ProcessingFlag` is a clone of the `Arc<AtomicBool>` the
//! bridge uses to disable the AI page's chat input while a
//! request is in flight. The egui page reads it via
//! `world.resource::<ProcessingFlag>()` instead of going
//! through `Arc<AiBridge>`.
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use bevy_ecs::prelude::Resource;

use crate::ai_bridge::AiBridge;

#[derive(Resource, Clone)]
#[expect(dead_code, reason = "yet to be populated by Runtime::resumed")]
pub struct AiBridgeResource(pub Arc<AiBridge>);

#[derive(Resource, Debug, Default)]
#[expect(dead_code, reason = "yet to be populated by Runtime::resumed")]
pub struct ProcessingFlag(pub Arc<AtomicBool>);
