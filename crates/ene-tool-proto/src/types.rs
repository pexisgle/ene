//! Core tool types: `ToolName`, `ToolSpec`, `ActionSpec`, `KeywordSet`,
//! `SideEffects`, `ToolExample`, `ToolVersion`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A validated, namespaced tool identifier.
///
/// Format:
/// - Mega-tools: `"<namespace>.<action>"` (e.g. `"filesystem.read"`)
/// - Individual tools: `"<name>"` (e.g. `"utility.get_current_time"`)
///
/// Use [`ToolName::try_new`] to validate untrusted input (IPC, MCP,
/// config, DB rows). The panicking constructors
/// ([`ToolName::new`], [`From<&str>`], [`From<String>`]) are intended
/// for compile-time-validated string literals only.
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolName(pub(crate) String);

impl JsonSchema for ToolName {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("ToolName")
    }
    fn json_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "A namespaced tool name (e.g. 'filesystem.read' or 'get_current_time')."
        })
    }
}

impl ToolName {
    /// Valid characters for a tool name: alphanumeric, `_`, `.`, `:`
    const fn valid_char(c: u8) -> bool {
        c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c == b':'
    }

    /// Returns `true` if the name is non-empty and contains only valid characters.
    pub fn is_valid(name: &str) -> bool {
        !name.is_empty()
            && name.bytes().all(Self::valid_char)
            && !name.starts_with('.')
            && !name.ends_with('.')
            && !name.starts_with(':')
            && !name.ends_with(':')
    }

    /// Construct a new `ToolName` from a string.
    ///
    /// # Panics
    /// Panics if the name is empty or contains invalid characters.
    /// Accepts alphanumeric, `_`, `.`, and `:` (no leading/trailing dots).
    ///
    /// Only use for trusted compile-time-validated inputs (e.g.
    /// string literals, `#[tool]` attribute names). For untrusted
    /// input from IPC, MCP, config, or DB rows, use
    /// [`ToolName::try_new`] and propagate the error.
    pub fn new(name: impl Into<String>) -> Self {
        let s = name.into();
        assert!(
            Self::is_valid(&s),
            "Invalid ToolName: '{s}' — must be non-empty, contain only alphanumeric/_/./:, and not start/end with '.' or ':'"
        );
        Self(s)
    }

    /// Construct a new `ToolName` from a string, returning an error on invalid input.
    ///
    /// Use this for all input that crosses a trust boundary:
    /// - IPC tool names (`HostRegistry::call_tool`)
    /// - MCP server tool names
    /// - Names loaded from the config file or env vars
    /// - Names loaded from the DB (`tool_*` rows)
    /// - Any other source that does not come from a string literal
    ///   in this crate's own source.
    pub fn try_new(name: impl Into<String>) -> Result<Self, String> {
        let s = name.into();
        if Self::is_valid(&s) {
            Ok(Self(s))
        } else {
            Err(format!(
                "Invalid ToolName: '{s}' — must be non-empty, contain only alphanumeric/_/./:, and not start/end with '.' or ':'"
            ))
        }
    }

    /// The namespace portion of the name (`"filesystem.read"` -> `"filesystem"`),
    /// or `None` for non-namespaced tools.
    #[must_use]
    pub fn namespace(&self) -> Option<&str> {
        self.0.split_once('.').map(|(ns, _)| ns)
    }

    /// The action portion of the name (`"filesystem.read"` -> `"read"`).
    #[must_use]
    pub fn action(&self) -> &str {
        self.0.rsplit_once('.').map_or(&self.0, |(_, a)| a)
    }

    /// Returns the inner string as a borrowed `&str`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the `ToolName` and returns the inner `String`.
    /// Use when handing the name to a non-`ToolName` consumer
    /// (e.g. an IPC error payload, a log line, a DB row key).
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for ToolName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ToolName {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for ToolName {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

/// Semantic version. Used to invalidate the embedding cache only on
/// semver-meaningful changes (i.e. major version bump, or `version` field
/// of the spec).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ToolVersion {
    /// Major version.
    pub major: u32,
    /// Minor version.
    pub minor: u32,
    /// Patch version.
    pub patch: u32,
}

impl ToolVersion {
    /// Construct a new version.
    #[must_use]
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl Default for ToolVersion {
    fn default() -> Self {
        Self::new(1, 0, 0)
    }
}

impl std::fmt::Display for ToolVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Structured keyword bag, replacing the legacy flat `Vec<String>`.
///
/// Each tier is weighted differently during Tool RAG scoring (see
/// `FieldWeights` in `ene-tool-host::rag`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KeywordSet {
    /// High-weight terms. Default weight `1.0`.
    #[serde(default)]
    pub primary: Vec<String>,
    /// Mid-weight terms. Default weight `0.6`.
    #[serde(default)]
    pub secondary: Vec<String>,
    /// Domain tags (language, framework, platform). Default weight `0.3`.
    /// Useful for hard filtering by language or runtime.
    #[serde(default)]
    pub domain: Vec<String>,
    /// Negative terms — when present in the query, *penalize* this tool.
    /// Default weight `-0.5` (soft penalty).
    #[serde(default)]
    pub negative: Vec<String>,
}

impl KeywordSet {
    /// Build a `KeywordSet` with only `primary` keywords.
    pub fn primary_only(primary: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            primary: primary.into_iter().map(Into::into).collect(),
            ..Default::default()
        }
    }

    /// Build a `KeywordSet` with primary + secondary keywords.
    pub fn with_secondary(
        primary: impl IntoIterator<Item = impl Into<String>>,
        secondary: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            primary: primary.into_iter().map(Into::into).collect(),
            secondary: secondary.into_iter().map(Into::into).collect(),
            ..Default::default()
        }
    }

    /// Returns true if no keywords are present.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.primary.is_empty()
            && self.secondary.is_empty()
            && self.domain.is_empty()
            && self.negative.is_empty()
    }
}

/// What kind of side effect the tool has. Used for safety analysis and
/// for filtering tools that need sandboxing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[derive(Default)]
pub enum SideEffects {
    /// Read-only: no observable side effects.
    #[default]
    ReadOnly,
    /// File-system interaction. `mutates: true` if it writes.
    FileSystem {
        /// Whether the operation mutates the file system.
        mutates: bool,
    },
    /// Network access. `external: true` if it goes outside the loopback.
    Network {
        /// Whether the network call goes to external services.
        external: bool,
    },
    /// System-level access (process spawn, signals, etc.).
    System {
        /// Whether privileged operations are involved.
        privileged: bool,
    },
    /// Browser automation.
    Browser {
        /// Whether the operation mutates the DOM.
        mutates_dom: bool,
    },
    /// Destructive: data loss is possible and rollback is not guaranteed.
    Destructive,
    /// Idempotent: calling twice with the same args yields the same effect.
    Idempotent,
}

/// One example of the tool in use, shown to the LLM and used for
/// example-based RAG embedding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolExample {
    /// Short description of the scenario.
    pub description: String,
    /// JSON-encoded input arguments.
    pub input: serde_json::Value,
    /// Optional sample output. When present, the example is treated as
    /// high-confidence and is weighted higher in the RAG index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

/// Tool category — used for classification and RAG filtering.
///
/// Mega-tools (`Filesystem`, `Shell`, `Browser`, `App`) carry additional
/// per-action specs at the IPC level via `IpcResponse::ActionSpecs`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    /// File-system tools (read, write, edit, delete, glob, grep, patch).
    Filesystem,
    /// Shell execution tools.
    Shell,
    /// Browser automation tools.
    Browser,
    /// GUI automation tools.
    App,
    /// Web search tools.
    WebSearch,
    /// Web fetch tools (URL → markdown).
    WebFetch,
    /// Utility tools (question, todo, time, system info).
    Utility,
    /// Long-term memory operations.
    Memory,
    /// Local search / RAG over user documents.
    Search,
    /// Meta-tools (e.g. self-introspection, tool selection).
    Meta,
}

impl ToolCategory {
    /// Human-readable label used in the embedding text for this category.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Filesystem => "filesystem_tools",
            Self::Shell => "shell_tools",
            Self::Browser => "browser_tools",
            Self::App => "app_tools",
            Self::WebSearch => "websearch_tools",
            Self::WebFetch => "webfetch_tools",
            Self::Utility => "utility_tools",
            Self::Memory => "memory_tools",
            Self::Search => "search_tools",
            Self::Meta => "meta_tools",
        }
    }
}

/// Action = one capability within a mega-tool. Always embedded separately
/// for Tool RAG and surfaced in `IpcResponse::ActionSpecs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActionSpec {
    /// Action name (e.g. "read", "write"). Namespaced via the parent
    /// `ToolSpec::name`.
    pub name: String,
    /// Short human-friendly name (e.g. "Read File").
    pub display_name: String,
    /// One-line summary used for embedding.
    pub summary: String,
    /// Full markdown description.
    pub description: String,
    /// Structured keywords.
    pub keywords: KeywordSet,
    /// Example invocations.
    #[serde(default)]
    pub examples: Vec<ToolExample>,
    /// Caveats the LLM should be aware of.
    #[serde(default)]
    pub caveats: Vec<String>,
    /// Side effects specific to this action.
    pub side_effects: SideEffects,
    /// Preconditions that must hold before invocation.
    #[serde(default)]
    pub preconditions: Vec<String>,
}

impl ActionSpec {
    /// Build a minimal `ActionSpec` (used by simple non-mega tools).
    pub fn minimal(name: impl Into<String>, summary: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            display_name: name.clone(),
            summary: summary.into(),
            description: String::new(),
            keywords: KeywordSet::default(),
            examples: Vec::new(),
            caveats: Vec::new(),
            side_effects: SideEffects::ReadOnly,
            preconditions: Vec::new(),
            name,
        }
    }
}

/// The structured, LLM-facing tool specification.
///
/// Per API v2 / #135 this is limited to the fields the model sees:
/// `name`, `description`, and `parameters` (JSON Schema). Host-side RAG
/// indexes from description (+ parameters text); a separate
/// `ToolRagProfile` is deferred.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolSpec {
    /// Unique, validated tool name.
    pub name: ToolName,
    /// Full description shown to the LLM (and used for RAG embedding).
    pub description: String,
    /// JSON Schema (auto-derived by `schemars` from the args struct).
    pub parameters: serde_json::Value,
}

impl ToolSpec {
    /// Construct an LLM-facing tool spec.
    #[must_use]
    pub fn new(
        name: impl Into<ToolName>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }

    /// Build a text representation of this spec for RAG embedding. The
    /// `field` parameter controls which fields are included.
    #[must_use]
    pub fn embedding_text(&self, field: EmbeddingField) -> String {
        match field {
            EmbeddingField::Summary => {
                let first_line = self
                    .description
                    .lines()
                    .next()
                    .unwrap_or(self.description.as_str())
                    .trim();
                if first_line.is_empty() {
                    self.name.as_str().to_string()
                } else {
                    format!("{}: {first_line}", self.name.as_str())
                }
            }
            EmbeddingField::Description => {
                let mut out = format!("{}\n{}", self.name.as_str(), self.description);
                let params = self.parameters.to_string();
                if params != "null" && params != "{}" {
                    out.push('\n');
                    out.push_str(&params);
                }
                out
            }
        }
    }
}

/// Which subset of a `ToolSpec`'s text content to embed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingField {
    /// Embed only `name + first line of description` (highest-signal).
    Summary,
    /// Embed the full description + parameters JSON (ranking refinement).
    Description,
}

impl EmbeddingField {
    /// Returns the string label persisted in the index.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Description => "description",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_name_namespaces() {
        let n = ToolName::new("filesystem.read");
        assert_eq!(n.namespace(), Some("filesystem"));
        assert_eq!(n.action(), "read");
        assert_eq!(n.as_str(), "filesystem.read");
    }

    #[test]
    fn tool_name_individual() {
        let n = ToolName::new("get_current_time");
        assert_eq!(n.namespace(), None);
        assert_eq!(n.action(), "get_current_time");
    }

    #[test]
    fn tool_name_serde_roundtrip() {
        let n = ToolName::new("utility.todo_add");
        let json = serde_json::to_string(&n).unwrap();
        assert_eq!(json, "\"utility.todo_add\"");
        let de: ToolName = serde_json::from_str(&json).unwrap();
        assert_eq!(de, n);
    }

    #[test]
    fn tool_version_display() {
        let v = ToolVersion::new(1, 2, 3);
        assert_eq!(v.to_string(), "1.2.3");
    }

    #[test]
    fn keyword_set_primary_only() {
        let k = KeywordSet::primary_only(["read", "open"]);
        assert_eq!(k.primary, vec!["read", "open"]);
        assert!(k.secondary.is_empty());
        assert!(!k.is_empty());
    }

    #[test]
    fn side_effects_default_is_read_only() {
        assert_eq!(SideEffects::default(), SideEffects::ReadOnly);
    }

    #[test]
    fn side_effects_serde_tag() {
        let s = SideEffects::FileSystem { mutates: true };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"kind\":\"file_system\""));
        let de: SideEffects = serde_json::from_str(&json).unwrap();
        assert_eq!(de, s);
    }

    #[test]
    fn tool_category_label() {
        assert_eq!(ToolCategory::Filesystem.label(), "filesystem_tools");
        assert_eq!(ToolCategory::WebFetch.label(), "webfetch_tools");
        assert_eq!(ToolCategory::Memory.label(), "memory_tools");
    }

    #[test]
    fn tool_spec_embedding_text_summary() {
        let s = ToolSpec::new(
            ToolName::new("filesystem.read"),
            "Read a file\nReads a file from disk and returns its contents.",
            serde_json::json!({}),
        );
        let t = s.embedding_text(EmbeddingField::Summary);
        assert_eq!(t, "filesystem.read: Read a file");
    }

    #[test]
    fn tool_spec_embedding_text_description_and_summary() {
        let s = ToolSpec::new(
            ToolName::new("filesystem.read"),
            "Read a file",
            serde_json::json!({"type": "object"}),
        );
        let desc = s.embedding_text(EmbeddingField::Description);
        assert!(desc.contains("filesystem.read"));
        assert!(desc.contains("Read a file"));
        let sum = s.embedding_text(EmbeddingField::Summary);
        assert_eq!(sum, "filesystem.read: Read a file");
    }

    #[test]
    fn action_spec_minimal() {
        let a = ActionSpec::minimal("read", "Read a file");
        assert_eq!(a.name, "read");
        assert_eq!(a.summary, "Read a file");
        assert_eq!(a.side_effects, SideEffects::ReadOnly);
    }

    #[test]
    fn tool_name_validation_accepts_valid() {
        assert!(ToolName::is_valid("filesystem.read"));
        assert!(ToolName::is_valid("utility.get_current_time"));
        assert!(ToolName::is_valid("a"));
        assert!(ToolName::is_valid("a.b.c"));
        assert!(ToolName::is_valid("0:filesystem.read"));
        assert!(ToolName::is_valid("prefix:name"));
    }

    #[test]
    fn tool_name_validation_rejects_invalid() {
        assert!(!ToolName::is_valid(""));
        assert!(!ToolName::is_valid(".leading_dot"));
        assert!(!ToolName::is_valid("trailing_dot."));
        assert!(!ToolName::is_valid(":leading_colon"));
        assert!(!ToolName::is_valid("trailing_colon:"));
        assert!(!ToolName::is_valid("has space"));
        assert!(!ToolName::is_valid("has-dash"));
        assert!(!ToolName::is_valid("has/slash"));
    }

    #[test]
    fn tool_name_try_new_ok() {
        assert!(ToolName::try_new("filesystem.read").is_ok());
    }

    #[test]
    fn tool_name_try_new_err() {
        assert!(ToolName::try_new("").is_err());
        assert!(ToolName::try_new("bad name").is_err());
    }

    #[test]
    #[should_panic]
    fn tool_name_new_panics_on_empty() {
        let _ = ToolName::new("");
    }

    #[test]
    #[should_panic]
    fn tool_name_new_panics_on_invalid() {
        let _ = ToolName::new("has space");
    }

    #[test]
    #[should_panic]
    fn tool_name_from_str_panics_on_invalid() {
        let _ = ToolName::from("bad/name");
    }

    #[test]
    #[should_panic]
    fn tool_name_from_string_panics_on_invalid() {
        let _ = ToolName::from(String::from("bad/name"));
    }
}
