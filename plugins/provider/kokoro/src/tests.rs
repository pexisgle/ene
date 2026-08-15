//! Plugin contract tests: synthesis against a scripted fake provider, so no
//! ONNX model file or ONNX Runtime is required.

#![expect(
    clippy::expect_used,
    clippy::float_cmp,
    reason = "unit tests use expect for concise assertions"
)]

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ene_ai::AudioProviderError;
use ene_ai::traits::{TtsChunk, TtsProvider};
use ene_plugin::{ConfigurablePlugin as _, TtsPlugin as _};
use serde_json::{Value, json};
use tokio_stream::Stream;

use crate::config::{DEFAULT_PROFILE, ResolvedConfig};
use crate::plugin::{KokoroPlugin, ensure_kokoro_files_present};

const KIND: &str = "kokoro";

/// Scripted provider standing in for the ONNX engine.
struct FakeTts {
    chunks: Vec<TtsChunk>,
}

impl FakeTts {
    fn single_chunk() -> Self {
        Self {
            chunks: vec![TtsChunk {
                pcm: vec![0.0, 0.5, 1.0, -1.0],
                sample_rate: 24_000,
            }],
        }
    }
}

#[async_trait]
impl TtsProvider for FakeTts {
    fn name(&self) -> &'static str {
        "fake-kokoro"
    }

    async fn synthesize_stream(
        &self,
        _text: &str,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<TtsChunk, AudioProviderError>> + Send>>,
        AudioProviderError,
    > {
        let chunks = self.chunks.clone();
        Ok(Box::pin(tokio_stream::iter(chunks.into_iter().map(Ok))))
    }
}

/// Records the resolved configs handed to the builder, then returns a fake
/// provider. `builds` counts cache misses.
fn counting_builder(
    builds: Arc<AtomicUsize>,
    seen: Arc<Mutex<Vec<ResolvedConfig>>>,
) -> crate::plugin::EngineBuilder {
    Arc::new(move |resolved| {
        builds.fetch_add(1, Ordering::SeqCst);
        seen.lock().expect("seen log").push(resolved.clone());
        Ok(Arc::new(FakeTts::single_chunk()) as Arc<dyn TtsProvider>)
    })
}

fn test_plugin() -> KokoroPlugin {
    KokoroPlugin::with_builder(Arc::new(|_| {
        Ok(Arc::new(FakeTts::single_chunk()) as Arc<dyn TtsProvider>)
    }))
}

#[tokio::test]
async fn synthesize_returns_wav() {
    let plugin = test_plugin();
    let wav = plugin
        .synthesize(
            KIND,
            json!({}),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");

    assert!(wav.starts_with(b"RIFF"));
    assert_eq!(wav.len(), 44 + 4 * 2);
    assert_eq!(
        u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
        24_000
    );
    assert_eq!(&wav[44..], &[0, 0, 0, 64, 255, 127, 1, 128]);
}

#[tokio::test]
async fn wrong_kind_is_not_supported() {
    let plugin = test_plugin();
    let err = plugin
        .synthesize(
            "voicevox",
            json!({}),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("wrong kind rejected");
    assert!(err.to_string().contains("not supported"));
}

#[tokio::test]
async fn wrong_format_is_rejected() {
    let plugin = test_plugin();
    let err = plugin
        .synthesize(
            KIND,
            json!({}),
            "hello".to_string(),
            String::new(),
            "mp3".to_string(),
        )
        .await
        .expect_err("wrong format rejected");
    assert!(err.to_string().contains("only emits wav"));
}

#[tokio::test]
async fn empty_text_is_rejected() {
    let plugin = test_plugin();
    let err = plugin
        .synthesize(
            KIND,
            json!({}),
            "   ".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("empty text rejected");
    assert!(err.to_string().contains("empty text"));
}

#[tokio::test]
async fn invalid_speed_is_rejected() {
    let plugin = test_plugin();
    let err = plugin
        .synthesize(
            KIND,
            json!({"speed": 5.0}),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("out-of-range speed rejected");
    assert!(err.to_string().contains("0.5"));
}

#[tokio::test]
async fn request_voice_wins_over_configured_voice() {
    let builds = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let plugin =
        KokoroPlugin::with_builder(counting_builder(Arc::clone(&builds), Arc::clone(&seen)));

    plugin
        .synthesize(
            KIND,
            json!({"voice": "af_bella", "speed": 1.5, "language": "ja"}),
            "こんにちは".to_string(),
            "jf_alpha".to_string(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");

    let seen = seen.lock().expect("seen log");
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].voice, "jf_alpha");
    assert_eq!(seen[0].speed, 1.5);
    assert_eq!(seen[0].language.as_deref(), Some("ja"));
}

#[tokio::test]
async fn profile_voices_path_is_used_as_fallback() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_in_builder = Arc::clone(&seen);
    let plugin = KokoroPlugin::with_builder(Arc::new(move |resolved| {
        seen_in_builder
            .lock()
            .expect("seen log")
            .push(resolved.clone());
        Ok(Arc::new(FakeTts::single_chunk()) as Arc<dyn TtsProvider>)
    }));
    plugin.set_profiles(&json!({DEFAULT_PROFILE: {"voices_path": "/profile/voices.bin"}}));

    plugin
        .synthesize(
            KIND,
            json!({}),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");

    let seen = seen.lock().expect("seen log");
    assert_eq!(
        seen[0].voices_path,
        std::path::PathBuf::from("/profile/voices.bin")
    );
}

#[tokio::test]
async fn engine_is_reused_until_resolved_config_changes() {
    let builds = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let plugin =
        KokoroPlugin::with_builder(counting_builder(Arc::clone(&builds), Arc::clone(&seen)));

    for _ in 0..2 {
        plugin
            .synthesize(
                KIND,
                json!({"voice": "af_heart"}),
                "hello".to_string(),
                String::new(),
                "wav".to_string(),
            )
            .await
            .expect("synthesis succeeds");
    }
    assert_eq!(
        builds.load(Ordering::SeqCst),
        1,
        "same config reuses the engine"
    );

    plugin
        .synthesize(
            KIND,
            json!({"voice": "jf_alpha"}),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");
    assert_eq!(
        builds.load(Ordering::SeqCst),
        2,
        "voice change rebuilds the engine"
    );
}

#[tokio::test]
async fn unknown_voice_fails_before_building_the_engine() {
    let builds = Arc::new(AtomicUsize::new(0));
    let builds_in_builder = Arc::clone(&builds);
    let plugin = KokoroPlugin::with_builder(Arc::new(move |_| {
        builds_in_builder.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(FakeTts::single_chunk()) as Arc<dyn TtsProvider>)
    }));

    for voice in ["not_a_voice", "  not_a_voice  "] {
        let err = plugin
            .synthesize(
                KIND,
                json!({}),
                "hello".to_string(),
                voice.to_string(),
                "wav".to_string(),
            )
            .await
            .expect_err("unknown voice rejected");
        assert!(err.to_string().contains("unknown Kokoro voice"));
        assert!(err.to_string().contains("available voices"));
    }
    assert_eq!(
        builds.load(Ordering::SeqCst),
        0,
        "no engine build attempted"
    );
}

#[tokio::test]
async fn engine_rebuilds_on_model_path_change() {
    let builds = Arc::new(AtomicUsize::new(0));
    let builds_in_builder = Arc::clone(&builds);
    let plugin = KokoroPlugin::with_builder(Arc::new(move |_| {
        builds_in_builder.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(FakeTts::single_chunk()) as Arc<dyn TtsProvider>)
    }));

    plugin
        .synthesize(
            KIND,
            json!({}),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");
    plugin
        .synthesize(
            KIND,
            json!({"model_path": "/other/kokoro.onnx"}),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");
    assert_eq!(
        builds.load(Ordering::SeqCst),
        2,
        "model path change rebuilds"
    );
}

#[tokio::test]
async fn failed_build_preserves_the_cached_engine() {
    let builds = Arc::new(AtomicUsize::new(0));
    let builds_in_builder = Arc::clone(&builds);
    let plugin = KokoroPlugin::with_builder(Arc::new(move |resolved| {
        builds_in_builder.fetch_add(1, Ordering::SeqCst);
        if resolved
            .model_path
            .to_string_lossy()
            .ends_with("missing.onnx")
        {
            return Err(ene_plugin::PluginError::provider("model init failed"));
        }
        Ok(Arc::new(FakeTts::single_chunk()) as Arc<dyn TtsProvider>)
    }));

    plugin
        .synthesize(
            KIND,
            json!({}),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("synthesis succeeds");
    let err = plugin
        .synthesize(
            KIND,
            json!({"model_path": "/data/missing.onnx"}),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect_err("failed build surfaces the error");
    assert!(err.to_string().contains("model init failed"));

    plugin
        .synthesize(
            KIND,
            json!({}),
            "hello".to_string(),
            String::new(),
            "wav".to_string(),
        )
        .await
        .expect("cached engine still serves");
    assert_eq!(
        builds.load(Ordering::SeqCst),
        2,
        "failed build did not evict the cached engine"
    );
}

#[test]
fn tts_capabilities_shape() {
    let caps = test_plugin().tts_capabilities();
    assert_eq!(caps.len(), 1);
    let spec = &caps[0];
    assert_eq!(spec.kind, "kokoro");
    assert_eq!(spec.formats, vec!["wav"]);
    assert_eq!(spec.voices.len(), 53);
    for voice in ["af_heart", "jf_alpha", "zf_xiaoyi"] {
        assert!(
            spec.voices.iter().any(|v| v == voice),
            "voice {voice} advertised"
        );
    }
    assert_eq!(
        spec.concurrency,
        ene_plugin::ConcurrencyHint {
            max_in_flight: 1,
            queue_depth: 2,
        }
    );
}

#[test]
fn missing_model_files_fail_with_explicit_acquisition_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let resolved = ResolvedConfig {
        model_path: dir.path().join("kokoro.onnx"),
        voices_path: dir.path().join("voices.bin"),
        voice: "af_heart".to_string(),
        speed: 1.0,
        language: None,
        ort_dylib_path: None,
    };
    let err = ensure_kokoro_files_present(&resolved).expect_err("missing files rejected");
    assert!(err.to_string().contains("not found"));
    assert!(err.to_string().contains("Engines page"));

    std::fs::write(&resolved.model_path, b"onnx").expect("write model");
    let err = ensure_kokoro_files_present(&resolved).expect_err("voices.bin still missing");
    assert!(err.to_string().contains("voices.bin"));

    std::fs::write(&resolved.voices_path, b"voices").expect("write voices");
    ensure_kokoro_files_present(&resolved).expect("all files present");
}

#[test]
fn config_schema_is_advertised() {
    let plugin = test_plugin();
    let schema = plugin.config_schema().expect("schema advertised");
    let properties = schema.get("properties").expect("properties object");
    for key in [
        "model_path",
        "voices_path",
        "voice",
        "speed",
        "language",
        "ort_dylib_path",
    ] {
        assert!(properties.get(key).is_some(), "schema has {key}");
    }
    let speed = properties.get("speed").expect("speed property");
    assert_eq!(speed.get("minimum"), Some(&Value::from(0.5)));
    assert_eq!(speed.get("maximum"), Some(&Value::from(2.0)));
}
