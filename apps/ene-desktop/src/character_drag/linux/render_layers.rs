use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;

use super::super::WindowBackendState;
use crate::platform::LinuxWindowBackend;
use crate::platform::character_render_layers;

pub(super) fn sync_character_render_layers(
    mut commands: Commands,
    roots: Query<Entity, With<crate::character::CharacterRoot>>,
    children_query: Query<&Children>,
    layers_query: Query<&RenderLayers>,
    lights: Query<Entity, With<DirectionalLight>>,
    window_backend: Res<WindowBackendState>,
) {
    if window_backend.backend != LinuxWindowBackend::Wayland {
        return;
    }

    let desired_layers = character_render_layers();
    for root in roots.iter() {
        apply_render_layers_recursive(
            root,
            &desired_layers,
            &children_query,
            &layers_query,
            &mut commands,
        );
    }

    for light in lights.iter() {
        if layers_query
            .get(light)
            .map_or(true, |layers| *layers != desired_layers)
        {
            commands.entity(light).insert(desired_layers.clone());
        }
    }
}

fn apply_render_layers_recursive(
    entity: Entity,
    desired_layers: &RenderLayers,
    children_query: &Query<&Children>,
    layers_query: &Query<&RenderLayers>,
    commands: &mut Commands,
) {
    if layers_query.get(entity) != Ok(desired_layers) {
        commands.entity(entity).insert(desired_layers.clone());
    }

    if let Ok(children) = children_query.get(entity) {
        for child in children.iter() {
            apply_render_layers_recursive(
                child,
                desired_layers,
                children_query,
                layers_query,
                commands,
            );
        }
    }
}
