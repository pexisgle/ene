# 感情とアバター Performance

モデルは気分を表現でき、Desktop はそれを VRM アバターに再生します。

## 2 つの層

1. **Affect** — エンジン側の感情状態。ターンをまたいで更新され、store 有効時は永続化される
2. **Performance キュー** — 最終提示ヒント（`EneEvent::Performance`）。Desktop がブレンドシェイプ / アニメにマップする

キューの調停は mind が所有します。`ene-vrm` は描画だけ行い、mind の型を取り込みません。

## テキスト中のトークン

`<|emo:name|>` のような感情マーカーがモデル出力に出ることがあります。ランタイムはこれをアドホックな UI イベントではなく、Performance 経路の一部として扱います。

## さらに深く

- [感情と Performance](../../reference/runtime/emotions.md)
- [Avatar Performance ADR](../../reference/architecture/avatar-performance.md)
- [Desktop アプリ](../apps/desktop.md)
