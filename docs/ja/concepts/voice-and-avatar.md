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

会話ターン中、`ene-runtime` はチャットバス上で `EneEvent::Performance { turn, origin, cues, source }` をブロードキャストします。`cues: Vec<ene_mind::PerformanceCue>` は `ene-mind` の出力調停から得られます。各 `PerformanceCue` は種別 (`kind`: 表情 / 動作 / 視線 / キャンセル)、キューを識別する `name`、および種別ごとのフィールド (表情の場合は目標ウェイトと保持時間、動作の場合は `MotionLayer`) を持ちます。正確なフィールドは `cargo doc -p ene-mind --open` (`output::performance::PerformanceCue`) を参照してください。

`ene-desktop` はこれらの Performance イベントを受信し、VRM のブレンドシェイプおよびボーンアニメーションにマッピングして再生します。

### 表情の解決

ターン最終の表情は `ene-mind` の表情アービターが、優先度順に以下の3つのソースから決定します。

1. **LLM 提案** — チャットモデルの `<|perf:expr=…|>` マーカー（PHI モード）、または感情分類器の `recommended_expression`（感情モード）。どちらもプロンプトでカードの表情一覧を受け取り、その中の名前を正確に選ぶ必要があります。一覧にない名前は neutral にフォールバックします（曖昧な文字列マッチングは適用されません）。
2. **感情写像** — LLM 提案がない場合、アービターは現在の感情状態を感情空間で*最も近い注釈付き表情*に写像します。注釈はカードの `extensions.ene.expressions[].affect` で表情ごとに定義します:

   ```json
   {
     "name": "にっこり",
     "vrm": { "happy": 1.0 },
     "affect": { "valence": 0.6, "arousal": 0.3 }
   }
   ```

   距離は `valence`, `arousal`, `irritation`, `fatigue` の4次元で計算します（欠損次元は `0.0` 扱い）。`affect` 注釈のない表情はこの経路では選択されません。組み込みのデフォルト表情には従来の閾値写像と等価な注釈が付与されているため、`extensions.expressions` を持たないカードは従来どおり動作します。
3. **neutral フォールバック** — 注釈付きの表情が存在しない場合（または表情一覧が空の場合）。
