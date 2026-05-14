/// 出力を制限サイズに収める
pub struct Truncate;

impl Truncate {
    /// テキストを max_lines / max_bytes に収める（head方向）
    pub fn output(text: &str, max_lines: usize, max_bytes: usize) -> TruncateResult {
        let lines: Vec<&str> = text.lines().collect();
        let total_bytes = text.len();

        if lines.len() <= max_lines && total_bytes <= max_bytes {
            return TruncateResult {
                content: text.to_string(),
                truncated: false,
            };
        }

        let mut out = Vec::new();
        let mut bytes = 0usize;
        let mut hit_bytes = false;

        for line in lines.iter().take(max_lines) {
            let size = line.len() + if out.is_empty() { 0 } else { 1 }; // +1 for newline
            if bytes + size > max_bytes {
                hit_bytes = true;
                break;
            }
            out.push(*line);
            bytes += size;
        }

        let removed = if hit_bytes {
            total_bytes - bytes
        } else {
            text.len() - out.join("\n").len()
        };

        let preview = out.join("\n");
        let unit = if hit_bytes { "bytes" } else { "lines" };

        TruncateResult {
            content: format!(
                "{}\n\n...{} {} truncated...\n\nUse offset/limit or grep to view specific sections.",
                preview, removed, unit
            ),
            truncated: true,
        }
    }

    /// tail方向に切り詰め
    pub fn tail(text: &str, max_lines: usize, max_bytes: usize) -> TruncateResult {
        let lines: Vec<&str> = text.lines().collect();
        let total_bytes = text.len();

        if lines.len() <= max_lines && total_bytes <= max_bytes {
            return TruncateResult {
                content: text.to_string(),
                truncated: false,
            };
        }

        let mut out = Vec::new();
        let mut bytes = 0usize;
        let mut hit_bytes = false;

        for line in lines.iter().rev().take(max_lines) {
            let size = line.len() + if out.is_empty() { 0 } else { 1 };
            if bytes + size > max_bytes {
                hit_bytes = true;
                break;
            }
            out.insert(0, *line);
            bytes += size;
        }

        let removed = if hit_bytes {
            total_bytes - bytes
        } else {
            text.len() - out.join("\n").len()
        };

        let preview = out.join("\n");
        let unit = if hit_bytes { "bytes" } else { "lines" };

        TruncateResult {
            content: format!(
                "...{} {} truncated...\n\n{}",
                removed, unit, preview
            ),
            truncated: true,
        }
    }
}

pub struct TruncateResult {
    pub content: String,
    pub truncated: bool,
}
