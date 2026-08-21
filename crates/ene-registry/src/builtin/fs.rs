use super::{arg_str, spec};
use ene_plugin_ipc::ToolSpecWire;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(super) fn specs() -> Vec<ToolSpecWire> {
    vec![
        spec(
            "fs.read",
            "Read a UTF-8 file in the workspace",
            json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "fs.write",
            "Write a UTF-8 file in the workspace",
            json!({"type":"object","properties":{"path":{"type":"string"},"text":{"type":"string"},"job_id":{"type":"string"}},"required":["path","text"],"additionalProperties":false}),
            vec!["fs.write".to_owned()],
        ),
        spec(
            "fs.edit",
            "Replace text in a workspace file",
            json!({"type":"object","properties":{"path":{"type":"string"},"old":{"type":"string"},"new":{"type":"string"},"replace_all":{"type":"boolean"},"job_id":{"type":"string"}},"required":["path","old","new"],"additionalProperties":false}),
            vec!["fs.write".to_owned()],
        ),
        spec(
            "fs.list",
            "List a workspace directory",
            json!({"type":"object","properties":{"path":{"type":"string"}},"additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "fs.search",
            "Search file contents in the workspace (literal by default)",
            json!({"type":"object","properties":{"path":{"type":"string"},"query":{"type":"string"},"max":{"type":"integer"},"regex":{"type":"boolean"}},"required":["query"],"additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "fs.patch",
            "Apply a unified diff to a workspace file",
            json!({"type":"object","properties":{"path":{"type":"string"},"diff":{"type":"string"},"job_id":{"type":"string"}},"required":["path","diff"],"additionalProperties":false}),
            vec!["fs.write".to_owned()],
        ),
        spec(
            "fs.undo",
            "Undo the last write/edit/patch this job made",
            json!({"type":"object","properties":{"job_id":{"type":"string"}},"additionalProperties":false}),
            vec!["fs.write".to_owned()],
        ),
    ]
}

pub(super) fn execute(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "fs.read" => read(args),
        "fs.write" => write(args),
        "fs.edit" => edit(args),
        "fs.list" => list(args),
        "fs.search" => search(args),
        "fs.patch" => patch(args),
        "fs.undo" => undo(args),
        other => Err(format!("unknown builtin {other}")),
    }
}

fn read(args: &Value) -> Result<Value, String> {
    let path = resolve(arg_str(args, "path")?, false)?;
    let body = std::fs::read_to_string(&path).map_err(|err| err.to_string())?;
    Ok(json!({ "text": body }))
}

fn write(args: &Value) -> Result<Value, String> {
    let path = resolve(arg_str(args, "path")?, true)?;
    let text = arg_str(args, "text")?;
    record_undo(args, &path)?;
    std::fs::write(&path, text).map_err(|err| err.to_string())?;
    Ok(json!({ "ok": true }))
}

fn edit(args: &Value) -> Result<Value, String> {
    let path = resolve(arg_str(args, "path")?, false)?;
    let old = arg_str(args, "old")?;
    let new = arg_str(args, "new")?;
    let replace_all = args
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let body = std::fs::read_to_string(&path).map_err(|err| err.to_string())?;
    if !body.contains(old) {
        return Err("old text not found".to_owned());
    }
    record_undo(args, &path)?;
    let next = if replace_all {
        body.replace(old, new)
    } else {
        body.replacen(old, new, 1)
    };
    std::fs::write(&path, next).map_err(|err| err.to_string())?;
    Ok(json!({ "ok": true }))
}

fn list(args: &Value) -> Result<Value, String> {
    let raw = args.get("path").and_then(Value::as_str).unwrap_or("");
    let path = if raw.is_empty() {
        workspace()?
    } else {
        resolve(raw, false)?
    };
    let mut entries = Vec::new();
    let rd = std::fs::read_dir(&path).map_err(|err| err.to_string())?;
    for ent in rd {
        let ent = ent.map_err(|err| err.to_string())?;
        let meta = ent.metadata().map_err(|err| err.to_string())?;
        let kind = if meta.is_dir() { "dir" } else { "file" };
        entries.push(json!({
            "name": ent.file_name().to_string_lossy(),
            "kind": kind,
            "bytes": meta.len(),
        }));
    }
    Ok(json!({ "entries": entries }))
}

fn search(args: &Value) -> Result<Value, String> {
    let query = arg_str(args, "query")?;
    let needle = if args.get("regex").and_then(Value::as_bool).unwrap_or(false) {
        Needle::Regex(regex::Regex::new(query).map_err(|err| err.to_string())?)
    } else {
        Needle::Literal(query.to_owned())
    };
    let raw = args.get("path").and_then(Value::as_str).unwrap_or("");
    let root = if raw.is_empty() {
        workspace()?
    } else {
        resolve(raw, false)?
    };
    let max = args
        .get("max")
        .and_then(Value::as_u64)
        .unwrap_or(50)
        .min(200) as usize;
    let mut matches = Vec::new();
    walk_search(&root, &needle, &mut matches, max)?;
    Ok(json!({ "matches": matches }))
}

enum Needle {
    Literal(String),
    Regex(regex::Regex),
}

impl Needle {
    fn is_match(&self, line: &str) -> bool {
        match self {
            Self::Literal(needle) => line.contains(needle),
            Self::Regex(re) => re.is_match(line),
        }
    }
}

fn walk_search(
    path: &Path,
    needle: &Needle,
    matches: &mut Vec<Value>,
    max: usize,
) -> Result<(), String> {
    if matches.len() >= max {
        return Ok(());
    }
    if path.is_dir() {
        if path.file_name().is_some_and(|name| name == ".ene") {
            return Ok(());
        }
        let rd = std::fs::read_dir(path).map_err(|err| err.to_string())?;
        for ent in rd {
            let ent = ent.map_err(|err| err.to_string())?;
            walk_search(&ent.path(), needle, matches, max)?;
            if matches.len() >= max {
                break;
            }
        }
        return Ok(());
    }
    let Ok(body) = std::fs::read_to_string(path) else {
        return Ok(());
    };
    for (idx, line) in body.lines().enumerate() {
        if needle.is_match(line) {
            matches.push(json!({
                "path": path.display().to_string(),
                "line": idx + 1,
                "text": line,
            }));
            if matches.len() >= max {
                break;
            }
        }
    }
    Ok(())
}

fn patch(args: &Value) -> Result<Value, String> {
    let path = resolve(arg_str(args, "path")?, false)?;
    let diff = arg_str(args, "diff")?;
    let body = std::fs::read_to_string(&path).map_err(|err| err.to_string())?;
    let next = apply_unified_diff(&body, diff)?;
    record_undo(args, &path)?;
    std::fs::write(&path, next).map_err(|err| err.to_string())?;
    Ok(json!({ "ok": true }))
}

#[derive(Debug)]
struct Hunk {
    old_start: usize,
    ops: Vec<HunkOp>,
}

#[derive(Debug)]
enum HunkOp {
    Keep(String),
    Remove(String),
    Add(String),
}

fn apply_unified_diff(body: &str, diff: &str) -> Result<String, String> {
    let hunks = parse_hunks(diff)?;
    if hunks.is_empty() {
        return Err("diff has no hunks".to_owned());
    }
    let mut lines: Vec<String> = body.lines().map(str::to_owned).collect();
    for hunk in hunks {
        lines = apply_hunk(lines, &hunk)?;
    }
    let mut next = lines.join("\n");
    if body.ends_with('\n') {
        next.push('\n');
    }
    Ok(next)
}

fn parse_hunks(diff: &str) -> Result<Vec<Hunk>, String> {
    let mut hunks = Vec::new();
    let mut current: Option<Hunk> = None;
    for raw in diff.lines() {
        if raw.starts_with("@@") {
            if let Some(hunk) = current.take() {
                hunks.push(hunk);
            }
            current = Some(Hunk {
                old_start: parse_hunk_start(raw)?,
                ops: Vec::new(),
            });
            continue;
        }
        if raw.starts_with("---") || raw.starts_with("+++") || raw.starts_with("diff ") {
            continue;
        }
        if raw.starts_with('\\') {
            continue;
        }
        let Some(hunk) = current.as_mut() else {
            continue;
        };
        let Some(first) = raw.chars().next() else {
            hunk.ops.push(HunkOp::Keep(String::new()));
            continue;
        };
        let rest = raw[first.len_utf8()..].to_owned();
        match first {
            ' ' => hunk.ops.push(HunkOp::Keep(rest)),
            '-' => hunk.ops.push(HunkOp::Remove(rest)),
            '+' => hunk.ops.push(HunkOp::Add(rest)),
            _ => {}
        }
    }
    if let Some(hunk) = current {
        hunks.push(hunk);
    }
    Ok(hunks)
}

fn parse_hunk_start(header: &str) -> Result<usize, String> {
    let Some(rest) = header.strip_prefix("@@ -") else {
        return Err("bad hunk header".to_owned());
    };
    let num: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if num.is_empty() {
        return Err("bad hunk header".to_owned());
    }
    num.parse::<usize>()
        .map_err(|_| "bad hunk header".to_owned())
}

fn apply_hunk(mut lines: Vec<String>, hunk: &Hunk) -> Result<Vec<String>, String> {
    let expected: Vec<String> = hunk
        .ops
        .iter()
        .filter_map(|op| match op {
            HunkOp::Keep(text) | HunkOp::Remove(text) => Some(text.clone()),
            HunkOp::Add(_) => None,
        })
        .collect();
    let hint = hunk.old_start.saturating_sub(1);
    let idx = find_block(&lines, &expected, hint)?;
    let replacement: Vec<String> = hunk
        .ops
        .iter()
        .filter_map(|op| match op {
            HunkOp::Keep(text) | HunkOp::Add(text) => Some(text.clone()),
            HunkOp::Remove(_) => None,
        })
        .collect();
    let end = idx + expected.len();
    lines.splice(idx..end, replacement);
    Ok(lines)
}

fn find_block(lines: &[String], expected: &[String], hint: usize) -> Result<usize, String> {
    if expected.is_empty() {
        return Ok(hint.min(lines.len()));
    }
    let last = lines.len().saturating_sub(expected.len());
    let mut best = None;
    let mut best_dist = usize::MAX;
    for idx in 0..=last {
        if !matches_at(lines, expected, idx) {
            continue;
        }
        let dist = idx.abs_diff(hint);
        if dist < best_dist {
            best_dist = dist;
            best = Some(idx);
        }
    }
    best.ok_or_else(|| "patch context missed".to_owned())
}

fn matches_at(lines: &[String], expected: &[String], idx: usize) -> bool {
    lines
        .get(idx..idx + expected.len())
        .is_some_and(|slice| slice == expected)
}

fn undo(args: &Value) -> Result<Value, String> {
    let journal = journal_path(args)?;
    let raw = std::fs::read_to_string(&journal).unwrap_or_default();
    let mut lines: Vec<&str> = raw.lines().filter(|line| !line.is_empty()).collect();
    let Some(last) = lines.pop() else {
        return Err("nothing to undo".to_owned());
    };
    let entry: Value = serde_json::from_str(last).map_err(|err| err.to_string())?;
    let path = entry
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "bad undo record".to_owned())?;
    if let Some(prev) = entry.get("prev").and_then(Value::as_str) {
        std::fs::write(path, prev).map_err(|err| err.to_string())?;
    } else {
        drop(std::fs::remove_file(path));
    }
    let rest = if lines.is_empty() {
        String::new()
    } else {
        let mut out = lines.join("\n");
        out.push('\n');
        out
    };
    std::fs::write(&journal, rest).map_err(|err| err.to_string())?;
    Ok(json!({ "ok": true, "path": path }))
}

fn record_undo(args: &Value, path: &Path) -> Result<(), String> {
    let journal = journal_path(args)?;
    let prev = std::fs::read_to_string(path).ok();
    let entry = json!({
        "path": path.display().to_string(),
        "prev": prev,
        "job_id": job_key(args),
    });
    let mut line = serde_json::to_string(&entry).map_err(|err| err.to_string())?;
    line.push('\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&journal)
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(line.as_bytes())
        })
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn journal_path(args: &Value) -> Result<PathBuf, String> {
    let dir = workspace()?.join(".ene").join("undo");
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    Ok(dir.join(format!("{}.jsonl", job_key(args))))
}

fn job_key(args: &Value) -> String {
    let raw = args
        .get("job_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| std::env::var("ENE_JOB_ID").ok())
        .unwrap_or_else(|| "_default".to_owned());
    let mut out = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        }
    }
    if out.is_empty() {
        "_default".to_owned()
    } else {
        out
    }
}

fn workspace() -> Result<PathBuf, String> {
    #[cfg(test)]
    if let Some(root) = TEST_WORKSPACE.lock().clone() {
        return Ok(root);
    }
    std::env::var("ENE_WORKSPACE")
        .map(PathBuf::from)
        .map_err(|_| "ENE_WORKSPACE is not set".to_owned())
}

#[cfg(test)]
static TEST_WORKSPACE: parking_lot::Mutex<Option<PathBuf>> = parking_lot::Mutex::new(None);

fn resolve(path: &str, create_parent: bool) -> Result<PathBuf, String> {
    let Ok(workspace) = std::env::var("ENE_WORKSPACE") else {
        return Ok(PathBuf::from(path));
    };
    crate::pipeline::confine_tool_path(Path::new(&workspace), Path::new(path), create_parent)
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::{Needle, TEST_WORKSPACE, apply_unified_diff, execute, job_key};
    use serde_json::json;

    #[test]
    fn unified_diff_replaces_matching_context() {
        let body = "alpha\nbeta\ngamma\n";
        let diff = "--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n alpha\n-beta\n+BETA\n gamma\n";
        let next = apply_unified_diff(body, diff).unwrap();
        assert_eq!(next, "alpha\nBETA\ngamma\n");
    }

    #[test]
    fn unified_diff_finds_hunk_when_line_numbers_drift() {
        let body = "keep\nalpha\nbeta\ngamma\n";
        let diff = "@@ -10,3 +10,3 @@\n alpha\n-beta\n+BETA\n gamma\n";
        let next = apply_unified_diff(body, diff).unwrap();
        assert_eq!(next, "keep\nalpha\nBETA\ngamma\n");
    }

    #[test]
    fn unified_diff_rejects_wrong_context() {
        let err =
            apply_unified_diff("alpha\nbeta\n", "@@ -1,2 +1,2 @@\n no\n-match\n").unwrap_err();
        assert!(err.contains("context missed"));
    }

    #[test]
    fn search_literal_does_not_compile_regex() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.txt"), "cost is $5+\n").unwrap();
        let found = execute(
            "fs.search",
            &json!({"path": dir.path().join("a.txt").to_string_lossy(), "query": "$5+"}),
        )
        .unwrap();
        assert_eq!(found["matches"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn undo_is_scoped_to_the_job() {
        let dir = tempfile::TempDir::new().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "one").unwrap();
        std::fs::write(&b, "two").unwrap();
        *TEST_WORKSPACE.lock() = Some(dir.path().to_path_buf());
        let result = (|| {
            execute(
                "fs.write",
                &json!({"path": a.to_string_lossy(), "text": "A", "job_id": "job-a"}),
            )?;
            execute(
                "fs.write",
                &json!({"path": b.to_string_lossy(), "text": "B", "job_id": "job-b"}),
            )?;
            execute("fs.undo", &json!({"job_id": "job-a"}))
        })();
        *TEST_WORKSPACE.lock() = None;
        result.unwrap();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "one");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "B");
        assert_eq!(job_key(&json!({"job_id": "job-a"})), "job-a");
    }

    #[test]
    fn needle_literal_vs_regex() {
        let lit = Needle::Literal("a+".to_owned());
        assert!(lit.is_match("xxa+yy"));
        assert!(!lit.is_match("aaa"));
        let re = Needle::Regex(regex::Regex::new("a+").unwrap());
        assert!(re.is_match("aaa"));
    }
}
