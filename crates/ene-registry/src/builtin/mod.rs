mod app;
mod exec;
mod fs;
mod utility;
mod web;

use ene_plugin_ipc::{BuiltinKind, ToolSpecWire};
use serde_json::{Value, json};

pub(crate) fn specs(kind: BuiltinKind) -> Vec<ToolSpecWire> {
    match kind {
        BuiltinKind::Utility => utility::specs(),
        BuiltinKind::Fs => fs::specs(),
        BuiltinKind::Exec => exec::specs(),
        BuiltinKind::Web => web::specs(),
        BuiltinKind::App => app::specs(),
    }
}

pub(crate) fn execute(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "utility.hash" | "utility.time" | "utility.calc" | "utility.random" | "utility.text" => {
            utility::execute(name, args)
        }
        "fs.read" | "fs.write" | "fs.edit" | "fs.list" | "fs.search" | "fs.patch" | "fs.undo" => {
            fs::execute(name, args)
        }
        "exec.run" => exec::execute(args),
        "web.fetch" | "web.search" => web::execute(name, args),
        "app.screenshot" | "app.window_list" | "app.active_window" | "app.clipboard_get"
        | "app.clipboard_set" | "app.click" | "app.type" | "app.key" => app::execute(name, args),
        other => Err(format!("unknown builtin {other}")),
    }
}

pub(crate) fn spec(
    name: &str,
    description: &str,
    parameters: Value,
    side_effects: Vec<String>,
) -> ToolSpecWire {
    ToolSpecWire {
        name: name.to_owned(),
        description: description.to_owned(),
        parameters,
        output: json!({"type":"object"}),
        side_effects,
    }
}

pub(crate) fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing {key}"))
}
