extern crate self as ene_config;

pub mod config;
pub mod character_card;
pub mod error;
pub mod paths;
pub mod resources;
pub mod character_settings;

pub use character_card::{
    CharacterCardV3, CharacterCardData, CharacterAsset, Lorebook, LorebookEntry,
    ExpressionDefinition, ResolvedExpression, resolve_expressions, expand_cbs_macros,
};

pub use config::{
    EneSettings,
    load_full_settings, load_full_settings_from, load_settings, load_settings_from,
    save_full_settings, generate_schema_json, register_schema, get_global_section,
    update_global_settings, get_global_settings,
};
pub use character_settings::CharacterPerSettings;
pub use error::ConfigError;
pub use paths::{
    IS_DEV_BUILD, app_data_dir, assets_dir, builtin_tools_dir, config_file_path, models_dir,
    schema_file_path, tool_socket_dir, user_tools_dir, character_settings_path,
};
pub use resources::ensure_resource_dirs;

// サブクレート向けに serde / schemars / ctor を公開
pub use serde;
pub use schemars;
pub use ctor::ctor;

/// 機能クレートがボイラープレートなしで Config 構造体を簡潔に定義・自動登録するための宣言的マクロ。
/// 各フィールドには `= デフォルト値` の形式でインラインのデフォルト値を指定でき、
/// 自動的に `Default` トレイトの実装も生成されます。
#[macro_export]
macro_rules! define_config {
    (
        $key:expr,
        $(#[$meta:meta])*
        pub struct $name:ident {
            $(
                $(#[$field_meta:meta])*
                pub $field:ident : $type:ty = $default:expr
            ),* $(,)?
        }
    ) => {
        #[derive(Debug, Clone, $crate::serde::Serialize, $crate::serde::Deserialize, $crate::schemars::JsonSchema)]
        #[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
        #[schemars(crate = "::ene_config::schemars")]
        $(#[$meta])*
        pub struct $name {
            $(
                $(#[$field_meta])*
                pub $field : $type,
            )*
        }

        impl Default for $name {
            fn default() -> Self {
                Self {
                    $(
                        $field : $default,
                    )*
                }
            }
        }

        const _: () = {
            #[$crate::ctor]
            fn register() {
                $crate::register_schema::<$name>($key);
            }
        };
    };
}

/// ラベル付きの Enum を簡単かつ統一的な形式で定義するための宣言的マクロ。
/// 各バリアントに `=> "ラベル文字列"` の形式で表示名をインライン指定でき、
/// オプションで `| 追加データ` と末尾の `[メソッド名: 型]` を記述することで、
/// カスタムの追加データ取得用メソッドも一緒に自動生成できます。
/// 自動的に `Serialize`/`Deserialize`/`JsonSchema`/`Default`/`Copy`/`Clone`/`Debug`/`PartialEq`/`Eq`
/// などの必要なトレイトと `label(&self) -> &'static str` メソッドも生成します。
#[macro_export]
macro_rules! define_label_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant:ident => $label:expr => $extra:expr
            ),* $(,)?
        }
        [$method:ident : $val_type:ty]
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, $crate::serde::Serialize, $crate::serde::Deserialize, $crate::schemars::JsonSchema)]
        #[serde(crate = "::ene_config::serde", rename_all = "snake_case")]
        #[schemars(crate = "::ene_config::schemars")]
        $(#[$meta])*
        pub enum $name {
            $(
                $(#[$variant_meta])*
                $variant,
            )*
        }

        impl $name {
            pub fn label(&self) -> &'static str {
                match self {
                    $(
                        Self::$variant => $label,
                    )*
                }
            }

            pub fn $method(&self) -> $val_type {
                match self {
                    $(
                        Self::$variant => $extra,
                    )*
                }
            }
        }
    };

    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant:ident => $label:expr
            ),* $(,)?
        }
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, $crate::serde::Serialize, $crate::serde::Deserialize, $crate::schemars::JsonSchema)]
        #[serde(crate = "::ene_config::serde", rename_all = "snake_case")]
        #[schemars(crate = "::ene_config::schemars")]
        $(#[$meta])*
        pub enum $name {
            $(
                $(#[$variant_meta])*
                $variant,
            )*
        }

        impl $name {
            pub fn label(&self) -> &'static str {
                match self {
                    $(
                        Self::$variant => $label,
                    )*
                }
            }
        }
    };
}

