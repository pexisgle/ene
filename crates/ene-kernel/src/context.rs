use ene_session::RecoveryReport;

/// Canonical System Context keys in draw order.
pub const SOURCE_ORDER: &[&str] = &[
    "platform_contract",
    "identity_kernel",
    "character_state",
    "memory.semantic",
    "memory.user_profile",
    "memory.commitments",
    "workspace.context",
    "skills.active",
    "mcp.resources",
    "scene_state",
    "inner_recent",
    "style_examples",
    "interruption_note",
    "delegation.active",
    "delegation.brief",
];

const PLATFORM_CONTRACT: &str =
    "Follow the user. Do not expose inner thoughts or tool internals on the surface.";
const IDENTITY_KERNEL: &str = "You are a local desktop companion. Stay in character using the companion persona from context.";

/// Context Source registry. Layers upsert per-turn snapshots; assemble walks
/// [`SOURCE_ORDER`].
pub struct ContextRegistry {
    sources: Vec<ContextSource>,
}

struct ContextSource {
    key: String,
    text: String,
}

/// Map legacy prefetch keys onto the registry contract.
#[must_use]
pub fn canonicalize_source_key(key: &str) -> &str {
    match key {
        "companion.persona" => "identity_kernel",
        "companion.recall" => "memory.semantic",
        other => other,
    }
}

impl ContextRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sources: vec![
                ContextSource {
                    key: "platform_contract".to_owned(),
                    text: PLATFORM_CONTRACT.to_owned(),
                },
                ContextSource {
                    key: "identity_kernel".to_owned(),
                    text: IDENTITY_KERNEL.to_owned(),
                },
            ],
        }
    }

    /// Drop per-turn sources so a successful empty load does not keep stale text.
    /// Persona / platform / interruption snapshots stay (Unavailable = last good).
    pub fn begin_turn(&mut self) {
        self.sources.retain(|source| {
            source.key == "platform_contract"
                || source.key == "identity_kernel"
                || source.key == "interruption_note"
        });
    }

    pub fn set(&mut self, key: impl Into<String>, text: String) {
        let key = canonicalize_source_key(&key.into()).to_owned();
        if text.trim().is_empty() {
            self.sources.retain(|source| source.key != key);
            return;
        }
        if let Some(existing) = self.sources.iter_mut().find(|source| source.key == key) {
            existing.text = text;
            return;
        }
        self.sources.push(ContextSource { key, text });
    }

    pub fn apply_loaded(&mut self, lines: Vec<(String, String)>) {
        for (key, text) in lines {
            self.set(key, text);
        }
    }

    pub fn set_interruption_note(&mut self, note: Option<String>) {
        self.sources
            .retain(|source| source.key != "interruption_note");
        if let Some(note) = note.filter(|text| !text.trim().is_empty()) {
            self.set("interruption_note", note);
        }
    }

    #[must_use]
    pub fn assemble(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for key in SOURCE_ORDER {
            if let Some(source) = self.sources.iter().find(|source| source.key == *key) {
                out.push((source.key.clone(), source.text.clone()));
            }
        }
        for source in &self.sources {
            if SOURCE_ORDER.contains(&source.key.as_str()) {
                continue;
            }
            out.push((source.key.clone(), source.text.clone()));
        }
        out
    }
}

impl Default for ContextRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Human-readable interruption note for the next surface turn (D-5 / D-13).
#[must_use]
pub fn format_recovery_note(reports: &[RecoveryReport]) -> Option<String> {
    let interrupted: usize = reports
        .iter()
        .map(|report| report.interrupted_turns.len())
        .sum();
    let abandoned: usize = reports
        .iter()
        .map(|report| report.abandoned_inbox.len())
        .sum();
    if interrupted == 0 && abandoned == 0 {
        return None;
    }
    Some(format!(
        "The previous turn was interrupted and was not resumed. \
         Unclaimed inbox items were closed as abandoned_interrupt ({abandoned} item(s)). \
         Do not continue the interrupted work unless the user asks."
    ))
}

#[cfg(test)]
mod tests {
    use super::{ContextRegistry, SOURCE_ORDER, canonicalize_source_key};

    #[test]
    fn identity_kernel_does_not_hardcode_ene() {
        let assembled = ContextRegistry::new().assemble();
        let identity = assembled
            .iter()
            .find(|(key, _)| key == "identity_kernel")
            .map_or("", |(_, text)| text.as_str());
        assert!(!identity.contains("Ene"));
        assert!(identity.contains("companion"));
    }

    #[test]
    fn assemble_follows_source_order() {
        let mut registry = ContextRegistry::new();
        registry.apply_loaded(vec![
            ("delegation.active".to_owned(), "job: research".to_owned()),
            ("character_state".to_owned(), "mood=calm".to_owned()),
            (
                "companion.recall".to_owned(),
                "- picnic: planned".to_owned(),
            ),
            ("companion.persona".to_owned(), "You are Alicia.".to_owned()),
        ]);
        let assembled = registry.assemble();
        let keys: Vec<&str> = assembled.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "platform_contract",
                "identity_kernel",
                "character_state",
                "memory.semantic",
                "delegation.active",
            ]
        );
        let identity = assembled
            .iter()
            .find(|(key, _)| key == "identity_kernel")
            .map(|(_, text)| text.as_str())
            .unwrap();
        assert_eq!(identity, "You are Alicia.");
        let order: Vec<&str> = SOURCE_ORDER
            .iter()
            .copied()
            .filter(|key| keys.contains(key))
            .collect();
        assert_eq!(keys, order);
    }

    #[test]
    fn begin_turn_drops_stale_recall_but_keeps_persona() {
        let mut registry = ContextRegistry::new();
        registry.set("identity_kernel", "You are Alicia.".to_owned());
        registry.set("memory.semantic", "- picnic".to_owned());
        registry.begin_turn();
        let assembled = registry.assemble();
        let keys: Vec<&str> = assembled.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, vec!["platform_contract", "identity_kernel"]);
        assert_eq!(assembled[1].1, "You are Alicia.");
    }

    #[test]
    fn canonicalize_maps_legacy_prefetch_keys() {
        assert_eq!(
            canonicalize_source_key("companion.persona"),
            "identity_kernel"
        );
        assert_eq!(
            canonicalize_source_key("companion.recall"),
            "memory.semantic"
        );
        assert_eq!(canonicalize_source_key("mcp.resources"), "mcp.resources");
    }
}
