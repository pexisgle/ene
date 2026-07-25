# ローカル音声パイプラインと VRM 3D アバター描画

本ドキュメントでは、Ene のローカル音声処理エンジン (`ene-voice`) および 3D アバター描画システム (`ene-vrm`) について解説します。

---

## 1. ローカル音声パイプライン (`ene-voice`)

Ene はオプションで完全ローカルな音声対話パイプラインを提供します：

```text
マイク入力
  │
  ├─> 1. オーディオ録音 (cpal ストリームキャプチャ)
  ├─> 2. 発話区間検出 (Silero VAD、ONNX Runtime / ort)
  ├─> 3. 音声認識 STT (Whisper モデル、whisper-rs)
  │
  └─> LLM ターン処理
        │
        ├─> 4. 音声合成 TTS (TTS エンジン)
        └─> 5. オーディオ再生 & 口パク連動 (rodio)
```

### 構成要素
- **STT (音声認識)**: `whisper-rs` を用いたローカル Whisper モデルによる高速認識。
- **VAD (発話区間検出)**: ONNX Runtime (`ort`) 上で動作する Silero VAD によるリアルタイム発話判定。
- **TTS (音声合成)**: ローカル音声合成エンジンにより再生用 PCM バッファを生成。
- **オーディオデバイス**: `cpal` および `rodio` を介した低レイテンシ PCM キャプチャ・出力。

---

## 2. VRM 3D アバター描画 (`ene-vrm`)

`ene-vrm` は `ene-desktop` 内で VRM 1.0 3D モデルを表示するための独立した wgpu レンダラーです。

### アーキテクチャ上の完全な独立性
`ene-vrm` は `ene-mind`, `ene-runtime`, `ene-store` に**一切依存しません**。純粋なメッシュ、テクスチャ、ボーン変換行列、ブレンドシェイプウェイトのみを受け取ってレンダリングします。

### Performance キューのマッピング

会話ターン中、 `ene-mind` はアバターの感情表現を含む `EneEvent::Performance` キューを発行します：

```rust
pub struct PerformanceCue {
    pub expression: ExpressionKind, // Happy, Angry, Surprised, Neutral など
    pub blink: bool,
    pub viseme: Option<VisemeCategory>, // あ, い, う, え, お の口形状
    pub motion: Option<MotionPreset>,
}
```

`ene-desktop` はこれらの Performance イベントを受信し、VRM のブレンドシェイプおよびボーンアニメーションに直接マッピングして再生します。
