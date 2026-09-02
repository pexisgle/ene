# product/ — プロダクト定義

「完成形」を定義する根幹文書群。全設計文書は vision / features / done から
逆引きされる。W7 記録は機構検出であり、完成宣言ではない。

| 文書 | 内容 |
|---|---|
| [vision.md](vision.md) | 位置づけ・利用者像・設計原則・参照系譜・**2層ランタイム**(表層 soul / 裏層ハーネス)・**用語集** |
| [features.md](features.md) | 機能目録。要件 ID(`P-1xx`〜`P-10xx`)と優先度(v1.0/stretch/後継) |
| [done.md](done.md) | 完成の段階的定義。v1.0 の観察可能な受入条件・後継マイルストーン・非目標。`[x]` は Echo / Scripted 以外または実プロセスで観察できる項目に限る |
| [w7-verification.md](w7-verification.md) | W7 の機構検出記録。経路(Echo / Scripted / host API / launcher)と完成宣言を列で分ける |

## 読み順

1. `vision.md` で用語集を押さえる(以降の全文書がこの用語を使う)。
2. `features.md` で機能 ID を確認する。
3. `done.md` で v1.0 の完了条件を確認する。`[x]` は観察可能な完成だけ。
4. `w7-verification.md` で機構検出の経路(Echo / Scripted / host API 等)を見る。
5. レビューで確定した決定は [../decisions.md](../decisions.md)。ここが矛盾したら勝つ。
6. 各設計フォルダの README から入る([../README.md](../README.md) に全体目次)。

## このフォルダの決定権

矛盾したときの優先順位は **[../decisions.md](../decisions.md) > この文書群 > 領域別文書**。
レビューで確定した決定とここが矛盾したら、decisions.md が勝つ。
設計文書とこの文書群が矛盾したら、**この文書群が勝つ**。

設計文書はこの文書群の `P-xxx` を実現する手段であり、
`P-xxx` にない機能を勝手に追加しない。追加が必要なら、まず
features.md に ID を足す(要レビュー)。

