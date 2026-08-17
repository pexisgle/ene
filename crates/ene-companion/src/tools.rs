use crate::memory::{
    MemoryKind, MemoryScope, MemorySource, NewMemory, RecalledMemory, recall_weights,
};
use crate::store::CompanionStore;
use async_trait::async_trait;
use ene_plane::Sensitivity;
use ene_registry::{Layer, ToolDefinition, ToolInvoke, ToolRegistry, ToolSource};
use ene_session::SoulId;
use serde_json::{Value, json};
use std::str::FromStr;
use std::sync::Arc;

/// Register harness memory tools (`memory.recall` on the surface,
/// `memory.write_shared` on the job layer).
pub fn register_memory_tools(registry: &ToolRegistry, store: Arc<CompanionStore>) {
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
        },
        Arc::new(MemoryInvoker {
            store: Arc::clone(&store),
        }),
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
            sensitivity: Sensitivity::None,
        },
        Arc::new(MemoryInvoker { store }),
    );
}

struct MemoryInvoker {
    store: Arc<CompanionStore>,
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
                let soul = soul_arg(&args)?;
                let hits = self
                    .store
                    .recall(
                        soul,
                        query,
                        8,
                        &chrono::Utc::now().to_rfc3339(),
                        recall_weights(&crate::config::RecallSettings::default()),
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
                let soul = soul_arg(&args)?;
                let kind =
                    MemoryKind::parse(args.get("kind").and_then(Value::as_str).unwrap_or(""));
                let record = self
                    .store
                    .insert_memory(NewMemory {
                        soul_id: soul,
                        scope: MemoryScope::Shared,
                        kind,
                        title: title.to_owned(),
                        content: content.to_owned(),
                        confidence: 1.0,
                        salience: 0.8,
                        source: MemorySource::Shared,
                        source_seq: None,
                        expires_at: None,
                    })
                    .map_err(|err| err.to_string())?;
                Ok(json!({ "id": record.id.to_string() }))
            }
            other => Err(format!("unknown memory tool {other}")),
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
