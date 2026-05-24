crate::define_config!(
    "character_settings",
    pub struct CharacterPerSettings {
        pub character_position: [f32; 3] = [0.0, 0.0, 0.0],
        pub selected_motion_path: String = String::new(),
        pub model_scale: f32 = 1.0,
        pub look_at_strength: f32 = 0.6,
        pub default_motion: String = String::new(),
        pub expressions: Option<serde_json::Value> = None,
    }
);
