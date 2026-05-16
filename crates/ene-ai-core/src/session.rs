use crate::character_card::{CharacterCardV3, ResolvedExpression, resolve_expressions};
use crate::conversation_manager::generate_session_id;
use crate::embedding::EmbeddingProvider;
use crate::memory::store::MemoryStore;
use crate::special_token::split_text_and_special_tokens;
use async_openai::types::chat::Role;
use chrono::{DateTime, Utc};
use std::sync::Arc;

#[derive(Clone)]
pub struct ConversationSession {
    pub conversation_history: Vec<(Role, String)>,
    pub max_history_turns: usize,
    pub character_card: Option<CharacterCardV3>,
    pub current_card_path: String,
    pub display_buffer: String,
    pub token_carry: String,

    // ── Long-Term Memory ──────────────────────────────────────────────────────
    pub memory_store: Option<Arc<MemoryStore>>,
    pub embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    pub session_id: String,
    pub session_started_at: chrono::DateTime<chrono::Utc>,
    pub pending_embedding: Option<Vec<f32>>,

    // ── Session Management (formerly ConversationManager) ─────────────────────
    pub last_input_embedding: Option<Vec<f32>>,
    pub last_message_time: Option<DateTime<Utc>>,
    pub current_turn_count: usize,
}

impl std::fmt::Debug for ConversationSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConversationSession")
            .field("conversation_history_len", &self.conversation_history.len())
            .field("max_history_turns", &self.max_history_turns)
            .field("current_card_path", &self.current_card_path)
            .field("memory_enabled", &self.memory_store.is_some())
            .field("session_id", &self.session_id)
            .field("turn_count", &self.current_turn_count)
            .finish()
    }
}

impl Default for ConversationSession {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversationSession {
    pub fn new() -> Self {
        Self {
            conversation_history: Vec::new(),
            max_history_turns: 20,
            character_card: None,
            current_card_path: String::new(),
            display_buffer: String::new(),
            token_carry: String::new(),
            memory_store: None,
            embedding_provider: None,
            session_id: generate_session_id(),
            session_started_at: chrono::Utc::now(),
            pending_embedding: None,
            last_input_embedding: None,
            last_message_time: None,
            current_turn_count: 0,
        }
    }

    pub fn init_memory(&mut self, store: Arc<MemoryStore>, embedder: Arc<dyn EmbeddingProvider>) {
        self.memory_store = Some(store);
        self.embedding_provider = Some(embedder);
    }

    pub fn load_card(&mut self, path: &str) -> Result<Vec<ResolvedExpression>, String> {
        if self.current_card_path == path && self.character_card.is_some() {
            if let Some(card) = &self.character_card {
                return Ok(resolve_expressions(card));
            }
        }

        let file_content = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read character card file: {e}"))?;

        let card = serde_json::from_str::<CharacterCardV3>(&file_content)
            .map_err(|e| format!("failed to parse character card: {e}"))?;

        self.character_card = Some(card);
        self.current_card_path = path.to_string();
        self.conversation_history.clear();

        Ok(resolve_expressions(self.character_card.as_ref().unwrap()))
    }

    pub fn add_user_message(&mut self, input: &str) {
        self.conversation_history
            .push((Role::User, input.to_string()));
        self.trim_history();
    }

    pub fn add_assistant_message(&mut self, text: &str) {
        self.conversation_history
            .push((Role::Assistant, text.to_string()));
        self.trim_history();
    }

    fn trim_history(&mut self) {
        let max = self.max_history_turns * 2;
        if self.conversation_history.len() > max {
            let excess = self.conversation_history.len() - max;
            self.conversation_history.drain(0..excess);
        }
    }

    pub fn process_delta(&mut self, chunk: &str) -> (Vec<String>, Vec<String>) {
        let (text_deltas, special_tokens) =
            split_text_and_special_tokens(&mut self.token_carry, chunk);
        for delta in &text_deltas {
            self.display_buffer.push_str(delta);
        }
        (text_deltas, special_tokens)
    }

    pub fn finalize_response(&mut self) -> Option<String> {
        let mut tail = None;
        if !self.token_carry.is_empty() {
            let t = std::mem::take(&mut self.token_carry);
            self.display_buffer.push_str(&t);
            tail = Some(t);
        }

        let assistant_text = self.display_buffer.clone();
        self.add_assistant_message(&assistant_text);

        tail
    }

    pub fn reset_display_buffer(&mut self) {
        self.display_buffer.clear();
        self.token_carry.clear();
    }

    pub fn reset_session(&mut self) -> String {
        let new_id = generate_session_id();
        self.conversation_history.clear();
        self.display_buffer.clear();
        self.token_carry.clear();
        self.session_id = new_id.clone();
        self.session_started_at = chrono::Utc::now();
        self.pending_embedding = None;
        self.last_input_embedding = None;
        self.last_message_time = None;
        self.current_turn_count = 0;
        new_id
    }

    pub fn set_pending_embedding(&mut self, embedding: Vec<f32>) {
        self.pending_embedding = Some(embedding);
    }

    pub fn set_last_input_embedding(&mut self, embedding: Vec<f32>) {
        self.last_input_embedding = Some(embedding);
    }

    pub fn record_user_input(&mut self) {
        self.current_turn_count += 1;
        self.last_message_time = Some(Utc::now());
    }

    pub fn record_assistant_response(&mut self) {
        self.current_turn_count += 1;
        self.last_message_time = Some(Utc::now());
    }

    pub fn card_name(&self) -> &str {
        self.character_card
            .as_ref()
            .map(|c| c.data.get_character_name())
            .unwrap_or("default")
    }

    pub fn session_elapsed_minutes(&self) -> i64 {
        (Utc::now() - self.session_started_at).num_minutes()
    }
}
