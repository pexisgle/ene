use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const WORKSPACE_EFFECT_PROTOCOL_GENERATION: u32 = 1;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceEffectHandshake {
    pub generation: u32,
}

#[cfg(any(test, feature = "test-support"))]
use crate::WorkspaceEffectStagingPause;
use crate::filesystem::WorkspaceEffectOptions;
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
    pub staging_directory: Option<String>,
    #[cfg(any(test, feature = "test-support"))]
    pub test_pause_after_staging: Option<WorkspaceEffectStagingPause>,
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
    let options = WorkspaceEffectOptions {
        staging_directory: request.staging_directory.as_deref().map(PathBuf::from),
        #[cfg(any(test, feature = "test-support"))]
        pause_after_staging: request.test_pause_after_staging,
    };
    let effect =
        root.execute_with_options(&target, operation, request.content.as_deref(), &options);
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
    let stdin = std::io::stdin();
    let mut input = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    if negotiate_handshake(&mut input, &mut output).is_none() {
        std::process::exit(6);
    }
    drop(output);

    let mut request = Vec::new();
    if input.read_to_end(&mut request).is_err() {
        std::process::exit(2);
    }
    let request = match serde_json::from_slice(&request) {
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

#[doc(hidden)]
pub fn negotiate_workspace_effect_handshake(
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> bool {
    negotiate_handshake(input, output).is_some()
}

fn negotiate_handshake(input: &mut impl BufRead, output: &mut impl Write) -> Option<u32> {
    let mut line = String::new();
    if input.read_line(&mut line).is_err() {
        return None;
    }
    let handshake = serde_json::from_str::<WorkspaceEffectHandshake>(line.trim()).ok()?;
    let response = serde_json::to_string(&WorkspaceEffectHandshake {
        generation: WORKSPACE_EFFECT_PROTOCOL_GENERATION,
    })
    .ok()?;
    if output
        .write_all(response.as_bytes())
        .and_then(|()| output.write_all(b"\n"))
        .and_then(|()| output.flush())
        .is_err()
    {
        return None;
    }
    (handshake.generation == WORKSPACE_EFFECT_PROTOCOL_GENERATION)
        .then_some(WORKSPACE_EFFECT_PROTOCOL_GENERATION)
}

#[cfg(test)]
mod tests {
    use super::{
        WORKSPACE_EFFECT_PROTOCOL_GENERATION, WorkspaceEffectHandshake, negotiate_handshake,
    };

    #[test]
    fn mismatch_response_reports_the_worker_generation() {
        let request = format!(
            "{{\"generation\":{}}}\n",
            WORKSPACE_EFFECT_PROTOCOL_GENERATION + 1
        );
        let mut input = std::io::BufReader::new(request.as_bytes());
        let mut output = Vec::new();

        assert_eq!(negotiate_handshake(&mut input, &mut output), None);

        let response: WorkspaceEffectHandshake =
            serde_json::from_slice(output.strip_suffix(b"\n").expect("handshake newline"))
                .expect("handshake response");
        assert_eq!(response.generation, WORKSPACE_EFFECT_PROTOCOL_GENERATION);
    }
}
