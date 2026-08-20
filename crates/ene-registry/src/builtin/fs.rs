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
            json!({"type":"object","properties":{"path":{"type":"string"},"text":{"type":"string"}},"required":["path","text"],"additionalProperties":false}),
            vec!["fs.write".to_owned()],
        ),
        spec(
            "fs.edit",
            "Replace text in a workspace file",
            json!({"type":"object","properties":{"path":{"type":"string"},"old":{"type":"string"},"new":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["path","old","new"],"additionalProperties":false}),
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
            "Search file contents in the workspace",
            json!({"type":"object","properties":{"path":{"type":"string"},"query":{"type":"string"},"max":{"type":"integer"}},"required":["query"],"additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "fs.patch",
            "Apply a unified diff to a workspace file",
            json!({"type":"object","properties":{"path":{"type":"string"},"diff":{"type":"string"}},"required":["path","diff"],"additionalProperties":false}),
            vec!["fs.write".to_owned()],
        ),
        spec(
            "fs.undo",
            "Undo the last write/edit/patch this job made",
            json!({"type":"object","additionalProperties":false}),
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
        "fs.undo" => undo(),
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
    record_undo(&path)?;
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
    record_undo(&path)?;
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
    let re = regex::Regex::new(query).map_err(|err| err.to_string())?;
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
    walk_search(&root, &re, &mut matches, max)?;
    Ok(json!({ "matches": matches }))
}

fn walk_search(
    path: &Path,
    re: &regex::Regex,
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
            walk_search(&ent.path(), re, matches, max)?;
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
        if re.is_match(line) {
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
    record_undo(&path)?;
    std::fs::write(&path, next).map_err(|err| err.to_string())?;
    Ok(json!({ "ok": true }))
}

fn apply_unified_diff(body: &str, diff: &str) -> Result<String, String> {
    let mut lines: Vec<String> = body.lines().map(str::to_owned).collect();
    let mut old_line = 0_usize;
    for raw in diff.lines() {
        if raw.starts_with("@@") {
            old_line = parse_hunk_start(raw)?;
            continue;
        }
        if raw.starts_with("---") || raw.starts_with("+++") || raw.starts_with("diff ") {
            continue;
        }
        let Some(first) = raw.chars().next() else {
            continue;
        };
        match first {
            ' ' => old_line += 1,
            '-' => {
                if old_line == 0 || old_line > lines.len() {
                    return Err("patch context missed".to_owned());
                }
                lines.remove(old_line - 1);
            }
            '+' => {
                let insert_at = old_line.saturating_sub(1).min(lines.len());
                lines.insert(insert_at, raw[1..].to_owned());
                old_line += 1;
            }
            _ => {}
        }
    }
    let mut next = lines.join("\n");
    if body.ends_with('\n') {
        next.push('\n');
    }
    Ok(next)
}

fn parse_hunk_start(header: &str) -> Result<usize, String> {
    let Some(rest) = header.strip_prefix("@@ -") else {
        return Err("bad hunk header".to_owned());
    };
    let num: String = rest.chars().take_while(char::is_ascii_digit).collect();
    num.parse::<usize>()
        .map_err(|_| "bad hunk header".to_owned())
}

fn undo() -> Result<Value, String> {
    let journal = journal_path()?;
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

fn record_undo(path: &Path) -> Result<(), String> {
    let journal = journal_path()?;
    let prev = std::fs::read_to_string(path).ok();
    let entry = json!({
        "path": path.display().to_string(),
        "prev": prev,
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

fn journal_path() -> Result<PathBuf, String> {
    let dir = workspace()?.join(".ene");
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    Ok(dir.join("undo.jsonl"))
}

fn workspace() -> Result<PathBuf, String> {
    std::env::var("ENE_WORKSPACE")
        .map(PathBuf::from)
        .map_err(|_| "ENE_WORKSPACE is not set".to_owned())
}

fn resolve(path: &str, create_parent: bool) -> Result<PathBuf, String> {
    let Ok(workspace) = std::env::var("ENE_WORKSPACE") else {
        return Ok(PathBuf::from(path));
    };
    crate::pipeline::confine_tool_path(Path::new(&workspace), Path::new(path), create_parent)
        .map_err(|err| err.to_string())
}
