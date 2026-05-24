use serde::{Deserialize, Serialize, de::DeserializeOwned};
use schemars::JsonSchema;
use std::collections::HashMap;
use std::path::Path;
use crate::error::ConfigError;





pub static GLOBAL_SETTINGS: std::sync::OnceLock<std::sync::RwLock<EneSettings>> = std::sync::OnceLock::new();

/// グローバルな EneSettings を更新します。
pub fn update_global_settings(settings: EneSettings) {
    if let Some(lock) = GLOBAL_SETTINGS.get() {
        if let Ok(mut guard) = lock.write() {
            *guard = settings;
        }
    } else {
        let _ = GLOBAL_SETTINGS.set(std::sync::RwLock::new(settings));
    }
}

/// グローバル設定全体のクローンを取得します。
pub fn get_global_settings() -> EneSettings {
    if let Some(lock) = GLOBAL_SETTINGS.get() {
        if let Ok(guard) = lock.read() {
            return guard.clone();
        }
    }
    EneSettings::default()
}

/// グローバル設定から指定されたキーのサブセクションをロードします。
pub fn get_global_section<T: serde::de::DeserializeOwned + Default>(key: &str) -> T {
    if let Some(lock) = GLOBAL_SETTINGS.get() {
        if let Ok(guard) = lock.read() {
            return guard.get_section::<T>(key).unwrap_or_default();
        }
    }
    T::default()
}

static SCHEMA_REGISTRY: std::sync::OnceLock<std::sync::Mutex<HashMap<String, schemars::schema::RootSchema>>> = std::sync::OnceLock::new();

/// 各クレートが自身の Config スキーマを登録するためのグローバルな静的ヘルパー
pub fn register_schema<T: JsonSchema>(key: &str) {
    let schema_gen = schemars::r#gen::SchemaSettings::draft07().into_generator();
    let schema = schema_gen.into_root_schema_for::<T>();
    let registry = SCHEMA_REGISTRY.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    if let Ok(mut reg) = registry.lock() {
        reg.insert(key.to_string(), schema);
    }
}

crate::define_config!(
    "ene_settings",
    pub struct EneSettings {
        pub version: u32 = 1,
        pub character: String = String::new(),
        pub user_name: String = "User".to_string(),
        pub runtime_rules: String = "Keep responses relatively short and sweet, suitable for displaying on a screen overlay.".to_string(),

        #[serde(flatten)]
        #[schemars(skip)]
        pub extra: HashMap<String, serde_json::Value> = HashMap::new(),
    }
);



impl EneSettings {

    /// 汎用的なセクションのデシリアライズヘルパー。
    /// 指定されたキーが存在すればそれをパースし、存在しなければ Default::default() を返します。
    pub fn get_section<T>(&self, key: &str) -> Result<T, ConfigError>
    where
        T: serde::de::DeserializeOwned + Default,
    {
        if let Some(val) = self.extra.get(key) {
            serde_json::from_value(val.clone()).map_err(|e| {
                ConfigError::GenericConfigError(format!("Failed to deserialize section '{}': {}", key, e))
            })
        } else {
            Ok(T::default())
        }
    }

    /// 汎用的なセクションのシリアライズ・保存ヘルパー。
    pub fn set_section<T>(&mut self, key: &str, section: &T) -> Result<(), ConfigError>
    where
        T: serde::Serialize,
    {
        let val = serde_json::to_value(section).map_err(|e| {
            ConfigError::GenericConfigError(format!("Failed to serialize section '{}': {}", key, e))
        })?;
        self.extra.insert(key.to_string(), val);
        Ok(())
    }
}


/// JSON Schema の JSON 表現を生成します
pub fn generate_schema_json() -> Result<String, serde_json::Error> {
    let schema_gen = schemars::r#gen::SchemaSettings::draft07().into_generator();
    let mut root_schema = schema_gen.into_root_schema_for::<EneSettings>();
    
    if let Some(registry) = SCHEMA_REGISTRY.get() {
        if let Ok(reg) = registry.lock() {
            for (key, sub_root) in reg.iter() {
                let schema_obj = &mut root_schema.schema;
                if let Some(object) = &mut schema_obj.object {
                    object.properties.insert(
                        key.clone(),
                        schemars::schema::Schema::Object(sub_root.schema.clone())
                    );
                    
                    for (def_name, def_schema) in &sub_root.definitions {
                        root_schema.definitions.insert(def_name.clone(), def_schema.clone());
                    }
                }
            }
        }
    }
    
    serde_json::to_string_pretty(&root_schema)
}

/// アセットディレクトリ of settings.json を読み込み、character_card_path などを解決した EneSettings を返す。
pub fn load_settings() -> EneSettings {
    let assets_dir = crate::paths::assets_dir();
    let config_path = crate::paths::config_file_path();
    load_settings_from(&assets_dir, &config_path)
}

/// 指定されたアセットディレクトリと設定ファイルパスから設定を読み込む。
pub fn load_settings_from(assets_dir: &Path, config_path: &Path) -> EneSettings {
    load_full_settings_from(assets_dir, config_path)
}

/// 設定ファイルを完全に読み込む。起動時に schema ファイルも自動更新する。
pub fn load_full_settings() -> EneSettings {
    let assets_dir = crate::paths::assets_dir();
    let config_path = crate::paths::config_file_path();
    load_full_settings_from(&assets_dir, &config_path)
}

/// 指定されたアセットディレクトリと設定ファイルパスから EneSettings を完全に読み込む。
pub fn load_full_settings_from(assets_dir: &Path, config_path: &Path) -> EneSettings {
    use figment::{Figment, providers::{Format, Json, Env, Serialized}};

    let figment = Figment::from(Serialized::defaults(EneSettings::default()))
        .merge(Json::file(config_path))
        .merge(Env::prefixed("ENE_").split("__"));

    let mut settings: EneSettings = figment.extract().unwrap_or_else(|e| {
        tracing::error!("Failed to load configuration: {e}, using default");
        EneSettings::default()
    });

    // スキーマの自動書き出し生成
    let schema_path = assets_dir.join("settings.schema.json");
    if let Ok(schema_json) = generate_schema_json() {
        let _ = std::fs::write(&schema_path, schema_json);
    }

    // キャラクター固有設定のスキーマ自動書き出し生成
    let char_schema_path = crate::paths::character_schema_file_path();
    if let Ok(char_schema_json) = crate::character_settings::generate_character_schema_json() {
        let _ = std::fs::write(&char_schema_path, char_schema_json);
    }

    if settings.character.trim().is_empty() {
        settings.character = format!("{}/characters/Alicia/character.json", assets_dir.display());
    } else if !settings.character.contains('/') && !settings.character.contains('\\') {
        settings.character = format!(
            "{}/characters/{}/character.json",
            assets_dir.display(),
            settings.character
        );
    }

    update_global_settings(settings.clone());
    settings
}

/// 設定ファイルを型安全に保存する
pub fn save_full_settings(settings: &EneSettings) -> Result<(), std::io::Error> {
    update_global_settings(settings.clone());
    let config_path = crate::paths::config_file_path();
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(config_path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    crate::define_config!(
        "dummy_test_config",
        pub struct DummyTestConfig {
            pub test_value: String = "hello".to_string(),
            pub test_number: i32 = 42,
        }
    );

    #[test]
    fn test_define_config_self_registration() {
        // ctor will run before tests, so the schema should be in the registry.
        let schema_json = generate_schema_json().unwrap();
        assert!(schema_json.contains("dummy_test_config"), "Schema should automatically include dummy_test_config");
        assert!(schema_json.contains("test_value"), "Schema should automatically include dummy_test_config field test_value");
    }



    #[test]
    fn test_global_settings_accessor() {
        let mut raw_settings = EneSettings::default();
        raw_settings.extra.insert(
            "dummy_test_config".to_string(),
            serde_json::json!({
                "test_value": "custom_val",
                "test_number": 999
            })
        );
        update_global_settings(raw_settings);

        let config = get_global_section::<DummyTestConfig>("dummy_test_config");
        assert_eq!(config.test_value, "custom_val");
        assert_eq!(config.test_number, 999);
    }
}
