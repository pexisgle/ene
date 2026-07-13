use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "typed_memories")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub scope: String,
    pub character_id: String,
    pub user_id: String,
    pub kind: String,
    pub title: String,
    pub content: String,
    pub source: String,
    pub source_ref: Option<String>,
    pub confidence: f32,
    pub salience: f32,
    pub affective_valence: f32,
    pub affective_arousal: f32,
    pub relationship_impact: f32,
    pub access_count: i64,
    pub last_accessed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub status: String,
    pub supersedes_id: Option<i64>,
    pub pinned: i32,
    pub faded_at: Option<DateTime<Utc>>,
    /// Optional FK to `commitments.id` when this row is a ledger reference (#124).
    pub commitment_id: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::memory_embeddings::Entity")]
    MemoryEmbeddings,
}

impl Related<super::memory_embeddings::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::MemoryEmbeddings.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
