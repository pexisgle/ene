use crate::conversation_manager::generate_session_id;
use crate::special_token::split_text_and_special_tokens;
use async_openai::types::chat::Role;
use chrono::{DateTime, Utc};
use ene_config::character_card::{CharacterCardV3, ResolvedExpression, resolve_expressions};
use ene_embedding::EmbeddingProvider;
use ene_memory::MemoryStore;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct ConversationHistory {
    pub conversation_history: Vec<(Role, String)>,
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

#[derive(Clone, Debug, Default)]
pub struct DisplayState {
    pub display_buffer: String,
    pub token_carry: String,
}

#[derive(Clone)]
pub struct MemoryContext {
    pub memory_store: Option<Arc<MemoryStore>>,
    pub embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    pub session_id: String,
    pub session_started_at: chrono::DateTime<chrono::Utc>,
    pub pending_embedding: Option<Vec<f32>>,
}

#[derive(Clone, Debug, Default)]
pub struct SessionState {
    pub last_input_embedding: Option<Vec<f32>>,
    pub last_message_time: Option<DateTime<Utc>>,
    pub current_turn_count: usize,
}

#[derive(Clone)]
pub struct ConversationSession {
    pub history: ConversationHistory,
    pub display: DisplayState,
    pub memory: MemoryContext,
    pub state: SessionState,
    pub character_card: Option<CharacterCardV3>,
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

    pub fn init_memory(&mut self, store: Arc<MemoryStore>, embedder: Arc<dyn EmbeddingProvider>) {
        self.memory.memory_store = Some(store);
        self.memory.embedding_provider = Some(embedder);
    }

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

        // Merge expressions from character_settings.json
        if let Some(settings_path) = std::path::Path::new(path)
            .parent()
            .map(|p| p.join("character_settings.json"))
            .filter(|p| p.exists())
        {
            if let Ok(settings_content) = std::fs::read_to_string(&settings_path) {
                if let Ok(settings_value) =
                    serde_json::from_str::<serde_json::Value>(&settings_content)
                {
                    if let Some(expr) = settings_value.get("expressions") {
                        card.data
                            .extensions
                            .insert("expressions".to_string(), expr.clone());
                    }
                    if let Some(dm) = settings_value
                        .get("default_motion")
                        .and_then(|v| v.as_str())
                    {
                        let mut ene = serde_json::Map::new();
                        ene.insert(
                            "default_motion".to_string(),
                            serde_json::Value::String(dm.to_string()),
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

    pub fn add_user_message(&mut self, input: &str) {
        self.history
            .conversation_history
            .push((Role::User, input.to_string()));
        self.history.trim_history();
    }

    pub fn add_assistant_message(&mut self, text: &str) {
        self.history
            .conversation_history
            .push((Role::Assistant, text.to_string()));
        self.history.trim_history();
    }

    pub fn process_delta(&mut self, chunk: &str) -> (Vec<String>, Vec<String>) {
        let (text_deltas, special_tokens) =
            split_text_and_special_tokens(&mut self.display.token_carry, chunk);
        for delta in &text_deltas {
            self.display.display_buffer.push_str(delta);
        }
        (text_deltas, special_tokens)
    }

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

    pub fn reset_display_buffer(&mut self) {
        self.display.display_buffer.clear();
        self.display.token_carry.clear();
    }

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

    pub fn set_pending_embedding(&mut self, embedding: Vec<f32>) {
        self.memory.pending_embedding = Some(embedding);
    }

    pub fn set_last_input_embedding(&mut self, embedding: Vec<f32>) {
        self.state.last_input_embedding = Some(embedding);
    }

    pub fn record_user_input(&mut self) {
        self.state.current_turn_count += 1;
        self.state.last_message_time = Some(Utc::now());
    }

    pub fn record_assistant_response(&mut self) {
        self.state.current_turn_count += 1;
        self.state.last_message_time = Some(Utc::now());
    }

    pub fn card_name(&self) -> &str {
        self.character_card
            .as_ref()
            .map(|c| c.data.get_character_name())
            .unwrap_or("default")
    }

    pub fn session_elapsed_minutes(&self) -> i64 {
        (Utc::now() - self.memory.session_started_at).num_minutes()
    }
}
