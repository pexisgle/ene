# `ene-voice` インターフェース

## 役割

ローカル音声パイプラインエンジン: STT（Whisper）・TTS（Kokoro）・VAD
（Silero）。プロバイダープラグインバイナリがプラグイン IPC 経由で消費します。
このクレートはエンジン層であり、プロバイダーではありません。

## 公開モジュール

| モジュール | ゲート | 内容 |
|---|---|---|
| `local_stt` | `local-stt` | Whisper STT エンジン（`LocalModel` 実装） |
| `local_tts` | `local-tts` | Kokoro TTS エンジン + モデル/ボイスファイル管理（`default_kokoro_model_path`・`default_kokoro_voices_path`・`ensure_kokoro_files_exist`） |
| `silero_vad` | `silero-vad` | Silero VAD エンジン |
| `g2p` | — | Kokoro 用の書記素→音素変換 |
| `ort_init` | ort 系フィーチャー | 共有 ONNX Runtime 初期化子 |

## 依存関係

- 依存: `ene-ai`・`ene-config`・`ene-infer`（+ 任意のネイティブランタイム:
  `whisper-rs`・`ort`）。
- 利用: `plugins/provider/whisper`・`plugins/provider/kokoro`・
  `plugins/provider/onnx`（Silero VAD）。

## リファクタリングの注目点

- ここのエンジンは**`LocalModel` 実装**です。シングルスレッドワーカーの規律は
  `ene-infer` 由来です。プロバイダー側で独自の並行処理を導入しないで
  ください。
- フィーチャーフラグ（`local-stt` / `local-tts` / `silero-vad`）が使わない
  ビルドからネイティブランタイムを除外します。ゲート付きエンジンの追加は
  ゲートを維持し、ORT 初期化子はエンジンごとではなく共有してください。
- モデルファイルは初回使用時に `assets/models/`（gitignore 済み）へ
  ダウンロードされます。パス既定値はプロバイダープラグイン向け
  インターフェースの一部です。
