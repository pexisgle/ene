# `ene-voice` — API リファレンス

> **クレート**: `ene-voice` | **役割**: ローカル音声 STT, TTS, VAD, および cpal PCM デバイス I/O

`ene-voice` は、ローカルの音声認識 (Whisper)、音声合成、発話区間検出 (Silero VAD)、およびクロスプラットフォームのオーディオデバイスストリーム (`cpal`/`rodio`) をカプセル化します。

---

## 主要コンポーネントと API

### 音声認識 STT (`SttEngine`)
`whisper-rs` を使用して、キャプチャされた PCM オーディオに対してローカル Whisper モデルによるテキスト化を行います：

```rust
pub struct SttEngine { /* ... */ }

impl SttEngine {
    pub async fn transcribe(&self, pcm_samples: &[f32]) -> Result<String, VoiceError>;
}
```

### 発話区間検出 (`SileroVad`)
ONNX Runtime (`ort`) を使用して、ユーザーの発話開始・終了境界をリアルタイムに検出します：

```rust
pub struct SileroVad { /* ... */ }

impl SileroVad {
    pub fn is_speech(&mut self, chunk: &[f32]) -> Result<bool, VoiceError>;
}
```

### 音声合成 TTS (`TtsEngine`)
応答テキストを PCM オーディオバッファに合成し、再生および口パク (Viseme) の計算用に提供します。

---

## 関連ドキュメント
- [音声とアバターの概念](../concepts/voice-and-avatar.md)
- [システムアーキテクチャ](../architecture.md)
