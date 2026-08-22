#[path = "../../../../plugins/tool/app/src/logic.rs"]
pub(crate) mod app;
#[path = "../../../../plugins/tool/exec/src/logic.rs"]
pub(crate) mod exec;
#[path = "../../../../plugins/tool/fs/src/logic.rs"]
pub(crate) mod fs;
#[path = "../../../../plugins/tool/utility/src/logic.rs"]
pub(crate) mod utility;
#[path = "../../../../plugins/tool/web/src/logic.rs"]
pub(crate) mod web;

use ene_plugin_ipc::{BuiltinKind, ToolSpecWire};
use serde_json::{Value, json};

pub(crate) fn specs(kind: BuiltinKind) -> Vec<ToolSpecWire> {
    let mut rows = match kind {
        BuiltinKind::Utility => utility::specs(),
        BuiltinKind::Fs => fs::specs(),
        BuiltinKind::Exec => exec::specs(),
        BuiltinKind::Web => web::specs(),
        BuiltinKind::App => app::specs(),
    };
    for row in &mut rows {
        apply_bundled_discovery(row);
    }
    rows
}

pub(crate) fn execute(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "utility.hash"
        | "utility.time"
        | "utility.system_info"
        | "utility.calc"
        | "utility.random"
        | "utility.text" => utility::execute(name, args),
        "fs.read" | "fs.write" | "fs.edit" | "fs.list" | "fs.search" | "fs.patch" | "fs.undo" => {
            fs::execute(name, args)
        }
        "exec.run" | "exec.shell" => exec::execute(name, args),
        "web.fetch" | "web.search" | "web.search_backends" => web::execute(name, args),
        "app.screenshot" | "app.window_list" | "app.active_window" | "app.clipboard_get"
        | "app.clipboard_set" | "app.click" | "app.type" | "app.key" | "app.capabilities"
        | "app.list_monitors" => app::execute(name, args),
        other => Err(format!("unknown builtin {other}")),
    }
}

fn apply_bundled_discovery(spec: &mut ToolSpecWire) {
    if !spec.category.is_empty() || !spec.keywords.is_empty() || !spec.examples.is_empty() {
        return;
    }
    let (category, keywords, examples) = match spec.name.as_str() {
        "fs.read" => (
            "filesystem",
            &["read", "file", "open"][..],
            &["read README.md"][..],
        ),
        "fs.write" => (
            "filesystem",
            &["write", "save"][..],
            &["write notes.txt"][..],
        ),
        "fs.edit" => (
            "filesystem",
            &["edit", "replace"][..],
            &["replace TODO with DONE"][..],
        ),
        "fs.list" => (
            "filesystem",
            &["list", "directory", "ls"][..],
            &["list src/"][..],
        ),
        "fs.search" => (
            "filesystem",
            &["search", "grep", "find text"][..],
            &["search for TODO"][..],
        ),
        "fs.patch" => (
            "filesystem",
            &["patch", "diff"][..],
            &["apply unified diff"][..],
        ),
        "fs.undo" => (
            "filesystem",
            &["undo", "revert"][..],
            &["undo last edit"][..],
        ),
        "exec.run" => (
            "process",
            &["run", "command", "subprocess"][..],
            &["run git status"][..],
        ),
        "exec.shell" => ("process", &["shell", "bash"][..], &["shell echo hello"][..]),
        "web.fetch" => (
            "web",
            &["fetch", "http"][..],
            &["fetch https://example.com"][..],
        ),
        "web.search" => (
            "web",
            &["search", "internet"][..],
            &["search rust book"][..],
        ),
        "utility.hash" => ("utility", &["hash", "digest"][..], &["hash hello"][..]),
        "utility.time" => ("utility", &["time", "clock"][..], &["time in Tokyo"][..]),
        "utility.calc" => (
            "utility",
            &["calc", "math", "convert"][..],
            &["calc 1+2*3"][..],
        ),
        "utility.random" => ("utility", &["random", "uuid"][..], &["random uuid"][..]),
        "utility.text" => (
            "utility",
            &["text", "encode", "regex"][..],
            &["encode base64"][..],
        ),
        "app.screenshot" => (
            "desktop",
            &["screenshot", "screen"][..],
            &["take screenshot"][..],
        ),
        "app.clipboard_get" => (
            "desktop",
            &["clipboard", "paste"][..],
            &["read clipboard"][..],
        ),
        _ => return,
    };
    category.clone_into(&mut spec.category);
    spec.keywords = keywords.iter().map(|item| (*item).to_owned()).collect();
    spec.examples = examples.iter().map(|item| (*item).to_owned()).collect();
}

/// JSON schema helper shared by bundled tool plugins.
#[must_use]
pub fn spec(
    name: &str,
    description: &str,
    parameters: Value,
    side_effects: Vec<String>,
) -> ToolSpecWire {
    spec_with_discovery(name, description, parameters, side_effects, "", &[], &[])
}

/// Bundled tool spec with optional discovery metadata.
#[must_use]
pub fn spec_with_discovery(
    name: &str,
    description: &str,
    parameters: Value,
    side_effects: Vec<String>,
    category: &str,
    keywords: &[&str],
    examples: &[&str],
) -> ToolSpecWire {
    ToolSpecWire {
        name: name.to_owned(),
        description: description.to_owned(),
        parameters,
        output: json!({"type":"object"}),
        side_effects,
        category: category.to_owned(),
        keywords: keywords.iter().map(|item| (*item).to_owned()).collect(),
        examples: examples.iter().map(|item| (*item).to_owned()).collect(),
    }
}

/// Required string argument, or `"missing {key}"`.
///
/// # Errors
///
/// Returns `Err` when `key` is absent or not a string.
pub fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing {key}"))
}
