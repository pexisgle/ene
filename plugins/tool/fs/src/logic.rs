use base64::{Engine, engine::general_purpose::STANDARD as B64};
use ene_plugin_ipc::{BrokerClient, BrokerRequest, BrokerResponse, ToolSpecWire};
use ene_registry::{arg_str, spec};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_UNDO_BYTES: usize = 1024 * 1024;

static PATH_LOCKS: LazyLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static TEMP_SUFFIX: AtomicU64 = AtomicU64::new(1);

pub(crate) fn specs() -> Vec<ToolSpecWire> {
    let expected_hash = json!({"type":"string"});
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
            json!({"type":"object","properties":{"path":{"type":"string"},"text":{"type":"string"},"job_id":{"type":"string"},"expected_hash":expected_hash.clone()},"required":["path","text"],"additionalProperties":false}),
            vec!["fs.write".to_owned()],
        ),
        spec(
            "fs.edit",
            "Replace text in a workspace file",
            json!({"type":"object","properties":{"path":{"type":"string"},"old":{"type":"string"},"new":{"type":"string"},"replace_all":{"type":"boolean"},"job_id":{"type":"string"},"expected_hash":expected_hash.clone()},"required":["path","old","new"],"additionalProperties":false}),
            vec!["fs.write".to_owned()],
        ),
        spec(
            "fs.list",
            "List a workspace directory",
            json!({"type":"object","properties":{"path":{"type":"string"}},"additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "fs.glob",
            "List workspace paths matching a glob pattern",
            json!({"type":"object","properties":{"pattern":{"type":"string"},"max":{"type":"integer"},"include_hidden":{"type":"boolean"}},"required":["pattern"],"additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "fs.delete",
            "Delete a workspace file or empty directory",
            json!({"type":"object","properties":{"path":{"type":"string"},"job_id":{"type":"string"}},"required":["path"],"additionalProperties":false}),
            vec!["fs.delete".to_owned()],
        ),
        spec(
            "fs.search",
            "Search file contents through the host ripgrep broker (literal by default)",
            json!({
                "type":"object",
                "properties":{
                    "path":{"type":"string"},
                    "query":{"type":"string"},
                    "regex":{"type":"boolean"},
                    "case_insensitive":{"type":"boolean"},
                    "include":{"type":"string"},
                    "context_lines":{"type":"integer","minimum":0,"maximum":10},
                    "count":{"type":"boolean"},
                    "max":{"type":"integer","minimum":1,"maximum":200}
                },
                "required":["query"],
                "additionalProperties":false
            }),
            Vec::new(),
        ),
        spec(
            "fs.patch",
            "Apply a unified diff to a workspace file",
            json!({"type":"object","properties":{"path":{"type":"string"},"diff":{"type":"string"},"job_id":{"type":"string"},"expected_hash":expected_hash},"required":["path","diff"],"additionalProperties":false}),
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

pub(crate) fn execute(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "fs.read" => read(args),
        "fs.write" => write(args),
        "fs.edit" => edit(args),
        "fs.list" => list(args),
        "fs.glob" => glob(args),
        "fs.delete" => delete(args),
        "fs.search" => search(args),
        "fs.patch" => patch(args),
        "fs.undo" => undo(args),
        other => Err(format!("unknown builtin {other}")),
    }
}

fn broker_search(args: &Value) -> Result<Value, String> {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(broker_search_async(args))
    })
}

async fn broker_search_async(args: &Value) -> Result<Value, String> {
    let mut client = BrokerClient::from_env()
        .await
        .map_err(|err| format!("broker unavailable: {err}"))?;
    let response = client
        .call(BrokerRequest::Hello {
            token: std::env::var("ENE_PLUGIN_SPAWN_TOKEN")
                .map_err(|_| "ENE_PLUGIN_SPAWN_TOKEN is not set".to_owned())?,
        })
        .await
        .map_err(|err| format!("broker hello failed: {err}"))?;
    if matches!(response, BrokerResponse::Error { .. }) {
        return Err("broker hello rejected".to_owned());
    }

    let query = arg_str(args, "query")?;
    let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let request = BrokerRequest::FsSearch {
        path: path.to_owned(),
        query: query.to_owned(),
        regex: args.get("regex").and_then(Value::as_bool).unwrap_or(false),
        case_insensitive: args
            .get("case_insensitive")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        include: args
            .get("include")
            .and_then(Value::as_str)
            .map(str::to_owned),
        context_lines: u32::try_from(
            args.get("context_lines")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        )
        .unwrap_or(0)
        .min(10),
        count: args.get("count").and_then(Value::as_bool).unwrap_or(false),
        max: u32::try_from(args.get("max").and_then(Value::as_u64).unwrap_or(50))
            .unwrap_or(50)
            .min(200),
    };

    match client.call(request).await {
        Ok(BrokerResponse::FsSearchOk { matches }) => Ok(json!({ "matches": matches })),
        Ok(BrokerResponse::Error { message, .. }) => Err(message),
        Err(err) => Err(err.to_string()),
        Ok(_) => Err("unexpected broker response".to_owned()),
    }
}

fn read(args: &Value) -> Result<Value, String> {
    let path = resolve(arg_str(args, "path")?, false)?;
    let bytes = std::fs::read(&path).map_err(|err| err.to_string())?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "file is not valid UTF-8".to_owned())?;
    Ok(json!({ "text": text, "hash": hash_bytes(&bytes) }))
}

fn write(args: &Value) -> Result<Value, String> {
    let path = resolve(arg_str(args, "path")?, true)?;
    let text = arg_str(args, "text")?;
    let job_id = job_key(args);
    let expected = optional_hash(args);
    with_path_lock(&path, || {
        check_precondition(&path, expected)?;
        record_undo(&job_id, &path)?;
        let bytes = text.as_bytes();
        if let Err(err) = atomic_write_bytes(&path, bytes) {
            drop(pop_journal_entry(&job_id));
            return Err(err);
        }
        Ok(json!({
            "ok": true,
            "job_id": job_id,
            "hash": hash_bytes(bytes),
        }))
    })
}

fn edit(args: &Value) -> Result<Value, String> {
    let path = resolve(arg_str(args, "path")?, false)?;
    let old = arg_str(args, "old")?;
    let new = arg_str(args, "new")?;
    let replace_all = args
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let job_id = job_key(args);
    let expected = optional_hash(args);
    with_path_lock(&path, || {
        check_precondition(&path, expected)?;
        let body = std::fs::read_to_string(&path).map_err(|err| err.to_string())?;
        let next = apply_edit(&body, old, new, replace_all)?;
        record_undo(&job_id, &path)?;
        if let Err(err) = atomic_write_bytes(&path, next.as_bytes()) {
            drop(pop_journal_entry(&job_id));
            return Err(err);
        }
        Ok(json!({
            "ok": true,
            "job_id": job_id,
            "hash": hash_bytes(next.as_bytes()),
        }))
    })
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

const MAX_GLOB_RESULTS: usize = 500;

fn glob(args: &Value) -> Result<Value, String> {
    let pattern = arg_str(args, "pattern")?;
    deny_glob_escape(pattern)?;
    let include_hidden = args
        .get("include_hidden")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let max = args
        .get("max")
        .and_then(Value::as_u64)
        .unwrap_or(200)
        .min(MAX_GLOB_RESULTS as u64) as usize;
    let root = workspace()?;
    let mut paths = Vec::new();
    walk_glob(&root, &root, pattern, include_hidden, &mut paths, max)?;
    paths.sort();
    Ok(json!({
        "paths": paths,
        "truncated": paths.len() >= max,
    }))
}

fn deny_glob_escape(pattern: &str) -> Result<(), String> {
    let path = Path::new(pattern);
    if path.is_absolute() {
        return Err("glob pattern must be workspace-relative".to_owned());
    }
    for component in path.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err("glob pattern must not contain ..".to_owned());
        }
    }
    Ok(())
}

fn walk_glob(
    root: &Path,
    dir: &Path,
    pattern: &str,
    include_hidden: bool,
    out: &mut Vec<String>,
    max: usize,
) -> Result<(), String> {
    if out.len() >= max {
        return Ok(());
    }
    let rd = std::fs::read_dir(dir).map_err(|err| err.to_string())?;
    for ent in rd {
        if out.len() >= max {
            break;
        }
        let ent = ent.map_err(|err| err.to_string())?;
        let path = ent.path();
        let name = ent.file_name();
        let name = name.to_string_lossy();
        if name == ".ene" {
            continue;
        }
        if !include_hidden && name.starts_with('.') {
            continue;
        }
        let meta = std::fs::symlink_metadata(&path).map_err(|err| err.to_string())?;
        if meta.file_type().is_symlink() {
            // Directory links (and Windows junctions) are never followed.
            let Ok(canonical) = path.canonicalize() else {
                continue;
            };
            if canonical.is_dir() || !canonical.starts_with(root) {
                continue;
            }
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|_| "path escapes workspace".to_owned())?
            .to_string_lossy()
            .replace('\\', "/");
        if glob_match(pattern, &rel) {
            out.push(rel.clone());
        }
        if meta.is_dir() && !meta.file_type().is_symlink() {
            walk_glob(root, &path, pattern, include_hidden, out, max)?;
        }
    }
    Ok(())
}

fn glob_match(pattern: &str, rel: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').filter(|part| !part.is_empty()).collect();
    let path: Vec<&str> = rel.split('/').filter(|part| !part.is_empty()).collect();
    glob_components(&pat, &path)
}

fn glob_components(pat: &[&str], path: &[&str]) -> bool {
    match (pat.first().copied(), path.first().copied()) {
        (None, None) => true,
        (Some("**"), _) => {
            glob_components(&pat[1..], path)
                || (!path.is_empty() && glob_components(pat, &path[1..]))
        }
        (Some(seg), Some(name)) if glob_segment(seg, name) => {
            glob_components(&pat[1..], &path[1..])
        }
        _ => false,
    }
}

fn glob_segment(pattern: &str, name: &str) -> bool {
    glob_stars(pattern.as_bytes(), name.as_bytes())
}

fn glob_stars(pattern: &[u8], name: &[u8]) -> bool {
    match (pattern.first().copied(), name.first().copied()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            glob_stars(&pattern[1..], name) || (!name.is_empty() && glob_stars(pattern, &name[1..]))
        }
        (Some(b'?'), Some(_)) => glob_stars(&pattern[1..], &name[1..]),
        (Some(p), Some(n)) if p == n => glob_stars(&pattern[1..], &name[1..]),
        _ => false,
    }
}

fn delete(args: &Value) -> Result<Value, String> {
    let path = resolve(arg_str(args, "path")?, false)?;
    let meta = std::fs::symlink_metadata(&path).map_err(|err| err.to_string())?;
    if is_link_or_reparse(&meta) {
        return Err("refusing to delete a symlink".to_owned());
    }
    if meta.permissions().readonly() {
        return Err("path is read-only".to_owned());
    }
    let job_id = job_key(args);
    if meta.is_dir() {
        let mut rd = std::fs::read_dir(&path).map_err(|err| err.to_string())?;
        if rd.next().is_some() {
            return Err("directory is not empty".to_owned());
        }
        record_undo_delete(&job_id, &path, None, "dir")?;
        std::fs::remove_dir(&path).map_err(|err| err.to_string())?;
    } else {
        let prev = std::fs::read_to_string(&path).ok();
        record_undo_delete(&job_id, &path, prev.as_deref(), "file")?;
        std::fs::remove_file(&path).map_err(|err| err.to_string())?;
    }
    Ok(json!({ "ok": true, "job_id": job_id }))
}

fn search(args: &Value) -> Result<Value, String> {
    if std::env::var_os("ENE_BROKER_SOCKET").is_some() {
        broker_search(args)
    } else {
        fallback_search(args)
    }
}

fn fallback_search(args: &Value) -> Result<Value, String> {
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
    let job_id = job_key(args);
    let expected = optional_hash(args);
    with_path_lock(&path, || {
        check_precondition(&path, expected)?;
        let body = std::fs::read_to_string(&path).map_err(|err| err.to_string())?;
        let next = apply_unified_diff(&body, diff)?;
        record_undo(&job_id, &path)?;
        if let Err(err) = atomic_write_bytes(&path, next.as_bytes()) {
            drop(pop_journal_entry(&job_id));
            return Err(err);
        }
        Ok(json!({
            "ok": true,
            "job_id": job_id,
            "hash": hash_bytes(next.as_bytes()),
        }))
    })
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

struct LineEnding {
    sep: &'static str,
    trailing: bool,
}

fn detect_line_ending(body: &str) -> LineEnding {
    let sep = if body.contains("\r\n") { "\r\n" } else { "\n" };
    let trailing = if sep == "\r\n" {
        body.ends_with("\r\n")
    } else {
        body.ends_with('\n')
    };
    LineEnding { sep, trailing }
}

fn split_lines(body: &str, ending: &LineEnding) -> Vec<String> {
    let mut lines = Vec::new();
    let mut rest = body;
    loop {
        if let Some(idx) = rest.find(ending.sep) {
            lines.push(rest[..idx].to_owned());
            rest = &rest[idx + ending.sep.len()..];
        } else {
            lines.push(rest.to_owned());
            break;
        }
    }
    if ending.trailing && lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

fn join_lines(lines: &[String], ending: &LineEnding) -> String {
    let mut out = lines.join(ending.sep);
    if ending.trailing {
        out.push_str(ending.sep);
    }
    out
}

fn apply_unified_diff(body: &str, diff: &str) -> Result<String, String> {
    let hunks = parse_hunks(diff)?;
    if hunks.is_empty() {
        return Err("diff has no hunks".to_owned());
    }
    let ending = detect_line_ending(body);
    let mut lines = split_lines(body, &ending);
    for hunk in hunks {
        lines = apply_hunk(lines, &hunk)?;
    }
    Ok(join_lines(&lines, &ending))
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

fn apply_edit(body: &str, old: &str, new: &str, replace_all: bool) -> Result<String, String> {
    if old.is_empty() {
        return Err("old text must not be empty".to_owned());
    }
    let exact_count = body.matches(old).count();
    if exact_count > 0 {
        if replace_all {
            return Ok(body.replace(old, new));
        }
        if exact_count == 1 {
            return Ok(body.replacen(old, new, 1));
        }
        return Err("ambiguous match: old text occurs multiple times".to_owned());
    }
    let normalized_old = normalize_line_endings(old);
    let replacement = adapt_newline_style(body, new);
    let mut candidates: Vec<(usize, usize)> = find_indent_matches(body, &normalized_old)
        .into_iter()
        .chain(find_line_matches(body, &normalized_old))
        .chain(find_block_matches(body, &normalized_old))
        .collect();
    candidates.sort_unstable();
    candidates.dedup();

    let selected = match (replace_all, candidates.as_slice()) {
        (_, []) => return Err("old text not found".to_owned()),
        (false, [candidate]) => vec![*candidate],
        (false, _) => return Err("ambiguous match: old text occurs multiple times".to_owned()),
        (true, matches) => matches.to_vec(),
    };

    replace_spans(body, &replacement, selected)
}

fn adapt_newline_style(body: &str, new: &str) -> String {
    if body.contains("\r\n") {
        normalize_line_endings(new).replace('\n', "\r\n")
    } else {
        new.to_owned()
    }
}

fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n")
}

fn replace_spans(body: &str, new: &str, spans: Vec<(usize, usize)>) -> Result<String, String> {
    if !spans.is_sorted()
        || spans
            .iter()
            .zip(spans.iter().skip(1))
            .any(|(a, b)| a.1 > b.0)
    {
        return Err("overlapping replacement matches".to_owned());
    }
    let mut out = String::with_capacity(
        body.len()
            .saturating_sub(spans.iter().map(|(start, end)| end - start).sum::<usize>())
            + new.len().saturating_mul(spans.len()),
    );
    let mut cursor = 0;
    for (start, end) in spans {
        out.push_str(body.get(cursor..start).ok_or("bad replacement span")?);
        out.push_str(new);
        cursor = end;
    }
    out.push_str(body.get(cursor..).ok_or("bad replacement span")?);
    Ok(out)
}

#[derive(Clone, Copy)]
struct LineSpan {
    start: usize,
    content_end: usize,
    term_end: usize,
}

fn line_spans(body: &str) -> Vec<LineSpan> {
    let mut spans = Vec::new();
    let mut start = 0;
    for segment in body.split_inclusive('\n') {
        let content_len = segment.strip_suffix('\n').map_or(segment.len(), str::len);
        spans.push(LineSpan {
            start,
            content_end: start + content_len,
            term_end: start + segment.len(),
        });
        start += segment.len();
    }
    spans
}

fn line_text(body: &str, span: LineSpan) -> &str {
    body.get(span.start..span.content_end).unwrap_or("")
}

fn matched_line_end(body: &str, span: LineSpan, consume_terminator: bool) -> usize {
    if consume_terminator {
        return span.term_end;
    }
    let mut end = span.content_end;
    if body.as_bytes().get(end.saturating_sub(1)) == Some(&b'\r') {
        end = end.saturating_sub(1);
    }
    end
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0_usize; b.len() + 1];
    for i in 1..=a.len() {
        current[0] = i;
        for j in 1..=b.len() {
            current[j] = if a[i - 1] == b[j - 1] {
                previous[j - 1]
            } else {
                1 + previous[j - 1].min(previous[j]).min(current[j - 1])
            };
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

fn find_indent_matches(body: &str, old: &str) -> Vec<(usize, usize)> {
    let consume_terminator = old.ends_with('\n');
    let old_lines: Vec<&str> = old.strip_suffix('\n').unwrap_or(old).split('\n').collect();
    let lines = line_spans(body);
    let mut matches = Vec::new();
    for i in 0..=lines.len().saturating_sub(old_lines.len()) {
        let window = &lines[i..i + old_lines.len()];
        if window.iter().zip(&old_lines).all(|(line, expected)| {
            line_text(body, *line).trim_start().trim_end_matches('\r') == expected.trim()
        }) {
            matches.push((
                window[0].start,
                matched_line_end(body, window[window.len() - 1], consume_terminator),
            ));
        }
    }
    matches
}

fn find_line_matches(body: &str, old: &str) -> Vec<(usize, usize)> {
    let consume_terminator = old.ends_with('\n');
    let old_lines: Vec<&str> = old.strip_suffix('\n').unwrap_or(old).split('\n').collect();
    let lines = line_spans(body);
    let mut matches = Vec::new();
    if old_lines.iter().any(|line| line.is_empty()) {
        return matches;
    }
    for i in 0..=lines.len().saturating_sub(old_lines.len()) {
        let window = &lines[i..i + old_lines.len()];
        if window
            .iter()
            .zip(&old_lines)
            .all(|(line, expected)| line_text(body, *line).trim_end_matches('\r') == *expected)
        {
            matches.push((
                window[0].start,
                matched_line_end(body, window[window.len() - 1], consume_terminator),
            ));
        }
    }
    matches
}

fn block_similarity(body: &str, lines: &[LineSpan], old_lines: &[&str]) -> Option<f64> {
    let inner_count = old_lines.len().saturating_sub(2);
    if inner_count == 0 || lines.len() < 3 {
        return None;
    }
    let lines_to_check = inner_count.min(lines.len() - 2);
    let mut score = 0.0;
    for idx in 1..=lines_to_check {
        let actual = line_text(body, lines[idx]).trim();
        let expected = old_lines[idx].trim();
        let distance = levenshtein(actual, expected);
        let max_len = actual.chars().count().max(expected.chars().count());
        if max_len == 0 {
            continue;
        }
        score += 1.0 - distance as f64 / max_len as f64;
    }
    Some(score / lines_to_check as f64)
}

fn find_block_matches(body: &str, old: &str) -> Vec<(usize, usize)> {
    let consume_terminator = old.ends_with('\n');
    let old_lines: Vec<&str> = old.split('\n').filter(|line| !line.is_empty()).collect();
    if old_lines.len() < 3 {
        return Vec::new();
    }
    let first = old_lines[0].trim();
    let last = old_lines[old_lines.len() - 1].trim();
    let lines = line_spans(body);
    let mut matches = Vec::new();
    for (start_idx, start_line) in lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line_text(body, **line).trim() == first)
    {
        let Some(end_idx) = lines[start_idx + 2..]
            .iter()
            .position(|line| line_text(body, *line).trim() == last)
            .map(|rel| rel + start_idx + 2)
        else {
            continue;
        };
        let window = &lines[start_idx..=end_idx];
        if block_similarity(body, window, &old_lines).is_some_and(|score| score >= 0.75) {
            matches.push((
                start_line.start,
                matched_line_end(body, lines[end_idx], consume_terminator),
            ));
        }
    }
    matches
}

fn undo(args: &Value) -> Result<Value, String> {
    let job_id = explicit_job_key(args).ok_or_else(|| "job_id is required".to_owned())?;
    let journal = journal_path(&job_id)?;
    let raw = std::fs::read_to_string(&journal).unwrap_or_default();
    let mut lines: Vec<&str> = raw.lines().filter(|line| !line.is_empty()).collect();
    let Some(last) = lines.pop() else {
        return Err("nothing to undo".to_owned());
    };
    let entry: Value = serde_json::from_str(last).map_err(|err| err.to_string())?;
    let path_raw = entry
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "bad undo record".to_owned())?;
    if entry
        .get("too_large")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err("undo refused: prior body exceeds journal size cap".to_owned());
    }
    let path = PathBuf::from(path_raw);
    if entry.get("op").and_then(Value::as_str) == Some("delete") {
        if entry.get("kind").and_then(Value::as_str) == Some("dir") {
            std::fs::create_dir_all(&path).map_err(|err| err.to_string())?;
        } else {
            let prev = entry.get("prev").and_then(Value::as_str).unwrap_or("");
            std::fs::write(&path, prev).map_err(|err| err.to_string())?;
        }
    } else {
        let existed = entry
            .get("existed")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        if existed {
            let prev_b64 = entry
                .get("prev_b64")
                .and_then(Value::as_str)
                .ok_or_else(|| "bad undo record".to_owned())?;
            let bytes = B64
                .decode(prev_b64)
                .map_err(|err| format!("bad undo record: {err}"))?;
            with_path_lock(&path, || atomic_write_bytes(&path, &bytes))?;
        } else {
            with_path_lock(&path, || {
                if path.exists() {
                    std::fs::remove_file(&path).map_err(|err| err.to_string())?;
                }
                Ok(())
            })?;
        }
    }
    let rest = if lines.is_empty() {
        String::new()
    } else {
        let mut out = lines.join("\n");
        out.push('\n');
        out
    };
    std::fs::write(&journal, rest).map_err(|err| err.to_string())?;
    Ok(json!({ "ok": true, "path": path_raw }))
}

fn record_undo(job_id: &str, path: &Path) -> Result<(), String> {
    if is_secret_path(path) {
        return Ok(());
    }
    let entry = if path.exists() {
        let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
        if bytes.len() > MAX_UNDO_BYTES {
            json!({
                "path": path.display().to_string(),
                "existed": true,
                "too_large": true,
            })
        } else {
            json!({
                "path": path.display().to_string(),
                "existed": true,
                "prev_b64": B64.encode(bytes),
            })
        }
    } else {
        json!({
            "path": path.display().to_string(),
            "existed": false,
        })
    };
    append_journal(job_id, &entry)
}

fn append_journal(job_id: &str, entry: &Value) -> Result<(), String> {
    let journal = journal_path(job_id)?;
    let mut line = serde_json::to_string(entry).map_err(|err| err.to_string())?;
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

fn pop_journal_entry(job_id: &str) -> Result<(), String> {
    let journal = journal_path(job_id)?;
    let raw = std::fs::read_to_string(&journal).unwrap_or_default();
    let mut lines: Vec<&str> = raw.lines().filter(|line| !line.is_empty()).collect();
    if lines.pop().is_none() {
        return Ok(());
    }
    let rest = if lines.is_empty() {
        String::new()
    } else {
        let mut out = lines.join("\n");
        out.push('\n');
        out
    };
    std::fs::write(&journal, rest).map_err(|err| err.to_string())
}

fn record_undo_delete(
    job_id: &str,
    path: &Path,
    prev: Option<&str>,
    kind: &str,
) -> Result<(), String> {
    if is_secret_path(path) {
        return Ok(());
    }
    let entry = json!({
        "path": path.display().to_string(),
        "prev": prev,
        "job_id": job_id,
        "op": "delete",
        "kind": kind,
    });
    append_journal(job_id, &entry)
}

fn is_link_or_reparse(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        meta.file_type().is_symlink() || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

fn is_secret_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    if name == ".env" || name == "vault.bin" {
        return true;
    }
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pem") || ext.eq_ignore_ascii_case("key"))
}

fn journal_path(job_id: &str) -> Result<PathBuf, String> {
    let dir = workspace()?.join(".ene").join("undo");
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    Ok(dir.join(format!("{job_id}.jsonl")))
}

fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn optional_hash(args: &Value) -> Option<&str> {
    args.get("expected_hash")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn check_precondition(path: &Path, expected: Option<&str>) -> Result<(), String> {
    let Some(expected) = expected else {
        return Ok(());
    };
    if !path.exists() {
        return Err("stale precondition: file does not exist".to_owned());
    }
    let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
    let current = hash_bytes(&bytes);
    if current != expected {
        return Err(format!(
            "stale precondition: file changed (expected {expected}, found {current})"
        ));
    }
    Ok(())
}

fn lock_key(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return path.canonicalize().map_err(|err| err.to_string());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "path has no parent directory".to_owned())?;
    let file_name = path
        .file_name()
        .ok_or_else(|| "path has no file name".to_owned())?;
    let canon_parent = parent.canonicalize().map_err(|err| err.to_string())?;
    Ok(canon_parent.join(file_name))
}

fn with_path_lock<T>(path: &Path, action: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let key = lock_key(path)?;
    let lock = {
        let mut locks = PATH_LOCKS.lock();
        locks
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    let _guard = lock.lock();
    action()
}

fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "path has no parent directory".to_owned())?;
    std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    let suffix = TEMP_SUFFIX.fetch_add(1, Ordering::Relaxed);
    let temp_path = parent.join(format!(".ene-write-{}-{suffix}.tmp", std::process::id()));
    let result = write_temp_then_rename(path, &temp_path, bytes);
    if result.is_err() {
        drop(std::fs::remove_file(&temp_path));
    }
    result
}

fn write_temp_then_rename(path: &Path, temp_path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = std::fs::File::create(temp_path).map_err(|err| err.to_string())?;
    file.write_all(bytes).map_err(|err| err.to_string())?;
    preserve_permissions(path, &file);
    drop(file);
    std::fs::rename(temp_path, path).map_err(|err| err.to_string())
}

/// Copies mode bits from an existing `src` onto the temp file before rename.
///
/// A missing source (first write) or any metadata error is ignored so the temp
/// file keeps the default mode. Without this, replacing an executable via
/// rename would drop `+x` (and other Unix mode bits).
#[cfg(unix)]
fn preserve_permissions(src: &Path, dst: &std::fs::File) {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let Ok(meta) = std::fs::metadata(src) else {
        return;
    };
    let perms = std::fs::Permissions::from_mode(meta.mode() & 0o7777);
    drop(dst.set_permissions(perms));
}

#[cfg(not(unix))]
fn preserve_permissions(_src: &Path, _dst: &std::fs::File) {}

fn job_key(args: &Value) -> String {
    explicit_job_key(args).unwrap_or_else(unique_job_key)
}

fn explicit_job_key(args: &Value) -> Option<String> {
    let raw = args
        .get("job_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| std::env::var("ENE_JOB_ID").ok())?;
    let mut out = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        }
    }
    (!out.is_empty()).then_some(out)
}

fn unique_job_key() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!(
        "anon-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

thread_local! {
    static SCOPED_WORKSPACE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

// Used when this source is included by ene-registry; unused in the standalone plugin binary
// unless referenced from the plugin binary (see main.rs link stub).
struct WorkspaceOverrideGuard(Option<PathBuf>);

impl Drop for WorkspaceOverrideGuard {
    fn drop(&mut self) {
        let previous = self.0.take();
        SCOPED_WORKSPACE.with(|slot| drop(slot.replace(previous)));
    }
}

// Host-only entry point; the standalone plugin binary keeps it live via main.rs.
pub fn with_workspace<T>(
    root: &Path,
    action: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let previous = SCOPED_WORKSPACE.with(|slot| slot.replace(Some(root.to_path_buf())));
    let _guard = WorkspaceOverrideGuard(previous);
    action()
}

fn scoped_workspace() -> Option<PathBuf> {
    SCOPED_WORKSPACE.with(|slot| slot.borrow().clone())
}

fn workspace() -> Result<PathBuf, String> {
    if let Some(root) = scoped_workspace() {
        return Ok(root);
    }
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

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "only plugin unit tests serialize workspace overrides"
    )
)]
static TEST_WORKSPACE_GATE: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

fn resolve(path: &str, create_parent: bool) -> Result<PathBuf, String> {
    if let Some(root) = scoped_workspace() {
        return ene_registry::confine_tool_path(&root, Path::new(path), create_parent)
            .map_err(|err| err.to_string());
    }
    #[cfg(test)]
    if let Some(root) = TEST_WORKSPACE.lock().clone() {
        return ene_registry::confine_tool_path(&root, Path::new(path), create_parent)
            .map_err(|err| err.to_string());
    }
    let Ok(workspace) = std::env::var("ENE_WORKSPACE") else {
        return Ok(PathBuf::from(path));
    };
    ene_registry::confine_tool_path(Path::new(&workspace), Path::new(path), create_parent)
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        Needle, TEST_WORKSPACE, TEST_WORKSPACE_GATE, apply_edit, apply_unified_diff, execute,
        hash_bytes, job_key, journal_path,
    };
    use serde_json::json;
    use std::fs;
    use std::thread;

    fn with_workspace(dir: &tempfile::TempDir, action: impl FnOnce() -> Result<(), String>) {
        let _gate = TEST_WORKSPACE_GATE.lock();
        *TEST_WORKSPACE.lock() = Some(dir.path().to_path_buf());
        let result = action();
        *TEST_WORKSPACE.lock() = None;
        result.unwrap();
    }

    #[test]
    fn read_returns_hash_of_raw_bytes() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("raw.txt");
        fs::write(&path, b"\xef\xbb\xbfhello").unwrap();
        with_workspace(&dir, || {
            let out = execute("fs.read", &json!({"path": path.to_string_lossy()}))?;
            assert_eq!(out["text"], "\u{feff}hello");
            assert_eq!(out["hash"], hash_bytes(b"\xef\xbb\xbfhello"));
            Ok(())
        });
    }

    #[test]
    fn stale_expected_hash_does_not_overwrite() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("stale.txt");
        fs::write(&path, "original").unwrap();
        with_workspace(&dir, || {
            let read = execute("fs.read", &json!({"path": path.to_string_lossy()}))?;
            let hash = read["hash"].as_str().unwrap();
            execute(
                "fs.write",
                &json!({
                    "path": path.to_string_lossy(),
                    "text": "changed",
                    "job_id": "stale-job",
                }),
            )?;
            let err = execute(
                "fs.write",
                &json!({
                    "path": path.to_string_lossy(),
                    "text": "mutated",
                    "expected_hash": hash,
                    "job_id": "stale-job",
                }),
            )
            .unwrap_err();
            assert!(err.contains("stale precondition"), "{err}");
            assert_eq!(fs::read_to_string(&path).unwrap(), "changed");
            Ok(())
        });
    }

    #[test]
    fn duplicate_old_without_replace_all_is_ambiguous() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("dup.txt");
        fs::write(&path, "foo bar foo").unwrap();
        with_workspace(&dir, || {
            let err = execute(
                "fs.edit",
                &json!({
                    "path": path.to_string_lossy(),
                    "old": "foo",
                    "new": "baz",
                    "job_id": "dup-job",
                }),
            )
            .unwrap_err();
            assert!(err.contains("ambiguous"), "{err}");
            assert_eq!(fs::read_to_string(&path).unwrap(), "foo bar foo");
            Ok(())
        });
    }

    #[test]
    fn crlf_is_preserved_on_edit() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("crlf.txt");
        fs::write(&path, "a\r\nb\r\n").unwrap();
        with_workspace(&dir, || {
            execute(
                "fs.edit",
                &json!({
                    "path": path.to_string_lossy(),
                    "old": "b",
                    "new": "B",
                    "job_id": "crlf-job",
                }),
            )?;
            assert_eq!(fs::read(&path).unwrap(), b"a\r\nB\r\n");
            Ok(())
        });
    }

    #[test]
    fn lf_and_bom_and_trailing_newline_are_preserved() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("mixed.txt");
        fs::write(&path, b"\xef\xbb\xbfline\n").unwrap();
        with_workspace(&dir, || {
            execute(
                "fs.edit",
                &json!({
                    "path": path.to_string_lossy(),
                    "old": "line",
                    "new": "LINE",
                    "job_id": "bom-job",
                }),
            )?;
            assert_eq!(fs::read(&path).unwrap(), b"\xef\xbb\xbfLINE\n");
            Ok(())
        });

        let no_nl = dir.path().join("no_nl.txt");
        fs::write(&no_nl, "keep").unwrap();
        with_workspace(&dir, || {
            execute(
                "fs.edit",
                &json!({
                    "path": no_nl.to_string_lossy(),
                    "old": "keep",
                    "new": "kept",
                    "job_id": "nonl-job",
                }),
            )?;
            assert_eq!(fs::read(&no_nl).unwrap(), b"kept");
            Ok(())
        });
    }

    #[test]
    fn concurrent_writes_are_serialized() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("shared.txt");
        fs::write(&path, "seed").unwrap();
        let _gate = TEST_WORKSPACE_GATE.lock();
        *TEST_WORKSPACE.lock() = Some(dir.path().to_path_buf());
        thread::scope(|scope| {
            scope.spawn(|| {
                for idx in 0..40 {
                    execute(
                        "fs.write",
                        &json!({
                            "path": "shared.txt",
                            "text": format!("left-{idx}"),
                            "job_id": "left",
                        }),
                    )
                    .unwrap();
                }
            });
            scope.spawn(|| {
                for idx in 0..40 {
                    execute(
                        "fs.write",
                        &json!({
                            "path": "shared.txt",
                            "text": format!("right-{idx}"),
                            "job_id": "right",
                        }),
                    )
                    .unwrap();
                }
            });
        });
        *TEST_WORKSPACE.lock() = None;
        let final_text = fs::read_to_string(&path).unwrap();
        assert!(
            final_text.starts_with("left-") || final_text.starts_with("right-"),
            "{final_text}"
        );
        assert!(final_text.len() < 12, "{final_text}");
    }

    #[test]
    fn atomic_write_replaces_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("atomic.txt");
        fs::write(&path, "before").unwrap();
        with_workspace(&dir, || {
            execute(
                "fs.write",
                &json!({
                    "path": path.to_string_lossy(),
                    "text": "after",
                    "job_id": "atomic-job",
                }),
            )?;
            assert_eq!(fs::read_to_string(&path).unwrap(), "after");
            Ok(())
        });
    }

    #[cfg(unix)]
    #[test]
    fn atomic_edit_preserves_executable_mode() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("tool.sh");
        fs::write(&path, "#!/bin/sh\necho before\n").unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();

        with_workspace(&dir, || {
            execute(
                "fs.edit",
                &json!({
                    "path": path.to_string_lossy(),
                    "old": "before",
                    "new": "after",
                    "job_id": "mode-job",
                }),
            )?;
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                "#!/bin/sh\necho after\n"
            );
            let mode = fs::metadata(&path).unwrap().mode() & 0o7777;
            assert_eq!(mode, 0o755, "executable mode must survive atomic replace");
            Ok(())
        });
    }

    #[test]
    fn undo_restores_overwrite_and_deletes_created_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let existing = dir.path().join("existing.txt");
        fs::write(&existing, "old").unwrap();
        let created = dir.path().join("created.txt");
        with_workspace(&dir, || {
            execute(
                "fs.write",
                &json!({
                    "path": existing.to_string_lossy(),
                    "text": "new",
                    "job_id": "undo-job",
                }),
            )?;
            execute(
                "fs.write",
                &json!({
                    "path": created.to_string_lossy(),
                    "text": "fresh",
                    "job_id": "undo-job",
                }),
            )?;
            execute("fs.undo", &json!({"job_id": "undo-job"}))?;
            assert!(!created.exists());
            assert_eq!(fs::read_to_string(&existing).unwrap(), "new");
            execute("fs.undo", &json!({"job_id": "undo-job"}))?;
            assert_eq!(fs::read_to_string(&existing).unwrap(), "old");
            Ok(())
        });
    }

    #[test]
    fn undo_is_scoped_to_the_job() {
        let dir = tempfile::TempDir::new().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, "one").unwrap();
        fs::write(&b, "two").unwrap();
        with_workspace(&dir, || {
            execute(
                "fs.write",
                &json!({"path": a.to_string_lossy(), "text": "A", "job_id": "job-a"}),
            )?;
            execute(
                "fs.write",
                &json!({"path": b.to_string_lossy(), "text": "B", "job_id": "job-b"}),
            )?;
            execute("fs.undo", &json!({"job_id": "job-a"}))?;
            assert_eq!(fs::read_to_string(&a).unwrap(), "one");
            assert_eq!(fs::read_to_string(&b).unwrap(), "B");
            Ok(())
        });
        assert_eq!(job_key(&json!({"job_id": "job-a"})), "job-a");
    }

    #[test]
    fn secret_files_are_not_stored_in_journal() {
        let dir = tempfile::TempDir::new().unwrap();
        let secret = dir.path().join(".env");
        fs::write(&secret, "SECRET=value").unwrap();
        with_workspace(&dir, || {
            execute(
                "fs.write",
                &json!({
                    "path": secret.to_string_lossy(),
                    "text": "SECRET=changed",
                    "job_id": "secret-job",
                }),
            )?;
            let journal = journal_path("secret-job").unwrap();
            let raw = fs::read_to_string(journal).unwrap_or_default();
            assert!(!raw.contains("SECRET"));
            assert!(!raw.contains("changed"));
            assert!(!raw.contains("value"));
            Ok(())
        });
    }

    #[test]
    fn unified_diff_replaces_matching_context() {
        let body = "alpha\nbeta\ngamma\n";
        let diff = "--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n alpha\n-beta\n+BETA\n gamma\n";
        let next = apply_unified_diff(body, diff).unwrap();
        assert_eq!(next, "alpha\nBETA\ngamma\n");
    }

    #[test]
    fn unified_diff_preserves_crlf() {
        let body = "alpha\r\nbeta\r\ngamma\r\n";
        let diff = "--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n alpha\n-beta\n+BETA\n gamma\n";
        let next = apply_unified_diff(body, diff).unwrap();
        assert_eq!(next, "alpha\r\nBETA\r\ngamma\r\n");
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
        fs::write(dir.path().join("a.txt"), "cost is $5+\n").unwrap();
        with_workspace(&dir, || {
            let found = execute("fs.search", &json!({"path": "a.txt", "query": "$5+"}))?;
            assert_eq!(found["matches"].as_array().unwrap().len(), 1);
            Ok(())
        });
    }

    #[test]
    fn undo_without_job_id_is_rejected() {
        let err = execute("fs.undo", &json!({})).unwrap_err();
        assert!(err.contains("job_id"), "{err}");
        assert_ne!(job_key(&json!({})), job_key(&json!({})));
    }

    #[test]
    fn needle_literal_vs_regex() {
        let lit = Needle::Literal("a+".to_owned());
        assert!(lit.is_match("xxa+yy"));
        assert!(!lit.is_match("aaa"));
        let re = Needle::Regex(regex::Regex::new("a+").unwrap());
        assert!(re.is_match("aaa"));
    }

    #[test]
    fn apply_edit_indent_tolerance_is_unique() {
        let body = "    alpha\n    beta\n";
        let next = apply_edit(body, "alpha\nbeta", "OK", false).unwrap();
        assert_eq!(next, "OK\n");
    }

    #[test]
    fn tolerant_edit_preserves_crlf() {
        let body = "    alpha\r\n    beta\r\n";
        let next = apply_edit(body, "alpha\nbeta", "gamma\ndelta", false).unwrap();
        assert!(
            next.contains("\r\n"),
            "expected CRLF line endings: {next:?}"
        );
        assert_eq!(next, "gamma\r\ndelta\r\n");
    }

    #[test]
    fn tolerant_edit_preserves_separator_before_following_line() {
        let lf = "    alpha\n    beta\nnext\n";
        assert_eq!(
            apply_edit(lf, "alpha\nbeta", "OK", false).unwrap(),
            "OK\nnext\n"
        );
        let crlf = "    alpha\r\n    beta\r\nnext\r\n";
        assert_eq!(
            apply_edit(crlf, "alpha\nbeta", "OK", false).unwrap(),
            "OK\r\nnext\r\n"
        );
    }

    #[test]
    fn tolerant_edit_rejects_ambiguous_indent_matches() {
        let body = "    dup\n    dup\n";
        let err = apply_edit(body, "dup", "x", false).unwrap_err();
        assert!(err.contains("ambiguous match"), "{err}");
    }

    #[test]
    fn tolerant_replace_all_replaces_every_indent_match() {
        let body = "    one\n    two\n    one\n";
        let next = apply_edit(body, "one", "X", true).unwrap();
        assert_eq!(next, "    X\n    two\n    X\n");
    }

    #[test]
    fn block_anchor_does_not_match_from_non_anchor_lines() {
        let body = "BEGIN\nnot anchor\nEND\nBEGIN\nmiddle\nEND\n";
        let old = "BEGIN\nmiddle\nEND";
        let next = apply_edit(body, old, "OK", false).unwrap();
        assert_eq!(next, "BEGIN\nnot anchor\nEND\nOK\n");
    }

    #[test]
    fn glob_lists_sorted_relative_paths_and_caps() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("a.rs"), "a").unwrap();
        fs::write(dir.path().join("src/b.rs"), "b").unwrap();
        fs::write(dir.path().join("src/c.txt"), "c").unwrap();
        fs::write(dir.path().join(".hidden.rs"), "h").unwrap();
        with_workspace(&dir, || {
            let listed = execute("fs.glob", &json!({"pattern": "**/*.rs"}))?;
            let paths = listed["paths"].as_array().unwrap();
            assert_eq!(paths.len(), 2);
            assert_eq!(paths[0], "a.rs");
            assert_eq!(paths[1], "src/b.rs");
            let capped = execute("fs.glob", &json!({"pattern": "**/*", "max": 1}))?;
            assert_eq!(capped["paths"].as_array().unwrap().len(), 1);
            assert_eq!(capped["truncated"], true);
            Ok(())
        });
    }

    #[test]
    fn glob_rejects_parent_escape() {
        let err = execute("fs.glob", &json!({"pattern": "../secret"})).unwrap_err();
        assert!(err.contains(".."), "{err}");
    }

    #[test]
    fn delete_removes_file_and_restores_on_undo() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("gone.txt");
        fs::write(&file, "keep-me").unwrap();
        with_workspace(&dir, || {
            execute(
                "fs.delete",
                &json!({"path": file.to_string_lossy(), "job_id": "del-job"}),
            )?;
            assert!(!file.exists());
            execute("fs.undo", &json!({"job_id": "del-job"}))?;
            Ok(())
        });
        assert_eq!(fs::read_to_string(&file).unwrap(), "keep-me");
    }

    #[test]
    fn delete_refuses_non_empty_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::create_dir(dir.path().join("nested")).unwrap();
        fs::write(dir.path().join("nested/a.txt"), "x").unwrap();
        with_workspace(&dir, || {
            let err = execute(
                "fs.delete",
                &json!({"path": dir.path().join("nested").to_string_lossy()}),
            )
            .unwrap_err();
            assert!(err.contains("not empty"), "{err}");
            Ok(())
        });
    }

    #[test]
    fn secret_delete_is_not_journaled() {
        let dir = tempfile::TempDir::new().unwrap();
        let secret = dir.path().join(".env");
        fs::write(&secret, "SECRET=1").unwrap();
        with_workspace(&dir, || {
            execute(
                "fs.delete",
                &json!({"path": secret.to_string_lossy(), "job_id": "secret-del"}),
            )?;
            assert!(!secret.exists());
            let journal = journal_path("secret-del")?;
            assert!(
                !journal.exists() || fs::read_to_string(&journal).unwrap().is_empty(),
                "secret must not enter the undo journal"
            );
            Ok(())
        });
    }

    #[test]
    fn delete_removes_empty_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let empty = dir.path().join("empty");
        fs::create_dir(&empty).unwrap();
        with_workspace(&dir, || {
            execute(
                "fs.delete",
                &json!({"path": empty.to_string_lossy(), "job_id": "empty-dir"}),
            )?;
            Ok(())
        });
        assert!(!empty.exists());
    }

    #[cfg(unix)]
    #[test]
    fn glob_skips_directory_symlink_escape() {
        let dir = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        fs::write(outside.path().join("secret.rs"), "nope").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("out")).unwrap();
        with_workspace(&dir, || {
            let listed = execute("fs.glob", &json!({"pattern": "**/*.rs"}))?;
            let paths = listed["paths"].as_array().unwrap();
            assert!(paths.is_empty(), "{listed}");
            Ok(())
        });
    }

    #[cfg(unix)]
    #[test]
    fn delete_refuses_symlink() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("real.txt");
        let link = dir.path().join("link.txt");
        fs::write(&target, "x").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        with_workspace(&dir, || {
            let err = execute("fs.delete", &json!({"path": link.to_string_lossy()})).unwrap_err();
            assert!(
                err.contains("symlink") || err.contains("path escapes workspace"),
                "{err}"
            );
            assert!(target.exists());
            Ok(())
        });
    }
}
