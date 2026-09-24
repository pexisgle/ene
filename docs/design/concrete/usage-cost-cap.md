# Usage / Cost / Cap Contract

この文書は、Stage 6 で必要な provider token usage、API cost、利用上限（cap）を current inference pipeline に接続するための具象契約です。

上位の意味論は変更しません。[状態所有権](../architecture/state-ownership.md)、[横断的関心事](../architecture/cross-cutting.md)、[Interface Boundaries](interface-boundaries.md)、[Persistence / Recovery](persistence-recovery.md)、[Concurrency Control](concurrency-control.md)、[Correspondence Identity](correspondence-identity.md)、[Crate / Module Decomposition](crate-module-decomposition.md) を前提とし、それらに分散していた usage / cost / cap の交差部分だけを一意にします。

## 1. authority split

責任は次のように分けます。

| 領域 | authoritative owner | 所有するもの |
|---|---|---|
| provider call / token usage / pricing snapshot / cost fact | inference | 実際にどの provider/model を呼び、どの token usage が報告され、どの rate で cost を確定したか |
| provider/system cap の設定と admission | permission / constraint | いま新しい利用を開始してよいか、daily/monthly の上限値、current cap revision |
| 永続化 | `ene-store` | owner-defined premise / fact / outcome の SQLite 実装。usage や cap の意味を独自に決めない |
| composition | `apps/ene-core` | current provider route、cap premise、first-party management path の接続 |

provider I/O を permission crate へ移しません。cap 判定を inference 側の process-local counter へ移しません。

## 2. usage identity と attribution

provider inference の durable usage は既存の `InferenceTicketId` へ一意に対応します。

```rust
struct UsageAttribution {
    ticket: InferenceTicketId,
    provider: ProviderRef,
    model: ModelRef,
    consumer: ConsumerKind,
    purpose: PurposeKind,
    task: Option<TaskUsageCorrelation>,
}
```

`consumer` / `purpose` / Task delegation 等の attribution は既存 `inference_attempt` correspondence から解決します。`usage_fact` に prompt / output / task body を複製して attribution を作ってはいけません。

同じ ticket について複数の actual usage fact を確定しません。retry / delayed provider response / duplicate completion は idempotent に同じ record へ対応します。

## 3. token usage fact

Stage 6 の OpenAI acceptance では次の3区分を保持します。

```rust
struct ReportedTokenUsage {
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
}

enum TokenUsageFact {
    Reported(ReportedTokenUsage),
    Unknown,
}
```

### 3.1 cached input

OpenAI Responses API の `input_tokens_details.cached_tokens` は input token の部分集合として扱います。

- `cached_input_tokens <= input_tokens` を満たさない report は正しい usage として採用しません。
- cached count が provider response に存在しない場合、勝手に `0` と推定しません。provider contract が「field 欠如 = 0」を明示的に保証する場合だけ 0 と確定できます。
- input/output の一部だけが取得できた場合も、完全な Reported fact に見せるために欠損値を 0 埋めしません。
- usage 全体を確定できない場合は `Unknown` とします。

current milestone の provider adapter は OpenAI だけですが、usage fact 自体は provider 固有 JSON shape を外へ漏らしません。

## 4. pricing snapshot

過去の cost が後日の価格変更で書き換わらないよう、provider call の admission 時に pricing snapshot を固定します。

```rust
struct PricingSnapshotRef(/* opaque */);

struct PricingSnapshot {
    id: PricingSnapshotRef,
    provider: ProviderRef,
    model: ModelRef,
    currency: CurrencyCode,
    input_rate: TokenRate,
    cached_input_rate: TokenRate,
    output_rate: TokenRate,
    effective_at: WallClockWithTz,
    source_revision: PricingCatalogRevision,
}
```

rate は浮動小数を canonical value にせず、固定小数の整数単位で保持します。例えば「1,000,000 token あたりの micro-currency」のような exact integer ratio を利用できます。

具体的な単位名は実装自由度ですが、次を満たします。

- overflow を検出し、wrap / saturating zero にしない。
- rounding rule は一意で再計算可能。
- historical usage は使用時に結び付いた snapshot で再計算する。
- current pricing catalog の更新で historical cost を変更しない。
- unknown model / missing rate を別 model の rate で代用しない。

### 4.1 pricing source

Stage 6 は pricing のネットワーク自動取得を要求しません。current milestone では reviewed first-party pricing catalog を利用できます。

pricing catalog の更新は revision を持ち、新しい provider call だけが新 snapshot を利用します。

## 5. cost fact

```rust
struct ReportedCost {
    input: Money,
    cached_input: Money,
    output: Money,
    total: Money,
    pricing: PricingSnapshotRef,
}

enum UsageCostFact {
    Reported(ReportedCost),
    Unknown { pricing: PricingSnapshotRef },
}
```

OpenAI の cached input cost は、`input_tokens` 全体へ通常 input rate を掛けた後に cached rate を追加するのではありません。

```text
non_cached_input = input_tokens - cached_input_tokens
input_cost        = non_cached_input × input_rate
cached_input_cost = cached_input_tokens × cached_input_rate
output_cost       = output_tokens × output_rate
```

`Unknown` を 0 token / 0 cost と表示・集計しません。

## 6. cap scope

Stage 6 では requirements にある次の cap を実装対象とします。

```rust
enum UsageCapScope {
    System,
    Provider(ProviderRef),
}

enum UsageCapWindow {
    DailyUtc,
    MonthlyUtc,
}

struct UsageCap {
    id: UsageCapId,
    revision: UsageCapRevision,
    scope: UsageCapScope,
    window: UsageCapWindow,
    limit: Money,
}
```

日・月境界は Stage 6 では UTC calendar boundary に固定します。UI は local time へ表示変換できますが、cap の集計境界を Client locale や実行中の timezone 変更で動かしません。

各scopeの `DailyUtc` / `MonthlyUtc` は独立して選択でき、いずれか一方のみ、またはcap未設定も有効な設定です。未設定を暗黙のゼロ円capや無限の費用報告として扱いません。

将来、billing timezone を明示設定する producer が必要になった場合は別 revisioned setting として追加します。

current milestone は OpenAI/USD の同一 currency 内で比較します。複数 currency を自動換算する producer はありません。currency が一致しない usage を推測レートで cap へ混ぜてはいけません。

## 7. reservation lifecycle

provider I/O の前に upper-bound cost を reserve します。

```rust
struct UsageReservationRef(/* opaque */);

enum UsageReservationState {
    Reserved,
    CommittedReported,
    CommittedUnknown,
    Released,
}

struct UsageReservation {
    id: UsageReservationRef,
    ticket: InferenceTicketId,
    pricing: PricingSnapshotRef,
    upper_bound: Money,
    state: UsageReservationState,
    opened_at: WallClockWithTz,
    daily_window: UtcCalendarDay,
    monthly_window: UtcCalendarMonth,
}
```

`UsageReservationRef` は permission / constraint の cap-admission boundary が所有します。inference は returned ref を durable attempt correlation として保持できます。

`opened_at` は予約を作った時刻であり、送信済み利用の帰属時刻ではありません。日額・月額の帰属は **provider dispatch を実際に開始する時点の UTC calendar window** です。予約時に候補となる両 window を保持し、その window の間にだけ送信を開始できます。settlement / recovery の時刻では移しません。各 window の集計は、その window に帰属する `Reserved` / `CommittedReported` / `CommittedUnknown` を合計し、`Released` は計上しません。送信前でも未解放の `Reserved` は残枠を占めます。

### 7.1 state transition

```text
Reserved ──reported usage──→ CommittedReported
    │
    ├──effect may have happened / crash / response lost──→ CommittedUnknown
    │
    └──provider I/O が始まっていないことを証明──→ Released
```

`CommittedReported` / `CommittedUnknown` / `Released` は terminal です。

- provider I/O が開始された可能性を否定できない場合は `Released` にしません。
- Host crash 後も `Reserved` のまま残った枠は、送信前の release が commit されていないため、recovery で `CommittedUnknown` へ確定します。送信未開始を証明できた場合の terminal な `Released` は別の状態です。
- `CommittedUnknown` は cap 集計で upper bound を消費したままにします。
- later usage report を安全に同一 external call へ対応付けられる contract が将来追加された場合だけ Unknown settlement を追加できます。Stage 6 では勝手な後修正を行いません。

## 8. safe upper bound

cap を守るには reservation が実際の請求より小さくならない必要があります。

provider adapter は admission 前に conservative な `UsageEstimate` を作ります。

```rust
struct UsageEstimate {
    input_tokens_upper_bound: u64,
    output_tokens_upper_bound: u64,
}
```

- input estimate は provider/model の tokenizer contract に対して安全な上限でなければなりません。
- output は provider request 自体に explicit maximum を設定し、その値を upper bound に使います。
- provider が finite な safe upper bound を構成できない場合、cap-enabled send を開始しません。
- cache hit は事前保証できないため reservation では通常 input rate を使うなど、安全側の上限を取ります。actual completion で cached rate へ精算します。
- estimator の具体的アルゴリズムは provider adapter の Design Freedom ですが、「経験上だいたいこの程度」の平均値は upper bound として不適切です。

## 9. provider-send admission の linearization

cap check を provider call のはるか前で行い、その結果を cached allow として使ってはいけません。

current inference attempt claim と usage reservation を同じ短い SQLite transaction にまとめます。

概念的な順序は次です。

```text
prepare authorized inference
→ provider/model pricing snapshot resolve
→ safe usage upper bound resolve
→ begin inference attempt + reserve usage + compare current cost / restore gap for applicable cap scope × window
→ commit
→ dispatch 直前の UTC window / applicable cap scope × window の current cost・restore gap / current cap revision・残枠 gate
→ provider I/O（gate と同じ window でのみ開始）
→ usage/cost settlement
```

同じ transaction で少なくとも次を行います。

1. existing AU14 consent / credential / Task premise / erasure currentness compare。
2. 実課金先に対応する復元済みcap設定と、§15.1 の復元前cap由来の短命な安全条件を比較して現在適用するscope × windowに限り、現在費用と `cost_restore_gap` の欠損期間・照合事実を照合する。該当cap windowがunknownならattempt / reservationを作らずprovider 0 byteで保留する。安全条件はcap設定としてマージしない。capがないwindowはgapだけで拒否しない。
3. applicable provider + system cap の current revision / amount と、復元前cap由来の未解除の短命な安全条件が対象とするwindowの暫定境界を読む。
4. current window の `Reserved + CommittedReported + CommittedUnknown` を集計する。
5. new upper-bound reservation を加えた合計がすべての適用capと暫定境界以下であることを確認する。
6. inference attempt と reservation を commit する。

cap 超過なら attempt も reservation も作らず provider は 0 byte です。これは reservation の `Released` ではありません。

日額と月額の候補は同じ admission 時刻から確定し、両 window の予約を不可分に計上します。ただし commit だけでは後日の送信権になりません。推論側が所有する dispatch gate は ready 済み provider transport の first-write 直前に、信頼できる UTC 時刻が予約の **両方** の window 内であることを確認します。gate 内の有界 first-write による開始までの上限を保証し、時計の誤差・境界までの余裕を差し引いても同じ window 内に開始できる場合だけ送信します。transport の内部キュー・接続待機では開始とせず、そこで待機した後は再検証します。有効期限は後続の clock 補正や境界到来で延長せず、再検証なしに非同期キューから送信してはいけません。時刻の信頼性、境界までの余裕、または送信開始までの上限を保証できなければ送信しません。送信開始を確認できた window を帰属とし、settlement が境界を跨いでも変更しません。

SQLite transaction が直列化するのは attempt claim・全適用capと復元前cap由来の暫定境界の残枠比較・予約、およびcap更新との順序までです。ネットワーク送信をtransaction内で行わず、commitと実際の送信開始を同一の原子的操作とみなしません。予約後・送信前にcapが下がる競合は次の短い publication gate で塞ぎます。接続と TLS を ready にしてから、credential / erasure の最終 gate と同じ共有 authority（[Credential Publication §4](credential-publication.md#4-並行する-scrub利用と失効)）の下で、同じ SQLite master の単一 writer が実課金先に現在適用されるcapと短命な安全条件のscope × UTC windowだけについて現在費用と`cost_restore_gap` / 照合事実を再評価し、すべてのcurrent cap revision・暫定境界・残枠（当該予約を含む）・restore cost hold、同意・credential・Task premise・消去条件を再比較します。適用cap windowの費用がunknownならprovider 0 byteで止めます。必要な配送・試行開始可能性を先にdurable commitし、SQLite transaction を解放したまま guard を保持して、ready 済み provider transport に有界の同期 nonblocking first-write（TLS record に対応する ciphertext の OS 書込を含む）を実行します。正の byte 受理または実際に開始した byte の不可逆 handoff を確認した時点を provider dispatch 開始として guard を解放します。内部 queue / TLS buffer への enqueue、書込予約、将来 flush するだけの引受けは開始ではありません。

cap変更・作成・解除、restore gap の照合・解除・失効、安全条件の明示解除、credential 更新と deletion start の全書き手も同じ gate と単一 writer を通り、DB commit と first-write の間に割り込めません。プロセス内 mutex のみで別 writer の commit を遮断したことにはしません。cap更新が先にcommitした場合は新revision / limitで評価し、旧予約を含む合計が現在の上限を超えるならprovider 0 byteで止めます。first-write が先ならその時点のcap / UTC window に帰属し、後続更新は既開始の消費を取り消したことにせず、未開始 byte は次の gate で止めます。guard / SQLite transaction 内で `.await`、接続・handshake・背圧待ち、blocking OS / TLS I/O は行わず、guard 内の I/O は上記の有界 nonblocking first-write のみとします。EAGAIN 等で未開始が確定したら guard を解放し、待機後に全条件を再比較します。正の部分書込は開始済みとして元 ticket に帰属し後続 byte は再 gate、結果不明は元予約を `CommittedUnknown` として保持し自動再送しません。物理ネットワーク到達や provider 応答を guard 中に待たず、first-write は到達保証ではありません。

UTC window または dispatch gate に間に合わなければ、transport がまだ request を引き受けていない、または送信不能に fence して provider 0 byte を客観的に確定した場合だけ、元の reservation を `Released` にし、元の attempt を `NotSent` として終えます。未送信 terminal 化と release は同じ短い transaction で確定し、その後に旧 ticket からの送信を許しません。元の attempt / ticket を再利用せず、新しい attempt と reservation の短い transaction で **現在の** UTC 日額・月額 window、実際の課金先に適用される cap window の current cost / restore gap、全適用 cap・cost hold、同意・認証情報・Task premise・消去条件を再照合してから次の dispatch を明示的に試みます。元の reservation の release と新規 claim / cap 比較は同一 SQLite master の書き手として直列化し、古い枠から新しい枠へ単に付け替えたり、一度の cap 判定を持ち越したりしません。送信が始まった可能性を否定できなければ release も自動再試行もせず、停止を best effort で試み、元の window の upper bound を保持して `CommittedUnknown` とします。

```rust
enum UsageAdmissionOutcome {
    Reserved(UsageReservationRef),
    HeldByCap(UsageCapRef),
    HeldByRestoreCostUnknown,
    StaleCapRevision { current: UsageCapRef },
}
```

実装では既存 `AttemptBeginOutcome` / `NotSentReason` へ owner-specific mapping を追加できますが、cap refusal と適用 cap window の restore cost unknown を互いに、また consent stale / technical storage failure / erasure hold と同じ意味に潰しません。

## 10. concurrent admission

provider cap と system cap は同じ SQLite master の短い transaction 内で比較するため、残り1枠に対する2つの concurrent send が両方通ることはありません。

applicable cap が複数ある場合は **すべて**を満たす必要があります。provider cap と system cap のどちらか一方だけを選んで評価しません。

process-local mutex / cached running total は authority ではありません。

## 11. settlement

provider completion が得られたら inference owner が token usage と cost を確定します。

### 11.1 Reported

provider が valid な complete usage を返した場合:

- token usage = `Reported`
- cost = admission 時の pricing snapshot で `Reported`
- reservation = `CommittedReported`
- cap accounting は reserved upper bound ではなく actual committed cost を数える
- unused reservation amount は同じ settlement で解放される
- settlement が翌日・翌月に届いても送信開始時の window の予約を実績へ置き換え、settlement 時の window へ二重計上しない

### 11.2 Unknown

timeout、`ResponseLost`、Host crash、transport interruption 等で external consumption を否定できない場合:

- token usage = `Unknown`
- cost = `Unknown`
- reservation = `CommittedUnknown`
- cap accounting では reservation upper bound を維持する

provider output の adoption 可否と usage settlement は別です。reply / learning / Task result が stale で採用されなくても provider call が実行済みなら usage fact を失いません。

## 12. Release してよい境界

`Released` は provider I/O が開始されなかったことを owner が証明できる場合だけです。

例:

- 予約 commit 後の request construction が provider transport へ渡る前に明確に失敗し、外部 I/O が 0 であることが型で区別できる場合
- UTC window が変わった、または dispatch 前の時刻を証明できず、transport が送信未開始（provider 0 byte）と確認できる場合。この attempt は `NotSent` として終え、新規 attempt で再照合する

cap / consent / credential / erasure / Task premise の **claim 時の** pre-send refusal は attempt / reservation を作らず、release 対象もありません。既存 reservation の後に送信前の拒否が確定した場合だけ、送信未開始を証明して release します。

provider transport error の文字列から「たぶん送信前」と推定して release しません。

post-claim error が NotStarted / MayHaveStarted を区別する必要がある場合は、transport outcome にその確定度を型として追加します。

## 13. cap update

cap mutation は first-party management boundary から permission / constraint owner へ届きます。

```rust
struct SetUsageCapCommand {
    expected: Option<UsageCapRef>,
    scope: UsageCapScope,
    window: UsageCapWindow,
    limit: Money,
}

enum SetUsageCapOutcome {
    StoredAs(UsageCapRef),
    Stale { current: UsageCapRef },
    InvalidLimit,
}
```

- LLM output、Task Agent、provider response は cap を自己変更できません。
- cap update と send admission は revision compare / same master transaction により直列化されます。
- cap update の commit と、既存 reservation の provider nonblocking first-write も §9 の共有 gate / 単一 writer で直列化します。cap を下げた後に旧上限の予約だけで新規送信しません。
- cap を上げる操作も trusted first-party control として扱います。
- 復元新世代では cap の新規作成・変更・解除・scope の付け替え・provider ID の変更も、§15.1 の復元前cap由来の短命な安全条件・復元済みcap設定と `cost_restore_gap` の該当 scope × window 判定を同じ短い不可分境界で行います。未照合の適用 window を hold できない場合は変更を拒否し、cap だけを公開しません。cap の増額や名称変更、同じ外部課金先を別の provider ID として登録する操作で既存の安全条件・gap 判定を省略しません。復元前cap由来の安全条件の解除は明示的な Owner 操作の事実を durable に確定し（復元後の一括有効化と別）、復元による行の欠落を解除とみなしません。解除は復元済みcap設定の変更でも費用0の証明でもありません。

短命な復元安全条件の明示解除は対象restore generation・実課金先・scope・UTC windowを指定する別のfirst-party commandとし、cap設定変更や復元後の一括有効化の副作用にはしません。現在の対象条件が一致する場合だけ同じcap admission authorityで解除事実を確定し、残る復元済みcapの費用holdとreservationの`Unknown`を維持します。

## 14. persistence

最低限、durable state は次を区別します。

- inference attempt ↔ usage reservation correlation
- token usage fact
- pricing snapshot
- actual / unknown cost fact
- usage reservation state
- cap definition + revision
- 復元新世代の cap scope 一覧から独立した非秘密 `cost_restore_gap`（バックアップ対象時点から新マスター公開までの欠損期間・信頼度、復元時点の UTC day / month window、世代）と、実課金先に紐づく scope × window ごとの current cost 照合事実、切替前に確保した復元前capの有効・解除済み状態を表す短命なrestore safety context（cap定義の別マスターではない）、その明示的なOwner解除事実、および適用cap windowに対するhold

現在の `usage_fact(ticket PK, provider, model, input_tokens, output_tokens, source)` は Stage 6 で上記 contract を満たすよう拡張します。

同じ意味を `usage_fact_v2` / `cap_cache` / `estimated_cost_cache` のような parallel source of truth に分けません。

## 15. restart / recovery

Host startup は terminal でない usage reservation を再評価します。

- `Reserved` のまま残った record は `CommittedUnknown` として確定する。送信未開始が証明された release は crash 前に `Released` として commit 済みであり、単に window が変わったことや dispatch gate の in-memory 状態を証明に使わない。
- crash を理由に reservation を release しない。
- historical pricing snapshot を current catalog へ付け替えない。
- `CommittedReported` / `CommittedUnknown` を再計上しない。
- cap current totals は durable facts から再構成する。memory counter の復元を authority にしない。
- 復旧時の `CommittedUnknown` も元の予約 window に帰属させる。復旧時刻の window へ移さない。

### 15.1 backup restore 後の current cost

[Backup / Restore §4](../critical-areas/backup-restore.md#4-現在stateと復元stateの分離staleの扱い) に従い、復元した usage / cost / reservation は historical ledger として保持します。バックアップ作成後も provider 側の課金は巻き戻らず、その間の同じ UTC 日額・月額 window に属する実費はバックアップに欠け得ます。復元した ledger の合計だけを現在の残枠とみなしたり、欠損分を 0 にしたりしません。

restore owner と permission / constraint owner は、切替前に復元前capの有効・解除済み状態、実課金先、scope、limit、対象UTC day / month windowを非秘密の短命なrestore safety contextとして確保します。cap設定自体はバックアップの内容で**全置換**し、復元前のcap定義を新マスターのcap設定として復元・マージしません。解除済みだった復元前capは再有効化しません。有効だった復元前capと復元済みcap設定を実課金先・scope・windowごとに比較し、対象windowにはより厳しい境界を暫定評価・保留します。このcontextは二つ目のcapマスターでも費用0の証拠でもなく、Ownerの明示的な解除判断（復元後の一括有効化とは別）または対象UTC window終了まで保持します。解除しても復元済みcapの費用不明holdやreservationの`Unknown`は消えません。復元前capの状態を確保できない・適用scopeを特定できない場合は、適用され得るscope/windowを安全側に分類してholdし、無関係な全providerを一律に停止しません。

ステージング中は旧マスターのcap変更を受け付けるため、早期に取得したcontextは切替の証拠にしません。復元の最終公開点では、旧マスターの現行cap revision・有効/解除済み状態と切替用contextの由来を、cap更新と同じauthorityの短い直列化区間で再比較します。差異があれば公開を見送り、変更後の現行状態からcontext・対象windowのholdを作り直します。比較から新マスターの単一マスター切替までの区間にcap更新を割り込ませず、切替後の変更は新マスターへだけ適用します。contextのdurable準備や重いステージングI/Oをこの区間に入れず、準備済みcontextのrevisionを最終比較してから短い公開を確定します。旧マスターの現行状態を読めない場合も、早期snapshotを現在値と推定せず、該当し得るscope/windowを保留します。切替前のcrashでは旧マスターが正本であり、未公開contextを新しいcap設定として使いません。

最終比較では信頼できる現在 UTC 時刻に対応する day / month window も再照合します。準備後に window が替われば、旧 window の安全条件を新 window のものとして流用せず、該当する新 window のcontext・gap・holdを準備し直してから公開します。

新マスター公開時に、バックアップ対象時点から復元切替までの欠損期間と、切替時点のUTC day / month windowを、cap scope一覧から独立した新世代のdurableな非秘密`cost_restore_gap`として公開します。対象時点・欠損期間を信頼して限定できない場合も不確かさを記録し、重複し得るwindowだけをunknownとします。gapと重なる未照合の適用cap windowにはdurable holdを作ります。切替とgap・context・比較・該当holdの公開は一つの成立条件とし、再起動を跨いで新マスターだけが有効になる状態を許しません。通常の新DB作成・restartはgapもrestore holdも生成しません。復元前の旧マスターにあるbackup後のaction / usage rowsを新マスターへ取り込んで補正せず、復元前の遅延結果も新しいticketのsettlementに転用しません。

cap が存在しなかった課金先もgapの記録対象から除外しません。ただしすべてのprovider-send admissionとtransport開始直前gateが現在費用・gapと照合するのは、実課金先に現在適用される復元済みprovider / system capまたは未解除のrestore safety contextが対象とする**種別・scopeに対応するUTC windowだけ**です。両者とも対象にしないwindowはgapだけを理由に送信を止めません。日額のみ適用中なら翌日の非重複dayは月額のunknownに左右されず送信でき、月額が適用中なら当月のunknownは日替わりだけでは解除されません。実課金先を同定できず適用capの判定を保証できない場合は該当し得るscope/windowを保留します。復元後にcapを作成・変更・付け替えする際は、新たに適用される各scope × windowとgapを同じ短い不可分境界で照合し、未照合ならdurable holdを作るか変更を拒否します。既存holdのない新provider scope、cap増額・再設定、名称変更・同じ実課金先の別IDへの付け替えでも同じです。admission・dispatch gateはgap / 照合事実とcurrent cap / safety context / holdを同じauthorityで照合し、cap更新と送信開始の順序で抜け道を作りません。

admission と §9 の dispatch gate は、現在適用されるcap windowのcurrent costがunknownなら新規provider送信を0 byteで保留します。既存reservationが復元前から存在する場合も送信権とはみなしません。復元後の一括有効化、capの増額・再設定、復元した過去実績の再集計、cap未設定という事実は費用0の証拠でもrestore safety contextの解除判断でもありません。費用unknownの解消には、該当する実際の外部課金先を含むscope / windowの**現在の**外部課金額、または課金発生がなかったことを客観的に証明し、未報告・結果不明・進行中の消費分も安全な上限で扱う必要があります。証明できたscope / windowだけ照合・費用hold解除事実をdurableに確定します。適用capには証明した現在額と未確定分を基準に残枠も確定し、復元後の新しい予約・実績を重複なく加えて以後のcap判定を行います。後から作ったcapにも、そのscope / windowの証明が適用できる場合だけ費用holdを省けます。証明不能な適用windowは当該UTC day / monthが終了するまで費用holdを維持し、終了を信頼できるUTC時刻で確認して初めてそのwindowのunknown判定とholdを失効できます。日額の失効は適用中の同じ月の月額unknown / holdを解除せず、gapと重ならない新windowは独立して判定し、復元由来の保留を作りません。失効後は新windowの判定に架空の消費を加えず、gapの記録自体もcapの付け替えによる解除根拠にしません。照合・安全条件の明示解除・失効・新規reservationは同じcap admission authorityで直列化し、現在適用されるcap windowがすべて利用可能と確認できる場合だけ送信します。復元済みcapも有効な短命の安全条件もない場合はgapのunknownだけで送信を止めません。usage reservation / external effectのUnknownはゼロや自動再送許可に変えません。

## 16. bounded first-party query

Stage 7 UI が private DB schema を知らずに利用できる first-party read boundary を Stage 6 で用意します。

query は少なくとも次で filter / page できます。

- time range
- provider
- model
- consumer / purpose
- status（Reported / Unknown / Reserved）

返却する summary:

- input / cached-input / output token
- cost component / total
- provider/model
- Reported / Unknown / Reserved の区別
- current provider/system cap
- 現在適用されるcapのscope × windowごとのconsumed / reserved / unknown / remaining / held state。復元前cap由来の短命な安全条件による暫定境界・保留と、復元済みcap設定・費用unknownの保留を区別する（いずれもないwindowのgapは送信保留と表示しない）

query は SELECT-only です。read によって reservation settlement、pricing refresh、cap mutation を行いません。

body text、prompt/output、credential value を usage view に含めません。

## 17. first-party control surface

cap set/update は bounded read と分離した command です。

Client-side UI selection は authority ではなく、Host が current authenticated connection、base view、cap revision、trusted confirmation requirement を再照合します。

Stage 7 GUI はこの query/command を利用し、`usage_fact` / `cap_limit` を直接読み書きしません。

## 18. Stage 6 slice 対応

| Slice | 実装範囲 |
|---|---|
| B1 | input / output / cached token accounting と ticket attribution |
| B2 | pricing snapshot と reproducible cost projection |
| B3 | reservation lifecycle、safe upper bound、provider/system daily/monthly cap、crash recovery |
| B4 | bounded first-party usage/cost/cap query と cap control surface |

B1/B2 は B3 より先に review できますが、B3 の actual settlement は B1/B2 の fact / pricing contract を利用します。

## 19. 必須 race walkthrough

### U1: concurrent calls / remaining one slot

```text
Call A reserve+cap compare ──commit──→ provider I/O
Call B reserve+cap compare ──────────→ sees A reservation → HeldByCap
```

両方が provider I/O を開始する第三の順序は許しません。

### U2: cap update vs admission

```text
old cap under admission transaction → admission commits → later cap affects next call
```

または

```text
cap update commits → admission reads new cap → new cap decides
```

古い cached cap judgement で third ordering を作りません。

予約後・送信前の更新でも同様です。cap update が先に commit したら dispatch gate は新しい上限と旧予約を比較し、超過なら 0 byte で拒否します。正の first-write が先なら後の更新はその送信を遡って未送信にはしません。送信開始が不明なら release せず Unknown にします。

### U3: crash after reservation

```text
attempt + reservation commit
→ crash before/during provider I/O
→ restart
→ external consumption を否定できない
→ CommittedUnknown
→ upper bound remains counted
```

### U4: admission 後・dispatch 前の日・月境界

```text
window A で claim + reserve → window B に変わる → gate は A での送信を拒否
→ provider 0 byte を確認 → A の予約を Released、元の attempt は NotSent
→ 新しい attempt で B の日額・月額と全適用 cap / current premise を再評価
→ 成立した場合だけ B で送信開始
```

provider 0 byte を証明できない場合は A の予約を Unknown として保持し、自動再送しません。

### U5: 送信開始後の日・月境界と settlement

```text
window A で reserve + dispatch 開始 → window B へ移る → window B で新しい call を admission
→ 古い call が Reported / Unknown として settlement
→ 古い call の予約・実績は window A のみ、新しい call は window B のみへ計上
```

window A の送信開始済み予約を境界到来だけで release したり、遅れた settlement を window B に加えたりしません。

### U6: backup 後の消費を含む window への restore

```text
window A で backup → 同じ window A で外部課金 → 古い backup を restore
→ cap scopeと独立した新世代のcost_restore_gapと、切替前に確保した復元前cap由来の短命な安全条件・全置換された復元済みcapを比較して公開し、Aに重なる適用capのscope × windowをhold（復元前capは新マスターの設定にマージしない）
→ cap のない課金先は gap の unknown だけでは止めず、日額のみなら翌日の非重複 day に再開可能
→ A 中に cap を追加・付け替えする場合も、変更時に該当 window の gap から hold を生成または拒否
→ 一括有効化後も適用 cap window が unknown のままなら provider 0 byte
→ 実課金先の現在額または課金なしを客観的に証明できた scope / window だけ解除
→ 照合不能なら適用される日・月それぞれの window 失効まで保留
```

復元 ledger の値を現在残枠の証拠として使わず、旧マスターの rows を新マスターへ混ぜません。

## 20. 完了条件

Stage 6 の usage / cost / cap は次をすべて満たした場合に完成です。

1. dialogue / learning / Task Agent の provider call が同じ ticket-based accounting path を通る。
2. input / output / cached input token の Reported / Unknown が durable。
3. cost が利用時点の immutable pricing snapshot から再現できる。
4. provider/system daily/monthly cap が provider I/O 前の reservation transaction と dispatch 直前の UTC window / current cap gate で強制される。
5. concurrent admission で cap を oversubscribe しない。
6. crash / response lost を 0 usage と推定しない。
7. historical price change で過去 cost が変わらない。
8. first-party bounded query から token / cost / cap state を確認できる。
9. first-party cap update が currentness / authority を通る。
10. usage/cost/cap records に prompt/output/credential secret を保存しない。
11. restore gapをcap scopeとは独立にdurableに公開し、復元前capを切替前に短命な非秘密の安全条件として確保して復元済みcap設定と比較する（新マスターのcap設定はバックアップで全置換）。Ownerの明示解除は復元後の一括有効化と区別し、確保不能なら適用され得るscope/windowのみ保留する。現在適用されるcapのscope × UTC windowだけをadmissionと送信開始直前に費用照合し、既存・後から設定するcapの該当holdを客観的な照合またはwindow失効まで維持する。新cap・ID替えでもgapを照合し、capがないwindowのgapのみでは送信を止めない。reservationのUnknownは維持する。
