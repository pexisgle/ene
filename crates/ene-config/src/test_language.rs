use crate::config::EneConfig;
use crate::define_config;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Default)]
#[schemars(crate = "::ene_config::schemars")]
pub enum Language {
    #[serde(rename = "en")]
    #[default]
    En,
    #[serde(rename = "ja")]
    Ja,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GraphicsSection {
    pub shadow_quality: String,
}

impl Default for GraphicsSection {
    fn default() -> Self {
        Self { shadow_quality: "medium".to_string() }
    }
}

define_config!(
    settings,
    "desktop",
    pub struct DesktopSection {
        pub graphics: GraphicsSection,
        #[serde(default)]
        pub language: Language,
    }
);

#[test]
fn test_language_serialize() {
    let mut config = EneConfig::default();
    let desktop = DesktopSection {
        graphics: GraphicsSection::default(),
        language: Language::Ja,
    };
    config.set_section(&desktop).unwrap();
    let json = serde_json::to_string_pretty(&config.extra).unwrap();
    println!("Serialized extra: {}", json);
    assert!(json.contains("ja"));
}
