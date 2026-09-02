use crate::broker::{Broker, BrokerError};
use crate::fiber::FiberUid;
use ene_tool_registry::BuiltinExecutor;
use serde_json::{Value, json};
use std::path::Path;

impl Broker {
    /// Execute a bundled filesystem tool inside the broker-owned workspace.
    pub fn fs_invoke(&self, uid: FiberUid, name: &str, args: &Value) -> Result<Value, BrokerError> {
        if name == "fs.search" {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .ok_or_else(|| tool_error("missing query"))?;
            let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
            let context_lines = u32::try_from(
                args.get("context_lines")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            )
            .unwrap_or(0)
            .min(10);
            let max = u32::try_from(args.get("max").and_then(Value::as_u64).unwrap_or(50))
                .unwrap_or(50)
                .min(200);
            let matches = self.fs_search(
                uid,
                Path::new(path),
                query,
                args.get("regex").and_then(Value::as_bool).unwrap_or(false),
                args.get("case_insensitive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                args.get("include").and_then(Value::as_str),
                context_lines,
                args.get("count").and_then(Value::as_bool).unwrap_or(false),
                max,
            )?;
            return Ok(json!({ "matches": matches }));
        }
        let cap = match name {
            "fs.read" => "fs.read",
            "fs.write" | "fs.edit" | "fs.patch" | "fs.undo" => "fs.write",
            "fs.list" => "fs.list",
            "fs.glob" => "fs.glob",
            "fs.delete" => "fs.delete",
            _ => return Err(tool_error(format!("unsupported fs tool {name}"))),
        };
        if !self.has_grant(uid, cap) {
            return Err(BrokerError::Denied {
                uid: uid.to_string(),
                op: cap.to_owned(),
            });
        }
        BuiltinExecutor
            .execute_fs_in_workspace(self.workspace(), name, args)
            .map_err(tool_error)
    }
}

fn tool_error(message: impl Into<String>) -> BrokerError {
    BrokerError::Fetch(format!("filesystem tool failed: {}", message.into()))
}
