# ene 実装ガイド

状態: **実装開始用**

本書は、ene を実装するときの **Stage 間の順序、依存関係、並列化条件、完了条件** を示す navigation document である。

製品挙動の正本は [`docs/requirements/`](../requirements/README.md)、内部設計の正本は [`docs/design/`](../design/README.md) であり、本書が新しい要件や設計上の意味を追加することはない。開発コマンドと ene 固有の実装規約は [`AGENTS.md`](../../AGENTS.md) に一度だけ置き、本書へ転載しない。

Requirements / Design と本書が矛盾した場合は Requirements / Design を優先する。Design が固定した契約を変える必要が生じた場合や、設計同士の矛盾を発見した場合は、その場の実装で意味を決めず、影響する実装だけを止めて Issue または Design 修正 PR として先に解決する。独立した別 stack は継続してよい。

## 1. 進め方

### 1.1 縦断 slice を優先する

実装順は crate 一覧の上から順ではなく、**ユーザーから観測できる一つの経路を端から端まで成立させる順序**を優先する。

例えばテキスト会話であれば、Client UI だけ、Store だけ、Provider adapter だけを長期間独立して完成させるのではなく、薄くてもよいので

`Client input → Host intake → Companion → Permission / Credential → Inference → response → persistence → Client presentation`

までを早い段階で通す。その後、同じ boundary を崩さずに Memory、Task、recovery 等を追加する。

crate、boundary、abstraction は、実際の slice に必要になった時点で追加する。全 crate の空実装を先に作らず、将来必要になりそうという理由だけで Manager、Coordinator、共通 Context、汎用 state machine 等を先行実装しない。

### 1.2 PR の分割

実装は原則として stacked PR で進める。一つの PR は一つの coherent change にし、行数ではなく **reviewer が一つの意図として理解でき、変更前後の contract を検証できるか**で粒度を決める。例えば「一つの primitive type とその test」「Host↔Client の一つの message flow」「presence transition の一つの race contract」は一つの PR になり、「Stage 全部」や無関係な rename と feature の混在は分割する。

- 依存する変更は前の PR の branch 上に積み、下から順に merge する。各 PR は、その base までが入った状態で単独に build / test できる。
- 互いに依存しない変更は同じ安定した base から別 stack として並列化する。ただし同じ public contract を複数 stack で同時に変更せず、並列化の前に共有 boundary を小さい prerequisite PR として固定する。
- Design 自体の変更が必要な場合は implementation PR と混ぜず、先に Design 修正 PR を置く。
- PR 本文には、何を成立させる PR か、対応する Requirements / Design artifact、意図的に含めていない範囲、実行した validation、後続 stack との関係を書く。`.old/` を参照した場合はその箇所と理由も記す。

## 2. 実装順

以下は dependency order であり、すべてを完全な直列で行うという意味ではない。第 3 節の並列化条件を満たす部分は同時に進めてよい。

### Stage 0 — repository / build foundation

新実装を置ける最小 repository foundation を作る。

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
- 後続 PR が共通して使う validation が `AGENTS.md` の Commands として定まっている。

### Stage 1 — 最小 foundation と共有 contract

依存: Stage 0。

縦断 slice が必要とする最小の leaf / contract を作る。

主な対象:

- `ene-primitive`
- `ene-config`
- `ene-api` の最小 wire-neutral contract
- Host / Client app の最小 entrypoint
- 必要になった範囲の error / outcome 規約

共有 error crate を先に作ることを目的にせず、error は Design の domain / technical error 分離に従って必要な owner に置く。domain crate を一覧どおり全部作らない。

完了の目安:

- Stage 2 の最初の PR が、ここで定めた primitive / config / api contract と entrypoint だけを前提に開始できる。

### Stage 2 — Setup とテキスト会話の最初の縦断 slice

依存: Stage 1。

最小 Setup から一対一のテキスト会話が実際に通る状態を作る。

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

完了の目安:

- 最小 Setup 後に一対一のテキスト会話が通り、Host / Client 再起動後も保存済み timeline が復元され、その経路の integration test が green である。

### Stage 3 — Experience Summary / Memory

依存: Stage 2 の Conversation History。

会話を根拠に Learning を追加する。

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

この段階では Global scope、Skill、Relationship、Companion State を完成させない。Privacy / Security の targeted deletion は Stage 6 で別経路として実装する。

完了の目安:

- 会話を根拠にした Companion scope Memory の形成・revision・retrieval が restart を跨いで動き、integration test が green である。

### Stage 4 — Task Agent と Workspace file 作業

依存: Stage 2。Stage 3 とは並列化できる（第 3 節）。

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

完了の目安:

- 一つの Workspace folder に対する file 作業の委任、進捗 / 結果報告、steering、best-effort Cancel が動き、path boundary と race の test が green である。

### Stage 5 — Client lifecycle / presence / recovery

依存: Stage 4。

Task が動いた後で、Client と Host の lifecycle を跨いでも意味が壊れないようにする。

主な範囲:

- Client を閉じても Host 上の Task は継続する
- 再接続後の progress / result 表示
- Host restart 後の途中 Task は自動再開せず、保存済み進捗・既知の外部作用を示して Owner の明示再開を待つ
- Running Companion の presence recovery
- stale Client / stale generation の拒否
- undelivered report

完了の目安:

- Client の切断 / 再接続と Host restart を跨いでも Task と presence の意味が保たれ、stale Client / generation の拒否と undelivered report が test で確認できる。

### Stage 6 — Targeted Deletion / Audit / usage safety

依存: Stage 3、Stage 5。

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

完了の目安:

- targeted deletion が derived data と in-flight result を含めて完了し、Audit、usage 記録、費用表示、Credential leakage test が揃っている。

### Stage 7 — 管理面 / Desktop Body / first acceptance integration

依存: Stage 6。Client / Body 基盤は先行して進められる（第 3 節）。

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

Stage 7 の完了が、[`acceptance.md`](../requirements/acceptance.md) に定義された**最初の milestone の完了判定**になる。これは製品全体の完成を意味しない。

## 3. 最初の milestone で並列化できる範囲

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

- Client は `ene-api` だけに依存し、Host domain crate を直接 import しない。
- API contract が未固定なら、まず小さい contract PR を作る。

### Test infrastructure

race fixture、fake Provider、temporary DB、IPC integration harness 等の test infrastructure は、それを利用する feature stack と並列に作ってよい。ただし test 用 abstraction が production architecture を決めないようにする。

### Security / leakage checks

secret leakage、log redaction、Debug capture 検査などは Stage 6 まで待たず、Credential / Inference が導入された時点から継続的に追加する。Stage 6 はそれらを first acceptance の範囲として閉じる段階である。

## 4. first acceptance 後の実装順

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

## 5. 完了の判定

各 PR で focused validation を行い、stack の安定点では workspace 全体を検証する。コマンドは `AGENTS.md` の Commands を使う。

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
- 各 Stage の「完了の目安」

両 OS が acceptance 条件になっている項目は、最終的に Windows / Linux の双方で同一手順を再現する。OS 固有実装を片方の結果だけで一般化しない。
