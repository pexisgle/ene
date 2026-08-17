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
                    renderer: Box::new(|| "You are a local companion named Ene.".to_owned()),
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
