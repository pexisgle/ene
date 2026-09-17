# Stage 6: 指定データの完全削除 (Targeted Deletion) と利用量・機密安全

[実装ガイド](../README.md) / [進捗](../PROGRESS.md)

この文書は Stage 6 の**実装順・並列化条件・完了 gate**だけを扱います。製品要件は `docs/requirements/`、内部設計は `docs/design/` を優先します。ここで新しい削除意味論や authority を定義してはいけません。

## 目的

Stage 4 で導入した erasure-currentness foundation を拡張し、次を完成させます。

- ユーザーが指定した対象データの system-wide Targeted Deletion
- deletion 中および遅延到着時の再出現防止
- participant ごとの局所削除と remainder verification
- 全域完了の判定と、本文を残さない audit
- provider token usage と cost の記録・表示
- API key などの credential が prompt / log / error へ漏れないことの強制と検証

## 設計の入口

- [Targeted Deletion](../../design/critical-areas/targeted-deletion.md)
- [Targeted Deletion Lifecycle / Participant Contract](../../design/concrete/targeted-deletion-lifecycle.md)
- [Concurrency Control](../../design/concrete/concurrency-control.md)
- [Persistence / Recovery](../../design/concrete/persistence-recovery.md)
- [Interface Boundaries](../../design/concrete/interface-boundaries.md)
- [Correspondence Identity](../../design/concrete/correspondence-identity.md)
- [Crate / Module Decomposition](../../design/concrete/crate-module-decomposition.md)
- [requirements](../../requirements/requirements.md) / [acceptance](../../requirements/acceptance.md)

## 既存 foundation

Stage 6 は Stage 4 の foundation を置き換えません。現在の canonical current-condition store、`ErasureConditionRef`、`data_use` correlation、AU14 inference-attempt claim の coverage check、`DataUseHeld` をそのまま利用・拡張します。

別の deletion gate、currentness registry、source-correlation 系統を新設しません。active condition が 0 件の場合も、canonical store の authoritative な空集合として扱います。

## 実装前の確認

最初の実装 PR の前に、以下だけを gap review します。

1. `ene-preservation` の公開型・trait と設計文書の対応。
2. `ene-store` に既にある erasure condition / inference `data_use` persistence の範囲。
3. deletion operation / participant / verification / finalization のうち production producer がまだ存在しない境界。
4. History、Learning、Task/Action、undelivered、cache、Client transient data など、design が要求する participant の列挙。
5. usage fact と credential scrub boundary の既存実装範囲。

設計上の曖昧さが見つかった場合はコードで推測せず、docs-only PR / Issue を先に出します。

## Workstream A: Targeted Deletion 本体

この workstream は順序依存です。

| Slice | 実装範囲 | Gate |
|---|---|---|
| A1 | deletion operation の production admission と durable lifecycle。operation と current erasure condition を actual deletion より先に永続化する | durable-before-enforce が成立し、restart 後も active operation / condition を復元できる。condition publication 前に participant の actual deletion を始めない |
| A2 | participant の列挙・登録と Host composition からの fan-out。`ErasureParticipant` のような横断 trait は preservation 側に置き、各 domain が実装する | `ene-preservation` が各 participant の具象 crate に逆依存しない。必須 participant の欠落を全域完了として扱わない |
| A3 | participant ごとの bounded sweep と actual erasure。History / Learning / Task・Action facts / undelivered / cache / Host・Client transient data など、各 semantic owner が自分の範囲だけを削除する | 他 domain を直接書き換えず、対象本文・派生データ・index/search data を participant 単位で消去できる |
| A4 | in-flight / delayed-arrival collection。既に開始済みの inference / Task / Action / reconnect から戻るデータを operation + sweep と照合し、再保存・再採用・再表示を防ぐ | deletion interval 内に生成・到着した対象データが新しい Experience / Memory 等として復活しない。未開始 use は hold、already-started use は相関を失わない |
| A5 | remainder verification、participant completion、finalizing、全域完了、audit | 全 participant の local completion と残存確認が揃うまで global complete にしない。audit には対象本文を残さない。restart を跨いでも unfinished operation を失わない |

A3 は participant ごとに複数 PR へ分割して構いません。巨大な全域 DELETE transaction や万能 deletion manager は作りません。

## Workstream B: Usage / Cost

Deletion 本体と独立した変更は並列化できます。

| Slice | 実装範囲 | Gate |
|---|---|---|
| B1 | 既存 `UsageRepository` / usage facts が dialogue・learning・Task Agent など全 provider send を漏れなく記録しているか確認し、不足 producer を接続する | input / output / cached token usage が durable fact として source に対応付く。provider call が成功・失敗した境界で二重計上しない |
| B2 | model / provider pricing と usage fact から cost を導出する query / projection を追加する | usage fact 自体を書き換えず、価格情報と usage の対応時点を明確にして再計算可能にする |
| B3 | first-party の bounded read / display 経路を追加する | Stage 7 の GUI を待たず、Stage 6 の acceptance で usage / cost を確認できる。表示のための第二の authoritative usage store を作らない |

## Workstream C: Credential safety

Deletion 本体と schema / API が衝突しない範囲で並列化できます。

| Slice | 実装範囲 | Gate |
|---|---|---|
| C1 | [#1530](https://github.com/pexisgle/ene/issues/1530) の `ScrubbedText` construction boundary を設計どおり seal し、scrubber を通らない proof の mint を禁止する | inference consumer が public field literal 等で scrub proof を偽造できない |
| C2 | dialogue / learning / Task Agent / report / error / logging の no-secret regression tests を追加する | API key 等が provider prompt、History、presentation、structured error、通常 log に現れない |
| C3 | credential revision currentness と scrubbed payload の既存 claim を Stage 6 の E2E で再確認する | stale credential premise や scrub failure を fail closed とし、秘密本文を diagnostic に含めない |

## 統合 Slice D

A1〜A5、B1〜B3、C1〜C3 が揃った後に Stage 6 の縦断 E2E を追加します。

最低限、次を通します。

- exact mechanical target を含む History / Summary / Memory / search data の削除
- deletion 中に発生した delayed inference result / learning output の再出現防止
- Host restart / Client reconnect を跨ぐ unfinished deletion の継続
- participant failure / unreachable range がある場合に global complete としないこと
- final verification 後に対象本文が残っていないこと
- audit / completion report 自体が対象本文を複製しないこと
- usage token / cost の記録と first-party 表示
- credential no-leak regression

## 並列化

- A1 は deletion workstream の基盤なので先行します。
- A2 後、互いに独立した participant の A3 実装は並列化できます。
- A4 は各 producer / receiver の correlation が必要なので、対象 participant の A3 と合わせて進めても構いません。
- B と C は A と独立して進められますが、共通 schema / public interface を変更する場合は小さな prerequisite PR を先に置きます。
- D は全 workstream の integration gate なので最後です。

## 完了条件

- 指定データが派生データを含めて system-wide に消去され、remainder verification を通る。
- deletion interval 中の遅延到着・再接続・再起動から対象データが復活しない。
- 全 participant の完了を確認するまで global completion を宣言しない。
- token usage / cost が durable facts から正しく記録・表示される。
- credential が prompt / log / error / History / presentation に漏洩しない。
- Stage 6 acceptance と Linux / Windows CI を通す。
