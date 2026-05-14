use std::path::PathBuf;

const APP_ID: &str = "dev.pexisgle.Ene";

pub fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .map(|p| p.join(APP_ID))
        .unwrap_or_else(|| PathBuf::from(APP_ID))
}

pub fn assets_dir() -> PathBuf {
    app_data_dir()
}

pub fn models_dir() -> PathBuf {
    app_data_dir().join("models")
}

pub fn config_file_path() -> PathBuf {
    assets_dir().join("settings.json")
}

pub fn schema_file_path() -> PathBuf {
    assets_dir().join("settings.schema.json")
}
