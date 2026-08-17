use crate::request::{AuthzRequest, Sensitivity};

/// Combined side-effect + sensitivity risk (approval.md §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Risk {
    None,
    Low,
    Medium,
    High,
}

impl Risk {
    #[must_use]
    pub fn classify(req: &AuthzRequest) -> Self {
        if is_high(req) {
            return Self::High;
        }
        if req.sensitivity == Sensitivity::High || req.tool == "app.screenshot" {
            return Self::Medium;
        }
        if req.sensitivity == Sensitivity::Medium {
            return Self::Medium;
        }
        if (req.tool.starts_with("fs.read") || req.tool == "fs.read") && !req.in_workspace {
            return Self::Medium;
        }
        if !req.side_effects.is_empty() && !req.in_workspace {
            return Self::Medium;
        }
        if req.side_effects.iter().any(|effect| effect == "fs.write") && req.in_workspace {
            return Self::Low;
        }
        if req.side_effects.is_empty() {
            Self::None
        } else {
            Self::Low
        }
    }
}

fn is_high(req: &AuthzRequest) -> bool {
    req.tool.starts_with("exec.")
        || req.tool.contains("cred.export")
        || req
            .side_effects
            .iter()
            .any(|effect| effect == "exec" || effect == "send" || effect == "cred.export")
}
