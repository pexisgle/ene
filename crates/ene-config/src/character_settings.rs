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

/// CharacterPerSettings の JSON Schema の JSON 表現を生成します
pub fn generate_character_schema_json() -> Result<String, serde_json::Error> {
    let schema_gen = schemars::r#gen::SchemaSettings::draft07().into_generator();
    let root_schema = schema_gen.into_root_schema_for::<CharacterPerSettings>();
    serde_json::to_string_pretty(&root_schema)
}

