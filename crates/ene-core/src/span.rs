//! Memory span domain types for rolling conversation compression.

/// Input for inserting a compressed conversation span.
#[derive(Debug, Clone)]
pub struct NewMemorySpan {
    /// Session this span belongs to.
    pub session_id: String,
    /// First turn index in the span.
    pub turn_start: i32,
    /// Last turn index in the span.
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
    /// Span database id.
    pub span_id: i64,
    /// Summary text.
    pub summary: String,
    /// Compression level.
    pub compression_level: i32,
}
