use crate::config::MindSettings;
use crate::memory::{
    MemoryCandidate, MemoryKind, MemoryScope, RecalledMemory, arbitrate, recall_weights,
};
use crate::store::CompanionStore;
use async_trait::async_trait;
use ene_plane::Sensitivity;
use ene_registry::{Layer, ToolDefinition, ToolInvoke, ToolRegistry, ToolSource};
use ene_session::SoulId;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::str::FromStr;
use std::sync::Arc;

/// Optional query embedding for `memory.recall` (fail-closed when unset).
#[async_trait]
pub trait QueryEmbed: Send + Sync {
    async fn embed_query(&self, text: &str) -> Option<Vec<f32>>;
}

/// Filled after the core daemon exists so memory tools can call `ai.tasks.embedding`.
#[derive(Default)]
pub struct SlotQueryEmbed {
    inner: Mutex<Option<Arc<dyn QueryEmbed>>>,
}

impl SlotQueryEmbed {
    pub fn bind(&self, embed: Arc<dyn QueryEmbed>) {
        *self.inner.lock() = Some(embed);
    }
}

#[async_trait]
impl QueryEmbed for SlotQueryEmbed {
    async fn embed_query(&self, text: &str) -> Option<Vec<f32>> {
        let inner = self.inner.lock().clone();
        match inner {
            Some(embed) => embed.embed_query(text).await,
            None => None,
        }
    }
}

/// Register harness memory tools (`memory.recall` on the surface,
/// `memory.write_shared` on the job layer).
pub fn register_memory_tools(
    registry: &ToolRegistry,
    store: Arc<CompanionStore>,
    embed: Option<Arc<dyn QueryEmbed>>,
) {
    let invoker = Arc::new(MemoryInvoker {
        store,
        bound_soul: None,
        embed,
    });
    registry.register_with(
        ToolDefinition {
            name: "memory.recall".to_owned(),
            description: "Recall memories as this companion's own knowledge.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "soul_id": { "type": "string" }
                },
                "required": ["query", "soul_id"]
            }),
            output: json!({ "type": "object" }),
            side_effects: Vec::new(),
            source: ToolSource::Harness {
                name: "memory".to_owned(),
            },
            timeout_ms: Some(5_000),
            sensitivity: Sensitivity::None,
            category: String::new(),
            keywords: Vec::new(),
            examples: Vec::new(),
            background: false,
        },
        Arc::clone(&invoker) as Arc<dyn ToolInvoke>,
    );
    registry.register_with(
        ToolDefinition {
            name: "memory.write_shared".to_owned(),
            description: "Write a fact into the shared memory pool.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "content": { "type": "string" },
                    "kind": { "type": "string" },
                    "soul_id": { "type": "string" }
                },
                "required": ["title", "content", "soul_id"]
            }),
            output: json!({ "type": "object" }),
            side_effects: vec!["memory.write".to_owned()],
            source: ToolSource::Harness {
                name: "memory".to_owned(),
            },
            timeout_ms: Some(5_000),
            sensitivity: Sensitivity::High,
            category: String::new(),
            keywords: Vec::new(),
            examples: Vec::new(),
            background: false,
        },
        invoker as Arc<dyn ToolInvoke>,
    );
}

/// Tool-side memory access. HTTP routes use `CompanionStore` directly with path soul ids.
struct MemoryInvoker {
    store: Arc<CompanionStore>,
    /// When set, `args.soul_id` is ignored and this soul is always used.
    bound_soul: Option<SoulId>,
    embed: Option<Arc<dyn QueryEmbed>>,
}

#[async_trait]
impl ToolInvoke for MemoryInvoker {
    async fn invoke(&self, name: &str, args: Value) -> Result<Value, String> {
        match name {
            "memory.recall" => {
                let query = args
                    .get("query")
                    .and_then(Value::as_str)
                    .ok_or("missing query")?;
                let soul = self.resolve_soul(&args)?;
                let query_vec = if let Some(embed) = &self.embed {
                    embed.embed_query(query).await
                } else {
                    None
                };
                let hits = self
                    .store
                    .recall_ranked(
                        soul,
                        query,
                        8,
                        &chrono::Utc::now().to_rfc3339(),
                        recall_weights(&crate::config::RecallSettings::default()),
                        query_vec.as_deref(),
                        false,
                    )
                    .map_err(|err| err.to_string())?;
                Ok(json!({
                    "memories": hits.iter().map(RecalledMemory::as_own_knowledge).collect::<Vec<_>>(),
                }))
            }
            "memory.write_shared" => {
                let title = args
                    .get("title")
                    .and_then(Value::as_str)
                    .ok_or("missing title")?;
                let content = args
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or("missing content")?;
                let soul = self.resolve_soul(&args)?;
                let kind =
                    MemoryKind::parse(args.get("kind").and_then(Value::as_str).unwrap_or(""));
                let cand = MemoryCandidate {
                    id: crate::ids::CandidateId::new(),
                    soul_id: soul,
                    kind,
                    title: title.to_owned(),
                    content: content.to_owned(),
                    scope: MemoryScope::Shared,
                    confidence: 0.9,
                    salience: 0.8,
                    sensitive: false,
                    expires_at: None,
                };
                let outcome =
                    arbitrate(&self.store, &cand, &MindSettings::default().memory_approval)
                        .map_err(|err| err.to_string())?;
                Ok(json!({ "outcome": format!("{outcome:?}") }))
            }
            other => Err(format!("unknown memory tool {other}")),
        }
    }
}

impl MemoryInvoker {
    fn resolve_soul(&self, args: &Value) -> Result<SoulId, String> {
        if let Some(bound) = self.bound_soul {
            return Ok(bound);
        }
        let soul = soul_arg(args)?;
        let known = self
            .store
            .list_souls()
            .map_err(|err| err.to_string())?
            .into_iter()
            .any(|row| row.id == soul);
        if known {
            Ok(soul)
        } else {
            Err("unknown soul_id".to_owned())
        }
    }
}

fn soul_arg(args: &Value) -> Result<SoulId, String> {
    let raw = args
        .get("soul_id")
        .and_then(Value::as_str)
        .ok_or("missing soul_id")?;
    SoulId::from_str(raw).map_err(|err| err.to_string())
}

/// `memory.write_shared` must not appear on the surface schema.
#[must_use]
pub fn surface_hides_write_shared(registry: &ToolRegistry) -> bool {
    !registry
        .schemas(Layer::Surface)
        .iter()
        .any(|schema| schema.get("name").and_then(Value::as_str) == Some("memory.write_shared"))
}
