# セッション分割と圧縮

**Hard-split は製品経路ではない。** 文脈境界は rolling compression のみ（`mind.context.compression_*`）。`ene-runtime` は hard-split / session-ID 発行タスクを起動しない。

## 現行ポリシー

- **圧縮（必須の製品経路）**
  - 古いターンは `memory_spans` に圧縮される。
  - 連続性のため Session ID は安定したまま。
  - 手動 `/session split` は **コンテキスト圧縮**を起動する（同じ session id）。
- **Hard-split**
  - ホストからは使わない。スコアリング / `execute_split` は `ene-mind` にライブラリ実験用として残ってもよいが、`ene-runtime` からは配線しない。

## 圧縮を優先する理由

- 進行中のセッション ID を維持し、関係の連続性を保つ。
- コンパニオン対話での硬い境界を避ける。
- rolling summary でプロンプトサイズを抑える。

## 運用メモ

- 保留中の圧縮タスクは同時に一つだけ処理する。
- ホストの手動 split は手動圧縮の挙動にルーティングする。

## 関連ドキュメント

- `docs/ja/reference/architecture/api-v1.md`
- `docs/ja/reference/architecture/cognitive-runtime.md`
- `docs/ja/reference/runtime/session.md`
- `docs/ja/reference/configuration/settings.md`（`mind.context.compression_*`）
