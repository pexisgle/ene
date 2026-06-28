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
    pub last_accessed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub status: String,
    pub supersedes_id: Option<i64>,
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
