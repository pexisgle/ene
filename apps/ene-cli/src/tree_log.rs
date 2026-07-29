//! Tracing layer that renders parallel spans as a live-updating ASCII tree.
//!
//! Out-of-order child events (A1 → B1 → A2) still produce a correct parent/child
//! tree because nodes are keyed by span id and re-rendered as a block.
//!
//! Lines keep level colors and a source label (`component` field, else target)
//! so the tree stays readable after replacing the default `fmt` subscriber.

use crate::terminal_ui::TerminalUi;
use console::style;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::fmt::Debug;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

#[derive(Debug, Clone)]
struct LogEvent {
    level: Level,
    /// `component` field, else short tracing target.
    source: String,
    message: String,
    /// Extra structured fields (tool, turn, …), already formatted.
    extras: String,
}

#[derive(Debug)]
struct TreeNode {
    name: String,
    children: Vec<Child>,
    closed: bool,
}

#[derive(Debug)]
enum Child {
    Event(LogEvent),
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

fn dim_branch(s: &str) -> String {
    style(s).dim().to_string()
}

fn style_span_name(name: &str) -> String {
    style(name).cyan().bold().to_string()
}

fn style_level(level: Level) -> String {
    match level {
        Level::ERROR => style("ERROR").red().bold().to_string(),
        Level::WARN => style("WARN").yellow().bold().to_string(),
        Level::INFO => style("INFO").green().to_string(),
        Level::DEBUG => style("DEBUG").dim().to_string(),
        Level::TRACE => style("TRACE").dim().to_string(),
    }
}

fn format_log_event(event: &LogEvent) -> String {
    let level = style_level(event.level);
    let source = style(event.source.as_str()).cyan().to_string();
    if event.extras.is_empty() {
        format!("{level} {source}: {}", event.message)
    } else {
        let extras = style(event.extras.as_str()).dim().to_string();
        format!("{level} {source}: {} {extras}", event.message)
    }
}

fn format_flat_line(event: &LogEvent) -> String {
    format_log_event(event)
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
    let branch = if is_last {
        dim_branch("└ ")
    } else {
        dim_branch("|- ")
    };
    lines.push(format!("{prefix}{branch}{}", style_span_name(&node.name)));

    let child_prefix = if is_last {
        format!("{prefix}{}", dim_branch("  "))
    } else {
        format!("{prefix}{}", dim_branch("| "))
    };
    let child_count = node.children.len();
    for (i, child) in node.children.iter().enumerate() {
        let last = i + 1 == child_count;
        match child {
            Child::Event(event) => {
                let b = if last {
                    dim_branch("└ ")
                } else {
                    dim_branch("|- ")
                };
                lines.push(format!("{child_prefix}{b}{}", format_log_event(event)));
            }
            Child::Span(cid) => {
                render_node(cid, nodes, &child_prefix, last, lines);
            }
        }
    }
}

fn short_target(target: &str) -> &str {
    target.rsplit("::").next().unwrap_or(target)
}

fn build_log_event(event: &Event<'_>) -> Option<LogEvent> {
    let mut visitor = FieldVisitor::default();
    event.record(&mut visitor);
    if visitor.message.is_empty() {
        return None;
    }
    let meta = event.metadata();
    let source = visitor
        .component
        .take()
        .unwrap_or_else(|| short_target(meta.target()).to_string());
    let extras = visitor.extras_string();
    Some(LogEvent {
        level: *meta.level(),
        source,
        message: visitor.message,
        extras,
    })
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
        let Some(log) = build_log_event(event) else {
            return;
        };

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
                node.children.push(Child::Event(log));
            }
            state.refresh();
            return;
        }

        // Flat line — end any live tree so subsequent rewrite cannot clobber it.
        if !state.forest.is_empty() {
            state.finalize();
        }
        state.ui.writeln(&format_flat_line(&log));
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
struct FieldVisitor {
    message: String,
    component: Option<String>,
    extras: Vec<(String, String)>,
}

impl FieldVisitor {
    fn extras_string(&self) -> String {
        if self.extras.is_empty() {
            return String::new();
        }
        self.extras
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn push_extra(&mut self, name: &str, value: String) {
        if name == "message" {
            return;
        }
        if name == "component" {
            self.component = Some(value);
            return;
        }
        // Keep the line scannable; truncate very large payloads (tool args/results).
        let value = if value.chars().count() > 120 {
            let truncated: String = value.chars().take(117).collect();
            format!("{truncated}…")
        } else {
            value
        };
        self.extras.push((name.to_string(), value));
    }
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        let text = format!("{value:?}");
        if field.name() == "message" {
            // Debug of &str includes quotes; prefer record_str when possible.
            self.message = text.trim_matches('"').to_string();
            return;
        }
        self.push_extra(field.name(), text);
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
            return;
        }
        self.push_extra(field.name(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.push_extra(field.name(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.push_extra(field.name(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.push_extra(field.name(), value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::{Child, Forest, LogEvent, TreeNode, format_log_event};
    use std::collections::HashMap;
    use tracing::Level;
    use tracing::span::Id;

    fn id(n: u64) -> Id {
        Id::from_u64(n)
    }

    fn plain(s: &str) -> String {
        console::strip_ansi_codes(s).into_owned()
    }

    enum TestChild {
        Event(LogEvent),
        Span(u64, String, Vec<TestChild>),
    }

    fn info_event(source: &str, message: &str) -> LogEvent {
        LogEvent {
            level: Level::INFO,
            source: source.to_string(),
            message: message.to_string(),
            extras: String::new(),
        }
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
                TestChild::Event(ev) => node_children.push(Child::Event(ev.clone())),
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
        forest.render().iter().map(|l| plain(l)).collect()
    }

    #[test]
    fn out_of_order_children_still_form_correct_tree() {
        let lines = render_forest(&[
            (
                1,
                "LOG_A",
                vec![
                    TestChild::Event(info_event("embedding", "LOG_A_1")),
                    TestChild::Event(info_event("embedding", "LOG_A_2")),
                ],
            ),
            (
                2,
                "LOG_B",
                vec![
                    TestChild::Event(info_event("ccv3_sync", "LOG_B_1")),
                    TestChild::Event(info_event("ccv3_sync", "LOG_B_2")),
                ],
            ),
        ]);

        assert_eq!(lines[0], "|- LOG_A");
        assert!(lines[1].contains("INFO embedding: LOG_A_1"));
        assert!(lines[2].contains("INFO embedding: LOG_A_2"));
        assert_eq!(lines[3], "└ LOG_B");
        assert!(lines[4].contains("INFO ccv3_sync: LOG_B_1"));
        assert!(lines[5].contains("INFO ccv3_sync: LOG_B_2"));
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
                    vec![TestChild::Event(info_event(
                        "streaming_cognitive",
                        "Generating embedding...",
                    ))],
                ),
                TestChild::Span(
                    3,
                    "ccv3_sync".to_string(),
                    vec![TestChild::Event(info_event(
                        "streaming_cognitive",
                        "Synchronizing...",
                    ))],
                ),
            ],
        )]);

        assert_eq!(lines[0], "└ phase_a");
        assert!(lines.iter().any(|l| l.contains("embedding")));
        assert!(lines.iter().any(|l| l.contains("ccv3_sync")));
        assert!(
            lines
                .iter()
                .any(|l| l.contains("INFO streaming_cognitive: Generating embedding..."))
        );
    }

    #[test]
    fn format_includes_level_source_and_extras() {
        let line = plain(&format_log_event(&LogEvent {
            level: Level::WARN,
            source: "MemoryWriter".into(),
            message: "Post-turn memory extraction failed".into(),
            extras: "character_id=Alicia".into(),
        }));
        assert_eq!(
            line,
            "WARN MemoryWriter: Post-turn memory extraction failed character_id=Alicia"
        );
    }
}
