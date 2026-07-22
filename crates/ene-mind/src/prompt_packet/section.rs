//! Prompt section kinds and deterministic ordering (#87).

/// Logical section of a [`super::PromptPacket`].
///
/// Variant order matches the deterministic render order in
/// [`super::PromptPacket::to_llm_messages`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PromptSectionKind {
    /// Platform / tool contract (desktop mascot framing).
    PlatformContract,
    /// Immutable character identity kernel (#82).
    IdentityKernel,
    /// Behavior rules and card system instructions.
    BehaviorContract,
    /// Current affect / mood summary.
    CharacterState,
    /// Active rolling scene summary (#79).
    SceneState,
    /// Lorebook and semantic character memory.
    SemanticContext,
    /// User profile, preferences, and relationship memories.
    UserProfile,
    /// Active companion commitments.
    ActiveCommitments,
    /// Episodic and other recalled memories.
    EpisodicMemories,
    /// `CCv3` style example anchors (#84).
    StyleExamples,
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
            Self::OutputContract,
            Self::UserInput,
        ]
    }
}

/// A single prompt section with optional token budget metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSection {
    /// Section classification.
    pub kind: PromptSectionKind,
    /// Rendered section body (without heading).
    pub content: String,
    /// Whether the section is required and must not be dropped.
    pub required: bool,
    /// Token budget hint used during packing (#81).
    pub budget_tokens: usize,
}

impl PromptSection {
    /// Create a new section.
    pub fn new(kind: PromptSectionKind, content: impl Into<String>, budget_tokens: usize) -> Self {
        let content = content.into();
        Self {
            kind,
            required: kind.is_required(),
            content,
            budget_tokens,
        }
    }

    /// Render the section for the system block (heading + body).
    pub fn render_system_block(&self) -> Option<String> {
        if self.content.trim().is_empty() {
            return None;
        }
        match self.kind.heading() {
            Some(heading) => Some(format!("{heading}\n{}", self.content.trim())),
            None => Some(self.content.clone()),
        }
    }
}
