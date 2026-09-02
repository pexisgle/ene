# character/ — キャラクターパッケージ

キャラの「配れる形」を定義する。実行時のコンパニオンは、ここで配る
soul/body に、コアデーモンが付ける裏層ハーネスを加えて1体になる
([../product/vision.md](../product/vision.md#54-パッケージとの関係))。

| 文書 | 責務 | 主な P 対応 |
|---|---|---|
| [package-format.md](package-format.md) | Ene 正規形式(soul/body/char)・マニフェスト・検証・インポート/エクスポート・配布仕様 | P-8xx |

## この層に効く主な決定

**v1.0 は署名を要求しない**(D-26)。検証はハッシュ照合と
「キャラクターはコードを実行しない」原則まで。
コミュニティ配布とマーケットは将来だが、そこへ大規模改修なしに
到達できるよう、マニフェストの構造・インストール先のレイアウト・
ライセンス宣言だけは v1.0 で固定する
([package-format.md §11](package-format.md#11-配布仕様p-806--d-26))。

## 他フォルダとの接点

- soul の実行時構造 → [../companion/soul-and-affect.md](../companion/soul-and-affect.md)
- body の実行時制御 → [../body/body-and-performance.md](../body/body-and-performance.md)
- skill の参照解決 → [../tasks/skills.md](../tasks/skills.md)
- 配布のセキュリティ → [../security/approval.md](../security/approval.md)

