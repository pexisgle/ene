use serde::{Deserialize, Serialize};

use crate::{
    ActionCertainty, ActionOutput, EffectGrounds, ListEntry, ListEntryKind, ObservedEffect,
    OperationKind, RealTargetRef, WorkspaceRoot,
};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceEffectRequest {
    pub root: String,
    pub target: String,
    pub operation: String,
    pub content: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceEffectResponse {
    pub certainty: String,
    pub grounds: String,
    pub output: Option<WorkspaceEffectOutput>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceEffectOutput {
    Bytes(Vec<u8>),
    Listing(Vec<WorkspaceEffectListEntry>),
    Created { target: String },
    Updated,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceEffectListEntry {
    pub name: String,
    pub kind: String,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceEffectWorkerError {
    #[error("workspace root unavailable")]
    WorkspaceUnavailable,
    #[error("workspace effect request invalid")]
    InvalidRequest,
}

pub fn execute_workspace_effect(
    request: WorkspaceEffectRequest,
) -> Result<WorkspaceEffectResponse, WorkspaceEffectWorkerError> {
    let operation = OperationKind::from_name(&request.operation)
        .ok_or(WorkspaceEffectWorkerError::InvalidRequest)?;
    let root = WorkspaceRoot::open(&request.root)
        .map_err(|_| WorkspaceEffectWorkerError::WorkspaceUnavailable)?;
    let target = RealTargetRef::from_canonical_path(request.target);
    let effect = root.execute(&target, operation, request.content.as_deref());
    Ok(WorkspaceEffectResponse {
        certainty: effect.certainty.as_str().to_owned(),
        grounds: effect.grounds.as_str().to_owned(),
        output: effect.output.map(encode_output),
    })
}

fn encode_output(output: ActionOutput) -> WorkspaceEffectOutput {
    match output {
        ActionOutput::Bytes(bytes) => WorkspaceEffectOutput::Bytes(bytes),
        ActionOutput::Listing(entries) => WorkspaceEffectOutput::Listing(
            entries
                .into_iter()
                .map(|entry| WorkspaceEffectListEntry {
                    name: entry.name,
                    kind: match entry.kind {
                        ListEntryKind::File => String::from("file"),
                        ListEntryKind::Directory => String::from("directory"),
                    },
                })
                .collect(),
        ),
        ActionOutput::Created { target } => WorkspaceEffectOutput::Created {
            target: target.as_path().to_owned(),
        },
        ActionOutput::Updated => WorkspaceEffectOutput::Updated,
    }
}

impl WorkspaceEffectResponse {
    pub fn into_effect(self) -> Result<ObservedEffect, WorkspaceEffectWorkerError> {
        let certainty = ActionCertainty::from_name(&self.certainty)
            .ok_or(WorkspaceEffectWorkerError::InvalidRequest)?;
        let grounds = EffectGrounds::from_name(&self.grounds)
            .ok_or(WorkspaceEffectWorkerError::InvalidRequest)?;
        let output = self.output.map(decode_output).transpose()?;
        Ok(ObservedEffect {
            certainty,
            grounds,
            output,
        })
    }
}

fn decode_output(
    output: WorkspaceEffectOutput,
) -> Result<ActionOutput, WorkspaceEffectWorkerError> {
    match output {
        WorkspaceEffectOutput::Bytes(bytes) => Ok(ActionOutput::Bytes(bytes)),
        WorkspaceEffectOutput::Listing(entries) => entries
            .into_iter()
            .map(|entry| {
                let kind = match entry.kind.as_str() {
                    "file" => ListEntryKind::File,
                    "directory" => ListEntryKind::Directory,
                    _ => return Err(WorkspaceEffectWorkerError::InvalidRequest),
                };
                Ok(ListEntry {
                    name: entry.name,
                    kind,
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(ActionOutput::Listing),
        WorkspaceEffectOutput::Created { target } => Ok(ActionOutput::Created {
            target: RealTargetRef::from_canonical_path(target),
        }),
        WorkspaceEffectOutput::Updated => Ok(ActionOutput::Updated),
    }
}

#[doc(hidden)]
pub fn run_workspace_effect_worker() {
    use std::io::{Read, Write};

    let mut input = Vec::new();
    if std::io::stdin().read_to_end(&mut input).is_err() {
        std::process::exit(2);
    }
    let request = match serde_json::from_slice(&input) {
        Ok(request) => request,
        Err(_) => std::process::exit(2),
    };
    let response = match execute_workspace_effect(request) {
        Ok(response) => response,
        Err(_) => std::process::exit(3),
    };
    let encoded = match serde_json::to_vec(&response) {
        Ok(encoded) => encoded,
        Err(_) => std::process::exit(4),
    };
    let mut stdout = std::io::stdout().lock();
    if stdout.write_all(&encoded).is_err() || stdout.flush().is_err() {
        std::process::exit(5);
    }
}
