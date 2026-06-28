use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "affect_states")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub character_id: String,
    pub user_id: String,
    pub valence: f32,
    pub arousal: f32,
    pub dominance: f32,
    pub trust: f32,
    pub affinity: f32,
    pub irritation: f32,
    pub curiosity: f32,
    pub fatigue: f32,
    pub mood_label: String,
    pub last_expression: String,
    pub discrete_emotions: String,
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
