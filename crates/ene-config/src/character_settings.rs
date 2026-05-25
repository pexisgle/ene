crate::define_config!(
    "character_settings",
    /// Per-character visual and motion settings used by the desktop GUI.
    pub struct CharacterPerSettings {
        /// 3D position of the character model in the scene.
        pub character_position: [f32; 3] = [0.0, 0.0, 0.0],
        /// Path to the selected VRMA motion file.
        pub selected_motion_path: String = String::new(),
        /// Scale factor applied to the character model.
        pub model_scale: f32 = 1.0,
        /// How strongly the character looks toward the user (0.0–1.0).
        pub look_at_strength: f32 = 0.6,
        /// Default motion (VRMA) file path.
        pub default_motion: String = String::new(),
        /// Expression overrides stored as raw JSON.
        pub expressions: Option<serde_json::Value> = None,
    }
);

/// CharacterPerSettings の JSON Schema の JSON 表現を生成します
pub fn generate_character_schema_json() -> Result<String, serde_json::Error> {
    let schema_gen = schemars::r#gen::SchemaSettings::draft07().into_generator();
    let root_schema = schema_gen.into_root_schema_for::<CharacterPerSettings>();
    serde_json::to_string_pretty(&root_schema)
}

