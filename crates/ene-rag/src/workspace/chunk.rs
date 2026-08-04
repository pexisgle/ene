//! Line-based document chunking with heading and line-range metadata.

/// Chunking parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkOptions {
    /// Target character budget per chunk; chunks are emitted once accumulated
    /// lines reach this budget.
    pub chunk_chars: usize,
    /// How many trailing characters of the previous chunk are repeated at the
    /// start of the next one (line-granular).
    pub overlap_chars: usize,
    /// Hard cap on chunks per document; exceeding it marks the document
    /// truncated instead of silently dropping the tail.
    pub max_chunks_per_file: usize,
}

/// One chunk with its citation metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentChunk {
    /// Zero-based position within the document.
    pub chunk_index: u32,
    /// Nearest heading at chunk start (empty when none was found).
    pub heading: String,
    /// Chunk text.
    pub content: String,
    /// First line of the chunk (1-based, inclusive).
    pub start_line: u32,
    /// Last line of the chunk (1-based, inclusive).
    pub end_line: u32,
}

/// Chunking result for one document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkedDocument {
    /// The chunks, in document order.
    pub chunks: Vec<DocumentChunk>,
    /// `true` when the chunk cap cut the document short. Callers must skip
    /// truncated documents rather than index a partial file.
    pub truncated: bool,
}

/// Splits a UTF-8 document into line-granular chunks.
///
/// Headings are tracked from Markdown ATX lines (`#`..`######`); files
/// without headings fall back to `fallback_heading` (the file stem).
/// Empty documents produce no chunks. A document whose chunk count would
/// exceed `options.max_chunks_per_file` is reported truncated.
pub fn chunk_document(
    text: &str,
    fallback_heading: &str,
    options: &ChunkOptions,
) -> ChunkedDocument {
    if options.chunk_chars == 0 || options.max_chunks_per_file == 0 {
        return ChunkedDocument {
            chunks: Vec::new(),
            truncated: false,
        };
    }

    let mut chunks = Vec::new();
    let mut heading = String::new();
    let mut pending: Vec<(u32, String)> = Vec::new();
    let mut pending_chars = 0usize;

    let flush = |chunks: &mut Vec<DocumentChunk>,
                 pending: &mut Vec<(u32, String)>,
                 heading: &mut String,
                 chunk_index: &mut u32,
                 overlap_chars: usize| {
        if pending.is_empty() {
            return;
        }
        let start_line = pending.first().map_or(1, |(line, _)| *line);
        let end_line = pending.last().map_or(1, |(line, _)| *line);
        let content = pending
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        chunks.push(DocumentChunk {
            chunk_index: *chunk_index,
            heading: heading.clone(),
            content,
            start_line,
            end_line,
        });
        *chunk_index = chunk_index.saturating_add(1);

        // Keep the trailing lines that fit the overlap budget so the next
        // chunk re-reads the boundary context.
        let mut kept: Vec<(u32, String)> = Vec::new();
        let mut kept_chars = 0usize;
        for entry in pending.iter().rev() {
            let chars = entry.1.chars().count().saturating_add(1);
            if kept_chars.saturating_add(chars) > overlap_chars {
                break;
            }
            kept.push(entry.clone());
            kept_chars = kept_chars.saturating_add(chars);
        }
        kept.reverse();
        pending.clear();
        pending.extend(kept);
    };

    let mut chunk_index = 0u32;
    for (line_index, raw_line) in text.lines().enumerate() {
        let line_no = u32::try_from(line_index.saturating_add(1)).unwrap_or(u32::MAX);
        let line = raw_line.trim_end();

        if let Some(h) = atx_heading(line) {
            heading = h.to_string();
        }

        if !line.is_empty() {
            pending.push((line_no, line.to_string()));
            pending_chars = pending_chars.saturating_add(line.chars().count().saturating_add(1));
        }

        if pending_chars >= options.chunk_chars {
            flush(
                &mut chunks,
                &mut pending,
                &mut heading,
                &mut chunk_index,
                options.overlap_chars,
            );
            pending_chars = pending
                .iter()
                .map(|(_, text)| text.chars().count().saturating_add(1))
                .sum();
            if chunks.len() >= options.max_chunks_per_file {
                return ChunkedDocument {
                    chunks,
                    truncated: true,
                };
            }
        }
    }

    if !pending.is_empty() {
        flush(
            &mut chunks,
            &mut pending,
            &mut heading,
            &mut chunk_index,
            options.overlap_chars,
        );
    }

    if chunks.is_empty() && !text.trim().is_empty() {
        // Pathological case (e.g. a single line longer than the budget):
        // emit one chunk holding the document so the file is not lost.
        let lines: Vec<&str> = text.lines().collect();
        let start_line = 1u32;
        let end_line = u32::try_from(lines.len()).unwrap_or(u32::MAX);
        chunks.push(DocumentChunk {
            chunk_index: 0,
            heading: fallback_heading.to_string(),
            content: text.trim_end().to_string(),
            start_line,
            end_line,
        });
    } else if !chunks.is_empty() && chunks[0].heading.is_empty() {
        chunks[0].heading = fallback_heading.to_string();
    }

    let truncated = chunks.len() > options.max_chunks_per_file;
    if truncated {
        chunks.truncate(options.max_chunks_per_file);
    }

    ChunkedDocument { chunks, truncated }
}

/// Returns the Markdown ATX heading text for a line, if any.
fn atx_heading(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let level = trimmed.bytes().take_while(|b| *b == b'#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &trimmed[level..];
    if !rest.starts_with(' ') {
        return None;
    }
    let heading = rest.trim();
    (!heading.is_empty()).then_some(heading)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> ChunkOptions {
        ChunkOptions {
            chunk_chars: 80,
            overlap_chars: 20,
            max_chunks_per_file: 16,
        }
    }

    #[test]
    fn chunks_carry_heading_and_line_ranges() {
        let doc = "# Intro\n\nfirst paragraph\nsecond line\n\n## Details\nmore content here\n";
        let mut opts = options();
        opts.chunk_chars = 30;
        let out = chunk_document(doc, "guide.md", &opts);
        assert!(!out.truncated);
        assert!(!out.chunks.is_empty());
        let first = &out.chunks[0];
        assert_eq!(first.heading, "Intro");
        assert_eq!(first.start_line, 1);
        assert!(first.content.contains("first paragraph"));
        let with_heading = out.chunks.iter().find(|c| c.heading == "Details").unwrap();
        assert!(with_heading.content.contains("more content here"));
        assert!(with_heading.chunk_index > first.chunk_index);
        assert!(with_heading.start_line >= first.start_line);
    }

    #[test]
    fn overlap_repeats_boundary_lines() {
        let doc = (0..60)
            .map(|i| format!("line {i:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = chunk_document(&doc, "f.txt", &options());
        assert!(out.chunks.len() > 1);
        let a = &out.chunks[0];
        let b = &out.chunks[1];
        assert!(a.end_line >= b.start_line, "chunks must overlap");
        let last_nl = a.content.rfind('\n');
        let tail_start = last_nl.map_or(0, |nl| a.content[..nl].rfind('\n').map_or(0, |n| n + 1));
        assert!(
            b.content.starts_with(&a.content[tail_start..]),
            "next chunk must repeat the previous chunk's tail"
        );
    }

    #[test]
    fn empty_document_produces_no_chunks() {
        let out = chunk_document("", "f.txt", &options());
        assert!(out.chunks.is_empty());
        assert!(!out.truncated);
    }

    #[test]
    fn single_long_line_is_kept_as_one_chunk() {
        let line = "x".repeat(500);
        let out = chunk_document(&line, "f.txt", &options());
        assert_eq!(out.chunks.len(), 1);
        assert_eq!(out.chunks[0].content, line);
        assert_eq!(out.chunks[0].heading, "f.txt");
    }

    #[test]
    fn chunk_cap_reports_truncation() {
        let doc = (0..200)
            .map(|i| format!("line {i:03}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut opts = options();
        opts.max_chunks_per_file = 2;
        let out = chunk_document(&doc, "f.txt", &opts);
        assert!(out.truncated);
        assert_eq!(out.chunks.len(), 2);
    }

    #[test]
    fn fallback_heading_is_file_stem() {
        let doc = "plain text without headings\n";
        let out = chunk_document(doc, "readme.md", &options());
        assert_eq!(out.chunks[0].heading, "readme.md");
    }
}
