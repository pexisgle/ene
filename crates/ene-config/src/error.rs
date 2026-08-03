use thiserror::Error;

/// Errors that can occur during configuration loading, schema generation, or
/// card resolution.
#[derive(Error, Debug)]
pub enum EneConfigError {
    /// An AI provider base URL is missing and no environment variable fallback
    /// was found.
    #[error("Missing base url: set {env_var} or configure AI Base URL")]
    MissingBaseUrl {
        /// The environment variable that was checked.
        env_var: String,
    },
    /// An API key is missing and no environment variable fallback was found.
    #[error("Missing API key: set {env_var} or configure AI API Key")]
    MissingApiKey {
        /// The environment variable that was checked.
        env_var: String,
    },
    /// No character card has been loaded yet.
    #[error("Character card not loaded")]
    NoCharacterCard,
    /// The `character` setting is empty: no character has been selected.
    ///
    /// Distinct from [`Self::CardReadError`], which reports a configured
    /// character whose card file is missing or unreadable.
    #[error("No character selected: the `character` setting is empty")]
    CharacterNotConfigured,
    /// A character name that escapes the working directory was rejected.
    ///
    /// Character cards are third-party artifacts, so `..` traversal
    /// components must not turn a card reference into a read outside the
    /// assets tree.
    #[error("Unsafe character path: {0}")]
    UnsafeCharacterPath(String),
    /// An asset URI uses a scheme this build does not consume.
    ///
    /// The Character Card Spec lets applications ignore unsupported URI
    /// types, so callers treat this as "skip the asset", not a hard failure.
    #[error("Unsupported asset URI scheme: {0}")]
    UnsupportedAssetUriScheme(String),
    /// An asset path escapes the card's directory.
    ///
    /// `assets[].uri` comes from third-party card distributions; traversal
    /// components and absolute paths must not resolve outside the card.
    #[error("Unsafe asset path: {0}")]
    UnsafeAssetPath(String),
    /// A malformed asset URI (embedded path, data URL, or http URL).
    #[error("Invalid asset URI: {0}")]
    InvalidAssetUri(String),
    /// A data-URL payload exceeds the materialization size cap.
    #[error("Asset data URI payload exceeds the {0} byte limit")]
    AssetPayloadTooLarge(u64),
    /// The card file format is not an importable container.
    #[error("Unsupported character card file: {0}")]
    UnsupportedCardFile(String),
    /// A CHARX archive could not be read (corrupt or unsupported zip).
    #[error("CHARX archive error: {0}")]
    CharxError(#[from] zip::result::ZipError),
    /// A CHARX archive has no `card.json` at its root.
    #[error("CHARX archive is missing card.json")]
    CharxMissingCard,
    /// A CHARX archive entry path escapes the extraction directory.
    #[error("Unsafe path in CHARX archive: {0}")]
    CharxUnsafePath(String),
    /// A CHARX archive contains an encrypted entry.
    #[error("CHARX archive contains an encrypted entry: {0}")]
    CharxEncrypted(String),
    /// A CHARX archive exceeds the extraction size limits.
    #[error("CHARX archive exceeds the extraction size limit at {0}")]
    CharxTooLarge(String),
    /// The PNG card has neither a `ccv3` nor a `chara` text chunk.
    #[error("PNG card is missing a ccv3 or chara text chunk")]
    PngCardMissingChunk,
    /// The PNG card bytes are not a valid PNG file.
    #[error("Invalid PNG card: {0}")]
    InvalidPngCard(String),
    /// The import target folder already exists.
    #[error("Character import target already exists: {0}")]
    CharacterImportExists(String),
    /// The card has no usable name to derive an import folder from.
    #[error("Character card has no usable name for import")]
    CharacterImportUnnamed,
    /// I/O error while reading a character card file.
    #[error("Failed to read character card: {0}")]
    CardReadError(#[from] std::io::Error),
    /// JSON deserialisation error.
    #[error("Failed to parse JSON: {0}")]
    JsonError(#[from] serde_json::Error),
    /// JSON serialisation error while writing a character card.
    ///
    /// Deliberately not `#[from]`: `JsonError` already owns the
    /// `serde_json::Error` conversion and a serialisation failure must not
    /// read as a parse failure.
    #[error("Failed to serialize JSON: {0}")]
    SerializeError(serde_json::Error),
    /// Catch-all configuration error with a free-form message.
    #[error("Configuration error: {0}")]
    GenericConfigError(String),
    /// The on-disk config declares a schema version newer than this build
    /// understands. This typically means the file was written by a newer
    /// application version (a downgrade). The file is left untouched so a
    /// newer build can still read it.
    #[error(
        "config version {found} is newer than the supported version {supported}; \
         refusing to load to avoid corrupting data written by a newer build"
    )]
    ConfigVersionTooNew {
        /// The version number found in the on-disk file.
        found: u32,
        /// The highest version this build can read.
        supported: u32,
    },
    /// General I/O error.
    #[error("I/O error: {0}")]
    IoError(#[source] std::io::Error),
    /// A runtime prompt pack could not be read from the assets directory. The
    /// caller is expected to fall back to the compile-time embedded library.
    #[error("Failed to read prompt pack at {path}: {source}")]
    PromptPackRead {
        /// The pack path that was attempted.
        path: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A runtime pattern pack could not be read from the assets directory. The
    /// caller is expected to fall back to the compile-time embedded library.
    #[error("Failed to read pattern pack at {path}: {source}")]
    PatternPackRead {
        /// The pack path that was attempted.
        path: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// Type alias for internal module usages.
pub type ConfigError = EneConfigError;
