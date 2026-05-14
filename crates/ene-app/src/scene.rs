use bevy::{
    anti_alias::{fxaa::Fxaa, smaa::Smaa, taa::TemporalAntiAliasing},
    camera::ScalingMode,
    light::DirectionalLightShadowMap,
    prelude::*,
    window::PrimaryWindow,
};
#[cfg(target_os = "windows")]
use bevy::window::{Monitor, PrimaryMonitor, WindowMode, WindowPosition};
use std::thread;
use std::time::{Duration, Instant};

use crate::app_config::{AntialiasingMode, CharacterSettings};
use crate::platform::character_render_layers;

pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PrimaryWindowPresentationBootstrap>()
            .init_resource::<FramePacingState>()
            .add_systems(Startup, (spawn_camera_3d, spawn_light))
            .add_systems(Update, (bootstrap_primary_window_presentation, apply_graphics_settings))
            .add_systems(PostUpdate, pace_frame_rate);
    }
}

#[derive(Component)]
pub struct MainViewCamera;

#[derive(Resource, Default, Debug)]
struct PrimaryWindowPresentationBootstrap {
    applied: bool,
}

#[derive(Resource, Default, Debug)]
struct FramePacingState {
    last_frame_end: Option<Instant>,
}

pub(crate) const BASE_CAMERA_POSITION: Vec3 = Vec3::new(0.0, 1.0, 3.0);
pub(crate) const BASE_LOOK_TARGET: Vec3 = Vec3::new(0.0, 1.0, 0.0);

fn spawn_camera_3d(mut commands: Commands) {
    commands.spawn((
        MainViewCamera,
        Camera3d::default(),
        Projection::from(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: 2.6,
            },
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_translation(BASE_CAMERA_POSITION).looking_at(BASE_LOOK_TARGET, Vec3::Y),
    ));
}

fn spawn_light(mut commands: Commands) {
    commands.spawn((
        DirectionalLight {
            shadows_enabled: true,
            illuminance: 12_000.0,
            ..default()
        },
        Transform::from_xyz(3.0, 8.0, 4.0).looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
        character_render_layers(),
    ));
}

fn apply_graphics_settings(
    settings: Res<CharacterSettings>,
    mut shadow_map: ResMut<DirectionalLightShadowMap>,
    camera_query: Query<Entity, With<MainViewCamera>>,
    mut commands: Commands,
) {
    shadow_map.size = settings.shadow_quality.shadow_map_size();

    let Ok(camera_entity) = camera_query.single() else {
        return;
    };

    let mut camera = commands.entity(camera_entity);
    match settings.antialiasing_mode {
        AntialiasingMode::Off => {
            camera.remove::<Fxaa>();
            camera.remove::<Smaa>();
            camera.remove::<TemporalAntiAliasing>();
            camera.insert(Msaa::Off);
        }
        AntialiasingMode::Fxaa => {
            camera.insert(Fxaa::default());
            camera.remove::<Smaa>();
            camera.remove::<TemporalAntiAliasing>();
            camera.insert(Msaa::Off);
        }
        AntialiasingMode::Smaa => {
            camera.insert(Smaa::default());
            camera.remove::<Fxaa>();
            camera.remove::<TemporalAntiAliasing>();
            camera.insert(Msaa::Off);
        }
        AntialiasingMode::Taa => {
            camera.insert(TemporalAntiAliasing::default());
            camera.remove::<Fxaa>();
            camera.remove::<Smaa>();
            camera.insert(Msaa::Off);
        }
    }
}

fn pace_frame_rate(settings: Res<CharacterSettings>, mut pacing: ResMut<FramePacingState>) {
    if settings.target_fps == 0 {
        pacing.last_frame_end = Some(Instant::now());
        return;
    }

    let target_frame_duration = Duration::from_secs_f64(1.0 / f64::from(settings.target_fps));
    let now = Instant::now();

    let Some(last_frame_end) = pacing.last_frame_end else {
        pacing.last_frame_end = Some(now);
        return;
    };

    let elapsed = now.duration_since(last_frame_end);
    if elapsed < target_frame_duration {
        thread::sleep(target_frame_duration - elapsed);
    }

    pacing.last_frame_end = Some(Instant::now());
}

#[cfg(target_os = "windows")]
fn bootstrap_primary_window_presentation(
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    monitors: Query<&Monitor, With<PrimaryMonitor>>,
    mut bootstrap: ResMut<PrimaryWindowPresentationBootstrap>,
) {
    if bootstrap.applied {
        return;
    }

    let Ok(mut window) = windows.single_mut() else {
        return;
    };

    let Ok(monitor) = monitors.single() else {
        return;
    };

    window.window_level = bevy::window::WindowLevel::AlwaysOnTop;
    window.mode = WindowMode::Windowed;
    window.position = WindowPosition::At(monitor.physical_position);

    let target_size = monitor.physical_size();
    window
        .resolution
        .set_physical_resolution(target_size.x, target_size.y);
    bootstrap.applied = true;
}

#[cfg(not(target_os = "windows"))]
fn bootstrap_primary_window_presentation(
    _windows: Query<&mut Window, With<PrimaryWindow>>,
    mut bootstrap: ResMut<PrimaryWindowPresentationBootstrap>,
) {
    bootstrap.applied = true;
}
