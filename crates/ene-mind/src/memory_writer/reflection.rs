//! Self-reflection pipeline (#210).
use ene_store::{AffectAnnotation, MemoryConfidence, MemoryKind, MemorySalience, MemoryScope,
    MemorySource, MemoryStatus, MemoryStore, NewMemoryItem};
use parking_lot::Mutex;
use crate::config::ReflectionConfig;
use crate::memory_writer::arbiter::AppliedDecision;

#[derive(Debug, Clone)]
pub struct OutcomeRecord {
    pub memory_id: i64,
    pub memory_title: String,
    pub rating: f32,
    pub turn_id: String,
}

#[derive(Debug)]
pub struct SelfReflectionPipeline {
    config: ReflectionConfig,
    state: Mutex<PipelineState>,
}

#[derive(Debug, Clone)]
struct PipelineState {
    turn_counter: usize,
    outcomes_buffer: Vec<OutcomeRecord>,
}

impl SelfReflectionPipeline {
    pub fn new(config: ReflectionConfig) -> Self {
        Self { config, state: Mutex::new(PipelineState { turn_counter: 0, outcomes_buffer: Vec::new() }) }
    }
    pub fn record_outcome(&self, decision: &AppliedDecision) {
        let Some(rating) = decision.outcome_rating else { return; };
        let memory_id = decision.inserted_id.unwrap_or(0);
        if memory_id == 0 { return; }
        let memory_title = decision.decision.candidate.title.clone();
        let mut s = self.state.lock();
        s.turn_counter = s.turn_counter.saturating_add(1);
        s.outcomes_buffer.push(OutcomeRecord { memory_id, memory_title, rating, turn_id: String::new() });
    }
    pub fn should_reflect(&self) -> bool {
        let s = self.state.lock();
        s.turn_counter >= self.config.interval_turns && s.outcomes_buffer.len() >= self.config.min_outcomes
    }
    fn drain(&self) -> Vec<OutcomeRecord> {
        let mut s = self.state.lock();
        s.turn_counter = 0;
        std::mem::take(&mut s.outcomes_buffer)
    }
    pub async fn generate_reflection(&self, store: &MemoryStore, character_id: &str,
        session_id: &str, user_id: &str, _sb: f32, _fp: f32) -> Vec<NewMemoryItem> {
        let outcomes = self.drain();
        if outcomes.is_empty() { return Vec::new(); }
        let items = Self::build_reflections(&outcomes, character_id, session_id, user_id);
        for item in &items {
            if let Err(e) = store.insert_typed_memory(item).await {
                tracing::warn!(component="SelfReflection", error=%e, title=%item.title, "Failed to persist");
            }
        }
        items
    }
    pub fn build_reflections(outcomes: &[OutcomeRecord], character_id: &str,
        session_id: &str, user_id: &str) -> Vec<NewMemoryItem> {
        let pos: Vec<_> = outcomes.iter().filter(|o| o.rating > 0.3).collect();
        let neg: Vec<_> = outcomes.iter().filter(|o| o.rating < -0.3).collect();
        let mut r = Vec::new();
        if !pos.is_empty() {
            let titles: Vec<&str> = pos.iter().map(|o| o.memory_title.as_str()).collect();
            r.push(NewMemoryItem {
                scope: MemoryScope::Shared, character_id: character_id.to_string(),
                user_id: user_id.to_string(), kind: MemoryKind::Reflection,
                title: "Successful strategies".into(),
                content: format!("Successful interaction strategies: {}", titles.join(", ")),
                source: MemorySource::Inferred, source_ref: Some(session_id.to_string()),
                confidence: MemoryConfidence::new(0.7), salience: MemorySalience::new(0.6),
                affect: AffectAnnotation::default(), relationship_impact: 0.0,
                valid_from: None, valid_until: None, status: MemoryStatus::Active,
                supersedes_id: None, pinned: false, created_at: None, commitment_id: None,
            });
        }
        if !neg.is_empty() {
            let titles: Vec<&str> = neg.iter().map(|o| o.memory_title.as_str()).collect();
            r.push(NewMemoryItem {
                scope: MemoryScope::Shared, character_id: character_id.to_string(),
                user_id: user_id.to_string(), kind: MemoryKind::Reflection,
                title: "Strategies to avoid".into(),
                content: format!("Less effective interaction strategies: {}", titles.join(", ")),
                source: MemorySource::Inferred, source_ref: Some(session_id.to_string()),
                confidence: MemoryConfidence::new(0.7), salience: MemorySalience::new(0.4),
                affect: AffectAnnotation::default(), relationship_impact: 0.0,
                valid_from: None, valid_until: None, status: MemoryStatus::Active,
                supersedes_id: None, pinned: false, created_at: None, commitment_id: None,
            });
        }
        r
    }
}

pub async fn load_reflection_memories(store: &MemoryStore, character_id: &str) -> Vec<ene_store::MemoryItem> {
    match store.get_typed_memories_by_character(character_id, Some(MemoryKind::Reflection), 50, 0).await {
        Ok(items) => items,
        Err(e) => { tracing::warn!(component="SelfReflection", error=%e, "Failed to load"); Vec::new() }
    }
}

pub fn apply_reflection_adjustment(memories: &mut [ene_store::ScoredMemory],
    reflections: &[ene_store::MemoryItem], success_boost: f32, failure_penalty: f32) {
    let (succ, fail) = parse_strategies(reflections);
    if succ.is_empty() && fail.is_empty() { return; }
    for m in memories.iter_mut() {
        let t = m.item.title.to_lowercase();
        if succ.iter().any(|s| t.contains(s.as_str())) { m.breakdown.total *= success_boost; }
        else if fail.iter().any(|s| t.contains(s.as_str())) { m.breakdown.total *= failure_penalty; }
    }
}

fn parse_strategies(reflections: &[ene_store::MemoryItem]) -> (Vec<String>, Vec<String>) {
    let mut s = Vec::new(); let mut f = Vec::new();
    for r in reflections {
        let c = r.content.to_lowercase();
        if r.title == "Successful strategies" {
            if let Some(st) = c.strip_prefix("successful interaction strategies: ") {
                s.extend(st.split(", ").map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()));
            }
        } else if r.title == "Strategies to avoid" {
            if let Some(st) = c.strip_prefix("less effective interaction strategies: ") {
                f.extend(st.split(", ").map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()));
            }
        }
    }
    (s, f)
}
