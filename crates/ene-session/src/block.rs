use serde::{Deserialize, Serialize};

/// Content block stored in message payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    ImageRef {
        artifact_id: String,
    },
    ToolRef {
        call_id: String,
    },
    InnerTag {
        aspect: InnerAspect,
        text: String,
    },
    /// Projection placeholder after a `redaction` event.
    Redacted {
        reason: String,
    },
}

impl Block {
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    #[must_use]
    pub fn redacted(reason: impl Into<String>) -> Self {
        Self::Redacted {
            reason: reason.into(),
        }
    }

    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } | Self::InnerTag { text, .. } => Some(text.as_str()),
            Self::ImageRef { .. } | Self::ToolRef { .. } | Self::Redacted { .. } => None,
        }
    }
}

/// Inner-channel aspect tags carried inside blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InnerAspect {
    Thought,
    Emotion,
    ActionIntent,
}

impl InnerAspect {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Thought => "thought",
            Self::Emotion => "emotion",
            Self::ActionIntent => "action_intent",
        }
    }
}
