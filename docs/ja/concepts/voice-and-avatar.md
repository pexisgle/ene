# 音声とアバター

## 音声

`ene-body` がデュプレックス音声状態（idle / listening / thinking /
responding / speaking / interrupting）、エネルギー VAD、リップシンクの
viseme を持ちます。TTS / STT は `ai.tasks.tts` / `ai.tasks.stt` で
プロバイダプラグイン（`provider.openai_compat`、`provider.elevenlabs`、
`provider.voicevox`、`provider.edge_tts`）に結びます。音声はプロバイダ
副プロトコル上の `f32` PCM です。マイクと再生デバイスは stage が持ちます。
ポリシーとライブバスはデーモンのままです。排他資源（マイク）は API で請求します。

リップシンクは PCM エネルギーを `ene-vrm` と同じ viseme ターゲットへ
写します。`ene-companion` の感情が表情キューを選び、`ene-body` が
パフォーマンスコマンドとして積みます。

## パフォーマンスコマンド

stage が消費するのは `ene-body::PerformanceCommand` です:

| コマンド | 意味 |
|---|---|
| `Expression` | ブレンドシェイプ表情 |
| `Motion` | モーションクリップ（レイヤー + 強度） |
| `LookAt` | 視線先 |
| `LipSync` | 現在の音声フレームの口形ウェイト |
| `Posture` / vitality | アイドル自律 |

キューはレート制限され、アバターがちらつかないようにします。

## アバター（VRM）

`ene-stage` は `ene-vrm`（wgpu）で **VRM 1.0** を描画します:

- **モデル読み込み** — キャラクターパッケージの `.vrm`
- **モーション** — ブレンドレイヤー付き VRMA
- **表情** — VRM ブレンドシェイプ + 手続き的なまばたき / 視線 / 口
- **Look-at** — 設定範囲内でカーソルを追う
- **スプリングボーン** — 髪 / 布
- **同時表示** — オーバーレイは `body.render.max_concurrent` 体まで合成します
  （既定 2）。ビセームはアクティブなソウルへ。A/D はチャットの対象を付け替え、
  もう一方の体はアンロードしません。

対応する描画 API は
[ene-vrm API リファレンス](../reference/api/ene-vrm.md) です。

## VRMA 再生

モーションがあるとき、stage はアイドルクリップを自動再生します。名前が
`idle` のもの、なければ名前に `VRMA_01` を含むもの、それもなければ最初の
`.vrma` です。クリップを切り替えるとレストポーズとスプリングボーンを
リセットします。

`ene-vrm` は `evaluate_retargeted` で VRMA をサンプリングします。回転は
NormalizedLocalRotation（NLR）で行き先ヒューマノイドのレストへ写し、ヒップ
平行移動は行き先の **ローカル** glTF 値です:

`dst_rest_local + (src_pose - src_rest_local) * (dst_global_y / src_global_y)`。

VRMA の translation チャネルは絶対ローカル値であり、ワールド差分ではありません。
レストのワールド Y に足すとヒップ高さが倍になり、モデルがクリップします。
オーバーレイはヒップの **XZ** を行き先レストに固定し（Y は維持）、歩行が画面内に
収まるようにします。Look-at は VRMA の後に適用します。VRoid モデルはもともと
`+Z` 向きなので、カメラの Y 180° 反転はしません。
