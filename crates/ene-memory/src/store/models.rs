use diesel::deserialize::{FromSql, FromSqlRow};
use diesel::expression::AsExpression;
use diesel::serialize::{Output, ToSql};
use diesel::sql_types::Binary;
use diesel::sqlite::Sqlite;

#[derive(Debug, Clone, FromSqlRow, AsExpression)]
#[diesel(sql_type = Binary)]
pub struct EmbeddingBlob(pub Vec<f32>);

impl FromSql<Binary, Sqlite> for EmbeddingBlob {
    fn from_sql(mut bytes: diesel::sqlite::SqliteValue) -> diesel::deserialize::Result<Self> {
        let raw_bytes = bytes.read_blob();
        let vec = raw_bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();
        Ok(EmbeddingBlob(vec))
    }
}

impl ToSql<Binary, Sqlite> for EmbeddingBlob {
    fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, Sqlite>) -> diesel::serialize::Result {
        let mut bytes = Vec::with_capacity(self.0.len() * 4);
        for f in &self.0 {
            bytes.extend_from_slice(&f.to_le_bytes());
        }
        out.set_value(bytes);
        Ok(diesel::serialize::IsNull::No)
    }
}

#[derive(Debug, Clone, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = crate::schema::conversation_summaries)]
pub struct ConversationSummaryRow {
    pub id: i64,
    pub session_id: String,
    pub card_name: String,
    pub summary: String,
    pub embedding: EmbeddingBlob,
    pub created_at: String,
    pub ended_at: String,
}

#[derive(diesel::Insertable)]
#[diesel(table_name = crate::schema::conversation_summaries)]
pub struct NewConversationSummary<'a> {
    pub session_id: &'a str,
    pub card_name: &'a str,
    pub summary: &'a str,
    pub embedding: EmbeddingBlob,
    pub created_at: &'a str,
    pub ended_at: &'a str,
}

#[derive(Debug, Clone, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = crate::schema::conversation_keyfacts)]
#[expect(dead_code)]
pub struct KeyFactRow {
    pub id: i64,
    pub card_name: String,
    pub summary_id: Option<i64>,
    pub key: String,
    pub value: String,
    pub created_at: String,
}

#[derive(diesel::Insertable)]
#[diesel(table_name = crate::schema::conversation_keyfacts)]
pub struct NewKeyFact<'a> {
    pub card_name: &'a str,
    pub summary_id: Option<i64>,
    pub key: &'a str,
    pub value: &'a str,
    pub created_at: &'a str,
}

#[derive(Debug, Clone, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = crate::schema::conversation_logs)]
#[expect(dead_code)]
pub struct ConversationLogRow {
    pub id: i64,
    pub session_id: String,
    pub card_name: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

#[derive(diesel::Insertable)]
#[diesel(table_name = crate::schema::conversation_logs)]
pub struct NewConversationLog<'a> {
    pub session_id: &'a str,
    pub card_name: &'a str,
    pub role: &'a str,
    pub content: &'a str,
    pub created_at: &'a str,
}

#[derive(Debug, Clone, diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = crate::schema::tool_embedding_index)]
#[expect(dead_code)]
pub struct ToolEmbeddingIndexRow {
    pub id: i32,
    pub tool_name: String,
    pub field: String,
    pub field_key: String,
    pub version_hash: String,
    pub model_name: String,
    pub source_text: String,
    pub embedding: EmbeddingBlob,
    pub created_at: String,
}

#[derive(diesel::Insertable)]
#[diesel(table_name = crate::schema::tool_embedding_index)]
pub struct NewToolEmbeddingIndex<'a> {
    pub tool_name: &'a str,
    pub field: &'a str,
    pub field_key: &'a str,
    pub version_hash: &'a str,
    pub model_name: &'a str,
    pub source_text: &'a str,
    pub embedding: EmbeddingBlob,
    pub created_at: &'a str,
}
