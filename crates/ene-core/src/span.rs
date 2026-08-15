/// Input for inserting a compressed conversation span.
#[derive(Debug, Clone)]
pub struct NewMemorySpan {
    pub session_id: String,
    pub turn_start: i32,
    pub turn_end: i32,
    /// Raw excerpt from source logs.
    pub raw_excerpt: Option<String>,
    /// Compressed summary (empty until compression runs).
    pub compressed_summary: Option<String>,
    /// Compression level (0 = scene, 1 = chapter, 2 = arc).
    pub compression_level: i32,
}

/// Active scene summary row for prompt injection.
#[derive(Debug, Clone)]
pub struct ActiveSceneSummaryRow {
    pub span_id: i64,
    pub summary: String,
    pub compression_level: i32,
}
