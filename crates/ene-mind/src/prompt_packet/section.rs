//! Prompt section kinds and deterministic ordering.

/// Logical section of a [`super::PromptPacket`].
///
/// Variant order matches the deterministic render order in
/// [`super::PromptPacket::to_llm_messages`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PromptSectionKind {
    /// Platform / tool contract (desktop mascot framing).
    PlatformContract,
    /// Immutable character identity kernel.
    IdentityKernel,
    /// Behavior rules and card system instructions.
    BehaviorContract,
    /// Current affect / mood summary.
    CharacterState,
    /// Active rolling scene summary.
    SceneState,
    /// Lorebook and semantic character memory.
    SemanticContext,
    /// User profile, preferences, and relationship memories.
    UserProfile,
    /// Active companion commitments.
    ActiveCommitments,
    /// Episodic and other recalled memories.
    EpisodicMemories,
    /// `CCv3` style example anchors.
    StyleExamples,
    /// Note about a previously interrupted response.
    InterruptionNote,
    /// Expression PHI / output contract (required).
    OutputContract,
    /// Current user turn (required; rendered as a user message).
    UserInput,
}

impl PromptSectionKind {
    /// Whether this section must survive budget overflow.
    pub const fn is_required(self) -> bool {
        matches!(
            self,
            Self::PlatformContract | Self::IdentityKernel | Self::OutputContract | Self::UserInput
        )
    }

    /// Packing priority: higher values are kept longer when the prompt must be
    /// shrunk to fit the model's context window.
    ///
    /// Required sections report [`u8::MAX`] — packing never drops them
    /// regardless of this value (it checks [`Self::is_required`] directly).
    /// Among droppable sections, packing sheds the lowest priority first, so
    /// the advisory [`Self::InterruptionNote`] goes before the structural
    /// [`Self::BehaviorContract`]. Content producers — not packing — bound each
    /// section's size (recall result limit, lorebook token budget, identity
    /// kernel cap), so packing only decides *which* sections survive.
    pub const fn priority(self) -> u8 {
        match self {
            Self::PlatformContract
            | Self::IdentityKernel
            | Self::OutputContract
            | Self::UserInput => u8::MAX,
            Self::InterruptionNote => 10,
            Self::StyleExamples => 20,
            Self::EpisodicMemories => 30,
            Self::SemanticContext => 40,
            Self::UserProfile => 50,
            Self::ActiveCommitments => 60,
            Self::CharacterState => 70,
            Self::SceneState => 80,
            Self::BehaviorContract => 90,
        }
    }

    /// Default markdown heading for system-block sections.
    pub const fn heading(self) -> Option<&'static str> {
        match self {
            Self::PlatformContract
            | Self::IdentityKernel
            | Self::OutputContract
            | Self::UserInput => None,
            Self::BehaviorContract => Some("## Behavior Contract"),
            Self::CharacterState => Some("## Current Mood"),
            Self::SceneState => Some("## Current Scene"),
            Self::SemanticContext => Some("## Semantic Context"),
            Self::UserProfile => Some("## User Profile"),
            Self::ActiveCommitments => Some("## Active Commitments"),
            Self::EpisodicMemories => Some("## Relevant Episodic Memories"),
            Self::StyleExamples => Some("## Style Examples"),
            Self::InterruptionNote => Some("## Previous Response Interrupted"),
        }
    }

    /// All kinds in deterministic render order.
    pub const fn render_order() -> &'static [Self] {
        &[
            Self::PlatformContract,
            Self::IdentityKernel,
            Self::BehaviorContract,
            Self::CharacterState,
            Self::SceneState,
            Self::SemanticContext,
            Self::UserProfile,
            Self::ActiveCommitments,
            Self::EpisodicMemories,
            Self::StyleExamples,
            Self::InterruptionNote,
            Self::OutputContract,
            Self::UserInput,
        ]
    }
}

/// A single prompt section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSection {
    /// Section classification.
    pub kind: PromptSectionKind,
    /// Rendered section body (without heading).
    pub content: String,
    /// Localized instruction appended below the body; not counted in the
    /// section's token estimate so a budget drop discards it with the section.
    pub note: Option<String>,
    /// Whether the section is required and must not be dropped.
    pub required: bool,
    /// Authoritative count of discrete items this section carries (recalled
    /// memories, style examples, …). Set from source data at build time and
    /// zeroed when packing clears the section — never derived from `content`.
    pub(crate) item_count: usize,
}

impl PromptSection {
    /// Create a new section with `item_count = 0`.
    pub fn new(kind: PromptSectionKind, content: impl Into<String>) -> Self {
        let content = content.into();
        Self {
            kind,
            required: kind.is_required(),
            content,
            note: None,
            item_count: 0,
        }
    }

    /// Set the authoritative item count carried by this section.
    #[must_use]
    pub(crate) const fn with_item_count(mut self, item_count: usize) -> Self {
        self.item_count = item_count;
        self
    }

    /// Clears the body and resets `item_count` and `note` together so
    /// metadata cannot diverge from the rendered output.
    pub fn clear(&mut self) {
        self.content.clear();
        self.item_count = 0;
        self.note = None;
    }

    /// Render the section for the system block (heading + body + note).
    pub fn render_system_block(&self) -> Option<String> {
        if self.content.trim().is_empty() {
            return None;
        }
        let body = match &self.note {
            Some(note) if !note.trim().is_empty() => {
                format!("{}\n{}", self.content.trim(), note.trim())
            }
            _ => self.content.clone(),
        };
        match self.kind.heading() {
            Some(heading) => Some(format!("{heading}\n{}", body.trim())),
            None => Some(body),
        }
    }
}
