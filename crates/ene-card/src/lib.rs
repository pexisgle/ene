//! # ene-card
//!
//! Character card containers (`CCv3` PNG chunks, `CHARX` archives), card
//! import/export, per-character settings, and localized card diffs.
//!
//! Depends on `ene-config` only for shared error, path, and language-alias
//! primitives; no settings-loading logic lives here.
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

/// Character card containers (PNG chunks, CHARX) and importing.
pub mod card_import;
/// App-specific asset types and URI resolution for card `assets`.
pub mod character_assets;
/// V3-format character card models with CBS macro expansion.
pub mod character_card;
/// Per-character configuration (position, motion, expressions).
pub mod character_config;
/// Per-character settings store with dirty tracking for auto-save.
pub mod character_store;
/// Character card enumeration and name-to-path resolution.
pub mod characters;
/// Localized card diffs (`character.{lang}.json`) and merge logic.
pub mod locale;

mod card_io;

pub use card_import::{ImportedCharacter, import_character_file};
pub use card_io::{
    generate_character_card_schema_json, generate_character_schema_json, save_character_card,
    write_character_schemas,
};
pub use character_assets::{
    DEFAULT_VRM_PATH, DEFAULT_VRMA_PATH, EneAssetKind, ResolvedAssetUri, decode_data_payload,
    resolve_asset_uri,
};
pub use character_card::{
    AffectBaseline, CharacterAsset, CharacterCardData, CharacterCardV3, EneExtension,
    ExpressionAffect, ExpressionDefinition, LabeledStyleExample, Lorebook, LorebookEntry,
    MacroContext, PolitenessLevel, RelationshipStage, ResolvedExpression, SceneBehavior,
    SpeechLength, SpeechStyleDefinition, TimePeriod, TimePeriodBehavior, expand_cbs_macros,
    expand_cbs_macros_ctx, expand_cbs_macros_with, resolve_expressions, session_pick_seed,
};
pub use character_config::{CharacterConfig, MotionCatalog, MotionEntry, MotionLayer};
pub use character_store::CharacterConfigStore;
pub use characters::{
    CharacterEntry, discover_characters, export_character_card, load_character_card,
    load_character_card_localized, resolve_character_path,
};
pub use locale::{
    LocalizedCharacterFields, LocalizedEneRoleplay, LocalizedLorebook, LocalizedLorebookEntry,
    LocalizedRelationshipStage, LocalizedSceneBehavior, LocalizedSpeechStyle,
    LocalizedStyleExample, LocalizedTimePeriodBehavior,
};

pub use schemars;
pub use serde;
