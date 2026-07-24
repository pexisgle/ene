//! # ene-config
//!
//! Centralized JSON-based configuration management and schema generation for the ene AI character platform.
//!
//! Re-exports `serde`, `schemars`, and `ctor` for use by downstream crates.
#![warn(missing_docs)]
#![cfg_attr(
    test,
    expect(
        clippy::expect_used,
        reason = "unit tests use Result::expect for concise assertions"
    )
)]
#![expect(
    clippy::option_if_let_else,
    reason = "nursery style; match/if-let clarity preferred locally"
)]
extern crate self as ene_config;

/// V3-format character card models with CBS macro expansion.
pub mod character_card;
/// Per-character configuration (position, motion, expressions).
pub mod character_config;
/// Configuration loading, schema generation, and the global config registry.
pub mod config;
/// Configuration-related error types.
pub mod error;
/// Platform-aware directory and file path resolution.
pub mod paths;
/// Prompt template management with multi-language support.
pub mod prompts;
/// First-launch asset deployment and resource directory initialization.
pub mod resources;
/// Centralized config store with dirty tracking for auto-save.
pub mod store;
/// Smart content truncation helpers (folded from former `ene-common`).
pub mod truncate;

#[doc(no_inline)]
pub use truncate::{Truncate, TruncateResult};

pub use character_card::{
    CharacterAsset, CharacterCardData, CharacterCardV3, EneExtension, ExpressionDefinition,
    Lorebook, LorebookEntry, ResolvedExpression, expand_cbs_macros, resolve_expressions,
};

pub use character_config::{CharacterConfig, MotionCatalog, MotionEntry, MotionLayer};
pub use config::{
    ConfigTarget, DEFAULT_RUNTIME_RULES, EneConfig, HasConfigKey, generate_character_schema_json,
    generate_schema_json, get_global_config, get_global_section, load_character_card, load_config,
    load_config_from, load_full_config, load_full_config_from, register_config_schema,
    register_runtime_schema, register_tool_schema, resolve_character_path, save_full_config,
    update_global_config, update_section, write_schemas,
};
pub use error::ConfigError;
pub use error::EneConfigError;
pub use paths::{
    IS_DEV_BUILD, app_data_dir, assets_dir, builtin_plugins_dir, builtin_tools_dir,
    character_card_schema_file_path, character_schema_file_path, character_settings_path,
    config_file_path, models_dir, plugin_socket_dir, schema_file_path, tool_socket_dir,
    user_plugins_dir, user_tools_dir,
};
pub use prompts::{PromptLibrary, substitute as substitute_prompt_vars};
pub use resources::ensure_resource_dirs;
pub use store::ConfigStore;

// Re-export serde / schemars / ctor for sub-crates
pub use ctor::ctor;
pub use schemars;
pub use serde;

/// Helper macro to handle default values for config struct fields.
#[macro_export]
#[doc(hidden)]
macro_rules! __field_default {
    ($type:ty, $default:expr) => {
        $default
    };
    ($type:ty) => {
        <$type as Default>::default()
    };
}

/// Declarative macro for defining Config structs with zero boilerplate.
///
/// Each field uses `= default_value` inline syntax (optional).
#[macro_export]
macro_rules! define_config {
    (
        settings,
        $key:expr,
        $(#[$meta:meta])*
        pub struct $name:ident {
            $(
                $(#[$field_meta:meta])*
                pub $field:ident : $type:ty $( = $default:expr )?
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
                        $field : $crate::__field_default!($type $(, $default)?),
                    )*
                }
            }
        }

        impl $crate::HasConfigKey for $name {
            const KEY: &'static str = $key;
            const TARGET: $crate::ConfigTarget = $crate::ConfigTarget::Settings;
            fn path() -> &'static [&'static str] {
                &[$key]
            }
        }

        const _: () = {
            /// # Safety
            ///
            /// Called by `ctor` before `main`. Only safe registration code
            /// is executed; no I/O, TLS, or cross-ctor ordering assumed.
            #[ctor::ctor(unsafe)]
            fn register() {
                $crate::register_config_schema::<$name>($crate::ConfigTarget::Settings, None);
            }
        };
    };

    (
        character,
        $key:expr,
        $(#[$meta:meta])*
        pub struct $name:ident {
            $(
                $(#[$field_meta:meta])*
                pub $field:ident : $type:ty $( = $default:expr )?
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
                        $field : $crate::__field_default!($type $(, $default)?),
                    )*
                }
            }
        }

        impl $crate::HasConfigKey for $name {
            const KEY: &'static str = $key;
            const TARGET: $crate::ConfigTarget = $crate::ConfigTarget::Character;
            fn path() -> &'static [&'static str] {
                &[$key]
            }
        }

        const _: () = {
            /// # Safety
            ///
            /// Called by `ctor` before `main`. Only safe registration code
            /// is executed; no I/O, TLS, or cross-ctor ordering assumed.
            #[ctor::ctor(unsafe)]
            fn register() {
                $crate::register_config_schema::<$name>($crate::ConfigTarget::Character, None);
            }
        };
    };

    (
        $parent:ident,
        $key:expr,
        $(#[$meta:meta])*
        pub struct $name:ident {
            $(
                $(#[$field_meta:meta])*
                pub $field:ident : $type:ty $( = $default:expr )?
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
                        $field : $crate::__field_default!($type $(, $default)?),
                    )*
                }
            }
        }

        impl $crate::HasConfigKey for $name {
            const KEY: &'static str = $key;
            const TARGET: $crate::ConfigTarget = <$parent as $crate::HasConfigKey>::TARGET;
            fn path() -> &'static [&'static str] {
                use std::sync::OnceLock;
                static PATH: OnceLock<Vec<&'static str>> = OnceLock::new();
                PATH.get_or_init(|| {
                    let mut p = <$parent as $crate::HasConfigKey>::path().to_vec();
                    p.push($key);
                    p
                }).as_slice()
            }
        }

        const _: () = {
            /// # Safety
            ///
            /// Called by `ctor` before `main`. Only safe registration code
            /// is executed; no I/O, TLS, or cross-ctor ordering assumed.
            #[ctor::ctor(unsafe)]
            fn register() {
                $crate::register_config_schema::<$name>(
                    <$parent as $crate::HasConfigKey>::TARGET,
                    Some(<$parent as $crate::HasConfigKey>::KEY),
                );
            }
        };
    };
}

/// Declarative macro for defining tool configuration schemas.
#[macro_export]
macro_rules! define_tool_config {
    (
        $tool_name:expr,
        $(#[$meta:meta])*
        pub struct $name:ident {
            $(
                $(#[$field_meta:meta])*
                pub $field:ident : $type:ty $( = $default:expr )?
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
                        $field : $crate::__field_default!($type $(, $default)?),
                    )*
                }
            }
        }

        const _: () = {
            /// # Safety
            ///
            /// Called by `ctor` before `main`. Only safe registration code
            /// is executed; no I/O, TLS, or cross-ctor ordering assumed.
            #[ctor::ctor(unsafe)]
            fn register() {
                $crate::register_tool_schema::<$name>($tool_name);
            }
        };
    };
}

/// Declarative macro for defining labeled enums with a consistent API.
///
/// The **first variant** listed becomes the `Default` for the enum (via
/// `#[derive(Default)]`). Ensure the most common or safest variant is listed
/// first.
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
