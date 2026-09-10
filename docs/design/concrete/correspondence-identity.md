# 対応関係・識別の具体設計 — Step 13 Concrete Design (Correspondence & Identity)

本書は Step 13 の最初の具体設計 artifact である。Ene 全体で必要になる「対応関係（correspondence）」を、具体実装で安全に表現・照合できる形まで具体化する。上位設計との優先順位と矛盾時の扱いは [設計文書 README](../README.md#正本と優先順位) に従う。本書内の SO / DR / CC は [State Ownership](../architecture/state-ownership.md) / [Dependency Rules](../architecture/dependency-rules.md) / [Cross-cutting Design](../architecture/cross-cutting.md) を指す。

## 1. 対象と非対象

### 1.1 本書が具体化するもの

- 実装上保持・照合しなければならない対応関係の体系化（第3節）。
- 共通化してよい識別・相関 concept と domain 固有に分離すべきものの境界（第4・5節）。
- Rust における newtype / enum / struct の方針と pseudo-type（第5節）。crate 分割そのものは決めない。
- 識別子・revision・generation・correlation の表現方針（第4節）。
- persistence 対象となる property（第7節）。DB schema 全体は決めない。
- transient / derived state の扱い（第7節）。
- serialization boundary の方針（第4・8節）。
- concurrency 上の照合単位＝「何と何を比較すれば現在として受け入れられるか」（第6節）。mechanism（lock / MVCC / channel / transaction protocol）は選ばない。

### 1.2 本書が決めないもの

- プロジェクト全体の crate 構成、module 分割、process / thread 配置。
- DB 製品の選定、完全な `CREATE TABLE`、index 設計、migration 方式。
- IPC 方式、wire format の byte 仕様、network protocol、heartbeat / lease の有無・値。
- queue / retry / timeout の値、scheduling algorithm、embedding / retrieval scoring、prompt 組立 algorithm。
- 各 Subsystem の完全な API 一覧。ただし「interface が落としてはならない情報」は第8節で固定する。
- 暗号方式、archive 形式、署名方式、marketplace / relay / account の有無（いずれも非目標のまま）。

### 1.3 用語

- **identity**: ある存在・要求・試行・操作を他と区別するための durable な識別子。意味の正しさを主張しない。
- **revision**: 同一 identity の意味内容が owner の判断で変わったことを追う、owner ごとの単調な順序。現在性の唯一の証拠にはならない。
- **generation**: 同一 lifecycle（帰属、復元、消去区間など）が切替・再成立したことを追う、lifecycle ごとの単調な順序。値の新旧ではなく「どの区間に属するか」を示す。
- **correlation**: 二つの identity / revision / generation の間の有向の対応（例: 結果→試行、派生物→source 範囲、提示→round）。graph 全体を一つの object にしない。
- **boundary token**: 照合の入力として渡す、期待する対応の写し（例: `expected_task_revision`）。authority ではなく比較材料である。
- **semantic owner**: SO で定めた意味・通常変更・lifecycle 判断の引受先。本書の record・ID はいずれも semantic owner にならない（第2節）。

## 2. 原則

### 2.1 Ownership を奪わない

本書の state 表現は、既存 owner の意味を**保持・照合するための実装上の表現**であり、意味そのものを移さない。以下をすべて維持する。

| 禁止 | 本書での守り方 |
|---|---|
| ID や correlation record 自体を semantic ownerにしない | ID は区別子、correlation は対応の記録に留める。内容の採否・達成・有効性の判断は各 owner が行う（第6節の述語は「受理可否の条件」であって判断の代替ではない）。 |
| provenance record を新 canonical knowledge にしない | 由来は参照・説明のための対応であり、現在知識の正本にしない。現在値は Memory / Task / Rule 等の owner が持つ。 |
| central context object を全 Subsystem の正本にしない | Context は用途限定の派生表現のまま。共通の `Context` object・store・lifecycle を新設しない。 |
| generic workflow state machine へ全 lifecycle を潰さない | Task / 試行 / 帰属 / 消去区間 / 復元は別 lifecycle のまま。共通 `Status` enum・遷移表を設けない。 |
| tracking record を Permission authority にしない | 判断記録・委任時の copy・cache された判定は生きた許可の正本にしない。現在の許可は権限・制約が domain 事実と照合して確定する。 |
| Action attempt record を Task success の authority にしない | 試行の把握された作用は実行・拡張、Task 目的の達成は作業。試行 record の存在・成功応答を受領したことと Task 達成を同一視しない。 |
| Client session identifier を presence authority にしない | presence の authority は Host 管理の個体別帰属記録のみ。Client 側の session / token / 接続主張は照合材料である。 |
| deletion tracking record を domain state の owner にしない | 消去の進捗・hold・再保存防止条件は調整のための記録であり、domain state の意味変更権ではない。意味変更は各 owner が行う。 |

### 2.2 過度の共通化をしない

以下へまとめない。実装上便利であることは共通化の理由にならない。

- Universal Context / Universal Operation / Universal Event / Universal State / Universal Correlation Graph。
- 万能 `ContextId` / `OperationId` / `CorrelationId` / `Status` / `Event` enum。
- 全 lifecycle 共通の revision counter、global version counter、全順序（total order）を前提とする照合。
- 全 domain 共通の `state store`、`repository`、`manager`、`coordinator` への集約。

共通化するのは第4節の**性質**（opaque 性、単調性、有向性、比較の形）だけであり、型・state・lifecycle は domain invariant が異なる限り分離する。

### 2.3 型方針

1. **同じ primitive representation を共有してよいが、semantic type は区別する。** 例えば内部が同じ UUID 互換表現でも `CompanionId` と `TaskId` は別型とし、相互の `From` / `Into` / 比較を設けない。generic ID enum を production path で使わない。
2. **newtype を既定とする。** 生の `String` / `u64` / `Uuid` を domain 境界を越えて渡さない。field 名で意味を区別するだけでなく型で区別する。
3. **revision / generation は生の数値として単独で持ち歩かない。** 必ず `(identity, revision)` または `(lifecycle, generation)` の組で扱う。revision だけ見て新旧・有効性を決めない。
4. **correlation は有向の pair として明示する。** 暗黙の時系列順・到着順・ID の大小を対応の根拠にしない。
5. **`unsafe` を本設計の表現のために要求しない。** 将来の実装で `unsafe` が必要になれば AGENTS.md に従い `// SAFETY:` を付すが、本書の型自体は safe Rust で表現できる範囲に留める。
6. **error 型は本書で固定しない。** 将来 library 化する際は `thiserror` を用い、bare `String` / `Box<dyn Error>` を public error にしない（repo 規約の再掲であり新規約束ではない）。

## 3. 必要な対応関係の体系化

既存 artifact から、実装上保持しなければならない対応を8領域に整理する。各行の owner は SO / Step 12 の確定事項の再掲であり、本書で変更しない。

### 3.1 Context の由来・用途・consumer との対応

- 要求の同一性は `(consumer, 用途, 担当 Companion, Task・委任関係, 目的・steering 前提, 元入力・出来事・管理操作, Client 依存なら取得 Client・round・候補, 活動継続関係, 期待する利用先)` の組で成立する。Companion ID や Task ID 単独では同一性にならない（CA §3.1）。
- 情報単位ごとに `(owner, 対象・source 関係, 位置付け・由来, revision・取得時点・有効期間, 所属・scope・共有関係, 用途・保存・共有・送信の制限, 参照の解決状態・欠落)` を保持または正本から解決できる必要がある（CA §3.2）。
- 説明文と強制情報を分ける。本文中の文字列だけで強制側が変わる構成は不可（CA §3.3）。
- 変換・圧縮は依存と制限を引き継ぐ。分離を確認できない混合出力は全入力に依存し得るものとして扱う（CA §4.3）。

### 3.2 Owner 意図 / Permission 判断 / 実対象との対応

- 認可判断の材料は、適用範囲で `(実行主体＋委任 chain、Task＋Workspace 範囲、目的、実対象＋操作種別＋送信先・作用想定、利用 data＋目的、費用・risk、依拠した Owner 意図・Rule)` である（AE §3.1）。
- LLM / Task Agent / Tool / MCP / shell / Computer Use / Plugin 等の生成 content は Permission を新設・拡大・自己承認できない（AE §3.2）。
- 判断記録と生きた許可は別の意味である。保存された Allow・委任時 copy・事前判定・復元 Rule・文脈内許可文・cache 判定を、取消・scope 変更・停止・帰属切替・cap・steering・消去・復元保留の再照合なしに現在許可として再利用しない（AE §3.3、SO §4.19）。

### 3.3 Task / delegation / Task Agent との対応

- Task は `(目的, 担当, 採用した追加指示, 進捗, 待機, 完了・失敗・Cancel, 結果, 未完了・次の判断)` を追う。Task context は `(採用した目的・指示・材料・途中理解, 由来・取得時点・目的・有効性)` である（SO §4.10）。
- 委任は `(誰から・どの範囲を・どの一時 Agent へ, 進捗・待機・停止・受領)` の対応である。Agent は永続 state の owner にならず、委任元の Capability・Permission・費用・Task・Workspace 境界を越えない（SO §4.11、AE §4.1）。
- 追加指示の発話 record（History）と Task への反映内容と未反映・待機は区別する。steering 前後で目的が変われば旧目的の結果を新目的の達成に自動採用しない（AE §7.2）。

### 3.4 Action の判断 / 試行 / 実作用 / outcome との対応

- 受付・開始・把握された作用・確定度・停止要求・停止結果を分ける。内部記録の保存成功≠外部作用の成功、外部作用の成功≠内部保存の成功（AE §6.1）。
- 確定度は少なくとも `確認済み成功 / 確認済み失敗 / 不明` を区別する。Cancel 受付済み≠停止済み、通信成功≠作用成功、UI 表示≠許可・完了（AE §6.2）。
- 重複し得る再送・retry は論理的な試行として不明・重複管理の対象にする。「同じ試行の継続」として除外する方式は採らない（AE §6.2）。
- 遅延到着は元の Action / Task へ記録し、現在の Task の達成・次の Action・提示とは別の受入として扱う（AE §7.3）。

### 3.5 Companion / Client / presence / round との対応

- authoritative presence は Host 管理の個体別帰属記録のみである。Client 表示・過去 active・hint・入力中 copy・再接続旧 state・復旧先・Provider 残存は根拠にならない（CPT、CCT、CNSO §4.15）。
- `presence（どこ） / Host 継続（存続） / 接続（到達） / 許可（可否）` を分離する。pairing 済み・接続済み・active・Action 許可済みは四つの異なる意味である（SO §4.15）。
- 切替区間は `旧 presence / 移行中 / 新 presence / active なし / 停止中 / 復旧待ち` を区別し、boolean や単一 active field へ潰さない。移行中は新旧いずれでも Client 依存の新規開始をしない（CPT、CNSO CN-4）。
- round は `(Companion, Client, round, 候補・試行)` の対応であり、移動・切断・再起動で旧 round の入力・未提示出力を新 round へ付け替えない（CPT、CNSO IO-1/IO-6）。

### 3.6 Targeted Deletion の対象 / 由来 / 依存 / 区間との対応

- 対象記述は二層である。機械的に必須の層（LLM 非依存の文字列検索・削除・残存検証）と、意味的な補助の層（言換え特定、完全性を保証しない）を分ける（DP、PE-1）。
- 全 holder（情報 owner だけでなく処理中 context・派生物・cache・Client / 拡張一時 copy・返却可能結果の保持者を含む）が `(source 関係, 処理中利用, 局所扱い・検証, 再保存防止, 未確認・未完了)` を保全・消去へ説明する（CA §8.2、PE-1）。
- 区間内に再到着・再生成した対象情報も同じ消去対象である。既知 source の追跡だけでは足りず、各受入箇所が飛行中の消去条件を適用する（CA §8.2、PE-1）。
- source を消した後に依存関係も消失し遅延結果を識別できなくなる実装は不可。本文を保持せず対応を維持できる必要がある（CA §8.2）。
- 局所完了≠全域完了。未確認・到達不能を成功と読まず、局所返却だけで hold を解除しない（PE-1、PE-7）。

### 3.7 Backup / Restore での復元 state と現在環境との対応

- Restore は merge ではなく、開始前 Credential secret を除く対象内部 data の対応 backup 時点への全置換である。成立前は復元前正常が正本、成立後は復元内容が正本であり、第三の混合状態を作らない（BR）。
- 復元された assignment / consent だけで自動利用を開始しない。現 Credential・現制約・復元後保留条件を満たす必要がある（BR、SO §4.12 原則12）。
- 旧 live 活動の結果を復元後の正本へ混入しない。由来別に用途受入する（BR §5）。
- stale な Permission / Provider / Client / 作用結果 / 外部参照を現在化しない。作用不明は不明のまま維持し、replay しない（BR §4）。
- Audit は backup 時点置換＋成立後の Restore 事実追記であり、順序の発生順化・許可正本化・再生・自動実行入力化をしない（BR、PE-6）。

### 3.8 delayed / stale result の元活動への帰属対応

- 結果は、少なくとも実際に入力した範囲に依存し得るものとして扱う。逐語の不在だけで消去・非共有・保存禁止の対応を外さない（CA §7.3）。
- 受入 owner は用途別である。会話・送信→個体調整、Task 判断→作業、Learning / Summary→認識・学習、Permission / Rule 解釈→権限・制約、次 Action 候補→起案者＋実行・拡張＋権限・制約、作用・利用量事実→元の owner（CA §7.3）。
- Cancel・失効・steering・移動・削除・復元後の到着物は、元の Action / Task へ記録し、旧承認の解除・旧結果の新目的採用・後続の自動開始をしない（AE §7.3、CA §7.3）。
- 参照時に正しかったことと今の利用に適することを分ける。固定 TTL だけでは scope 失効・Stop・消去を処理できない（CA §7.2）。

## 4. 共通に持つ識別・correlation concept

本節は性質だけを共通化する。型・lifecycle・counter は共有しない。

### 4.1 Identity（ durable 識別子）

- **性質。** opaque、衝突しない程度に一意、再利用しない、内容から推測しない、意味の正しさを主張しない。削除後に同じ値を再発行しない。
- **表現方針。** Rust では各 domain の newtype の内部表現として共通の primitive（例: UUID 互換 128bit、または同等の opaque byte 列）を用いてよい。文字列化するときは型ごとに接頭辞を付けず、field 名と型で区別する。接頭辞による判別（例: `"task_"` を見て分岐）を照合 logic に使わない。log・Audit・Debug では型名と値を組で記録する。
- **生成。** 生成主体は各 lifecycle の owner または coordinator が定める。生成に LLM・外部 content・Client 主張を用いない。衝突時の再生成は新規 identity として扱い、旧 identity の上書き・再利用にしない。
- **共有しないもの。** 採番主体、発行タイミング、寿命、削除時の扱い。例えば Companion identity の発行（個体調整）と Action 試行の発行（実行・拡張）は別の生成器・別の系列であり、一つの sequence を共有しない。

### 4.2 Revision（owner ごとの内容順序）

- **性質。** 同一 identity に対する、意味 owner の判断による変更順序。単調増加。値の大小は同じ `(identity, owner)` の範囲でのみ意味を持つ。異なる identity 間・異なる owner 間で比較しない。
- **表現方針。** Rust では `Revision(u64)` のような共通の内部形を持ってもよいが、単独で持ち歩かず必ず `(Id, Revision)` の組で扱う。content digest（hash 等）は revision の代替ではなく検証用の付随情報として任意に添えられる。digest の一致を同一意味の証拠にしないし、不一致だけで無効ともしない。owner の確定が優先する。
- **時刻との関係。** wall-clock・取得時点・timezone は revision の代替にならない。時刻は説明・表示・Schedule 計算のために保持し、現在性の判定は第6節の述語で行う。共通時計・全順序・固定 TTL を要求しない（CA §7.2）。

### 4.3 Generation（lifecycle ごとの区間順序）

- **性質。** 同一 lifecycle（presence 帰属、復元、消去区間など）の切替・再成立の順序。単調増加。値の新旧ではなく「どの区間に属するか」を示す。異なる lifecycle 間で比較しない。
- **表現方針。** Rust では `Generation(u64)` の共通内部形を用いてよいが、型と field 名で lifecycle を区別する（例: `presence_generation` と `restore_generation` は別型・別 field）。初期値は lifecycle ごとに定め、0 予約等の全域約束は設けない。
- **global version counter を設けない。** 全 state 共通の単調 counter、全 state 共通の snapshot id、全利用を束ねる transaction id を前提にしない。多 owner 情報に共通 snapshot を要求しない（CA §7.2）。

### 4.4 Correlation（有向の対応）

- **性質。** `from → to` の有向 pair。少なくとも `(from_id, from_rev_or_gen, to_id, to_rev_or_gen, 目的・用途)` を含む。graph 全体を永続 object にしない。必要な対応だけを保持し、辿れればよい。
- **表現方針。** Rust では各 domain の correlation struct を別に定義する。共通の `Correlation { from: String, to: String }` を production path で使わない。correlation 自体に採否・達成・許可の意味を持たせない。
- **本文非複製。** 対応の伝達に private 本文を複製しない。識別用の値であっても対象情報を復元できるなら保護・消去対象であり、由来保存の理由で削除本文や Credential を別保管しない（CA §3.2）。

### 4.5 Boundary token（期待する対応の写し）

- **性質。** 照合の入力として渡す「期待する相手」の写し（例: 判断時に見た `expected_task_revision`、送信前に解決した `expected_presence_generation`）。authority ではなく比較材料である。不一致なら新規利用を開始せず、hold・deny・不足・再評価へ戻す。
- **表現方針。** 各 interface は必要な boundary token を明示 field として受け渡す。暗黙の「最新を使う」指定を設けない。token の欠落は「制約なし」ではなく「照合不能＝不受理」の理由になる。

### 4.6 Serialization boundary

- Host 永続化・Host↔Client 間の受渡し・Audit / Debug 記録では、ID・revision・generation・correlation を明示 field として serialize する。自由記述の本文中に埋め込んだ文字列を照合に使わない（CA §3.3）。
- Client・Provider・MCP・Plugin との境界では、Host の内部 ID 体系をそのまま露出する必要はない。露出する場合は用途限定の参照（non-secret reference）に留め、内部正本の主 key として再利用できる形で渡さない。認証秘密値は通常経路に載せない（DR-05）。
- 時刻は ISO 的な wall-clock 表現＋作成時 timezone を保持する。Schedule の timezone 黙置換をしない（BR §4）。

## 5. Domain 固有の型・state 表現

本節の型名・field 名は提案である。後続 interface 設計で同義の改名は許すが、型の分離と field の意味は維持すること。pseudo-type は Rust のつもりで読むが、コンパイル対象の code ではない。

共通の禁止を再掲する。`CompanionId` と `TaskId` の内部表現が同じでも相互変換しない。`AttemptId` から Task 達成を導かない。`PresenceGeneration` から presence を創設しない。`DeletionOperationId` から domain state を変更しない。`RestoreGeneration` から権限を復活させない。

### 5.1 個体・Character・適用関係

```rust
struct CompanionId(/* opaque */);
struct CharacterId(/* opaque */);
struct CharacterRevision(/* opaque revision marker */);
struct CharacterPart(/* e.g. persona/body/voice/skill-ref, enum */);

/// 個体調整が持つ適用関係の正本に対応する表現。
/// 内容の正本は Character、適用関係の正本は個体調整。
struct CompanionCharacterApplication {
    companion: CompanionId,
    character: CharacterId,
    revision: CharacterRevision,
    parts: Vec<AppliedPart>,
    selected_by_owner_at: WallClockWithTz,
}

/// 部品ごとの適用。revision 有無の推測・未確認部品の更新済み扱いをしない。
struct AppliedPart {
    part: CharacterPart,
    applied_revision: CharacterRevision,
    owner_selection: OwnerSelectionRef,
}
struct OwnerSelectionRef { /* 管理操作との対応。時刻＋操作 identity を含む */ }
```

- revision の存在≠適用・適用すべきという推測をしない。供給は確定済み revision のみから行う（CH）。
- Package 更新を成長の初期化にしない。適用禁止種别（Credential / Permission / 同意 / authority / Plugin・MCP authority / Global Skill / system control / Rule / cap を含む revision 内容）は適用対象から除外する（CH）。

### 5.2 会話・round・未伝達

```rust
struct RoundId(/* opaque */);
struct ClientId(/* opaque */);

/// 入出力・提示が持つ round の実際に対応する表現。
struct ConversationRound {
    companion: CompanionId,
    client: ClientId,
    round: RoundId,
    presence_generation: PresenceGeneration,
    // 旧 round の入力・未提示出力を新 round へ付け替えない。
}

/// 個体調整が持つ未伝達の管理に対応する表現。
/// Task 由来→ Task record、対話・交流由来→活動 record が正本であり、
/// 本 struct は対応と報告状況の照合用である。第二の Task 正本にしない。
struct UndeliveredRef {
    companion: CompanionId,
    source: UndeliveredSource, // TaskRecord(TaskId) | ActivityRecord(ActivityId)
    summary_report_status: ReportStatus,
}
enum ReportStatus { Pending, SummarizedForNextClient, Presented, PresentationUnknown }
```

- 生成≠提示、送信≠報告完了、報告済み≠承認・再開（CC-07）。
- round 終了≠timeline 終了・削除。Provider session・表示 copy を第二正本にしない。

### 5.3 Task・委任・Task context・Workspace・Schedule

```rust
struct TaskId(/* opaque */);
struct TaskRevision(u64); // 同一 TaskId に対する作業の判断順序。目的変更を含む。
struct DelegationId(/* opaque */);
struct TaskAgentEphemeralId(/* opaque; 一時的。永続 state の owner にならない */);
struct WorkspaceAssocId(/* opaque */);
struct ScheduleId(/* opaque */);
struct ScheduleOccurrenceId(/* opaque */);

struct TaskRef {
    task: TaskId,
    revision: TaskRevision,
}

struct DelegationRef {
    delegation: DelegationId,
    task: TaskRef,               // 委任元 Task とその revision 前提
    delegator: CompanionId,      // 委任元 Companion
    agent: TaskAgentEphemeralId, // 一時主体。長期人格・Relationship を持たない
    scope: DelegationScope,      // Task・Workspace 境界の写し
}

struct TaskContextRef {
    task: TaskRef,
    // 採用した目的・指示・材料・途中理解の identity 群。
    // 内容の正本は Task record。本文複製を要求しない。
}

struct ScheduleOccurrenceRef {
    schedule: ScheduleId,
    occurrence: ScheduleOccurrenceId,
    // 各回は新 Task。missed を実行済みに書き換えない。
    started_task: Option<TaskId>,
    occurrence_status: OccurrenceStatus,
}
enum OccurrenceStatus { Missed, Started, CancelledAsTaskContract }
```

- Task record の lifetime を担当 Companion への参照が決めない。担当削除後も残 Task record は管理面から到達できる（W-7、CNSO X-10）。
- Task 化閾値・分類 algorithm は本書で固定しない。軽微 Action をすべて Task 化しないし、まとまった作業の Task 化を省かない（AE §4.3）。

### 5.4 Action・試行・作用・確定度

```rust
struct ActionAttemptId(/* opaque */);
struct EffectCertainty { /* 確認済み成功 / 確認済み失敗 / 不明＋根拠対応 */ }
enum ActionCertainty { ConfirmedSuccess, ConfirmedFailure, Unknown }

/// 実行・拡張が持つ把握された作用に対応する表現。
struct ActionAttemptRef {
    attempt: ActionAttemptId,
    task: Option<TaskRef>,          // 軽微な非 Task Action では None。個体調整の活動 record と対応。
    delegation: Option<DelegationId>,
    workspace: Option<WorkspaceAssocId>,
    real_target: RealTargetRef,     // 解決後の実対象。対象記述の文字列一致だけでは対応にしない。
    operation: OperationKind,       // Read | Create | Edit | Delete | Execute を潰さない
    relied_permission: PermissionEvaluationRef, // 依拠した判断の対応（生きた許可ではない）
}

/// 判断記録と生きた許可を区別するための、判断時の対応の写し。
struct PermissionEvaluationRef {
    evaluation: PermissionEvaluationId,
    // 何についてどう判断したかの対応。Audit・説明・対応付けの材料。
}
struct PermissionEvaluationId(/* opaque */);
```

- 受付・開始・作用・確定度・Task 達成・制御変更・未伝達・保存・Audit は別の事実・別の owner である（AE §6.1）。
- 外部 `成功` 応答は、対象・作用の対応と由来・scope を保った上で evidence として扱う。Agent 自己申告を証拠にしない。
- retry・再送は新しい `ActionAttemptId` で試行する。重複し得る再送を「同じ試行の継続」として不明・重複管理から除外しない。

### 5.5 権限・制約・Credential・推論割当・利用量

```rust
struct RuleId(/* opaque */);
struct RuleRevision(u64);
struct AssignmentConsentId(/* opaque */);
struct DevicePermissionRef { /* per-device 制御状態の対応 */ }
struct SandboxExceptionRef { /* 特定 Local MCP の隔離外例外の対応。Plugin へ流用しない */ }
struct UsageAttributionRef {
    consumer: UsageConsumer, // CompanionReasoning(CompanionId) | ObserverDedicated | ...
    // 報告値 / 推定 / 不明 / 処理中を区別する。未報告・処理中・不明をゼロにしない。
}

/// 現在の許可の確定は権限・制約が行う。本 struct は照合材料である。
struct CurrentPermissionBoundary {
    permission_evaluation: PermissionEvaluationRef,
    rule: Option<(RuleId, RuleRevision)>,
    assignment_consent: Option<AssignmentConsentId>,
    device: Option<DevicePermissionRef>,
    // 保存された Allow・委任時 copy・事前判定・復元 Rule・文脈内許可文・cache 判定は
    // ここに入れても生きた許可にならない。再照合が必要。
}
```

- 明確な現在依頼は一回限りの承認として解釈できるが、永続 Deny・Always ask・Capability 境界を黙って上書きしない（CC-01）。
- Credential 値は本節のいずれの struct にも入れない。用途参照は非秘密の参照に留める（DR-05）。model 出力からの自動登録・変更をしない。

### 5.6 presence・接続・Client 主張

```rust
struct PresenceGeneration(u64); // 個体ごとの帰属 lifecycle 順序。authority は Host 記録。

/// 接続・存在が持つ個体別帰属記録に対応する表現。唯一の authority。
struct PresenceAttribution {
    companion: CompanionId,
    state: PresenceState,
    active_client: Option<ClientId>,
    generation: PresenceGeneration,
}
enum PresenceState {
    Present,        // active_client == Some
    NoActive,       // Running だが active なし。正当な状態。
    InTransition,   // 移行中。新旧いずれも新規開始しない。
    Stopped,        // presence なし。hint・保存 data と別。
    RecoveryWait,   // 再起動復旧待ち。active なし。
}

/// Client 側の一時的な主張。presence の authority ではない。
struct ClientPresenceClaim {
    companion: CompanionId,
    client: ClientId,
    claimed_generation: PresenceGeneration,
    round: Option<RoundId>,
    candidate_or_trial: Option<ObservationCandidateOrTrialRef>,
}

/// 再配置 hint・再起動前復旧先。記録だけで presence にならない。
struct RelocationHint {
    companion: CompanionId,
    last_client: ClientId,
    recovery_destination: Option<ClientId>,
}
```

- `PresenceGeneration` の一致だけでは不十分であり、現接続・機能可用性・権限・制約・停止・保留も第6節の述語で照合する。確認不能を現在と推定しない。
- hint・復旧先の保持は接続・存在が行うが、現在性の確認なしに復活させない（SO §4.15）。

### 5.7 Observer の限定 routing 文脈

```rust
struct ObservationCandidateId(/* opaque */);
struct ObservationCandidateOrTrialRef { /* candidate・試行の対応 */ }

/// 共有観測が保持する派生表現に対応するもの。
/// 新正本・新 scope・包括共有ではない。
struct RoutingContextRef {
    source_companion: Option<CompanionId>, // 起源。History=個体調整、Memory=認識・学習等
    target_companion: CompanionId,
    purpose: RoutingPurpose,
    constraint: RoutingConstraintRef, // source 制約＋専用 assignment 送信条件
    selection_premise: SelectionPremiseRef, // 選択時点の前提。話題・Task 変化を判定できる粒度。
}
```

- Task store の恣意的読取をしない。Task context は既存の個体調整–作業 coordination 経由でのみ受け取る（SO §4.16）。
- Observer 専用 assignment と個体設定を合成しない。routing permission は候補→個体の関連付けだけを成立させる（CA §6.2）。

### 5.8 Learning・Summary・根拠・scope

```rust
struct LearningId(/* opaque */); // Memory / Skill / Relationship / State の各内容 identity
struct LearningRevision(u64);    // owner（認識・学習）の判断順序
struct SummaryId(/* opaque */);
struct SourceRangeRef { /* 元 Conversation・Task 等の大まかな範囲。逐語引用は保持 History */ }

/// Summary→形成・改訂利用根拠の対応。Summary を現在知識の正本にしない。
struct SummaryGroundsRef {
    summary: SummaryId,
    used_grounds: Vec<GroundsRef>,
    formed_or_revised: Vec<(LearningId, LearningRevision)>,
}
struct GroundsRef {
    source_range: SourceRangeRef,
    // 根拠の本文複製を要求しない。参照で辿れること。
}
```

- Experience は出来事と結果の概念であり、全活動を格納する新 canonical Raw store を設けない（SO §3）。
- Global 化は内容＋背景からの判断であり、私的 revision・History・Summary 全文への拡大ではない（CC-02、L-7）。
- 誤っていた場合の訂正と、後から状況が変化した場合を区別し、後者では過去の時間的有効性を失わない（requirements）。

### 5.9 消去・backup・復元・保持

```rust
struct DeletionOperationId(/* opaque */);
struct DeletionSweepGeneration(u64); // 消去区間の順序。interval の識別用。

/// 保全・消去が持つ全域操作の調整に対応する表現。domain state の owner ではない。
struct DeletionOperationRef {
    operation: DeletionOperationId,
    sweep: DeletionSweepGeneration,
    purpose_target_impact_exclusion: OperationScopeRef,
    participant_completions: Vec<ParticipantCompletionRef>,
    hold_conditions: Vec<HoldConditionRef>,
}

/// 消去条件。各 holder・各受入箇所が飛行中の利用・再保存に適用する。
struct ErasureConditionRef {
    operation: DeletionOperationId,
    sweep: DeletionSweepGeneration,
    valid_interval: ErasureInterval, // 開始〜検証完了。区間内再到着を含む。
}

struct BackupPointId(/* opaque */);
struct RestoreGeneration(u64); // 成功した Restore ごとの順序。旧 live と復元の区別用。

struct RestoreRef {
    generation: RestoreGeneration,
    backup_point: BackupPointId,
    post_restore_hold: HoldConditionRef,
}

struct HoldConditionRef { /* 失効 / cap / 復元後保留 / 消去 hold 等の対応 */ }
struct OperationScopeRef { /* 目的・範囲・影響・除外・要確認の対応 */ }
struct ParticipantCompletionRef { /* 参加者ごとの処理・検証・未完了・失敗・未確認範囲 */ }
```

- `DeletionOperationRef` の完了は保全・消去が全域として確定するが、各 domain の意味変更は各 owner が行う。coordinator は任意の通常変更権を取得しない（DR-09）。
- 旧 backup の明示 Restore による復活は、自動再形成の例外ではなく別操作として事前説明・Audit・保留・再評価を経る（PE-3、BR §8）。

## 6. 現在性・照合単位（concurrency の比較内容）

mechanism を選ぶ前に、「何と何を比較すれば現在として受け入れられるか」を述語として固定する。実装 mechanism（lock / MVCC / channel / queue / transaction protocol）は architecture から導くものとし、本書では選ばない。global version counter を前提にしない。

### 6.1 一般形

新規の利用・採用・実行は、少なくとも次の比較を満たすときに限り受け入れる。満たさなければ hold・deny・不足・再評価へ戻し、古い判定・解決済み経路だけで開始しない。

1. 期待した相手の identity が一致すること。
2. 期待した revision / generation が現在の owner / lifecycle の revision / generation と対応すること（等価または許された継承・再評価済みであること）。
3. 依拠した許可・同意・device・費用・停止・保留・消去・復元条件が現在の条件と対応すること。
4. Client 依存なら現在の帰属 generation と現接続・機能可用性が対応すること。
5. 遅延物なら元の試行・区間・世代との対応が保たれ、現在の目的への自動採用にならないこと。

「最新の値を読んだ」こと、「cache に hit した」こと、「到着順で最後」であることは、いずれも単独では受入根拠にならない。

### 6.2 用途別の照合

| 利用・受入 | 比較するもの | 不一致時の扱い |
|---|---|---|
| Context の参照・選択 | 要求の `(consumer, 用途, Task・委任, 目的・steering 前提, 活動継続関係)` × 各情報の `(owner, source 範囲, revision, scope, 制限)` × 現在の制約 | 用途不成立として不足へ戻す。到着先の現在活動へ付け替えない。 |
| Provider への実送信（初回・fallback・再送・補助推論・継続部分の各々） | 論理選択の範囲 × 解決済み consumer / Capability assignment と実送信先・data・用途・取扱い・費用の同意 × 現在の認証用途・制限・保留・利用量 × 元要求・範囲との対応 | 送信しない。同意不足を本文削減で黙って解消しない。 |
| Action の新規開始 | `ActionAttemptRef` の `(Task revision 前提, 委任 scope, Workspace 範囲, 実対象・操作種別, 依拠 Permission)` × 現在の `(Task revision, 委任有効性, Workspace 有効性, 実対象解決, 現在許可・device・費用・停止・保留・消去・復元条件)` × Client 依存なら `PresenceGeneration` と現接続・可用性 | 開始しない。旧判定・解決済み経路だけで開始しない。Owner 確認待ちは実行せず待機にする。 |
| 重要変化の再評価 | 対象 identity・操作種別・作用拡大、目的・steering 変化、Task・委任変化、Workspace・scope 変化、帰属・対象変化、依拠 Rule・同意・device・Credential 変化、費用・risk の重要変化、停止・保留発生の有無 | 該当すれば再評価。無関係な locale・Memory 更新・Task 進捗・Body・Voice staging を再評価理由にしない。 |
| Task 達成の受入 | 到着結果の `(attempt, Task revision 前提, 目的)` × 現在の `(Task revision, 目的・steering 前提)` | 旧目的の結果を新目的の達成に自動採用しない。元 Action / Task への記録と現在 Task への採用を分ける。 |
| 結果の Learning / Summary 化 | 到着結果の `(source 範囲, 取得時点, 依拠 revision)` × 現在の `(Memory revision, scope, 制約)` | 遅延形成が現在を無条件上書きしない。到着順を根拠の新旧にしない。 |
| Permission / Rule 解釈の採用 | モデル `許可`・引用・過去 Allow・復元 Rule・文脈内許可文・cache 判定 × 現在の `(Rule revision, 同意, device, cap, 失効・停止・帰属・消去・復元保留)` | 制御を変更しない。将来 Rule は解釈結果・適用範囲の表示・保存・Undo を経る。 |
| Client 依存活動の開始・継続 | `ClientPresenceClaim.claimed_generation` × `PresenceAttribution.generation` × 現接続・機能可用性 × 現在許可・停止・保留 | 確認不能なら継続しない。旧一時 state・旧承認・判定 copy・解決済み経路だけで成立させない。 |
| 切替区間の活動 | 旧 presence / 移行中 / 新 presence / active なし / 停止中 / 復旧待ちの区別 × 新旧いずれの帰属か | 移行中は新旧いずれも Client 依存の新規開始をしない。旧 in-flight は安全な区切りまで、旧作用の別 Client 自動継続をしない。 |
| 消去区間の受入・生成・再保存 | 到着・生成・処理中情報の `(source 関係, 取得・生成時点, sweep 以前・区間内の別)` × `ErasureConditionRef.valid_interval` × 保持者の局所検証 | 区間内再到着・再生成は消去対象。実行中処理による再保存をしない。完了していないのに完了と表示しない。 |
| 復元後の利用 | 利用の `(restore generation 前提, assignment / consent revision, Credential 照合, 依拠 Rule revision)` × 現在の `(RestoreGeneration, 現 Credential store, 現制約, 復元後保留)` | 旧 live・旧同意・旧 assignment だけで自動利用・自動処理を開始しない。一括有効化後も現在条件を守る。 |
| 費用・資源の継続判断 | 消費の `(用途・送信先の対応, 報告・推定・不明・処理中の別)` × 現在の cap・資源・不明の扱い | 処理中・遅延・不明をゼロにしない。並列要求で同一残額を独立に使い切れる扱いにしない。cap・不明で安全継続不可なら data 保持のまま停止・判断待ちにする。 |

### 6.3 試行・重複・不明の扱い

- retry・再送・fallback・再委任は新しい `ActionAttemptId`（または実行・拡張が定める試行系列の次要素）で試行する。重複し得る再送を「同じ試行の継続」として除外しない。
- 不明は粘着させる。Cancel 受付・通信成功・表示・保存成功・再接続・復元・移動で不明を未実行・成功へ書き換えない。新しい evidence が事実 owner に確認されたときに限り確定度を更新する。
- 遅延結果の帰属は `attempt → task revision → 現在 Task` の順に辿る。記録（元へ残す）と semantic 更新・次実行・提示（現在の受入）を分ける。

### 6.4 世代タグの付与規則

- presence 依存の round・試行・候補・routing 派生物には `PresenceGeneration` を添える。世代が変われば旧世代の材料だけで新規利用を開始しない。
- 復元を跨ぐ参照（Task・Rule・同意・assignment・作用・未伝達・全域未完了）には `RestoreGeneration` を添える。旧世代の参照だけで復元後に利用・実行・送信しない。
- 消去区間を跨ぐ保持・処理・cache・Client 一時 copy・返却可能結果には `DeletionOperationId`＋`DeletionSweepGeneration` の消去条件を添える。区間内再到着・遅延再保存・旧根拠からの自動再形成を防止する。
- revision と generation は混ぜない。Task の目的変更は `TaskRevision` の前進であり、`RestoreGeneration` や `PresenceGeneration` の変化で代替しない。

## 7. Persistence（restart を跨ぐものと跨がないもの）

DB schema 全体は固定しない。固定するのは「どの対応が restart 後も必要か」と「そのために保持すべき persistent property」である。保存方式・table 分割・repository 分割・writer・transaction は後続設計に委ねる。物理保存を共有しても semantic owner を一つへ統合しない（SO）。

### 7.1 Persisted でなければならない対応

Host 正本として restart 後も必要なもの。いずれも意味の owner は SO のまま。本節は対応を残すための property である。

| 対応 | 保持すべき persistent property | 備考 |
|---|---|---|
| 個体・適用関係 | `(CompanionId, CharacterId, CharacterRevision, AppliedPart 群, OwnerSelectionRef)` | 途中編集・import・export・適用供給の中断で最終正常と旧適用関係を保つ。部分適用・未確認編集を正本にしない。 |
| 会話・活動 record・未伝達 | History 原 record（参加者・文脈）、保存された非会話 record・evidence、進行中意味判断、`UndeliveredRef`＋報告状況 | Client 入力途中・表示 timeline・audio buffer は失ってよい。受理済み指示・必要作業 record・未伝達は失わない。 |
| Task・委任・context・Workspace・Schedule | `(TaskId, TaskRevision, 目的・担当・進捗・待機・結果・未完了・次の判断)`、`(DelegationId, TaskRef, 委任元・範囲・進捗・待機・停止・受領)`、`TaskContextRef`（由来・取得時点・目的・有効性）、Workspace 関連付け・保存先・待機、Schedule 設定・timezone・初期入力・回到達対応・各回 Task | 途中 Task は保存済み進捗・既知作用・不明・未完了を示し明示再開待ちにする。Agent 終了・担当削除で Task record を消さない。 |
| Action の把握された作用・不明・停止結果 | `(ActionAttemptId, Task・委任対応, 実対象・操作種別, 確定度, 根拠対応, hold)` | buffer 寿命を越えて保持する。不明を未実行に戻さない。 |
| Learning・Summary・根拠・scope | Summary＋`SummaryGroundsRef`、Memory 現在・重要度・scope・過去 revision・履歴、Skill 有効 revision・過去 revision・原本対応・実行結果対応、Relationship 現在・保持過去・根拠、Companion State（継続に要る状態・一時の時間意味・保持根拠） | 派生物（embedding / index / query 派生 / cache / session / 経路 / 表示集計）は独立復元対象にしない。古派生から権限・状態を復活させない。 |
| 権限・制約・同意・cap・利用量 | Rule 本文・解釈・scope・`RuleRevision`・Undo 対応、Permission 判断記録（生きた許可ではない）、assignment 同意・fallback 順序・device・禁止・非共有・cap、利用量（報告・推定・不明・処理中の別） | 過去 Allow・委任時 copy・事前判定・復元 Rule・文脈内許可文・cache 判定を現在許可として復活させない。cap 用の利用量事実は cache clear・log 整理・Agent 終了・移動で reset しない。 |
| Credential | 現在 store（値・用途・参照・有効性・登録・更新・失効） | backup に secret を含めない。Restore で巻き戻さない。現在 store を維持し旧 context 参照だけで認証・再実行を示唆しない。 |
| 接続・帰属・hint・復旧先 | 個体別帰属 record・現接続・最終接続管理 record、`PresenceGeneration`、hint・復旧先（非現在の参照として） | 現在接続を古い保存値から再成立させない。Running presence は現接続・許可・排他の確認ができれば復元前 Client へ自動復元し、できなければ active なしにする。Stopped に移動・復旧しない。 |
| 観測・自発性・一般設定 | Owner 観測・自発性・一般設定、Host 自動起動選択（日常利用の意味） | OS への適用効果・全域操作参加は別 owner。 |
| 消去・保持・backup・復元・Audit・Debug | 消去要求の目的・対象・影響・除外・要確認対応、参加者ごとの処理・検証・未完了・失敗・未確認範囲、消去条件・有効区間・完了境界、局所・全域完了・保留・到達不能・検証失敗の別、backup 時点・参照対応・除外・未完了状況・結果・失敗、Restore 受理・説明・隔離・照合・置換・保留・一括有効化対応、Reset 列挙・強確認・trust 失効、保持・明示 opt-in 範囲・影響、Audit 追記順・保持、Debug 対象・内容・期限・停止・削除 | 処理中の restart を完了と誤らず hold を黙って解除しない。完了根拠に target 本文・秘密・private 本文を残さない。 |
| Provider 登録・能力・割当解決 | 非秘密の登録・能力観測・解決対応、Observer 専用 assignment | 解決結果を独立した利用可能 assignment の正本にしない。 |

### 7.2 Process 内一時状態でよいもの

- Client 入力途中・表示 timeline・描画 frame・motion 位置・audio buffer・VAD・barge-in・現在 Mute。
- 観測 Raw・候補・routing 用一時 data・推論 session・cache・補助 data・期限切れ派生物。
- Provider session 残存・Client 直接送信の一時 copy・MCP Apps 表示・操作 data。
- I-7 の進行中意味判断・W-2 の Agent 一時 context・L の一時 state buffer 的側面。
- 未確定の編集・未提示の表示 copy・再取得可能な表示 copy。

ただし「一時的だから失ってよい」のは上記に限る。受理済み指示・必要作業 record・未伝達・作用不明・全域未完了は一時的扱いにしない。

### 7.3 Restart を跨ぐ必要があるもののまとめ

- 未知の Action outcome（確定度 `Unknown` と根拠対応・hold）。
- 遅延結果の帰属に要る `(attempt, Task revision 前提, source 範囲, 世代)` の対応。本文複製は要らない。
- targeted deletion 中の再保存防止に要る消去条件・有効区間・完了境界・hold。
- restore 途中の旧 live 分離に要る `RestoreGeneration` と復元操作自体の受理・説明・保留対応（成立まで旧正常を破壊しない）。
- Client stale 識別に要る `PresenceGeneration` と帰属・接続・hint・復旧先の対応。
- 未伝達結果（`UndeliveredRef`＋報告状況）と Task・委任の継続に要る Task・context・Workspace・Schedule の対応。
- 再構築可能性は全文 context・内部推論の永続保存を要求しない。必要な正本から再構築し、保存された派生物は材料として使っても旧要求の許可・実行を直接 resume しない（CA §8.1）。

### 7.4 Stale / delayed 判定に必要なもの

- 各情報の `(owner, revision・取得時点・有効期間, scope, 制限)` と要求の `(用途, 目的・steering 前提, 活動継続関係)`。
- 各試行の `(attempt, Task revision 前提, 委任 scope, 実対象・操作種別, 依拠 Permission)` と現在の同型の組。
- 各帰属の `(generation, 現接続・可用性, 許可・停止・保留)`。
- 各消去の `(operation, sweep, 有効区間)` と各保持の `(source 関係, 取得・生成時点)`。
- 各復元の `(generation, backup 時点, 復元後保留)` と各利用の `(前提 generation, 同意 revision, Credential 照合)`。
- いずれも単独の時刻・TTL・到着順・ID 大小で判定しない。矛盾する対応（例: steering 後目的＋旧委任）は不採用・再評価にする。一貫性判断を context 専用 owner へ移さない。

### 7.5 全域 operation と対応付ける必要があるもの

- 全 holder・変換者・送信者・受信者が scope を保全・消去へ説明できること（CA §8.2）。情報 owner だけでは足りない。
- backup の時点・参照・除外・保護の照合（BR §6）。外部 folder・source 実体を関連付け先辿りで収集しない。secret・外部実体を除外する。
- Restore の受理・説明・隔離・照合・置換成立・復元後保留・一括有効化（BR §7）。単一 boolean・flag へ潰さない。
- Reset の列挙・強確認・trust 失効（PE-4）。設定 Reset と全 data Reset を混同しない。
- Audit 追記順・保持、Debug の明示対象・内容・短期失効・停止・削除（PE-6）。Audit・Debug を secret 置場・削除迂回・stale 正本・確定度強化の根拠にしない。

## 8. Interface が落としてはならない情報

各 interface の具体形は [Interface Boundaries](interface-boundaries.md) が定める。本節は受け渡す際に semantic information を落とさないための必須情報を固定する。

### 8.1 受け渡し一般の必須情報

- 要求の同一性に要る `(consumer, 用途, 担当 Companion, Task・委任関係, 目的・steering 前提, 元入力・出来事・管理操作, Client 依存なら取得 Client・round・候補, 活動継続関係, 期待する利用先)` のうち適用する範囲。欠落した対応は「制約なし」ではなく不受理の理由にする。
- 情報単位の `(owner, 対象・source 関係, 位置付け・由来, revision・取得時点・有効期間, 所属・scope, 用途・保存・共有・送信の制限, 参照解決・欠落の別)` のうち適用する範囲。制限の異なる部分を混ぜた単位を狭い利用先へ渡さない。
- 期待する相手の boundary token（`expected Task revision`、`expected presence generation`、`expected restore generation`、消去条件の写し等）。暗黙の最新指定を設けない。
- 不足・省略の理由。空・切詰めを「該当事実なし」「承認条件なし」へ変換しない。不足説明自体にも同じ利用範囲を適用する。

### 8.2 用途別の必須情報

| 受け渡し | 必須情報 |
|---|---|
| Context 参照・選択・変換 | §8.1 の要求・情報単位・token・不足理由。変換では依存元と制限の引継ぎ、分離できた範囲の限定。モデル自己申告だけでの依存外しをしない。 |
| Provider 実送信 | 論理選択範囲、解決済み consumer / Capability assignment、実送信先・data・用途・取扱い・費用の同意対応、認証用途・制限・保留・利用量、元要求・範囲の対応。費用の報告・推定・不明・処理中の別。 |
| Task 化・委任・steering・結果統合 | 意図・目的・steering 前提・Workspace 条件・Cancel 対象と進捗・結果・待機・反映不可理由・代替の対応。発話 record と Task 反映内容と未反映・待機の区別。 |
| Action 要求・試行・停止・結果 | `ActionAttemptRef`（試行・Task・委任・Workspace・実対象・操作種別・依拠 Permission）、`PermissionEvaluationRef`、実対象解決の前提、費用・停止・保留・消去・復元条件の写し、把握された作用・確定度・根拠対応。 |
| Permission 判断の依頼・回答 | 判断対象（主体・委任・Task・Workspace・目的・実対象・操作・送信先・data・作用・費用 risk）、依拠 Owner 意図・Rule の対応、重要変化の有無、現在の有効性の確定（権限・制約が行う）。判断記録と生きた許可の区別。 |
| presence・round・入出力 | `PresenceAttribution`（個体・状態・active・generation）、`ClientPresenceClaim`、`ConversationRound`、旧・新・移行中・active なし・停止・復旧待ちの区別。生成≠提示、送信≠報告の区別。 |
| Observer routing | `RoutingContextRef`（source・target・目的・制約・選択前提）、起源 Client・取得時点・候補対応。混合文の無条件配送をしない。対象 Companion の利用可能 content・背景だけを渡す。 |
| Learning・Summary・根拠 | `SummaryGroundsRef`、訂正と状況変化の別、過去の時間的有効性、scope・制約。旧 Summary を訂正後 Memory の代わりにしない。 |
| 消去・保持・backup・復元 | `DeletionOperationRef`・`ErasureConditionRef`（目的・範囲・影響・除外・要確認、参加者・未完了・検証・hold）、backup 時点・参照・除外・保護・結果、Restore 世代・保留・一括有効化対応、Audit 順序・保持、Debug 対象・内容・期限。完了根拠に本文・秘密を残さない。 |
| 利用量・費用 | 用途・送信先の対応、報告・推定・不明・処理中の別、cap・資源の現在条件。未報告・処理中・不明のゼロ化をしない。 |

### 8.3 禁止の再掲（interface 設計への制約）

- 本文中の文字列・モデル出力・引用・画面表示・案内・prompt 要約を承認・同意・scope・許可の evidence にしない。
- Voice に話者認証済みの意味を足さない。
- Provider session・round・表示 state・timeline・進捗要約・Agent 自己申告・notification memo・UI 表示を History・Task・進捗・作用の正本にしない。
- Client を domain state の唯一保持者にしない。Client に History・Summary・Learning・Relationship・Companion State・登録 Credential の永続 cache を持たせない。
- 未確認・到達不能・未検証を成功・承認・新 Experience・現在値にしない。
- 旧設定 copy・外部 code・Provider 残存・古派生・cache・session を同意・現在性の根拠にしない。

## 9. 検証

具体化した表現が既存 lifecycle を壊さず、必要な対応を失わないことを、少なくとも以下の walkthrough で確認する。いずれも「古い対応の自動採用・自動再実行・自動復活をしない」ことが合格条件である。

### 9.1 Context Assembly

- 用途限定の派生表現のまま新たな正本を作らないこと。要求同一性・情報単位・強制分離（CA §3）が §8.1 の必須情報で運べること。
- 不足・省略が結果へ伝わり、空・切詰めが事実なし・承認条件なしへ変換されないこと。
- 変換が依存・制限を引き継ぎ、分離不能な混合を全入力依存として扱うこと。

### 9.2 Action Execution

- **stale result。** Cancel・steering・失効後の到着物を元 Action / Task へ記録し、旧承認の解除・新目的採用・後続自動開始をしない（AE §7.3）。述語は第6節の Task 達成・結果採用の行で運べる。
- **retry。** 新 `ActionAttemptId` で試行し、重複 risk を Owner 判断に載せる。別経路・別表現での迂回をしない（AE §4.2）。
- **Cancel。** 受付と外部停止完了を分け、既作用・停止不能・不明・未保存を残して報告する。Cancel を LLM・Agent 正常終了待ちにしない（AE §7.1）。
- **restart。** 途中 Task は保存済み進捗・既知作用・不明・未完了を示し明示再開待ちにする。外部作用不明の自動再実行・Client 移動・接続回復を理由とする replay をしない（AE §7.1、SO §4.10）。
- **Task Agent delegation。** 委任元が失った条件を引き継がず、独立 Allow・Credential・Provider override・budget・長期人格・Relationship・Agent-scope Learning を持たせない（AE §4.1）。

### 9.3 Targeted Deletion

- **削除中再到着。** 区間内再到着・再生成を `ErasureConditionRef` で消去対象にし、各受入箇所が飛行中消去条件を適用すること。既知 source 追跡だけに頼らない（CA §8.2）。
- **遅延再保存。** 削除前情報を利用する実行中処理による再保存をせず、旧根拠だけの自動再形成をしないこと。完了していないのに完了と表示しないこと。
- **restart。** 未完了消去・hold・再保存防止を Host 保全し、再起動を完了・解除と誤らないこと。本文を保持せず対応を維持できること。
- **完了後。** 新規提供は新 Experience として許し、旧 backup の明示 Restore は別操作として事前説明・Audit・保留・再評価を経ること。cache・session・遅延物の意図しない再出現は再保存防止で抑止すること（PE-3、BR §8）。

### 9.4 Client Presence Transition

- **Client move。** 切替区間の新旧新規開始禁止、旧 in-flight の安全な区切り、二重 presence の禁止、旧一時 state の正本化禁止、未終了作用の別 Client 自動継続禁止（CPT）。
- **round。** 旧入力は元 round、新入力は成立後の新 round、未提示出力は生成≠提示として次 Client 要約報告へ運ぶこと。表示 copy 送信を報告完了にしないこと。
- **stale 識別。** Host 帰属 vs Client 主張の対応、現在・旧・なし・停止・復旧待ちの区別、対象 Companion・Client・round・候補・試行の対応、消去・失効・停止・保留の対応を `PresenceGeneration`＋第6節の述語で運べること。確認不能を現在と推定しないこと。
- **Computer Use。** 現在 active Client 限定、安全な区切りまでの移動遅延、切断 best-effort＋不明、別 Client・Host 再実行の禁止（CPT §10）。

### 9.5 Backup / Restore

- **restore 前後 live result。** 旧 live 結果の復元混入をせず、由来別に用途受入すること。stale Permission・Provider・Client・作用・参照の現在化をせず、dangling 未解決・不明維持・replay 禁止を守ること（BR §4・§5）。
- **旧 live 分離。** `RestoreGeneration` で旧世代参照の自動利用・自動処理を抑止し、復元後保留→一括有効化を経ること。復元成立を自動開始にしないこと。
- **Credential。** 現在 store 維持・secret 非復元・現 store 照合・不足時再認証を守ること。復元参照だけで送信・利用しないこと。

### 9.6 Step 12 全 Subsystem artifact

- 個体・Character・作業・学習・接続・入出力・観測・推論・実行・権限・秘密・保全の各 owner が、第5節の型で対応を運べること。各 owner の意味判断を ID・correlation が代替しないこと。
- 通常忘却・History 整理・容量 retention（既定 OFF・明示 opt-in）・Companion 削除・targeted deletion・Reset（設定既定化 vs 全削除分離）を混同しないこと（CC-05）。
- 報告・Audit・復旧が確定度を強めないこと（CC-07）。Audit 順序を発生順・許可正本にしないこと。Raw・CoT を通常保存しないこと。

## 10. 意図的に残した Design Freedom

- crate・module・process・service・thread の分割と配置。
- DB 製品・schema・index・repository 分割・writer・transaction・lock・MVCC の選択。
- IPC・queue・event・callback・channel の選択と値（retry・timeout・polling 間隔等。待機 polling 自体は禁止のまま）。
- ID 生成器の実装（UUID v4 / v7 等の選択、採番主体の配置）。衝突しない程度の一意性・非再利用・LLM 非依存の性質は維持する。
- content digest の有無・algorithm。revision の代替にしない範囲で任意。
- Task 化閾値・分類 algorithm、Summary・revision 粒度、embedding・scoring・cache key、prompt 形成 algorithm、削除・追跡・検証・保持期間の手段、backup・暗号・復元・Credential・sandbox の手段、UI・renderer・round・確認・資源・Provider catalog の手段（いずれも SO・Step 12 の Freedom のまま）。
- 未伝達メモの保持方式、Task 記録所有・参照の内部表現、次回表示の導出方式、呼出し・自発移動の検知 algorithm、Observer routing 文脈の生成方式・model・形式・頻度（assignment・同意・費用の契約は維持する）。

## 11. Requirement / Architecture Issue の有無

- **Requirement 変更。** なし。
- **Step 11 / Step 12 semantic contract の変更。** なし。
- **semantic owner 変更。** なし。
- **subsystem boundary 変更。** なし。
- **Security / Privacy / Permission semantics 変更。** なし。
- 具体実装方式の選択（ID 生成器、digest 有無、table 分割、mechanism 選択等）は Issue ではない。第10節の Freedom の範囲で後続設計が行う。
