# 音声とアバター

## 音声

`ene-body` がデュプレックス音声状態（idle / listening / thinking /
responding / speaking / interrupting）、エネルギー VAD、リップシンクの
viseme を持ちます。TTS と ASR はトレイトです（`TtsEngine` / `AsrEngine`）。
それらのエンジン用プロバイダープラグインはまだこのツリーに無いので、
`ene-plugin-ipc` へ書き直されるまで会話は Echo のみです。

マイクと再生デバイスは desktop が持ちます。ポリシーとライブバスはデーモンの
ままです。排他資源（マイク）は API で請求します。

リップシンクは PCM エネルギーを `ene-vrm` と同じ viseme ターゲットへ
写します。`ene-companion` の感情が表情キューを選び、`ene-body` が
パフォーマンスコマンドとして積みます。

## パフォーマンスコマンド

desktop が消費するのは `ene-body::PerformanceCommand` です:

| コマンド | 意味 |
|---|---|
| `Expression` | ブレンドシェイプ表情 |
| `Motion` | モーションクリップ（レイヤー + 強度） |
| `LookAt` | 視線先 |
| `LipSync` | 現在の音声フレームの口形ウェイト |
| `Posture` / vitality | アイドル自律 |

キューはレート制限され、アバターがちらつかないようにします。

## アバター（VRM）

`ene-desktop`（任意のデバッグクライアント `ene-stage` も含む）は `ene-vrm`（wgpu）で **VRM 1.0** を描画します:

- **モデル読み込み** — キャラクターパッケージの `.vrm`
- **モーション** — ブレンドレイヤー付き VRMA
- **表情** — VRM ブレンドシェイプ + 手続き的なまばたき / 視線 / 口
- **Look-at** — 設定範囲内でカーソルを追う
- **スプリングボーン** — 髪 / 布

対応する描画 API は
[ene-vrm API リファレンス](../reference/api/ene-vrm.md) です。
