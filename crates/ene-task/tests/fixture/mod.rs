//! Shared `TaskRepository` stubs for `ene-task` integration tests.

use ene_task::TaskTechnicalError;

/// The refusal every fake repository returns for a method outside the test's
/// scenario, so the reason strings cannot drift between fixtures.
pub fn unsupported(method: &str) -> TaskTechnicalError {
    TaskTechnicalError::StorageUnavailable {
        reason: format!("{method} is outside this fixture's scope"),
    }
}
