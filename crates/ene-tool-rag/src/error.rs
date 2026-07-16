/// Tool RAG pipeline errors.
#[derive(Debug, thiserror::Error)]
pub enum ToolRagError {
    /// Configuration validation failure — e.g. an invalid forced tool name.
    #[error("Tool RAG configuration error: {message}")]
    Config {
        /// Human-readable description of the configuration problem.
        message: String,
    },
}
