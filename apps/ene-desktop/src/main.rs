//! # ene-desktop
//!
//! Desktop GUI application for the ene AI character platform.
//! Built with Bevy, featuring a VRM character overlay, egui settings window,
//! character drag interaction, and system tray integration.
#![warn(missing_docs)]
#![allow(warnings)]

mod ai_bridge;
mod app_config;
mod character;
mod character_drag;
mod platform;
mod resources;
mod scene;
mod settings_ui;
mod tray;

use bevy::asset::AssetPlugin;
use bevy::light::DirectionalLightShadowMap;
use bevy::prelude::*;
use bevy::render::{
    RenderPlugin,
    settings::{Backends, RenderCreation, WgpuSettings},
};
use bevy::winit::WinitSettings;
use bevy_egui::EguiPlugin;
use bevy_vrm1::prelude::*;

use ai_bridge::EnePlugin;
use app_config::{CharacterSettings, DEFAULT_SHADOW_QUALITY, window_plugin};
use character::CharacterPlugin;
use character_drag::CharacterDragPlugin;
use scene::ScenePlugin;
use settings_ui::SettingsUiPlugin;
use tray::TrayPlugin;

fn main() {
    #[cfg(target_os = "windows")]
    unsafe {
        std::env::set_var("WGPU_DX12_PRESENTATION_SYSTEM", "DxgiFromVisual");
    }

    #[cfg(target_os = "linux")]
    if let Err(err) = gtk::init() {
        panic!("Failed to initialize GTK: {err}");
    }

    let runtime = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
    let _runtime_guard = runtime.enter();

    let assets_dir = resources::ensure_resource_dirs();
    let (default_vrm, _default_vrma) = app_config::read_cli_paths();
    let settings = CharacterSettings::discover(&assets_dir, default_vrm);

    App::new()
        .insert_resource(settings)
        .insert_resource(DirectionalLightShadowMap {
            size: DEFAULT_SHADOW_QUALITY.shadow_map_size(),
        })
        .insert_resource(ClearColor(Color::NONE))
        .insert_resource(WinitSettings::desktop_app())
        .add_plugins((
            DefaultPlugins
                .set(window_plugin())
                .set(render_plugin())
                .set(AssetPlugin {
                    file_path: assets_dir.to_string_lossy().into(),
                    ..default()
                }),
            EguiPlugin::default(),
            VrmPlugin,
            VrmaPlugin,
            ScenePlugin,
            EnePlugin,
            CharacterPlugin,
            TrayPlugin,
            SettingsUiPlugin,
            CharacterDragPlugin,
        ))
        .run();
}

fn render_plugin() -> RenderPlugin {
    #[cfg(target_os = "windows")]
    {
        RenderPlugin {
            render_creation: RenderCreation::Automatic(WgpuSettings {
                backends: Some(Backends::DX12),
                ..default()
            }),
            ..default()
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        RenderPlugin::default()
    }
}
