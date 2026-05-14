mod app_config;
mod ai_bridge;
mod character;
mod platform;
mod resources;
mod scene;
mod settings_ui;
mod tray;
mod window_drag;

use bevy::prelude::*;
use bevy::asset::AssetPlugin;
use bevy::light::DirectionalLightShadowMap;
use bevy_egui::EguiPlugin;
use bevy_vrm1::prelude::*;

use app_config::{CharacterSettings, DEFAULT_SHADOW_QUALITY, window_plugin};
use ai_bridge::AiPlugin;
use character::CharacterPlugin;
use scene::ScenePlugin;
use settings_ui::SettingsUiPlugin;
use tray::TrayPlugin;
use window_drag::WindowDragPlugin;

fn main() {
    let assets_dir = resources::ensure_resource_dirs();
    let (default_vrm, default_vrma) = app_config::read_cli_paths();
    let settings = CharacterSettings::discover(&assets_dir, default_vrm, default_vrma);

    App::new()
        .insert_resource(settings)
        .insert_resource(DirectionalLightShadowMap {
            size: DEFAULT_SHADOW_QUALITY.shadow_map_size(),
        })
        .insert_resource(ClearColor(Color::NONE))
        .add_plugins((
            DefaultPlugins
                .set(window_plugin())
                .set(AssetPlugin {
                    file_path: assets_dir.to_string_lossy().into(),
                    ..default()
                }),
            EguiPlugin::default(),
            VrmPlugin,
            VrmaPlugin,
            ScenePlugin,
            AiPlugin,
            CharacterPlugin,
            TrayPlugin,
            SettingsUiPlugin,
            WindowDragPlugin,
        ))
        .run();
}
