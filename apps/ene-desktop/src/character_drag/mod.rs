use bevy::{
    camera::primitives::{Aabb, MeshAabb},
    input::{ButtonState, mouse::MouseButtonInput},
    prelude::*,
    window::{CursorOptions, PrimaryWindow},
};

use crate::app_config::CharacterSettings;
use crate::platform::cursor_position_for_window;
#[cfg(not(target_os = "windows"))]
use crate::platform::{LinuxWindowBackend, detect_linux_window_backend};
use crate::scene::MainViewCamera;
#[cfg(target_os = "windows")]
use bevy::window::RawHandleWrapper;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;

pub struct CharacterDragPlugin;

impl Plugin for CharacterDragPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CharacterDragState>();
        #[cfg(not(target_os = "windows"))]
        app.init_resource::<WindowBackendState>();

        #[cfg(target_os = "windows")]
        windows::register(app);
        #[cfg(target_os = "linux")]
        linux::register(app);

        app.add_systems(Update, update_character_drag);
    }
}

#[derive(Resource, Default)]
struct CharacterDragState {
    last_cursor_world_pos: Option<Vec2>,
}

impl CharacterDragState {
    fn is_dragging(&self) -> bool {
        self.last_cursor_world_pos.is_some()
    }
}

#[cfg(not(target_os = "windows"))]
#[derive(Resource, Clone, Copy, Debug)]
pub(super) struct WindowBackendState {
    pub(super) backend: LinuxWindowBackend,
}

#[cfg(not(target_os = "windows"))]
impl FromWorld for WindowBackendState {
    fn from_world(_world: &mut World) -> Self {
        Self {
            backend: detect_linux_window_backend(),
        }
    }
}

/// Reads the cursor once, then uses a flat world point for dragging and a ray for hit testing.
fn update_character_drag(
    mut mouse_button_inputs: MessageReader<MouseButtonInput>,
    primary_window_entity: Single<Entity, With<PrimaryWindow>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut cursor_options: Single<&mut CursorOptions, With<PrimaryWindow>>,
    camera_query: Single<(&Camera, &GlobalTransform), With<MainViewCamera>>,
    character_meshes: Query<(&Mesh3d, &GlobalTransform)>,
    meshes: Res<Assets<Mesh>>,
    mut settings: ResMut<CharacterSettings>,
    mut drag_state: ResMut<CharacterDragState>,
    #[cfg(not(target_os = "windows"))] window_backend: Res<WindowBackendState>,
    #[cfg(target_os = "windows")] raw_handles: Query<&RawHandleWrapper, With<PrimaryWindow>>,
    #[cfg(target_os = "windows")] mut windows_hit_test_state: NonSendMut<
        windows::WindowsHitTestState,
    >,
) {
    let primary_window_entity = *primary_window_entity;
    let window = *window;
    let (camera, camera_transform) = *camera_query;

    #[cfg(target_os = "windows")]
    let cursor_position =
        cursor_position_for_window(window, raw_handles.get(primary_window_entity).ok());
    #[cfg(not(target_os = "windows"))]
    let cursor_position = cursor_position_for_window(window);

    let cursor_world_pos = cursor_world_position_2d(cursor_position, camera, camera_transform);
    let cursor_ray =
        cursor_position.and_then(|cursor| camera.viewport_to_world(camera_transform, cursor).ok());
    let cursor_over_character = cursor_ray.as_ref().is_some_and(|ray| {
        character_meshes.iter().any(|(mesh_handle, mesh_tf)| {
            let Some(mesh) = meshes.get(mesh_handle) else {
                return false;
            };
            let Some(aabb) = mesh.compute_aabb() else {
                return false;
            };
            let (world_min, world_max) = transformed_aabb_bounds(&aabb, mesh_tf);
            ray_intersects_aabb(ray.origin, *ray.direction, world_min, world_max)
        })
    });
    let allows_input = cursor_over_character || drag_state.is_dragging();

    #[cfg(target_os = "windows")]
    {
        let _ = windows::sync_windows_hit_test_hook(
            raw_handles.get(primary_window_entity).ok(),
            allows_input,
            &mut windows_hit_test_state,
        );
        cursor_options.hit_test = allows_input;
    }

    #[cfg(not(target_os = "windows"))]
    {
        cursor_options.hit_test = if window_backend.backend == LinuxWindowBackend::Wayland {
            true
        } else {
            allows_input
        };
    }

    update_drag_state(
        &mut mouse_button_inputs,
        primary_window_entity,
        cursor_world_pos,
        cursor_over_character,
        &mut settings,
        &mut drag_state,
    );
}

/// Projects the cursor into the camera plane for drag movement.
fn cursor_world_position_2d(
    cursor_position: Option<Vec2>,
    camera: &Camera,
    camera_transform: &GlobalTransform,
) -> Option<Vec2> {
    let cursor_position = cursor_position?;
    camera
        .viewport_to_world_2d(camera_transform, cursor_position)
        .ok()
}

/// Uses left-button transitions as the only drag state changes, then applies movement.
fn update_drag_state(
    mouse_button_inputs: &mut MessageReader<MouseButtonInput>,
    primary_window_entity: Entity,
    cursor_world_pos: Option<Vec2>,
    cursor_over_character: bool,
    settings: &mut CharacterSettings,
    drag_state: &mut CharacterDragState,
) {
    for input in mouse_button_inputs.read() {
        if input.window != primary_window_entity || input.button != MouseButton::Left {
            continue;
        }

        match input.state {
            ButtonState::Pressed if cursor_over_character => {
                drag_state.last_cursor_world_pos = cursor_world_pos;
            }
            ButtonState::Released => {
                drag_state.last_cursor_world_pos = None;
            }
            _ => {}
        }
    }

    let Some(last_cursor_world_pos) = drag_state.last_cursor_world_pos else {
        return;
    };
    let Some(cursor_world_pos) = cursor_world_pos else {
        drag_state.last_cursor_world_pos = None;
        return;
    };
    if cursor_world_pos == last_cursor_world_pos {
        return;
    }

    settings.character_state.character_position +=
        (cursor_world_pos - last_cursor_world_pos).extend(0.0);
    drag_state.last_cursor_world_pos = Some(cursor_world_pos);
}

fn aabb_world_corners(aabb: &Aabb, mesh_tf: &GlobalTransform) -> [Vec3; 8] {
    let min = aabb.min();
    let max = aabb.max();
    [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(min.x, max.y, max.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(max.x, max.y, max.z),
    ]
    .map(|corner| mesh_tf.transform_point(corner))
}

/// Expands a mesh-local AABB into world-space bounds after the mesh transform.
fn transformed_aabb_bounds(aabb: &Aabb, mesh_tf: &GlobalTransform) -> (Vec3, Vec3) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for world_corner in aabb_world_corners(aabb, mesh_tf) {
        min = min.min(world_corner);
        max = max.max(world_corner);
    }
    (min, max)
}

/// Runs a fast slab test against the transformed world-space AABB.
fn ray_intersects_aabb(origin: Vec3, direction: Vec3, min: Vec3, max: Vec3) -> bool {
    let eps = 1e-6;
    let mut t_min = f32::NEG_INFINITY;
    let mut t_max = f32::INFINITY;

    let mut check_axis = |origin_axis: f32, dir_axis: f32, min_axis: f32, max_axis: f32| -> bool {
        if dir_axis.abs() < eps {
            return origin_axis >= min_axis && origin_axis <= max_axis;
        }
        let inv = 1.0 / dir_axis;
        let mut t1 = (min_axis - origin_axis) * inv;
        let mut t2 = (max_axis - origin_axis) * inv;
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        t_min = t_min.max(t1);
        t_max = t_max.min(t2);
        t_min <= t_max
    };

    check_axis(origin.x, direction.x, min.x, max.x)
        && check_axis(origin.y, direction.y, min.y, max.y)
        && check_axis(origin.z, direction.z, min.z, max.z)
        && t_max >= 0.0
}
