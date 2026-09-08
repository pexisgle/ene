use crate::block::Block;
use crate::error::SessionError;
use crate::event::{EventKind, EventPayload, LoggedEvent, TurnOutcome};
use crate::ids::TurnId;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// How inner messages appear in a projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InnerVisibility {
    /// Table-surface UI and default export.
    Off,
    /// Model history: trailing `model_visible` window.
    SelfReference,
    /// Detail view: every inner message.
    Full,
}

/// How thinking blocks appear in a projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingVisibility {
    Off,
    /// Provider multi-turn replay.
    Provider,
    Full,
}

/// Display-plane depth (D-11). Projection parameters follow from this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayDepth {
    Surface,
    Detail,
}

impl DisplayDepth {
    /// Parse `surface` / `detail`. Unknown values are rejected.
    pub fn parse(raw: &str) -> Result<Self, SessionError> {
        match raw {
            "surface" => Ok(Self::Surface),
            "detail" => Ok(Self::Detail),
            other => Err(SessionError::InvalidId(format!("display depth {other}"))),
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Surface => "surface",
            Self::Detail => "detail",
        }
    }

    #[must_use]
    pub const fn inner(self) -> InnerVisibility {
        match self {
            Self::Surface => InnerVisibility::Off,
            Self::Detail => InnerVisibility::Full,
        }
    }

    #[must_use]
    pub const fn thinking(self) -> ThinkingVisibility {
        match self {
            Self::Surface => ThinkingVisibility::Off,
            Self::Detail => ThinkingVisibility::Full,
        }
    }

    #[must_use]
    pub const fn include_tool_args(self) -> bool {
        matches!(self, Self::Detail)
    }
}

/// Role of a projected message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    System,
    Thinking,
    Inner,
    Tool,
    Status,
}

/// One reconstructed history item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectedMessage {
    pub seq: u64,
    pub role: Role,
    pub blocks: Vec<Block>,
    pub turn_id: Option<TurnId>,
    pub step_index: Option<u32>,
    pub tool_name: Option<String>,
    pub tool_args: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ProjectedMessage {
    #[must_use]
    pub fn text(&self) -> String {
        self.blocks
            .iter()
            .filter_map(Block::as_text)
            .collect::<Vec<_>>()
            .join("")
    }
}

/// Result of `derive_messages`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectedHistory {
    pub messages: Vec<ProjectedMessage>,
    pub truncated_prefix: bool,
}

/// Projection knobs. Callers must not pass `Full` inner/thinking to a surface UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectOptions {
    pub inner: InnerVisibility,
    pub thinking: ThinkingVisibility,
    pub apply_redaction: bool,
    pub include_tool_args: bool,
    pub self_reference_window: u32,
    pub include_turn_failures: bool,
    /// Exclude incomplete tool-call/result exchanges from provider-visible history.
    ///
    /// Surface and export projections keep them visible for diagnostics.
    pub isolate_incomplete_tool_groups: bool,
}

impl ProjectOptions {
    #[must_use]
    pub const fn for_depth(depth: DisplayDepth, self_reference_window: u32) -> Self {
        Self {
            inner: depth.inner(),
            thinking: depth.thinking(),
            apply_redaction: true,
            include_tool_args: depth.include_tool_args(),
            self_reference_window,
            include_turn_failures: true,
            isolate_incomplete_tool_groups: false,
        }
    }

    #[must_use]
    pub const fn model_visible(self_reference_window: u32) -> Self {
        Self {
            inner: InnerVisibility::SelfReference,
            thinking: ThinkingVisibility::Provider,
            apply_redaction: true,
            include_tool_args: true,
            self_reference_window,
            include_turn_failures: false,
            isolate_incomplete_tool_groups: true,
        }
    }
}

#[expect(
    clippy::struct_field_names,
    reason = "field names match the compaction payload (from_seq/to_seq)"
)]
struct CompactionRange {
    from_seq: u64,
    to_seq: u64,
    summary_event_seq: u64,
}

/// Rebuild history from a seq-ordered event list (L-1 / I-6).
#[must_use]
pub fn derive_messages(events: &[LoggedEvent], options: ProjectOptions) -> ProjectedHistory {
    let redacted = collect_redactions(events, options.apply_redaction);
    let compacted = collect_compactions(events);
    let mut messages = Vec::new();
    let mut pending_inner: Vec<ProjectedMessage> = Vec::new();

    for event in events {
        if should_skip_compacted(event.seq, event.kind.as_str(), &compacted) {
            continue;
        }
        if matches!(event.payload, EventPayload::Skipped { .. }) {
            continue;
        }
        let blocks = redact_blocks(event, &redacted);
        match event.kind {
            EventKind::UserMessage => messages.push(ProjectedMessage {
                seq: event.seq,
                role: Role::User,
                blocks,
                turn_id: event.payload.turn_id(),
                step_index: None,
                tool_name: None,
                tool_args: None,
                tool_call_id: None,
            }),
            EventKind::AssistantMessage => {
                let (turn_id, step_index) = assistant_meta(&event.payload);
                messages.push(ProjectedMessage {
                    seq: event.seq,
                    role: Role::Assistant,
                    blocks,
                    turn_id,
                    step_index,
                    tool_name: None,
                    tool_args: None,
                    tool_call_id: None,
                });
            }
            EventKind::AssistantThinking
                if !matches!(options.thinking, ThinkingVisibility::Off) =>
            {
                let (turn_id, step_index) = assistant_meta(&event.payload);
                messages.push(ProjectedMessage {
                    seq: event.seq,
                    role: Role::Thinking,
                    blocks,
                    turn_id,
                    step_index,
                    tool_name: None,
                    tool_args: None,
                    tool_call_id: None,
                });
            }
            EventKind::InnerMessage => {
                if let Some(msg) = project_inner(event, blocks, options) {
                    pending_inner.push(msg);
                }
            }
            EventKind::ContextSystemMessage => {
                messages.push(ProjectedMessage {
                    seq: event.seq,
                    role: Role::System,
                    blocks,
                    turn_id: None,
                    step_index: None,
                    tool_name: None,
                    tool_args: None,
                    tool_call_id: None,
                });
            }
            EventKind::SessionSummary => {
                let summary_blocks = match &event.payload {
                    EventPayload::SessionSummary { summary, .. } => {
                        vec![Block::text(summary.clone())]
                    }
                    _ => blocks,
                };
                messages.push(ProjectedMessage {
                    seq: event.seq,
                    role: Role::System,
                    blocks: summary_blocks,
                    turn_id: None,
                    step_index: None,
                    tool_name: None,
                    tool_args: None,
                    tool_call_id: None,
                });
            }
            EventKind::TurnEnd if options.include_turn_failures => {
                if let EventPayload::TurnEnd {
                    outcome: TurnOutcome::Failed,
                    turn_id,
                    error_detail,
                    error_class,
                    ..
                } = &event.payload
                {
                    let text = error_detail
                        .clone()
                        .or_else(|| error_class.clone())
                        .unwrap_or_else(|| "turn failed".to_owned());
                    messages.push(ProjectedMessage {
                        seq: event.seq,
                        role: Role::Status,
                        blocks: vec![Block::text(text)],
                        turn_id: Some(*turn_id),
                        step_index: None,
                        tool_name: None,
                        tool_args: None,
                        tool_call_id: None,
                    });
                }
            }
            EventKind::ToolCall => {
                if let EventPayload::ToolCall {
                    tool_name,
                    args,
                    turn_id,
                    step_index,
                    call_id,
                    ..
                } = &event.payload
                {
                    let projected_blocks = if options.include_tool_args {
                        vec![Block::text(format!("{tool_name} {args}"))]
                    } else {
                        vec![Block::text(tool_name.clone())]
                    };
                    messages.push(ProjectedMessage {
                        seq: event.seq,
                        role: Role::Tool,
                        blocks: projected_blocks,
                        turn_id: Some(*turn_id),
                        step_index: Some(*step_index),
                        tool_name: Some(tool_name.clone()),
                        tool_args: options.include_tool_args.then(|| args.clone()),
                        tool_call_id: Some(call_id.to_string()),
                    });
                }
            }
            EventKind::ToolResult | EventKind::ToolSpill if options.include_tool_args => {
                let tool_call_id = match &event.payload {
                    EventPayload::ToolResult { call_id, .. }
                    | EventPayload::ToolSpill { call_id, .. } => Some(call_id.to_string()),
                    _ => None,
                };
                messages.push(ProjectedMessage {
                    seq: event.seq,
                    role: Role::Tool,
                    blocks,
                    turn_id: None,
                    step_index: None,
                    tool_name: None,
                    tool_args: None,
                    tool_call_id,
                });
            }
            _ => {}
        }
    }

    if options.isolate_incomplete_tool_groups {
        isolate_incomplete_tool_groups(&mut messages);
    }
    append_inner(&mut messages, pending_inner, options);
    ProjectedHistory {
        messages,
        truncated_prefix: !compacted.is_empty(),
    }
}

struct ToolProjectionGroup {
    call_ids: HashSet<String>,
    result_ids: HashSet<String>,
    first_index: usize,
    last_index: usize,
    valid: bool,
}

fn isolate_incomplete_tool_groups(messages: &mut Vec<ProjectedMessage>) {
    let mut groups = Vec::new();
    let mut groups_by_key = HashMap::new();
    let mut groups_by_call_id: HashMap<String, usize> = HashMap::new();
    let mut message_groups = vec![None; messages.len()];
    let mut keep_result = vec![false; messages.len()];

    for (index, message) in messages.iter().enumerate() {
        if message.role != Role::Tool || message.tool_args.is_none() {
            continue;
        }
        let Some(call_id) = message.tool_call_id.clone() else {
            continue;
        };
        let Some(turn_id) = message.turn_id else {
            continue;
        };
        let Some(step_index) = message.step_index else {
            continue;
        };
        let group_index = *groups_by_key
            .entry((turn_id, step_index))
            .or_insert_with(|| {
                let group_index = groups.len();
                groups.push(ToolProjectionGroup {
                    call_ids: HashSet::new(),
                    result_ids: HashSet::new(),
                    first_index: index,
                    last_index: index,
                    valid: true,
                });
                group_index
            });
        groups[group_index].first_index = groups[group_index].first_index.min(index);
        groups[group_index].last_index = groups[group_index].last_index.max(index);
        if let Some(previous) = groups_by_call_id.get(call_id.as_str()).copied() {
            // The same call id logged twice inside one group is the intent
            // event and the result-bearing record of one invocation; a reuse
            // across different groups is still a projection fault.
            if previous != group_index {
                groups[previous].valid = false;
                groups[group_index].valid = false;
                continue;
            }
            message_groups[index] = Some(group_index);
            continue;
        }
        groups_by_call_id.insert(call_id.clone(), group_index);
        groups[group_index].call_ids.insert(call_id);
        message_groups[index] = Some(group_index);
    }

    for (index, message) in messages.iter().enumerate() {
        if message.role != Role::Tool || message.tool_args.is_some() {
            continue;
        }
        let Some(call_id) = message.tool_call_id.as_deref() else {
            continue;
        };
        let Some(&group_index) = groups_by_call_id.get(call_id) else {
            continue;
        };
        groups[group_index].first_index = groups[group_index].first_index.min(index);
        groups[group_index].last_index = groups[group_index].last_index.max(index);
        message_groups[index] = Some(group_index);
        if groups[group_index].result_ids.insert(call_id.to_owned()) {
            keep_result[index] = true;
        }
    }

    for (group_index, group) in groups.iter_mut().enumerate() {
        if group.call_ids.is_empty() || group.call_ids != group.result_ids {
            group.valid = false;
        }
        if group.valid
            && (group.first_index..=group.last_index).any(|index| {
                messages[index].role != Role::Tool || message_groups[index] != Some(group_index)
            })
        {
            group.valid = false;
        }
    }

    let valid_groups: HashSet<usize> = groups
        .iter()
        .enumerate()
        .filter_map(|(index, group)| group.valid.then_some(index))
        .collect();
    let mut keep = vec![false; messages.len()];
    for (index, message) in messages.iter().enumerate() {
        keep[index] = if message.role == Role::Tool {
            message_groups[index].is_some_and(|group_index| {
                valid_groups.contains(&group_index)
                    && (message.tool_args.is_some() || keep_result[index])
            })
        } else {
            true
        };
    }
    let original = std::mem::take(messages);
    *messages = original
        .into_iter()
        .enumerate()
        .filter_map(|(index, message)| keep[index].then_some(message))
        .collect();
}

fn collect_redactions(events: &[LoggedEvent], apply: bool) -> Vec<u64> {
    if !apply {
        return Vec::new();
    }
    events
        .iter()
        .filter_map(|event| match event.payload {
            EventPayload::Redaction { target_seq, .. } => Some(target_seq),
            _ => None,
        })
        .collect()
}

fn collect_compactions(events: &[LoggedEvent]) -> Vec<CompactionRange> {
    events
        .iter()
        .filter_map(|event| match event.payload {
            EventPayload::CompactionApplied {
                from_seq,
                to_seq,
                summary_event_seq,
                ..
            } => Some(CompactionRange {
                from_seq,
                to_seq,
                summary_event_seq,
            }),
            _ => None,
        })
        .collect()
}

fn should_skip_compacted(seq: u64, kind: &str, ranges: &[CompactionRange]) -> bool {
    ranges.iter().any(|range| {
        seq >= range.from_seq
            && seq < range.to_seq
            && seq != range.summary_event_seq
            && kind != "compaction/applied"
    })
}

fn redact_blocks(event: &LoggedEvent, redacted: &[u64]) -> Vec<Block> {
    if redacted.contains(&event.seq) {
        return vec![Block::redacted("redacted")];
    }
    event.payload.blocks().to_vec()
}

fn assistant_meta(payload: &EventPayload) -> (Option<TurnId>, Option<u32>) {
    match *payload {
        EventPayload::AssistantMessage {
            turn_id,
            step_index,
            ..
        }
        | EventPayload::AssistantThinking {
            turn_id,
            step_index,
            ..
        } => (Some(turn_id), Some(step_index)),
        _ => (None, None),
    }
}

fn project_inner(
    event: &LoggedEvent,
    blocks: Vec<Block>,
    options: ProjectOptions,
) -> Option<ProjectedMessage> {
    let EventPayload::InnerMessage {
        turn_id,
        step_index,
        model_visible,
        ..
    } = &event.payload
    else {
        return None;
    };
    let include = match options.inner {
        InnerVisibility::Off => false,
        InnerVisibility::Full => true,
        InnerVisibility::SelfReference => *model_visible,
    };
    include.then(|| ProjectedMessage {
        seq: event.seq,
        role: Role::Inner,
        blocks,
        turn_id: *turn_id,
        step_index: *step_index,
        tool_name: None,
        tool_args: None,
        tool_call_id: None,
    })
}

fn append_inner(
    messages: &mut Vec<ProjectedMessage>,
    mut pending: Vec<ProjectedMessage>,
    options: ProjectOptions,
) {
    if matches!(options.inner, InnerVisibility::SelfReference) {
        let window = options.self_reference_window as usize;
        if pending.len() > window {
            let skip = pending.len().saturating_sub(window);
            pending = pending.split_off(skip);
        }
    }
    messages.extend(pending);
}

/// Hash projected messages for L-1 capture tests.
pub fn hash_projected(history: &ProjectedHistory) -> Result<String, SessionError> {
    let encoded = rmp_serde::to_vec_named(&history.messages).map_err(SessionError::codec)?;
    Ok(blake3::hash(&encoded).to_hex().to_string())
}

/// Hash an arbitrary serializable model-visible payload.
pub fn hash_model_visible<T: Serialize>(value: &T) -> Result<String, SessionError> {
    let encoded = rmp_serde::to_vec_named(value).map_err(SessionError::codec)?;
    Ok(blake3::hash(&encoded).to_hex().to_string())
}

pub fn surface_leaks_inner(history: &ProjectedHistory) -> bool {
    history
        .messages
        .iter()
        .any(|message| message.role == Role::Inner || message.role == Role::Thinking)
}
