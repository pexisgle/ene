//! Identity Kernel compilation from CCv3 character cards (#82 seed for #100).

use ene_config::CharacterCardV3;

/// Compiled immutable identity block for prompt injection.
#[derive(Debug, Clone)]
pub struct IdentityKernel {
    /// Character display name.
    pub name: String,
    /// Rendered identity kernel text.
    pub text: String,
}

/// Compile a minimal Identity Kernel from a V3 character card.
#[must_use]
pub fn compile_identity_kernel(card: &CharacterCardV3, user_name: &str) -> IdentityKernel {
    let data = &card.data;
    let name = if data.nickname.trim().is_empty() {
        data.name.clone()
    } else {
        data.nickname.clone()
    };

    let mut sections = Vec::new();
    sections.push(format!("# Identity Kernel: {name}"));
    sections.push(format!(
        "You are {name}, a desktop AI companion living on {user_name}'s screen."
    ));

    if !data.system_prompt.trim().is_empty() {
        sections.push(format!(
            "## Core Instructions\n{}",
            data.system_prompt.trim()
        ));
    }
    if !data.personality.trim().is_empty() {
        sections.push(format!("## Personality\n{}", data.personality.trim()));
    }
    if !data.description.trim().is_empty() {
        sections.push(format!("## Background\n{}", data.description.trim()));
    }
    if !data.scenario.trim().is_empty() {
        sections.push(format!("## Current Scene\n{}", data.scenario.trim()));
    }

    IdentityKernel {
        name: name.clone(),
        text: sections.join("\n\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_config::CharacterCardData;

    #[test]
    fn kernel_includes_name_and_system_prompt() {
        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.system_prompt = "Stay cheerful.".into();
        card.data.personality = "Energetic.".into();

        let kernel = compile_identity_kernel(&card, "User");
        assert!(kernel.text.contains("Ene"));
        assert!(kernel.text.contains("Stay cheerful."));
        assert!(kernel.text.contains("Energetic."));
    }

    #[test]
    fn nickname_preferred_over_name() {
        let mut card = CharacterCardV3::default();
        card.data.name = "Official".into();
        card.data.nickname = "Ene".into();
        let kernel = compile_identity_kernel(&card, "User");
        assert!(kernel.text.contains("Ene"));
        assert!(!kernel.text.contains("Official"));
    }
}
