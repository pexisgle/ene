use std::collections::{HashMap, HashSet};

use crate::fiber::{Fiber, FiberState};
use crate::supervisor::ProfileRow;
use ene_plugin_ipc::BuiltinKind;
use ene_registry::definitions_for;

/// Outcome of [`crate::Supervisor::apply_profile`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileApplyReport {
    pub activated: Vec<String>,
    pub unloaded: Vec<String>,
    pub waiting: Vec<String>,
    pub cycle_rows: Vec<String>,
}

pub(crate) fn row_provides(plugin: &str, capabilities: &[String]) -> HashSet<String> {
    let mut keys = HashSet::new();
    if let Some(kind) = plugin_kind(plugin) {
        for def in definitions_for(kind) {
            keys.insert(format!("tool.{}", def.name));
        }
    } else if plugin == "tool.dummy" {
        keys.insert("tool.dummy.ping".to_owned());
    }
    for cap in capabilities {
        keys.insert(format!("broker.{cap}"));
    }
    keys
}

pub(crate) fn detect_require_cycles(rows: &[ProfileRow]) -> Vec<String> {
    let provides_by_row: HashMap<String, HashSet<String>> = rows
        .iter()
        .map(|row| {
            (
                row.row_id.clone(),
                row_provides(&row.plugin, &row.capabilities),
            )
        })
        .collect();
    let mut depends_on: HashMap<String, HashSet<String>> = HashMap::new();
    for row in rows {
        let mut deps = HashSet::new();
        for req in &row.requires {
            for (other_id, provides) in &provides_by_row {
                if other_id == &row.row_id {
                    continue;
                }
                if provides.contains(req) {
                    deps.insert(other_id.clone());
                }
            }
        }
        depends_on.insert(row.row_id.clone(), deps);
    }
    let mut cyclic = HashSet::new();
    for row in rows {
        let mut visiting = HashSet::new();
        let mut stack = Vec::new();
        let _ = dfs_cycle(
            &row.row_id,
            &depends_on,
            &mut visiting,
            &mut stack,
            &mut cyclic,
        );
    }
    let mut rows: Vec<String> = cyclic.into_iter().collect();
    rows.sort();
    rows
}

fn dfs_cycle(
    node: &str,
    graph: &HashMap<String, HashSet<String>>,
    visiting: &mut HashSet<String>,
    stack: &mut Vec<String>,
    cyclic: &mut HashSet<String>,
) -> bool {
    if visiting.contains(node) {
        if let Some(start) = stack.iter().position(|n| n == node) {
            for member in &stack[start..] {
                cyclic.insert(member.clone());
            }
        }
        return true;
    }
    visiting.insert(node.to_owned());
    stack.push(node.to_owned());
    let mut found = false;
    if let Some(deps) = graph.get(node) {
        for dep in deps {
            if dfs_cycle(dep, graph, visiting, stack, cyclic) {
                found = true;
            }
        }
    }
    stack.pop();
    visiting.remove(node);
    found
}

pub(crate) fn missing_requires(
    row: &ProfileRow,
    active_provides: &HashSet<String>,
) -> Vec<String> {
    row.requires
        .iter()
        .filter(|req| !active_provides.contains(*req))
        .cloned()
        .collect()
}

pub(crate) fn active_provides(fibers: &HashMap<String, Fiber>) -> HashSet<String> {
    fibers
        .values()
        .filter(|fiber| fiber.state == FiberState::Active)
        .flat_map(|fiber| fiber.provides.clone())
        .collect()
}

pub(crate) fn waiting_fiber(row: &ProfileRow, reason: impl Into<String>) -> Fiber {
    let mut fiber = Fiber::new(&row.row_id, &row.plugin);
    fiber.requires.clone_from(&row.requires);
    fiber.sandbox_required = row.sandbox_required;
    fiber.state = FiberState::Waiting;
    fiber.wait_reason = Some(reason.into());
    fiber
}

fn plugin_kind(plugin: &str) -> Option<BuiltinKind> {
    match plugin {
        "tool.fs" => Some(BuiltinKind::Fs),
        "tool.exec" => Some(BuiltinKind::Exec),
        "tool.web" => Some(BuiltinKind::Web),
        "tool.utility" => Some(BuiltinKind::Utility),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, plugin: &str, requires: &[&str]) -> ProfileRow {
        ProfileRow {
            row_id: id.to_owned(),
            plugin: plugin.to_owned(),
            requires: requires.iter().map(|s| (*s).to_owned()).collect(),
            capabilities: Vec::new(),
            sandbox_required: false,
        }
    }

    #[test]
    fn detects_mutual_requires_cycle() {
        let rows = vec![
            row("a", "tool.utility", &["tool.web.fetch"]),
            row("b", "tool.web", &["tool.utility.hash"]),
        ];
        let cyclic = detect_require_cycles(&rows);
        assert_eq!(cyclic, vec!["a", "b"]);
    }
}
