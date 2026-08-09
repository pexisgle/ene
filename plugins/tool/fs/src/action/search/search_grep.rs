use super::MAX_RESULTS;
use crate::utils::sandbox::SandboxConfig;
use crate::utils::{SandboxRef, default_sandbox, resolve_sandbox};
use ene_plugin::prelude::*;
use std::path::Path;

const MAX_LINE_CHARS: usize = 2000;
const MAX_CAPTURE_CHARS: usize = 200;

/// Rendering options for [`grep_search`].
#[derive(Debug, Clone)]
pub struct GrepOptions {
    /// Match case-insensitively.
    pub case_insensitive: bool,
    /// Prefix each match with its 1-based line number.
    pub line_numbers: bool,
    /// Number of non-matching context lines to print around each match.
    pub context_lines: usize,
    /// Print only the match count instead of the matched lines.
    pub count: bool,
}

impl Default for GrepOptions {
    fn default() -> Self {
        Self {
            case_insensitive: false,
            line_numbers: true,
            context_lines: 0,
            count: false,
        }
    }
}

/// A single rendered line of a grep result: either a match or context.
enum Entry {
    Match {
        line: usize,
        text: String,
        captures: Option<String>,
    },
    Context {
        line: usize,
        text: String,
    },
}

pub async fn grep_search(
    pattern: &str,
    path: Option<&str>,
    include: Option<&str>,
    options: &GrepOptions,
    sandbox: &SandboxConfig,
) -> Result<String, ToolError> {
    if pattern.is_empty() {
        return Err(ToolError::execution_failed(
            "pattern is required".to_string(),
        ));
    }

    let re = regex::RegexBuilder::new(pattern)
        .case_insensitive(options.case_insensitive)
        .build()
        .map_err(|e| ToolError::execution_failed(format!("Invalid regex pattern: {e}")))?;

    let base = if let Some(p) = path {
        sandbox.resolve_and_check(Path::new(p), false)?
    } else {
        std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf())
    };

    let search_dir = if base.is_dir() {
        base
    } else {
        base.parent().unwrap_or(&base).to_path_buf()
    };

    let broker = sandbox.broker()?;
    let mut per_file: Vec<(String, Vec<Entry>)> = Vec::new();
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut total_matches = 0usize;
    let mut exceeded = false;

    let mut files = Vec::new();
    crate::action::search::search_glob::walk_all(&broker, &search_dir, 0, &mut files).await;

    'walk: for file_path_str in files {
        let file_path = std::path::PathBuf::from(&file_path_str);

        if let Some(inc) = include {
            let file_name = file_path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if !glob::Pattern::new(inc).is_ok_and(|p| p.matches(file_name)) {
                continue;
            }
        }

        let Ok(Some(metadata)) = broker.stat(&file_path_str).await else {
            continue;
        };
        if metadata.size > 1024 * 1024 {
            continue;
        }

        let Ok(content) = broker.read_text(&file_path_str, 1024 * 1024).await else {
            continue;
        };

        let lines: Vec<&str> = content.lines().collect();
        let mut matched = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if re.is_match(line) {
                matched.push(i);
            }
        }
        if matched.is_empty() {
            continue;
        }

        total_matches += matched.len();
        if options.count {
            counts.push((file_path.to_string_lossy().to_string(), matched.len()));
            continue;
        }

        per_file.push((
            file_path.to_string_lossy().to_string(),
            build_entries(&re, &lines, &matched, options.context_lines),
        ));
        if total_matches > MAX_RESULTS {
            exceeded = true;
            break 'walk;
        }
    }

    if options.count {
        return Ok(render_counts(&counts, total_matches));
    }

    if per_file.is_empty() {
        return Ok("No files found".to_string());
    }

    let truncated = exceeded;
    let mut output = vec![format!(
        "Found {} matches{}",
        total_matches,
        if truncated {
            format!(" (showing first {MAX_RESULTS})")
        } else {
            String::new()
        }
    )];

    let mut remaining = MAX_RESULTS;
    for (path, entries) in &per_file {
        if remaining == 0 {
            break;
        }
        output.push(format!("{path}:"));
        for entry in entries {
            match entry {
                Entry::Match { .. } if remaining == 0 => break,
                Entry::Match { .. } => remaining -= 1,
                Entry::Context { .. } => {}
            }
            output.push(render_entry(entry, options.line_numbers));
        }
    }

    if truncated {
        output.push(String::new());
        output.push(format!(
            "(Results truncated: showing {} of {} matches ({} hidden). Consider using a more specific path or pattern.)",
            MAX_RESULTS, total_matches, total_matches - MAX_RESULTS
        ));
    }

    Ok(output.join("\n"))
}

fn build_entries(
    re: &regex::Regex,
    lines: &[&str],
    matched: &[usize],
    context: usize,
) -> Vec<Entry> {
    let mut entries = Vec::new();
    let has_groups = re.captures_len() > 1;
    for (idx, &m) in matched.iter().enumerate() {
        let prev_after_end = if idx > 0 {
            matched[idx - 1].saturating_add(context).saturating_add(1)
        } else {
            0
        };
        let start = m.saturating_sub(context).max(prev_after_end);
        for (j, line) in lines.iter().enumerate().take(m).skip(start) {
            entries.push(Entry::Context {
                line: j,
                text: (*line).to_string(),
            });
        }
        let captures = if has_groups {
            re.captures(lines[m]).map(|caps| format_captures(&caps))
        } else {
            None
        };
        entries.push(Entry::Match {
            line: m,
            text: lines[m].to_string(),
            captures,
        });
        let next_end = matched.get(idx + 1).map_or(lines.len(), |&next| next);
        let after_end = m.saturating_add(context).saturating_add(1).min(next_end);
        for (j, line) in lines.iter().enumerate().take(after_end).skip(m + 1) {
            entries.push(Entry::Context {
                line: j,
                text: (*line).to_string(),
            });
        }
    }
    entries
}

fn render_entry(entry: &Entry, line_numbers: bool) -> String {
    match entry {
        Entry::Match {
            line,
            text,
            captures,
        } => {
            let mut parts = vec![if line_numbers {
                format!("  Line {}: {}", line + 1, truncate(text, MAX_LINE_CHARS))
            } else {
                format!("  {}", truncate(text, MAX_LINE_CHARS))
            }];
            if let Some(caps) = captures {
                parts.push(format!("    Captures: {caps}"));
            }
            parts.join("\n")
        }
        Entry::Context { line, text } => {
            if line_numbers {
                format!("  Context {}: {}", line + 1, truncate(text, MAX_LINE_CHARS))
            } else {
                format!("  Context: {}", truncate(text, MAX_LINE_CHARS))
            }
        }
    }
}

fn format_captures(caps: &regex::Captures<'_>) -> String {
    let mut parts = Vec::new();
    for i in 1..caps.len() {
        let value = match caps.get(i) {
            Some(m) => truncate(m.as_str(), MAX_CAPTURE_CHARS),
            None => "(none)".to_string(),
        };
        parts.push(format!("{i}=\"{value}\""));
    }
    parts.join(", ")
}

fn render_counts(counts: &[(String, usize)], total: usize) -> String {
    if counts.is_empty() {
        return "No files found".to_string();
    }
    let mut output: Vec<String> = counts
        .iter()
        .map(|(path, count)| format!("{path}: {count}"))
        .collect();
    output.push(format!("Total: {total}"));
    output.join("\n")
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() > max_chars {
        let byte_end = text
            .char_indices()
            .nth(max_chars)
            .map_or(text.len(), |(i, _)| i);
        format!("{}...", &text[..byte_end])
    } else {
        text.to_string()
    }
}

const fn default_true() -> bool {
    true
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "filesystem",
    name = "grep",
    summary = "Search for regex patterns within file contents.",
    description = "Search for regex patterns within file contents. Optionally report capture group values, switch to a match-count-only mode, add surrounding context lines, and control case sensitivity.",
    category = "Filesystem",
    keywords_primary = "grep, search, regex, find, pattern, content",
    side_effects = "ReadOnly"
)]
pub struct FsGrepAction {
    /// Regex pattern to search for.
    pattern: String,
    /// Base directory or file to search in (defaults to cwd).
    #[serde(default)]
    path: Option<String>,
    /// File glob filter (e.g. '*.rs'; one pattern per call, '{a,b}' brace expansion is not supported).
    #[serde(default)]
    include: Option<String>,
    /// Match case-insensitively.
    #[serde(default)]
    case_insensitive: bool,
    /// Prefix each match with its 1-based line number.
    #[serde(default = "default_true")]
    line_numbers: bool,
    /// Number of non-matching context lines to print around each match.
    #[serde(default)]
    context_lines: usize,
    /// Print only the match count instead of the matched lines.
    #[serde(default)]
    count: bool,

    #[tool(skip)]
    #[serde(skip, default = "default_sandbox")]
    sandbox: SandboxRef,
}

impl FsGrepAction {
    pub const fn new(sandbox: SandboxRef) -> Self {
        Self {
            pattern: String::new(),
            path: None,
            include: None,
            case_insensitive: false,
            line_numbers: true,
            context_lines: 0,
            count: false,
            sandbox,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let sandbox = resolve_sandbox(&self.sandbox);
        let options = GrepOptions {
            case_insensitive: self.case_insensitive,
            line_numbers: self.line_numbers,
            context_lines: self.context_lines,
            count: self.count,
        };

        grep_search(
            &self.pattern,
            self.path.as_deref(),
            self.include.as_deref(),
            &options,
            sandbox.config(),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use uuid::Uuid;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ene_fs_grep_test_{name}_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn sandbox_config(dir: &Path) -> SandboxConfig {
        SandboxConfig {
            enabled: true,
            allowed_directories: vec![dir.to_path_buf()],
            writable_directories: vec![dir.to_path_buf()],
            ..Default::default()
        }
    }

    fn dir_str(dir: &Path) -> String {
        dir.to_string_lossy().to_string()
    }

    #[tokio::test]
    async fn reports_matching_lines_with_numbers() {
        let dir = temp_dir("basic");
        std::fs::write(dir.join("sample.txt"), "alpha beta\ngamma\nalpha again\n")
            .expect("write fixture");
        let config = sandbox_config(&dir);
        let options = GrepOptions::default();

        let out = grep_search("alpha", Some(&dir_str(&dir)), None, &options, &config)
            .await
            .unwrap();

        assert!(out.contains("Found 2 matches"));
        assert!(out.contains("sample.txt:"));
        assert!(out.contains("Line 1: alpha beta"));
        assert!(out.contains("Line 3: alpha again"));
        assert!(!out.contains("gamma"));
    }

    #[tokio::test]
    async fn reports_capture_groups() {
        let dir = temp_dir("captures");
        std::fs::write(
            dir.join("log.txt"),
            "user alice logged in\nuser bob logged in\n",
        )
        .expect("write fixture");
        let config = sandbox_config(&dir);
        let options = GrepOptions::default();

        let out = grep_search(
            r"user (\w+) logged",
            Some(&dir_str(&dir)),
            None,
            &options,
            &config,
        )
        .await
        .unwrap();

        assert!(out.contains("Captures: 1=\"alice\""));
        assert!(out.contains("Captures: 1=\"bob\""));
    }

    #[tokio::test]
    async fn non_participating_groups_render_as_none() {
        let dir = temp_dir("captures_none");
        std::fs::write(dir.join("log.txt"), "hello\nhello world\n").expect("write fixture");
        let config = sandbox_config(&dir);
        let options = GrepOptions::default();

        let out = grep_search(
            r"(\w+)( \w+)?",
            Some(&dir_str(&dir)),
            None,
            &options,
            &config,
        )
        .await
        .unwrap();

        assert!(out.contains("Captures: 1=\"hello\", 2=\"(none)\""));
        assert!(out.contains("Captures: 1=\"hello\", 2=\" world\""));
    }

    #[tokio::test]
    async fn count_mode_prints_counts_only() {
        let dir = temp_dir("count");
        std::fs::write(dir.join("a.txt"), "hit\nmiss\nhit\n").expect("write fixture");
        std::fs::write(dir.join("b.txt"), "hit\n").expect("write fixture");
        let config = sandbox_config(&dir);
        let options = GrepOptions {
            count: true,
            ..Default::default()
        };

        let out = grep_search("hit", Some(&dir_str(&dir)), None, &options, &config)
            .await
            .unwrap();

        assert!(out.contains("a.txt: 2"));
        assert!(out.contains("b.txt: 1"));
        assert!(out.contains("Total: 3"));
        assert!(!out.contains("Line "));
    }

    #[tokio::test]
    async fn count_mode_with_no_matches() {
        let dir = temp_dir("count_empty");
        std::fs::write(dir.join("a.txt"), "nothing here\n").expect("write fixture");
        let config = sandbox_config(&dir);
        let options = GrepOptions {
            count: true,
            ..Default::default()
        };

        let out = grep_search("hit", Some(&dir_str(&dir)), None, &options, &config)
            .await
            .unwrap();

        assert_eq!(out, "No files found");
    }

    #[tokio::test]
    async fn case_insensitive_matching() {
        let dir = temp_dir("case");
        std::fs::write(dir.join("case.txt"), "Hello World\n").expect("write fixture");
        let config = sandbox_config(&dir);
        let options = GrepOptions::default();

        let out = grep_search("hello", Some(&dir_str(&dir)), None, &options, &config)
            .await
            .unwrap();
        assert_eq!(out, "No files found");

        let options = GrepOptions {
            case_insensitive: true,
            ..Default::default()
        };
        let out = grep_search("hello", Some(&dir_str(&dir)), None, &options, &config)
            .await
            .unwrap();
        assert!(out.contains("Line 1: Hello World"));
    }

    #[tokio::test]
    async fn context_lines_around_matches_without_duplicates() {
        let dir = temp_dir("context");
        std::fs::write(
            dir.join("ctx.txt"),
            "line1\nneedle here\nline3\nline4\nneedle again\nline6\n",
        )
        .expect("write fixture");
        let config = sandbox_config(&dir);
        let options = GrepOptions {
            context_lines: 1,
            ..Default::default()
        };

        let out = grep_search("needle", Some(&dir_str(&dir)), None, &options, &config)
            .await
            .unwrap();

        assert!(out.contains("Context 1: line1"));
        assert!(out.contains("Context 3: line3"));
        assert!(out.contains("Context 4: line4"));
        assert!(out.contains("Context 6: line6"));
        assert!(out.contains("Line 2: needle here"));
        assert!(out.contains("Line 5: needle again"));
        assert!(!out.contains("Context 2:"));
        assert!(!out.contains("Context 5:"));
    }

    #[tokio::test]
    async fn adjacent_matches_share_no_context_lines() {
        let dir = temp_dir("adjacent");
        std::fs::write(dir.join("adj.txt"), "a\nb\nc\nd\n").expect("write fixture");
        let config = sandbox_config(&dir);
        let options = GrepOptions {
            context_lines: 1,
            ..Default::default()
        };

        let out = grep_search("b|c", Some(&dir_str(&dir)), None, &options, &config)
            .await
            .unwrap();

        assert_eq!(out.matches("Context").count(), 2);
        assert!(out.contains("Context 1: a"));
        assert!(out.contains("Context 4: d"));
        assert!(out.contains("Line 2: b"));
        assert!(out.contains("Line 3: c"));
    }

    #[tokio::test]
    async fn line_numbers_can_be_disabled() {
        let dir = temp_dir("no_numbers");
        std::fs::write(dir.join("raw.txt"), "alpha beta\n").expect("write fixture");
        let config = sandbox_config(&dir);
        let options = GrepOptions {
            line_numbers: false,
            ..Default::default()
        };

        let out = grep_search("alpha", Some(&dir_str(&dir)), None, &options, &config)
            .await
            .unwrap();

        assert!(out.contains("  alpha beta"));
        assert!(!out.contains("Line 1:"));
    }

    #[tokio::test]
    async fn results_are_capped_at_max_results() {
        let dir = temp_dir("cap");
        std::fs::write(dir.join("many.txt"), "match\n".repeat(120)).expect("write fixture");
        let config = sandbox_config(&dir);
        let options = GrepOptions::default();

        let out = grep_search("match", Some(&dir_str(&dir)), None, &options, &config)
            .await
            .unwrap();

        assert!(out.contains("Found 120 matches (showing first 100)"));
        assert!(out.contains("showing 100 of 120 matches (20 hidden)"));
        assert_eq!(out.matches("Line ").count(), 100);
    }

    #[tokio::test]
    async fn count_mode_is_not_capped_at_max_results() {
        let dir = temp_dir("count_cap");
        std::fs::write(dir.join("many.txt"), "match\n".repeat(120)).expect("write fixture");
        let config = sandbox_config(&dir);
        let options = GrepOptions {
            count: true,
            ..Default::default()
        };

        let out = grep_search("match", Some(&dir_str(&dir)), None, &options, &config)
            .await
            .unwrap();

        assert!(out.contains("Total: 120"));
        assert!(!out.contains("hidden"));
        assert!(!out.contains("showing first"));
    }

    #[tokio::test]
    async fn invalid_pattern_is_an_error() {
        let dir = temp_dir("invalid");
        let config = sandbox_config(&dir);
        let options = GrepOptions::default();

        let err = grep_search("([", Some(&dir_str(&dir)), None, &options, &config)
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("Invalid regex pattern"), "{err}");
    }

    #[tokio::test]
    async fn empty_pattern_is_an_error() {
        let dir = temp_dir("empty");
        let config = sandbox_config(&dir);
        let options = GrepOptions::default();

        let err = grep_search("", Some(&dir_str(&dir)), None, &options, &config)
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("pattern is required"), "{err}");
    }
}
