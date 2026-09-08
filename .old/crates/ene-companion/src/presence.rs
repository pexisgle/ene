//! Presence/memory/voice pipeline for #1204.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BodyState {
    Idle,
    LookAt { x: f32, y: f32 },
    Speaking,
    Listening,
}

pub fn body_for_task(task_state: &str, speaking: bool) -> BodyState {
    if speaking {
        return BodyState::Speaking;
    }
    match task_state {
        "running" => BodyState::Idle,
        "completed" => BodyState::LookAt { x: 0.0, y: 0.0 },
        _ => BodyState::Idle,
    }
}

#[derive(Debug, Clone)]
pub struct MemoryRef {
    pub scope: MemoryScope,
    pub provenance: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryScope {
    Private,
    Shared,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_not_contradictory() {
        assert_eq!(body_for_task("running", true), BodyState::Speaking);
        assert_eq!(body_for_task("running", false), BodyState::Idle);
    }

    #[test]
    fn memory_provenance() {
        let m = MemoryRef {
            scope: MemoryScope::Shared,
            provenance: "task:t1".into(),
        };
        assert_eq!(m.scope, MemoryScope::Shared);
    }
}
