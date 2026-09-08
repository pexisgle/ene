# ene 実装ガイド

状態: **実装開始用**

この文書は、ene を実装するときの**進め方・順序・分割方法・検証方針**を示す navigation document である。

製品挙動の正本は [`docs/requirements/`](../requirements/README.md)、内部設計の正本は [`docs/design/`](../design/README.md) であり、この文書が新しい要件や設計上の意味を追加することはない。

実装中に Requirements / Design とこの文書が矛盾した場合は、Requirements / Design を優先する。下位設計同士、または上位設計との矛盾を発見した場合は、その場の実装で意味を決めず Issue / Design 修正として扱う。

## 1. 実装の基本方針

### 1.1 新規実装を基本とする

現在の実装は Requirements / Design を基準に新しく構築する。

- 既存コードの構造・命名・責務分割・挙動をそのまま引き継がない。
- 「以前そう実装されていた」という理由だけで、要件・設計・互換性制約を追加しない。
- 先に全 crate の空実装を作るのではなく、実際の縦断 slice に必要になった boundary から追加する。
- 将来必要になりそうという理由だけで abstraction、Manager、Coordinator、共通 Context、汎用 state machine 等を先行実装しない。
- Design が固定した semantic owner、authority、identity / revision / generation、保存分類、Host / Client 境界、secret 非返却等は実装都合で変更しない。

### 1.2 縦断 slice を優先する

実装順は crate 一覧の上から順ではなく、**ユーザーから観測できる一つの経路を端から端まで成立させる順序**を優先する。

例えばテキスト会話であれば、Client UI だけ、Store だけ、Provider adapter だけを長期間独立して完成させるのではなく、薄くてもよいので

`Client input → Host intake → Companion → Permission / Credential → Inference → response → persistence → Client presentation`

までを早い段階で通す。

その後、同じ boundary を崩さずに Memory、Task、recovery 等を追加する。

### 1.3 中間状態も壊さない

各実装段階・各 PR は、可能な限り単独で build / test 可能な状態にする。

- 一時的に authority boundary を迂回する実装を入れない。
- 後で必ず捨てる fake 経路を本番経路へ混ぜない。
- 未実装機能は明示的な unsupported / unavailable とし、成功したように見せない。
- stale、deny、hold、unknown 等の domain outcome を technical error や成功へ潰さない。
- 外部作用の成功が不明な場合、自動再実行しない。

## 2. `.old/` の扱い

`.old/` は**旧実装の参照資料**であり、新実装の source of truth ではない。

原則として参照せず、Requirements / Design と新実装だけで進める。

参照してよいのは、例えば次のように現在の文書だけでは確認しづらい事項があり、実装を進めるために実際に価値がある場合に限る。

- 過去に利用していた asset / file format の実例
- 外部 protocol / OS API / platform 固有処理の実装例
- 既存 data の形を調査する必要がある場合
- 過去に遭遇した platform 固有 bug や workaround の確認

`.old/` を参照した場合も、次を守る。

- code をそのまま copy しない。
- `.old/` の crate や module へ依存しない。
- 旧構造との互換性を目的に新設計を歪めない。
- 旧挙動を要件へ昇格させない。
- 採用した考え方が現在の Requirements / Design と一致することを確認する。
- 旧実装由来の処理には、現在の contract に基づいた test を付ける。
- PR 本文に `.old/` を参照した箇所と理由を簡潔に記録する。

「旧コードの方が実装しやすい」は参照理由にしない。

## 3. PR の基本運用

実装は、原則として **stacked PR** で進める。

一つの大きな milestone を一つの巨大 PR にせず、review 可能で、単独で意味があり、検証可能な変更へ分割する。

### 3.1 PR の粒度

一つの PR は、原則として一つの coherent change にする。

良い分割の例:

- primitive type とその test を追加する
- 一つの repository boundary と persistence 実装を追加する
- Host↔Client の一つの message flow を通す
- OpenAI Responses adapter の最小経路を追加する
- Memory formation の candidate → commit 経路を追加する
- Workspace file read/write の Action adapter を追加する
- presence transition の一つの race contract を実装する

避ける例:

- 「M2 全部」のように複数 subsystem を一度に完成させる
- 無関係な rename / cleanup / feature を同じ PR に混ぜる
- 大量の将来用 abstraction と実機能を同時に入れる
- review できない量の generated / mechanical change と domain behavior change を混ぜる

行数そのものではなく、**reviewer が一つの意図として理解でき、変更前後の contract を検証できるか**で粒度を決める。

### 3.2 stacked PR の作り方

依存する変更は、次の PR を前の PR の branch 上に積む。

```text
main
 └─ PR A: foundation
     └─ PR B: repository boundary
         └─ PR C: vertical flow
             └─ PR D: integration test
```

各 PR は、その base までが入った状態で green であることを基本とする。

- 下から順に merge する。
- 下位 PR の変更後は上位 stack を rebase / restack する。
- 後続 PR にしか存在しない code を前提に、下位 PR の test を壊したままにしない。
- stack が長くなりすぎた場合は、安定した境界で一度 main へ merge して新しい stack を始める。

### 3.3 並列作業

互いに依存しない変更は、無理に一つの stack にせず、同じ安定した base から別 stack として並列化する。

```text
main
 ├─ stack A: Host / persistence
 ├─ stack B: Client / Body
 └─ stack C: test infrastructure
```

ただし同じ public contract を同時に別 stack で変更しない。並列化する前に共有 boundary を小さい prerequisite PR として固定する。

典型例:

1. `ene-api` の DTO / contract を prerequisite PR で固定する。
2. Host 実装と Client 実装を別 stack で進める。
3. 両方が入った後に integration PR を作る。

### 3.4 PR 本文に最低限含めるもの

- 何を成立させる PR か
- 対応する Requirements / Design artifact
- 意図的に含めていない範囲
- 実行した validation
- 後続 stack がある場合はその関係
- `.old/` を参照した場合は、その箇所と理由

Design 自体の変更が必要な場合は、可能なら implementation PR と混ぜず、先に Design 修正 PR を置く。

## 4. 実装順

以下は dependency order であり、すべてを完全な直列で行うという意味ではない。後述の並列化条件を満たす部分は同時に進めてよい。

### Stage 0 — repository / build foundation

最初に、新実装を置ける最小 repository foundation を作る。

主な範囲:

- root Cargo workspace
- Rust toolchain / formatter / lint の基準
- 最小 CI
- Linux / Windows で build するための環境定義
- 新しい `crates/` / `apps/` の入口
- `.old/` を新 workspace の対象外にする

この段階では domain 機能を作らない。

完了の目安:

- 空または最小 crate の workspace が build / test / lint できる。
- `.old/` が build graph に入らない。
- 後続 PR が共通して使える validation が定まっている。

### Stage 1 — 最小 foundation と共有 contract

縦断 slice が必要とする最小の leaf / contract を作る。

主な対象:

- `ene-primitive`
- `ene-config`
- `ene-api` の最小 wire-neutral contract
- Host / Client app の最小 entrypoint
- 必要になった範囲の error / outcome 規約

注意:

- 共有 error crate を先に作ることを目的にしない。error は Design の domain / technical error 分離に従い、必要な owner に置く。
- domain crate を一覧どおり全部作らない。
- ID / revision / generation を一つの汎用 integer / UUID 型として意味共有しない。

### Stage 2 — Setup とテキスト会話の最初の縦断 slice

最初に、最小 Setup から一対一のテキスト会話が実際に通る状態を作る。

実装順の目安:

1. Host↔Client の接続と最小 handshake
2. Setup に必要な設定・Provider assignment・Credential 登録
3. Permission / Credential / Inference の最小 boundary
4. OpenAI Responses API の単一 Provider 経路
5. Host 発行 round と Client text input
6. response streaming と Client presentation
7. Conversation History の durable 保存
8. Host / Client 再起動後の保存済み timeline 復元
9. 同じ経路を通す integration test

この Stage では fallback、複数 Provider、Voice、Observation、Task 等を入れない。

重要な注意:

- Credential を登録しただけで Provider 利用を開始しない。
- Client message は authority ではない。
- round identity は Host owner が発行する。
- `ene-api` に Host 内部 row / secret / domain repository type を出さない。
- DB transaction を business / Client / Provider boundary に露出させない。

### Stage 3 — Experience Summary / Memory

Stage 2 の Conversation History を根拠に Learning を追加する。

実装順の目安:

1. Experience candidate と Summary 形成
2. Summary の provenance / evidence
3. Companion scope Memory の形成
4. Memory revision と更新履歴
5. retrieval / context assembly への供給
6. 会話による訂正・状況変化の区別
7. 重要度と想起優先度
8. 管理面へ出す read model
9. restart を跨ぐ integration test

この段階では Global scope、Skill、Relationship、Companion State を完成させない。

通常の忘却・訂正・失効・置換・統合で、保存済み Memory / revision / evidence を削除しない。Privacy / Security の targeted deletion は Stage 6 で別経路として実装する。

### Stage 4 — Task Agent と Workspace file 作業

一つの Workspace folder を対象に、Task Agent へ file 作業を委任できる最小経路を作る。

実装順の目安:

1. Task / TaskRevision / TaskContext
2. Workspace association
3. delegation / Task Agent lifecycle
4. Action boundary と current Permission check
5. Workspace 内 file list / read / create / edit
6. path boundary / traversal 拒否
7. progress / result reporting
8. steering
9. best-effort Cancel
10. unknown external effect / late result の扱い
11. integration / race test

最初の acceptance では Delete、shell、Network、MCP、Workspace 外操作を許可しない。

Task の成立、Permission の成立、Action の成功を一つの state にまとめない。

### Stage 5 — Client lifecycle / presence / recovery

Task が動いた後で、Client と Host の lifecycle を跨いでも意味が壊れないようにする。

主な範囲:

- Client を閉じても Host 上の Task は継続する
- 再接続後の progress / result 表示
- Host restart 後の途中 Task は自動再開しない
- 保存済み進捗・既知の外部作用を示し、Owner の明示再開を待つ
- Running Companion の presence recovery
- stale Client / stale generation の拒否
- undelivered report

注意:

- Task resume と presence recovery を同じ authority にしない。
- reconnect したという事実だけで旧 Client の state を current に戻さない。
- Stop と disconnect を区別する。
- unknown Action を restart 後に自動 retry しない。

### Stage 6 — Targeted Deletion / Audit / usage safety

最初の acceptance に必要な data safety と費用・秘密情報の経路を完成させる。

主な範囲:

- Privacy / Security 目的の targeted deletion
- deletion scope の durable 化
- participant ごとの局所消去
- derived data / cache / index の消去
- in-flight result による再形成防止
- 残存検証後だけ global completion
- Audit
- OpenAI usage の input / output / cached token 記録
- Provider reported / estimated / unknown の区別
- OpenAI 単一 Provider についての費用表示
- Credential leakage test
- crash で孤立した usage reservation の unknown 確定

通常の忘却と targeted deletion を同じ API / state transition にしない。

### Stage 7 — 管理面 / Desktop Body / first acceptance integration

ここまでの Host 機能を Owner が確認・操作できる管理面と、最初の Desktop Body を完成させる。

主な範囲:

- Setup / Provider / Credential / assignment 管理
- Conversation / Memory / Task / usage / failure の Text 管理面
- 基本 Desktop Body
- VRM overlay
- move / resize / hide
- idle / response 状態の表示
- Body failure 時の Text fallback
- Debug capture への secret 非露出
- acceptance scenario の end-to-end 実行
- Windows / Linux の性能 Gate と baseline

Stage 7 の完了が、[`acceptance.md`](../requirements/acceptance.md) に定義された**最初の milestone の完了判定**になる。

これは製品全体の完成を意味しない。

## 5. 最初の milestone で並列化できる範囲

### Stage 2 完了後

**Stage 3 Learning** と **Stage 4 Task / Action** は、共有 contract が安定していれば別 stack で並列に進められる。

ただし次を同時に変更する場合は、先に prerequisite PR を作る。

- `ene-store` の共通 persistence mechanism
- `ene-api` の共通 DTO
- Companion が Learning / Task へ渡す public premise
- usage / permission の共通 boundary

### Client / Body

Desktop Body の Client 側基盤は、Host の Stage 3〜6 と並行して進めてよい。

条件:

- Client は `ene-api` だけに依存する。
- Host domain crate を直接 import しない。
- Body state を Companion / Task / Learning の正本にしない。
- API contract が未固定なら、まず小さい contract PR を作る。

### Test infrastructure

race fixture、fake Provider、temporary DB、IPC integration harness 等の test infrastructure は、それを利用する feature stack と並列に作ってよい。

ただし test 用 abstraction が production architecture を決めないようにする。

### Security / leakage checks

secret leakage、log redaction、Debug capture 検査などは Stage 6 まで待たず、Credential / Inference が導入された時点から継続的に追加する。

Stage 6 はそれらを first acceptance の範囲として閉じる段階である。

## 6. first acceptance 後の実装順

後続機能も Requirements の一部である。以下は dependency を基準にした推奨順であり、依存が閉じていれば別 stack で並列化してよい。

| Stage | 機能 | 主な前提 |
|---|---|---|
| 8 | Schedule | Task / lifecycle / Permission / usage |
| 9 | Backup / Restore / Host auto-start | Preservation / management / Credential boundary |
| 10 | Voice | Presentation / Client / Permission |
| 11 | Observation / Observer | Task / presence / Presentation / Permission |
| 12 | Group conversation / spontaneity / Companion interaction | Conversation / Learning / presence |
| 13 | Global scope / Skill / Companion State / Relationship | Learning |
| 14 | Remote Client | presence / trusted management / backup-restore / device auth |
| 15 | Character editing / Package distribution | Character / Action / management |
| 16 | Multi-provider / fallback / full cost cap | Inference / Permission / usage / management |

後続 Stage の番号は依存関係を示すための navigation であり、Release 番号ではない。

特に次は並列化しやすい。

- Voice と Schedule
- Observation と Backup / Restore
- Character editing と Multi-provider
- Group conversation 系と Global / Skill / State / Relationship の一部

一方、Remote Client は trust boundary を広げるため、Host-local の管理・presence・Restore・device auth が固まる前に先行させない。

## 7. validation の考え方

Stage 0 で新 workspace の build 基準を作った後は、各 PR で focused validation を行い、stack の安定点では workspace 全体を検証する。

基本:

```bash
cargo fmt --all -- --check
cargo check -p <pkg>
cargo test -p <pkg>
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
```

repository の実際の構成に応じて Stage 0 で CI と正式コマンドを確定し、この文書の例より repository 側の現行設定を優先する。

### 各 PR で確認するもの

- 追加した contract の unit / integration test
- stale / retry / duplicate / crash が関係する場合は negative / race test
- secret が関係する場合は leak test
- persistence が関係する場合は restart test
- boundary を跨ぐ場合は public interface だけを使った integration test

### Stage 完了時に確認するもの

- 対応する Requirements acceptance scenario
- 関連する Design walkthrough / race walkthrough
- workspace test / lint
- 後続 Stage が依存してよい public contract が成立していること

両 OS が acceptance 条件になっている項目は、最終的に Windows / Linux の双方で同一手順を再現する。OS 固有実装を片方の結果だけで一般化しない。

## 8. 実装中の判断

Design が明示的に自由としている実装詳細は、合理的なものを選んでよい。

例:

- crate 内 module 粒度
- method / local type 名
- SQL column / index の具体形
- async / channel / lock 等の局所 mechanism
- retry / timeout の具体値
- cache / prompt assembly / scoring の具体方式
- test fixture の実装方法

一方、次を変える必要が生じた場合は実装で吸収しない。

- semantic owner
- authority の所在
- identity / revision / generation の意味
- caller ≠ authority
- compare-before-commit の契約
- secret の露出範囲
- durable / derived / external の分類
- Host-local / remote-capable の trust boundary
- Privacy / Security の削除意味
- Requirements にないユーザー可視挙動

その場合は、影響する実装だけを止め、Issue または Design 修正 PR として先に解決する。独立した別 stack は継続してよい。

## 9. 実装時に常に注意する事項

- 保存された Allow、古い Client state、Task Agent result、表示成功等を authority にしない。
- current 条件を必要とする更新は compare-before-commit で守る。
- `SecretValue` や Credential 平文を public API、wire、History、Learning、Task result、log、Debug capture に出さない。
- derived index / embedding / cache を durable 正本にしない。
- 通常の忘却を削除として実装しない。
- Task Cancel は best-effort であり、受付を停止完了にしない。
- unknown external effect を success / failure のどちらかへ推測で寄せない。
- Client は Host state の永続正本を持たない。
- Composition root に domain の意味判断を集めない。
- 一つの便利な共通 abstraction のために owner boundary を統合しない。
- performance optimization は contract を保った局所変更として行い、測定前の複雑化を避ける。

## 10. 参照先

実装前に必要な artifact だけを読み、同じ情報をこの文書へ転載しない。

- [Requirements](../requirements/README.md)
- [Acceptance](../requirements/acceptance.md)
- [Design](../design/README.md)
- [Crate / Module Decomposition](../design/concrete/crate-module-decomposition.md)
- [Interface Boundaries](../design/concrete/interface-boundaries.md)
- [Persistence / Recovery](../design/concrete/persistence-recovery.md)
- [Concurrency Control](../design/concrete/concurrency-control.md)
- [Correspondence / Identity](../design/concrete/correspondence-identity.md)
- [Host ↔ Client IPC](../design/concrete/host-client-ipc.md)

この Guide は実装の進行に合わせて更新してよい。ただし、Requirements / Design の意味変更をこの Guide の更新だけで済ませてはならない。
