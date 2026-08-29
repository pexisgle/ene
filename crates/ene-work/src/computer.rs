//! Computer control for #1203.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct WindowIdentity(pub String);
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObservationId(pub u64);
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementRef {
    pub window: WindowIdentity,
    pub observation: ObservationId,
    pub id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputerAction {
    pub element: ElementRef,
    pub kind: ActionKind,
    pub grant: GrantId,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActionKind {
    Click,
    Type(String),
    Key(String),
    Scroll(i32),
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GrantId(pub String);
#[derive(Debug, Clone)]
pub struct TaskGrant {
    pub id: GrantId,
    pub scope: GrantScope,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantScope {
    Window(WindowIdentity),
    Task,
}
#[derive(Debug)]
pub enum ComputerError {
    StaleElement,
    FocusLost,
    GrantDenied,
}
pub fn verify_stale(cur: &ObservationId, el: &ElementRef) -> Result<(), ComputerError> {
    if cur.0 != el.observation.0 {
        return Err(ComputerError::StaleElement);
    }
    Ok(())
}
pub fn verify_focus(has: bool) -> Result<(), ComputerError> {
    if !has {
        return Err(ComputerError::FocusLost);
    }
    Ok(())
}
pub fn verify_grant(g: &TaskGrant, a: &ComputerAction) -> Result<(), ComputerError> {
    if g.id != a.grant {
        return Err(ComputerError::GrantDenied);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale() {
        let el = ElementRef {
            window: WindowIdentity("w1".into()),
            observation: ObservationId(1),
            id: "e1".into(),
        };
        assert!(verify_stale(ObservationId(2), &el).is_err());
        assert!(verify_stale(ObservationId(1), &el).is_ok());
    }
    #[test]
    fn focus() {
        assert!(verify_focus(false).is_err());
    }
    #[test]
    fn grant() {
        let g = TaskGrant {
            id: GrantId("g1".into()),
            scope: GrantScope::Task,
        };
        let ok = ComputerAction {
            element: ElementRef {
                window: WindowIdentity("w".into()),
                observation: ObservationId(1),
                id: "e".into(),
            },
            kind: ActionKind::Click,
            grant: GrantId("g1".into()),
        };
        assert!(verify_grant(&g, &ok).is_ok());
        let bad = ComputerAction {
            grant: GrantId("g2".into()),
            ..ok
        };
        assert!(verify_grant(&g, &bad).is_err());
    }
}
