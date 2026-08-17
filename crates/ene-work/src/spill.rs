use crate::error::WorkError;
use sha2::{Digest, Sha256};
use std::fmt::Write;
use std::path::{Path, PathBuf};

pub const DEFAULT_SOFT_LIMIT_BYTES: usize = 8_000;
pub const DEFAULT_HARD_LIMIT_BYTES: usize = 32_000;

/// Inline summary plus optional workspace spill file for huge tool output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpillResult {
    pub inline: String,
    pub spill_path: Option<PathBuf>,
    pub spill_ref: Option<String>,
    pub spilled: bool,
}

/// Spill output that exceeds `soft_limit`; always spill when above `hard_limit`.
pub fn spill_tool_output(
    text: &str,
    workspace_dir: &Path,
    soft_limit: usize,
    hard_limit: usize,
) -> Result<SpillResult, WorkError> {
    let soft = soft_limit.max(1);
    let hard = hard_limit.max(soft);
    if text.len() <= soft {
        return Ok(SpillResult {
            inline: text.to_owned(),
            spill_path: None,
            spill_ref: None,
            spilled: false,
        });
    }
    let spill_ref = sha256_hex(text);
    let spill_dir = workspace_dir.join("spill");
    std::fs::create_dir_all(&spill_dir)?;
    let spill_path = spill_dir.join(&spill_ref);
    std::fs::write(&spill_path, text)?;
    let summary_len = if text.len() > hard { 500 } else { 1_000 };
    let preview = truncate_chars(text, summary_len);
    let inline = format!(
        "[spilled {bytes} bytes to {ref}]\n{preview}",
        bytes = text.len(),
        ref = spill_ref,
        preview = preview
    );
    Ok(SpillResult {
        inline,
        spill_path: Some(spill_path),
        spill_ref: Some(spill_ref),
        spilled: true,
    })
}

/// Bound a job brief by spilling overflow into the workspace spill store.
pub fn bound_brief(
    brief: &str,
    workspace_dir: &Path,
    max_inline_bytes: usize,
) -> Result<String, WorkError> {
    if brief.len() <= max_inline_bytes {
        return Ok(brief.to_owned());
    }
    let spilled = spill_tool_output(brief, workspace_dir, max_inline_bytes, max_inline_bytes)?;
    Ok(spilled.inline)
}

fn sha256_hex(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(out, "{byte:02x}").ok();
    }
    out
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut out = text.chars().take(max).collect::<String>();
    out.push('…');
    out
}
