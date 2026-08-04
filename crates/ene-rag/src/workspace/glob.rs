//! Glob matching for ignore rules.
//!
//! Supports `*` (within a path segment), `?` (single char within a segment),
//! and `**` (across segments). A pattern without `/` matches the basename
//! only (e.g. `*.gguf`); a trailing `/**` also matches the directory itself
//! (e.g. `target/**` matches `target` and everything under it).

/// Whether `path` (relative to a scan root, `/`-separated) matches `pattern`.
pub fn glob_matches(pattern: &str, path: &str) -> bool {
    let Some(re) = glob_regex(pattern) else {
        return false;
    };
    re.is_match(path)
}

fn glob_regex(pattern: &str) -> Option<regex::Regex> {
    let mut pattern = pattern.trim();
    if pattern.is_empty() {
        return None;
    }

    let mut out = String::with_capacity(pattern.len() * 2);
    out.push('^');

    let has_slash = pattern.contains('/');
    if !has_slash {
        // Basename patterns match any segment.
        out.push_str("(?:.*/)?");
    } else if let Some(rest) = pattern.strip_prefix("**/") {
        // Leading `**/` is optional so `**/.env` matches at the root too.
        out.push_str("(?:.*/)?");
        pattern = rest;
    }

    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    // `**` crosses segments.
                    out.push_str(".*");
                } else {
                    out.push_str("[^/]*");
                }
            }
            '?' => out.push_str("[^/]"),
            '.' | '+' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            other => out.push(other),
        }
    }

    // A trailing `/**` matches the directory itself as well as its contents.
    if out.ends_with("/.*") {
        out.truncate(out.len() - 3);
        out.push_str("(?:/.*)?");
    }
    out.push('$');

    regex::Regex::new(&out).ok()
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
