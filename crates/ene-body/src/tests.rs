use crate::{
    BargeInSettings, BodyCatalog, BodySettings, DuplexState, EmotionCue, FallbackSettings,
    InputEffect, PerformanceBus, PerformanceCommand, Stage, Viseme, Vitality, VoiceRuntime,
    VoiceSettings,
};
use ene_session::{BodyId, SoulId};

fn bus() -> PerformanceBus {
    PerformanceBus::default()
}

#[test]
fn emotion_always_emits_expression_even_without_body() {
    let bus = bus();
    let soul = SoulId::new();
    bus.attach(soul, None, BodyCatalog::text_default()).unwrap();
    let issued = bus
        .apply_emotion(
            soul,
            &EmotionCue {
                label: "happy".into(),
                intensity: 0.8,
            },
        )
        .unwrap();
    assert!(issued.body.is_none());
    assert!(matches!(
        issued.command,
        PerformanceCommand::Expression { ref label, .. } if label == "happy"
    ));
    let drained = bus.drain(soul).unwrap();
    assert_eq!(drained.len(), 1);
}

#[test]
fn unknown_emotion_falls_back_with_warning() {
    let bus = bus();
    let soul = SoulId::new();
    bus.attach(soul, Some(BodyId::new()), BodyCatalog::text_default())
        .unwrap();
    let issued = bus
        .apply_emotion(
            soul,
            &EmotionCue {
                label: "overjoyed".into(),
                intensity: 0.9,
            },
        )
        .unwrap();
    assert!(issued.warning.is_some());
    assert!(matches!(
        issued.command,
        PerformanceCommand::Expression { .. }
    ));
}

#[test]
fn missing_map_without_fallback_is_rejected() {
    let bus = PerformanceBus::new(
        FallbackSettings {
            nearest_expression: false,
        },
        crate::AutonomySettings::default(),
    );
    let soul = SoulId::new();
    let mut catalog = BodyCatalog::text_default();
    catalog.emotion_map.clear();
    bus.attach(soul, None, catalog).unwrap();
    let err = bus
        .apply_emotion(
            soul,
            &EmotionCue {
                label: "happy".into(),
                intensity: 1.0,
            },
        )
        .unwrap_err();
    assert!(matches!(err, crate::BodyError::UnknownExpression(_)));
}

#[test]
fn hot_swap_drops_pending_cues() {
    let bus = bus();
    let soul = SoulId::new();
    let a = BodyId::new();
    let b = BodyId::new();
    bus.attach(soul, Some(a), BodyCatalog::text_default())
        .unwrap();
    bus.apply_emotion(
        soul,
        &EmotionCue {
            label: "calm".into(),
            intensity: 0.4,
        },
    )
    .unwrap();
    let generation = bus
        .hot_swap(soul, Some(b), BodyCatalog::text_default())
        .unwrap();
    assert_eq!(generation, 1);
    assert!(bus.drain(soul).unwrap().is_empty());
    assert_eq!(bus.body_of(soul), Some(b));
}

#[test]
fn autonomy_tick_does_not_require_a_turn() {
    let bus = bus();
    let soul = SoulId::new();
    bus.attach(soul, None, BodyCatalog::text_default()).unwrap();
    bus.set_vitality(soul, Vitality::Tired).unwrap();
    let cmds = bus.autonomy_tick(soul).unwrap();
    assert!(
        cmds.iter()
            .any(|c| matches!(c, PerformanceCommand::LookAt { .. }))
    );
}

#[test]
fn lipsync_from_tone_has_amplitude() {
    let pcm: Vec<f32> = (0..1600).map(|i| ((i as f32) * 0.2).sin() * 0.3).collect();
    let weights = crate::LipSyncAnalyzer::analyze(&pcm);
    assert!(weights.amplitude() > 0.05);
    assert!(weights.dominant().is_some());
    assert!(matches!(
        weights.dominant(),
        Some(Viseme::Aa | Viseme::Oh | Viseme::Ou | Viseme::Ee | Viseme::Ih)
    ));
}

fn speech_tone(freq: f32, n: usize) -> Vec<f32> {
    (0..n).map(|i| ((i as f32) * freq).sin() * 0.3).collect()
}

#[test]
fn self_voice_during_playback_is_ignored() {
    let mut voice = VoiceRuntime::scripted(VoiceSettings::default());
    let body = BodyId::new();
    let out = voice.speak(body, "hello there", 0).unwrap();
    assert!(
        matches!(out.lipsync, PerformanceCommand::LipSync { amplitude, .. } if amplitude > 0.0)
    );
    assert_eq!(voice.state(), DuplexState::Speaking);
    let effect = voice.push_input(&out.pcm[..800.min(out.pcm.len())], 20);
    assert_eq!(effect, InputEffect::IgnoredSelfVoice);
}

#[test]
fn barge_in_stops_playback_after_min_speech() {
    let mut voice = VoiceRuntime::scripted(VoiceSettings::default());
    let body = BodyId::new();
    voice.speak(body, "long reply text here", 0).unwrap();
    let other = speech_tone(0.31, 1600);
    let first = voice.push_input(&other, 10);
    assert_eq!(first, InputEffect::HoldForMinSpeech);
    let barged = voice.push_input(&other, 500);
    assert!(matches!(barged, InputEffect::BargeIn { body: b } if b == body));
    assert_eq!(voice.state(), DuplexState::Interrupting);
    assert!(voice.speaking_body().is_none());
}

#[test]
fn interruption_accepts_further_speech_as_listening() {
    let mut voice = VoiceRuntime::scripted(VoiceSettings::default());
    let body = BodyId::new();
    voice.speak(body, "long reply text here", 0).unwrap();
    let other = speech_tone(0.31, 1600);
    voice.push_input(&other, 10);
    voice.push_input(&other, 500);
    assert_eq!(voice.state(), DuplexState::Interrupting);
    let effect = voice.push_input(&other, 600);
    assert_eq!(effect, InputEffect::Listening);
    assert_eq!(voice.state(), DuplexState::Listening);
}

#[test]
fn barge_in_disabled_holds_speaking() {
    let settings = VoiceSettings {
        barge_in: BargeInSettings {
            enabled: false,
            ..BargeInSettings::default()
        },
        ..VoiceSettings::default()
    };
    let mut voice = VoiceRuntime::scripted(settings);
    let body = BodyId::new();
    voice.speak(body, "hello there friends", 0).unwrap();
    let other = speech_tone(0.31, 1600);
    let effect = voice.push_input(&other, 500);
    assert_eq!(effect, InputEffect::IgnoredSelfVoice);
    assert_eq!(voice.state(), DuplexState::Speaking);
    assert_eq!(voice.speaking_body(), Some(body));
}

#[test]
fn short_backchannel_does_not_barge_in() {
    let mut voice = VoiceRuntime::scripted(VoiceSettings::default());
    let body = BodyId::new();
    voice.speak(body, "hello there friends", 0).unwrap();
    let other = speech_tone(0.31, 800);
    let effect = voice.push_input(&other, 200);
    assert_eq!(effect, InputEffect::HoldForMinSpeech);
    assert_eq!(voice.state(), DuplexState::Speaking);
}

#[test]
fn second_body_cannot_speak_over_first() {
    let mut voice = VoiceRuntime::scripted(VoiceSettings::default());
    let a = BodyId::new();
    let b = BodyId::new();
    voice.speak(a, "first", 0).unwrap();
    let err = voice.speak(b, "second", 10).unwrap_err();
    assert!(matches!(err, crate::BodyError::SpeakerBusy));
}

#[test]
fn idle_speech_becomes_transcript() {
    let mut voice = VoiceRuntime::new(
        VoiceSettings::default(),
        Box::new(crate::ScriptedTts),
        Box::new(crate::ScriptedAsr::new(["hello"])),
    );
    let pcm = speech_tone(0.2, 1600);
    assert_eq!(voice.push_input(&pcm, 0), InputEffect::Listening);
    let done = voice.push_input(&[0.0; 160], 500);
    assert_eq!(done, InputEffect::Transcript("hello".into()));
    assert_eq!(voice.state(), DuplexState::Thinking);
}

#[test]
fn stage_caps_concurrent_rendered_bodies() {
    let settings = BodySettings {
        render: crate::RenderSettings {
            enabled: true,
            max_concurrent: 1,
        },
        ..BodySettings::default()
    };
    let stage = Stage::new(
        std::sync::Arc::new(PerformanceBus::default()),
        VoiceRuntime::scripted(VoiceSettings::default()),
        settings,
    );
    let s1 = SoulId::new();
    let s2 = SoulId::new();
    stage
        .present(s1, Some(BodyId::new()), BodyCatalog::text_default())
        .unwrap();
    stage
        .present(s2, Some(BodyId::new()), BodyCatalog::text_default())
        .unwrap();
    assert!(stage.bus().body_of(s1).is_some());
    assert!(stage.bus().body_of(s2).is_none());
    let occupants = stage.occupants();
    assert_eq!(occupants.len(), 2);
    assert!(
        occupants
            .iter()
            .any(|(soul, body)| *soul == s1 && body.is_some())
    );
    assert!(
        occupants
            .iter()
            .any(|(soul, body)| *soul == s2 && body.is_none())
    );
}

#[test]
fn commands_never_include_pad_numbers() {
    let bus = bus();
    let soul = SoulId::new();
    bus.attach(soul, None, BodyCatalog::text_default()).unwrap();
    let issued = bus
        .apply_emotion(
            soul,
            &EmotionCue {
                label: "sad".into(),
                intensity: 0.5,
            },
        )
        .unwrap();
    let blob = serde_json::to_string(&issued.command).unwrap();
    assert!(!blob.contains("valence"));
    assert!(!blob.contains("arousal"));
}
