use crate::config::VoiceSettings;
use crate::error::BodyError;
use crate::lipsync::{LipSyncAnalyzer, VisemeWeights};
use crate::queue::PerformanceCommand;
use ene_session::BodyId;
use parking_lot::Mutex;
use std::collections::VecDeque;

/// Duplex voice state machine (P-102 / P-103).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplexState {
    Idle,
    Listening,
    Thinking,
    Responding,
    Speaking,
    Interrupting,
}

/// What happened after an input frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEffect {
    Silence,
    IgnoredDisabled,
    IgnoredSelfVoice,
    Listening,
    HoldForMinSpeech,
    BargeIn { body: BodyId },
    Transcript(String),
}

pub trait TtsEngine: Send + Sync {
    fn synthesize(&self, text: &str) -> Result<Vec<f32>, BodyError>;
}

pub trait AsrEngine: Send + Sync {
    fn transcribe(&self, pcm: &[f32]) -> Result<String, BodyError>;
}

/// Energy VAD: RMS above `threshold` is speech.
#[derive(Debug, Clone, Copy)]
pub struct EnergyVad {
    pub threshold: f32,
}

impl Default for EnergyVad {
    fn default() -> Self {
        Self { threshold: 0.02 }
    }
}

impl EnergyVad {
    #[must_use]
    pub fn is_speech(self, pcm: &[f32]) -> bool {
        rms(pcm) >= self.threshold
    }
}

/// Scripted TTS: ~200 ms of tone per character (16 kHz).
#[derive(Debug, Default)]
pub struct ScriptedTts;

impl TtsEngine for ScriptedTts {
    fn synthesize(&self, text: &str) -> Result<Vec<f32>, BodyError> {
        let n = (text.chars().count().max(1) * 3_200).min(48_000);
        Ok((0..n).map(|i| ((i as f32) * 0.12).sin() * 0.25).collect())
    }
}

/// Scripted ASR: returns the next queued transcript, else `"heard"`.
#[derive(Debug, Default)]
pub struct ScriptedAsr {
    replies: Mutex<VecDeque<String>>,
}

impl ScriptedAsr {
    #[must_use]
    pub fn new(replies: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            replies: Mutex::new(replies.into_iter().map(Into::into).collect()),
        }
    }
}

impl AsrEngine for ScriptedAsr {
    fn transcribe(&self, _pcm: &[f32]) -> Result<String, BodyError> {
        Ok(self
            .replies
            .lock()
            .pop_front()
            .unwrap_or_else(|| "heard".to_owned()))
    }
}

/// Core-owned duplex pipeline. Clients only stream frames.
pub struct VoiceRuntime {
    settings: VoiceSettings,
    state: DuplexState,
    vad: EnergyVad,
    tts: Box<dyn TtsEngine>,
    asr: Box<dyn AsrEngine>,
    speaking: Option<BodyId>,
    playback_until_ms: u64,
    speech_started_ms: Option<u64>,
    last_output: Vec<f32>,
    lips: LipSyncAnalyzer,
    last_viseme: VisemeWeights,
}

impl VoiceRuntime {
    #[must_use]
    pub fn new(settings: VoiceSettings, tts: Box<dyn TtsEngine>, asr: Box<dyn AsrEngine>) -> Self {
        Self {
            settings,
            state: DuplexState::Idle,
            vad: EnergyVad::default(),
            tts,
            asr,
            speaking: None,
            playback_until_ms: 0,
            speech_started_ms: None,
            last_output: Vec::new(),
            lips: LipSyncAnalyzer::default(),
            last_viseme: VisemeWeights::default(),
        }
    }

    #[must_use]
    pub fn scripted(settings: VoiceSettings) -> Self {
        Self::new(
            settings,
            Box::new(ScriptedTts),
            Box::new(ScriptedAsr::default()),
        )
    }

    #[must_use]
    pub fn state(&self) -> DuplexState {
        self.state
    }

    #[must_use]
    pub fn speaking_body(&self) -> Option<BodyId> {
        self.speaking
    }

    /// Begin TTS for `body`. One speaker at a time (P-405).
    pub fn speak(
        &mut self,
        body: BodyId,
        text: &str,
        now_ms: u64,
    ) -> Result<SpeakOutput, BodyError> {
        if !self.settings.enabled {
            return Err(BodyError::VoiceDisabled);
        }
        if let Some(current) = self.speaking
            && current != body
            && matches!(self.state, DuplexState::Speaking | DuplexState::Responding)
        {
            return Err(BodyError::SpeakerBusy);
        }
        let pcm = self.tts.synthesize(text)?;
        let duration_ms = pcm_duration_ms(&pcm);
        self.speaking = Some(body);
        self.last_output.clone_from(&pcm);
        self.playback_until_ms = now_ms
            .saturating_add(duration_ms)
            .saturating_add(self.settings.mask_pad_ms);
        self.state = DuplexState::Speaking;
        self.speech_started_ms = None;
        let viseme = self.lips.push(&pcm);
        self.last_viseme = viseme;
        Ok(SpeakOutput {
            pcm,
            lipsync: viseme_command(viseme),
        })
    }

    pub fn push_output_pcm(&mut self, pcm: &[f32], now_ms: u64) -> PerformanceCommand {
        self.last_output = pcm.to_vec();
        let viseme = self.lips.push(pcm);
        self.last_viseme = viseme;
        self.playback_until_ms = now_ms
            .saturating_add(pcm_duration_ms(pcm))
            .saturating_add(self.settings.mask_pad_ms);
        viseme_command(viseme)
    }

    pub fn end_playback(&mut self, now_ms: u64) {
        self.speaking = None;
        self.playback_until_ms = now_ms.saturating_add(self.settings.mask_pad_ms);
        self.state = DuplexState::Idle;
        self.lips = LipSyncAnalyzer::default();
        self.last_viseme = VisemeWeights::default();
        self.last_output.clear();
        self.speech_started_ms = None;
    }

    /// Mic/WS frame. Playback echoes that match TTS are dropped (self-voice).
    pub fn push_input(&mut self, pcm: &[f32], now_ms: u64) -> InputEffect {
        if !self.settings.enabled {
            return InputEffect::IgnoredDisabled;
        }
        let playing = matches!(self.state, DuplexState::Speaking | DuplexState::Responding)
            && now_ms <= self.playback_until_ms;
        if playing && looks_like_playback(pcm, &self.last_output) {
            return InputEffect::IgnoredSelfVoice;
        }
        let speech = self.vad.is_speech(pcm);
        if playing && speech {
            return self.on_possible_barge(now_ms);
        }
        if playing {
            return InputEffect::Silence;
        }
        match self.state {
            DuplexState::Interrupting if speech => {
                self.state = DuplexState::Listening;
                InputEffect::Listening
            }
            DuplexState::Idle | DuplexState::Thinking if speech => {
                self.state = DuplexState::Listening;
                self.speech_started_ms = Some(now_ms);
                InputEffect::Listening
            }
            DuplexState::Listening if speech => InputEffect::Listening,
            DuplexState::Listening => self.finish_utterance(pcm, now_ms),
            _ => InputEffect::Silence,
        }
    }

    fn on_possible_barge(&mut self, now_ms: u64) -> InputEffect {
        if !self.settings.barge_in.enabled {
            return InputEffect::IgnoredSelfVoice;
        }
        let started = *self.speech_started_ms.get_or_insert(now_ms);
        let elapsed = now_ms.saturating_sub(started);
        if elapsed < self.settings.barge_in.min_speech_ms {
            return InputEffect::HoldForMinSpeech;
        }
        let Some(body) = self.speaking else {
            return InputEffect::Silence;
        };
        self.state = DuplexState::Interrupting;
        self.speaking = None;
        self.playback_until_ms = now_ms;
        self.last_output.clear();
        self.speech_started_ms = Some(now_ms);
        InputEffect::BargeIn { body }
    }

    fn finish_utterance(&mut self, pcm: &[f32], now_ms: u64) -> InputEffect {
        let started = self.speech_started_ms.take().unwrap_or(now_ms);
        let elapsed = now_ms.saturating_sub(started);
        if elapsed < self.settings.barge_in.min_speech_ms {
            self.state = DuplexState::Idle;
            return InputEffect::HoldForMinSpeech;
        }
        match self.asr.transcribe(pcm) {
            Ok(text) if !text.trim().is_empty() => {
                self.state = DuplexState::Thinking;
                InputEffect::Transcript(text)
            }
            _ => {
                self.state = DuplexState::Idle;
                InputEffect::HoldForMinSpeech
            }
        }
    }

    pub fn mark_thinking(&mut self) {
        if !matches!(self.state, DuplexState::Speaking) {
            self.state = DuplexState::Thinking;
        }
    }

    #[must_use]
    pub fn last_viseme(&self) -> VisemeWeights {
        self.last_viseme
    }
}

#[derive(Debug)]
pub struct SpeakOutput {
    pub pcm: Vec<f32>,
    pub lipsync: PerformanceCommand,
}

fn pcm_duration_ms(pcm: &[f32]) -> u64 {
    ((pcm.len() as u64) * 1000) / 16_000
}

fn viseme_command(weights: VisemeWeights) -> PerformanceCommand {
    PerformanceCommand::LipSync {
        amplitude: weights.amplitude(),
        viseme: weights.dominant(),
    }
}

fn rms(pcm: &[f32]) -> f32 {
    if pcm.is_empty() {
        return 0.0;
    }
    let sum: f32 = pcm.iter().map(|s| s * s).sum();
    (sum / pcm.len() as f32).sqrt()
}

fn looks_like_playback(input: &[f32], output: &[f32]) -> bool {
    if input.is_empty() || output.is_empty() {
        return false;
    }
    let ir = rms(input);
    let or_ = rms(output);
    if or_ < 0.005 || ir < 0.005 {
        return false;
    }
    let ratio = ir / or_;
    if !(0.5..=1.5).contains(&ratio) {
        return false;
    }
    let n = input.len().min(output.len()).min(256);
    let mut dots = 0.0f32;
    let mut a2 = 0.0f32;
    let mut b2 = 0.0f32;
    for i in 0..n {
        dots += input[i] * output[i];
        a2 += input[i] * input[i];
        b2 += output[i] * output[i];
    }
    let denom = (a2 * b2).sqrt();
    denom > 0.0 && (dots / denom) > 0.85
}

impl Default for VoiceRuntime {
    fn default() -> Self {
        Self::scripted(VoiceSettings::default())
    }
}
