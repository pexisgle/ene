
use ene_task::TaskTechnicalError;

pub fn unsupported(method: &str) -> TaskTechnicalError {
    TaskTechnicalError::StorageUnavailable {
        reason: format!("{method} is outside this fixture's scope"),
    }
}
