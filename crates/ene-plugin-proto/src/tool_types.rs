//! Core tool types: `ToolName`, `ToolSpec`, `ToolRagProfile`, `KeywordSet`,
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
/// config, DB rows). The panicking constructor [`ToolName::new`] is
/// intended for compile-time-validated string literals only.
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

    /// Returns `true` if the name is non-empty, contains only valid
    /// characters, and does not start/end with or contain consecutive
    /// `.`/`:` separators.
    pub fn is_valid(name: &str) -> bool {
        if name.is_empty() {
            return false;
        }
        let bytes = name.as_bytes();
        if !bytes.iter().all(|&b| Self::valid_char(b)) {
            return false;
        }
        let Some(&first) = bytes.first() else {
            return false;
        };
        let Some(&last) = bytes.last() else {
            return false;
        };
        if matches!(first, b'.' | b':') || matches!(last, b'.' | b':') {
            return false;
        }
        // Reject consecutive separator characters (e.g. "a..b", "a::b", "a.:b").
        !bytes
            .windows(2)
            .any(|w| matches!(w, [b'.' | b':', b'.' | b':']))
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
            "Invalid ToolName: '{s}' — must be non-empty, contain only alphanumeric/_/./:, \
             not start/end with '.' or ':', and not contain consecutive '.'/':' separators"
        );
        Self(s)
    }

    /// Construct a new `ToolName` from a string, returning an error on invalid input.
    ///
    /// Use this for all input that crosses a trust boundary:
    /// - IPC tool names (the plugin dispatch loop's tool-call path)
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
                "Invalid ToolName: '{s}' — must be non-empty, contain only alphanumeric/_/./:, \
                 not start/end with '.' or ':', and not contain consecutive '.'/':' separators"
            ))
        }
    }

    /// The namespace portion of the name (`"filesystem.read"` -> `"filesystem"`),
    /// or `None` for non-namespaced tools.
    pub fn namespace(&self) -> Option<&str> {
        self.0.split_once('.').map(|(ns, _)| ns)
    }

    /// The action portion of the name (`"filesystem.read"` -> `"read"`).
    pub fn action(&self) -> &str {
        self.0.rsplit_once('.').map_or(&self.0, |(_, a)| a)
    }

    /// Borrow the fully-qualified name (e.g. `"filesystem.read"`).
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the `ToolName` and returns the inner `String`.
    /// Use when handing the name to a non-`ToolName` consumer
    /// (e.g. an IPC error payload, a log line, a DB row key).
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for ToolName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
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
/// Per-tool RAG metadata (keywords, examples, …) lives on
/// [`ToolRagProfile`] and is exchanged via `IpcResponse::RagProfiles`.
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

    /// Config-key form used by `tools.rag.per_category_limits` (e.g. `"Filesystem"`).
    pub const fn config_key(&self) -> &'static str {
        match self {
            Self::Filesystem => "Filesystem",
            Self::Shell => "Shell",
            Self::Browser => "Browser",
            Self::App => "App",
            Self::WebSearch => "WebSearch",
            Self::WebFetch => "WebFetch",
            Self::Utility => "Utility",
            Self::Memory => "Memory",
            Self::Search => "Search",
            Self::Meta => "Meta",
        }
    }
}

/// Host/RAG-only metadata for a callable tool (#137).
///
/// Never passed to the LLM tool list — exchanged via
/// `IpcResponse::RagProfiles` and consumed by `ene-rag`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolRagProfile {
    /// Full tool name (e.g. `"filesystem.read"`).
    pub name: ToolName,
    /// Short human-friendly name (e.g. `"Read File"`).
    pub display_name: String,
    /// One-line summary used for the primary embedding field.
    pub summary: String,
    /// Full markdown description used for description/capability embeddings.
    pub description: String,
    /// Category used for `per_category_limits` filtering.
    pub category: ToolCategory,
    /// Structured keywords (primary / secondary / domain / negative).
    pub keywords: KeywordSet,
    /// Example invocations — one embedding row each (`field_key = ex_N`).
    #[serde(default)]
    pub examples: Vec<ToolExample>,
    /// Caveats the RAG index may surface in description text.
    #[serde(default)]
    pub caveats: Vec<String>,
    /// Preconditions that must hold before invocation.
    #[serde(default)]
    pub preconditions: Vec<String>,
    /// Side effects specific to this tool.
    pub side_effects: SideEffects,
    /// Related tool names.
    #[serde(default)]
    pub related: Vec<ToolName>,
    /// Semantic version (invalidates embedding cache on meaningful bumps).
    pub version: ToolVersion,
}

impl ToolRagProfile {
    /// Build a minimal profile from an LLM-facing [`ToolSpec`] (e.g. MCP tools).
    pub fn from_tool_spec(spec: &ToolSpec) -> Self {
        let first_line = spec
            .description
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or(spec.description.as_str())
            .trim();
        let summary = if first_line.is_empty() {
            spec.name.as_str().to_string()
        } else {
            first_line.to_string()
        };
        Self {
            name: spec.name.clone(),
            display_name: spec.name.as_str().to_string(),
            summary,
            description: spec.description.clone(),
            category: ToolCategory::Utility,
            keywords: KeywordSet::default(),
            examples: Vec::new(),
            caveats: Vec::new(),
            preconditions: Vec::new(),
            side_effects: SideEffects::ReadOnly,
            related: Vec::new(),
            version: ToolVersion::default(),
        }
    }

    /// Build embedding text for a single index field.
    ///
    /// `parameters` is the LLM-facing JSON Schema from the matching
    /// [`ToolSpec`]; pass `None` when schema context is unavailable.
    ///
    /// For [`EmbeddingField::Example`], pass `example_index` to select
    /// which example row to embed; other fields ignore it.
    pub fn embedding_text(
        &self,
        field: EmbeddingField,
        parameters: Option<&serde_json::Value>,
        example_index: Option<usize>,
    ) -> String {
        match field {
            EmbeddingField::Summary => {
                if self.summary.trim().is_empty() {
                    self.name.as_str().to_string()
                } else {
                    format!("{}: {}", self.name.as_str(), self.summary.trim())
                }
            }
            EmbeddingField::Description => {
                let mut out = format!("{}\n{}", self.name.as_str(), self.description);
                let kw = format_keywords(&self.keywords);
                if !kw.is_empty() {
                    out.push('\n');
                    out.push_str(&kw);
                }
                if let Some(params) = parameters {
                    let summary = extract_schema_summary(params);
                    if !summary.is_empty() {
                        out.push('\n');
                        out.push_str(&summary);
                    }
                }
                out
            }
            EmbeddingField::Capability => {
                let mut parts = vec![self.category.label().to_string(), self.summary.clone()];
                if !self.keywords.primary.is_empty() {
                    parts.push(self.keywords.primary.join(", "));
                }
                format!("{}: {}", self.name.as_str(), parts.join(" | "))
            }
            EmbeddingField::Example => {
                let Some(i) = example_index else {
                    return String::new();
                };
                let Some(example) = self.examples.get(i) else {
                    return String::new();
                };
                format!("{}: {}", example.description, example.input)
            }
            EmbeddingField::Negative => {
                if self.keywords.negative.is_empty() {
                    String::new()
                } else {
                    format!(
                        "{} NOT: {}",
                        self.name.as_str(),
                        self.keywords.negative.join(", ")
                    )
                }
            }
        }
    }
}

/// Format a [`KeywordSet`] for inclusion in description embeddings.
fn format_keywords(keywords: &KeywordSet) -> String {
    let mut parts = Vec::new();
    if !keywords.primary.is_empty() {
        parts.push(format!("keywords: {}", keywords.primary.join(", ")));
    }
    if !keywords.secondary.is_empty() {
        parts.push(format!("also: {}", keywords.secondary.join(", ")));
    }
    if !keywords.domain.is_empty() {
        parts.push(format!("domain: {}", keywords.domain.join(", ")));
    }
    parts.join("; ")
}

/// Whether a tool operation can be rolled back (#178).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Reversibility {
    /// The operation can be reverted (e.g. file write/edit/delete/patch).
    Reversible,
    /// The operation cannot be reverted (e.g. shell execution, external sends).
    Irreversible,
}

/// Runtime-level undo metadata recorded per tool execution (#178).
///
/// The runtime records one entry per mutating tool call so `/undo` can
/// surface what a rollback affects and, for reversible operations, drive
/// the owning tool's undo action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UndoMetadata {
    /// Turn that performed the operation.
    pub turn_id: String,
    /// Namespaced tool name (e.g. `filesystem.write`).
    pub tool_name: String,
    /// Target resources affected (e.g. file paths), best-effort.
    #[serde(default)]
    pub target_resources: Vec<String>,
    /// Whether the operation can be rolled back.
    pub reversibility: Reversibility,
}

/// The structured, LLM-facing tool specification.
///
/// Per API v1 / #135 the model-facing surface is limited to `name`,
/// `description`, and `parameters` (JSON Schema). The remaining fields
/// (`background_capable`, `side_effects`) are host-execution metadata that
/// providers strip before serializing tools to a model API; they follow the
/// same precedent as `background_capable` (#196).
///
/// RAG metadata lives on [`ToolRagProfile`] (#137), not here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolSpec {
    /// Unique, validated tool name.
    pub name: ToolName,
    /// Full description shown to the LLM.
    pub description: String,
    /// JSON Schema (auto-derived by `schemars` from the args struct).
    pub parameters: serde_json::Value,
    /// Whether this tool supports deferred (background) execution (#196).
    ///
    /// When `true`, the host may issue a deferred call that returns a
    /// `task_id` immediately instead of blocking on the result; the
    /// completion is delivered later as a background-completed event.
    /// Defaults to `false` for ordinary synchronous tools.
    #[serde(default)]
    pub background_capable: bool,
    /// Declared side effects, used to decide whether a call may run in a
    /// bounded parallel batch (#400).
    ///
    /// `None` means "unknown" and is treated fail-closed: such tools are
    /// never parallelized. Only an explicit [`SideEffects::ReadOnly`] marks a
    /// tool eligible for concurrent execution. Older plugin binaries that omit
    /// the field on the wire deserialize to `None` via `#[serde(default)]`,
    /// so they keep the safe sequential behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side_effects: Option<SideEffects>,
}

impl ToolSpec {
    /// Construct an LLM-facing tool spec.
    ///
    /// `side_effects` defaults to `None` (unknown), which keeps the tool
    /// sequential under the parallel tool-call policy (#400). Use
    /// [`Self::side_effects`] to declare a concrete classification.
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
            background_capable: false,
            side_effects: None,
        }
    }

    /// Mark this spec as capable of deferred (background) execution (#196).
    #[must_use]
    pub const fn background_capable(mut self, capable: bool) -> Self {
        self.background_capable = capable;
        self
    }

    /// Declare the tool's side effects (#400).
    ///
    /// Only [`SideEffects::ReadOnly`] makes a tool eligible for bounded
    /// parallel execution; any other value (or leaving it unset) keeps the
    /// tool on the sequential path.
    #[must_use]
    pub const fn side_effects(mut self, side_effects: SideEffects) -> Self {
        self.side_effects = Some(side_effects);
        self
    }

    /// Whether this tool may run in a bounded parallel batch (#400).
    ///
    /// A tool is parallelizable only when it explicitly declares
    /// [`SideEffects::ReadOnly`] and is not background-capable (deferred tools
    /// already return immediately, so parallelizing them buys nothing and
    /// would reorder their acceptance events). Tools with unknown side effects
    /// (`None`) are treated fail-closed and stay sequential.
    #[must_use]
    pub const fn is_parallelizable(&self) -> bool {
        if self.background_capable {
            return false;
        }
        matches!(self.side_effects, Some(SideEffects::ReadOnly))
    }
}

/// Extract a human-readable summary from a JSON Schema, suitable for
/// embedding. Returns property names and their descriptions only —
/// the full schema is omitted to keep embeddings concise and
/// high-signal.
fn extract_schema_summary(params: &serde_json::Value) -> String {
    let Some(props) = params.get("properties").and_then(|p| p.as_object()) else {
        return String::new();
    };
    let mut parts = Vec::new();
    for (key, val) in props {
        let desc = val
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("");
        let typ = val.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if typ.is_empty() && desc.is_empty() {
            continue;
        }
        if desc.is_empty() {
            parts.push(format!("{key} ({typ})"));
        } else if typ.is_empty() {
            parts.push(format!("{key}: {desc}"));
        } else {
            parts.push(format!("{key} ({typ}): {desc}"));
        }
    }
    parts.join(". ")
}

/// Which subset of a [`ToolRagProfile`]'s text content to embed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingField {
    /// Embed `name + summary` (highest-signal).
    Summary,
    /// Embed description + keywords + optional schema summary.
    Description,
    /// Embed category label + summary + primary keywords.
    Capability,
    /// Embed one worked example (`field_key = ex_N`).
    Example,
    /// Embed negative keywords for RAG penalty scoring.
    Negative,
}

impl EmbeddingField {
    /// The index field name used in the tool RAG embedding store.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Description => "description",
            Self::Capability => "capability",
            Self::Example => "example",
            Self::Negative => "negative",
        }
    }

    /// Parse from the index field name. Returns `None` for
    /// unrecognized strings (e.g. legacy `"hyde"` rows).
    pub fn from_field_name(s: &str) -> Option<Self> {
        match s {
            "summary" => Some(Self::Summary),
            "description" => Some(Self::Description),
            "capability" => Some(Self::Capability),
            "example" => Some(Self::Example),
            "negative" => Some(Self::Negative),
            _ => None,
        }
    }
}

/// Structured result of a tool execution.
///
/// Replaces the opaque `String` return value with typed content that the
/// host can route appropriately (text to LLM, images to vision models, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ToolResult {
    /// Result content items (text, JSON, images, etc.).
    pub content: Vec<ToolContent>,
    /// Optional metadata (execution time, cache TTL, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// A single piece of content in a [`ToolResult`].
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolContent {
    /// Plain text content.
    Text {
        /// The text content.
        text: String,
    },
    /// Structured JSON content.
    Json(serde_json::Value),
    /// Base64-encoded image data with MIME type.
    Image {
        /// MIME type of the image (e.g. `"image/png"`, `"image/jpeg"`).
        mime_type: String,
        /// Base64-encoded image data.
        data_base64: String,
    },
}

impl ToolResult {
    /// Create a `ToolResult` with a single text content item.
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::Text { text: s.into() }],
            metadata: None,
        }
    }

    /// Extract text content suitable for passing to an LLM.
    ///
    /// Image content cannot be flattened into text, but silently dropping it
    /// would make the model believe a tool returned nothing. A visible
    /// placeholder preserves the fact that an image was produced so callers
    /// can route the structured [`ToolContent::Image`] to a vision model
    /// separately while the text projection stays coherent.
    pub fn text_for_llm(&self) -> String {
        self.content
            .iter()
            .map(|c| match c {
                ToolContent::Text { text } => text.clone(),
                ToolContent::Json(v) => v.to_string(),
                ToolContent::Image { .. } => "[image omitted]".to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n")
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
    fn tool_spec_defaults_to_unknown_side_effects() {
        // `ToolSpec::new` leaves side effects unknown (`None`), which is
        // fail-closed: the tool is not eligible for parallel execution (#400).
        let spec = ToolSpec::new(
            ToolName::new("mystery.tool"),
            "Unknown side effects",
            serde_json::json!({}),
        );
        assert_eq!(spec.side_effects, None);
        assert!(!spec.is_parallelizable());
    }

    #[test]
    fn tool_spec_is_parallelizable_only_for_explicit_read_only() {
        let read_only = ToolSpec::new(
            ToolName::new("weather.get"),
            "Read the weather",
            serde_json::json!({}),
        )
        .side_effects(SideEffects::ReadOnly);
        assert!(read_only.is_parallelizable());

        let mutating = ToolSpec::new(
            ToolName::new("filesystem.write"),
            "Write a file",
            serde_json::json!({}),
        )
        .side_effects(SideEffects::FileSystem { mutates: true });
        assert!(!mutating.is_parallelizable());

        // Background-capable tools are never parallelized even if read-only.
        let background = ToolSpec::new(
            ToolName::new("background.sleep"),
            "Sleep",
            serde_json::json!({}),
        )
        .side_effects(SideEffects::ReadOnly)
        .background_capable(true);
        assert!(!background.is_parallelizable());
    }

    #[test]
    fn tool_spec_side_effects_serde_is_optional() {
        // An older plugin binary that omits `side_effects` on the wire must
        // deserialize to `None` (fail-closed), not an error (#400).
        let json = r#"{"name":"legacy.tool","description":"d","parameters":{}}"#;
        let spec: ToolSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.side_effects, None);
        assert!(!spec.is_parallelizable());

        // When `None`, the field is omitted from serialization.
        let out = serde_json::to_string(&spec).unwrap();
        assert!(!out.contains("side_effects"));
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
    fn tool_category_config_key() {
        assert_eq!(ToolCategory::Filesystem.config_key(), "Filesystem");
        assert_eq!(ToolCategory::WebSearch.config_key(), "WebSearch");
    }

    fn sample_profile() -> ToolRagProfile {
        ToolRagProfile {
            name: ToolName::new("filesystem.read"),
            display_name: "Read File".into(),
            summary: "Read a file".into(),
            description: "Reads a file from disk and returns its contents.".into(),
            category: ToolCategory::Filesystem,
            keywords: KeywordSet {
                primary: vec!["read".into(), "open".into()],
                secondary: vec!["file".into()],
                domain: vec!["fs".into()],
                negative: vec!["write".into(), "delete".into()],
            },
            examples: vec![ToolExample {
                description: "Read /tmp/a".into(),
                input: serde_json::json!({"path": "/tmp/a"}),
                output: Some("hello".into()),
            }],
            caveats: Vec::new(),
            preconditions: Vec::new(),
            side_effects: SideEffects::ReadOnly,
            related: Vec::new(),
            version: ToolVersion::default(),
        }
    }

    #[test]
    fn profile_embedding_text_summary() {
        let p = sample_profile();
        assert_eq!(
            p.embedding_text(EmbeddingField::Summary, None, None),
            "filesystem.read: Read a file"
        );
    }

    #[test]
    fn profile_embedding_text_description_keywords_and_schema() {
        let p = sample_profile();
        let params = serde_json::json!({
            "properties": {
                "path": {"type": "string", "description": "File path to read"},
            }
        });
        let t = p.embedding_text(EmbeddingField::Description, Some(&params), None);
        assert!(t.contains("filesystem.read"));
        assert!(t.contains("Reads a file from disk"));
        assert!(t.contains("keywords: read, open"));
        assert!(t.contains("path (string): File path to read"));
        assert!(!t.contains("\"properties\""));
    }

    #[test]
    fn profile_embedding_text_capability() {
        let p = sample_profile();
        let t = p.embedding_text(EmbeddingField::Capability, None, None);
        assert!(t.contains("filesystem_tools"));
        assert!(t.contains("Read a file"));
        assert!(t.contains("read, open"));
    }

    #[test]
    fn profile_embedding_text_example() {
        let p = sample_profile();
        let t = p.embedding_text(EmbeddingField::Example, None, Some(0));
        assert!(t.contains("Read /tmp/a"));
        assert!(t.contains("/tmp/a"));
    }

    #[test]
    fn profile_embedding_text_negative() {
        let p = sample_profile();
        assert_eq!(
            p.embedding_text(EmbeddingField::Negative, None, None),
            "filesystem.read NOT: write, delete"
        );
    }

    #[test]
    fn profile_from_tool_spec() {
        let s = ToolSpec::new(
            ToolName::new("mcp.hello"),
            "Say hello\nLonger body.",
            serde_json::json!({}),
        );
        let p = ToolRagProfile::from_tool_spec(&s);
        assert_eq!(p.name.as_str(), "mcp.hello");
        assert_eq!(p.summary, "Say hello");
        assert_eq!(p.category, ToolCategory::Utility);
    }

    #[test]
    fn tool_result_text_for_llm_text_and_json() {
        let result = ToolResult {
            content: vec![
                ToolContent::Text {
                    text: "hello".into(),
                },
                ToolContent::Json(serde_json::json!({"k": "v"})),
            ],
            metadata: None,
        };
        assert_eq!(result.text_for_llm(), "hello\n{\"k\":\"v\"}");
    }

    #[test]
    fn tool_result_text_for_llm_image_not_dropped() {
        // Image content must surface as a visible placeholder rather than
        // being silently flattened to an empty string (WS9).
        let result = ToolResult {
            content: vec![
                ToolContent::Text {
                    text: "before".into(),
                },
                ToolContent::Image {
                    mime_type: "image/png".into(),
                    data_base64: "AAAA".into(),
                },
                ToolContent::Text {
                    text: "after".into(),
                },
            ],
            metadata: None,
        };
        let text = result.text_for_llm();
        assert!(
            text.contains("[image omitted]"),
            "image must not be dropped: {text}"
        );
        assert!(text.contains("before"));
        assert!(text.contains("after"));
    }

    #[test]
    fn tool_result_text_for_llm_image_only_not_empty() {
        let result = ToolResult {
            content: vec![ToolContent::Image {
                mime_type: "image/jpeg".into(),
                data_base64: "AAAA".into(),
            }],
            metadata: None,
        };
        assert!(!result.text_for_llm().is_empty());
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
    fn tool_name_validation_rejects_consecutive_separators() {
        assert!(!ToolName::is_valid("a..b"));
        assert!(!ToolName::is_valid("a::b"));
        assert!(!ToolName::is_valid("a.:b"));
        assert!(!ToolName::is_valid("a:.b"));
        assert!(!ToolName::is_valid("a...b"));
        assert!(!ToolName::is_valid("a:::b"));
        assert!(ToolName::try_new("a..b").is_err());
        assert!(ToolName::try_new("a::b").is_err());
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
    #[should_panic(expected = "Invalid ToolName:")]
    fn tool_name_new_panics_on_empty() {
        let _ = ToolName::new("");
    }

    #[test]
    #[should_panic(expected = "Invalid ToolName:")]
    fn tool_name_new_panics_on_invalid() {
        let _ = ToolName::new("has space");
    }
}
