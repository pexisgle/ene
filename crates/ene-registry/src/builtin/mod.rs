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
        "utility.hash"
        | "utility.time"
        | "utility.system_info"
        | "utility.calc"
        | "utility.random"
        | "utility.text" => utility::execute(name, args),
        "fs.read" | "fs.write" | "fs.edit" | "fs.list" | "fs.search" | "fs.patch" | "fs.undo" => {
            fs::execute(name, args)
        }
        "exec.run" => exec::execute(name, args),
        "web.fetch" | "web.search" => web::execute(name, args),
        "app.screenshot" | "app.window_list" | "app.active_window" | "app.clipboard_get"
        | "app.clipboard_set" | "app.click" | "app.type" | "app.key" | "app.capabilities"
        | "app.list_monitors" => app::execute(name, args),
        other => Err(format!("unknown builtin {other}")),
    }
}

/// JSON schema helper shared by bundled tool plugins.
#[must_use]
pub fn spec(
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
