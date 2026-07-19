//! Tracing layer that renders parallel spans as a live-updating ASCII tree.
//!
//! Out-of-order child events (A1 → B1 → A2) still produce a correct parent/child
//! tree because nodes are keyed by span id and re-rendered as a block.

use crate::terminal_ui::TerminalUi;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::fmt::Debug;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

#[derive(Debug)]
struct TreeNode {
    name: String,
    children: Vec<Child>,
    closed: bool,
}

#[derive(Debug)]
enum Child {
    Event(String),
    Span(Id),
}

struct Forest {
    nodes: HashMap<Id, TreeNode>,
    roots: Vec<Id>,
    displayed_lines: usize,
}

impl Forest {
    fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            roots: Vec::new(),
            displayed_lines: 0,
        }
    }

    fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    fn all_roots_closed(&self) -> bool {
        !self.roots.is_empty()
            && self
                .roots
                .iter()
                .all(|id| self.nodes.get(id).is_some_and(|n| n.closed))
    }

    fn render(&self) -> Vec<String> {
        let mut lines = Vec::new();
        let count = self.roots.len();
        for (i, id) in self.roots.iter().enumerate() {
            let is_last = i + 1 == count;
            render_node(id, &self.nodes, "", is_last, &mut lines);
        }
        lines
    }
}

fn render_node(
    id: &Id,
    nodes: &HashMap<Id, TreeNode>,
    prefix: &str,
    is_last: bool,
    lines: &mut Vec<String>,
) {
    let Some(node) = nodes.get(id) else {
        return;
    };
    let branch = if is_last { "└ " } else { "|- " };
    lines.push(format!("{prefix}{branch}{}", node.name));

    let child_prefix = if is_last {
        format!("{prefix}  ")
    } else {
        format!("{prefix}| ")
    };
    let child_count = node.children.len();
    for (i, child) in node.children.iter().enumerate() {
        let last = i + 1 == child_count;
        match child {
            Child::Event(msg) => {
                let b = if last { "└ " } else { "|- " };
                lines.push(format!("{child_prefix}{b}{msg}"));
            }
            Child::Span(cid) => {
                render_node(cid, nodes, &child_prefix, last, lines);
            }
        }
    }
}

struct TreeLogState {
    ui: TerminalUi,
    forest: Forest,
}

impl TreeLogState {
    fn refresh(&mut self) {
        if self.forest.is_empty() {
            return;
        }
        let lines = self.forest.render();
        self.forest.displayed_lines = self.ui.replace_block(self.forest.displayed_lines, &lines);
        if self.forest.all_roots_closed() {
            self.finalize();
        }
    }

    fn finalize(&mut self) {
        self.forest = Forest::new();
    }

    fn ensure_fresh_forest_for_new_root(&mut self) {
        if self.forest.all_roots_closed() {
            self.finalize();
        }
    }
}

/// Custom tracing layer that drives [`TerminalUi`] tree / flat output.
pub struct TreeLogLayer {
    state: Mutex<TreeLogState>,
}

impl TreeLogLayer {
    #[must_use]
    pub fn new(ui: TerminalUi) -> Self {
        Self {
            state: Mutex::new(TreeLogState {
                ui,
                forest: Forest::new(),
            }),
        }
    }
}

impl<S> Layer<S> for TreeLogLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let name = attrs.metadata().name().to_string();

        let parent_id = if let Some(parent) = attrs.parent() {
            Some(parent.clone())
        } else if attrs.is_contextual() {
            ctx.lookup_current().map(|s| s.id())
        } else {
            None
        };

        let mut state = self.state.lock();
        let can_attach = parent_id.as_ref().is_some_and(|pk| {
            state
                .forest
                .nodes
                .get(pk)
                .is_some_and(|parent| !parent.closed)
        });
        if can_attach {
            let Some(pk) = parent_id else {
                return;
            };
            if let Some(parent) = state.forest.nodes.get_mut(&pk) {
                parent.children.push(Child::Span(id.clone()));
            }
            state.forest.nodes.insert(
                id.clone(),
                TreeNode {
                    name,
                    children: Vec::new(),
                    closed: false,
                },
            );
            state.refresh();
            return;
        }

        // New root (or parent already closed / unknown).
        state.ensure_fresh_forest_for_new_root();
        state.forest.roots.push(id.clone());
        state.forest.nodes.insert(
            id.clone(),
            TreeNode {
                name,
                children: Vec::new(),
                closed: false,
            },
        );
        state.refresh();
    }

    fn on_record(&self, _span: &Id, _values: &Record<'_>, _ctx: Context<'_, S>) {}

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let message = visitor.message;
        if message.is_empty() {
            return;
        }

        let current = ctx.event_span(event).map(|s| s.id());
        let mut state = self.state.lock();

        let attached = current.as_ref().is_some_and(|span_id| {
            state
                .forest
                .nodes
                .get(span_id)
                .is_some_and(|node| !node.closed)
        });
        if attached {
            let Some(span_id) = current else {
                return;
            };
            if let Some(node) = state.forest.nodes.get_mut(&span_id) {
                node.children.push(Child::Event(message));
            }
            state.refresh();
            return;
        }

        // Flat line — end any live tree so subsequent rewrite cannot clobber it.
        if !state.forest.is_empty() {
            state.finalize();
        }
        state.ui.writeln(&message);
    }

    fn on_close(&self, id: Id, _ctx: Context<'_, S>) {
        let mut state = self.state.lock();
        if let Some(node) = state.forest.nodes.get_mut(&id) {
            node.closed = true;
            state.refresh();
        }
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: String,
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Child, Forest, TreeNode};
    use std::collections::HashMap;
    use tracing::span::Id;

    fn id(n: u64) -> Id {
        Id::from_u64(n)
    }

    enum TestChild {
        Event(String),
        Span(u64, String, Vec<TestChild>),
    }

    fn insert_recursive(
        nodes: &mut HashMap<Id, TreeNode>,
        span_id: Id,
        name: &str,
        children: &[TestChild],
    ) {
        let mut node_children = Vec::new();
        for child in children {
            match child {
                TestChild::Event(msg) => node_children.push(Child::Event(msg.clone())),
                TestChild::Span(cid, cname, grandchildren) => {
                    let child_id = id(*cid);
                    node_children.push(Child::Span(child_id.clone()));
                    insert_recursive(nodes, child_id, cname, grandchildren);
                }
            }
        }
        nodes.insert(
            span_id,
            TreeNode {
                name: name.to_string(),
                children: node_children,
                closed: false,
            },
        );
    }

    fn render_forest(roots: &[(u64, &str, Vec<TestChild>)]) -> Vec<String> {
        let mut forest = Forest::new();
        for (raw_id, name, children) in roots {
            let span_id = id(*raw_id);
            forest.roots.push(span_id.clone());
            insert_recursive(&mut forest.nodes, span_id, name, children);
        }
        forest.render()
    }

    #[test]
    fn out_of_order_children_still_form_correct_tree() {
        let lines = render_forest(&[
            (
                1,
                "LOG_A",
                vec![
                    TestChild::Event("LOG_A_1".into()),
                    TestChild::Event("LOG_A_2".into()),
                ],
            ),
            (
                2,
                "LOG_B",
                vec![
                    TestChild::Event("LOG_B_1".into()),
                    TestChild::Event("LOG_B_2".into()),
                ],
            ),
        ]);

        assert_eq!(
            lines,
            vec![
                "|- LOG_A".to_string(),
                "| |- LOG_A_1".to_string(),
                "| └ LOG_A_2".to_string(),
                "└ LOG_B".to_string(),
                "  |- LOG_B_1".to_string(),
                "  └ LOG_B_2".to_string(),
            ]
        );
    }

    #[test]
    fn nested_span_children_render() {
        let lines = render_forest(&[(
            1,
            "phase_a",
            vec![
                TestChild::Span(
                    2,
                    "embedding".to_string(),
                    vec![TestChild::Event("Generating embedding...".into())],
                ),
                TestChild::Span(
                    3,
                    "ccv3_sync".to_string(),
                    vec![TestChild::Event("Synchronizing...".into())],
                ),
            ],
        )]);

        assert_eq!(lines[0], "└ phase_a");
        assert!(lines.iter().any(|l| l.contains("embedding")));
        assert!(lines.iter().any(|l| l.contains("ccv3_sync")));
    }
}
