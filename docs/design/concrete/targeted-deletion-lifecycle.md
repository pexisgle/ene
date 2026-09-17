# Targeted Deletion Lifecycle / Participant Contract

この文書は、[指定データの完全削除](../critical-areas/targeted-deletion.md)を Stage 6 で実装するための具象契約です。

上位の意味論は変更しません。[状態所有権](../architecture/state-ownership.md)、[Interface Boundaries](interface-boundaries.md)、[Persistence / Recovery](persistence-recovery.md)、[Concurrency Control](concurrency-control.md)、[Correspondence Identity](correspondence-identity.md)、[Crate / Module Decomposition](crate-module-decomposition.md) を前提とし、それらに分散していた operation / participant / completion の交差部分だけを一意にします。

## 1. この文書が決めるもの

- Targeted Deletion を開始する preservation-owned command / outcome。
- `DeletionOperationId + DeletionSweepGeneration` の lifecycle と current erasure condition の有効期間。
- required participant set の snapshot、Host composition からの fan-out、participant completion fact。
- local erasure、remainder verification、delayed-arrival collection、`finalizing`、global completion の順序。
- restart 後の復旧と retry / stale generation の扱い。
- operation 中だけ必要な検索材料と、完了後に残してよい audit の境界。

この文書は各ドメインの具体的な DELETE 文、ページサイズ、検索アルゴリズム、暗号化方式、UI 文言を固定しません。

## 2. 既存 foundation をそのまま使う

Stage 4 で導入した次の仕組みは置き換えません。

- `DeletionOperationId`
- `DeletionSweepGeneration`
- `ErasureConditionRef`
- `erasure_condition` / `erasure_condition_source`
- inference attempt の `data_use` correlation
- AU14 の current erasure coverage check
- `DataUseHeld`

Stage 6 は同じ canonical current-condition store に producer と lifecycle を追加します。`deletion_gate`、`current_deletion_state`、`no_deletion` のような第二の registry や sentinel は作りません。

active condition が 0 件の場合も、canonical store を実際に照会した結果としての authoritative empty set です。

## 3. Target と scope

Targeted Deletion は **mechanical layer が必須**です。LLM による意味的な補助探索だけで operation を成立させてはいけません。

```rust
struct TargetedDeletionTarget {
    mechanical: MechanicalDeletionTarget,
    semantic_hints: Vec<SemanticDeletionHint>,
}

enum MechanicalDeletionTarget {
    ExactText(DeletionSearchMaterial),
    // producer を持つ別の機械的 selector が将来必要になった場合だけ追加する。
}
```

`ExactText` は acceptance のキーワード完全消去を満たすための機械的条件です。`semantic_hints` は source / Memory / Task 等の追加候補を発見する補助であり、機械的検索・消去・remainder verification の代替にはなりません。

### 3.1 operation 中だけ保持する検索材料

exact target を restart 後も検索できる必要があるため、operation が未完了の間は recovery 可能な検索材料を保持できます。ただし次を固定します。

- `deletion_operation` の通常 audit metadata と target 本文を同じ長期 record にしない。
- operation 中だけ利用する検索材料は専用の protected material とし、通常の log / Debug / management view / participant completion fact へ複製しない。
- global completion を commit する前に検索材料を破棄し、completed operation から target 本文・検索 token・可逆な派生値を復元できない状態にする。
- audit 用に target hash を残すことも禁止する。本文の照合に再利用できる fingerprint を「本文ではないから」と長期保持してはならない。

具体的な保存形式・暗号化方式は Design Freedom です。

## 4. operation admission

Owner の明示的な privacy / security purpose と trusted first-party confirmation が揃ったときだけ preservation owner が operation を開始します。

```rust
struct StartTargetedDeletionCommand {
    target: TargetedDeletionTarget,
    purpose: DeletionPurpose,
    confirmation: TrustedOwnerConfirmationRef,
    requested_at: WallClockWithTz,
}

enum DeletionPurpose {
    Privacy,
    Security,
}

enum StartTargetedDeletionOutcome {
    Started(DeletionOperationRef),
    NeedsClarification,
    ConfirmationRequired,
    AlreadyCoveredBy(DeletionOperationRef),
    HeldByOperation(HoldConditionRef),
}
```

型名は実装上の同義名へ変更できますが、意味は維持します。

- `confirmation` は Host-local trusted first-party boundary で成立した fact です。Client payload や LLM output はこの型を mint できません。
- 単なる「忘れて」「気にしないで」は Targeted Deletion admission になりません。
- `AlreadyCoveredBy` は idempotency / 重複操作の判定であり、既存 operation を勝手に completed とみなす結果ではありません。
- technical storage failure は上記 outcome へ丸めず technical error として返します。

### 4.1 durable-before-enforce

operation の開始 commit は、少なくとも次を **同じ短い transaction** で確定します。

1. `deletion_operation` の operation identity / initial phase / initial sweep。
2. operation 中だけ必要な mechanical search material への参照。
3. initial `erasure_condition`。
4. operation 開始時点で既知の covered source correlation。

この commit より前に participant の actual erasure を開始してはいけません。

commit 後は current erasure condition がすでに有効であり、participant fan-out がまだ始まっていなくても新規利用・再保存の防止が成立します。

## 5. lifecycle

```rust
enum DeletionOperationPhase {
    Active,
    Held,
    Finalizing,
    Completed,
}
```

`Completed` だけが terminal です。

### 5.1 transition

```text
start
  ↓
Active ↔ Held
  ↓
Finalizing ──remainder / delayed arrival──→ Active
  │
  └──────────────────────────────────────→ Completed
```

- `Held` は retryable な未完了状態です。participant failure、unreachable required participant、検索材料の一時利用不能等で使います。
- operation を「失敗したので削除を諦めた」という terminal failure へ自動遷移させません。privacy operation は未完了のまま保持し、Owner の判断または回復を待ちます。
- `Held` 中も current erasure condition は active のままです。
- `Finalizing` は local erasure と current sweep の verification が揃った後、検索材料の破棄・audit commit・condition closure を行う区間です。
- `Finalizing` 中に remainder / delayed arrival が見つかった場合は、新しい sweep generation を発行して `Active` へ戻ります。
- `Completed` は吸収的です。同じ operation identity を再開・再利用しません。

## 6. sweep generation

`DeletionSweepGeneration` は operation 内で単調増加し、generation 0 を sentinel として使いません。

新しい generation を作るのは、少なくとも次の場合です。

- initial sweep の後に delayed-arrival collection が新しい対象を発見した。
- remainder verification が current sweep の未消去範囲を発見した。
- finalizing 中に current interval へ新しい対象データが到着した。

stale generation の participant result / verification result は current state を更新できません。

世代番号を進められない場合は fail closed で `Held` とし、古い generation を再利用しません。

## 7. current erasure condition

`ErasureConditionRef` は `(operation, sweep)` の identity です。Stage 6 では同じ canonical record に有効期間を加えます。

```rust
struct CurrentErasureCondition {
    condition: ErasureConditionRef,
    scope: OperationScopeRef,
    opened_at: WallClockWithTz,
    closed_at: Option<WallClockWithTz>,
}
```

- `closed_at = None` の current sweep だけを current-condition query が返します。
- condition closure は global completion より先に行いません。
- source coverage と mechanical target coverage の両方を同じ operation scope に属させます。
- 既知の source から派生した provider send / write は `erasure_condition_source` の相関で防ぎます。
- deletion interval 中に新しく現れた exact target は、text-bearing ingestion / adoption boundary で mechanical condition と照合して collection 対象へ入れます。
- operation 完了後に Owner が同じ文字列を新しく提供した場合は、過去 operation の永続的な keyword ban として扱いません。closed condition は新しい由来を cover しません。

## 8. required participant snapshot

operation は開始後に「今存在する全 crate 名」を毎回動的に数え直すのではなく、current product surface に必要な participant set を durable に snapshot します。

```rust
struct DeletionParticipantRef {
    operation: DeletionOperationId,
    owner: ParticipantOwnerRef,
}

enum ParticipantProgress {
    Pending,
    Running { sweep: DeletionSweepGeneration },
    LocalComplete { sweep: DeletionSweepGeneration },
    Verified { sweep: DeletionSweepGeneration },
    Held { sweep: DeletionSweepGeneration, reason: ParticipantHoldClass },
}
```

最低限、current product で対象データを保持・利用し得る semantic owner と first-party transient holder を列挙します。将来機能の未実装 crate を架空の participant として登録しません。

### 8.1 Client participant

Client は durable master ではありませんが、target-bearing transient copy を実際に受信した current incarnation は local erasure の participant になり得ます。

- Host が「その Client は target-bearing copy を持っていない」と客観的に証明できる場合は required participant に含める必要はありません。
- target-bearing copy の保持可能性があり、local erasure の確認が必要な Client が unreachable の場合は `Held` とします。
- disconnect、connection replacement、ACK timeout だけを local erase 完了の証明にしてはいけません。
- Client 宛て wire に target の平文本文や検索 material を送らず、Host が把握する source / receipt / local-copy identity など最小 correlation へ写像します。

## 9. participant boundary

横断 trait は preservation 側が所有し、各 participant crate が実装します。`ene-preservation` は participant の具象 crate に依存しません。fan-out は `apps/ene-core` の composition が担当します。

```rust
struct DemandLocalErasureCommand {
    condition: ErasureConditionRef,
    participant: ParticipantOwnerRef,
    scope: ParticipantErasureScope,
}

struct ParticipantCompletionFact {
    condition: ErasureConditionRef,
    participant: ParticipantOwnerRef,
    status: ParticipantCompletionStatus,
    erased_count: u64,
    remainder_count: u64,
    observed_at: WallClockWithTz,
}

enum ParticipantCompletionStatus {
    MoreWork,
    LocalComplete,
    Verified,
    Held(ParticipantHoldClass),
}
```

`DemandLocalErasureCommand` は bounded work です。1回の call ですべての table / file / cache を無制限走査する必要はありません。continuation cursor の具体型は participant owner が持ちます。

### 9.1 idempotency

- 同じ `(operation, sweep, participant, bounded-range)` の retry は二重 semantic effect を起こしません。
- older sweep の completion fact は current sweep を進めません。
- participant は他 domain の row を直接更新しません。
- completion fact には target body、検索 material、credential value、prompt/output body を入れません。

## 10. local completion と verification は別

`LocalComplete` は「その participant が current sweep の erase pass を終えた」という fact であり、global completion ではありません。

`Verified` は同 participant の bounded remainder check が current sweep に対して 0 remainder を確認した fact です。

全域完了候補になるためには、required participant の **全員**が current sweep で `Verified` でなければなりません。

1 participant の完了、Host DB の1 transaction 成功、Client ACK、LLM の自己申告を global completion の代わりにしてはいけません。

## 11. delayed-arrival collection

operation が `Active` / `Held` / `Finalizing` の間、対象データの新しい arrival は current operation に回収します。

- condition より前に provider/action claim が commit 済みなら、その実行 fact 自体は消去を理由に改ざんしません。
- 後から到着した本文は各 semantic owner の adoption / save / presentation boundary で current condition と照合します。
- covered payload は通常 current state へ採用せず、erase collection / next sweep の対象として記録します。
- source absence、revision stale、permission denial、technical error と erasure hold を同じ outcome に丸めません。
- action certainty、provider usage、Task lifecycle 等の objective fact は対象本文の消去と別に保持します。

## 12. finalizing と global completion

required participant 全員が current sweep で `Verified` になったら `Finalizing` へ進めます。

`Finalizing` で次を順に満たします。

1. current generation に delayed-arrival / remainder が増えていないことを再確認する。
2. operation 中だけ必要だった target/search material を破棄する。
3. target/search material が復元不能であることを確認する。
4. 本文を含まない completion audit を確定する。
5. current erasure condition を閉じる。
6. operation を `Completed` として commit する。

2〜6 の正確な SQL grouping は実装自由度ですが、crash により **condition だけ先に解除される状態**を作ってはいけません。restart 後に `Finalizing` から安全に再開できる durable marker を持ちます。

検索材料を破棄した後に verification をやり直す必要が生じる順序も作ってはいけません。verification が無効になった場合は破棄前に新 sweep へ戻します。

## 13. audit

audit に残してよいのは operation の客観 metadata だけです。

例:

- `DeletionOperationId`
- purpose class（privacy / security）
- started / completed time
- sweep count
- participant owner と最終 status
- erased / verified count
- failure / hold の固定 class

残してはいけないもの:

- target 本文
- target の可逆な encoding
- target hash / fingerprint
- search token / matcher material
- erased History / Summary / Memory / Task body
- credential value
- provider prompt / output

## 14. restart / recovery

Host startup は durable `deletion_operation` を読み、`Active` / `Held` / `Finalizing` を unfinished operation として復元します。

- operation identity を作り直しません。
- current sweep を勝手に進めません。
- participant status を memory default へ戻しません。
- `Running` のまま orphaned になった bounded participant work は、同じ sweep の idempotent work として再駆動できます。
- `Finalizing` の operation は condition closure / audit / material wipe の durable markers を読み、安全に残りだけを実行します。
- restart 自体を completion の証拠にしません。

## 15. first-party management boundary

wire の `ManagementIntentKind::RequestDeletionBackupRestoreReset` は intent の入口であり、Targeted Deletion の authority ではありません。

Stage 6 の first-party surface は少なくとも次を区別します。

- Targeted Deletion request / preview
- trusted final confirmation
- operation status query
- retry / resume of a held operation

Client から届いた intent、会話 LLM、Task Agent は trusted final confirmation を mint できません。

status view は operation id、phase、participant progress、hold/failure class、counts を表示できますが、target body/search material を返しません。

## 16. 実装 slice への対応

| Stage 6 slice | この契約で固定する境界 |
|---|---|
| A1 | §4〜7: operation admission、durable-before-enforce、phase、sweep、condition lifecycle |
| A1b | §15: first-party request / confirmation / status |
| A2 | §8〜9: required participant snapshot、Host fan-out、completion fact |
| A3 | §9〜10: owner-local bounded erase / verify |
| A4 | §7・11: current-condition enforcement、delayed-arrival collection |
| A5 | §10・12〜14: remainder verification、finalizing、audit、restart-safe completion |

## 17. 必須 race walkthrough

実装では少なくとも次の2順序を barrier で固定します。

### R1: deletion が先

```text
erasure condition commit
→ write / provider claim / adoption / presentation admission
→ covered
→ current state へ対象本文を入れない
```

### R2: use が先

```text
use / attempt claim commit
→ erasure condition commit
→ external work may finish
→ delayed result arrives
→ current erasure condition check
→ objective attempt/effect fact は保持
→ target-bearing body は collection / erase 対象
```

第三の順序として、古い cached `no deletion` 判定で send/save/present を通してはいけません。

## 18. 完了条件

Targeted Deletion を completed と表示できるのは、次をすべて満たす場合だけです。

1. required participant 全員が current sweep で verified。
2. mechanical remainder が system-wide に 0。
3. delayed-arrival collection が current sweep に未処理対象を残していない。
4. operation 用 target/search material が破棄済みで復元不能。
5. audit が対象本文を保持していない。
6. current erasure condition が安全な finalizing boundary を経て closure 済み。
7. durable operation phase が `Completed`。

これらの一部だけを満たして global completion を宣言してはいけません。
