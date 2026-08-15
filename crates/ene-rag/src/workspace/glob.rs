//! Supports `*` (within a path segment), `?` (single char within a segment),
//! and `**` (across segments). A pattern without `/` matches the basename
//! only (e.g. `*.gguf`); a trailing `/**` also matches the directory itself
//! (e.g. `target/**` matches `target` and everything under it).

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

/// `path` is relative to a scan root and `/`-separated.
pub fn glob_matches(pattern: &str, path: &str) -> bool {
    build_matcher(pattern).is_some_and(|matcher| matcher.is_match(path))
}

fn build_matcher(pattern: &str) -> Option<GlobSet> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return None;
    }

    let mut builder = GlobSetBuilder::new();
    // Basename patterns match any segment; globset needs an explicit `**/`
    // prefix to express that.
    let normalized = if pattern.contains('/') {
        pattern.to_string()
    } else {
        format!("**/{pattern}")
    };
    builder.add(build_glob(&normalized)?);
    if let Some(prefix) = normalized.strip_suffix("/**")
        && !prefix.is_empty()
    {
        builder.add(build_glob(prefix)?);
    }
    builder.build().ok()
}

fn build_glob(pattern: &str) -> Option<globset::Glob> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .backslash_escape(false)
        .build()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::glob_matches;

    #[test]
    fn basename_patterns_match_any_depth() {
        assert!(glob_matches("*.gguf", "model.gguf"));
        assert!(glob_matches("*.gguf", "assets/models/model.gguf"));
        assert!(!glob_matches("*.gguf", "model.gguf.bak"));
        assert!(!glob_matches("*.rs", "src/lib.txt"));
    }

    #[test]
    fn leading_double_star_is_optional() {
        assert!(glob_matches("**/.env", ".env"));
        assert!(glob_matches("**/.env", "sub/dir/.env"));
        assert!(glob_matches("**/.env.*", "sub/.env.local"));
        assert!(!glob_matches("**/.env", "sub/notenv"));
    }

    #[test]
    fn trailing_double_star_matches_directory_and_contents() {
        assert!(glob_matches("target/**", "target"));
        assert!(glob_matches("target/**", "target/debug/app"));
        assert!(!glob_matches("target/**", "src/target2/app"));
    }

    #[test]
    fn question_mark_matches_single_char() {
        assert!(glob_matches("file?.txt", "file1.txt"));
        assert!(!glob_matches("file?.txt", "file12.txt"));
    }

    #[test]
    fn invalid_pattern_never_matches() {
        assert!(!glob_matches("[", "anything"));
    }
}
