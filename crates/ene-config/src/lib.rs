//! # ene-config
//!
//! Centralized JSON-based configuration management and schema generation for the ene AI character platform.
//!
//! ## Key Features
//!
//! - **Settings loading**: Multi-layer config resolution from `settings.json`, environment variables, and defaults
//! - **Auto schema generation**: `settings.schema.json` and character schemas are generated automatically
//! - **Character cards**: V3-format character card models with expression definitions and CBS macro expansion
//! - **Declarative macros**: `define_config!` and `define_label_enum!` for boilerplate-free config struct and enum definitions
//! - **Path resolution**: Platform-aware directory discovery for assets, tools, sockets, and config files
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use ene_config::load_settings;
//!
//! let settings = load_settings();
//! println!("Using character: {}", settings.character);
//! ```
//!
//! Re-exports `serde`, `schemars`, and `ctor` for use by downstream crates.
#![warn(missing_docs)]
extern crate self as ene_config;

/// V3-format character card models with CBS macro expansion.
pub mod character_card;
/// Per-character settings (position, motion, expressions).
pub mod character_settings;
/// Settings loading, schema generation, and the global settings registry.
pub mod config;
/// Configuration-related error types.
pub mod error;
/// Platform-aware directory and file path resolution.
pub mod paths;
/// First-launch asset deployment and resource directory initialization.
pub mod resources;

pub use character_card::{
    CharacterAsset, CharacterCardData, CharacterCardV3, ExpressionDefinition, Lorebook,
    LorebookEntry, ResolvedExpression, expand_cbs_macros, resolve_expressions,
};

pub use character_settings::{CharacterPerSettings, generate_character_schema_json};
pub use config::{
    EneSettings, generate_schema_json, get_global_section, get_global_settings, load_full_settings,
    load_full_settings_from, load_settings, load_settings_from, register_runtime_schema,
    register_schema, register_schema_with_parent, save_full_settings, update_global_settings,
    update_section,
};
pub use error::ConfigError;
pub use paths::{
    IS_DEV_BUILD, app_data_dir, assets_dir, builtin_tools_dir, character_schema_file_path,
    character_settings_path, config_file_path, models_dir, schema_file_path, tool_socket_dir,
    user_tools_dir,
};
pub use resources::ensure_resource_dirs;

// Re-export serde / schemars / ctor for sub-crates
pub use ctor::ctor;
pub use schemars;
pub use serde;

/// Declarative macro for defining Config structs with zero boilerplate.
///
/// Each field uses `= default_value` inline syntax. The macro automatically derives
/// `Default`, `Serialize`, `Deserialize`, and `JsonSchema`, and auto-registers the schema.
///
/// Use `parent = "ParentName"` to nest the schema under a parent definition at generation time.
///
/// # Example
///
/// ```rust,ignore
/// use ene_config::define_config;
///
/// define_config!(
///     "provider",
///     pub struct ProviderSettings {
///         pub model: String = "gpt-4o-mini".into(),
///         pub base_url: String = String::new(),
///         pub api_key: String = String::new(),
///     }
/// );
/// ```
#[macro_export]
macro_rules! define_config {
    (
        $key:expr,
        parent = $parent:expr,
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
                $crate::register_schema_with_parent::<$name>($key, $parent);
            }
        };
    };

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

/// Declarative macro for defining labeled enums with a consistent API.
///
/// Each variant uses `Variant => "label_string"` syntax. Optionally supports
/// `| extra_data` and a trailing `[method_name: Type]` to auto-generate an extra data accessor.
///
/// Automatically derives: `Serialize`, `Deserialize`, `JsonSchema`, `Default`, `Copy`, `Clone`,
/// `Debug`, `PartialEq`, `Eq`, and generates a `label(&self) -> &'static str` method.
///
/// # Example
///
/// ```rust,ignore
/// use ene_config::define_label_enum;
///
/// define_label_enum!(
///     pub enum EmbeddingProviderType {
///         Api => "API",
///         Local => "Local",
///     }
///     [default_model: &str]
/// );
/// ```
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
            /// Returns the display label for this variant.
            pub fn label(&self) -> &'static str {
                match self {
                    $(
                        Self::$variant => $label,
                    )*
                }
            }

            /// Returns extra data associated with this variant.
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
            /// Returns the display label for this variant.
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
