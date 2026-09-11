# Subsystem間 Interface Boundary / Command・Query・Result Contract — Step 13 Concrete Design

本書は Step 13 の Interface Boundary artifact である。[対応関係・識別](correspondence-identity.md)（CI）、[Persistence / Recovery](persistence-recovery.md)（PR）、[Concurrency Control](concurrency-control.md)（CCT）が定めた identity・保存分類・atomicity / recovery・concurrency の契約を前提とし、変更しない。上位設計との優先順位と矛盾時の扱いは [設計文書 README](../README.md#正本と優先順位) に従う。本書内の SO / DR / CC は [State Ownership](../architecture/state-ownership.md) / [Dependency Rules](../architecture/dependency-rules.md) / [Cross-cutting Design](../architecture/cross-cutting.md) を指す。

本書の Rust pseudo-type / pseudo-trait はコンパイル対象ではない。型名・field 名の同義改名は許すが、型の分離と field の意味は維持すること。crate 分割は [Crate / Module 分解](crate-module-decomposition.md)、IPC wire schema は [Host↔Client IPC](host-client-ipc.md) が定める。

## 1. 対象と非対象

### 1.1 本書が具体化するもの

- どの semantic boundary を明示的な interface として表現すべきかの導出（第3節）。
- 各主要 interface の request / command 開始責務、authoritative 判断 owner、必要な identity / revision / generation、provenance / purpose / scope、expected current condition、Permission / consent / cap 参照、cancellation / hold 関係、result certainty、stale / rejected / needs-revalidation 等の結果、persistence / commit との関係（第4–9節）。
- Command / Query / Candidate / Decision / Fact / Observation / Result / Notification の使い分け（第2・4–9節）。
- boundary token の具体的な渡し方（第10節）。巨大な同一 context struct を渡さない。
- domain-specific acceptance 結果と technical error の分離（第11節）。共通巨大 Status enum を作らない。
- 長時間処理の request / completion 分離と durable correlation（第12節）。
- repository の compare-and-commit interface（第13節）。DB transaction を business layer へ露出させない。
- error model の配置（第11節）。
- IPC readiness：process / network を越える可能性がある interface の明示（第15節）。
- crate / module decomposition への材料：依存方向（第16節）。
- walkthrough による検証（第17節）。

### 1.2 本書が決めないもの

- プロジェクト全体の crate 構成、module 分割、process / thread 配置。
- DB 製品の選定、完全な `CREATE TABLE`、index、migration。
- IPC 方式、wire format の byte 仕様、network protocol、heartbeat / lease の有無・値。
- queue / retry / timeout の値、scheduling algorithm、embedding / retrieval scoring、prompt 組立 algorithm。
- 各 Subsystem の完全な関数一覧。内部 helper の列挙はしない。
- 暗号方式、archive 形式、署名方式。

### 1.3 用語

- **command**：意味状態の変更を owner へ要求する。受理・確定は owner が行う。呼出し自体は authority ではない。
- **query**：現在の意味・利用可否・参照範囲の取得。取得成功は後続の利用許可ではない。
- **candidate**：採用可否の判断材料としての提案（Action 候補、Learning 形成候補、fallback 候補、観測候補等）。提案の存在は採否ではない。
- **decision**：owner が確定した判断（許可、scope 決定、routing 採否、全域完了等）。生きた許可の再利用可否は別契約に従う。
- **fact**：把握された作用・利用量・試行・遷移等の記録。確定度（確認済み成功 / 確認済み失敗 / 不明）を伴う。
- **observation**：Client 主張、到達性、提示状況等の観測事実。authority ではない。
- **result**：長時間処理の到着物。元の試行・区間・世代への帰属を伴う。現在への自動採用ではない。
- **notification**：event delivery 等の一方向の伝達。受信は理解・採否を意味しない。
- **boundary token**：CI §4.5 の期待する対応の写し。authority ではなく比較材料。不一致なら hold・deny・不足・再評価へ戻す。欠落は「制約なし」ではなく不受理の理由。

## 2. Interface 設計原則（採用した原則）

1. **要求は提案、確定は owner。** すべての command / candidate は「採用してくれ」という提案であり、authoritative 判断は SO の owner が行う。interface 名に `execute` / `apply` / `update` とあっても、実装は premise 照合＋owner 確定を経る。呼び出せたこと・構築できたことを authority にしない。
2. **前提は typed に明示し、暗黙の最新を使わない。** 各 interface は必要な `expected_*` を明示 field として受け取る。revision / generation は生の数値で単独に持ち歩かず、必ず `(identity, revision)` / `(lifecycle, generation)` の組で扱う（CI §4.2–4.3）。token 欠落は不受理の理由。
3. **意味の異なる interaction を一つの万能型へまとめない。** Universal Command / Event / Result envelope、generic workflow interface、generic repository、generic policy interface、service locator、generic message bus abstraction を導入しない。transport representation を後で共有できても semantic type は domain ごとに区別する。
4. **短い commit、長い work は外。** 長時間処理（推論、Task Agent、外部 Tool、Computer Use、Backup / Restore、Targeted Deletion 参加作業）は premise 付きで開始し、lock / transaction なしで実行し、commit 直前に短い原子区間で current と比較する（CCT 第5節）。DB transaction 内で await・外部 I/O を行わない。
5. **巨大な同一 context struct を渡さない。** 各 boundary で必要な前提だけを typed に表現する（第10節の field matrix）。共通化するのは性質（opaque 性、単調性、有向性、比較の形）だけであり、型・lifecycle・counter は共有しない（CI §2.2）。
6. **結果は domain ごとに区別し、technical と semantic を分離する。** `Result<T, E>` だけに潰さず、`Result<DomainAcceptance, TechnicalError>` の形で返す（第11節）。stale / denied / held / superseded / insufficient / unknown-external / historical-only 等は `Ok` 側の domain outcome であり、`Err` 側の technical failure（DB unavailable、Provider transport error 等）と混同しない。共通巨大 Status enum を導入しない。
7. **長時間処理は durable correlation で戻れる。** request object を process memory だけに保持しない。Ticket / attempt / operation / staging の durable 対応から identity / revision / generation / attempt / provenance へ戻れる（第12節）。
8. **秘密を通常値として返さない。** Credential subsystem は認証用途への利用を供給しても、caller へ secret ownership を渡さない（DR-05）。non-secret reference と秘密値本体を別の意味として扱う。
9. **trait は boundary 隔離・差替え・testability の理由がある場合だけ。** 内部 pure function まで trait 化しない（第14節）。

## 3. Boundary discovery — どの boundary に明示的 interface が必要か

### 3.1 導出方法

Step 12 の H-1〜H-10（個体・作業・学習）、K-1〜K-12（権限・実行クラスタ）、X-1〜X-10（接続・提示・観測）、CH/CD（Character）、DP/PE（保全・消去）と、CI 第3節の8領域の対応関係、PR 第4節の table group、CC-01〜07、CCT 第4節の serialization domain を材料に、次の基準で判定した。

明示的 interface が必要な条件（いずれかに該当すれば定義する）：

- (a) semantic owner を跨ぐ意味の受渡しがある（要求開始責務と authoritative 判断責務が異なる）。
- (b) 現在条件の照合が必要（Permission / consent / cap / 帰属 / 消去 / 復元 / steering 前提のいずれかが受入可否を決める）。
- (c) lifecycle / revision / generation を跨ぐ対応が必要（遅延結果の帰属、steering 前後の区別、復元前後の区別、消去区間の区別）。
- (d) durable commit / atomic compare が必要（PR §7 の AU、 CCT SD に対応）。
- (e) trust boundary を跨ぐ（Client / Provider / MCP / Plugin / 外部 file、秘密の分離）。

内部 helper に留め、architecture interface へ昇格させないもの（例）：

- prompt 組立、context 圧縮・選択 algorithm、embedding / retrieval scoring、diff / merge algorithm、scheduling algorithm、費用推定式、集計表示の整形、描画・音声の staging、candidate 検知の内部 scoring、探索 query の組立。いずれも owner の判断を支える内部手段であり、ownership boundary ではない。
- 単なる表示集計、由来説明文の整形、次回時刻の導出計算。derived の生成手段であり、正本の受渡しではない。

Subsystem = crate / process / actor と仮定しない。同一 process / 同一 crate 内部でも ownership boundary を守るために interface を定義する（例：Task–Learning 間の自動昇格禁止、Permission 評価と Action 実行の分離）。逆に同一 owner 内の手順分割は interface にしない。

### 3.2 Interface inventory（明示的 interface の一覧）

ID は本書内の参照用であり、製品要件 ID ではない。`種類`は主たる interaction の種類。`開始`は request / command を開始する責務、`owner`は authoritative に判断する責務。詳細は第4–9・13節。

| ID | boundary | 種類 | 開始 | owner | 対応する Step 12 契約 |
|---|---|---|---|---|---|
| H-A | Task 化・委任・steering・Cancel・結果統合 | command + fact | 個体調整 | 作業 | H-3、W-1〜W-3、AE §4・§7、CC-01/03/04/07 |
| H-B | Experience 提出 → 形成判断 | candidate + decision | 個体調整・作業 | 認識・学習 | H-1、L-1〜L-2、CA |
| H-C | 会話による訂正 | command (訂正要求) + decision | 個体調整 | 認識・学習 | H-2、L-3〜L-6 |
| H-D | Learning 参照・由来説明 | query + fact | 個体調整・作業・共有観測（範囲限定） | 認識・学習 | H-4、L-8、CC-02 |
| H-E | scope 変更（Companion/Global） | candidate + decision | 認識・学習（意味判断） | 認識・学習（意味）＋権限・制約と各利用箇所（強制） | H-5、L-7、CC-02 |
| H-F | Observer 向け限定 routing 文脈の供給 | fact (派生表現) | 個体調整との協働（仲介） | 各元 owner（History・個体文脈=個体調整、Memory・Learning=認識・学習、Task context=作業） | H-6、X-5、OB-4、CC-02 |
| H-G | 未伝達の登録・報告状況更新 | fact + observation | 個体調整（管理）＋入出力・提示（提示事実） | 個体調整（必要性・状況） | H-7、X-9、SO 4.4 |
| H-H | Schedule 設定・到来・各回対応 | command + fact | Owner（管理経路）→作業 | 作業 | H-8、W-4 |
| K-A | Owner 意図 → 制御変更（Rule・同意・cap 等） | command + decision | 入出力・提示＋個体調整・作業（意図供給） | 権限・制約 | K-1、C-1、CC-01 |
| K-B | 現在の利用可否の照合（live authorization check） | query + decision | 利用・実行箇所（推論・実行・拡張・各参照保存箇所） | 権限・制約 | K-2、C-7、AE §5、CC-01 |
| K-C | 秘密利用（認証用途への供給） | command (利用要求) + fact | 推論・実行・拡張・接続・存在 | 認証秘密 | K-4、S-2〜S-4、CC-02 |
| K-D | Provider 割当解決 | query + candidate | 推論（解決） | 権限・制約（同意の意味） | K-5 前半、I-1、SO 4.18 |
| K-E | 推論の実利用ごとの成立・送信 | command + fact/result | 利用元（個体調整・作業・認識・学習・共有観測・入出力・提示） | 推論（経路・能力照合）＋権限・制約（可否）＋各送信箇所（適用） | K-5、I-4、RF §3、CC-02 |
| K-F | fallback 選択 | candidate + decision | 推論（候補） | 権限・制約（承認済み順序・現在条件） | K-6、I-5、CC-02 |
| K-G | 利用量の予約・確定・解放 | command + fact | 各利用 owner（推論・作業・実行・拡張・保全・消去） | 権限・制約（可否）＋各利用 owner（原記録） | K-7、C-4、CC-06、CCT 第9節 |
| K-H | Action 候補 → 認可 → 実作用 → 結果確定 | candidate + decision + command + fact | 利用元（候補）→権限・制約（認可）→実行・拡張（作用・確定度） | 権限・制約（認可）・実行・拡張（作用・確定度）・作業（Task 達成）で分離 | K-8、E-1〜E-2、AE §3・§5・§6、CC-01/07 |
| K-I | 拡張受入（MCP / Plugin / MCP Apps / sandbox 例外） | command + decision + fact | 機能利用元 | 実行・拡張（受入・制限）＋権限・制約（例外の意味） | K-9、E-4〜E-6、C-5 |
| K-J | Client 依存作用の限定・移動協調 | command + fact | 作業・個体調整（要求） | 実行・拡張（作用）＋接続・存在（帰属）＋権限・制約（device・許可） | K-10、E-3、X-6、CC-04 |
| K-K | 失効・停止・保留の伝播、遅延到着・不明の扱い | notification + fact | 権限・制約・個体調整・保全・消去（成立） | 各開始箇所（新規禁止の適用）＋各 owner（事実の帰属） | K-11、C-8、AE §7–§8、CC-03/04 |
| X-A | presence 成立・移動・復帰・復旧 | command + decision + fact | 個体調整（移動意図） | 接続・存在（帰属成立） | X-1、CN-2〜CN-4、CC-04 |
| X-B | Text / Voice round の帰属・区切り・提示 | candidate + fact + observation | 入出力・提示（受付・提示事実） | 個体調整（会話の意味）＋入出力・提示（round の実際）＋接続・存在（帰属） | X-2、IO-1〜IO-2・IO-6、CC-07 |
| X-C | Body 表示の帰属 | query (帰属参照) + fact | 入出力・提示 | 接続・存在（帰属）・個体調整（活動状態）・認識・学習（内的状態の意味）・Character（資材内容） | X-3、IO-3 |
| X-D | Observation eligibility の連動 | notification + decision | 接続・存在・入出力・提示（変化通知） | 共有観測（対象・時機） | X-4、OB-1、CC-04 |
| X-E | Observation result routing（三段階分離） | candidate + decision + notification + fact | 共有観測（候補・routing） | 共有観測（routing 採否）・個体調整（理解・発話・Action）で分離 | X-5、OB-2〜OB-6、CC-02/03 |
| X-F | 移動・切断・再接続時の三者協調 | notification + fact | 接続・存在・入出力・提示・実行・拡張 | 接続・存在（切替）・入出力・提示（区切り）・実行・拡張（作用不明）・作業（Host 継続）・個体調整（未伝達） | X-6、CN-6、CC-03/04 |
| X-G | Host restart 時の協調 | fact (保全された正本からの再構成) | 保全・消去（保全） | 接続・存在・入出力・提示・共有観測・作業・個体調整（各用途） | X-7、CC-04 |
| X-H | 未伝達の次 Client 報告 | query + fact | 個体調整（要約報告材料） | 個体調整（必要性）＋入出力・提示（提示事実） | X-9、H-G |
| C-A | Character static revision の取得 | query + fact | 個体調整（適用判断のため） | Character（静的内容） | CH-1〜CH-4、CD（供給） |
| C-B | 既存 Companion への revision 適用 | candidate + decision + fact | 個体調整（Owner 選択付き提案） | 個体調整（適用関係）・Character（内容供給）で分離 | CH-7、H-5 の適用側、CC-02 |
| C-C | Skill import・package validation | command + fact | Owner（管理経路） | Character（受入・validation）・認識・学習（内部 Skill としての意味）で分離 | CH-5、SO 4.7 |
| C-D | Character export | command + fact | Owner（管理経路） | Character（静的範囲の選択） | CH-6、CC-02 |
| D-A | Targeted Deletion の開始・範囲確定 | command + decision | Owner（管理経路）→保全・消去（調整） | 保全・消去（範囲・影響・除外の対応付け） | DP-1 (PE-1)、TD |
| D-B | 参加者への局所消去要求・局所完了返却 | command + fact | 保全・消去 | 各参加 owner（局所処理・検証） | DP（参加）、PE-1、TD |
| D-C | 残存検証・全域完了確定 | query + decision | 保全・消去 | 保全・消去（集約・検証） | DP、PE-1・PE-7、TD |
| D-D | Backup 作成・Restore staging・正本 switch・復元後保留・一括有効化 | command + candidate + decision + fact | Owner（管理経路）→保全・消去 | 保全・消去（調整）＋各 owner（参加・保留適用） | DP-2〜DP-4 (PE-2〜PE-4)、BR |
| D-E | 通常保持・retention / cleanup・Reset | command + decision | Owner（管理経路） | 保全・消去（調整）＋各 owner（局所） | DP (PE-4〜PE-5)、CC-05 |
| R-* | Repository compare-and-commit 群（第13節） | command (compare 付き) + fact | 各 business owner | 各 persistence group の担当 owner（意味変更は business owner、保持・照合可能性は persistence） | PR §7・§11、CCT 第5節、CI §6 |

呼び出せること自体を authority にしない。例えば `H-A` の Task 化要求が届いたことは Task 受理ではなく、`K-H` の Action 候補が構築できたことは送信・作用許可ではなく、`K-C` の credential reference を持っていることは秘密利用可能ではなく、`X-B` の Client message 受信は current presence ではなく、`H-A` の Task Agent result 到着は current Task への採用可能ではない。必要な current-condition check は各 owner の boundary で行う（第4–9節の `expected_*` と第11節の outcome）。

## 4. Task / Learning / Companion クラスタの interface semantics（H-A〜H-H）

### H-A Task 化・委任・steering・Cancel・結果統合（個体調整 → 作業）

Task と Learning を一つの generic workflow interface へ統合しない。Task は追跡される作業単位、Learning は認識・学習の意味判断であり、別の owner・別の revision・別の lifecycle である。

```rust
// --- identity / revision 前提（CI §5.3 の再利用。改名は許す） ---
struct TaskId(/* opaque */);
struct TaskRevision(u64);
struct TaskRef { task: TaskId, revision: TaskRevision }

struct DelegationId(/* opaque */);
struct TaskAgentEphemeralId(/* opaque */);
struct WorkspaceAssocId(/* opaque */);

// Task 化・委任の要求。個体調整が開始し、作業が受理・反映・達成を確定する。
struct ProposeTaskCommand {
    requester: CompanionId,          // 委任元 Companion（個体調整の同一性）
    purpose: TaskPurposeRef,         // 目的。将来 Rule そのものは目的にしない
    steering_premise: SteeringPremiseRef, // 目的・steering 前提。旧目的との区別用
    workspace_need: WorkspaceNeedRef,     // Workspace 利用条件（関連付けは作業が確定）
    client_binding: Option<ClientBindingPremise>, // Client 依存条件（あれば）
    provenance: ExperienceProvenanceRef,  // 会話・自発・Schedule 各回等の由来
}

struct ProposeSteeringCommand {
    task: TaskRef,                   // expected revision（boundary token）
    new_purpose: TaskPurposeRef,     // 新目的。旧目的との区別を保つ
    adopted_instruction: AdoptedInstructionRef, // 採用した追加指示（発言 record と区別）
    unadopted: Vec<UnadoptedReasonRef>, // 未反映・待機の対応（あれば）
}

struct CancelTaskCommand {
    task: TaskId,                    // Cancel 対象。revision は問わない（現在への記録のため）
    reason: CancelReasonRef,
}

// 作業の確定。会話上の受付と Task 反映を区別する。
enum TaskProposalOutcome {
    AcceptedAsTask(TaskRef),         // 新 Task として受理
    AcceptedAsSteering(TaskRef),     // 新 revision として受理（旧 revision を残す）
    CancelAccepted,                  // Cancel を現在 Task に記録（停止完了ではない）
    StalePremise { current: TaskRef }, // expected revision 不一致。再評価へ戻す
    HeldByGlobalHold(HoldConditionRef), // 消去・復元保留・停止等で新規禁止
    NeedsRevalidation(NeedsRevalidationRef), // 権限・帰属・cap 等の再照合が必要
    InsufficientContext(InsufficientRef),    // 目的・Workspace 条件が不足
}

// Task Agent への委任作成。作業が確定し、Agent は一時主体に留まる。
struct CreateDelegationCommand {
    task: TaskRef,                   // expected revision（boundary token）
    scope_copy: DelegationScopeRef,  // Task・Workspace 境界の写し（独立権限にしない）
    consumer_assignment: AssignmentRef, // 推論・費用の consumer 対応（cap 合算用）
}

enum DelegationOutcome {
    Delegated(DelegationRef),
    StaleTaskRevision { current: TaskRef },
    HeldByGlobalHold(HoldConditionRef),
    NeedsRevalidation(NeedsRevalidationRef),
}

// 長時間処理の分離：委任作成（request）と Agent 結果到着（completion）は別 interface。
struct TaskAgentResultArrival {
    delegation: DelegationRef,       // 委任対応（Task revision 前提を含む）
    attempt_refs: Vec<ActionAttemptId>, // 作用との対応（あれば）
    result_body_ref: TaskResultBodyRef, // 結果内容への参照（本文複製を要求しない）
    certainty: AgentResultCertainty, // Agent 側の申告（作用証拠にしない）
}

enum TaskResultAcceptance {
    AdoptedToCurrent(TaskRef),       // 現在 revision・目的と対応。達成判断は作業
    RecordedToOriginalOnly,          // steering / Cancel 後の旧結果。元 revision へ記録し現在不採用
    HeldForPermissionReview,         // 権限・消去・復元条件の再照合が必要
    DiscardedAsStaleWithRecord,      // 旧目的・旧 revision のため現在不採用（元への記録は残す）
}
```

- request 開始責務：個体調整（Task 化・steering・Cancel の意図）。authoritative 判断：作業（受理・反映・達成）。Permission 判断は権限・制約、作用成功は実行・拡張に残る。
- 入力として必要なもの：`TaskRef`（expected revision）、steering 前提、委任 scope 写し、Workspace 利用条件、Client 依存条件、provenance。軽微処理の Task 化省略は本体が行える範囲だが、閾値・分類 algorithm は固定しない（H-9）。
- provenance / purpose / scope：目的・由来・委任範囲・Workspace 範囲を欠落させない。発言 record と Task 反映内容と未反映・待機を区別する。
- expected current condition：現在の Task revision・委任有効性・Workspace 有効性・steering 前提。Client 依存なら presence generation＋現接続・可用性。
- cancellation / hold：Cancel 受付と遂行停止・外部作用の停止完了を分ける。Cancel 後の遅延結果は `RecordedToOriginalOnly` に留める。
- result certainty：Agent 申告を作用証拠にしない。作用確定度は実行・拡張の fact を参照・集約し、作業側で独立更新しない。
- persistence / commit：Task 作成は `task + task_revision + 初期 context + workspace_assoc` の原子 durable 後に可視化（durable-before-visible）。steering は新 revision＋新 context の原子 forward。同一 `TaskId` の steering・委任受付・完了確定・結果採用は SD-Task で逐次化（CCT §4）。
- IPC：Host-local。Client へは進捗・結果の必要表示だけを渡し、Task 正本は渡さない。

### H-B Experience 提出 → 形成判断（個体調整・作業 → 認識・学習）

```rust
struct ProposeExperienceCandidate {
    experiencer: CompanionId,        // 経験した個体
    source_range: SourceRangeRef,    // 元 Conversation・Task 等の大まかな範囲
    source_kind: ExperienceSourceKind, // 対話 / Task / Tool / Observation / 交流の区別
    task_ref: Option<TaskRef>,       // Task 由来なら revision 前提
    delegation: Option<DelegationId>,
    intended_use: IntendedUse,       // 返答 / Learning候補 / routing / Permission解釈 / Task判断の別
    // Raw 本文・詳細 payload の複製を要求しない。参照で辿れること。
}

enum FormationDecision {
    Formed { summary: SummaryId },   // Summary evidence を保存し、少なくとも1件の変更を適用した
    NoChangesApplied,                // compare-before-commit 敗北・対象欠落・scope 不一致・既存・revision 枯渇など。何も保存していない
    DeferredForContext,              // 文脈不足で保留（再提出は新 Experience 扱いにしない）
    DeclinedAsNoEndValue,            // 保存価値なし（全件保存を要求しない）
}
```

- Client・round・presence generation の correspondence は current stage の Experience formation が消費しないため interface に含めない。必要 stage で再導入する。
- 消去区間・保存禁止・非共有による保留（`HeldByErasureOrConstraint`）は current stage の formation では発生させず、deletion / constraint を扱う stage で再導入する。
- 開始：個体調整・作業。判断：認識・学習（保存価値・形成・更新・統合・想起必要性）。Task 限り情報の Learning 化は別判断。
- 失ってはならないもの：由来の区別、対象 Companion・Task・委任との関係、期待する利用先。
- Learning と Task を統合しない。応答完了と全 Learning 更新完了を同一条件にしない。

### H-C 会話による訂正（個体調整 → 認識・学習）

```rust
struct ProposeCorrectionCommand {
    target: LearningId,              // 訂正対象
    expected_revision: LearningRevision, // boundary token
    new_experience: ProposeExperienceCandidate, // 訂正根拠の新 Experience
    temporal_kind: TemporalKind,     // InitiallyWrong vs ChangedSince（誤訂正と時間変化の区別）
}

enum CorrectionOutcome {
    CorrectedAs(LearningRevision),   // 新 revision（過去有効性を保つ関係付き）
    StaleTarget { current_revision: LearningRevision },
    HeldByErasureOrConstraint(HoldConditionRef),
}
```

- 個体調整が会話で訂正を受け取ることは学習状態の直接上書きではない。対象を定めた意味変更は認識・学習。過去発言を現在認識へ書き換えない。

### H-D Learning 参照・由来説明（query + fact）

```rust
struct LearningQuery {
    requester: LearningConsumer,     // 個体調整 / 作業 / 共有観測（routing 範囲限定）等の consumer
    purpose: LearningUsePurpose,     // 返答 / Task判断 / routing / 由来説明の別
    scope_need: ScopeNeedRef,        // 必要な scope（Companion / Global の要求範囲）
    current_constraints: ConstraintSnapshotRef, // scope・保存禁止・消去条件の写し
}

struct LearningView {
    learning: LearningId,
    revision: LearningRevision,
    scope: LearningScope,            // Companion / Global（意味状態）
    grounds: SummaryGroundsRef,      // Summary→形成・改訂の対応（本文複製しない）
    // 共有 current から私的過去全文へ辿らせない。取得後の copy も現在の制限に従う。
}
```

- query 成功は後続の送信・共有許可ではない。Global 本文が使えても私的根拠は漏らさない。到着順が新しい＝根拠が新しいにしない。

### H-E scope 変更

意味判断と強制を分離する。具体的な依存方向・API・制約表現の完成ではなく、interface の分離が要点。

```rust
struct ProposeScopeChangeCandidate {
    learning: LearningId,
    expected_revision: LearningRevision,
    proposed_scope: LearningScope,
    shared_content: SharedContentRef, // 共有する内容と必要背景の選択・要約
    non_share_intent: Option<NonShareIntentRef>, // Owner の明示的非共有（あれば優先）
}

enum ScopeDecision {
    ScopeChanged(LearningRevision),
    KeptAsCompanionScope,            // 不明なら Companion に留める
    DeniedByExplicitConstraint,      // 明示保存禁止・非共有で解除不能
    StaleTarget { current_revision: LearningRevision },
}
// 決定後の迂回不能な適用は権限・制約と各利用箇所の協調（K-B・第10節の hold 参照）。
// 認識・学習が強制の owner にならない。
```

- 重要度だけで Global 化しない。削除を契機とする自動 Global 化をしない。

### H-F Observer 向け限定 routing 文脈の供給

Task と Learning を統合せず、Task context の参照は既存の個体調整–作業の協調を通じて行い、共有観測へ Task 所有・包括 access を移さない。

```rust
struct RoutingContextOffer {
    source_owner: RoutingSourceOwner, // History・個体文脈=個体調整 / Memory・Learning=認識・学習 / Task context=作業
    target_companion: CompanionId,
    purpose: RoutingPurpose,         // routing 用途に限定
    constraint: RoutingConstraintRef, // source 制約＋専用 assignment 送信条件
    selection_premise: SelectionPremiseRef, // 選択時点の前提
    // 元情報に従属する一時的な派生表現。Summary・新 Learning 正本・新 scope ではない。
}
```

- 全 private 公開・Global 化・他 Companion 共有にしない。要約・変換で制約を消さない。scope 変更・同意失効・消去は処理中派生物にも反映する。

### H-G 未伝達の登録・報告状況更新

```rust
struct RegisterUndeliveredFact {
    companion: CompanionId,
    source: UndeliveredSource,       // TaskRecord(TaskId) | ActivityRecord(ActivityId)
    round: Option<RoundId>,
    presence_generation: PresenceGeneration,
    restore_generation: RestoreGeneration,
}

enum ReportStatusTransition {
    PendingToSummarized,
    SummarizedToPresented,
    MarkedPresentationUnknown,       // 提示不明を保持（確定済みにしない）
    StaleSource,                     // 元記録が通常保持管理で失われた場合の関係確認用
}
// 接続・表示 copy 送信・Task 完了だけで報告済みにしない。実際の提示状況で更新する。
```

- 正本は個体調整。Task 由来なら Task 記録へ参照を戻す。報告用要約は派生表現。persistence は PR Group B（`undelivered`）の D1+D3。

### H-H Schedule 設定・到来・各回対応

```rust
struct CreateScheduleCommand {
    owner_selection: OwnerSelectionRef,
    assignee: CompanionId,
    content: ScheduleContentRef,     // 実行内容・初期 Workspace 入力
    timezone: ScheduleTimezoneRef,   // 作成時 tz（黙置換しない）
}

struct ScheduleOccurrenceFact {
    schedule: ScheduleId,
    occurrence: ScheduleOccurrenceId,
    status: OccurrenceStatus,        // Missed | Started | CancelledAsTaskContract
    started_task: Option<TaskId>,    // 各回は新 Task
}
// 作成依頼を特別な Permission token にしない。各回で現在の制限を再評価する。
```

## 5. Permission / Credential / Inference / Execution クラスタの interface semantics（K-A〜K-K）

「一つの security middleware を通れば全部成立」という設計にしない。評価・解決・送信・作用・記録の各 boundary を分離する。秘密値を通常の interface value として広く返さない。

### K-A Owner 意図 → 制御変更

```rust
struct ProposeControlChangeCommand {
    origin: OwnerIntentOrigin,       // 会話由来 / 管理面操作の区別＋引用対応
    kind: ControlChangeKind,         // 一回承認 / 将来 Rule / 割当同意 / cap 変更等の別
    target_scope: ControlTargetScopeRef, // 対象活動・目的・担当・Task/委任・Client/round 等の対応
    relied_intent: ReliedIntentRef,  // 依拠した Owner 意図・Rule の対応
}

enum ControlChangeDecision {
    AppliedAsOneTimeApproval,        // 現在の明確な依頼の一回限りの承認
    NeedsClarification,              // 曖昧・矛盾・過度に広い・重大
    DeniedByBoundary,                // 永続 Deny・Always ask・Capability 境界の黙上書きに当たる
}
```

- 開始：入出力・提示＋個体調整・作業（意図供給）。判断：権限・制約。LLM 出力・Learning・Character・Skill・外部 content は材料であり、Owner 由来の管理意図との対応なしに control plane を変更できない。Rule 保存自体は Action の trigger にしない。
- Rule store の identity 型（`RuleId`）・`StoredAsRule` decision・K-B の `rule_revision` expected 欄は rule store 導入 stage で再導入する。

### K-B 現在の利用可否の照合（live authorization check）

判断記録と生きた許可を区別する。保存 Allow・委任時 copy・事前判定・復元 Rule・文脈内許可文・cache 判定を現在許可として再利用しない。

```rust
struct CheckLiveAuthorizationQuery {
    candidate: ActionCandidateRef,   // 目的・対象・data・送信先・作用の解釈（K-H の候補対応）
    relied_evaluation: PermissionEvaluationId, // 依拠した判断の対応（あれば）
    task: Option<TaskRef>,           // Task revision 前提
    delegation: Option<DelegationId>,
    workspace: Option<WorkspaceAssocId>,
    presence: Option<PresenceCheckRef>, // Client 依存なら claimed generation＋現接続前提
    consent_revision: Option<ConsentRevisionExpectation>,
    cap_context: CapCheckContextRef, // 費用・資源・並列・反復の上限照合用
    hold_context: HoldCheckContextRef, // 失効・停止・消去・復元保留の照合用
}

enum LiveAuthorizationDecision {
    AllowForThisUse(PermissionEvaluationId), // 今回の利用に対する確定（再利用可能な包括許可ではない）
    Deny(DenyReasonRef),
    AskOwner(OwnerQuestionRef),      // Owner 判断待ち（実行せず待機）
    WaitForCondition(WaitConditionRef),
    NeedsRevalidation(NeedsRevalidationRef),
}
```

- 各開始について現在の条件を実対象へ適用する。適用責任は実行・拡張（作用）・推論（送信）・各参照保存箇所に残り、許可の意味自体は権限・制約に残る。無関係な変更ごとの再承認は要求しない。再評価要否は重要な変更で決める（K-H）。
- setup completeness（consent 記録・credential 登録・bearer の有無）は推論 admission（K-E）が唯一の owner であり、本 query は setup 状態を含めない。本 live check は consent 状態だけを判定し、setup 不足を独立に拒否しない。

### K-C 秘密利用（認証用途への供給）

caller へ secret ownership を渡さない形を優先する。参照を持つことは利用可能ではない。

```rust
struct RequestAuthenticatedUseCommand {
    credential_ref: CredentialRef,   // 非秘密の用途参照（秘密値を含めない）
    connection: ConnectionNeedRef,   // どの接続・用途を必要とするか（秘密を含まない参照）
    operation: OperationKind,        // 認証を伴う操作の種別
    // LLM が記述した参照だけでは利用可能にならない。設定済み接続・用途・有効性の照合が必要。
}

enum AuthenticatedUseOutcome {
    UsedWithinScope,                 // 必要範囲での秘密利用の供給（値は返さない）
    NeedsReauthentication,           // 失効・不足・無効
    DeniedByConstraint,              // 保存禁止・失効・保留等
    StaleReference,                  // 参照と現在の store・用途の不一致
}

// 供給側の約束：秘密値を model context・LLM 生成 Tool argument・通常 result・UI・
// History・Summary・Learning・Task結果・log・Audit・Debug・backup へ流さない。
// 認証先 error・result の反射にも受入・保存・提示箇所が参加して非露出を維持する。
```

- 秘密値本体は OS credential store 等の分離保管（E）。DB 側は非秘密参照だけを持つ（PR Group K）。Rust interface 上も `SecretValue` 型を public に返さない。必要な認証用途へ「利用させる」が、値は認証秘密の boundary 内に留める。例えば `with_credential(handle, |opaque| ...)` 的な限定利用、または接続 owner への直接供給であり、汎用 `get_secret() -> String` を設けない。

### K-D Provider 割当解決

解決済み経路を独立した利用可能 assignment の正本にしない。

```rust
struct ResolveAssignmentQuery {
    consumer: UsageConsumer,         // CompanionReasoning / ObserverDedicated / TaskAgent(inherit) 等
    capability: CapabilityKind,
    current_consent: ConsentExpectationRef, // expected consent revision
}

struct ResolvedRouteCandidate {
    route: ProviderRouteRef,         // 派生結果（authority ではない）
    consent: AssignmentConsentId,    // 依拠した同意の対応
    capability_gap: Option<CapabilityGapRef>, // 能力不足なら利用前に不足を示す
}
// 利用可否の確定は K-B の live check で現在条件と照合して行う。登録・認証成功で成立させない。
// 送信（K-E）は確定済み前提を再照合しない。
```

- Observer は専用 assignment を解決し、Companion override・同意の選択・合成を行わない。

### K-E 推論の実利用ごとの成立・送信

最初の送信・再送・補助推論・継続的な送受信の各継続部分もそれぞれ実際の利用として成立させる。fallback（K-F）は Stage 16（Multi-provider / fallback / full cost cap）で再導入する。

```rust
struct AdmissionRequest {
    candidate: InferenceUseCandidate, // consumer・用途・論理 context・capability の対応（写し）
    // consent / credential premise は admission が読む確定材料。
    // 秘密値・permission 内部を利用元へ返さない。
}

enum Admission {
    Admitted(Box<AuthorizedInference>), // 確定済み前提（ticket・consent premise・provider/model・candidate）
    Declined(NotSentReason),            // 前提不足・失効・allowlist 外。送信前に落下し副作用を残さない
}

struct InferenceAttempt {
    ticket: InferenceTicketRef,
    capability: CapabilityKind,
    expected_consent: ConsentPremise,               // (id, rev) を組で運ぶ
    expected_credential_set: CredentialSetRevision, // prompt が scrub された credential set
    provider: ProviderRouteRef,
    model: ProviderRouteRef,
}

enum InferenceDispatchOutcome {
    Completed {
        arrival: InferenceResultArrival,
        adopted: bool, // await 後の adoption consent が成立したか
    },
    NotSent(NotSentReason), // 送信前に拒否。usage fact を残さない
}

struct InferenceResultArrival {
    ticket: InferenceTicketRef,      // 元要求・範囲との対応
    output_text: ProviderOutputRef,  // 送信表現・Provider 側 context・戻り結果と元要求・情報範囲の対応
    usage: UsageFactRef,             // 報告 / 不明の区別付き
}
```

- 送信手順：admission が現在の consent / credential premise と K-B の single-use authorization を確定して `Admission` を返し、attempt claim が保存 consent・credential set との一致を単一 transaction で確定してから `ProviderTransport` で送信する。input cap は claim 前、prompt の credential-set premise は claim と同一 transaction で照合する。claim 後の provider I/O は lock なし並列に行い、送信時点で権限・route を再 gate しない（admission / claim との二重 gate を作らない）。await 後の adoption consent 不成立は結果の採用だけを止め、usage 記録は attempt に従う。
- setup completeness（consent 記録・credential 登録・bearer の有無）の判定は admission が唯一の owner であり、不足は `NotSent` として送信前に処理する。permission の live check は consent 状態だけを判定する。
- 参照できたことと送れることの区別、解決済み送信先の包括許可化の禁止、同意不足の本文削減による黙解消の禁止。Prompt cache・session は最適化に限る。判定用推論にも自身の割当同意・認証用途・費用制限を適用する。

### K-F fallback 選択

```rust
struct ProposeFallbackCandidate {
    ticket: InferenceTicketRef,
    failed_route: ProviderRouteRef,
    next_candidate: ProviderRouteRef, // 承認済み順序内の次候補
}

enum FallbackDecision {
    AllowedAsApprovedFallback,
    DeniedAsUnapprovedRoute,         // 未承認 Cloud への自動移送を含む
    NeedsRevalidation(NeedsRevalidationRef), // 用途・data・privacy・Credential・cap の現在照合が必要
}
```

- 安価さ・Capability 不足を例外にしない。登録先変更等で同意の意味が重要に変わるなら以前の同意をそのまま適用しない。
- 本 interface は Stage 16（Multi-provider / fallback / full cost cap）で再導入する。

### K-G 利用量の予約・確定・解放（cost reservation）

同一残額の独立使い切りを許さない。処理中・不明をゼロにしない。

```rust
struct ReserveUsageCommand {
    consumer: UsageConsumer,
    cap: CapId,                      // per CapId（＋ provider / 全体 scope）
    upper_bound: UsageUpperBoundRef, // 推定上限（過小予約による超過をしない）
    attribution: UsageAttributionRef, // 用途・送信先の対応
}

enum ReservationOutcome {
    Reserved(ReservationId),
    DeniedByCap(CapStateRef),
    HeldForUnknownCost,              // 費用不明で安全継続不可
    NeedsRevalidation(NeedsRevalidationRef),
}

struct CommitUsageCommand {
    reservation: ReservationId,
    actual: UsageActualRef,          // 報告値 / 不明の別（推定は provider 実測が入る stage で再導入）
}

struct ReleaseUsageCommand {
    reservation: ReservationId,      // 未使用予約の解放（既生消費・不明消費は release しない）
}
```

- 原記録は各利用 owner（推論・作業・実行・拡張・保全・消去）に残し、権限・制約は可否だけを管理する。persistence は PR Group F（`usage_fact_*`）の D2+D3。commit は短い transaction 内の原子照合（CCT SD-Cap）。予約後の inference 実行は lock なし並列。費用計算法・集計期間・推定方式は Design Freedom。
- `CommitUsageCommand` は当該利用 owner の再起動・Agent 停止後の再評価経路からも呼べる。所有 in-flight を失った `Reserved` を元の reservation / 利用対応に基づいて不明として確定し、release しない（PR §6.2、CCT §9.2）。caller の process 生存を必要条件にせず、別 owner に利用事実の変更権を移さない。不明と理由は費用管理面で区別して表示する。

### K-H Action 候補 → 認可 → 実作用 → 結果確定

判断時の対象記述と実行時の実対象の文字列一致だけでは対応にしない。実対象解決を経て対応を確かめる。

```rust
struct ActionCandidate {
    principal_chain: PrincipalChainRef, // 実行主体＋委任 chain
    task: Option<TaskRef>,           // Task revision 前提
    delegation: Option<DelegationId>,
    workspace: Option<WorkspaceAssocId>,
    purpose: ActionPurposeRef,       // 依頼・自発・Schedule 各回・steering のどれに基づくか
    described_target: DescribedTargetRef, // 判断時の対象記述
    operation: OperationKind,        // Read | Create | Edit | Delete | Execute を潰さない
    data_use: DataUseRef,            // 主な data と用途（Credential 値を含めない）
    cost_risk: CostRiskRef,          // 費用・risk（推定・不明の区別付き）
    relied_intent_rule: ReliedIntentRuleRef, // 依拠 Owner 意図・Rule の対応
}

// 実利用直前の現在条件（実行・拡張が適用）。重要な意味が変われば再評価する。
struct RecheckTrigger {
    // 対象同一性・操作種別・作用拡大、目的・steering 変化、Task・委任変化、
    // Workspace・scope 変化、帰属・対象変化、依拠 Rule・同意・device・Credential 変化、
    // 費用・risk の重要変化、停止・保留発生の有無。無関係な変更は再評価理由にしない。
}

struct ExecuteActionCommand {
    candidate: ActionCandidate,
    authorization: PermissionEvaluationId, // K-B の今回確定（生きた許可の写しではない）
    resolved_target: RealTargetRef,  // 解決後の実対象
    presence: Option<PresenceCheckRef>, // Client 依存なら generation＋現接続前提
    cost_reservation: Option<ReservationId>,
    erasure_check: ErasureConditionRef, // 飛行中の消去条件の照合用
    restore_premise: RestoreGeneration, // 復元跨ぎ参照の世代タグ
}

enum ActionStartOutcome {
    StartedAsAttempt(ActionAttemptId),
    Denied(DenyReasonRef),
    AskOwner(OwnerQuestionRef),      // 実行せず待機
    StalePremise(StalePremiseRef),   // Task revision・委任・Workspace・実対象・許可・帰属・消去・復元条件の不一致
    HeldByGlobalHold(HoldConditionRef),
}

// 開始後の outcome tracking（並列、per-attempt CAS）。実行・拡張が確定する。
struct ReportEffectFact {
    attempt: ActionAttemptId,
    stage: ActionStage,              // 受付 / 開始 / 送信 / 把握の別
    certainty: ActionCertainty,      // ConfirmedSuccess | ConfirmedFailure | Unknown（粘着）
    grounds: EffectGroundsRef,       // 根拠対応（Agent 申告を証拠にしない）
    prior_unknown: Option<ActionAttemptId>, // retry の元不明対応（新試行の証明）
}
```

- 受入の分離：入力受付・Task 受理・認可判断・実作用開始・把握作用・内部保存・Task 達成・Owner 報告は別の事実。一つの成功状態へ潰さない。確認済み成功 / 確認済み失敗 / 不明を区別し、確認できないことを未実行・失敗・成功へ勝手に変換しない。retry・再実行は新試行とする。
- Timeout は確定度を変更しない（`Unknown` のまま）。遅延成功は元 attempt へ `Unknown→Confirmed` の CAS で記録し、現在 Task への採用は H-A の受入で別途照合する。
- persistence は PR Group E（`action_attempt`）の D2+D3。開始前 compare は短い `Immediate` transaction（CCT §8.1、PR AU5）。

### K-I 拡張受入

```rust
struct RequestExtensionUseCommand {
    extension: ExtensionKind,        // MCP Tool/Resource/Prompt | Plugin(限定拡張点) | MCP Apps UI
    target_action: ActionCandidate,  // 実際の作用との対応（名称だけで Capability を縮小しない）
    mcp_connection: McpConnectionRef, // 非秘密接続設定・command・設定の由来
    sandbox_exception: Option<SandboxExceptionRef>, // 特定 Local MCP の隔離外例外（Plugin へ流用しない）
}

enum ExtensionUseOutcome {
    AcceptedUnderLimit,
    DeniedAsBypass,                  // Deny 迂回・内部回り込み・自己承認に当たる
    NeedsRevalidation(NeedsRevalidationRef),
}
// 外部 process 内部への完全強制・確実な停止は報告しない。確認できない作用は不明とする。
// MCP Apps の操作は外部 Tool UI の一時入力であり、第一者の制御変更・承認を直接成立させない。
```

### K-J Client 依存作用の限定・移動協調

```rust
struct RequestClientBoundActionCommand {
    base: ExecuteActionCommand,      // K-H の実作用要求
    delegator_presence: PresenceCheckRef, // 委任元 Companion の現在の active Client 前提
    device_permission: DevicePermissionExpectationRef,
}
// 現在の Client に存在することは Action の許可を意味しない。
// ambient Observation の有効化を操作の承認としない。
// 移動先が利用可能でも旧 Action 不明を自動再実行しない。
```

### K-K 失効・停止・保留の伝播、遅延到着・不明の扱い

```rust
struct PropagateHoldNotification {
    scope: HoldScopeRef,             // 対象と理由（失効・停止・cap・消去・復元保留等）
    hold: HoldConditionRef,
}

struct LateArrivalAttribution {
    attempt: Option<ActionAttemptId>,
    task_revision_premise: Option<TaskRef>,
    source_range: Option<SourceRangeRef>,
    generation_tags: GenerationTagsRef, // presence / restore / sweep の区別
    arrival_kind: LateArrivalKind,   // Tool結果 / 承認 / 推論結果 / usage報告等の別
}

enum LateArrivalHandling {
    RecordedToOriginal,              // 元 Action・Task への対応付け（次の自動開始なし）
    AdoptedToCurrentIfPremiseHolds,  // 現在照合が成立する用途だけ accept
    SuppressedByErasure,             // 消去条件で再保存を抑止
    KeptUnknownWithDupRisk,          // 不明のまま重複 risk 付き Owner 判断へ戻す
}
```

- 複数理由が重なる場合、一つの解除だけで他の禁止を解除しない。Cancel・失効がすべての事実報告を禁止するわけではなく、記録できたことが次の利用を許すわけでもない。

## 6. Presence / I/O / Observation クラスタの interface semantics（X-A〜X-H）

Client-originated message だけで authoritative presence を成立させない。Observer routing と Companion individual inference を別 boundary として維持する。

### X-A presence 成立・移動・復帰・復旧（個体調整 → 接続・存在）

```rust
struct RequestMoveCommand {
    companion: CompanionId,
    from_client: Option<ClientId>,
    to_client: ClientId,
    reason: MoveReasonRef,           // Owner呼出し / 事前指示 / 文脈上の自発 / Host発の通常切断fallback / Host再起動復旧の別
    expected_generation: PresenceGeneration, // boundary token
    expected_state: PresenceStateExpectation,
    round_closure: RoundClosureRef,  // 入出力・提示と実行・拡張からの区切り・利用不能・作用不明の対応
}

enum MoveDecision {
    TransitioningToNew(PresenceGeneration), // 旧→移行中→新の durable 遷移を開始
    RejectedAsStalePresence(StalePresenceRef), // expected generation / state 不一致
    DeniedByConstraint,              // pairing・device・停止・保留等
}

struct PresenceAttributionFact {
    companion: CompanionId,
    state: PresenceState,            // Present | NoActive | InTransition | Stopped | RecoveryWait
    active_client: Option<ClientId>,
    generation: PresenceGeneration,  // 単調。区間の識別
}
```

- 判断：帰属成立は接続・存在。移動の必要性は個体調整、pairing・device 可否は権限・制約。Host 同居 Client にも同じ帰属制約を適用する。移行中は新旧いずれも Client 依存の新規開始をしない。二重 presence を禁止する。同一 `CompanionId` の帰属切替は SD-Presence で逐次化（CCT §4）。
- hint・復旧先の更新と帰属成立を同一視しない。現在接続を古い保存値から再成立させない。

### X-B Text / Voice round の帰属・区切り・提示

```rust
struct SubmitClientInputCandidate {
    companion: CompanionId,
    client: ClientId,
    claimed_generation: PresenceGeneration, // Client 側の主張（authority ではない）
    round: RoundId,
    input_ref: ClientInputRef,       // 入力内容への参照（一時表現）
    trial_or_candidate: Option<ObservationCandidateOrTrialRef>,
}

enum RoundIntakeOutcome {
    AcceptedForRound(RoundId),       // 現在帰属・現接続・許可・停止・保留の照合が成立
    StaleRound(StaleRoundRef),       // 旧世代・旧 round。元 round へ対応付け、新 round へ付け替えない
    HeldForTransition,               // 切替区間の新規開始禁止
    NeedsRevalidation(NeedsRevalidationRef),
}
```

- `RoundClosureFact` / `ConfirmPresentationObservation`（presentation crate の outcome 型）は current stage で producer / consumer がなく、必要 stage で再導入する。提示確認は wire `PresentationStatus` から `PresentationMark`（round と presented / unknown）への ingress で行い、送信≠報告完了を維持する。
- 会話の意味・History は個体調整、round の実際・提示・区切りは入出力・提示、帰属成立は接続・存在。旧 round の入力・生成途中・未提示を新 round へ付け替えない。生成済み＝提示済みにしない。Voice に話者認証済みの意味を足さない。
- wire の新規 round 開始要求（IPC §13.1 の `SubmitTextInput.round = None`）は入出力・提示が現在条件の照合と round 発行を行ってから、本 interface の `RoundId` を満たす。Client や ingress mapping が domain identity を発行せず、旧 round の拒否を新規開始へ自動読替えしない。

### X-C Body 表示の帰属

Body 表示は帰属・活動状態・内的状態・資材内容の連言であり、単独の成立では開始しない。排他性を確認できない Client では継続させない。出力した表情・motion を Companion State の正本・永続変化の根拠にしない。描画失敗・fullscreen・高負荷を Text・管理・復旧へ波及させない。

```rust
struct CheckBodyEligibilityQuery {
    companion: CompanionId,
    client: ClientId,
    presence: PresenceCheckRef,      // 現在帰属の写し
    activity: CompanionActivityRef,  // 個体調整の活動状態
}
// Decision は X-A・X-B の成立に従属する独立の巨大判定にしない。必要な連言の照合結果を返す。
```

### X-D Observation eligibility の連動

```rust
struct NotifyPresenceChangeFact {
    client: ClientId,
    present_companions: Vec<CompanionId>, // authoritative 帰属に従う存在人数（Stopped 除外）
    fullscreen: FullscreenStateRef,  // 入出力・提示から
    observer_control: ObserverControlRef, // Client別・全体 Pause/OFF
}

enum EligibilityDecision {
    EligibleForCapture,
    Ineligible(IneligibilityReasonRef), // Stoppedのみ・fullscreen・Pause/OFF・activeなし・cap 等
}
```

- 対象・時機は共有観測、帰属成立は接続・存在。旧 Client の旧 Capture・候補で新規 Capture・delivery を続けない。複数対象 Client は同時に Capture しない。

### X-E Observation result routing（三段階分離）

Observer routing（共有観測）と Companion individual inference（個体調整＋当該 Companion の Provider 設定）を別 boundary として維持する。

```rust
struct PublishObservationCandidate {
    client: ClientId,
    candidate: ObservationCandidateId,
    captured_at: WallClockWithTz,
    capture_range: CaptureRangeRef,  // desktop 全体（個別 window 対象と誤認させない）
    routing_context: RoutingContextOffer, // H-F の派生表現
}

enum RoutingDecision {
    RoutedTo(Vec<CompanionId>),      // その Client 上の関連する Running Companion だけ
    SuppressedByControl,             // Pause/OFF・fullscreen・同意・cap・消去等
    StaleCandidate,                  // 移動・Stop・Pause 前の旧候補
}

struct DeliverEventNotification {
    candidate: ObservationCandidateId,
    target_companion: CompanionId,
    deliverable_range: DeliverableRangeRef, // その個体が利用可能な観測内容と必要背景
    // 受信は理解・採否を意味しない。Observer による routing は各 Companion の
    // 意味判断・最終発話・Action 判断を置き換えない。
}

struct CompanionUnderstandingFact {
    companion: CompanionId,
    candidate: ObservationCandidateId,
    understood_as: UnderstandingRef, // 個体文脈での意味判断（個体調整）
    experience_link: Option<ProposeExperienceCandidate>, // Experience への利用（H-B へ）
}
```

- 全 private 公開・新 Learning 正本・Global 化・他 Companion 共有の禁止、要約・変換による制約消去の禁止、Companion override・各個体同意の選択・合成による代用の禁止、混合生成文しかなく分離を確認できない場合の配送禁止、一方の個体の判断成功の他個体へのコピーの禁止。delivery 後の個体 Provider 送信は別 consumer の新たな利用として K-E・K-B へ戻す。

### X-F 移動・切断・再接続時の三者協調、X-G Host restart 時の協調

X-A・X-B の facts/notifications の組合せであり、新しい巨大 interface を追加しない。要点のみ固定する。

- 帰属切替は接続・存在、round 区切りは入出力・提示、Client 依存作用の区切り・不明は実行・拡張、通常 Host 作業の継続・Client 依存 step の判断待ち・保留は作業、未伝達の Host 保持・次 Client 要約報告は個体調整が確定する。通常 Host Task の終了・移送を切替の前提にしない。
- 移動・切断中の到着物は元帰属・元 round・元試行へ対応付け、用途別に受け入れる。到着先の現在活動へ付け替えず、Client copy で Host を上書きしない。未送信操作を自動 Action queue にしない。遅延 message・表示 copy で現在帰属・Permission・Task 状態を復活させない。不明を未実行・成功へ変換しない。
- Host restart 時：presence 復旧と round / Computer Use / Task / Action の再実行・再開は別の条件。Running は再起動前 Client へ自動復元（元 Client が利用不能なら active なし）。途中 Task は明示再開待ち。停止中の Schedule 回は missed。未完了消去・復旧の保留を維持する。

### X-H 未伝達の次 Client 報告

H-G の登録に加え、次 Client での提示側の interface。

```rust
struct RequestUndeliveredSummaryQuery {
    companion: CompanionId,
    to_client: ClientId,
    presence: PresenceCheckRef,      // 新 presence 成立後の現在帰属
}

struct UndeliveredSummaryFact {
    items: Vec<UndeliveredRef>,      // 元 Task・活動記録への対応付き
    current_filter: CurrentFilterRef, // 現在の元結果・利用制限・削除状況への照合
}
// 提示途中で再び切断したなら不明を保持する。exactly-once・既読保証は追加しない。
```

## 7. Character クラスタの interface semantics（C-A〜C-D）

validation 成功を Permission / execution authorization として返さない。

### C-A Character static revision の取得

```rust
struct GetCharacterRevisionQuery {
    character: CharacterId,
    revision: Option<CharacterRevision>, // None は現在 revision 一覧の取得
}

struct CharacterRevisionView {
    character: CharacterId,
    revision: CharacterRevision,
    parts: Vec<CharacterPartView>,       // 静的部品・推奨 Skill 指定（実行許可ではない）
    import_provenance: ImportProvenanceRef, // 受入対応（外部原本の所有ではない）
}
// revision の存在≠適用・適用すべきという推測をしない。供給は確定済み revision のみから行う。
```

### C-B 既存 Companion への revision 適用

内容の正本（Character）と適用関係の正本（個体調整）を同一更新にしない。

```rust
struct ProposeCharacterApplicationCandidate {
    companion: CompanionId,
    character: CharacterId,
    expected_character_revision: CharacterRevision, // boundary token
    selected_parts: Vec<CharacterPart>,  // Owner が部品ごとに明示選択した範囲
    owner_selection: OwnerSelectionRef, // 管理操作との対応
}

enum CharacterApplicationOutcome {
    AppliedAs(AppliedPartListRef),   // 個体調整が適用関係を確定（経験状態を初期化しない）
    StaleRevision { current_applied: AppliedPartListRef },
    DeniedProhibitedPart(ProhibitedPartRef), // Credential / Permission / 同意 / authority 等の適用禁止種別
    NeedsOwnerSelection,             // 部品ごとの明示選択が不足
}
// Package 更新を成長の初期化にしない。未確認部品を更新済みにしない。
```

- persistence は PR Group A/B（`character_revision`＋`companion_applied_current`＋履歴）。commit は SD-CharApply の短い transaction（CCT §4）。

### C-C Skill import・package validation

```rust
struct ImportSkillCommand {
    source: SkillImportSourceRef,    // Package 推奨 / 単体 import の別＋外部参照（所有ではない）
    scope_choice: SkillScopeChoice,  // Package 由来→Companion scope 既定、単体→Owner 選択
    target_companion: Option<CompanionId>, // Companion scope の場合の帰属先
}

struct ValidatePackageQuery {
    package_ref: ExternalPackageRef, // 外部 Package 原本への参照（所有ではない）
}

struct PackageValidationReport {
    result: ValidationVerdict,       // 受入可否・内容範囲・trusted/untrusted 扱い
    import_provenance: ImportProvenanceRef,
    // validation 成功は Permission / Credential / 実行 authority / Global Skill の成立ではない。
}

enum SkillImportOutcome {
    ImportedAsCompanionSkill(SkillId),
    ImportedAsGlobalSkill(SkillId),  // 単体 import の Owner 選択時のみ
    RejectedByScopeRule,             // 自動 Global 化・自動昇格に当たる要求
    RejectedByValidation,
}
```

### C-D Character export

```rust
struct ExportCharacterCommand {
    character: CharacterId,
    revision: CharacterRevision,
    // 静的範囲だけを選択する。個体固有 Experience・Learning・関係・内的状態・
    // 履歴・Credential・Permission を混入させない。内容と権利上の注意を提示可能にする。
}

struct CharacterExportFact {
    export_ref: ExportRef,           // 外部 copy（所有ではなく配布物）
    included_scope: IncludedStaticScopeRef,
}
```

## 8. Data preservation / lifecycle クラスタの interface semantics（D-A〜D-E）

generic `delete(entity_id)` で Targeted Deletion を表現しない。coordinator が semantic owner の内部 state を直接自由に編集する interface にしない。

### D-A Targeted Deletion の開始・範囲確定

```rust
struct RequestTargetedDeletionCommand {
    purpose: DeletionPurposeRef,     // Privacy / Security 目的の明示（通常忘却等との区別）
    mechanical_condition: MechanicalConditionRef, // LLM 非依存の文字列条件（必須層）
    semantic_hint: Option<SemanticHintRef>, // 意味的補助層（完全性を保証しない）
    exclusion: ExclusionRef,         // 除外・要確認の対応
    impact_notice: ImpactNoticeNeedRef, // 目的・範囲・影響・除外の事前説明用
}

struct DeletionScopeDecision {
    operation: DeletionOperationId,
    sweep: DeletionSweepGeneration,  // 消去区間の順序
    valid_interval: ErasureInterval, // 開始〜検証完了（区間内再到着を含む）
    participants: Vec<ParticipantOwnerRef>, // 参加すべき領域（保存場所を Owner に選ばせない）
}
```

- 保全・消去が Owner の明示目的・対象・影響・除外を対応付け、参加すべき領域の特定を支援する。特定文字列の機械的検索・削除・残存検証を LLM へ依存させない。

### D-B 参加者への局所消去要求・局所完了返却

```rust
struct DemandLocalErasureCommand {
    operation: DeletionOperationId,
    sweep: DeletionSweepGeneration,
    erasure_condition: ErasureConditionRef, // 各受入・保存先が照合する条件・有効区間・完了境界
}

struct ParticipantCompletionFact {
    operation: DeletionOperationId,
    participant: ParticipantOwnerRef, // 情報 owner だけでなく処理中 context・派生物・cache・Client/拡張一時 copy・返却可能結果の保持者を含む全 holder
    local_result: LocalErasureResult, // 処理・検証・未完了・失敗・未確認範囲の別
    source_relation: SourceRelationRef, // source 関係・処理中利用・局所扱い・検証・再保存防止・未確認の対応
    // 局所完了≠全域完了。未確認・到達不能を成功と読まず、局所返却だけで hold を解除しない。
}
```

- 各 holder は `(source 関係, 処理中利用, 局所扱い・検証, 再保存防止, 未確認・未完了)` を保全・消去へ説明する。source を消した後に依存関係も消失し遅延結果を識別できなくなる実装は不可。本文を保持せず対応を維持できること。

### D-C 残存検証・全域完了確定

```rust
struct VerifyRemainderQuery {
    operation: DeletionOperationId,
    sweep: DeletionSweepGeneration,
}

enum RemainderVerification {
    NoRemainderMechanically,         // 機械的残存検証の成功（意味的完全性ではない）
    RemainderFound(RemainderRef),
    UnreachableScope(UnreachableRef), // 到達不能は成功にしない
}

enum GlobalDeletionCompletion {
    GloballyCompleted,               // 全域完了（保全・消去が確定。各 domain の意味変更は各 owner が行う）
    HeldPending(PendingRef),         // 未完了・保留・再保存防止の維持
    FailedVerification,              // 検証失敗
}
// 全域完了は内部全域の除去または復元不能化・機械的残存検証・区間内再到着の取込み・
// 再保存/再形成防止・未完了集約の全体。完了記録・Audit へ対象本文を戻さない。
// 完了時に機械的条件の検索 token を wipe し、完了後の backup に token を残さない。
```

### D-D Backup 作成・Restore staging・正本 switch・復元後保留・一括有効化

```rust
struct CreateBackupCommand {
    setting: BackupSettingRef,       // 保存先・独自 schedule・保持数・保護選択
}

struct BackupPointFact {
    backup_point: BackupPointId,
    covered_scope: CoveredScopeRef,  // 対象時点・参照対応・除外（secret・外部実体）・未完了状況
    // 各部 copy 成功だけを成功にしない。対象時点・参照・履歴・未完了の対応が揃って初めて成功。
}

struct RequestRestoreCommand {
    backup_point: BackupPointId,
    // 事前説明：対象・version互換性・外部非変更・削除済み情報や旧 Rule・同意・Schedule の復活可能性・再認証必要性。
}

struct StagedRestoreCandidate {
    restore: RestoreId,
    backup_point: BackupPointId,
    isolation_check: IsolationCheckRef, // staging（別 file / 別 group）での照合・検証
}

struct SwitchRestoreDecision {
    restore: RestoreId,
    new_generation: RestoreGeneration, // 成功ごとに前進。旧 live 分離用
    hold: HoldConditionRef,          // 復元後保留
    // switch 前は復元前正常が正本、switch 後は復元内容が正本。第三の混合を作らない。
    // switch の瞬間だけ singleton の短い排他（SD-Restore）。全期間の read 停止はしない。
}

struct BulkEnableAfterRestoreCommand {
    restore: RestoreId,
    // Owner が内容確認後まとめて有効化できる。一件ずつ再承認を要求しない。
    // 復元済み assignment / consent だけで自動利用を開始しない。
    // 現在 Credential・現制約・復元後保留・Deny・cap・認証不足・不明を無視しない。
}
```

- Restore は開始前 Credential secret を除く対象内部 data の全置換であり、merge ではない。失敗時は復元前正常を維持する。旧 live 結果・Client copy を新正本へ混ぜない。外部 file の現内容・存在・access、外部作用、認証成功は巻き戻らない。復元参照を現在 store と照合し、不足・無効なら再認証を要求する。

### D-E 通常保持・retention / cleanup・Reset

目的を一つの削除・初期化へ潰さない。通常忘却・訂正・統合・失効・置換、通常 History/log 削除・retention、容量 retention（既定 OFF・明示 opt-in）、Companion 削除、Targeted Deletion、設定 Reset、全データ Reset、Restore を別の lifecycle・別の完了条件として扱う。

```rust
struct ApplyRetentionCommand {
    policy: RetentionPolicyRef,      // 通常 History/log 保持・容量 retention（既定 OFF）の別を明示
    target_range: RetentionTargetRef, // 指定日以前等の範囲
}

struct ResetCommand {
    kind: ResetKind,                 // Settings | FullData
    enumeration: ResetEnumerationRef, // 削除対象の列挙・強い確認（全データ時）
}
// 設定 Reset は一般設定の既定化であり、Companion・History・Summary・Learning・
// Relationship・State・Task・Schedule・Credential・Rule・同意・cap を削除しない。
// 全データ Reset は Host 内部 data と Credential を削除し、外部 Workspace・
// Workspace 内 Skill・Owner 保存 backup を削除しない。
```

## 9. 管理経路の独立（第一者管理面 → 各 owner）

第一者の入出力・提示は、作業への Cancel・Schedule 管理・記録確認、個体調整への個体停止・削除、権限・制約への承認拒否・Rule・同意・cap・device 管理、認証秘密への明示的な認証設定、保全・消去への消去・backup・restore・Reset を直接要求できる。ここで「直接」は本体 LLM の承認や長時間 Task の完了を介在させない意味であり、具体 API の指定ではない。各 owner の受理・確認・結果に従い、UI に任意 state の書換権を与えない。受付と完了、保存済み data への影響、既知作用と不明を区別して提示する（H-10 / X-10 / K-1 / D-A〜D-E の管理入口の共通性質）。

高権限操作の最終確認は Host PC 上の trusted first-party management surface に限定する（[要件「信頼境界」](../../requirements/requirements.md#信頼境界)、具体的な対象・確認境界は IPC §18）。pairing / device trust・revocation、Credential 変更、Restore・復元後一括有効化、Full Reset 等の要求を Remote Client が送れても、remote の確認申告だけでは成立させない。担当 owner は Host が確認した入口の由来と当該操作・対象・現在前提に結び付く確認事実を必要とし、下記 command の存在を確認済みと読まない。同居 transport や pairing だけでは trusted first-party 性を与えず、Tool / Computer Use 等の代理入力による最終確認も受け入れない。管理面全体に会話の active 制約は課さない。

```rust
struct ManagementOperationCommand {
    operation: ManagementOperationKind, // StopCompanion | DeleteCompanion | CancelTask |
                                       // ManageSchedule | DenyOrRefuse | ManageRuleConsentCap |
                                       // ManageDevice | ConfigureCredential | RequestDeletionBackupRestoreReset
    target: ManagementTargetRef,
    // 本体 LLM の承認・長時間 Task の完了を介在させない。LLM 応答成功を必要としない。
}
```

## 10. Boundary token の具体的な渡し方（field matrix）

巨大な同一 context struct を渡さない。各 boundary で必要な前提だけを typed に表現する。token は authority ではなく比較材料であり、不一致なら新規利用を開始せず hold・deny・不足・再評価へ戻す。欠落は不受理の理由。

凡例：● = 必須 field、○ = 該当する場合に必須、— = 当該 boundary では持たない。`世代`は presence / restore / sweep のいずれかを型で区別する（混同しない）。

| interface | expected identity+revision | expected generation（型付き） | source / consumer | purpose・用途・scope | operation / attempt | certainty | hold / 削除 / 復元関係 |
|---|---|---|---|---|---|---|---|
| H-A Task/steering/委任 | ● `TaskRef` | ○ `RestoreGeneration`（復元跨ぎ参照） | ● 委任元 Companion・Task・Workspace | ● 目的・steering 前提・委任 scope | ○ `DelegationRef` | —（達成は作業、確定度は実行・拡張） | ● 停止・保留・消去・復元保留の照合 |
| H-A Agent 結果受入 | ● `DelegationRef`（Task revision 前提含む） | ○ presence/restore/sweep タグ | ● 委任元・Task | ● 現在目的との対応 | ● attempt 対応 | ○ Agent 申告（証拠にしない） | ● Cancel・steering・消去・復元との照合 |
| H-B Experience | ○ Task 由来なら `TaskRef` | ○ presence/restore/sweep タグ | ● 経験個体・Task・委任 | ● intended_use・scope 期待 | — | — | ● 保存禁止・非共有・消去条件 |
| H-C 訂正 | ● `(LearningId, expected_revision)` | ○ sweep タグ | ● 対象・新 Experience | ● 時間的意味 | — | — | ● 消去・制約 |
| H-D Learning参照 | ○ `(LearningId, revision)`（取得物側） | ○ sweep タグ | ● consumer・purpose・scope 需要 | ● 用途・scope・制約写し | — | — | ● scope・禁止・消去 |
| H-E scope変更 | ● `(LearningId, expected_revision)` | ○ sweep タグ | ● 対象・共有内容 | ● 共有内容・背景・非共有意図 | — | — | ● 明示制約 |
| H-F routing文脈 | ○ source 対応 | ○ presence/sweep タグ | ● 元 owner・対象 Companion・用途 | ● routing 用途・制約・選択前提 | ○ 候補対応 | — | ● 同意・消去・失効 |
| H-G 未伝達 | ○ source 対応（Task/Activity） | ● presence＋restore generation | ● 対象 Companion | ● 報告状況 | ○ round | ○ Presented/Unknown | ● 削除状況 |
| K-B live check | ● Task/Rule/consent の expected revision | ○ presence/restore/sweep タグ | ● 実行主体・委任・Task・Workspace | ● 目的・対象・操作・data・送信先・費用 | ○ attempt 前提 | — | ● 失効・停止・cap・保留 |
| K-C 秘密利用 | ○ `CredentialRef`（非秘密）＋用途 | — | ● 接続・用途 | ● 認証用途の制限 | ○ 操作種別 | ○ 認証成功/失敗・再認証要否 | ● 失効・保留 |
| K-D/E/F 推論 | ○ consent expected revision | ○ restore/sweep タグ | ● consumer・Capability | ● 用途・data・送信先・取扱い・費用 | ○ ticket | ○ 不足・失敗・利用量 | ● 禁止・cap・保留・消去 |
| K-G 予約 | ○ `CapId`＋帰属 | — | ● consumer・用途・送信先 | ● 上限・引当量 | ○ reservation | ○ 報告/不明/処理中 | ● cap・不明 |
| K-H Action開始 | ● Task revision・Rule/consent expected | ● presence＋restore＋sweep（該当分） | ● 主体・委任・Task・Workspace | ● 目的・実対象・操作・data・送信先・費用 | ● relied evaluation＋reservation | —（開始時点） | ● 失効・停止・cap・保留・消去・復元 |
| K-H 結果確定 | ● `attempt`＋expected certainty | ○ 世代タグ | ● Task・委任 | ● 対象・作用の対応 | ● prior_unknown | ● Confirmed/Unknown | ● Cancel・再実行条件 |
| X-A 移動 | ● `(CompanionId, expected_generation+state)` | ● `PresenceGeneration` | ● 対象個体・移動元先 | ● 移動理由 | ○ round 閉鎖対応 | ○ 到達性・排他性 | ● pairing・device・停止・保留 |
| X-B round受入 | ○ `(Companion, Client, round)`＋claimed generation | ● `PresenceGeneration` | ● 対象・Client・round・候補 | ● 会話用途 | ○ round・試行 | ○ 受付/提示/不明 | ● 切替・停止・保留・消去 |
| X-E routing | ○ 候補＋文脈対応 | ○ presence/sweep タグ | ● 由来 Client・対象個体・用途 | ● routing 用途・制約 | ○ 候補 | — | ● 観測制御・同意・消去 |
| D-B 局所消去 | ● `(operation, sweep)`＋source 関係 | ● `DeletionSweepGeneration` | ● 参加 owner・保持対応 | ● 目的・範囲・除外 | — | ○ 局所検証結果 | ● 有効区間・完了境界・hold |
| D-D restore | ● `backup_point`＋世代前提 | ● `RestoreGeneration` | ● 対象・参照対応 | ● 用途・再有効化範囲 | ○ restore/staging | ○ staging 検証・switch 成否 | ● 復元後保留・ credential 照合 |
| C-B 適用 | ● `(CharacterId, expected_revision)`＋適用 pointer | ○ restore/sweep タグ | ● 対象 Companion | ● 部品・Owner 選択 | — | — | ● 禁止種別 |

`PresenceGeneration` と `RestoreGeneration` と `DeletionSweepGeneration` は別型・別 field であり、混ぜない。Task の目的変更は `TaskRevision` の前進であり、generation 変化で代替しない（CI §6.4）。

## 11. Acceptance result と error model（domain outcome と technical error の分離）

単純な `Result<T, Error>` だけでは stale / denied / held / superseded / insufficient / unknown-external / historical-only 等を表せない場合がある。一方、すべての domain へ共通巨大 Status enum を導入しない。technical failure と semantic rejection / hold / stale を分離する。

### 11.1 返却形の約束

```rust
// domain outcome は Ok 側、technical failure は Err 側。
type InterfaceResult<Outcome, TechErr> = Result<Outcome, TechErr>;

// 例：Task 提案
fn propose_task(cmd: ProposeTaskCommand)
    -> impl Future<Output = InterfaceResult<TaskProposalOutcome, TaskTechnicalError>>;
// 例：Action 開始
fn request_action(cmd: ExecuteActionCommand)
    -> impl Future<Output = InterfaceResult<ActionStartOutcome, ActionTechnicalError>>;
```

- `Ok(...)`：owner が確定した domain 受入（受理・stale・deny・hold・不足・再評価・元記録のみ等）。呼び出し側は variant に従って hold・再評価・Owner 判断待ちへ戻す。黙って queue・replay しない。
- `Err(...)`：DB unavailable、Provider transport error、OS I/O 失敗等の technical failure。意味の採否ではない。再試行可否は別途 CCT・AE の不明・重複契約に従う（自動再実行しない）。

### 11.2 domain outcome enum の一覧（domain ごとに区別）

共通 enum に潰さない。各 enum は当該 owner の意味だけを持つ。

| domain | outcome enum（例） | 主な variant の意味 |
|---|---|---|
| Task 提案・steering・委任 | `TaskProposalOutcome`、`DelegationOutcome` | Accepted / StalePremise(current 付き) / HeldByGlobalHold / NeedsRevalidation / InsufficientContext |
| Agent 結果受入 | `TaskResultAcceptance` | AdoptedToCurrent / RecordedToOriginalOnly / HeldForPermissionReview / DiscardedAsStaleWithRecord |
| Experience・訂正・scope | `FormationDecision`、`CorrectionOutcome`、`ScopeDecision` | Formed / Deferred / Declined / Corrected / KeptAsCompanion / DeniedByExplicitConstraint / StaleTarget / HeldByErasure |
| Permission live check | `LiveAuthorizationDecision` | AllowForThisUse / Deny / AskOwner / WaitForCondition / NeedsRevalidation |
| 秘密利用 | `AuthenticatedUseOutcome` | UsedWithinScope / NeedsReauthentication / DeniedByConstraint / StaleReference |
| 推論・fallback | `InferenceDispatchOutcome`、`FallbackDecision` | Completed / NotSent / AllowedAsApprovedFallback / DeniedAsUnapprovedRoute |
| 予約・確定・解放 | `ReservationOutcome` | Reserved / DeniedByCap / HeldForUnknownCost / NeedsRevalidation |
| Action 開始・確定 | `ActionStartOutcome`、`LateArrivalHandling` | StartedAsAttempt / Denied / AskOwner / StalePremise / HeldByGlobalHold / RecordedToOriginal / KeptUnknownWithDupRisk / SuppressedByErasure |
| presence・round | `MoveDecision`、`RoundIntakeOutcome` | TransitioningToNew / RejectedAsStalePresence / DeniedByConstraint / AcceptedForRound / StaleRound / HeldForTransition |
| routing | `RoutingDecision` | RoutedTo / SuppressedByControl / StaleCandidate |
| Character 適用・import | `CharacterApplicationOutcome`、`SkillImportOutcome` | AppliedAs / StaleRevision / DeniedProhibitedPart / NeedsOwnerSelection / ImportedAsCompanionOrGlobal / RejectedByScopeRule / RejectedByValidation |
| 削除・検証・完了 | `ParticipantCompletionFact`、`RemainderVerification`、`GlobalDeletionCompletion` | 局所処理・検証・未完了・失敗・未確認範囲の別 / NoRemainderMechanically / RemainderFound / UnreachableScope / GloballyCompleted / HeldPending / FailedVerification |
| Backup・Restore | `BackupPointFact`、`SwitchRestoreDecision` 等 | 成功は対象時点・参照・履歴・未完了の対応が揃って初めて成功。各部 copy 成功だけを成功にしない |
| Repository compare | 各 `*Outcome`（第13節） | Committed / StaleExpected / HeldByOperation / MergedAsNewRevision（該当 domain のみ） |

`accepted only as historical evidence` は `RecordedToOriginalOnly` / `AdoptedToOriginalOnly` 的な variant で表す。`accepted for recording but not current semantic use` は元 Action / Task / 元 revision への記録と現在 Task・現在認識への採用の分離で表す（H-A、K-H、K-K）。

### 11.3 technical error の配置（`thiserror` 等）

- technical error 型は各 infrastructure / owner 層で定義する。bare `String` / `Box<dyn Error>` を public library error にしない（repo 規約）。
- 例（配置の提案。改名は許す）：
  - `TaskTechnicalError`（作業の durable 層）：`StorageUnavailable`、`CommitConflictIo`（compare 自体の I/O 失敗。stale ではない）等。
  - `ActionTechnicalError`（実行・拡張の作用層）：`DeviceIoFailed`、`SandboxLaunchFailed` 等。Deny・stale・hold は含めない。
  - `InferenceTechnicalError`（推論の transport 層）：`ProviderTransportFailed`、`ResponseLost`（不明の原因。確定度の書換えではない）等。
  - `PresenceTechnicalError`（接続・存在の観測層）：`ReachabilityCheckFailed`（確認不能。不在の断定ではない）等。
  - `PreservationTechnicalError`（保全・消去の file/DB 層）：`BackupIoFailed`、`StagingCorrupted` 等。検証失敗・到達不能とは区別する。
- domain outcome と technical error を同じ error category へ潰さない。`stale` / `denied` / `held` / `not-current` / `cap exceeded` は `Ok` 側の variant であり、`Err` 側の retry 対象ではない。呼び出し側が `Err` を `Denied` と誤読して Owner へ誤った成功・拒否表示をしないこと（CC-07）。

## 12. Long-running operations — request と completion の分離

推論、Task Agent、external Tool、Computer Use、Backup / Restore、Targeted Deletion の長時間処理について、request 開始時の interface と completion / result interface を分離する。request object を process memory だけに保持しないと帰属不能になる設計は避ける。

| 長時間処理 | request（開始） | completion / result（到着） | 戻れる対応（durable） |
|---|---|---|---|
| 推論（単発・継続・fallback・再送） | `AdmissionRequest` / `AuthorizedInference` → attempt claim（ticket 発行・予約・admission の前提確定） | `InferenceResultArrival`（ticket→結果・利用量） | `(ticket, consumer, Task/委任対応, 用途, revision/generation 前提, provenance)`。PR Group F/I、CI §6.4 の世代タグ |
| Task 委任・Task Agent | `CreateDelegationCommand`（expected revision の atomic compare） | `TaskAgentResultArrival`（delegation→現在 Task の受入） | `(delegation, TaskRef 前提, scope 写し, attempt 対応, 目的)`。PR Group D、CI §5.3 |
| Action 試行・外部 Tool・Computer Use | `ExecuteActionCommand`（開始前 atomic compare）→ `StartedAsAttempt(attempt)` | `ReportEffectFact`（per-attempt CAS）＋ `LateArrivalAttribution`（遅延帰属） | `(attempt, Task revision 前提, 実対象・操作, 依拠 Permission, presence/restore 世代, prior_unknown)`。PR Group E |
| Backup 作成 | `CreateBackupCommand` | `BackupPointFact`（対象時点・参照・未完了の対応が揃って成功） | `(backup_point, 対象時点・参照対応・除外・未完了状況)`。PR Group J |
| Restore | `RequestRestoreCommand` → `StagedRestoreCandidate`（隔離検証） | `SwitchRestoreDecision`（generation bump＋switch）→ `BulkEnableAfterRestoreCommand` | `(restore, backup_point, RestoreGeneration, hold, Credential 照合)`。PR Group J |
| Targeted Deletion | `RequestTargetedDeletionCommand` → `DeletionScopeDecision` | `DemandLocalErasureCommand` → `ParticipantCompletionFact` → `VerifyRemainderQuery` → `GlobalDeletionCompletion` | `(operation, sweep, valid_interval, participant 対応, hold)`。PR Group J、CI §5.9 |
| 未伝達報告 | `RegisterUndeliveredFact`（親原子で登録） | `RequestUndeliveredSummaryQuery` → `UndeliveredSummaryFact` → `PresentationMark`（提示確認後のみ確定） | `(undelivered_id, source 対応, 報告状況, round・世代対応)`。PR Group B |
| Character 適用供給 | `GetCharacterRevisionQuery`（供給） | `ProposeCharacterApplicationCandidate`（Owner 選択付き提案→確定） | `(character, revision, parts, OwnerSelectionRef)`。PR Group A/B |

completion 側では元の identity / revision / generation / attempt / operation / provenance へ戻れる。遅延物は `attempt → task revision → 現在 Task` の順に辿り、記録（元へ残す）と semantic 更新・次実行・提示（現在の受入）を分ける（CI §6.3）。Cancel・失効・steering・移動・削除・復元後の到着物は元の Action / Task へ記録し、旧承認の解除・旧結果の新目的採用・後続の自動開始をしない。

## 13. Repository interfaces — compare-and-commit を成立させる形

単純な `load() -> modify -> save()` だけでは race する domain について、expected revision / generation / compare-and-update / append fact / reserve / commit / release の operation をどう表すかを設計する。DB transaction そのものを business layer へ露出しすぎない。

### 13.1 方針

- business layer は `Transaction` object を保持・受渡ししない。repository method が内部で短い `Immediate` transaction（または owner-local serialization との協調）を行い、compare と durable 更新を不可分にする。transaction 内で await・外部 I/O を行わない（CCT §5）。
- business layer が渡すのは `expected_*`（boundary token）と新規内容であり、読み直し・比較・更新の順序は repository が守る。戻り値は domain outcome（Committed / Stale / Held）であり、生の row count ではない。
- 万能の generic repository / generic CRUD interface を導入しない。同じ invariant を持つ場合だけ共通化する（例：per-row CAS の形は共有しても、型・lifecycle・counter は domain ごとに区別する）。
- persistence の table / record は semantic owner にならない。意味変更は各 owner が行い、persistence は対応の保持・照合可能性だけを支える（PR §2）。

### 13.2 domain 別の repository interface（pseudo-trait。必要なものだけ abstract する）

実装差し替え・boundary 隔離・testability の理由がある interface だけ abstract する。内部 pure function まで trait 化しない。

```rust
// --- Task（PR Group D。SD-Task。前提 read → 長時間処理 → 短い commit compare） ---
struct TaskCommitPremise {
    expected: TaskRef,               // expected revision
    new_purpose: Option<TaskPurposeRef>, // steering なら新目的
    new_context: Vec<TaskContextEntryRef>, // 新 revision 対応の context（本文複製を要求しない）
}

enum TaskCommitOutcome {
    CommittedAs(TaskRef),            // 新 revision（steering 時は cur+1）
    StaleExpected { current: TaskRef },
    HeldByGlobalHold(HoldConditionRef),
}

trait TaskRepository {
    // Task 作成：task + revision + 初期 context + workspace_assoc の原子 durable。
    // commit 前は委任・実行から不可視（durable-before-visible）。
    async fn create_task(
        &self,
        purpose: TaskPurposeRef,
        assignee: CompanionId,
        workspace_need: WorkspaceNeedRef,
        restore_generation: RestoreGeneration,
    ) -> Result<TaskRef, TaskTechnicalError>;

    // steering：新 revision＋新 context の原子 forward。旧 revision を残す。
    async fn forward_steering(
        &self,
        premise: TaskCommitPremise,
    ) -> Result<TaskCommitOutcome, TaskTechnicalError>;

    // 委任作成：expected_task_revision の atomic compare を満たして作成する。
    async fn create_delegation(
        &self,
        task: TaskRef,
        scope_copy: DelegationScopeRef,
    ) -> Result<DelegationOutcome, TaskTechnicalError>;

    // 結果採用の判定：現在 revision・目的との対応を確認して行う。二重完了にしない。
    async fn adopt_result(
        &self,
        arrival: TaskAgentResultArrival,
        current_hold: HoldCheckContextRef,
    ) -> Result<TaskResultAcceptance, TaskTechnicalError>;

    // 履歴・context は append 系。現在値の上書きではない。
    async fn append_task_revision_record(
        &self,
        record: TaskRevisionRecordRef,
    ) -> Result<(), TaskTechnicalError>;
}

// --- Action attempt（PR Group E。SD-Attempt。append-only＋per-row CAS） ---
struct AttemptCommitPremise {
    expected_task: Option<TaskRef>,  // Task revision 前提
    relied_evaluation: PermissionEvaluationId,
    real_target: RealTargetRef,
    operation: OperationKind,
    presence: Option<PresenceCheckRef>,
    erasure: ErasureConditionRef,
    restore_generation: RestoreGeneration,
    reservation: Option<ReservationId>,
}

trait ActionAttemptRepository {
    // 開始：同一 Immediate transaction 内で Task・委任・Workspace・実対象・
    // 現在許可・device・費用・停止・保留・消去・復元条件を照合して原子に insert する。
    async fn insert_attempt_if_current(
        &self,
        premise: AttemptCommitPremise,
    ) -> Result<ActionStartOutcome, ActionTechnicalError>;

    // 確定度更新：(attempt_id, expected_certainty) の CAS。新 evidence の事実 owner 確認を必須とする。
    // Unknown は粘着させ、Cancel 受付・通信成功・表示・保存成功・再接続・復元・移動で書き換えない。
    async fn compare_and_set_certainty(
        &self,
        attempt: ActionAttemptId,
        expected: ActionCertainty,   // 通常 Unknown
        new: ActionCertainty,
        grounds: EffectGroundsRef,
    ) -> Result<CertaintyUpdateOutcome, ActionTechnicalError>;
}

// --- Presence（PR Group G。SD-Presence。per-Companion CAS＋旧→移行中→新） ---
trait PresenceRepository {
    async fn compare_and_begin_transition(
        &self,
        companion: CompanionId,
        expected_generation: PresenceGeneration,
        expected_state: PresenceStateExpectation,
        to_client: ClientId,
    ) -> Result<MoveDecision, PresenceTechnicalError>;

    async fn confirm_transition(
        &self,
        companion: CompanionId,
        transitioning_generation: PresenceGeneration,
        live_check: LiveReachabilityRef, // DB 外確認の結果（atomic には含めない）
    ) -> Result<PresenceAttributionFact, PresenceTechnicalError>;
}

// --- 費用・cap（PR Group F。SD-Cap。予約＋commit/release の三状態） ---
trait UsageRepository {
    // 予約 insert＋cap 照合を同一 transaction で行う。処理中・不明をゼロにしない。
    async fn reserve_if_under_cap(
        &self,
        cmd: ReserveUsageCommand,
    ) -> Result<ReservationOutcome, UsageTechnicalError>;

    async fn commit_reserved(
        &self,
        cmd: CommitUsageCommand,
    ) -> Result<(), UsageTechnicalError>;

    async fn release_reserved(
        &self,
        cmd: ReleaseUsageCommand,
    ) -> Result<(), UsageTechnicalError>;
}

// --- Learning（PR Group C。形成 commit 時に Memory revision・scope・制約・消去条件を照合） ---
trait LearningRepository {
    async fn commit_formation_if_current(
        &self,
        target: Option<(LearningId, LearningRevision)>, // None は新規
        grounds: SummaryGroundsRef,
        scope: LearningScope,
        constraint: ConstraintSnapshotRef,
        erasure: ErasureConditionRef,
    ) -> Result<FormationDecision, LearningTechnicalError>;

    // 過去 revision・根拠は append 系。現在値の上書きではない。到着順を根拠の新旧にしない。
    async fn append_revision_record(
        &self,
        record: LearningRevisionRecordRef,
    ) -> Result<(), LearningTechnicalError>;
}

// --- Character 適用 pointer（PR Group A/B。SD-CharApply） ---
trait CharacterApplicationRepository {
    async fn compare_and_set_applied(
        &self,
        candidate: ProposeCharacterApplicationCandidate,
    ) -> Result<CharacterApplicationOutcome, CharacterTechnicalError>;
}

// --- 保全・消去（PR Group J。SD-Deletion / SD-Restore） ---
trait PreservationRepository {
    // operation＋erasure_condition の先行 durable（durable-before-enforce）。
    async fn durable_operation_first(
        &self,
        scope: DeletionScopeDecision,
    ) -> Result<(), PreservationTechnicalError>;

    // 参加者の局所完了は各 owner で durable 化してから返却する。返却で hold を解除しない。
    async fn record_participant_completion(
        &self,
        fact: ParticipantCompletionFact,
    ) -> Result<(), PreservationTechnicalError>;

    // 全域完了の原子確定：各 participant の局所完了・検証・未完了・失敗×
    // 機械的残存検証×区間内再到着の取込みを満たして確定する。完了時に検索 token を wipe する。
    async fn declare_global_completion_if_verified(
        &self,
        operation: DeletionOperationId,
        sweep: DeletionSweepGeneration,
        verification: RemainderVerification,
    ) -> Result<GlobalDeletionCompletion, PreservationTechnicalError>;

    // Restore switch：staging 検証後の restore_generation bump＋正本 pointer switch の原子確定。
    async fn switch_restore_generation(
        &self,
        restore: RestoreId,
        expected_generation: RestoreGeneration,
    ) -> Result<SwitchRestoreDecision, PreservationTechnicalError>;
}

// --- 未伝達（PR Group B。SD-Undelivered。登録は親原子、確定は per-row CAS） ---
trait UndeliveredRepository {
    // 会話由来は History append と同一原子、Task 由来は Task durable 後の別 transaction 原子登録。
    async fn register_if_parent_durable(
        &self,
        fact: RegisterUndeliveredFact,
    ) -> Result<(), UndeliveredTechnicalError>;

    // 提示確認後のみ Presented にする（durable-after-confirmed）。送信だけで Presented にしない。
    async fn compare_and_mark_reported(
        &self,
        id: UndeliveredId,
        expected: ReportStatus,
        mark: PresentationMark,      // 提示 round と presented / unknown の観測
    ) -> Result<ReportStatusTransition, UndeliveredTechnicalError>;
}
```

- `*_TechnicalError` は infrastructure の失敗であり、stale / hold / deny ではない。`StaleExpected` / `HeldByOperation` 等は `Ok` 側の outcome で返す。
- cross-owner の更新を一つの巨大 transaction・一つの actor へまとめない。必要なのは短い cross-owner atomic read（同一 SQLite transaction の共有）であり、意味変更権の統合ではない（PR §7）。

## 14. Rust-oriented design — trait / struct / enum の具体化範囲

本書は具体的な Rust interface 方針まで固定し、trait / service / repository interface / command / query struct / domain-specific result enum / newtype / borrowed / owned / async method を pseudo-code で示す（第4–9・13節）。ただし「trait を使えるところは全部 trait」にしない。

| 抽象化するもの（trait 化） | 理由 | 抽象化しないもの |
|---|---|---|
| `TaskRepository`、`ActionAttemptRepository`、`PresenceRepository`、`UsageRepository`、`LearningRepository`、`CharacterApplicationRepository`、`PreservationRepository`、`UndeliveredRepository` | boundary 隔離（business owner と durable 保持の分離）、compare-and-commit の不可分性の隠蔽、testability（in-memory fake による stale / hold / delayed の再現） | 具体 SQL・index・migration。DB 製品固有の詳細 |
| 推論の transport（Provider protocol adapter） | 未対応 protocol の Plugin 補完・既知 protocol の直接接続の差替え、testability（fake Provider による不足・失敗・利用量の再現） | 論理的 context 選択・プロンプト組立・圧縮・scoring。利用元の意味判断 |
| 実行・拡張の OS・device・MCP 境界 | sandbox・device・adapter の差替え、testability（fake 作用・不明の再現） | 実対象解決の domain logic、確定度の意味判断 |
| 観測の Capture・adapter 境界 | Observation adapter の外部 code 境界、testability | 候補検知の scoring・routing 選択 algorithm |
| Credential の分離保管（OS store 抽象） | DPAPI / libsecret / Keychain 等の差替え、秘密の非露出の強制 | 用途・参照元の意味、同意・許可の確定 |

- borrowed / owned：ID・revision・generation・token は `Copy` 可能な小さな値または参照として渡し、本文・payload・派生物は参照・ID 対応で辿る。対応の伝達に private 本文を複製しない（CI §4.4、PR §2）。
- async method：repository・推論・実行・観測等の I/O・長時間処理を伴う boundary は `async fn` とする。commit の原子区間自体は短く、transaction 内で await しない（CCT §5）。長時間処理は premise 付きで開始し commit 時に compare する。
- newtype：CI §5 の `CompanionId` / `TaskId` / `TaskRevision` / `PresenceGeneration` / `RestoreGeneration` / `DeletionOperationId` / `DeletionSweepGeneration` 等を維持する。内部表現が同じでも相互変換しない。revision / generation は生の数値として単独で持ち歩かない。
- `unsafe` を本設計の表現のために要求しない（CI §2.3）。
- error 型は第11節の配置に従い、各層で `thiserror` を用いる。bare `String` / `Box<dyn Error>` を public error にしない。

## 15. IPC readiness — process / network boundary を越える可能性がある interface

IPC wire schema は [Host↔Client IPC](host-client-ipc.md) が定める。本節は process-local interface と remote-capable interface の区別とその理由を固定する。

| 区分 | interface | 越境するもの・理由 |
|---|---|---|
| remote-capable（Host↔Client） | X-B round 受付・提示・区切り（`SubmitClientInputCandidate` / `RoundIntakeOutcome` / 提示確認の `PresentationMark`） | Client は入力・表示の一時表現だけを持ち、Host 正本を持たない。Client message は candidate であり、Host の現在帰属・許可・保留との照合が必要。未送信操作の自動 queue・Client copy による Host 上書きをしない |
| remote-capable | X-A 移動・復帰（`RequestMoveCommand` / `PresenceAttributionFact`） | 呼出し・移動意図は Client から届くが、成立は Host の帰属記録。hint・復旧先だけで presence を成立させない |
| remote-capable | X-H 未伝達の次 Client 報告（`UndeliveredSummaryFact`） | 次 Client での要約報告は Host の元記録・利用制限・削除状況へ照合した派生表現。接続・表示 copy 送信だけで報告完了にしない |
| remote-capable | X-D eligibility 通知の一部（`NotifyPresenceChangeFact` の Client 向け表示） | 観測状態・対象範囲の説明可能性のため。Raw・候補・routing 用 data・私的 context は送らない |
| remote-capable | D-B の Client 一時 data 参加（`DemandLocalErasureCommand` の Client 宛て分） | 接続中 Client の一時 data・拡張一時 data も参加する。切断・応答なしを消去成功とせず、未確認範囲を保全・消去へ伝える。再接続時に旧 copy を戻さない |
| remote-capable | C-A の表示資材利用（Body・Voice 資材） | Character 静的資材の表示利用。適用関係・経験状態は送らない。内部正本の主 key として再利用できる形で渡さない |
| remote-capable（高権限の最終確認を除く） | 管理面の表示・操作（第9節の `ManagementOperationCommand` の Client 側入口） | 管理操作の意味・適用・成功判定は担当 owner から受ける。高権限の要求は送れても最終確認は Host PC 上の trusted first-party management surface で行う（IPC §18）。UI に任意 state の書換権を与えない |
| Host-local（越境させない） | H-B〜H-E の形成・訂正・scope 意味判断、K-A〜K-C の制御確定・秘密利用、K-D〜K-G の割当解決・送信条件・予約の確定、K-H の認可・作用確定、D-A・D-C・D-D の範囲確定・完了確定・switch | 現在条件の照合・秘密・世代・消去条件を Host canonical で確定するため。Client・Provider・MCP・Plugin を正本・owner にしない。解決済み経路・判定 copy・Client 主張をそのまま越境先の authority にしない |
| Host-local（越境させない） | 第13節の repository compare-and-commit 群 | durable primary の compare を Host 単一 SQLite transaction で不可分にするため。DB transaction を Client・Provider・MCP へ露出させない |
| 外部境界（Host→Provider / MCP / Plugin / 外部 file） | K-E の実送信、K-I の拡張利用、K-C の認証用途供給、C-C の import 受入 | 用途・data・送信先・同意・費用・秘密保護の条件付き。Provider・MCP・Plugin を domain owner にしない。秘密値を通常経路に載せない |

- Host 永続化・Host↔Client 間の受渡し・Audit / Debug 記録では、ID・revision・generation・correlation を明示 field として serialize する。自由記述の本文中に埋め込んだ文字列を照合に使わない（CI §4.6）。
- Client・Provider・MCP・Plugin との境界では、Host の内部 ID 体系をそのまま露出する必要はない。露出する場合は用途限定の参照（non-secret reference）に留め、内部正本の主 key として再利用できる形で渡さない。認証秘密値は通常経路に載せない（DR-05）。
- 時刻は wall-clock＋作成時 timezone を保持する。Schedule の timezone 黙置換をしない。

## 16. Crate / module decomposition への材料 — 依存方向

crate 構成は [Crate / Module 分解](crate-module-decomposition.md) が定める。本節は semantic dependency と interface 呼び出し方向を固定する。

### 16.1 依存方向表（caller → owner の interface 依存）

矢印は architectural dependency（DR §1）であり、crate・module・process への直写ではない。B の結果が A へ返ることだけで逆向きの architectural dependency を追加しない。一方、B が A の活動状態を判断根拠として必要とするなら別行の参照依存として明記する。

| caller（開始・利用） | → owner（確定・意味） | interface | 依存の役割 |
|---|---|---|---|
| 個体調整 | → 作業 | H-A（Task/steering/Cancel） | semantic change 要求＋read/use（進捗・結果）。Task 受理・達成は作業 |
| 個体調整・作業 | → 認識・学習 | H-B / H-C / H-D / H-E（Experience・訂正・参照・scope 意味） | semantic change 要求＋read/use（利用範囲内）。形成・scope 意味は認識・学習 |
| 個体調整（仲介） | → 各元 owner | H-F（routing 文脈） | read/use（範囲限定）。Task 所有・包括 access の移転ではない |
| 個体調整 | → 接続・存在 | X-A（移動要求） | lifecycle coordination（必要性と成立の分離）。排他帰属は接続・存在 |
| 入出力・提示・共有観測・実行・拡張 | → 接続・存在 | X-A・X-D・X-F（帰属・接続・可用性の参照） | read/use。帰属から Action 許可を導かない |
| 接続・存在 | → 入出力・提示・実行・拡張 | X-F（区切り・停止結果・不明の利用） | lifecycle coordination。Task 全体終了を条件にしない |
| 共有観測 | → 個体調整・認識・学習 | H-F・X-E（限定文脈・候補の意味判断要求） | read/use（範囲限定）。最終 Action 判断を行わない |
| 個体調整・作業・認識・学習・共有観測・入出力・提示 | → 推論 | K-E（推論利用） | execution 要求に類する利用＋read/use（能力・利用量）。最終意味は利用元 |
| 個体調整・作業 | → 実行・拡張 | K-H・K-I・K-J（作用・拡張・Client 依存作用の要求） | execution 要求。Permission 確定・成功の自己申告ではない |
| 各利用・保存・実行箇所 | → 権限・制約 | K-A・K-B・K-F・K-G（意図確定・live check・fallback・cap 可否） | enforcement。domain 意味決定・制御自己変更ではない |
| 権限・制約 | → 個体調整・作業・接続・存在・認識・学習・推論・保全・消去 | K-B の前提参照（活動状態・委任範囲・帰属・scope・利用量・保留の参照） | read/use（判断に必要な範囲）。参照先の意味更新・包括取得ではない |
| 推論・実行・拡張・接続・存在 | → 認証秘密 | K-C（認証用途の利用要求） | enforcement に類する限定利用。秘密値の一般参照ではない |
| 各 owner | → 保全・消去 | D-A〜D-E（操作範囲・未完了・保留の依存） | deletion / backup / restore participation。任意 domain 編集ではない |
| 保全・消去 | → 各参加 owner | D-B（対象特定・処理・検証の要求） | lifecycle coordination（目的限定）。通常 owner 化・無条件 cascade ではない |
| Character 利用側（個体調整） | → Character | C-A（静的内容・revision の利用） | read/use。適用関係・経験状態の更新ではない |
| Owner（管理経路） | → 各 owner | 第9節・H-H・C-C・C-D・D-A・D-D・D-E | semantic change 要求＋lifecycle coordination。本体 LLM・長時間 Task を介在させない |

- 必要な双方向依存は限定して残す（DR-12）。例えば権限・制約は作業の委任範囲・推論の消費事実を読み、作業・推論は現在制限に従う。事実報告のために次の Action 許可を必要とせず、制約判断のために審査対象 Action を先に実行しない。循環した承認・成功待ちを前提にしない。graph を一方向に見せるための汎用 abstraction は不要。
- 新しい汎用 mediator、Context、Settings、Persistence 等の Subsystem は追加しない（DR §1）。

### 16.2 分解への制約

- 上表の caller → owner 方向が module / crate 依存方向の第一材料である。owner が caller の内部を知る逆依存を作らない（例：作業が個体調整の会話内部を所有しない、認識・学習が Task 記録を所有しない、権限・制約が学習内容全体を取得しない、保全・消去が任意 domain 編集権を持たない）。
- 同一 owner 内の責務（例：I-1〜I-7、W-1〜W-7、L-1〜L-8、C-1〜C-8、E-1〜E-6、CN-1〜CN-7、IO-1〜IO-8、OB-1〜OB-7、PE-1〜PE-7）は別の state・lifecycle であり、一つの state・trait・table・actor へ潰さないことが分解の制約になる。
- 同じ storage technology の共有（PR §4 の単一 SQLite file 等）は ownership 統合の理由にならない。transaction 共有は mechanism であり ownership ではない。

## 17. Validation — interface だけを追う walkthrough

具体 implementation を想像しなくても、必要な identity / premise / result semantics が interface 上で失われないことを確認する。各 walkthrough は interface ID・渡る token・owner 確定・outcome のみを追う。

### V-1 Owner text → Companion response → Learning（H-B・H-D・K-E・K-B）

1. 入出力・提示が `SubmitClientInputCandidate(companion, client, claimed_generation, round)` を個体調整へ渡す。Client message だけで presence は成立しない。個体調整は X-B の `RoundIntakeOutcome` を経て現在 round として受理する。旧 round なら `StaleRound` として元 round へ対応付け、新 round へ付け替えない。
2. 個体調整は用途（返答）と論理的 context を定め、認識・学習へ `LearningQuery(purpose=返答, scope_need, constraints)` で利用可能な理解を問い合わせる。取得成功は後続の送信許可ではない。
3. 応答のための推論は `AdmissionRequest(consumer=CompanionReasoning, 用途, 論理 context)` の authorize と attempt claim で前提を確定し（K-B の single-use authorization を含む）、`ProviderTransport` へ送信する。同意不足・cap・保留・帰属・消去条件の不一致は `NotSent` として不足・判断待ちへ戻す。
4. 応答後、個体調整・作業は `ProposeExperienceCandidate(experiencer, source_range, source_kind=対話, intended_use=Learning候補)` を認識・学習へ渡す。Raw 複製を要求しない。
5. 認識・学習は `FormationDecision` を確定する。保存価値がなければ終了し、全件保存しない。形成する場合は `SummaryGroundsRef` を対応付け、Memory 等の必要な状態だけを形成・変更する。応答完了と全 Learning 更新完了を同一条件にしない。
6. 失われないこと：由来の区別、対象 Companion・Task・委任との関係、期待する利用先（返答と Learning 候補の別）。到着順が新しい＝根拠が新しいにしない。

### V-2 Task creation → Task Agent → steering → result（H-A・K-H・K-K）

1. 個体調整は `ProposeTaskCommand(requester, purpose, steering_premise, workspace_need, provenance)` を作業へ渡す。会話受付は Task 反映ではない。作業は `TaskProposalOutcome::AcceptedAsTask(TaskRef)` を確定する。作成は原子 durable 後に可視化する。
2. 作業は `CreateDelegationCommand(task=TaskRef(expected), scope_copy, consumer_assignment)` で委任を作成する。Agent は一時主体に留まり、独立権限・Credential・Provider override・予算を持たない。
3. Owner の追加指示は `ProposeSteeringCommand(task=expected, new_purpose, adopted_instruction, unadopted)` で新 revision＋新 context の原子 forward となる。旧 revision を残す。二つの steering 競合は SD-Task の順序で直列化し、先勝ちを現在にし、後着は新現在への再 steering として評価する。
4. steering 後に旧 revision 前提の委任作成・Action 開始が届いたら `StalePremise{current}`・`StaleTaskRevision{current}` として不受理・再評価へ戻す。実行中の旧委任は best-effort 停止・縮小し、旧結果を新目的に自動採用しない。
5. 遅延 Agent 結果は `TaskAgentResultArrival(delegation, attempt_refs, result_body_ref, certainty)` で到着し、`(attempt の Task revision 前提, 目的)×現在の(Task revision, 目的・steering 前提)` を比較する。一致しなければ `RecordedToOriginalOnly` として元 revision へ記録し、現在不採用とする。古い承認で Cancel を解除しない。
6. 失われないこと：発言 record と Task 反映内容と未反映・待機の対応、steering 前後の目的の区別、委任・Workspace・Client 依存条件、Task revision 前提。

### V-3 Action candidate → authorization → external effect → timeout → late result（K-H・K-B・K-K・H-A）

1. 利用元は `ActionCandidate(principal_chain, task, delegation, workspace, purpose, described_target, operation, data_use, cost_risk, relied_intent_rule)` を構築する。構築できたことは許可ではない。
2. 利用・実行箇所は `CheckLiveAuthorizationQuery(candidate, relied_evaluation, task, delegation, workspace, presence, rule/consent expected, cap_context, hold_context)` で K-B の今回確定を受ける。保存 Allow・解決済み経路だけでは開始しない。Owner 確認待ちは `AskOwner` として実行せず待機にする。
3. 実行・拡張は `ExecuteActionCommand(candidate, authorization, resolved_target, presence, cost_reservation, erasure_check, restore_premise)` で開始前 atomic compare を満たして `StartedAsAttempt(attempt)` となる。対象記述の文字列一致だけでは対応にしない。解決後の実対象を保持する。
4. timeout は確定度を変更しない（`Unknown` のまま）。`ReportEffectFact(attempt, stage, certainty=Unknown, grounds, prior_unknown)` の粘着を維持する。Cancel 受付・通信成功・表示・保存成功・再接続・復元・移動で書き換えない。
5. 遅延成功は元 attempt へ `compare_and_set_certainty(attempt, expected=Unknown, new=Confirmed, grounds)` の CAS で記録する。新 evidence の事実 owner 確認を必須とし、Agent 申告を証拠にしない。現在 Task への採用は H-A の受入で別途照合する。不明試行の再実行は新 attempt＋重複 risk を示した Owner 判断を必要とする。
6. 失われないこと：判断対象と実対象の対応、委任不変、確定度（確認済み成功 / 失敗 / 不明）、試行と作用の区別、遅延帰属、重複防止に必要な保持。

### V-4 Provider request near cap → reservation → usage（K-D・K-G・K-E・K-B）

1. 利用元は用途と論理的 context を定め、推論は `ResolveAssignmentQuery(consumer, capability, current_consent)` で経路を解決する。解決済み経路は派生結果であり、設定・同意変更後も以前の選択を有効とする根拠にしない。
2. 各利用 owner は `ReserveUsageCommand(consumer, cap, upper_bound, attribution)` で予約する。同一 transaction 内で `cap_limit`＋関連 `usage_fact_*`（Reserved＋Committed＋Unknown の合計）を読み取って cap 照合する（SD-Cap）。上限超過・不明で継続不可なら開始しない。
3. 予約後の inference・Tool 実行は lock なし並列に行う。他 request を block しない。失効・停止・保留が発生したら best-effort 停止する。
4. 確定時は `CommitUsageCommand(reservation, actual=報告/不明の別)` で原子更新し、余剰を解放する。遅延 usage 報告は元の `usage_id`・attempt・task・assignment 対応へ帰属させる。未報告・処理中・不明をゼロ化・リセットしない（Agent 終了・移動・cache clear・log 整理でも reset しない）。
5. Observer 検知は対象 Client・専用 assignment の一つの利用として扱い、個体数分の重複計上をしない。delivery 後の個体推論は別の実利用として同じ全体 cap へ含める。
6. 失われないこと：報告値・不明・処理中の区別、consumer・用途・送信先の対応、同一残額の独立使い切りの防止、cap・不明での data 保持のままの停止・判断待ち。

### V-5 Client summon → move → stale old Client input（X-A・X-B・X-F）

1. 個体調整は移動意図（Owner 呼出し・事前指示・文脈上の自発）を `RequestMoveCommand(companion, from, to, reason, expected_generation, expected_state, round_closure)` で接続・存在へ渡す。移動の必要性と成立は別。
2. 接続・存在は `expected_generation＋expected_state` の CAS で `旧→移行中→新` の durable 遷移を確定する（SD-Presence）。simultaneous summon は先勝ちのみ成立させ、後着は不受理・再評価へ戻す。移行中は新旧いずれも Client 依存の新規開始をしない。
3. 旧 Client の旧入力・未提示出力は `StaleRound` として元 round・元帰属・元試行へ対応付け、新 round へ付け替えない。旧生成済み未提示出力は未伝達管理へ接続し、次 Client で現在の結果・利用制限に基づいて要約報告する。
4. 新 Client の入力は新しい round・試行だけを許す。成立前の新 Client 入力で新 presence を成立させず、旧 round 継続として実行しない。再接続した Client の古い一時 state だけで現在 presence・Permission・実行再開を成立させない。
5. 失われないこと：presence・Host 継続・接続・許可の区別、切替区間の区別、二重 presence の禁止、安全な区切り、旧作用の別 Client 自動継続の禁止。

### V-6 shared Observation → routing → Companion reaction（H-F・X-D・X-E・K-E）

1. 接続・存在は `NotifyPresenceChangeFact(client, present_companions, fullscreen, observer_control)` を共有観測へ知らせる。Stopped を人数・routing 対象に数えない。旧 Client の旧 Capture・候補で新規 Capture・delivery を続けない。
2. 共有観測は対象・時機を確定し（`EligibilityDecision`）、`PublishObservationCandidate(client, candidate, captured_at, capture_range, routing_context=H-F)` で候補を検知する。Observer 専用 assignment で行い、Companion override・同意を選択・合成しない。全個体への無条件配信・個体ごとの検知重複をしない。
3. 共有観測は `RoutingDecision::RoutedTo(関連する Running Companion だけ)` を確定し、`DeliverEventNotification(candidate, target, deliverable_range)` で伝達する。受信は理解・採否を意味しない。
4. 各個体の個体調整は `CompanionUnderstandingFact(companion, candidate, understood_as, experience_link)` で自身の文脈を踏まえた意味判断を行う。Observer routing は意味判断・発話・Action 判断を置き換えない。発話・Action の最終判断は各 Companion が自身の Character・関係・状況・Rule に基づいて行う。
5. delivery 後の個体推論は別 consumer の新たな利用として K-E・K-B の現在条件へ戻す。Observer で送信できた画面・背景をそのまま各個体 Provider へ送れるとはみなさない。
6. 失われないこと：Client 単位の共有と個体判断の分離、routing 用文脈の派生性（新正本・新 scope・包括共有ではないこと）、元情報の利用制約と専用 assignment 送信同意の変換後の維持、scope 変更・同意失効・消去の処理中派生物への反映。

### V-7 Targeted Deletion 中の delayed Learning result（D-A・D-B・H-B・K-K）

1. 保全・消去は `RequestTargetedDeletionCommand(purpose, mechanical_condition, semantic_hint, exclusion)` を受け、`DeletionScopeDecision(operation, sweep, valid_interval, participants)` を確定する。`deletion_operation＋erasure_condition` の durable を参加開始より先行させる（durable-before-enforce）。
2. 各受入・保存先は保存・採用の commit 時に飛行中の `ErasureConditionRef(operation, sweep, valid_interval)` を lock-free に照合する。到着・生成情報の `(source 関係, 取得・生成時点)×(operation, sweep, valid_interval)` で判定する。区間内再到着・再生成は消去対象とし、新 Experience として救済しない。
3. 削除前に開始した Learning 形成の遅延結果が届いたら、用途別受入（現在 Memory revision・scope・制約）に加えて消去条件を照合する。旧 provenance の遅延物は元記録への事実残しに留めるか破棄し、現在への採用・再保存をしない。削除前情報を利用する実行中形成による再保存をしない。
4. 全域完了は `declare_global_completion_if_verified(operation, sweep, verification=NoRemainderMechanically＋区間内再到着の取込み)` で原子に確定する。局所返却で hold を解除しない。未確認・到達不能を成功と読まない。完了時に検索 token を wipe し、完了記録・Audit へ対象本文を戻さない。
5. 失われないこと：機械的必須層と意味的補助層の区別、消去区間・完了境界・hold、遅延結果の帰属と再保存防止、旧 provenance と完了後新規提供の区別。

### V-8 Restore 後の old live result（D-D・K-K・H-A）

1. 保全・消去は `RequestRestoreCommand(backup_point)` を受け、`StagedRestoreCandidate(restore, backup_point, isolation_check)` で staging（別 file / 別 group）の照合・検証を先行させる。staging 期間の live mutation は旧正本への通常 commit として継続し、staging 内容へ混ぜない。
2. `SwitchRestoreDecision(restore, new_generation, hold)` で `restore_generation` の原子 switch を確定する（SD-Restore）。switch 前は復元前正常が正本、switch 後は復元内容が正本。第三の混合を作らない。失敗時は復元前正常を維持する。
3. 切替前に開始した推論・Tool 結果が切替後に届いても、利用の `(restore generation 前提, assignment/consent revision, Credential 照合, 依拠 Rule revision)×現在の(restore_generation, 現 store, 現制約, 復元後保留)` の照合で抑止し、旧 live・旧同意・旧 assignment だけで自動利用・自動処理を開始しない。旧 live 要求・結果・Client copy を復元正本へ混ぜない。
4. 復元を跨ぐ参照（Task・Rule・同意・assignment・作用・未伝達・全域未完了）には `RestoreGeneration` を添える。旧世代の参照だけで復元後に利用・実行・送信しない。Task の目的変更は `task_revision` の前進であり generation 変化で代替しない。
5. 復元成立後も Task・Schedule・外部接続の自動処理は保留し、Owner が内容確認後 `BulkEnableAfterRestoreCommand` でまとめて有効化できる。現在 Credential・現制約・復元後保留・Deny・cap・認証不足・不明を無視しない。
6. 失われないこと：復元範囲・正本切替・再有効化の分離、stale の現在事実化の禁止、旧 live 混入禁止、Credential 現在 store 維持・外部非巻戻し・単一正本・復元後保留。

### V-9 Character revision apply（C-A・C-B・H-5）

1. 個体調整は `GetCharacterRevisionQuery(character, revision)` で確定済み revision の静的内容・差分を取得する。revision の存在から適用・適用すべきことを推測しない。
2. 個体調整は Owner の部品ごとの明示選択を付けて `ProposeCharacterApplicationCandidate(companion, character, expected_character_revision, selected_parts, owner_selection)` を提案する。適用禁止種別（Credential / Permission / 同意 / authority / Plugin・MCP authority / Global Skill / system control / Rule / cap を含む revision 内容）は適用対象から除外する。
3. 個体調整は適用関係を確定し（`AppliedAs`）、経験由来状態を初期化しない。未確認部品を更新済みにしない。内容の正本（Character）と適用関係の正本（個体調整）を同一更新にしない（SD-CharApply の短い transaction）。
4. validation 成功は Permission / execution authorization として返さない（`PackageValidationReport` は受入可否・内容範囲・trusted/untrusted 扱いであり、制御変更ではない）。
5. 失われないこと：静的 revision と適用関係の分離、部品ごとの Owner 選択、経験状態の非初期化、推奨＝実行許可・有効切替えではないこと、export への private 混入防止。

## 18. 意図的に残した Design Freedom

- 各 interface の concrete method 名・module 配置・同期 / 非同期の粒度（本書は `async fn` を提案するが、actor / channel / 直接呼出し等の mechanism は固定しない）。
- provenance・source_range・selection_premise 等の具体表現、確認のまとめ方、Permission evaluator の内部 algorithm、識別・鮮度確認・競合制御の mechanism（CCT 第5節の三択：楽観 CAS / owner-local mailbox / 短い DB transaction 内 atomic read＋insert のいずれでもよい）。
- Context Assembly・routing 用 context の生成方法・形式・更新頻度・鮮度・選択 algorithm、要約・検索・scoring、prompt template、cache 実装。
- 費用予約量の算定式・集計期間・推定方式、資源配分、反復抑制・Capture 時機、駆動・待機の機構。
- pairing / bootstrap の認証材料の具体形式・保護・受渡し、再起動後の元 Client への再接続・待機、切断検知、帰属調停、安全な round・作用の区切り、停止伝達の mechanism。
- 探索・無効化・検証の実装、backup の整合時点・形式、restore 切替・復旧、保存・暗号化方式、`SealedSearchToken` の実装。
- audit format、診断・telemetry stack、提示確認、要約粒度、具体保持期間、UI layout。
- 上表の対応関係から統一 Context layer、Policy Engine、Manager、Service、Coordinator の追加を導かない。既存の12責務、semantic owner、Host／Client 配置と trust boundary の下で実現方法を選ぶ。
