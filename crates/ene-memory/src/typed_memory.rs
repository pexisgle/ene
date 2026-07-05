use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing;

/// The kind of a typed memory item.
///
/// Each memory has exactly one kind, which drives retrieval strategy
/// and lifecycle behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// Specific events or conversations (what happened when).
    Episodic,
    /// Facts and general knowledge (what is true).
    Semantic,
    /// Information about the user's identity, background, and traits.
    UserProfile,
    /// Information about the relationship between the user and the companion.
    Relationship,
    /// Memories with strong emotional salience.
    Affective,
    /// Promises, tasks, and obligations the companion has made.
    Commitment,
    /// User likes, dislikes, and preferences.
    Preference,
    /// How-to knowledge and procedural instructions.
    Procedure,
    /// Self-reflections by the companion about past interactions.
    Reflection,
}

impl MemoryKind {
    /// Returns the snake_case string representation for storage/DB use.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Episodic => "episodic",
            Self::Semantic => "semantic",
            Self::UserProfile => "user_profile",
            Self::Relationship => "relationship",
            Self::Affective => "affective",
            Self::Commitment => "commitment",
            Self::Preference => "preference",
            Self::Procedure => "procedure",
            Self::Reflection => "reflection",
        }
    }

    /// Parse from a snake_case string, logging a warning on unrecognized values.
    pub(crate) fn from_db_str(s: &str) -> Self {
        match s {
            "episodic" => Self::Episodic,
            "semantic" => Self::Semantic,
            "user_profile" => Self::UserProfile,
            "relationship" => Self::Relationship,
            "affective" => Self::Affective,
            "commitment" => Self::Commitment,
            "preference" => Self::Preference,
            "procedure" => Self::Procedure,
            "reflection" => Self::Reflection,
            other => {
                tracing::warn!(
                    other,
                    "unrecognized MemoryKind in DB, falling back to Semantic"
                );
                Self::Semantic
            }
        }
    }
}

/// The lifecycle status of a memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    /// Currently relevant and retrievable.
    Active,
    /// Decayed but still retrievable with lower priority.
    Faded,
    /// No longer shown in normal recall but preserved.
    Archived,
    /// User has disputed or corrected this memory.
    Disputed,
    /// Replaced by a newer, conflicting memory.
    Superseded,
    /// Explicitly deleted by the user.
    UserDeleted,
}

impl MemoryStatus {
    /// Returns the snake_case string representation for storage/DB use.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Faded => "faded",
            Self::Archived => "archived",
            Self::Disputed => "disputed",
            Self::Superseded => "superseded",
            Self::UserDeleted => "user_deleted",
        }
    }

    /// Parse from a snake_case string, logging a warning on unrecognized values.
    pub(crate) fn from_db_str(s: &str) -> Self {
        match s {
            "active" => Self::Active,
            "faded" => Self::Faded,
            "archived" => Self::Archived,
            "disputed" => Self::Disputed,
            "superseded" => Self::Superseded,
            "user_deleted" => Self::UserDeleted,
            other => {
                tracing::warn!(
                    other,
                    "unrecognized MemoryStatus in DB, falling back to Active"
                );
                Self::Active
            }
        }
    }
}

/// The scope or ownership of a memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    /// Memory associated with the character/companion.
    Character,
    /// Memory associated with the user.
    User,
    /// Memory shared between character and user.
    Shared,
}

impl MemoryScope {
    /// Returns the snake_case string representation for storage/DB use.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Character => "character",
            Self::User => "user",
            Self::Shared => "shared",
        }
    }

    /// Parse from a snake_case string, logging a warning on unrecognized values.
    pub(crate) fn from_db_str(s: &str) -> Self {
        match s {
            "character" => Self::Character,
            "user" => Self::User,
            "shared" => Self::Shared,
            other => {
                tracing::warn!(
                    other,
                    "unrecognized MemoryScope in DB, falling back to Character"
                );
                Self::Character
            }
        }
    }
}

/// The provenance of a memory (how it was created).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    /// Extracted directly from conversation turns.
    Conversation,
    /// Explicitly stated by the user.
    UserStated,
    /// Extracted by an LLM-based extraction pipeline.
    LlmExtracted,
    /// Inferred by deterministic rules.
    Inferred,
    /// Imported from external data.
    Imported,
    /// Derived from CCv3 character card data.
    Ccv3,
}

impl MemorySource {
    /// Returns the snake_case string representation for storage/DB use.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::UserStated => "user_stated",
            Self::LlmExtracted => "llm_extracted",
            Self::Inferred => "inferred",
            Self::Imported => "imported",
            Self::Ccv3 => "ccv3",
        }
    }

    /// Parse from a snake_case string, logging a warning on unrecognized values.
    pub(crate) fn from_db_str(s: &str) -> Self {
        match s {
            "conversation" => Self::Conversation,
            "user_stated" => Self::UserStated,
            "llm_extracted" => Self::LlmExtracted,
            "inferred" => Self::Inferred,
            "imported" => Self::Imported,
            "ccv3" => Self::Ccv3,
            other => {
                tracing::warn!(
                    other,
                    "unrecognized MemorySource in DB, falling back to Conversation"
                );
                Self::Conversation
            }
        }
    }
}

/// A normalised confidence score for a memory (0.0–1.0).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryConfidence(f32);

impl MemoryConfidence {
    /// The maximum possible confidence.
    pub const MAX: Self = Self(1.0);

    /// Create a new confidence value, clamping to [0.0, 1.0].
    #[must_use]
    pub fn new(raw: f32) -> Self {
        Self(raw.clamp(0.0, 1.0))
    }

    /// Return the inner `f32`.
    #[must_use]
    pub fn get(self) -> f32 {
        self.0
    }
}

impl Default for MemoryConfidence {
    fn default() -> Self {
        Self(0.5)
    }
}

/// A normalised salience / importance score for a memory (0.0–1.0).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemorySalience(f32);

impl MemorySalience {
    /// The maximum possible salience.
    pub const MAX: Self = Self(1.0);

    /// Create a new salience value, clamping to [0.0, 1.0].
    #[must_use]
    pub fn new(raw: f32) -> Self {
        Self(raw.clamp(0.0, 1.0))
    }

    /// Return the inner `f32`.
    #[must_use]
    pub fn get(self) -> f32 {
        self.0
    }
}

impl Default for MemorySalience {
    fn default() -> Self {
        Self(0.5)
    }
}

/// Affect / emotion annotation attached to a memory item.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AffectAnnotation {
    /// Pleasure–displeasure (-1.0..=1.0).
    pub valence: f32,
    /// Excitement–calm (-1.0..=1.0).
    pub arousal: f32,
}

impl Default for AffectAnnotation {
    fn default() -> Self {
        Self {
            valence: 0.0,
            arousal: 0.0,
        }
    }
}

/// A typed memory item.
///
/// The central unit of the typed memory store. Each item has a
/// [`MemoryKind`], [`MemoryStatus`], [`MemorySource`], and
/// associated confidence and salience scores.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryItem {
    /// Primary key (`None` until persisted).
    pub id: Option<i64>,
    /// Ownership scope.
    pub scope: MemoryScope,
    /// Character identifier.
    pub character_id: String,
    /// User identifier (may be empty).
    pub user_id: String,
    /// Memory kind.
    pub kind: MemoryKind,
    /// Short title or label.
    pub title: String,
    /// Full memory content.
    pub content: String,
    /// Provenance of the memory.
    pub source: MemorySource,
    /// Optional reference to the source (e.g. session_id or turn sequence).
    pub source_ref: Option<String>,
    /// Confidence score.
    pub confidence: MemoryConfidence,
    /// Salience / importance score.
    pub salience: MemorySalience,
    /// Emotional annotation (valence + arousal).
    pub affect: AffectAnnotation,
    /// Relationship impact score (-1.0..=1.0).
    pub relationship_impact: f32,
    /// Number of times this memory has been accessed.
    pub access_count: i64,
    /// Timestamp of the last access.
    pub last_accessed_at: Option<DateTime<Utc>>,
    /// When the memory was created.
    pub created_at: DateTime<Utc>,
    /// When the memory was last updated.
    pub updated_at: DateTime<Utc>,
    /// Start of validity period.
    pub valid_from: Option<DateTime<Utc>>,
    /// End of validity period.
    pub valid_until: Option<DateTime<Utc>>,
    /// Lifecycle status.
    pub status: MemoryStatus,
    /// ID of the memory this one supersedes (predecessor link).
    ///
    /// Set on replacement rows; `None` when this memory is not a superseding
    /// successor. Superseded rows do not populate this field.
    pub supersedes_id: Option<i64>,
    /// User-pinned memories are exempt from natural decay (#76).
    #[serde(default)]
    pub pinned: bool,
    /// When the memory entered `faded` status (archive-decay anchor).
    #[serde(default)]
    pub faded_at: Option<DateTime<Utc>>,
}

/// Payload for creating a new memory item (fields set by the store are omitted).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewMemoryItem {
    /// Ownership scope.
    pub scope: MemoryScope,
    /// Character identifier.
    pub character_id: String,
    /// User identifier (may be empty).
    pub user_id: String,
    /// Memory kind.
    pub kind: MemoryKind,
    /// Short title or label.
    pub title: String,
    /// Full memory content.
    pub content: String,
    /// Provenance of the memory.
    pub source: MemorySource,
    /// Optional reference to the source.
    pub source_ref: Option<String>,
    /// Confidence score.
    pub confidence: MemoryConfidence,
    /// Salience / importance score.
    pub salience: MemorySalience,
    /// Emotional annotation.
    pub affect: AffectAnnotation,
    /// Relationship impact score.
    pub relationship_impact: f32,
    /// Start of validity period.
    pub valid_from: Option<DateTime<Utc>>,
    /// End of validity period.
    pub valid_until: Option<DateTime<Utc>>,
    /// Initial lifecycle status.
    pub status: MemoryStatus,
    /// ID of the memory this one supersedes.
    pub supersedes_id: Option<i64>,
    /// Pin state — pinned memories skip natural decay.
    #[serde(default)]
    pub pinned: bool,
    /// Optional created timestamp (defaults to now on insert).
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
}

/// Recall source that surfaced a memory candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCandidateSource {
    /// Vector embedding similarity search.
    Vector,
    /// Lexical token overlap with query text.
    Lexical,
    /// Recent / active memory listing.
    Recent,
    /// Active companion commitment ledger.
    Commitment,
}

/// Weights for hybrid memory search scoring components.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HybridSearchWeights {
    /// Weight for vector cosine similarity.
    pub vector: f32,
    /// Weight for lexical token overlap.
    pub lexical: f32,
    /// Weight for recency decay score.
    pub recency: f32,
    /// Weight for memory salience.
    pub salience: f32,
    /// Weight for memory confidence.
    pub confidence: f32,
    /// Weight for emotional/affect match.
    pub emotional_match: f32,
    /// Weight for relationship impact.
    pub relationship: f32,
    /// Weight for prior access boost.
    pub access_boost: f32,
}

impl Default for HybridSearchWeights {
    fn default() -> Self {
        Self {
            vector: 0.40,
            lexical: 0.15,
            recency: 0.10,
            salience: 0.15,
            confidence: 0.05,
            emotional_match: 0.05,
            relationship: 0.05,
            access_boost: 0.05,
        }
    }
}

/// Options for hybrid typed-memory search.
#[derive(Debug, Clone)]
pub struct MemorySearchOptions<'a> {
    /// Natural-language query for lexical scoring.
    pub query_text: &'a str,
    /// Query embedding for vector similarity.
    pub query_embedding: &'a [f32],
    /// Character scope.
    pub character_id: &'a str,
    /// Optional user scope filter.
    pub user_id: Option<&'a str>,
    /// Embedding model name for vector index lookup.
    pub model_name: &'a str,
    /// Maximum results to return.
    pub limit: usize,
    /// Minimum vector similarity for vector-sourced candidates.
    pub similarity_threshold: f32,
    /// Upper bound on candidates gathered before scoring.
    pub candidate_pool_size: usize,
    /// Optional query affect for emotional match scoring.
    pub query_affect: Option<AffectAnnotation>,
    /// Component weights for the hybrid formula.
    pub weights: HybridSearchWeights,
    /// Half-life in days for recency decay.
    pub decay_half_life_days: f64,
    /// Reference time for recency and expiry checks.
    pub now: DateTime<Utc>,
    /// Minimum hybrid total score required to return a result.
    pub min_score: f32,
    /// Boost applied when a candidate is surfaced via an active commitment.
    pub commitment_boost: f32,
    /// Maximum number of pure-recent fallback candidates to gather.
    pub recent_fallback_limit: usize,
}

/// Explainable score breakdown for a recalled memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryScoreBreakdown {
    /// Raw vector cosine similarity.
    pub vector_similarity: f32,
    /// Lexical overlap score.
    pub lexical_score: f32,
    /// Recency decay score.
    pub recency_score: f32,
    /// Memory salience value.
    pub salience: f32,
    /// Memory confidence value used in the weighted score.
    pub confidence: f32,
    /// Affect match score.
    pub emotional_match: f32,
    /// Normalized relationship impact score.
    pub relationship: f32,
    /// Access-frequency boost.
    pub access_boost: f32,
    /// Penalty for disputed status.
    pub contradiction_penalty: f32,
    /// Penalty for faded or expired memories.
    pub stale_penalty: f32,
    /// Boost for active commitment candidates.
    pub commitment_boost: f32,
    /// Final hybrid total score.
    pub total: f32,
}

/// A typed memory with hybrid score breakdown and recall sources.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoredMemory {
    /// The recalled memory item.
    pub item: MemoryItem,
    /// Explainable score components.
    pub breakdown: MemoryScoreBreakdown,
    /// Sources that contributed this candidate.
    pub sources: Vec<MemoryCandidateSource>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_kind_serde_roundtrip() {
        let kinds = [
            MemoryKind::Episodic,
            MemoryKind::Semantic,
            MemoryKind::UserProfile,
            MemoryKind::Relationship,
            MemoryKind::Affective,
            MemoryKind::Commitment,
            MemoryKind::Preference,
            MemoryKind::Procedure,
            MemoryKind::Reflection,
        ];
        for kind in kinds {
            let json = serde_json::to_string(&kind).unwrap();
            let back: MemoryKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, back, "roundtrip failed for {kind:?}");
        }
    }

    #[test]
    fn memory_status_serde_roundtrip() {
        let statuses = [
            MemoryStatus::Active,
            MemoryStatus::Faded,
            MemoryStatus::Archived,
            MemoryStatus::Disputed,
            MemoryStatus::Superseded,
            MemoryStatus::UserDeleted,
        ];
        for status in statuses {
            let json = serde_json::to_string(&status).unwrap();
            let back: MemoryStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, back, "roundtrip failed for {status:?}");
        }
    }

    #[test]
    fn memory_scope_serde_roundtrip() {
        let scopes = [
            MemoryScope::Character,
            MemoryScope::User,
            MemoryScope::Shared,
        ];
        for scope in scopes {
            let json = serde_json::to_string(&scope).unwrap();
            let back: MemoryScope = serde_json::from_str(&json).unwrap();
            assert_eq!(scope, back);
        }
    }

    #[test]
    fn memory_source_serde_roundtrip() {
        let sources = [
            MemorySource::Conversation,
            MemorySource::UserStated,
            MemorySource::LlmExtracted,
            MemorySource::Inferred,
            MemorySource::Imported,
            MemorySource::Ccv3,
        ];
        for source in sources {
            let json = serde_json::to_string(&source).unwrap();
            let back: MemorySource = serde_json::from_str(&json).unwrap();
            assert_eq!(source, back);
        }
    }

    #[test]
    fn memory_confidence_clamps() {
        assert!((MemoryConfidence::new(1.5).get() - 1.0).abs() < f32::EPSILON);
        assert!((MemoryConfidence::new(-0.3).get() - 0.0).abs() < f32::EPSILON);
        assert!((MemoryConfidence::new(0.73).get() - 0.73).abs() < f32::EPSILON);
    }

    #[test]
    fn memory_salience_clamps() {
        assert!((MemorySalience::new(1.5).get() - 1.0).abs() < f32::EPSILON);
        assert!((MemorySalience::new(-0.3).get() - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn memory_item_serde_roundtrip() {
        let item = MemoryItem {
            id: Some(42),
            scope: MemoryScope::Character,
            character_id: "ene".into(),
            user_id: String::new(),
            kind: MemoryKind::Episodic,
            title: "Test memory".into(),
            content: "The user said hello".into(),
            source: MemorySource::Conversation,
            source_ref: Some("sess-1/turn-3".into()),
            confidence: MemoryConfidence::new(0.9),
            salience: MemorySalience::new(0.7),
            affect: AffectAnnotation {
                valence: 0.3,
                arousal: -0.1,
            },
            relationship_impact: 0.2,
            access_count: 5,
            last_accessed_at: Some(chrono::Utc::now()),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            faded_at: None,
        };
        let json = serde_json::to_string(&item).unwrap();
        let back: MemoryItem = serde_json::from_str(&json).unwrap();
        assert_eq!(item.id, back.id);
        assert_eq!(item.scope, back.scope);
        assert_eq!(item.kind, back.kind);
        assert_eq!(item.title, back.title);
        assert_eq!(item.content, back.content);
        assert_eq!(item.source, back.source);
        assert_eq!(item.status, back.status);
        assert!((item.confidence.get() - back.confidence.get()).abs() < f32::EPSILON);
        assert!((item.salience.get() - back.salience.get()).abs() < f32::EPSILON);
    }

    #[test]
    fn new_memory_item_serde_roundtrip() {
        let new_item = NewMemoryItem {
            scope: MemoryScope::User,
            character_id: "ene".into(),
            user_id: "user-1".into(),
            kind: MemoryKind::Preference,
            title: "Likes cats".into(),
            content: "The user mentioned they like cats".into(),
            source: MemorySource::LlmExtracted,
            source_ref: None,
            confidence: MemoryConfidence::new(0.8),
            salience: MemorySalience::new(0.6),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
        };
        let json = serde_json::to_string(&new_item).unwrap();
        let back: NewMemoryItem = serde_json::from_str(&json).unwrap();
        assert_eq!(new_item.kind, back.kind);
        assert_eq!(new_item.title, back.title);
    }
}
