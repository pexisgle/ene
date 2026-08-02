//! Integration tests for the provider capability derive macros:
//! `#[derive(LlmPlugin)]` / `#[derive(TtsPlugin)]` / `#[derive(SttPlugin)]`.
#![expect(
    clippy::expect_used,
    reason = "proc-macro integration tests use expect for spec assertions"
)]

use ene_plugin::prelude::*;

// ── LlmPlugin ────────────────────────────────────────────────────────────

#[derive(LlmPlugin)]
#[provider(
    kind = "test-llm",
    models = "model-a, model-b, model-c",
    streaming,
    vision,
    concurrency = 8,
    queue_depth = 16,
    context_window = 200_000
)]
pub struct TestLlmProvider;

impl ConfigurablePlugin for TestLlmProvider {}

impl LlmPlugin for TestLlmProvider {
    fn llm_capabilities(&self) -> Vec<LlmProviderSpec> {
        vec![Self::llm_spec()]
    }
}

#[test]
fn llm_capabilities_match_attributes() {
    let caps = TestLlmProvider.llm_capabilities();
    assert_eq!(caps.len(), 1);
    let spec = caps.first().expect("one generated spec");
    assert_eq!(spec.kind, "test-llm");
    assert_eq!(
        spec.supported_models,
        vec![
            "model-a".to_string(),
            "model-b".to_string(),
            "model-c".to_string()
        ]
    );
    assert!(spec.supports_streaming);
    assert!(spec.supports_vision);
    assert_eq!(spec.concurrency.max_in_flight, 8);
    assert_eq!(spec.concurrency.queue_depth, 16);
    assert_eq!(spec.context_window, Some(200_000));
}

#[test]
fn llm_provider_kind_const() {
    assert_eq!(TestLlmProvider::LLM_PROVIDER_KIND, "test-llm");
}

// ── TtsPlugin ────────────────────────────────────────────────────────────

#[derive(TtsPlugin)]
#[provider(kind = "test-tts", voices = "voice-a, voice-b", formats = "wav, mp3")]
pub struct TestTtsProvider;

impl ConfigurablePlugin for TestTtsProvider {}

impl TtsPlugin for TestTtsProvider {
    fn tts_capabilities(&self) -> Vec<TtsProviderSpec> {
        vec![Self::tts_spec()]
    }
}

#[test]
fn tts_capabilities_match_attributes() {
    let caps = TestTtsProvider.tts_capabilities();
    assert_eq!(caps.len(), 1);
    let spec = caps.first().expect("one generated spec");
    assert_eq!(spec.kind, "test-tts");
    assert_eq!(
        spec.voices,
        vec!["voice-a".to_string(), "voice-b".to_string()]
    );
    assert_eq!(spec.formats, vec!["wav".to_string(), "mp3".to_string()]);
    // Concurrency defaults to the conservative serial `ConcurrencyHint`.
    assert_eq!(spec.concurrency, ConcurrencyHint::default());
}

#[test]
fn tts_provider_kind_const() {
    assert_eq!(TestTtsProvider::TTS_PROVIDER_KIND, "test-tts");
}

// ── SttPlugin ────────────────────────────────────────────────────────────

#[derive(SttPlugin)]
#[provider(kind = "test-stt", models = "model-x", formats = "wav, flac")]
pub struct TestSttProvider;

impl ConfigurablePlugin for TestSttProvider {}

impl SttPlugin for TestSttProvider {
    fn stt_capabilities(&self) -> Vec<SttProviderSpec> {
        vec![Self::stt_spec()]
    }
}

#[test]
fn stt_capabilities_match_attributes() {
    let caps = TestSttProvider.stt_capabilities();
    assert_eq!(caps.len(), 1);
    let spec = caps.first().expect("one generated spec");
    assert_eq!(spec.kind, "test-stt");
    assert_eq!(spec.models, vec!["model-x".to_string()]);
    assert_eq!(spec.formats, vec!["wav".to_string(), "flac".to_string()]);
    assert_eq!(spec.concurrency, ConcurrencyHint::default());
}

#[test]
fn stt_provider_kind_const() {
    assert_eq!(TestSttProvider::STT_PROVIDER_KIND, "test-stt");
}

// ── Defaults ─────────────────────────────────────────────────────────────

#[derive(LlmPlugin)]
#[provider(kind = "test-default", models = "model-d")]
pub struct DefaultedProvider;

impl ConfigurablePlugin for DefaultedProvider {}

impl LlmPlugin for DefaultedProvider {
    fn llm_capabilities(&self) -> Vec<LlmProviderSpec> {
        vec![Self::llm_spec()]
    }
}

#[test]
fn omitted_flags_and_concurrency_take_defaults() {
    let caps = DefaultedProvider.llm_capabilities();
    let spec = caps.first().expect("one generated spec");
    assert!(!spec.supports_streaming);
    assert!(!spec.supports_vision);
    assert_eq!(spec.concurrency, ConcurrencyHint::default());
    assert_eq!(spec.context_window, None);
}

#[derive(LlmPlugin)]
#[provider(kind = "test-partial", models = "model-p", concurrency = 4)]
pub struct PartialConcurrencyProvider;

impl ConfigurablePlugin for PartialConcurrencyProvider {}

impl LlmPlugin for PartialConcurrencyProvider {
    fn llm_capabilities(&self) -> Vec<LlmProviderSpec> {
        vec![Self::llm_spec()]
    }
}

#[test]
fn partial_concurrency_keeps_default_queue_depth() {
    let caps = PartialConcurrencyProvider.llm_capabilities();
    let spec = caps.first().expect("one generated spec");
    assert_eq!(spec.concurrency.max_in_flight, 4);
    assert_eq!(
        spec.concurrency.queue_depth,
        ConcurrencyHint::default().queue_depth
    );
}

// ── Compound provider (TTS/STT rehearsal) ────────────────────────────────

#[derive(LlmPlugin, TtsPlugin)]
#[provider(
    kind = "combo",
    models = "combo-llm",
    streaming,
    voices = "combo-voice",
    formats = "wav",
    concurrency = 2
)]
pub struct ComboProvider;

impl ConfigurablePlugin for ComboProvider {}

impl LlmPlugin for ComboProvider {
    fn llm_capabilities(&self) -> Vec<LlmProviderSpec> {
        vec![Self::llm_spec()]
    }
}

impl TtsPlugin for ComboProvider {
    fn tts_capabilities(&self) -> Vec<TtsProviderSpec> {
        vec![Self::tts_spec()]
    }
}

#[test]
fn compound_derive_exposes_both_capabilities() {
    let llm_caps = ComboProvider.llm_capabilities();
    let tts_caps = ComboProvider.tts_capabilities();
    let llm = &llm_caps[0];
    let tts = &tts_caps[0];
    assert_eq!(llm.kind, "combo");
    assert_eq!(llm.supported_models, vec!["combo-llm".to_string()]);
    assert!(llm.supports_streaming);
    assert_eq!(llm.concurrency.max_in_flight, 2);

    assert_eq!(tts.kind, "combo");
    assert_eq!(tts.voices, vec!["combo-voice".to_string()]);
    assert_eq!(tts.formats, vec!["wav".to_string()]);
    // Both traits read the same `kind` / `concurrency` from the shared
    // `#[provider(...)]` attribute.
    assert_eq!(tts.concurrency, llm.concurrency);
}

#[test]
fn compound_provider_kind_consts() {
    // Per-trait consts avoid the collision two same-named inherent consts
    // would cause when several provider derives share one struct.
    assert_eq!(ComboProvider::LLM_PROVIDER_KIND, "combo");
    assert_eq!(ComboProvider::TTS_PROVIDER_KIND, "combo");
}
