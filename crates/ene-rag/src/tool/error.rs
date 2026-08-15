#[derive(Debug, thiserror::Error)]
pub enum ToolRagError {
    /// Configuration validation failure — e.g. an invalid forced tool name.
    #[error("Tool RAG configuration error: {message}")]
    Config { message: String },
}
