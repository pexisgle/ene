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
}
```

`UsageReservationRef` は permission / constraint の cap-admission boundary が所有します。inference は returned ref を durable attempt correlation として保持できます。

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
- Host crash 後に残った `Reserved` は「使っていないはず」と推定せず、recovery で `CommittedUnknown` へ確定します。
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
→ begin inference attempt + reserve usage + compare all applicable caps
→ commit
→ provider I/O
→ usage/cost settlement
```

同じ transaction で少なくとも次を行います。

1. existing AU14 consent / credential / Task premise / erasure currentness compare。
2. applicable provider + system cap の current revision / amount を読む。
3. current window の `Reserved + CommittedReported + CommittedUnknown` を集計する。
4. new upper-bound reservation を加えた合計がすべての cap 以下であることを確認する。
5. inference attempt と reservation を commit する。

cap 超過なら attempt も reservation も作らず provider は 0 byte です。

```rust
enum UsageAdmissionOutcome {
    Reserved(UsageReservationRef),
    HeldByCap(UsageCapRef),
    StaleCapRevision { current: UsageCapRef },
}
```

実装では既存 `AttemptBeginOutcome` / `NotSentReason` へ owner-specific mapping を追加できますが、cap refusal を consent stale / technical storage failure / erasure hold と同じ意味に潰しません。

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

- cap / consent / credential / erasure / Task premise の pre-send refusal
- request construction が provider transport へ渡る前に明確に失敗し、外部 I/O が 0 であることが型で区別できる場合

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
- cap を上げる操作も trusted first-party control として扱います。

## 14. persistence

最低限、durable state は次を区別します。

- inference attempt ↔ usage reservation correlation
- token usage fact
- pricing snapshot
- actual / unknown cost fact
- usage reservation state
- cap definition + revision

現在の `usage_fact(ticket PK, provider, model, input_tokens, output_tokens, source)` は Stage 6 で上記 contract を満たすよう拡張します。

同じ意味を `usage_fact_v2` / `cap_cache` / `estimated_cost_cache` のような parallel source of truth に分けません。

## 15. restart / recovery

Host startup は terminal でない usage reservation を再評価します。

- `Reserved` のまま残った record は `CommittedUnknown` として確定する。
- crash を理由に reservation を release しない。
- historical pricing snapshot を current catalog へ付け替えない。
- `CommittedReported` / `CommittedUnknown` を再計上しない。
- cap current totals は durable facts から再構成する。memory counter の復元を authority にしない。

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
- current window consumed / reserved / unknown / remaining / held state

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

### U3: crash after reservation

```text
attempt + reservation commit
→ crash before/during provider I/O
→ restart
→ external consumption を否定できない
→ CommittedUnknown
→ upper bound remains counted
```

## 20. 完了条件

Stage 6 の usage / cost / cap は次をすべて満たした場合に完成です。

1. dialogue / learning / Task Agent の provider call が同じ ticket-based accounting path を通る。
2. input / output / cached input token の Reported / Unknown が durable。
3. cost が利用時点の immutable pricing snapshot から再現できる。
4. provider/system daily/monthly cap が provider I/O 前の reservation transaction で強制される。
5. concurrent admission で cap を oversubscribe しない。
6. crash / response lost を 0 usage と推定しない。
7. historical price change で過去 cost が変わらない。
8. first-party bounded query から token / cost / cap state を確認できる。
9. first-party cap update が currentness / authority を通る。
10. usage/cost/cap records に prompt/output/credential secret を保存しない。
