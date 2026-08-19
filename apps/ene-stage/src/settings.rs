use ene_config::{EneConfigError, load_full_config, save_full_config};

ene_config::define_config!(
    settings,
    "desktop",
    /// Stage client UI, overlay, and core lifetime preferences.
    pub struct DesktopSettings {
        pub core_lifetime: String = "app".to_owned(),
        pub theme: String = "system".to_owned(),
        pub language: String = String::new(),
        pub mic_device: String = String::new(),
        pub spotlight_enabled: bool = true,
        pub caption_enabled: bool = true,
        pub caption_font_size: f32 = 18.0,
        pub caption_position: String = "bottom".to_owned(),
        pub caption_pinned: bool = false,
        pub beat_sync: bool = false,
        pub beat_sync_device: String = String::new(),
        pub graphics_quality: String = "high".to_owned(),
        pub always_on_top: bool = true,
        pub transparent_overlay: bool = true,
        pub model_scale: f32 = 1.0,
        pub character_x: f32 = 0.7,
        pub character_y: f32 = 0.15,
        pub look_at_strength: f32 = 0.6,
    }
);

#[must_use]
pub fn load_desktop_settings() -> DesktopSettings {
    match load_full_config() {
        Ok(config) => config
            .get_section::<DesktopSettings>()
            .unwrap_or_else(|err| {
                tracing::warn!(error = %err, "desktop settings section missing; using defaults");
                DesktopSettings::default()
            }),
        Err(err) => {
            tracing::warn!(error = %err, "failed to load config; using desktop defaults");
            DesktopSettings::default()
        }
    }
}

pub fn save_desktop_settings(settings: &DesktopSettings) -> Result<(), EneConfigError> {
    let mut config = load_full_config()?;
    config.set_section(settings)?;
    save_full_config(&config)
}
