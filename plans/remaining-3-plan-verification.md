# 検証: remaining-3-plan.md

対象 3件の網羅・優先度・根本原因・並列順序・検証観点を確認。
- 3件全列挙と scope/label 反映: OK
- #1250 文言修正の範囲と命名一致: OK
- #1251 projection/normalize は対応済み、app.rs の has_assistant 分岐が原因と特定: OK
- #1198 参照不整合を harness-redesign で代替、境界と段階移行を考慮: OK
- 並列順序 A/B小規模→先行、C工数大→並列開始で block しない: OK
判定: 実装移行可能。

