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
    RegisteredCredential { r#ref: CredentialRef },
}
```

`ExactText` は指定文字列を、`RegisteredCredential` は Owner が選択した非秘密の登録参照に属する対象 version の値を機械的条件にします。後者は単なる参照行の削除ではありません。認証秘密 owner が OS secure store の該当 version を限定的に読み、Ene 管理下の本文・派生データ全域をその値で検索・消去・残存検証します。参照だけで値を取得・検証できなければ開始前は確認待ち、開始後は `Held` とし、参照削除だけで完了しません。`semantic_hints` は source / Memory / Task 等の追加候補を発見する補助であり、両 variant の機械的検索・消去・remainder verification の代替にはなりません。

`RegisteredCredential` の秘密値は Client DTO・通常 SQLite・検索 token・hash・audit へ出しません。`ExactText` が登録秘密値と一致する場合も認証秘密 owner が OS item の値と限定的に照合し、漏洩コピーのみか登録自体も抹消するか、同値の複数登録のどれを対象に含めるかを確認対象に固定します。`RegisteredCredential` でも選択参照と同値の他登録を owner が検出し、各登録の用途・削除による接続停止・再認証の必要性と、分離不能な影響を秘密値なしで提示します。対象外の登録を推測で破棄せず、同値の登録を残したまま全域の値の残存 0 を主張できない場合は対象範囲の再確認を求めます。漏洩コピーだけの削除と登録の完全抹消を混同しません。

remote の `RegisteredCredential` は通常の第一者削除 DTO に非秘密 ref だけを載せる。一方、remote Owner が任意の `ExactText` を指定すると、その文字列は Host で照合される前から登録秘密と一致し得る。従って `DeletionSearchMaterial` は Host 内部の保護型であり、remote の通常 DTO / chat / management intent の文字列フィールドにはしない。値指定は §15 の削除専用 secret-bearing input で先に認証秘密 owner へ渡し、照合後に target を構築する。専用経路が利用不能なら値指定の遠隔開始は受理せず、登録済みの対象は非秘密 ref の選択へ誘導する。ref 指定を含む remote 削除全体を拒否しない。

### 3.1 operation 中だけ保持する検索材料

両 variant の対象値を restart 後も検索できる必要があるため、operation が未完了の間は recovery 可能な検索材料を保持できます。ただし次を固定します。

- `deletion_operation` の通常 audit metadata と target 本文を同じ長期 record にしない。
- operation 中だけ利用する検索材料は専用の protected material とし、通常の log / Debug / management view / participant completion fact へ複製しない。登録秘密値（`RegisteredCredential` から読み出す値、およびそれに一致する `ExactText`）の一時検証材料は、対象 OS item と独立に消去できる OS secure store の operation 専用範囲にだけ保持する。通常 SQLite には非秘密の参照・phase のみ置き、値・検索 token・hash を置かない。各 participant への機械照合も限定的な秘密利用境界を通し、値を DTO や永続 index に渡さない。
- 登録秘密値の一時材料は OS `put` **前**に、その installation namespace・operation 専用 item ID / version・準備 phase を非秘密の durable preparation として記録する。OS `put` 後、削除開始 commit でその準備行を同じ非秘密参照の operation 行へ原子的に移す。commit 失敗・確認失効・stale でも別の値で同じ ID を再 `put` せず、準備 material の消去を確認するまで pending とする。準備段階の material は削除参加者の検証材料としては使わない。
- serving startup は読み取り / startup sweep とは別の mutating reconciliation で、operation 専用 namespace の **全候補** OS item を ID / version で列挙し、現在の operation 参照・preparation・restore recovery context と突き合わせる。起動前の準備 session は失効させ、operation に結び付かない candidate と残存 preparation は zeroize / OS delete / 再照会で消去を検証する。OS 列挙不能、対応不明、消去成否不明なら正常な登録秘密利用と新しい Targeted Deletion を保留する。operation に紐づく未完了材料は保持し、登録値の全参加者 scan・残存検証が終わるまで sweep が消さない。通常 read / open は reconciliation を起動しない。
- 対象 OS item は、全 required participant の scan と局所残存検証、旧 in-flight の本文受入・送信の収束、`Finalizing` の新規本文受入停止、全域残存検証が済むまで破棄しない。これらの検証後に対象 item と独立した一時検証材料をそれぞれ削除・再照会する。item を先に失っても一時材料から継続できる場合を除き、値の再来を照合せずに通してはならない。
- global completion を commit する前に検索材料を破棄し、completed operation から target 本文・検索 token・可逆な派生値を復元できない状態にする。
- audit 用に target hash を残すことも禁止する。本文の照合に再利用できる fingerprint を「本文ではないから」と長期保持してはならない。

秘密値以外の `ExactText` の保護形式と照合方式は Design Freedom です。crash 後に必要な検証材料が欠落・使用不能、または破棄の結果が未確定なら current condition と本文受入停止を維持して pending / unavailable とし、必要なら Owner に再提供・再確認を求めます。値の不在や再起動を残存 0・完了の証拠にしません。

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

- `confirmation` は Targeted Deletion 専用の trusted first-party 直接確認を Host が検証した fact です。Host-local の専用確認面、または認証済み・ペアリング済みの通常 Client（remote を含む）の専用確認面で成立できます。一般の Client intent / payload の自己申告や LLM output からは mint できません（§15）。
- preservation owner は開始 commit の直前の短い直列化境界で operation ID、privacy / security purpose、target variant とその機械的 scope（登録なら対象参照・全対象 version・同値登録の扱い）、影響・除外事項、expected revision / generation、確認の由来・期限・未消費状態を現在値と再比較して one-shot に消費します。remote では確認時の device / authenticated session / connection / incarnation も一致させます。確認不能・期限切れ・接続置換・前提変化は hold / refusal とし、開始 commit も参加者の mutation も行いません。新しい確認なしに対象を差し替えません。
- 単なる「忘れて」「気にしないで」は Targeted Deletion admission になりません。
- `AlreadyCoveredBy` は idempotency / 重複操作の判定であり、既存 operation を勝手に completed とみなす結果ではありません。
- technical storage failure は上記 outcome へ丸めず technical error として返します。

### 4.1 durable-before-enforce

operation の開始 commit は、少なくとも次を **同じ短い transaction** で確定します。

1. `deletion_operation` の operation identity / initial phase / initial sweep。
2. operation 中だけ必要な mechanical search material への非秘密参照（登録秘密なら OS secure store の独立した一時検証材料への参照）。
3. initial `erasure_condition`。
4. operation 開始時点で既知の covered source correlation。
5. 削除対象本文を含まない確認 provenance（確認経路、operation / request ID、目的、対象範囲と影響の非秘密参照、現在性と消費結果）。

この commit より前に participant の actual erasure を開始してはいけません。

登録秘密材料の準備は §3.1 の durable preparation → OS 保存 / read-back → 開始 commit であり、OS I/O 中に SQLite transaction / publication guard を保持しません。開始時に準備 ID と確認された値の同一性・現行 credential version / scope を再照合し、stale な準備を別対象へ流用しません。未完了の準備・照合不能な OS item を startup reconciliation で解決できなければ新しい開始は保留します。

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
- `Finalizing` は本文領域の local erasure と current sweep の残存検証が揃った後、登録秘密 owner の OS item / 一時材料の最終消去・検証、audit commit、condition closure を行う区間です。
- `Finalizing` 中に remainder / delayed arrival が見つかった場合は、新しい sweep generation を発行して `Active` へ戻ります。
- `Finalizing` に入ったことだけで ingress を通してはいけません。current condition を参照する text-bearing 受入と finalizing の境界は直列化し、検索材料を破棄する区間に対象本文が新たに採用・保存・提示されないようにします。
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
- deletion interval 中に新しく現れた対象値は、両 target variant とも text-bearing ingestion / adoption boundary で機械的条件と照合して collection 対象へ入れます。秘密値の照合は認証秘密 owner の限定的な境界で行い、照合不能なら本文を受け入れません。
- operation 完了後に Owner が同じ文字列を新しく提供した場合は、過去 operation の永続的な keyword ban として扱いません。closed condition は新しい由来を cover しません。

### 7.1 論理本文と完了後の由来バリア

`erasure_condition_source` は既知の source 依存を照合する手段であり、本文の機械的 coverage の代用ではありません。Task Agent の送信では、採用目的・指示・既実行 facts に加え、送信する turn に実際に残った provider 出力 / Action・Workspace 観測の execution-local transcript を含む **dispatch 予定の論理本文全体**を current mechanical condition と照合します。source のない transcript も対象です。省略済み exchange は送信対象外ですが、残した exchange や境界を跨ぐ一致を見落としてはいけません。判定不能なら送信せず、source ID が非被覆・active set が空というだけで covered 本文を通しません。本文・対象検索 material・本文 hash を Task / Inference の durable attempt や `data_use` へ複製しません。

Action owner の Workspace `Create / Edit` も、権限や path の照合だけでは消去条件を満たしません。実際に書く確定済み本文（source のない生成本文も含む）の機械的 coverage と既知 source lineage・操作対象の消去制約を、AU5 の attempt 挿入と同じ SQLite master の短い `Immediate` transaction で current condition に照合します。`Read / List` も対象と既知 source を同じ開始比較にかけ、取得した本文・名前は利用・返却・再保存の前に現在の機械的条件と最終照合します。covered / 判定不能なら独立した `DataUseHeld` とし、新しい Action 試行も作用も開始しません。Action の権限 refusal・task stale・technical error と混同しません。重い本文準備や scan、ファイル I/O を長い transaction に入れず、事前判定を固定した候補と条件集合の世代に束縛して current condition の世代・phase を同じ commit で確かめます。変化した場合は transaction 外で再判定し、同一性や coverage が証明できなければ保留します。対象本文・検索 material・本文 hash は Action attempt に複製せず、既知 lineage と試行の body-free identity / 順序だけを保持します。

完了時に検索 material を消す代わりに、削除開始の同じ durable 境界で発行した非本文の operation / provenance cut と、その operation に被覆された旧 source identity の body-free な対応を完了後も保持します。開始前に受理された provider attempt・delegation execution・Action attempt（およびそれらの source lineage）を識別する durable identity / 順序を受入側から照合できるようにし、cut より前の処理から戻る結果がこの operation の旧 source に依存するか、依存しないと証明できないなら、完了後も本文受入を拒否します。cut は壁時計や到着時刻ではなく、開始と attempt / execution の登録を同一 SQLite master で直列化した順序です。既存の attempt facts と source 相関は削除で消さず、不要になった対象本文・hash を監査や barrier に残しません。完了後に新しく Owner が提供した入力は新しい source / 新しい受付順序として扱い、同じ文字列の恒久的な禁止にはしません。由来・順序が欠けて旧処理と新規 Owner 入力を区別できなければ本文は受け入れず、本文なしの客観的事実だけを保持します。

### 7.2 Character export 公開と削除の競合

Character の配布候補は公開前 staging の間も Ene 管理下の target-bearing copy です。Character owner は候補の本文、manifest・ファイル名・参照先、管理下 asset のバイト・埋込情報、およびそれらの source lineage を、実際に公開する範囲全体で機械的条件と完了後 cut に照合し、確定した候補・出力範囲・由来を Action owner に渡します。source 相関が空でも本文の機械的 coverage は省略せず、静的設定・PII 分類・形式検証・Owner の目視確認を消去の証明にしません。重い scan・候補生成・asset I/O は transaction 外で行い、結果を同一候補・`CharacterRevision`・current condition set の generation / phase に束縛します。判定不能・被覆あり・途中で候補が変化した場合は公開せず、候補を除外したなら参照と必須部品の整合を再検証します。Character owner の検証は外部ファイル作用の許可・開始ではありません。

実際の外部 publish は Action owner が Permission owner の今回限りの評価を受け、外部パスの実対象を保持して行います。`TaskBound` なら現在の Task / delegation / Workspace と必要な確認済み保存先、Companion 付き `Direct` なら認証済み Owner 指示・同じ Running Companion・許可対象範囲を AU5 で照合します。個体非依存の管理画面からの export は `Direct { instruction: Management(_), companion: None, system_operation: Some(CharacterExport) }` とし、認証済み管理操作の現在性・Owner の指定した出力先・用途・操作・実対象を Permission owner の single-use 評価・AU5・作用直前で照合します。Stop はこの管理起点には適用せず、削除・復元保留は適用します。架空の Task / Companion や stage を権限にしません。AU5 の同じ SQLite master の短い `Immediate` 比較で確定済み revision・候補と実出力内容の同一性・現在の condition / phase / generation・既知 lineage / 旧由来 cut を確認し、試行を body-free で記録します。削除開始が先なら `DataUseHeld`（試行・作用なし）、AU5 が先なら削除参加者が already-started use として追跡します。AU5 は公開完了ではありません。Action owner は同じ内容・source・保持した実対象と権限・帰属を作用直前にも再照合し、不可逆な公開開始と deletion start / finalizing / completion を同じ master の短い同期区間で直列化します。先に削除・失効したら作用を止め、変化に伴う再 scan は transaction 外で行います。公開開始を区間内に確定できなければ解放して再照合し、外部 I/O 完了まで DB transaction を保持しません。公開開始後の作用不明は Action 試行の `Unknown` として保ち、客観的証拠なしに公開成功・無作用・自動再実行としません。外部に公開済みのコピーは Ene の管理外ですが、試行開始だけで staging を管理外扱いしません。同一公開先への writer は lock / CAS / 単一 writer で直列化し、実対象を取り違えない no-clobber と、半端なコピーを成功としない atomic publication を維持します。

公開前 stage（生成途中の一時ファイルを含む）と管理下 asset は operation の required participant / local erase / remainder verification に含めます。削除開始後の stage の追記・再構築・公開と、完了後に旧 stage からの後出し再流入を current condition / durable cut で防ぎます。完了後の新しい Owner source とは由来と受付順序で区別し、証明不能なら公開を保留します。一度公開済みの外部コピーは Ene が管理できないため、後日の Targeted Deletion の participant 消去義務や global completion 条件に含めず、内部の stage や管理下 asset まで外部コピー扱いして除外しません。

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

Client は durable master ではありませんが、target-bearing transient copy を受信し得た incarnation は local erasure の participant になり得ます。

- Host が「その Client は target-bearing copy を持っていない」と客観的に証明できる場合は required participant に含める必要はありません。
- target-bearing copy の保持可能性があり、local erasure の確認が必要な Client が unreachable の場合は `Held` とします。
- disconnect、connection replacement、ACK timeout だけを local erase 完了の証明にしてはいけません。
- Client 宛て wire に target の平文本文や検索 material を送らず、Host が把握する source / receipt / local-copy identity など最小 correlation へ写像します。
- body-bearing read（History item、presentation excerpt、report source、management view の Memory 本文 / revision 履歴 / grounds）は、Host が本文送信の確定点で current condition と phase を直列化して照合し、covered 本文と `Finalizing` 中の通常受入を withhold します。送信可能な本文についても、対象 Client incarnation と local-copy class / source / receipt 等の body-free な配送可能性を Host の canonical store に durable commit **してから** 本文を Client に渡します。commit 失敗時は送信せず、送信失敗・未配送時の過大追跡は許容します。Client retry で本文を再送する場合も現在条件を再照合し、同じ送信前 commit を満たします。operation 開始時の required participant snapshot と開始後の追加判定にはこの記録を用い、Host restart・disconnect・connection replacement・ACK timeout や未配送の推定では消さず、該当 class 全域の検証済み local erasure だけで消去します。

送信前記録の commit だけを、後からいつでも本文を送れる許可として使い回しません。Host は消去条件の開始と **各** body-bearing frame / stream chunk の dispatch を直列化する短い gate で、その部分の source と本文に current condition を再照合し、その区間内に送信を開始できなければ dispatch を保留して再照合します。ストリーム開始時の許可を後続 frame へ流用せず、対象文字列が chunk 境界を跨ぐ場合も漏れなく照合できない部分は保留します。新しい source / class / receipt の配送可能性も各部分の送信前に durable commit します。通信の完了待ち中に gate や DB transaction を保持しません。消去開始より先に dispatch を始めた未完了 frame は already-started delivery として追跡し、該当 Client は削除要求を受けた時点で古い送信区間の body-bearing frame を受け入れないよう fence します。Host は旧区間の送信が停止・収束し、Client の該当 class 全域の消去と遅延 frame の不受理が確認されるまでその参加者を `Verified` にしません。送信の収束や Client 側の fence を確認できなければ `Held` とし、再接続だけで完了とみなしません。

### 8.2 登録済み Credential の participant

`RegisteredCredential` または登録値そのものに一致する `ExactText` の登録抹消が確認済みの対象なら、認証秘密 owner を required participant に含めます。履歴内の漏洩コピーのみが対象の場合は、登録秘密を自動失効させません。開始 commit は対象 version と非秘密参照の利用禁止・revision 前進を同じ current condition と不可分に公開し、旧 lease や未送信 stream の本文を消去または body-free unavailable に閉じます。owner は全参加者の scan・残存検証と旧 in-flight の収束を確認し、`Finalizing` の本文受入停止を確立した後、対象 OS item の全 version と snapshot、および独立した一時検証材料を削除・検証します。`unknown` な試行は空の used-version 集合として扱わず、利用量・外部作用の事実と確定度を残します。OS item の消去不能や遅延再流入の検証不能は `Held` とし、通常 credential 更新の cleanup 完了を局所 `Verified` に代用しません。外部 provider 側の鍵の失効は全域完了の範囲に含めません（[Credential publication §4.1](credential-publication.md#41-登録秘密を対象とする-targeted-deletion)）。

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

`Verified` は同 participant の bounded remainder check が current sweep に対して 0 remainder を確認した fact です。登録秘密 owner の最終 `Verified` には、対象 OS item と一時材料の破棄・再照会も必要です。本文全域 scan 済みでも、その前は `LocalComplete` / pending のままです。

全域完了候補になるためには、required participant の **全員**が current sweep で `Verified` でなければなりません。

1 participant の完了、Host DB の1 transaction 成功、Client ACK、LLM の自己申告を global completion の代わりにしてはいけません。

## 11. delayed-arrival collection

operation が `Active` / `Held` / `Finalizing` の間、対象データの新しい arrival は current operation に回収します。

- condition より前に provider/action claim が commit 済みなら、その実行 fact 自体は消去を理由に改ざんしません。
- AU5 が先に commit した Action attempt は、既知 source lineage・操作対象と開始順序から削除参加者が列挙する already-started use です。停止・縮小を試み、作用直前にも current condition を再照合します。新たに covered / 判定不能なら止め、無作用が客観的に証明できる場合だけ確定度を進め、証明できなければ `Unknown` を維持します。遅延した Action 結果本文や Workspace への再保存も owner の確定点で condition・完了後 cut を照合し、旧由来 / covered / 判定不能なら本文なしの fact に留めます。
- 後から到着した本文は各 semantic owner の adoption / save / presentation boundary で current condition と照合します。
- covered payload は通常 current state へ採用せず、erase collection / next sweep の対象として記録します。
- source absence、revision stale、permission denial、technical error と erasure hold を同じ outcome に丸めません。
- action certainty、provider usage、Task lifecycle 等の objective fact は対象本文の消去と別に保持します。
- Task final result の到着は AU15a で機械的 coverage と上記 provenance cut を同じ不分区間で照合します。covered / 判定不能 / 旧処理の未証明 lineage の場合は本文を保存せず、body-free の result identity・依拠 delegation / TaskRef・到着と seal・検証済み Action 相関を維持して erasure-held とします。本文を後から埋めたり採用・完了へ昇格したりしません。
- 到着時には安全だった保存済み `task_result` の本文が後から対象になった場合は、削除参加者が `recorded → body_erased` と本文の NULL 化を原子的に確定します。到着時保留の erasure-held と区別し、採用済み結果の identity・delegation seal・`adopted_revision`・Action 相関・Task progress は巻き戻しません。非依拠理由や達成根拠の参照先にも対象本文があれば各 owner の消去規則で除去し、本文なしの過去の採用事実から対象本文を再構成・再提示しません。

## 12. finalizing と global completion

required participant の本文領域の current sweep の scan・残存検証が揃ったら `Finalizing` へ進めます。登録秘密 owner の OS item と一時材料の破棄が残る場合、その participant の最終 `Verified` と global completion はまだ確定しません。

最後の verification と完了 commit の間も、受入側の「対象なし」という古い判定を使い回しません。受入・保存・提示の確定点は current condition と finalizing phase を同じ直列化境界で照合します。finalizing 中に到着した本文は未検証のまま受け入れず、消去区間内の入力として保留または破棄します（作用・利用量などの本文を含まない客観的事実は元の試行へ記録できます）。入区間前に確定済みの受入が残存検証へ反映されていない場合、または新たな対象が検出された場合は、検索材料を破棄する前に新 sweep へ戻して再検証します。

`Finalizing` で次を順に満たします。

1. text-bearing 受入と直列化した境界で、current generation に未処理の delayed-arrival / remainder がないこと、本文領域の verification が依然有効であること、旧 in-flight の本文受入・送信が収束したことを再確認する。以後 condition closure まで未検証本文の新規採用を許さない。
2. operation 中だけ必要だった target/search material を破棄する。登録秘密なら全域検証を終えた後、対象 OS item と独立した一時検証材料の両方を消去する。
3. target/search material が復元不能であり、登録秘密 owner を含む全 required participant が current sweep で `Verified` であることを確認する。
4. 本文を含まない completion audit を確定する。
5. current erasure condition を閉じる。
6. operation を `Completed` として commit する。

完了 commit は旧処理の durable provenance cut を残したまま行います。active condition だけを解除して旧 in-flight 本文を通す順序は許しません。AU14 / provider dispatch・AU5 / Action 作用直前・Action 結果保存・AU15a と deletion start / finalizing / completion は同じ master の短い直列化境界を共有し、待機中の provider / Action I/O ではロックを保持しません。cut の durable 化が不明なら完了とせず、再起動後の本文受入は保留します。

2〜6 の正確な SQL grouping は実装自由度ですが、crash により **condition だけ先に解除される状態**を作ってはいけません。検索材料破棄後に crash した場合も、finalizing の受入禁止を durable に復旧してから serving を開始し、残存検証を無効にする新規採用を許さずに残りの完了処理だけを進めます。安全な再開が証明できない場合は未完了として保留し、完了と表示しません。restart 後に `Finalizing` から安全に再開できる durable marker を持ちます。

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
- `Finalizing` の operation は condition closure / audit / material wipe の durable markers を読み、安全に残りだけを実行します。登録秘密の OS item と一時材料の破棄順・成否が未確定なら本文受入を止め、照合材料を回復できなければ Owner の再提供・再確認を待ちます。
- finalizing の受入禁止が確立する前に read-only query を serving する場合も、それを新規受入の許可へ流用しません。再起動後の新規受入は finalizing の durable phase を照合してから許可します。
- restart 自体を completion の証拠にしません。

## 15. first-party management boundary

wire の `ManagementIntentKind::RequestDeletionBackupRestoreReset` は intent の入口であり、Targeted Deletion の authority ではありません。

Stage 6 の first-party surface は少なくとも次を区別します。

- Targeted Deletion request / preview
- trusted final confirmation
- operation status query
- retry / resume of a held operation

Host は正規化した operation ID、目的、target variant と対象・範囲、分離不能な影響と除外事項、expected currentness を専用画面へ提示します。Host-local の control seat ではその専用 channel と Owner の直接入力を確認します。remote では信頼する公式 Client が専用面で Owner の直接入力を受け、Host は認証済み接続から報告された one-shot・短期限の確認を受理します。remote flow は Host が発行した challenge と device / session / connection / incarnation / Host incarnation に束縛し、当該接続の失効・置換・切断・Host restart で未消費確認を失効させます。Host が検証できるのは Client の認証と challenge・現在性であり、Client 上の人の物理入力や公式 UI の由来そのものではありません。Client process / OS / 公式コードと対話 session の完全性をこの経路の信頼前提とします。専用 first-party 面への入力とその由来を通常の management intent や chat / tool frame と分離し、会話 LLM、Task Agent、tool / plugin、Computer Use の操作・結果、代理承認、`confirmed=true` 等の自己申告から確認を mint しません。Host-local の高権限操作用 seat は remote へ移譲しません。GUI が Host に存在しなくても、remote の同じ通常 Client の専用面で確認できるなら削除開始を拒否しません。

`RegisteredCredential` では remote 側に秘密値を送らず非秘密の `CredentialRef` を Owner が選択します。Host は認証秘密 owner に対象 version と同値登録を照会し、秘密を表示せずに登録用途・消去影響・再認証の必要性と owner participation を提示して同じ admission に通します。Host GUI が無い remote も ref 指定ができます。

remote `ExactText` は、公式 Client の削除専用秘密入力面から、認証済みの当該接続の限定 secret-bearing deletion input frame へ一回だけ送る。通常の削除 request / preview / completion、management / chat DTO、Client 長期 cache、履歴、log / Debug / telemetry に値を載せない。Host は短寿命の入力 buffer を credential owner の限定照合へ渡し、登録値との一致・同値登録の範囲を値を表示せずに決める。preview / one-shot 確認は値・hash ではなく当該入力の短寿命な非秘密 handle と対象範囲・目的・影響・現在性に束縛し、接続断・失効・期限切れ・Host restart で未消費入力を無効化 / zeroize する。成立後は §3.1 の保護材料へ移し、照合前・後とも通常 SQLite / Client DTO に生値を流さない。Client / Host がこの限定 frame の非保持・短寿命・owner 到達を保証できない場合は remote 値指定を明示拒否し、登録済み対象は ref 指定へ案内する。これは通常 API キー登録・更新を Host-local から remote へ移す channel ではない。削除開始後の durable condition・participant sweep・確認 provenance と restart 復旧は §4.1〜14 のまま適用します。

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

### R3: finalizing と新規受入

```text
受入 commit が先 → current sweep の再検証に含める（含められなければ新 sweep）→ material wipe
finalizing 境界が先 → 対象本文の新規採用を保留・破棄 → material wipe → completion
```

material wipe 後・completion 前に crash しても、再起動した Host が保留中の本文を採用してから完了を確定する第三の順序は許しません。

### R4: Client 本文配送と deletion

```text
配送可能性の durable commit → Host crash / ACK 喪失 → Client に届いた可能性として追跡
ある frame の dispatch が先 → deletion condition commit → 後続 frame は current condition で再照合・拒否 → 旧送信区間を fence → Client class 全域の消去・遅延 frame 不受理を検証
deletion condition が先 → dispatch が covered 本文を拒否 → Client へ送らない
```

送信前記録が失敗した本文は dispatch できません。dispatch が先でも旧 frame を後から Client に受理させたまま `Verified` とする順序はありません。区間の収束が確認できない場合は `Held` を維持します。

### R5: Task Agent transcript と結果到着

```text
Workspace observation に対象本文、origin.source は非被覆 → AU14 / dispatch が論理本文を照合 → DataUseHeld、0 byte
AU14 claim → deletion start → final result 到着 → AU15a は body-free erasure_held と seal、AU15b は ErasureHeld、Unknown は残す
deletion Completed → 削除前に始まった provider / Action の結果到着 → durable cut が旧由来を検出 → body-free erasure_held
deletion Completed → 新しい Owner source から同じ文字列を提供 → cut 上は新規由来、過去 keyword の永久禁止はしない
```

検索 material 消去後に再び対象文字列を検索できると仮定しません。由来が証明できない結果は body-free held とし、再起動しても後日本文を充填しません。

### R6: Workspace Action 本文と deletion

```text
deletion condition commit → AU5 (`Create / Edit` 実書込本文・既知 source・対象を比較) → covered / 判定不能 → DataUseHeld、試行 0・作用 0
AU5 commit → deletion condition commit → attempt を already-started use として追跡 → 作用直前に再照合して停止・縮小 → 遅延結果の本文保存を再照合
```

`Read / List` は AU5 で対象・既知 source を照合し、実際に取得したデータの利用・返却前にも最終照合します。作用直前の比較を削除開始と同じ master の condition 更新に結び付け、古い許可やローカル mutex だけで作用を通しません。AU5 が先でも作用済み・停止済みを推測せず、実際の結果に従い `Unknown` を保持します。完了後は旧 attempt / execution の body-free lineage と cut によって再保存を拒否し、完了後に新しく受理された Owner 入力を旧由来と混同しません。

### R7: Character export stage と deletion

```text
候補検証 → deletion condition commit → 旧 stage の公開比較 → current generation / phase 不一致 → 公開保留、stage を削除参加へ
公開開始 fence → deletion condition commit → 既に公開した外部コピーは管理外、未公開 stage・管理下 asset は削除参加へ
deletion Completed → 旧 stage の遅延公開 → durable cut が旧由来を検出 → 公開拒否
```

候補の静的分類や revision の一致だけで削除後の公開を許可しません。完了後の新規 Owner source を旧 stage と混同せず、由来不明なら公開を保留します。

### R8: 登録秘密の operation 材料と開始 commit

```text
preparation durable → OS 保存前に crash → namespace 全列挙 / 準備参照の照合 → 不在・消去を確認して準備を閉じる
preparation durable → OS put → 開始 commit 前に crash → 未接続候補を削除・再照会 → 成否不明なら serving / 新 deletion 保留
preparation durable → OS put / read-back → 開始 commit → preparation を operation 参照へ原子的に移動 → 全参加者の機械検証まで材料維持 → finalizing wipe
```

開始 commit の成否が不明でも material の新規 `put` や別値の再投稿はしません。DB / OS の照合は mutating startup reconciliation が行い、通常 read は行いません。restore context 上の未完了 operation に結び付く材料は、復元 DB に参照が無いだけで孤立物と判定して消しません。

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
