use crate::conversation_manager::{
    PendingSplitTask, SplitResult, SplitTaskInput, generate_session_id, poll_split_result,
};
use crate::error::SessionError;
use crate::special_token::split_text_and_special_tokens;
use async_openai::types::chat::Role;
use chrono::{DateTime, Utc};
use ene_config::{CharacterCardV3, EneConfig, ResolvedExpression, resolve_expressions};
use ene_embedding::EmbeddingProvider;
use ene_memory::{MemoryConfig, MemoryStore};
use std::collections::HashMap;
use std::sync::Arc;

/// Manages the conversation history with automatic trimming.
#[derive(Clone, Debug)]
pub struct ConversationHistory {
    /// List of (role, content) pairs.
    pub conversation_history: Vec<(Role, String)>,
    /// Maximum number of turns to retain.
    pub max_history_turns: usize,
}

impl ConversationHistory {
    fn trim_history(&mut self) {
        let max = self.max_history_turns * 2;
        if self.conversation_history.len() > max {
            let excess = self.conversation_history.len() - max;
            self.conversation_history.drain(0..excess);
        }
    }
}

/// Holds the current display buffer and partial token carry-over.
#[derive(Clone, Debug, Default)]
pub struct DisplayState {
    /// Accumulated display text for the current response.
    pub display_buffer: String,
    /// Partial token text carried from a previous chunk.
    pub token_carry: String,
}

/// Context for the memory subsystem within a session.
#[derive(Clone)]
pub struct MemoryContext {
    /// Optional memory store.
    pub memory_store: Option<Arc<MemoryStore>>,
    /// Optional embedding provider.
    pub embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    /// The current session ID.
    pub session_id: String,
    /// Timestamp when the session started.
    pub session_started_at: chrono::DateTime<chrono::Utc>,
    /// Embedding of the pending user input.
    pub pending_embedding: Option<Vec<f32>>,
}

/// Tracks session metadata like embedding and timing.
#[derive(Clone, Debug, Default)]
pub struct SessionState {
    /// The embedding of the last user input.
    pub last_input_embedding: Option<Vec<f32>>,
    /// Timestamp of the last received message.
    pub last_message_time: Option<DateTime<Utc>>,
    /// The current conversation turn count.
    pub current_turn_count: usize,
}

/// Central session container holding conversation history, display state, memory context,
/// and the loaded character card. Shared between the streaming engine and the CLI/GUI frontends.
#[derive(Clone)]
pub struct ConversationSession {
    /// Conversation history state.
    pub history: ConversationHistory,
    /// Display buffer state.
    pub display: DisplayState,
    /// Memory context state.
    pub memory: MemoryContext,
    /// Session metadata state.
    pub state: SessionState,
    /// The loaded character card.
    pub character_card: Option<CharacterCardV3>,
    /// The filesystem path to the current character card.
    pub current_card_path: String,
}

impl std::fmt::Debug for ConversationSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConversationSession")
            .field(
                "conversation_history_len",
                &self.history.conversation_history.len(),
            )
            .field("max_history_turns", &self.history.max_history_turns)
            .field("current_card_path", &self.current_card_path)
            .field("memory_enabled", &self.memory.memory_store.is_some())
            .field("session_id", &self.memory.session_id)
            .field("turn_count", &self.state.current_turn_count)
            .finish()
    }
}

impl Default for ConversationSession {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversationSession {
    /// Creates a new empty session with a fresh session ID and zero turn count.
    pub fn new() -> Self {
        Self {
            history: ConversationHistory {
                conversation_history: Vec::new(),
                max_history_turns: 20,
            },
            display: DisplayState {
                display_buffer: String::new(),
                token_carry: String::new(),
            },
            memory: MemoryContext {
                memory_store: None,
                embedding_provider: None,
                session_id: generate_session_id(),
                session_started_at: chrono::Utc::now(),
                pending_embedding: None,
            },
            state: SessionState {
                last_input_embedding: None,
                last_message_time: None,
                current_turn_count: 0,
            },
            character_card: None,
            current_card_path: String::new(),
        }
    }

    /// Attaches a memory store and embedding provider for long-term memory.
    pub fn init_memory(&mut self, store: Arc<MemoryStore>, embedder: Arc<dyn EmbeddingProvider>) {
        self.memory.memory_store = Some(store);
        self.memory.embedding_provider = Some(embedder);
    }

    /// Loads a character card from `path`, merges `character_settings.json` expressions,
    /// and clears the conversation history.
    pub fn load_card(
        &mut self,
        path: &str,
    ) -> Result<Vec<ResolvedExpression>, crate::error::SessionError> {
        if self.current_card_path == path && self.character_card.is_some() {
            if let Some(card) = &self.character_card {
                return Ok(resolve_expressions(card));
            }
        }

        let file_content =
            std::fs::read_to_string(path).map_err(crate::error::SessionError::CardReadError)?;

        let mut card = serde_json::from_str::<CharacterCardV3>(&file_content)
            .map_err(crate::error::SessionError::JsonError)?;

        // Merge expressions from character_settings.json (section-based with fallback)
        if let Some(parent) = std::path::Path::new(path).parent() {
            let folder = parent.file_name().unwrap_or_default().to_string_lossy();
            let settings_path = ene_config::character_settings_path(&folder);
            if let Ok(settings_content) = std::fs::read_to_string(&settings_path) {
                let per = serde_json::from_str::<HashMap<String, serde_json::Value>>(
                    &settings_content,
                )
                .ok()
                .and_then(|map| {
                    map.get("character_settings")
                        .cloned()
                        .and_then(|v| serde_json::from_value::<ene_config::CharacterPerConfig>(v).ok())
                })
                // Fallback: flat CharacterPerConfig
                .or_else(|| {
                    serde_json::from_str::<ene_config::CharacterPerConfig>(&settings_content).ok()
                });

                if let Some(per) = per {
                    if let Some(expr) = per.expressions {
                        card.data.extensions.insert("expressions".to_string(), expr);
                    }
                    if !per.default_motion.is_empty() {
                        let mut ene = serde_json::Map::new();
                        ene.insert(
                            "default_motion".to_string(),
                            serde_json::Value::String(per.default_motion),
                        );
                        card.data
                            .extensions
                            .insert("ene".to_string(), serde_json::Value::Object(ene));
                    }
                }
            }
        }

        self.character_card = Some(card);
        self.current_card_path = path.to_string();
        self.history.conversation_history.clear();

        Ok(resolve_expressions(self.character_card.as_ref().unwrap()))
    }

    /// Appends a user message and trims history if it exceeds `max_history_turns * 2`.
    pub fn add_user_message(&mut self, input: &str) {
        self.history
            .conversation_history
            .push((Role::User, input.to_string()));
        self.history.trim_history();
    }

    /// Appends an assistant message and trims history if it exceeds `max_history_turns * 2`.
    pub fn add_assistant_message(&mut self, text: &str) {
        self.history
            .conversation_history
            .push((Role::Assistant, text.to_string()));
        self.history.trim_history();
    }

    /// Processes a streaming text chunk, splitting it into text deltas and special tokens
    /// (e.g., `<|emo:happy|>`). Appends text to the display buffer.
    ///
    /// Returns `(text_deltas, special_tokens)`.
    pub fn process_delta(&mut self, chunk: &str) -> (Vec<String>, Vec<String>) {
        let (text_deltas, special_tokens) =
            split_text_and_special_tokens(&mut self.display.token_carry, chunk);
        for delta in &text_deltas {
            self.display.display_buffer.push_str(delta);
        }
        (text_deltas, special_tokens)
    }

    /// Finalizes the current response: flushes any remaining token carry, commits the
    /// display buffer as an assistant message, and returns any lingering token fragment.
    pub fn finalize_response(&mut self) -> Option<String> {
        let mut tail = None;
        if !self.display.token_carry.is_empty() {
            let t = std::mem::take(&mut self.display.token_carry);
            self.display.display_buffer.push_str(&t);
            tail = Some(t);
        }

        let assistant_text = self.display.display_buffer.clone();
        self.add_assistant_message(&assistant_text);

        tail
    }

    /// Resets the display buffer (used when a response is interrupted or discarded).
    pub fn reset_display_buffer(&mut self) {
        self.display.display_buffer.clear();
        self.display.token_carry.clear();
    }

    /// Resets all session state (history, display, turn count) and returns a new session ID.
    pub fn reset_session(&mut self) -> String {
        let new_id = generate_session_id();
        self.history.conversation_history.clear();
        self.display.display_buffer.clear();
        self.display.token_carry.clear();
        self.memory.session_id = new_id.clone();
        self.memory.session_started_at = chrono::Utc::now();
        self.memory.pending_embedding = None;
        self.state.last_input_embedding = None;
        self.state.last_message_time = None;
        self.state.current_turn_count = 0;
        new_id
    }

    /// Stores an embedding for the current pending user input (used for memory search).
    pub fn set_pending_embedding(&mut self, embedding: Vec<f32>) {
        self.memory.pending_embedding = Some(embedding);
    }

    /// Stores the embedding of the most recent user input (used for topic boundary detection).
    pub fn set_last_input_embedding(&mut self, embedding: Vec<f32>) {
        self.state.last_input_embedding = Some(embedding);
    }

    /// Tracks timing and turn count after a user sends a message.
    pub fn record_user_input(&mut self) {
        self.state.current_turn_count += 1;
        self.state.last_message_time = Some(Utc::now());
    }

    /// Tracks timing and turn count after the assistant sends a response.
    pub fn record_assistant_response(&mut self) {
        self.state.current_turn_count += 1;
        self.state.last_message_time = Some(Utc::now());
    }

    /// Returns the current character name, or `"default"` if no card is loaded.
    pub fn card_name(&self) -> &str {
        self.character_card
            .as_ref()
            .map(|c| c.data.get_character_name())
            .unwrap_or("default")
    }

    /// Returns the number of minutes elapsed since the session started.
    pub fn session_elapsed_minutes(&self) -> i64 {
        (Utc::now() - self.memory.session_started_at).num_minutes()
    }

    /// Polls for a completed split result and applies it to the session.
    ///
    /// If a split has completed, the conversation history is cleared and the
    /// session ID is updated to the new one from the split result.
    pub fn apply_pending_split(
        &mut self,
        pending_split: &mut Option<PendingSplitTask>,
    ) -> Option<Result<SplitResult, SessionError>> {
        let result = poll_split_result(pending_split)?;
        if let Ok(ref split_result) = result {
            self.reset_session();
            self.memory.session_id = split_result.new_session_id.clone();
        }
        Some(result)
    }

    /// Builds a [`SplitTaskInput`] from the current session state and settings.
    ///
    /// Returns `None` if the memory store or embedding provider has not been initialized.
    pub fn prepare_split_input(
        &self,
        config: &EneConfig,
        user_input: &str,
        user_name: &str,
        api_key: &str,
    ) -> Option<SplitTaskInput> {
        let store = self.memory.memory_store.clone()?;
        let embedder = self.memory.embedding_provider.clone()?;
        let session_config = config
            .get_section::<crate::SessionConfig>("session")
            .unwrap_or_default();
        let mem_config = config
            .get_section::<MemoryConfig>("memory")
            .unwrap_or_default();

        Some(SplitTaskInput {
            last_input_embedding: self.state.last_input_embedding.clone(),
            last_message_time: self.state.last_message_time,
            current_turn_count: self.state.current_turn_count,
            user_input: user_input.to_string(),
            session_config,
            summarization_model: mem_config.resolve_summarization_model(),
            summarization_base_url: mem_config
                .resolve_summarization_base_url()
                .unwrap_or_default(),
            api_key: api_key.to_string(),
            history: self.history.conversation_history.clone(),
            session_id: self.memory.session_id.clone(),
            card_name: self.card_name().to_string(),
            user_name: user_name.to_string(),
            store,
            embedder,
        })
    }
}
