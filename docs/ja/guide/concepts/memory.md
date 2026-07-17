# 記憶

ene はセッションをまたぐ **長期記憶** を持てます。事実、エピソード、好み、コミットメントなどを SQLite に保存し、任意でベクトル検索します。

## 役割分担

| 層 | 責任 |
|----|------|
| **mind** | 想起の計画、重要度の判断、ターン後の書き込み、忘却ポリシー |
| **store** | テキスト（と任意の埋め込み）の永続化、フィルタ付き検索 |

永続化は `store.enabled` と必要なら `store.db_path`。想起・減衰のつまみは `mind.*` です。

## ハイブリッド想起（考え方）

埋め込み、字句一致、新しさ、顕著性などを組み合わせられます。mind がオーケストレーションし、store が実行します。

## さらに深く

- [長期記憶リファレンス](../../reference/memory/memory.md)
- [ene-store API](../../reference/api/ene-store.md)
- [ene-mind API](../../reference/api/ene-mind.md)
