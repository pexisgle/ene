# 感情とアバター Performance

モデルは気分を表現でき、Desktop はそれを VRM アバターに再生します。

## 2 つの層

1. **Affect（感情状態）** — エンジン側で保持する感情状態。ターンをまたいで更新され、`store` 有効時は永続化される
2. **Performance キュー（演出キュー）** — アバターに適用する最終的な表現指示（`EneEvent::Performance`）。Desktop がブレンドシェイプやアニメーションに変換する

キューの調停は mind が所有します。`ene-vrm` は描画だけ行い、mind の型を取り込みません。

## テキスト中のトークン

`<|perf:expr=NAME|>` のような Performance マーカーがモデル出力に出ることがあります。ランタイムはこれを個別に生成される一時的な UI イベントではなく、Performance 経路の一部として扱います。

## さらに深く

- [感情と Performance](../../reference/runtime/emotions.md)
- [Avatar Performance ADR](../../reference/architecture/avatar-performance.md)
- [Desktop アプリ](../apps/desktop.md)
