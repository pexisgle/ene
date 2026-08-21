use ene_session::RecoveryReport;

/// Context Source registry (W0: no epoch; assemble on each turn).
pub struct ContextRegistry {
    sources: Vec<ContextSource>,
}

/// One System Context contributor.
pub struct ContextSource {
    pub key: String,
    renderer: Box<dyn Fn() -> String + Send + Sync>,
}

impl ContextRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sources: vec![
                ContextSource {
                    key: "platform_contract".to_owned(),
                    renderer: Box::new(|| {
                        "Follow the user. Do not expose inner thoughts or tool internals on the surface."
                            .to_owned()
                    }),
                },
                ContextSource {
                    key: "identity_kernel".to_owned(),
                    renderer: Box::new(|| {
                        "You are a local desktop companion. Stay in character using the companion persona from context."
                            .to_owned()
                    }),
                },
            ],
        }
    }

    pub fn set_interruption_note(&mut self, note: Option<String>) {
        self.sources
            .retain(|source| source.key != "interruption_note");
        if let Some(note) = note {
            self.sources.push(ContextSource {
                key: "interruption_note".to_owned(),
                renderer: Box::new(move || note.clone()),
            });
        }
    }

    /// Replace or append a System Context block (skill catalog, host extras).
    pub fn insert(&mut self, key: impl Into<String>, text: String) {
        let key = key.into();
        self.sources.retain(|source| source.key != key);
        self.sources.push(ContextSource {
            key,
            renderer: Box::new(move || text.clone()),
        });
    }

    #[must_use]
    pub fn assemble(&self) -> Vec<(String, String)> {
        self.sources
            .iter()
            .map(|source| (source.key.clone(), (source.renderer)()))
            .collect()
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
    use super::ContextRegistry;

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
    fn insert_replaces_skill_catalog_source() {
        let mut registry = ContextRegistry::new();
        registry.insert(
            "skills.catalog",
            "Installed skills:\n- travel: trips".to_owned(),
        );
        registry.insert(
            "skills.catalog",
            "Installed skills:\n- briefing: mornings".to_owned(),
        );
        let assembled = registry.assemble();
        let catalog = assembled
            .iter()
            .filter(|(key, _)| key == "skills.catalog")
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(catalog.len(), 1);
        assert!(catalog[0].contains("briefing"));
        assert!(!catalog[0].contains("travel"));
    }
}
