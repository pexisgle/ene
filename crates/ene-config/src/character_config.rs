crate::define_config!(
    "character_settings",
    /// Per-character visual and motion config used by the desktop GUI.
    pub struct CharacterPerConfig {
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

/// Generates the JSON representation of the CharacterPerConfig JSON Schema
pub fn generate_character_schema_json() -> Result<String, serde_json::Error> {
    let schema_gen = schemars::SchemaGenerator::default();
    let root_schema = schema_gen.into_root_schema_for::<CharacterPerConfig>();
    serde_json::to_string_pretty(&root_schema)
}
