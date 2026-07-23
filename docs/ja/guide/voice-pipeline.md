# 音声パイプライン

音声パイプラインにより、キャラクターとの音声会話が可能になります。マイク入力がテキストに文字起こしされ、応答が音声合成され、キャラクターの口が同期して動きます。

[← ガイド索引](index.md) · [English](../../guide/voice-pipeline.md)

## アーキテクチャ概要

```
マイク (cpal) → VAD (Silero) → STT (whisper.cpp) → EneHandle::run(text)
                                                        │
                                                        ▼
                                            LLM ストリーミング (TextDelta)
                                                        │
                                                        ▼
                                            TTS (Kokoro ONNX) → AudioChunk イベント
                                                        │
                                          ┌─────────────┼─────────────┐
                                          ▼             ▼             ▼
                                     rodio 再生      VisemeAnalyzer  セルフボイス
                                     (スピーカー)    (リップシンク)   抑制
```

**データフロー:**

1. **キャプチャ** — `cpal` がマイク音声をキャプチャし、VAD エンジンへ送ります。
2. **VAD** — Silero VAD が発話の開始/終了境界を検出します。
3. **STT** — 発話終了時に、蓄積した PCM を whisper.cpp が文字起こしし、`EneHandle::run` 経由でテキストターンとして送信します。
4. **LLM** — mind ストリーミングパイプラインが応答を `TextDelta` イベントとして生成します。
5. **TTS** — 蓄積したテキストを文単位で Kokoro ONNX が合成し、各チャンクを `EneEvent::AudioChunk` として発行します。
6. **再生** — `rodio` が PCM 音声をデフォルト出力デバイスで再生します。
7. **ビゼーム** — 同じ PCM が `VisemeAnalyzer`（`ene-vrm` 内）を駆動し、口形ウェイト（`aa`/`ih`/`ou`/`ee`/`oh`）を算出して各レンダリングフレームで VRM 表情レイヤーに適用します。

## 音声機能の有効化

### Cargo feature

デスクトップアプリはデフォルトで音声を有効にします:

```toml
# apps/ene-desktop/Cargo.toml
[features]
default = ["voice"]
voice = ["dep:cpal", "dep:rodio"]
```

ネイティブ音声ツールチェーンなしのテキスト専用シェルをビルドする場合:

```bash
rtk cargo build -p ene-desktop --no-default-features
```

`ene-ai` クレートは各ローカルプロバイダーを個別の feature でゲートします:

| Feature | プロバイダー | ネイティブ依存 |
|---------|----------|-------------------|
| `local-stt` | whisper.cpp STT | `whisper-rs` |
| `local-tts` | Kokoro ONNX TTS | `ort`（ONNX Runtime） |
| `silero-vad` | Silero VAD | `ort`（ONNX Runtime） |
| `voice` | 上記すべて | — |

### 設定

`settings.json`（または環境変数）でプロバイダー名を設定します:

```json
{
  "ai": {
    "tts": { "provider": "kokoro", "voice": "af_heart", "speed": 1.0 },
    "stt": { "provider": "whisper", "language": "ja" },
    "vad": { "provider": "silero", "threshold": 0.5 }
  }
}
```

すべてのプロバイダーはデフォルトで `"none"`（無効）です。全フィールドは[設定リファレンス](../reference/configuration/settings.md#ai-tts--text-to-speech)を参照してください。

### モデルファイル

ローカルプロバイダーにはディスク上のモデル重みが必要です。それぞれの解決順序:

1. 明示的な環境変数（`ENE_AI__STT__MODEL_PATH`、`ENE_AI__TTS__MODEL_PATH`、`ENE_AI__VAD__MODEL_PATH`）
2. `ai.{stt,tts,vad}.model` がファイルシステムパスに見える場合
3. デフォルトキャッシュ場所: `{assets_dir}/models/gguf/{whisper.gguf,kokoro.onnx,silero_vad.onnx}`

Kokoro TTS には `voices.bin` ファイルも必要です（`ENE_AI__TTS__VOICES_PATH` または同じキャッシュディレクトリで解決）。

重みは**同梱も自動ダウンロードもされません** — 手動で配置するか、設定で既存ファイルを指してください。

## 対応プロバイダー

| モダリティ | プロバイダー名 | バックエンド | サンプルレート | 備考 |
|----------|--------------|---------|-------------|-------|
| STT | `whisper` | whisper.cpp（`whisper-rs`） | 16 kHz モノ | デバイスレートから自動リサンプル |
| TTS | `kokoro` | Kokoro ONNX（`ort`） | 24 kHz モノ | 約 0.25 秒チャンクをストリーミング。`voices.bin` が必要 |
| VAD | `silero` | Silero VAD v5 ONNX（`ort`） | 16 kHz モノ | 512 サンプル（32 ms）チャンク。閾値設定可能 |

ONNX Runtime は実行時に動的ロードされます（`load-dynamic` feature）— `libonnxruntime` 共有ライブラリが発見可能である必要があります（例：`LD_LIBRARY_PATH` またはデスクトップアプリにバンドルされたライブラリ）。

## デスクトップアプリの使い方

### マイクボタン

チャット UI にマイクトグルボタンがあります。クリックすると:

1. 現在の `AiConfig` から STT と VAD プロバイダーを解決します。
2. 選択した（またはデフォルトの）入力デバイスを `cpal` 経由で開きます。
3. VAD → STT → `AiBridge::run` への音声ストリーミングを開始します。

もう一度クリックするとキャプチャを停止します。STT が無効（`ai.stt.provider = "none"`）の場合、ボタンはエラーを表示します。

### 設定（Features タブ）

Features 設定ページで以下を操作できます:

- **マイクデバイス** — デバイス名オーバーライド（空 = OS デフォルト）
- **VAD 閾値** — 発話確率スライダー（0.0–1.0）
- **STT / TTS プロバイダー** — 設定済みプロバイダーの読み取り専用表示

### セルフボイス抑制

TTS 音声が再生中（`AudioState::tts_playing`）の間、キャプチャコールバックはすべてのマイク入力を破棄し VAD をリセットします。これにより、キャラクター自身の合成音声が文字起こしされて新しいターンに戻ることを防ぎます。再生が終了すると抑制は自動的に解除されます。

## バージイン動作

キャラクターがまだ応答中（または TTS がまだ再生中）の間にユーザーが話し始めた場合:

1. **キャンセル** — デスクトップが `EneCommand::Cancel` をランタイムに送り、LLM ストリームと TTS 合成を停止します。
2. **部分履歴** — `ConversationSession::mark_interrupted` が、それまでに生成された（通常は音声として話された）応答の一部を会話履歴にコミットし、コンテキストが失われないようにします。
3. **中断タグ** — メモリライターが中断されたターンの候補に `"interrupted"` タグを付け、下流の記憶検索が完全な交換と部分的な交換を区別できるようにします。
4. **次ターンコンテキスト** — 次のターンで、`take_interruption()` が中断を認識するシステムプロンプト注記を注入し、モデルが自然に再開または認識できるようにします。

マイクが有効な場合、このフローは自動です — ユーザーが明示的な「停止」操作を行う必要はありません。

## トラブルシューティング

| 症状 | 考えられる原因 | 修正方法 |
|---------|-------------|-----|
| マイクボタンが反応しない | `ai.stt.provider = "none"` | `ai.stt.provider` を `"whisper"` に設定 |
| 起動時に `audio init error` | モデルファイルが見つからない | 期待されるパスに重みを配置するか `ENE_AI__*_MODEL_PATH` を設定 |
| ONNX Runtime が見つからない | `libonnxruntime.so` がライブラリパスにない | `LD_LIBRARY_PATH` を設定するか ONNX Runtime パッケージをインストール |
| リップシンクが動かない | TTS が無効、または `AudioChunk` が消費されていない | `ai.tts.provider` が設定され、デスクトップの `voice` feature が有効であることを確認 |
| キャラクターが自分の声を聞く | セルフボイス抑制が機能していない | 再生が `tts_playing` を正しく設定していることを確認（デスクトップ `voice` feature） |

## 関連ドキュメント

- [ストリーミングイベント](../reference/runtime/streaming-events.md#音声ストリーミング) — `AudioChunk` イベントリファレンス
- [設定](../reference/configuration/settings.md#ai-tts--text-to-speech) — `ai.tts` / `ai.stt` / `ai.vad` フィールド
- [能動発話](proactive-speech.md) — 自発的なコンパニオン発話（音声 I/O とは別）
