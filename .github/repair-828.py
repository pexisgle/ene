from pathlib import Path
import re


def resolve_ours(path: str) -> None:
    p = Path(path)
    s = p.read_text()
    pat = re.compile(r"<<<<<<< ours\n(.*?)=======\n(.*?)>>>>>>> theirs", re.S)
    while pat.search(s):
        s = pat.sub(lambda m: m.group(1), s, count=1)
    if any(x in s for x in ("<<<<<<<", "=======", ">>>>>>>")):
        raise SystemExit(f"unresolved markers: {path}")
    p.write_text(s)


for f in [
    "crates/ene-fiber/src/broker.rs",
    "crates/ene-registry/src/builtins.rs",
    "docs/guides/tools/builtin-tools.md",
    "docs/ja/guides/tools/builtin-tools.md",
]:
    resolve_ours(f)

# broker.rs: retain current rg implementation and add the #828 broker surface.
p = Path("crates/ene-fiber/src/broker.rs")
s = p.read_text()
if "use ene_registry::BuiltinExecutor;" not in s:
    s = s.replace(
        "use crate::fiber::FiberUid;\n",
        "use crate::fiber::FiberUid;\nuse ene_registry::BuiltinExecutor;\n",
        1,
    )
anchor = '    #[error("invalid glob: {0}")]\n    InvalidGlob(String),\n'
if "    Symlink," not in s:
    s = s.replace(
        anchor,
        anchor
        + '    #[error("refusing to delete a symlink")]\n    Symlink,\n'
        + '    #[error("directory is not empty")]\n    NotEmpty,\n'
        + '    #[error("path is read-only")]\n    ReadOnly,\n'
        + '    #[error("filesystem tool failed: {0}")]\n    Tool(String),\n',
        1,
    )
elif "    Tool(String)," not in s:
    s = s.replace(
        "    ReadOnly,\n",
        '    ReadOnly,\n    #[error("filesystem tool failed: {0}")]\n    Tool(String),\n',
        1,
    )

if "fn broker_walk_glob(" not in s:
    s += r'''

fn broker_walk_glob(
    root: &Path,
    dir: &Path,
    pattern: &str,
    out: &mut Vec<String>,
    max: usize,
) -> Result<(), BrokerError> {
    if out.len() >= max {
        return Ok(());
    }
    for ent in std::fs::read_dir(dir)? {
        if out.len() >= max {
            break;
        }
        let ent = ent?;
        let path = ent.path();
        let name = ent.file_name();
        let name = name.to_string_lossy();
        if name == ".ene" || name.starts_with('.') {
            continue;
        }
        let meta = std::fs::symlink_metadata(&path)?;
        if is_link_or_reparse(&meta) {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|_| BrokerError::PathEscape(path.display().to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        if broker_glob_match(pattern, &rel) {
            out.push(rel);
        }
        if meta.is_dir() {
            broker_walk_glob(root, &path, pattern, out, max)?;
        }
    }
    Ok(())
}

fn broker_glob_match(pattern: &str, rel: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').filter(|p| !p.is_empty()).collect();
    let path: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
    broker_glob_components(&pat, &path)
}

fn broker_glob_components(pat: &[&str], path: &[&str]) -> bool {
    match (pat.first().copied(), path.first().copied()) {
        (None, None) => true,
        (Some("**"), _) => {
            broker_glob_components(&pat[1..], path)
                || (!path.is_empty() && broker_glob_components(pat, &path[1..]))
        }
        (Some(seg), Some(name)) if broker_glob_segment(seg, name) => {
            broker_glob_components(&pat[1..], &path[1..])
        }
        _ => false,
    }
}

fn broker_glob_segment(pattern: &str, name: &str) -> bool {
    broker_glob_stars(pattern.as_bytes(), name.as_bytes())
}

fn broker_glob_stars(pattern: &[u8], name: &[u8]) -> bool {
    match (pattern.first().copied(), name.first().copied()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            broker_glob_stars(&pattern[1..], name)
                || (!name.is_empty() && broker_glob_stars(pattern, &name[1..]))
        }
        (Some(b'?'), Some(_)) => broker_glob_stars(&pattern[1..], &name[1..]),
        (Some(p), Some(n)) if p == n => broker_glob_stars(&pattern[1..], &name[1..]),
        _ => false,
    }
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
'''

if "pub fn fs_invoke(" not in s:
    net = "    pub fn net_fetch(&self, uid: FiberUid, url: &str) -> Result<Value, BrokerError> {"
    method = r'''    /// Execute a bundled filesystem tool inside the broker-owned workspace.
    pub fn fs_invoke(
        &self,
        uid: FiberUid,
        name: &str,
        args: &Value,
    ) -> Result<Value, BrokerError> {
        if name == "fs.search" {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .ok_or_else(|| BrokerError::Tool("missing query".to_owned()))?;
            let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
            let context_lines = u32::try_from(
                args.get("context_lines").and_then(Value::as_u64).unwrap_or(0),
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
            _ => return Err(BrokerError::Tool(format!("unsupported fs tool {name}"))),
        };
        self.require(uid, cap)?;
        BuiltinExecutor
            .execute_fs_in_workspace(&self.workspace, name, args)
            .map_err(BrokerError::Tool)
    }

'''
    if net not in s:
        raise SystemExit("net_fetch anchor missing")
    s = s.replace(net, method + net, 1)

if "host_fs_invoke_uses_broker_workspace" not in s:
    s += r'''

#[cfg(test)]
mod host_fs_invoke_tests {
    use super::{Broker, BrokerError};
    use crate::fiber::FiberUid;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn host_fs_invoke_uses_broker_workspace() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("inside.txt"), "inside").unwrap();
        let mut broker = Broker::new(dir.path().to_path_buf());
        let uid = FiberUid::new();
        broker.grant(uid, "fs.read");
        let value = broker
            .fs_invoke(uid, "fs.read", &json!({"path":"inside.txt"}))
            .unwrap();
        assert_eq!(value["text"], "inside");
    }

    #[test]
    fn host_fs_invoke_requires_grant() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("inside.txt"), "inside").unwrap();
        let broker = Broker::new(dir.path().to_path_buf());
        assert!(matches!(
            broker.fs_invoke(FiberUid::new(), "fs.read", &json!({"path":"inside.txt"})),
            Err(BrokerError::Denied { .. })
        ));
    }
}
'''
p.write_text(s)

# Explicit scoped workspace for host-side execution.
p = Path("plugins/tool/fs/src/logic.rs")
s = p.read_text()
if "SCOPED_WORKSPACE" not in s:
    a = "fn workspace() -> Result<PathBuf, String> {"
    scoped = r'''thread_local! {
    static SCOPED_WORKSPACE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

struct WorkspaceOverrideGuard(Option<PathBuf>);

impl Drop for WorkspaceOverrideGuard {
    fn drop(&mut self) {
        let previous = self.0.take();
        SCOPED_WORKSPACE.with(|slot| drop(slot.replace(previous)));
    }
}

pub(crate) fn with_workspace<T>(
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

'''
    s = s.replace(a, scoped + a, 1)
    s = s.replace(
        a + "\n    #[cfg(test)]",
        a + "\n    if let Some(root) = scoped_workspace() {\n        return Ok(root);\n    }\n    #[cfg(test)]",
        1,
    )
    r = "fn resolve(path: &str, create_parent: bool) -> Result<PathBuf, String> {\n    #[cfg(test)]"
    rr = "fn resolve(path: &str, create_parent: bool) -> Result<PathBuf, String> {\n    if let Some(root) = scoped_workspace() {\n        return ene_registry::confine_tool_path(&root, Path::new(path), create_parent)\n            .map_err(|err| err.to_string());\n    }\n    #[cfg(test)]"
    s = s.replace(r, rr, 1)
p.write_text(s)

# Registry entry point + sensitivity merge.
p = Path("crates/ene-registry/src/builtins.rs")
s = p.read_text()
if "execute_fs_in_workspace" not in s:
    a = "\n}\n\n#[must_use]\npub fn host_spec_for"
    m = r'''

    /// Execute a bundled fs tool against an explicit workspace.
    pub fn execute_fs_in_workspace(
        &self,
        workspace: &Path,
        name: &str,
        args: &Value,
    ) -> Result<Value, String> {
        builtin::fs::with_workspace(workspace, || builtin::fs::execute(name, args))
    }
'''
    if a not in s:
        raise SystemExit("BuiltinExecutor anchor missing")
    s = s.replace(a, m + a, 1)
needle = (
    '        "app.screenshot" | "app.window_list" | "app.active_window" | "app.clipboard_get"\n'
    '        | "app.list_monitors" => Sensitivity::High,'
)
if '"fs.delete" | "app.screenshot"' not in s:
    if needle not in s:
        raise SystemExit("sensitivity anchor missing")
    s = s.replace(
        needle,
        '        "fs.delete" | "app.screenshot" | "app.window_list" | "app.active_window"\n'
        '        | "app.clipboard_get" | "app.list_monitors" => Sensitivity::High,',
        1,
    )
p.write_text(s)

# HostFsInvoker for both activation paths.
p = Path("crates/ene-fiber/src/supervisor.rs")
s = p.read_text()
web = "struct HostWebInvoker {\n    uid: FiberUid,\n    inner: Arc<SupervisorInner>,\n}\n"
if "struct HostFsInvoker" not in s:
    s = s.replace(
        web,
        web + "\nstruct HostFsInvoker {\n    uid: FiberUid,\n    inner: Arc<SupervisorInner>,\n}\n",
        1,
    )
if "impl ToolInvoke for HostFsInvoker" not in s:
    a = "#[async_trait]\nimpl ToolInvoke for HostWebInvoker {"
    imp = r'''#[async_trait]
impl ToolInvoke for HostFsInvoker {
    async fn invoke(&self, name: &str, args: Value) -> Result<Value, String> {
        self.inner
            .broker
            .lock()
            .fs_invoke(self.uid, name, &args)
            .map_err(|err| err.to_string())
    }
}

'''
    s = s.replace(a, imp + a, 1)
old = '''        let web_invoke = (row.plugin == "tool.web").then(|| {
            Arc::new(HostWebInvoker {
                uid: fiber.uid,
                inner: Arc::clone(&self.inner),
            }) as Arc<dyn ToolInvoke>
        });
        let builtin_invoke = Arc::new(BuiltinInvoker) as Arc<dyn ToolInvoke>;
        for def in definitions_for(kind) {
            let invoke = web_invoke
                .as_ref()
                .map_or_else(|| Arc::clone(&builtin_invoke), Arc::clone);
            self.inner.record_tool(&mut fiber, def, invoke);
        }
'''
if old in s:
    new = '''        let host_invoke: Option<Arc<dyn ToolInvoke>> = match row.plugin.as_str() {
            "tool.web" => Some(Arc::new(HostWebInvoker {
                uid: fiber.uid,
                inner: Arc::clone(&self.inner),
            })),
            "tool.fs" => Some(Arc::new(HostFsInvoker {
                uid: fiber.uid,
                inner: Arc::clone(&self.inner),
            })),
            _ => None,
        };
        let builtin_invoke = Arc::new(BuiltinInvoker) as Arc<dyn ToolInvoke>;
        for def in definitions_for(kind) {
            let invoke = host_invoke
                .as_ref()
                .map_or_else(|| Arc::clone(&builtin_invoke), Arc::clone);
            self.inner.record_tool(&mut fiber, def, invoke);
        }
'''
    s = s.replace(old, new, 1)
s = s.replace(
    '''        } else if row.plugin == "tool.fs" {
            Arc::new(BuiltinInvoker)
        } else {''',
    '''        } else if row.plugin == "tool.fs" {
            Arc::new(HostFsInvoker {
                uid: fiber.uid,
                inner: Arc::clone(&self.inner),
            })
        } else {''',
    1,
)
p.write_text(s)

# Profile grants include #891 search.
p = Path("apps/ene-core/src/plugin_profile.rs")
s = p.read_text()
if '"fs.search".to_owned()' not in s:
    s = s.replace(
        '            "fs.delete".to_owned(),\n        ],',
        '            "fs.delete".to_owned(),\n            "fs.search".to_owned(),\n        ],',
        1,
    )
p.write_text(s)

# Preserve current utility/exec/search docs and add only #828 details.
for path, ja in [
    (Path("docs/guides/tools/builtin-tools.md"), False),
    (Path("docs/ja/guides/tools/builtin-tools.md"), True),
]:
    lines = path.read_text().splitlines()
    idx = [i for i, line in enumerate(lines) if line.startswith("| `fs` | `ene-tool-fs` |")]
    if len(idx) != 1:
        raise SystemExit(f"fs row missing: {path}")
    line = lines[idx[0]]
    if ja:
        line = line.replace(
            "read / write / edit / list / search / patch / undo",
            "read / write / edit / list / glob / delete / search / patch / undo",
        )
        line = line.replace(
            "シェルは持たない。",
            "シェルは持たない。`fs.glob` はワークスペース相対・件数上限付きで symlink を辿らない。`fs.delete` は承認対象で、ファイルまたは空ディレクトリのみを削除する。",
            1,
        )
        extra = "`tool.fs` 子プロセスにはワークスペース権限を渡さず、fs tool は Fiber の FileBroker で grant 検査してホスト側で実行する。"
    else:
        line = line.replace(
            "Read / write / edit / list / search / patch / undo",
            "Read / write / edit / list / glob / delete / search / patch / undo",
        )
        line = line.replace(
            "No shell.",
            "No shell. `fs.glob` is workspace-relative and capped without following directory symlinks. `fs.delete` is approval-gated and removes only files or empty directories.",
            1,
        )
        extra = "The `tool.fs` child has no workspace filesystem rights; fs tools are grant-checked by the fiber FileBroker and executed host-side."
    if extra not in line:
        line = line[:-2] + " " + extra + " |"
    lines[idx[0]] = line
    path.write_text("\n".join(lines) + "\n")
