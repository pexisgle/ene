# Host↔Client IPC / wire protocol の具体設計 — Step 13 Concrete Design

本書は Step 13 の Host↔Client IPC / wire protocol artifact である。Step 11（Context Assembly, Action Execution, Targeted Deletion, Client Presence Transition, Backup / Restore）および Step 12（Subsystem Decomposition, 各 subsystem 詳細設計, State Ownership (SO), Dependency Rules (DR), Cross-cutting Design (CC), Runtime Topology, Runtime Flows, System Context）と Step 13 の[対応関係・識別](correspondence-identity.md)（CI）、[Persistence / Recovery](persistence-recovery.md)（PR）、[Concurrency Control](concurrency-control.md)（CCT）、[Interface Boundaries](interface-boundaries.md)（IB）、[Crate / Module 分解](crate-module-decomposition.md)（CM）を**固定前提**とする。semantic ownership、identity / revision / generation / directed correlation / boundary token の意味、durable / derived / transient / external 分類、atomicity / ordering boundary、recovery semantics、serialization domain、compare-before-commit、caller ≠ authority、typed expected、domain outcome / technical error 分離、secret 非返却、Host-local / remote-capable の区別、crate 依存方向のいずれも変更しない。変更が必要に見えた場合は設計で吸収せず Issue として報告する（第29節）。

製品挙動の source of truth は[要件 Baseline](../../requirements/README.md)、[製品定義](../../requirements/product.md)、[要件](../../requirements/requirements.md)とする。[受け入れ条件](../../requirements/acceptance.md)も検証範囲へ含め、[参考資料](../../requirements/references.md)は非規範として扱う。既存実装から製品挙動を補わない。

実装コードは変更しない。本書の Rust pseudo-type はコンパイル対象ではない。型名・field 名・message 名の同義改名は許すが、型の分離と field の意味は維持すること。

## 0. 固定 premise（再掲。変更しない）

1. **Host が canonical authority。** canonical state と control authority は Host が持つ。Client は canonical persistent state holder ではない。
2. **Client-originated message は authority ではない。** Permission / presence / Task / Action 等の確定にはならない。Host の現在条件との照合を経て初めて受入可否が決まる。
3. **Host-local は IPC へ公開しない。** Host-local な Permission 確定、Credential 利用、Provider assignment、cost reservation、Action 認可・outcome 確定、repository compare、Restore switch 等は wire へ出さない。Client へ送るのは必要表示と、照合のための最小 correlation だけである。
4. **Client は Host authority crate へ直接依存しない。** Client が依存してよいのは `ene-api`（wire DTO）と `ene-primitive`（opaque 性質）のみである（CM 第9節）。
5. **`ene-api` は wire DTO のみ。** business logic・authority 判定を置かない。必要なら極小の opaque primitive のみ共有し、Host domain object そのものを公開しない。
6. **Currentness を失わない。** CI / CCT の identity・revision・generation・typed correspondence・expected current relation・stale / delayed handling を wire 境界でも維持する。ただし Host 内部の boundary token 全体を Client へ渡さない。Client が保持・返送する最小限の opaque / typed correlation だけを選ぶ（第6節）。
7. **後方互換は制約ではない。** AGENTS.md および要件（非目標）により、既存 config / IPC / Plugin protocol / 保存形式 / CLI との互換性維持は要求しない。clean な設計を優先する。

## 1. 対象と非対象

### 1.1 今回具体化するもの

- Host↔Client 間を越える必要がある semantic interface の選別（第2節）。
- protocol layer 構成（wire semantic と transport の分離、envelope と payload の分離）（第3・5節）。
- interaction pattern の区別（第4節）。
- message / domain identity の分離と correlation（第6節）。
- Client incarnation / stale rejection（第11節）。
- Presence / Text / Voice / presentation（第12・13節）。
- Observation（第14節）。
- Client-dependent Action（第15節）。
- Cancellation（第16節）。
- Targeted Deletion 参加（第17節）。
- Management surface（第18節）。
- Body / presentation resources（第19節）。
- wire DTO pseudo-code（第21節）。Host domain type の derive Serialize 公開はしない。DTO → domain command への変換点を明示する。
- serialization 選択と比較（第7節）。format 選択だけで compatibility が成立するとは仮定しない。
- protocol versioning / compatibility（第7節）。
- capability negotiation（第8節）。claim ≠ Permission。
- authentication / pairing（第9節）。具体暗号 library・key format は固定しない。秘密 material を通常 payload へ載せない。
- transport 範囲と adapter boundary（第10節）。
- backpressure / streams（第22節）。global total ordering を要求しない。重要 control と高頻度 capture を同じ drop policy にしない。
- security（第23節）。typed DTO を boundary の代替にしない。
- error / rejection model（第24節）。transport error と domain rejection を潰さない。
- crate placement（第25節）。`ene-api` に logic を置かず、mapping module を別配置にする。
- walkthrough による検証（第26節）。transport success を domain success へ読み替えない。

### 1.2 今回決めないもの（Design Freedom へ送る。第30節）

- 具体暗号 library・key format・鍵導出・証明書運用の詳細（property は第9節で固定）。
- heartbeat / keepalive / timeout / retry 回数・値、scheduling・capture 時機 algorithm、費用算定式。
- 具体 TCP port・mDNS 有無・NAT traversal・relay（relay は非目標のまま導入しない）。
- audio codec・capture 画像形式の最終選定（制約条件のみ第13・14節で固定）。
- UI layout・表示文言・具体保持期間・audit format。

## 2. Remote-capable 選別 — 何を wire へ出すか

IB 第15節・CM 第11節の remote-capable 7 群を起点に、「network / process boundary を越えなければ成立しないか」で再判定した。固定リストの機械的 protocol 化はしない。

### 2.1 越境させるもの（wire 化する）

| # | wire 群 | 越境する理由 | 対応する IB interface | 越境させる内容（最小） |
|---|---|---|---|---|
| W-1 | Owner Text / Voice input・presentation | Client が入力・表示の一時表現だけを持ち、Host 正本を持たないため | X-B（round 受付・提示・区切り） | input candidate、round 参照、stream frame、提示確認。会話の意味・History・Learning は出さない |
| W-2 | Companion presence / movement | 呼出し・移動意図は Client から届くが、成立は Host の帰属記録のため | X-A（移動・復帰） | move intent、transition ack、attribution fact。hint・復旧先だけでは成立させない |
| W-3 | undelivered presentation | 次 Client での要約報告は Host の元記録へ照合した派生表現のため | X-H / H-G（未伝達報告） | filtered summary、提示確認。元 Task・活動記録の正本は出さない |
| W-4 | Client availability / device capability | Host が presence・observation・Action 可否の前提として Client 側事実を必要とするため | X-D・X-F の観測事実部分 | availability fact、capability advertise/update。Permission にはならない |
| W-5 | Observer eligibility に必要な Client 側情報 | 対象・時機の判断材料が Client 側（fullscreen・pause・負荷等）にあるため | X-D（eligibility 連動） | eligibility 表示・状態 fact。Raw・候補・routing 文脈は出さない |
| W-6 | shared observation / capture の Client 側入出力 | Capture 実行者が Client であり、Host が ticket で統制するため | X-E（routing の両端のみ） | capture ticket、capture frame（candidate）、受入結果。routing semantic・専用 assignment は出さない |
| W-7 | Body / presentation resources | 表示資材の供給元が Host（Character 静的資材）で利用者が Client のため | C-A の資材利用分 | asset descriptor＋binary chunk。適用関係・経験状態は出さない |
| W-8 | Client-side Targeted Deletion 参加 | 接続中 Client の一時 data も holder として参加するため | D-B の Client 宛て分 | deletion demand（本文なし）、local result。対象本文・検索 token の永続化はしない |
| W-9 | Owner 管理面の入口と表示 | 管理操作の入口が Client にあっても確定は各 owner のため | 第9節 `ManagementOperationCommand` の Client 側入口 | management intent（candidate）、filtered view。control 正本・secret・判定 copy は出さない |
| W-10 | Client-dependent Action（Computer Use 等）の遂行 | 作用の実行者が Client device であるため | K-J（Client 限定）＋ K-H の Client 向け投影 | action command（concrete device op のみ）、receipt ack、progress、effect report。認可判断・許可条件は出さない |

### 2.2 越境させないもの（Host-local に留める）

- H-B〜H-E の形成・訂正・scope 意味判断、K-A〜K-C の制御確定・秘密利用、K-D〜K-G の割当解決・送信条件・予約確定、K-H の認可・作用確定、D-A・D-C・D-D の範囲確定・完了確定・switch、第13節の repository compare-and-commit 群。理由は IB 第15節・CM 第11節のとおり。durable compare を Host 単一 SQLite transaction で不可分にするため、DB transaction を Client へ露出させない。秘密値を通常経路に載せない。
- Observer 専用 Provider assignment・routing semantic そのもの。Client へ authority として公開しない。Client が見るのは ticket と自身の capture 受入結果だけである。
- repository compare、cost reservation、Restore switch。wire へ出さない。

### 2.3 選別の帰結

- Client が受け取る ID はすべて**用途限定参照（non-secret reference）**である。内部正本の主 key として再利用できる形で渡さない（CI §4.6、CM §4.2）。
- Client が保持・返送する correlation は第6節の最小 set に限定する。Host 内部の boundary token 全体（Task revision 前提・委任 scope・Permission evaluation・消去条件・復元条件の全文）は渡さない。Client が返すのは「どの wire 参照について」「Host が見たどの世代表示を前提にしたか」だけであり、Host が current と再照合する。

## 3. Protocol layer 構成

wire semantic と transport を分離する。envelope（routing / compatibility）と domain semantic payload を分離し、envelope を semantic owner・authority にしない。

```mermaid
flowchart TB
  subgraph Client["Client (presentation / input / device / observation)"]
    CApp["app (ene-stage / ene-ctl)<br/>presentation・capture・device adapter"]
    CMap["client ipc adapter<br/>DTO ↔ 表示・device"]
  end
  subgraph Wire["wire (versioned, MessagePack canonical)"]
    Env["wire envelope<br/>(routing / compat のみ)"]
    Pay["domain payload<br/>(typed message)"]
    Blob["binary attachment<br/>(audio / capture / asset chunk)"]
  end
  subgraph Transport["transport adapter"]
    Local["local socket<br/>(same-machine)"]
    WS["WebSocket+TLS<br/>(LAN / VPN remote)"]
  end
  subgraph Host["Host (canonical authority)"]
    HTrans["transport adapter<br/>(frame I/O・liveness)"]
    HAuth["connection auth<br/>(pairing・session)"]
    HMap["host ipc mapping<br/>(DTO ↔ domain premise)"]
    Dom["domain owners<br/>(Companion / Task / Presence /<br/>Observer / Action / Preservation...)"]
  end

  CApp <--> CMap
  CMap <--> Env
  Env <--> Pay
  Pay <--> Blob
  Env <--> Transport
  Transport <--> HTrans
  HTrans <--> HAuth
  HAuth <--> HMap
  HMap <--> Dom
```

層の責務：

| 層 | 持つもの | 持たないもの |
|---|---|---|
| transport adapter | frame I/O、liveness（heartbeat）、backpressure の伝達、peer 切断検知 | domain 意味・presence・許可・世代の判断。liveness を presence・報告完了にしない |
| connection auth | pairing・session・revoke・re-auth（第9節） | domain 許可・Task 反映・作用確定 |
| wire envelope | routing（recipient hint・stream mux）、compatibility（version・message type・correlation ID）、duplicate suppression key（第5・6節） | semantic owner・authority。envelope の存在・到着を確定にしない |
| domain payload | typed message（第4節の pattern 別）。Client は candidate / observation / ack を送り、Host は fact / decision 投影・command を送る | Host-local の確定条件全文・secret・判定 copy |
| binary attachment | audio frame・capture frame・asset chunk の byte 列（descriptor と対応付け） | 意味。descriptor なしの attachment を解釈しない |
| host ipc mapping | DTO validation、wire ref → domain premise への mapping、domain fact → DTO 投影（第25節） | 採否・達成・許可・確定度の判断（各 owner） |
| client ipc adapter | DTO → 表示・device 操作、device fact → DTO | canonical mutation・正本保持・authority 宣言 |

## 4. Protocol model — interaction pattern

すべてを一つの generic Event message へ統合しない。transport framing で共通 envelope を使っても semantic type は domain ごとに区別する。

| pattern | 意味 | 応答の約束 | 使う domain（例） |
|---|---|---|---|
| request / response | 問合せと回答。回答はその時点の view であり、後続利用の許可ではない | `request_id` で 1:1 対応。timeout は不明の原因であり、成功・失敗・未実行への書換えではない | pairing challenge、capability query（接続時以外）、management view 取得、asset descriptor 取得 |
| command + acknowledgement | Host→Client または Client→Host の「してほしい」と、その受付確認。ack は受信・記録であり、効果・完了ではない | `command_id` で対応。ack は `Received \| RejectedStale \| DeniedByHold \| Unsupported` 等の domain outcome。完了は別 message | move intent→transition ack、action command→receipt ack、deletion demand→local result（完了は Host 集約）、cancel request→cancel-received |
| one-way fact | authoritative 側からの一方向の伝達。受信は理解・採否を意味しない | 対応不要。最新値意味のものは supersede する | presence attribution broadcast、eligibility 表示、availability fact、revocation notice、body-state hint |
| subscription + notification | 購読登録と、その後の条件付き通知。購読は許可・presence ではない | `subscription_id` で管理。条件不成立・停止・失効で server 側が終了・保留を通知する | presence subscription、undelivered summary 更新、observation eligibility 変更 |
| stream (open / frame / close) | 順序付き frame 列。open が前提（ticket・session・round）を確定し、frame はその範囲内でのみ有効 | `stream_id`＋`seq`。close は `Completed \| Interrupted \| Cancelled \| Stale` を区別する。旧 stream の自動継続をしない | text token stream、voice audio stream、asset chunk stream |
| progress + completion | 長時間処理の中間報告と最終確定。progress は確定ではない | progress は `operation_id`＋単調 `progress_seq`。completion は certainty（成功 / 失敗 / 不明）を伴う | action progress→effect report、asset transfer progress→assembled、deletion local progress→local result |

pattern 横断の禁止：

- ack・受信・表示・保存成功を効果・完了・許可・報告完了にしない（CC-07）。
- subscription の存在を presence・許可・実行再開の根拠にしない。
- stream の open 成功を round・presence・許可の成立にしない。
- progress の到着を certainty の更新にしない。completion の `Unknown` を未実行・成功へ書き換えない。

## 5. Wire envelope — routing / compatibility 用

envelope は routing と compatibility のためだけに存在する。semantic owner・authority にならない。envelope の検証成功は payload の受入ではない。Host mapping は envelope 検証後に payload を validation し、domain premise へ mapping して各 owner の照合へ渡す。

```rust
// ene-api::v1::envelope（pseudo-code。wire DTO であり domain 型ではない）
struct WireEnvelope {
    protocol: ProtocolVersion,      // major.minor（第7節）
    message_id: WireMessageId,      // transport duplicate suppression key（第6節）
    correlation: WireCorrelation,   // request/response・command/ack・stream の対応付け
    sender: WireSender,             // device / incarnation / connection（第11節）
    observed: ObservedMarks,        // Client が見た世代表示の写し（主張ではなく照合材料）
    message_type: WireMessageType,  // payload の型識別（unknown type は拒否。第7節）
    payload: WirePayload,           // typed domain payload（第21節）
}

struct WireCorrelation {
    request_id: Option<RequestWireId>,  // request/response 用。Client 発行可
    command_id: Option<CommandWireId>,  // command/ack 用。発行者は方向別（第6節）
    stream_id: Option<StreamWireId>,    // stream 用。Host 発行が既定
    reply_to: Option<WireMessageId>,    // 応答対象 message（transport 対応用。domain 対応は ID 別）
    causation_span: Option<SpanWireId>, // 診断・追跡用。authority・順序の根拠にしない
}

struct WireSender {
    device_id: DeviceWireId,           // pairing 紐付け（第9節）
    incarnation_id: ClientIncarnationId, // Client process incarnation（第11節）
    connection_id: Option<ConnectionWireId>, // auth 後に Host が付与。auth 前は None
}

struct ObservedMarks {
    presence_generation_view: Option<u64>, // Client が見た presence generation 値の写し
    round_view: Option<RoundWireId>,       // Client が属するつもりの round
    ticket_view: Option<TicketWireId>,     // capture ticket 等の前提表示
}
```

envelope の扱い：

- `message_type` は routing の hint であり、payload の意味確定ではない。unknown type は `UnsupportedMessage` として拒否し、guess して処理しない。
- `observed` は Client の主張ではなく「Client が何を見て送ったか」の写しである。Host は durable・live と比較し、不一致なら stale として不受理にする。Client が自身を current だと宣言しただけでは成立しない。
- 秘密 material（pairing token・session proof・鍵）は通常 payload・envelope へ載せない。auth 専用 frame（第9節）でのみ扱う。
- 時刻は wall-clock＋作成時 timezone を保持する（CI §4.6）。時刻を revision / generation の代替・stale 判定の根拠にしない。

## 6. Message identity and correlation

### 6.1 識別子の分離

| 識別子 | 発行者 | 寿命・scope | 用途 | 再利用 |
|---|---|---|---|---|
| `WireMessageId` | 送信者（両方向） | 当該 message のみ。receiver cache は短期間（例：数分〜接続期間。値は Freedom） | transport duplicate suppression のみ | しない。新規送信・retry は新 ID |
| `RequestWireId` | request 送信者 | request/response 対 | request/response 対応 | しない |
| `CommandWireId` | command 送信者（方向別に発行者固定。第6.2節） | command→ack→completion の saga 範囲 | domain idempotency key（Host durable） | retry は同 ID＋新 `message_id` |
| `StreamWireId` | Host が既定（Client は提案のみ） | stream open〜close | frame の帰属。旧 stream の frame を新 stream へ付け替えない | しない。再接続で旧 stream を継続しない |
| domain wire ref（`CompanionWireRef`・`ClientWireRef`・`RoundWireId`・`AttemptWireRef`・`OperationWireId`・`TicketWireId` 等） | Host が既定（Client-local ID は別系列） | 用途別（round・attempt・ticket・operation） | wire 上の対応付け。Host が内部 identity へ mapping する | しない。削除後に再発行しない |
| `ClientInputLocalId`・`CaptureLocalId` | Client | Client-local。Host へ送るのは対応付け用のみ | Client 側の送信物と ack・結果の対応 | Client 内で単調。Host の正本にしない |
| domain identity（`CompanionId`・`TaskId`・`ActionAttemptId` 等） | 各 lifecycle の owner（Host） | durable | canonical 対応 | しない。**`MessageId` を domain identity として再利用しない** |

wire ref と domain identity の関係：wire ref は Host mapping が内部 identity へ解決する opaque な参照であり、内部 newtype の文字列表現ではない。Client は wire ref の内部構造を解釈・合成・推測しない。Host は解決不能な ref を `UnknownRef`（domain outcome）として不受理にし、guess しない。

### 6.2 発行者規則

- Client→Host の input・ack・fact・capability・availability の `command_id` / `request_id` は Client が発行する。Host は `command_id` を idempotency key として durable（短期間）に保持し、同一 `command_id` の再送には再実行せず prior outcome を返す。
- Host→Client の move transition・action command・deletion demand・capture ticket の `command_id` / `operation_id` / `stream_id` は Host が発行する。Client はこれらを minted しない。Client が command を合成・推測して送ることは protocol 違反として拒否する。
- retry の関係：transport retry（同一内容の再送）は**同一 `command_id`＋新規 `message_id`**で行う。receiver は `message_id` cache で重複配送を沈黙破棄し（再実行なし、ack 再送は可）、`command_id` の既処理記録で二重実行を抑止する。前者を transport duplicate suppression、後者を domain idempotency とし、混同しない。
- external effect retry（作用の再実行）は transport retry ではなく**新しい attempt / operation**とし、Owner 判断を経る（第15節）。transport の再送で作用が二重実行される設計にしない。

### 6.3 Round・presence generation・incarnation の区別

- round（会話の区切り）は Host 発行の `RoundWireId` で識別する。`message_id`・`request_id` とは別である。旧 round の入力・未提示出力を新 round へ付け替えない（CI §3.5）。
- presence generation は Host-authoritative な lifecycle 順序（`PresenceGeneration`）であり、Client は `observed.presence_generation_view` として写しを返すだけである。一致だけでは不十分であり、現接続・機能可用性・許可・停止・保留も Host が照合する（CI §5.6）。
- Client incarnation（第11節）は connection identity・Client instance・presence generation とは別の次元である。三者を一つの「session id」へ潰さない。

## 7. Serialization と protocol versioning

### 7.1 serialization 選択

| 候補 | debuggability | Rust / 他言語 | schema 進化 | 評価 |
|---|---|---|---|---|
| JSON（text） | ◎（そのまま可読） | ◎ | △（field 規約次第） | control の可読性は最良だが、audio・capture・asset の binary を含む stream で size・encode 効率が悪い。binary を base64 で包む設計は避けたい |
| MessagePack | ○（JSON へ機械変換可） | ◎（serde＋多言語 library） | ○（field 規約＋version と併用） | JSON-compatible な論理 model のまま byte 効率を得られる。既存 `ene-plugin-ipc` の frame 実績とも整合する |
| CBOR | ○（JSON へ機械変換可） | ○ | ○ | MessagePack と同系統。Rust・将来 Client 言語の library 普及と既存実績では MessagePack が優位 |
| Protobuf | △（decode なしに読めない） | ○（codegen が必要） | ◎（field 番号） | schema registry 的運用・codegen 配布・可読 log の追加機構が必要になり、単一 Owner-managed Host の topology には過剰。custom binary protocol も作らない方針と衝突する |

**採用：MessagePack を canonical wire encoding とし、論理 data model は JSON-compatible に保つ。** すなわちすべての DTO は JSON としても表現できる形（string・integer・boolean・array・map のみ。binary は attachment へ分離）に定義し、wire 上は MessagePack で encode する。debug・log・監査表示では JSON rendering を用いる。binary blob（audio・capture・asset chunk）は payload に base64 埋めせず、binary attachment frame として descriptor と対応付けて送る（第21節）。

format 選択だけで compatibility が成立するとは仮定しない。互換性は第7.2節の version・capability・field 規約で成立させる。

field 規約（encoding によらず成立）：

- optional field の追加は許す。receiver は unknown field を無視する（log への記録は可）。無視したことを理由に意味を変えない。
- required field の追加・意味変更・unit 変更・enum variant の意味変更は required semantic change とし、major version bump を必要とする。guess して処理しない。
- enum は open にしない。unknown variant を受信したら当該 message を `UnsupportedFieldValue` として拒否する。default variant への黙読替えをしない。
- ID・revision・generation・correlation は明示 field として serialize する。本文中の文字列を照合に使わない（CI §4.6）。

### 7.2 protocol versioning

- `ProtocolVersion { major: u16, minor: u16 }` を envelope に載せる。`major` は semantic 互換の境界、`minor` は optional 追加等の後方互換の範囲である。
- capability negotiation（第8節）で connection ごとに negotiated version を確定し、connection record に保持する。以後の当該 connection の message は negotiated version で解釈する。version の混在を一つの connection 内で許さない。
- unknown optional field → 無視して処理継続する。
- unknown message type → `UnsupportedMessage`（domain outcome）で拒否し、connection は維持する。当該 command の副作用を起こさない。
- required semantic change → major bump。理解できない major の message は処理せず、`IncompatibleProtocol` で拒否する。guess しない。
- Host newer / Client older → Host は negotiated（older major/minor の共通範囲）で話すか、共通範囲がなければ upgrade hint 付きで拒否する。older Client へ newer 意味を黙って送らない。
- Client newer / Host older → Client は Host の advertised max へ downgrade するか、対応不能として拒否する。Host は理解できない newer 意味を guess しない。

version compatibility matrix：

| 組合せ | 方針 |
|---|---|
| major 一致・minor 一致 | 通常処理 |
| major 一致・片方が minor older | older の理解範囲で処理。unknown optional field は無視。不足 field は older の送信では送らず、newer の受信では default 扱いにせず「省略」として扱う（「制約なし」への変換禁止） |
| major 不一致（共通 major なし） | 相互運用しない。`IncompatibleProtocol{ host_max, client_max, hint }` で拒否する。自動互換にしない |
| unknown message type（negotiated 範囲内） | 当該 message のみ `UnsupportedMessage` で拒否。connection 維持 |
| unknown enum variant / required field 不足 | 当該 message のみ拒否（`UnsupportedFieldValue` / `MissingRequiredField`）。他 message へ波及させない |

## 8. Capability negotiation

Client によって Body・Voice・screen capture・Computer Use・notification・platform integration 等の対応能力が異なる。connection 時に Client capability を Host へ伝える。

```rust
// ene-api::v1::capability（pseudo-code）
struct CapabilityAdvertise {
    supported_protocol: Vec<ProtocolVersion>, // 対応 version 一覧
    features: Vec<ClientFeature>,             // 機能名＋可否＋制約
    limits: ClientLimits,                     // frame 上限・同時 stream 上限等の申告
    platform: PlatformDescriptor,             // OS・device 種別等の表示用。許可の根拠にしない
}

struct ClientFeature {
    kind: ClientFeatureKind, // Body2D | VoiceDuplex | ScreenCapture | ComputerUse |
                             // Notification | TrayIntegration | ...
    available: bool,
    detail: FeatureDetailRef, // codec・解像度上限等の申告。検証なしに信用しない
}

struct NegotiatedConnection {
    version: ProtocolVersion,          // Host が選択した negotiated version
    accepted_features: Vec<ClientFeatureKind>, // Host が受領した申告（許可ではない）
}
```

- capability claim ≠ Permission。Host は availability（申告）と Permission / current presence 等を別々に照合する。申告があっても許可・presence・device 許可なしには開始しない。
- Host は申告を availability fact として保持し、Action 可否・observation 対象・routing・UI 表示の材料にする。申告の存在を能力の証明にしない（必要なら Host が到達性・device 許可と合わせて確認する）。
- capability の変化（fullscreen 開始・device 喪失・負荷・mute 等）は `CapabilityUpdate` fact / `AvailabilityFact` で通知する。変化前に発行した ticket・command の有効性は延びない（各受入で現在照合する）。
- negotiated version・accepted features は connection record に保持し、`RestoreGeneration`・presence generation とは別の次元として扱う。混ぜない。

## 9. Authentication / pairing

上位 architecture の確定事項（Client-specific connection material、Credential との区別、Host 側での保護・revoke・recovery 確認）を具体 protocol へ落とす。具体暗号 library・key format は固定しない。秘密 material を通常 message payload へ載せない。

### 9.1 概念の区別

| 概念 | 意味 | 扱い |
|---|---|---|
| Credential（登録済み秘密） | Provider・MCP 等の認証のための Owner 登録秘密 | OS credential store 等の分離保管。wire へ平文で出さない。Client へ渡さない。Client auth の材料にしない |
| pairing material | 当該 Client device を Host が認識するための接続材料 | Host 正本の domain data でも Credential の cache でもない。Client で保持する場合は ene 内部の保護対象とし、接続目的へ限定する。device 失効を適用する |
| session material | 当該 connection のための一時的証明材料 | connection ごとに発行・破棄する。旧 session で新 session を復活させない |

### 9.2 pairing

1. 新しい Client は Owner が Host 側で確認できる device pairing を必要とする（要件 Remote Client）。pairing 開始は Client からの `PairingRequest{ device_descriptor }` とし、Host は Owner 確認待ち（管理面）にする。Owner 確認なしに pairing を成立させない。
2. Owner 確認後、Host は `DeviceWireId` を発行し、device record（descriptor・許可された機能範囲・失効 flag・最終接続）を durable に保持する。pairing material は auth 専用 frame で Client へ渡す。通常 payload へ載せない。
3. 古い接続材料だけで Host 側の pairing・許可を復活させない。pairing 失効後の再 pairing は新規 pairing として Owner 確認を必要とする。

### 9.3 connection authentication・reconnect authentication

1. connection 確立ごとに `AuthChallenge (Host nonce) → AuthProof (Client 証明) → AuthResult (Host 判定＋ConnectionWireId 付与)` を行う。Client は秘密を平文で送らず、所有証明のみ送る。具体方式は Freedom とするが、「秘密の非露出」「nonce の単発性」「旧 proof の再利用禁止」の property を満たすこと。
2. 認証成功時に Host は当該 connection の `ConnectionWireId` を発行し、current connection（device ごと）を更新する。以後の Client→Host message は `sender.connection_id` を載せる。auth 前の message は pairing・auth 用に限定し、domain 操作を受け付けない。
3. reconnect は新規 connection として認証する。旧 `ConnectionWireId`・旧 stream・旧 ticket・旧 round を引き継がない。旧 connection の message は `StaleConnection` として拒否する。

### 9.4 revoke

- Owner は pairing 済み device・最終接続・許可された機能を確認し、device ごとに失効できる（要件）。失効は Host の device record に durable に記録し、現 connection を切断し、以後の auth を拒否する。
- 失効した device の旧 material・旧 session では再接続・復活できない。失効の伝達は `RevocationNotice` fact で当該 Client へ知らせる（到達不能でも失効は成立する）。
- Host restart・Restore 後も失効は維持する。Restore で過去の pairing・許可を復活させない（PR §9）。

## 10. Transport

### 10.1 範囲の判断

wire semantic と transport を分離し、以下を共通化の範囲とする：envelope・payload・binary attachment の encoding、identity・correlation・version・auth property、domain pattern の意味。transport 差は adapter boundary の内側に閉じ込める。

| transport | 用途 | 選択 |
|---|---|---|
| same-machine | Host と同 PC の Client | OS local socket（Linux: Unix domain socket、Windows: named pipe または loopback＋OS peer 認証相当）。OS account 保護を前提とし、TLS は必須にしない。frame は length-prefixed（既存 `ene-plugin-ipc` frame と同型の 4-byte BE exclusive-length、上限付き） |
| LAN / remote device | 別 PC の Client（同一 LAN・Owner 管理 VPN） | WebSocket（binary message）＋TLS。ene 運営 relay・account・Cloud を接続要件にしない。単一 connection で論理 stream を多重する（`stream_id` で mux） |
| future transport | 将来の追加 | adapter 追加で対応する。wire semantic・DTO・version・auth property を変えない |

QUIC 等の採用は現時点でしない。理由：現在の topology（単一 Owner-managed Host、少数 Client、ticket 制御の低頻度 capture、WebSocket で足りる stream 多重）では必要性がなく、over-engineering になるためである。将来 transport は adapter として追加できる（第30節）。

### 10.2 transport adapter boundary

```rust
// Host / Client 共有の adapter 概念（pseudo-trait。crate 配置は第25節）
trait TransportAdapter {
    // frame 送受信。frame payload は envelope＋payload の MessagePack byte 列、
    // または binary attachment frame（descriptor 対応付き）である。
    async fn send_frame(&self, frame: TransportFrame) -> Result<(), TransportError>;
    async fn recv_frame(&self) -> Result<TransportFrame, TransportError>;
    // liveness。heartbeat・keepalive の値自体は Freedom。liveness を
    // presence・許可・報告完了の根拠にしない。
    async fn peer_liveness(&self) -> PeerLiveness; // Reachable | Suspected | Lost
}

enum TransportFrame {
    ControlFrame { bytes: Vec<u8> },          // envelope＋payload（上限付き）
    BinaryFrame { descriptor_ref: WireMessageId, bytes: Vec<u8> }, // attachment
}
```

- frame 上限を設け、超過は `FrameTooLarge`（transport error）として受信前に拒否する。具体値は Freedom とするが、control と binary で上限を分け、重要 control を高頻度 capture と同じ drop policy にしない（第22節）。
- heartbeat は transport liveness のためだけに使い、presence・帰属・許可・報告完了の authority にしない。`Suspected` / `Lost` は切断検知の一時状態であり、帰属 durable を即時破棄しない（CCT §10）。

## 11. Client incarnation / stale rejection

### 11.1 三者の区別

| 概念 | 識別子 | 発行者・寿命 | 意味 |
|---|---|---|---|
| connection identity | `ConnectionWireId` | Host が connection ごとに発行 | 当該 connection の識別。auth 成功時に current になる |
| Client instance / incarnation | `ClientIncarnationId` | Client が process boot ごとに発行（persisted counter＋random） | 当該 Client process 世代の識別。restart すれば変わる |
| presence generation | `PresenceGeneration`（値の写し） | Host の帰属 lifecycle が発行。Client は写しを返す | どの帰属区間に属するかの識別 |

三者を一つの session id へ潰さない。connection が変われば incarnation の新旧によらず旧 connection は stale であり、incarnation が変われば connection の新旧によらず旧 incarnation は stale である。presence generation は帰属の区間であり、connection・incarnation の新旧で代替しない。

### 11.2 wire property

- すべての Client→Host message の envelope は `sender{ device_id, incarnation_id, connection_id }` を載せる（第5節）。欠落は不受理の理由にする（「制約なし」への変換禁止）。
- Host は device ごとの current `(incarnation_id, connection_id)` を保持する（durable の最終接続管理 record＋runtime の live 表）。受信時に次を照合する：
  1. `device_id` が pairing 済み・非失効であること。
  2. `connection_id` が当該 device の current であること。
  3. `incarnation_id` が current incarnation と対応すること（旧 incarnation からの到着は stale）。
  4. `observed.presence_generation_view` が現在の帰属 generation と対応すること（Client 依存操作の場合）。
  5. `round_view` / `ticket_view` が現在の round / ticket と対応すること（該当操作の場合）。
- Client 側が自身を current だと宣言しただけでは成立しない。Host の durable・live との照合が必須である。確認不能を現在と推定しない。

### 11.3 stale 時の扱い

- stale は transport error ではなく domain outcome（`StaleConnection` / `StaleIncarnation` / `StaleRound` / `StaleTicket` 等の typed reject DTO）として返す。connection は維持する（auth 失敗・decode 失敗・unsupported protocol とは区別する。第24節）。
- stale message による presence・許可・Task 反映・作用開始・削除参加の成立・復活をしない。旧 round の入力は元 round へ対応付け、新 round へ付け替えない。

## 12. Presence protocol

Host 側の authoritative presence generation を基準とする。二つの Client が同一 Companion について同時に active だと確定できない protocol にする。Client 側 UI acknowledgment を presence 成立そのものの authority にしない。

### 12.1 message 群

| message | 方向 | pattern | 意味 |
|---|---|---|---|
| `MoveIntent` | Client→Host | command | 呼出し・移動意図（Owner 呼出し・事前指示・自発の別＋`expected_generation`）。成立ではなく提案 |
| `TransitionAck` | Host→関係 Client | ack＋fact | `旧→移行中→新` の durable 遷移結果（新 generation 付き）。移動元・移動先の両方へ送る |
| `PresenceAttributionFact` | Host→購読 Client | fact | 現在帰属（companion・state・active・generation）。最新値意味で supersede する |
| `DisconnectNotice` | 両方向 | fact | 切断の観測事実（検知側が送る）。帰属 durable の即時破棄ではない |
| `ReconnectHello` | Client→Host | command（再認証付き） | 再接続の申告。新規 connection auth を伴う。旧 state の復活要求ではない |
| `RecoveryInvite` | Host→Client | fact | Host restart 後の復旧誘い（復元前 Client への自動復元用）。presence 成立ではなく確認の求め |

### 12.2 規則

- 移動は Host の `expected_generation＋expected_state` の CAS による `旧→移行中→新` の durable 遷移で確定する（CCT §10）。simultaneous summon は先勝ちのみ成立させ、後着は `StalePresence` として不受理・再評価へ戻す。
- 移行中は新旧いずれでも Client 依存の新規開始をしない。旧 in-flight は安全な区切りまで継続し、旧作用の別 Client 自動継続をしない。
- Client の UI ack（`PresencePresentedAck`）は「表示した」ことの確認であり、presence 成立の authority ではない。ack がなくても帰属は成立し、ack があっても帰属は変わらない。
- 通常の Client 切断では Running Companion の帰属は Host 側の current として保持し、再接続・復旧の確認を経て確定する。Stop は disconnect とは異なり、停止中 Companion を Host 側へ移動して presence を残さない（要件）。
- Host restart restoration：Host は `presence_attribution`＋hint・復旧先（非現在の参照）を読み、`RecoveryWait` として再構成し、復元前 Client へ `RecoveryInvite` を送る。現接続・許可・排他性の確認ができれば `Present` へ確定し、できなければ active なしにする。別 Client への無条件自動移動・Stopped への適用・Task 再開権限化をしない。

### 12.3 DTO（抜粋。全体は第21節）

```rust
struct MoveIntent {
    companion: CompanionWireRef,
    from_client: Option<ClientWireRef>,
    to_client: ClientWireRef,
    reason: MoveReason, // OwnerSummon | PriorInstruction | SpontaneousNeed | ReconnectRecovery
    expected_generation: u64, // presence generation の写し（authority ではない）
    intent_id: CommandWireId, // idempotency key
}
enum MoveOutcome {
    Transitioning { new_generation: u64 },
    RejectedStalePresence { current_generation: u64 },
    DeniedByConstraint { reason: DenyReasonWire },
    HeldForSafeClosure,
}
```

## 13. Text / Voice / presentation

生成完了、Host 送信、Client 受信、Owner への提示を同一事実として扱わない。

### 13.1 Text

| message | 方向 | pattern | 意味 |
|---|---|---|---|
| `SubmitTextInput` | Client→Host | command | Owner 入力 candidate（companion 参照・round 参照・`ClientInputLocalId`・本文）。受理ではなく提案 |
| `RoundIntakeOutcome` | Host→Client | ack（domain outcome） | `AcceptedForRound \| StaleRound \| HeldForTransition \| NeedsRevalidation`。旧 round なら元 round へ対応付け、新 round へ付け替えない |
| `TextStreamOpen` | Host→Client | stream open | 応答 stream の開始（`stream_id`・round・generation 付き）。open 成功は提示・達成ではない |
| `TextStreamFrame` | Host→Client | stream frame | 部分出力（`seq`・delta text・`is_final`）。`seq` 順に提示する |
| `TextStreamClose` | Host→Client | stream close | `Completed \| Interrupted \| Cancelled \| Stale` の区別 |
| `ConfirmPresentation` | Client→Host | observation | 提示確認（`Presented \| Unknown \| Failed`＋理由）。送信≠報告完了 |

- input attribution：入力は「どの Client のどの round で Owner が送ったか」の対応（companion・client・round・generation）を伴う。話者認証の意味を足さない。
- round identity：round は Host 発行の `RoundWireId` である。移動・切断・再起動で旧 round の入力・未提示出力を新 round へ付け替えない。
- partial / streaming output：`stream_id`＋`seq`＋`is_final` で順序付ける。`is_final` なしの frame を完了にしない。
- 未提示出力は `UndeliveredSummary`（第18節・W-3）へ接続し、次 Client で現在の結果・利用制限・削除状況へ照合して要約報告する。送信・受信を報告完了にしない。

### 13.2 Voice

| message | 方向 | pattern | 意味 |
|---|---|---|---|
| `VoiceStreamOpen` | 両方向合意（Client 要求＋Host 確定） | stream open | voice session の開始（`VoiceSessionWireId`・round・codec・generation 付き） |
| `VoiceAudioFrame` | 両方向 | stream frame | audio binary（attachment。`seq`・時刻付き） |
| `VoiceControl` | 両方向 | command＋fact | Mute・barge-in・interruption・停止要求等の制御。`Mute` は Client-local 即時＋Host への fact 通知 |
| `VoiceStreamClose` | 両方向 | stream close | `Completed \| Interrupted \| Cancelled \| Stale` の区別 |

- Voice stream の再接続で旧 stream を自動継続しない。stream / session-specific identity（`VoiceSessionWireId`）を導入し、再接続は新規 open とする。旧 session の frame を新 session へ付け替えない。旧 session の到着は `StaleStream` として破棄・元記録に留める。
- Mute・Voice 停止・会話停止・承認拒否は Voice だけに依存せず keyboard で操作できる（要件）。protocol 上は `VoiceControl` とは別の `ManagementIntent`（停止・拒否）としても送れること。Voice frame の到着停止を停止完了・承認拒否とみなさない。
- VAD・barge-in・audio buffer は Client-local transient であり、wire へ状態正本として送らない。送るのは制御と frame のみである。
- Microphone が周囲発話を拾い得ることの明示は UI・管理面の責務であり、protocol は話者認証済みの意味を付与しない。

### 13.3 presentation acknowledgement・delivery failure・undelivered

- `ConfirmPresentation{ Presented | Unknown | Failed }` を Client→Host の observation とする。`Presented` は当該 Client での提示確認であり、Task 達成・作用成功・承認ではない。`Unknown`（提示不明）は保持し、確定済みにしない。
- delivery failure（transport 到達不能・decode 失敗・Client crash 等）は transport・Client adapter の事実であり、Host は該当出力を `Undelivered` として保持し、次 Client で要約報告する。failure を報告完了・削除・再送の自動化にしない。再送は新 round・新 stream の現在照合を経る。

## 14. Observation

共有 Observation について、capture request / eligibility・captured data / candidate・routing に必要な Client relation・stale capture・Stop / move 後の late capture / result を wire 上で安全に扱える形にする。Raw capture を必要以上に Host へ常時送る設計を前提にせず、privacy / performance 要件から ticket 制の pull にする。

### 14.1 message 群

| message | 方向 | pattern | 意味 |
|---|---|---|---|
| `EligibilityFact` | Host→Client | fact | 当該 Client の観測可否・条件（対象範囲＝desktop 全体、頻度上限、pause/off、fullscreen 休止等）の表示用 |
| `CaptureTicket` | Host→Client | command | 一回分の capture 許可（`TicketWireId`・対象・期限・generation 付き）。ticket なしの capture 送信を受け付けない |
| `CaptureFrame` | Client→Host | command（candidate 提出） | capture 結果（`CaptureLocalId`・ticket 参照・取得時刻・descriptor＋binary attachment）。Raw の常時送信ではなく ticket 応答の一回分 |
| `CaptureOutcome` | Host→Client | ack（domain outcome） | `AcceptedForRouting \| SuppressedByControl \| StaleTicket \| StaleGeneration`。受信は routing 採否・理解・発話判断ではない |
| `AvailabilityFact` | Client→Host | fact | fullscreen・pause・負荷・capture 可否等の Client 側事実 |

### 14.2 規則

- Host は対象 Client へ順番に timing をずらして ticket を配る（要件の負荷分散）。複数対象 Client に同時 capture させない。各 ticket は一回限り・期限付きであり、期限切れ・移動・Stop・Pause 後の ticket は無効になる。
- Client は ticket があるときだけ capture し、ticket 参照付きで送る。ticket なし・旧 ticket・旧 generation の capture は `StaleTicket` / `StaleGeneration` として不受理にし、現在活動へ付け替えない。
- routing に必要な Client relation（どの Client の・いつの・どの ticket の capture か）は envelope＋`CaptureFrame` の明示 field で運ぶ。本文・画像 byte の解析に依存しない。
- Stop / move 後の late capture / result は元の ticket・世代へ対応付け、現在の対象・routing への自動採用をしない。旧 Client の旧 capture・候補で新規 capture・delivery を続けない。
- Raw capture は通常保存しない（要件）。Host は受信した capture を routing 判断の範囲で利用し、Task の Computer Use とは区別する。画面内指示を Owner 依頼・承認にしない。
- Observer 専用 Provider assignment・routing semantic そのものは Host 側 contract であり、Client へ authority として公開しない。Client が見るのは ticket・自身の `CaptureOutcome`・`EligibilityFact` だけである。

## 15. Client-dependent Action（Computer Use 等）

Host での現在認可・Client target・attempt identity・presence generation・concrete device operation・acknowledgement・effect result / unknown を失わない wire contract にする。**Client へ action command が届いたこと ≠ effect 成功**である。disconnect / response loss 時に Host が自動 retry 可能だと誤認する protocol にしない。transport retry と external effect retry を分離する。

### 15.1 message 群

| message | 方向 | pattern | 意味 |
|---|---|---|---|
| `ClientActionCommand` | Host→Client | command | concrete device operation（`OperationWireId`・`AttemptWireRef`・target client・generation・操作記述・制約・idempotency key）。認可判断そのものは含めない |
| `ActionReceiptAck` | Client→Host | ack | 受信確認（`Received \| RejectedStale \| DeniedByHold \| UnsupportedCapability`）。受信は効果・成功ではない |
| `ActionProgress` | Client→Host | progress | 中間報告（`progress_seq`・状態 hint）。確定ではない |
| `EffectReport` | Client→Host | completion | 効果報告（`ConfirmedSuccess \| ConfirmedFailure \| Unknown`＋根拠参照）。`Unknown` は粘着させる |
| `ActionCancel` | Host→Client | command | 停止要求（第16節）。delivery を停止完了にしない |

### 15.2 規則

- Host は現在認可（K-B の今回確定）を経て初めて `ClientActionCommand` を発行する。command には `AttemptWireRef`（当該試行）・`OperationWireId`（論理操作。retry を束ねる対応）・presence generation・target client・concrete 操作を載せる。Task revision 前提・委任 scope・実対象・操作種別・依拠 Permission の全文は載せない（Host が mapping で保持する）。
- Client は受信したら `ActionReceiptAck` を返す。`Received` は「受け取った」ことであり、成功・実行開始・完了のいずれでもない。stale（旧 generation・旧 connection・旧 attempt・ capability 不足）は `RejectedStale` 等で返す。
- 効果の確定は `EffectReport` の certainty で行う。`Unknown`（disconnect・response loss・確認不能）は未実行・成功・失敗へ書き換えない。Host は attempt を `Unknown` のまま durable に保持し、重複 risk 付き Owner 判断へ戻す（CCT §8）。
- disconnect / response loss 時の自動 retry をしない。transport の再送（同一 `command_id`＋新 `message_id`）は受信・ack の再送に留め、作用の再実行ではない。作用の再実行は新しい attempt（新 `AttemptWireRef`）＋Owner 判断を必要とする。Client は再接続時に旧 command を自動再実行しない。
- Computer Use の対象は現在の active Client に限定する（要件）。移動時は安全に区切れるところまで移動を遅らせ、元 Client の Action を別 Client で自動再実行しない。ambient Observation の有効化を操作の承認にしない。

## 16. Cancellation

wire cancellation について、cancel request・Client receive acknowledgement・computation / action stop acknowledgement・actual effect certainty を区別する。Cancel message delivery を停止完了として扱わない。

| message | 方向 | pattern | 意味 |
|---|---|---|---|
| `CancelRequest` | 両方向（要求元→実行側） | command | 停止要求（対象 operation / stream / attempt 参照＋理由）。受付と完了を分ける |
| `CancelReceived` | 実行側→要求元 | ack | 受付確認（記録したこと）。停止完了ではない |
| `StopAck` | 実行側→要求元 | completion（停止側） | 停止結果（`Stopped \| AlreadyCompleted \| StopUnknown`＋既作用・未保存の対応）。外部作用の rollback 保証ではない |
| `EffectCertaintyUpdate` | 実行側→要求元 | fact | 最終の効果確定度（`ConfirmedSuccess \| ConfirmedFailure \| Unknown`）。新 evidence でのみ更新する |

- Host→Client の Action cancel と Client→Host の inference / stream cancel（barge-in・応答停止等）の両方向に同じ区別を適用する。
- future drop・connection 切断を停止完了とみなさない。停止後に届く遅延結果は元 attempt・元 operation へ記録し、現在の目的への自動採用・後続自動開始をしない。

## 17. Targeted Deletion 参加

Client 内に Ene 管理下の temporary copy が存在する場合の wire 参加を成立させる。削除対象本文そのものを「照合用」として無制限に Client へ再送しない。Client local completion だけを global completion にしない。

### 17.1 message 群

| message | 方向 | pattern | 意味 |
|---|---|---|---|
| `DeletionDemand` | Host→Client | command | local 削除要求（`DeletionOpWireId`・sweep・valid interval・target descriptor。本文なし） |
| `DeletionProgress` | Client→Host | progress | 処理中報告（任意） |
| `LocalErasureResult` | Client→Host | completion（局所） | local 検証・結果（wiped・unverified range・unreachable の区別）。全域完了ではない |
| `DeletionCompletedNotice` | Host→Client | fact | 全域完了の通知（Client の後続再保存防止・token 破棄用）。Client の完了証拠にしない |

### 17.2 target / condition の Client 向け representation（本文を送らない）

```rust
struct DeletionDemand {
    operation: DeletionOpWireId,   // 全域操作 identity
    sweep: u64,                    // 消去区間の順序（写し）
    valid_interval: ValidIntervalWire, // 開始〜検証完了（区間内再到着を含む）
    targets: Vec<DeletionTargetWire>,  // 本文なし。class＋interval＋item ref のみ
}

enum DeletionTargetWire {
    // Client 一時 class の全量 wipe（本文照合なし）。例：入力途中・表示 cache・
    // audio buffer・capture cache・asset cache・未送信操作・Tool UI data。
    WipeClass { class: ClientTempClass, range: TimeRangeWire },
    // Host が管理する特定 item の ref 指定（ID のみ。本文なし）。
    // 例：undelivered copy・asset chunk・capture ticket 対応物。
    EraseItemRef { item: ItemWireRef },
}
```

- 機械的条件の文字列そのものを Client へ送らない。Client は class＋interval＋item ref で特定できる一時 data を wipe し、範囲・未確認を報告する。Host 側の機械的検索・残存検証は Host が行う。意味的補助（言換え特定）の完全性は保証しない（CI §3.6）。
- Client は `LocalErasureResult{ wiped, unverified_range, unreachable_detail }` を返す。到達不能・未確認を成功と読まない。再接続時に旧 copy を Host へ戻して再形成しない。
- stale Client result（旧 operation・旧 sweep の結果）は現在の全域完了に採用しない。元 operation への記録に留める。
- `DeletionDemand` の target descriptor・検索 token は操作期間のみ保持し、完了時に破棄する。完了記録・Audit へ対象本文を戻さない。

## 18. Management surface

Owner 管理面が Client に存在しても、Client から送られる control 変更は **Owner intent / candidate request** であり、Host-side control owner が最終的に成立させる既存 contract（IB 第9節・K-A）を維持する。管理画面から Permission・Provider・Rule 等を変更できる場合でも、Client が control state 正本を所有する protocol にしない。

```rust
struct ManagementIntent {
    intent_id: CommandWireId,      // idempotency key
    kind: ManagementIntentKind,    // StopCompanion | DeleteCompanion | CancelTask |
                                   // ManageSchedule | DenyOrRefuse | ManageRuleConsentCap |
                                   // ManageDevice | ConfigureCredentialIntent | RequestDeletionBackupRestoreReset
    target: ManagementTargetWire,  // 対象参照（wire ref のみ）
    base_view: BaseViewMark,       // Client が見た表示 revision の写し（authority ではない）
    rationale: IntentRationaleWire, // Owner の意図の写し（会話由来 / 管理面操作の別＋引用対応）
}
enum ManagementOutcome {
    AppliedAsOneTime,              // 一回承認として適用
    StoredAsRuleView,              // Rule 等として保存（revision view 付き）
    NeedsClarification,            // 曖昧・矛盾・過度・重大のため確認へ戻す
    DeniedByBoundary,              // 永続 Deny・Always ask・Capability 境界の黙上書きに当たる
    StaleBaseView { current: ViewMark }, // 表示が古い。再取得・再評価へ戻す
    HeldByOperation,               // 消去・復元保留・停止等で新規禁止
}
```

- Host は intent を `ProposeControlChangeCommand` 等の domain premise へ mapping し、各 owner の確定を経て `ManagementOutcome` を返す。Client の送信成功・表示更新を確定にしない。
- Client が受け取る view（rule 概要・consent 概要・cap・device・audit 概要等）は filtered display fact であり、正本ではない。secret・判定 copy・内部 permission 条件の全文は送らない。view の revision 表示は correlation の写しであり、Client がそれを権限の根拠にしない。

## 19. Body / presentation resources

- Host は表示資材（VRM・motion 設定・voice 設定等の静的資材）を asset descriptor＋chunk stream で供給する。適用関係・経験状態は送らない。内部正本の主 key として再利用できる形で渡さない。
- Client は資材を transient cache として扱う（canonical にしない）。version（`CharacterRevision` の表示ラベル）が変われば旧 cache を破棄する。Targeted Deletion・Reset の参加時は asset cache も wipe 対象にする。
- Host→Client の身体表現は high-level body-state hint（idle / listening / speaking / working / attention 等の fact）に留める。関節角・blendshape 等の staging は Client-local で行い、wire 正本にしない。出力した表情・motion を Companion State の正本・永続変化の根拠にしない。
- 描画失敗・fullscreen・高負荷は Client fact として Host へ知らせるが、Text・管理・復旧へ波及させない。Body 表示に失敗してもテキスト会話・Task 管理・設定・復旧操作を利用できる（要件）。

## 20. Message inventory

pattern・方向・authority の所在を一覧する。envelope 自体は含めない。

| # | message | 方向 | pattern | authority / 確定者 |
|---|---|---|---|---|
| M-1 | `PairingRequest / PairingResult` | C→H / H→C | request/response | Host（Owner 確認）。Client 要求は申込み |
| M-2 | `AuthChallenge / AuthProof / AuthResult` | H→C / C→H / H→C | request/response（auth 専用） | Host。旧 material で復活させない |
| M-3 | `CapabilityAdvertise / NegotiatedConnection` | C→H / H→C | request/response（接続時） | Host（選択）。申告は availability fact |
| M-4 | `CapabilityUpdate / AvailabilityFact` | C→H | fact | Host（材料）。許可・presence ではない |
| M-5 | `MoveIntent / MoveOutcome(TransitionAck)` | C→H / H→C | command+ack | 接続・存在（帰属成立）。意図は個体調整・Client |
| M-6 | `PresenceAttributionFact` | H→C | fact（subscribe） | 接続・存在。最新値意味 |
| M-7 | `DisconnectNotice / ReconnectHello / RecoveryInvite` | 両方向 | fact / command / fact | 帰属は接続・存在。hello は申告 |
| M-8 | `SubmitTextInput / RoundIntakeOutcome` | C→H / H→C | command+ack | 個体調整（会話受理）＋接続・存在（帰属照合） |
| M-9 | `TextStreamOpen / Frame / Close` | H→C | stream | 入出力・提示（round 実際）。意味は個体調整 |
| M-10 | `ConfirmPresentation` | C→H | observation | 個体調整（報告状況）。送信≠報告 |
| M-11 | `UndeliveredSummary / UndeliveredAck` | H→C / C→H | subscription＋fact / observation | 個体調整（必要性）＋入出力・提示（提示事実） |
| M-12 | `VoiceStreamOpen / AudioFrame / VoiceControl / VoiceStreamClose` | 両方向 | stream＋command | round 実際は入出力・提示。会話意味は個体調整 |
| M-13 | `EligibilityFact / CaptureTicket` | H→C | fact / command | 共有観測（対象・時機） |
| M-14 | `CaptureFrame / CaptureOutcome` | C→H / H→C | command+ack | 共有観測（routing 採否）。受信は理解・採否ではない |
| M-15 | `ClientActionCommand / ActionReceiptAck / ActionProgress / EffectReport` | H→C / C→H | command+ack＋progress+completion | 実行・拡張（作用・確定度）。command 到着≠成功 |
| M-16 | `ActionCancel(CancelRequest) / CancelReceived / StopAck / EffectCertaintyUpdate` | 両方向 | command+ack＋completion＋fact | 各 owner（事実の帰属）。delivery≠停止完了 |
| M-17 | `DeletionDemand / DeletionProgress / LocalErasureResult / DeletionCompletedNotice` | H→C / C→H | command+ack相当＋fact | 保全・消去（全域確定）。局所完了≠全域完了 |
| M-18 | `ManagementIntent / ManagementOutcome` | C→H / H→C | command+ack（candidate+decision 投影） | 各 control owner。intent は提案 |
| M-19 | `ManagementViewRequest / ManagementView` | C→H / H→C | request/response | 各 owner（表示用投影）。view は正本ではない |
| M-20 | `AssetDescriptorRequest / AssetDescriptor / AssetChunkStream` | C→H / H→C | request/response＋stream | Character（静的内容供給）。適用確定は個体調整 |
| M-21 | `BodyStateHint` | H→C | fact | 個体調整（活動状態）＋認識・学習（内的状態の意味）。表示 staging ではない |
| M-22 | `RevocationNotice` | H→C | fact | 権限・制約＋接続・存在。到達不能でも失効は成立 |
| M-23 | `UnsupportedMessage / IncompatibleProtocol` | H→C（主に） | reject（第24節） | transport / mapping。副作用なし |

## 21. Wire DTO（pseudo-code）

すべて `ene-api::v1::*` の wire DTO であり、Host domain newtype・durable row・secret を含まない。wire ref は Host mapping が内部 identity へ解決する opaque 参照である。Client は構造を解釈・合成しない。

```rust
// ---- 共通 ----
struct ProtocolVersion { major: u16, minor: u16 }
struct WireMessageId(/* opaque; 新規送信ごとに新規 */);
struct RequestWireId(/* opaque */);
struct CommandWireId(/* opaque */);
struct StreamWireId(/* opaque */);
struct ConnectionWireId(/* opaque */);
struct DeviceWireId(/* opaque */);
struct ClientIncarnationId { counter: u64, random: u64 }
struct CompanionWireRef(/* opaque; Host 発行 */);
struct ClientWireRef(/* opaque; Host 発行 */);
struct RoundWireId(/* opaque; Host 発行 */);
struct VoiceSessionWireId(/* opaque; Host 発行 */);
struct TicketWireId(/* opaque; Host 発行 */);
struct OperationWireId(/* opaque; Host 発行 */);
struct AttemptWireRef(/* opaque; Host 発行 */);
struct DeletionOpWireId(/* opaque; Host 発行 */);
struct ItemWireRef(/* opaque; Host 発行 */);

// ---- presence ----
struct PresenceAttributionWire {
    companion: CompanionWireRef,
    state: PresenceStateWire, // Present | NoActive | InTransition | Stopped | RecoveryWait
    active_client: Option<ClientWireRef>,
    generation: u64, // PresenceGeneration の値の写し
}

// ---- text ----
struct SubmitTextInput {
    companion: CompanionWireRef,
    round: RoundWireId,
    local_id: ClientLocalId, // Client の対応付け用
    body: TextBodyWire,      // 本文（一時表現。Host は History へ正本化する）
}
struct TextBodyWire { text: String /* 上限付き */, lang: TextLangWire }
enum RoundIntakeOutcomeWire {
    AcceptedForRound { round: RoundWireId },
    StaleRound { current_round: Option<RoundWireId>, current_generation: u64 },
    HeldForTransition,
    NeedsRevalidation { reason: RevalidationReasonWire },
}
struct TextStreamFrameWire {
    stream: StreamWireId,
    seq: u64,          // stream 内順序。欠落・重複は破棄・再要求の対象であり guess しない
    delta: String,     // 部分 text（上限付き）
    is_final: bool,
}
struct ConfirmPresentationWire {
    round: RoundWireId,
    stream: Option<StreamWireId>,
    status: PresentationStatusWire, // Presented | Unknown | Failed
    detail: Option<String>, // 理由の表示用（秘密・本文の複製を含めない）
}

// ---- voice ----
struct VoiceStreamOpenWire {
    session: VoiceSessionWireId,
    round: RoundWireId,
    codec: VoiceCodecWire, // 採用 codec・rate の合意表示
    generation: u64,
}
struct VoiceControlWire {
    session: VoiceSessionWireId,
    control: VoiceControlKindWire, // MuteOn | MuteOff | BargeIn | Interrupt | StopVoice
}

// ---- observation ----
struct CaptureTicketWire {
    ticket: TicketWireId,
    scope: CaptureScopeWire, // desktop 全体（window 個別対象と誤認させない表示）
    expires_at: WallClockWire, // wall-clock＋tz。stale 判定の根拠にはしない（ticket 失効は Host が確定）
    generation: u64,
}
struct CaptureFrameWire {
    ticket: TicketWireId,
    local_id: ClientLocalId,
    captured_at: WallClockWire,
    descriptor: CaptureDescriptorWire, // 解像度・形式等の表示（上限付き）
    // binary 本体は attachment frame で送り、descriptor_ref で対応付ける。
}
enum CaptureOutcomeWire {
    AcceptedForRouting,
    SuppressedByControl { reason: IneligibilityReasonWire },
    StaleTicket,
    StaleGeneration { current_generation: u64 },
}

// ---- action ----
struct ClientActionCommandWire {
    operation: OperationWireId,
    attempt: AttemptWireRef,
    target_client: ClientWireRef,
    generation: u64, // presence generation の写し
    device_op: DeviceOpWire, // concrete device operation（種別＋対象＋参数。上限付き）
    constraint: ActionConstraintWire, // timeout・中断条件等の実行制約（許可条件ではない）
    idempotency_key: CommandWireId,
}
struct DeviceOpWire {
    kind: DeviceOpKindWire, // Click | TypeText | KeyPress | FileOpWithinTask | ...（閉じた enum）
    target: DeviceTargetWire, // 対象記述（解決済み実対象の投影。文字列一致を対応にしない）
    params: DeviceParamsWire, // 参数（上限付き。secret・credential を含めない）
}
enum ActionReceiptAckWire {
    Received,
    RejectedStale { current_generation: u64 },
    DeniedByHold { reason: HoldReasonWire },
    UnsupportedCapability { kind: ClientFeatureKindWire },
}
struct EffectReportWire {
    operation: OperationWireId,
    attempt: AttemptWireRef,
    certainty: CertaintyWire, // ConfirmedSuccess | ConfirmedFailure | Unknown
    grounds_ref: GroundsRefWire, // 根拠への参照（evidence 本文を含めない）
}

// ---- cancel ----
struct CancelRequestWire {
    target: CancelTargetWire, // Operation | Stream | Attempt の wire ref
    reason: CancelReasonWire,
}
enum CancelReceivedWire { Recorded }
enum StopAckWire { Stopped, AlreadyCompleted, StopUnknown }

// ---- deletion ----
struct DeletionDemandWire {
    operation: DeletionOpWireId,
    sweep: u64,
    valid_interval: ValidIntervalWire,
    targets: Vec<DeletionTargetWire>, // 第17.2節。本文なし
}
struct LocalErasureResultWire {
    operation: DeletionOpWireId,
    wiped: Vec<WipedClassWire>,
    item_results: Vec<ItemErasureResultWire>,
    unverified_range: Vec<UnverifiedRangeWire>, // 未確認範囲（成功にしない）
}

// ---- management ----
struct ManagementViewWire {
    mark: ViewMarkWire, // 表示 revision の写し
    sections: Vec<ViewSectionWire>, // rule 概要・consent 概要・cap・device・schedule 等の filtered 表示
    // secret・判定 copy・内部条件全文を含めない。
}
```

DTO → Host domain command への変換点（Host ingress mapping。判断は各 owner）：

| wire DTO | mapping 先（domain premise） | 判断 owner |
|---|---|---|
| `SubmitTextInput` | `SubmitClientInputCandidate{ companion, client, claimed_generation, round }`（IB X-B） | 個体調整（受理）＋接続・存在（帰属照合） |
| `ConfirmPresentation` | `ConfirmPresentationObservation`（IB X-B） | 入出力・提示＋個体調整 |
| `MoveIntent` | `RequestMoveCommand`（IB X-A） | 接続・存在 |
| `CaptureFrame` | `PublishObservationCandidate` の Client 由来部分（IB X-E） | 共有観測 |
| `ActionReceiptAck`・`EffectReport` | `ReportEffectFact` の Client 由来部分（IB K-H） | 実行・拡張 |
| `LocalErasureResult` | `ParticipantCompletionFact` の Client 参加分（IB D-B） | 保全・消去（集約） |
| `ManagementIntent` | `ProposeControlChangeCommand` 等の intent 供給（IB K-A・第9節） | 権限・制約＋各 owner |

## 22. Backpressure and streams

すべての message へ global total ordering を要求しない。順序が必要なのは stream 内（`stream_id`＋`seq`）だけであり、stream 間・fact・command 間は順序付けしない。重要 control message を高頻度 capture frame と同じ drop policy にしない。

| domain | buffering | drop / stale policy | ordering |
|---|---|---|---|
| control（presence・move・cancel・deletion・management・auth） | bounded queue＋backpressure（送信側へ `BackpressureHold` を返す。黙って drop しない） | drop しない。stale は reject DTO で返す。unknown type は拒否 | 不要。世代・round・operation の対応で判定する |
| text token stream | bounded buffer（stream ごと）。slow consumer には pause hint（`StreamPauseHint`） | 旧 stream の frame を破棄。`seq` 欠落は guess せず `StaleStream`・再 open へ戻す | per-stream ordered（`seq`） |
| voice audio stream | bounded＋latest-value 的な振舞い（古い audio frame を queue に溜めない。gap を `AudioGapMark` で明示する） | stale session の frame を破棄。再送しない。旧 audio を新 session へ付け替えない | per-stream ordered（`seq`）。gap は明示する |
| capture frame | outstanding ticket は Client ごとに 1（新規 ticket は旧 ticket を無効化する） | stale ticket・旧 generation は `StaleTicket` で破棄。Raw の再送・常時送信をしない | 不要（ticket 対応で判定） |
| presence attribution・eligibility・body hint | latest-value（新 fact が旧 fact を supersede する） | 旧 fact を破棄する。欠落を成功・現在と読まない | 不要 |
| action progress | bounded（最新 progress のみ保持可） | 旧 `progress_seq` を破棄。progress を certainty にしない | per-operation 単調 `progress_seq` |
| asset chunk stream | bounded＋ack 駆動（次 chunk 要求方式） | 旧 version の chunk を破棄。descriptor なし attachment を解釈しない | per-stream ordered |

transport adapter は backpressure を上位へ伝える（`send_frame` の pending・`BackpressureHold`）。broadcast（観測・表示配信）は authority・commit 順序・排他に使わず、`Lagged` は skip し、欠落を成功・現在と読まない（CCT §15）。

## 23. Security

- wire から漏らさないもの：Credential secret、Host-local permission internals で不要なもの、他 Companion の private routing context、unnecessary Memory / History、deletion target 本文の不要な複製。view・summary・descriptor は filtered 投影に留め、私的根拠全文・判定 copy・秘密を含めない。
- Client から受け取る field は untrusted input として validation する。Host ingress mapping は次を行う：length 上限、閉じた enum の variant 確認、ID の opaque 形式確認、数値範囲、`observed` と current の照合（authority 化しない）、secret・credential らしき値の混入検査（検出時は受信拒否＋監査。完全識別は保証しない。要件 Credential 節）。
- typed Rust DTO であることを security boundary の代替にしない。DTO の deserialize 成功は validation 成功ではない。`validate()` を mapping の必須段階とする。
- log・Audit・Debug へ秘密値・削除対象本文・不要な会話本文・file 本文を出さない。debug 表示では wire ref・generation・outcome を残し、本文・binary を redact する。
- transport 保護：same-machine は OS account 領域・peer 認証、remote は TLS＋LAN/VPN＋pairing＋per-connection auth を組み合わせる。いずれも ene 運営 relay・account を置かない。

## 24. Error and rejection model

transport error と domain rejection を区別する。domain rejection は `Ok` 側の typed DTO で返し、`Err` 側の retry 対象にしない（IB §11）。

| 層 | 種別 | 例 | 扱い |
|---|---|---|---|
| transport | connection lost | peer 切断・TLS 失敗 | connection 終了。帰属・attempt・未伝達は Host durable に残し、再接続は新規 connection として認証する |
| transport | decode failure | MessagePack decode 失敗・frame 上限超過 | 当該 frame を破棄し `DecodeFailed` を通知（可能な場合）。副作用なし。累積する場合は connection を切断する |
| transport | unsupported protocol | major 不一致・auth 前の domain 操作 | `IncompatibleProtocol` で拒否。guess しない |
| transport | auth failure | proof 不一致・失効 device・nonce 再利用 | `AuthFailed` で拒否。旧 material で復活させない |
| domain reject | stale generation / connection / incarnation | `StaleConnection`・`StaleIncarnation`・`StalePresence`・`StaleRound`・`StaleTicket`・`StaleStream` | 現在への不採用。元 round・元 attempt・元 ticket への対応付けに留める。新 round・新 attempt への付け替えをしない |
| domain reject | no current presence | `NoCurrentPresence` | 新規開始しない。判断待ち・保留へ戻す |
| domain reject | denied / held | `DeniedByConstraint`・`DeniedByHold`・`HeldForTransition`・`HeldForSafeClosure` | 実行せず待機・判断待ちにする。黙って queue・replay しない |
| domain reject | capability missing | `UnsupportedCapability`・`InsufficientCapability` | 利用前に不足を示す。本文削減で黙って解消しない |
| domain reject | deletion no longer current | `DeletionSuperseded{ current_operation }` | 旧 operation の結果を全域完了に採用しない |
| domain reject | needs revalidation | `NeedsRevalidation{ reason }` | 現在条件の再照合へ戻す |

必要な domain-specific reject DTO は第21節の各 outcome enum が担う。共通巨大 error enum・単一 error code へ潰さない。`stale` / `denied` / `held` / `not-current` / `cap exceeded` を `Err` 側に混ぜない。呼び出し側が `Err` を `Denied` と誤読して誤った成功・拒否表示をしないこと（CC-07）。

## 25. IPC crate placement

CM を前提とし、crate 追加・依存方向の変更をしない。mapping は既存 crate の module として配置する。

| 配置 | 責務 | 持つもの / 持たないもの |
|---|---|---|
| `ene-api`（`ene-api::v1::*`） | wire DTO のみ。versioned module（`v1`）に envelope・payload・capability・auth frame 型・reject DTO を置く | 持つ：serde DTO・version 型・message type 識別・JSON rendering helper。持たない：business logic・authority 判定・Host domain 型・secret・durable row・transport I/O。Ene 内依存なし（serde 等の外部のみ）を維持する |
| Host adapter（`apps/ene-core` の `ipc_map` module＋各 domain の premise 受付） | DTO validation、wire ref → domain premise mapping、domain fact → DTO 投影、connection・incarnation・version・capability の保持（durable は各 owner の record） | 持つ：`validate()`・mapping 関数・subscription 管理・stream mux。持たない：採否・達成・許可・確定度の判断（各 owner）。domain crate に wire 依存を持ち込まない |
| Client adapter（`apps/ene-stage`・`apps/ene-ctl` 内の `ipc` module＋device adapter） | DTO → 表示・device 操作、device fact → DTO、transient cache 管理、削除参加時の local wipe | 持つ：presentation・capture・audio・tray adapter。持たない：Host domain crate への依存・canonical mutation・正本保持。依存は `ene-api`・`ene-primitive`・Client adapter のみ |

mapping の方向（CM §5.3・§10 の inversion に従う）：

- Host mapping は wire ref → domain premise の解決だけを行い、domain newtype 間の `From` を設けない。cross-domain 参照は `RawId`＋用途別 premise による inversion で解決し、crate 依存を一方向に保つ。
- `ene-api` に `ene-primitive` への依存を持ち込まない（CM §10.1 の現行条件を維持）。opaque 性質の共有が必要な場合は byte・integer の表現に留め、semantic newtype を集めない。
- `rusqlite::Transaction`・生 SQL・`SecretValue` を mapping・DTO へ露出させない。repository compare は Host domain 側の短 transaction で行う（IB §13）。

## 26. Validation — wire message だけを追う walkthrough

transport success を domain success へ読み替えないことを、各 walkthrough の合格条件とする。

### V-1 Client connect → authenticate → capability advertise

1. Client が transport 接続し、`PairingRequest`（未 pairing 時。Owner 確認待ち）または `AuthChallenge→AuthProof`（pairing 済み）を行う。秘密を通常 payload へ載せない。
2. Host は auth 成功時に `ConnectionWireId` を発行し、`CapabilityAdvertise` を受けて negotiated version・accepted features を確定する。申告は availability fact であり、許可・presence ではない。
3. 失格条件：auth なしの domain 操作は不受理にする。失効 device の旧 material では復活させない。

### V-2 Owner Text → Host → response stream → presentation acknowledgement

1. Client が `SubmitTextInput`（round・local_id・本文）を送る。送信成功は受理ではない。
2. Host mapping が validation→`SubmitClientInputCandidate` へ mapping し、現在帰属・現接続・許可・停止・保留を照合して `RoundIntakeOutcome::AcceptedForRound` を返す。旧 round なら `StaleRound` とし、新 round へ付け替えない。
3. Host は `TextStreamOpen→Frame(seq,is_final)→Close(Completed)` を送る。生成完了・送信・受信を同一事実にしない。
4. Client は提示後に `ConfirmPresentation::Presented` を送る。送信・受信だけでは報告完了にしない。提示不明は `Unknown` を保持する。

### V-3 Companion move A → B

1. B の Client が `MoveIntent{ companion, from=A, to=B, expected_generation }` を送る。意図は成立ではない。
2. Host が SD-Presence の CAS で `旧→移行中→新` を確定し、A・B へ `TransitionAck{ new_generation }`、購読者へ `PresenceAttributionFact` を送る。移行中は新旧いずれも新規開始しない。
3. 二重 active の禁止：simultaneous summon の後着は `RejectedStalePresence` とする。Client UI ack を成立の authority にしない。

### V-4 stale A から late input

1. 移動確定後に A から旧 `connection_id`・旧 generation・旧 round の `SubmitTextInput` が届く。
2. Host は `StaleConnection` / `StaleRound` として不受理にし、元 round へ対応付ける。新 round への付け替え・presence 復活・許可復活をしない。

### V-5 Client disconnect during Computer Use

1. 実行中の `ClientActionCommand{ operation, attempt }` に対し、Client 切断を検知する。切断検知は帰属 durable の即時破棄ではない。
2. Host は attempt を `Unknown` のまま保持し、best-effort 停止を試み、停止不能・既知作用・不明を残して報告する。成功・未実行へ書き換えない。
3. 自動 retry・別 Client での自動再実行をしない。再実行は新 attempt＋Owner 判断を必要とする。

### V-6 reconnect with unresolved Action

1. Client が新規 connection として再認証する。旧 stream・旧 ticket・旧 round を引き継がない。
2. Host は未確定 attempt を `Unknown` のまま提示し、重複 risk を示して Owner 判断を求める。旧 command の自動再実行・旧 ack の復活をしない。
3. transport の再送（同一 `command_id`＋新 `message_id`）は ack・受信の再送に留め、作用の再実行ではないことを確認する。

### V-7 Voice interruption

1. `VoiceStreamOpen{ session=S1 }` で開始し、`AudioFrame(seq)` を送る。途中で barge-in・Mute・停止が起きる。
2. `VoiceControl{ Interrupt }`→`VoiceStreamClose{ Interrupted }` とし、S1 の frame を S2 へ付け替えない。再開は新規 `VoiceStreamOpen{ session=S2 }` とする。
3. 旧 session の遅延 frame は `StaleStream` として破棄する。停止完了と効果確定を混同しない。

### V-8 Observation capture during Companion movement

1. A 在室中に発行した `CaptureTicket{ ticket=T1, generation=G1 }` に対し、移動確定（G2）後に A から `CaptureFrame{ ticket=T1 }` が届く。
2. Host は `StaleTicket` / `StaleGeneration` として不受理にし、現在 routing へ採用しない。元 ticket への記録に留める。
3. 新 Client B には新規 ticket を発行する。旧 capture で新規 capture・delivery を続けない。

### V-9 Targeted Deletion while Client offline

1. Host が `DeletionDemand{ operation }` を発行する。到達不能 Client は `pending/unreachable` として保全し、成功と読まない。
2. Client 再接続時、Host は現 operation の demand を再送する（旧 copy の Host への持ち帰りをさせない）。Client は class wipe＋item ref 消去を行い、`LocalErasureResult{ wiped, unverified_range }` を返す。
3. Host は全参加の集約＋機械的残存検証＋区間内再到着の取込みを満たして初めて全域完了とする。局所完了だけで hold を解除しない。完了記録へ対象本文を戻さない。

### V-10 Host restart → reconnect → presence restoration

1. Host restart 後、presence は `RecoveryWait` として再構成する。Task は明示再開待ち、Unknown attempt は `Unknown` のまま、未完了消去・保留は維持する。
2. Host は復元前 Client へ `RecoveryInvite` を送り、Client は新規 connection として再認証する。現接続・許可・排他性の確認ができれば `Present` へ確定し、できなければ active なしにする。
3. 古い一時 state・旧承認・解決済み経路だけでの presence・許可・再開の成立をしない。Task・Action の再開権限化をしない。

### V-11 Host newer / old Client

1. negotiated version が older major の共通範囲にない場合、`IncompatibleProtocol{ host_max, client_max, hint }` で拒否する。自動互換・guess をしない。
2. 共通 major がある場合、Host は older の理解範囲で話す。unknown optional field は無視し、required 意味は送らない。理解できない newer 意味を older へ黙って送らない。

### V-12 duplicate / delayed message

1. 同一 `message_id` の重複配送は沈黙破棄する（再実行なし、ack 再送は可）。
2. 同一 `command_id`＋新 `message_id` の retry は domain idempotency で prior outcome を返す（二重実行しない）。
3. 遅延到着物は元の round・attempt・ticket・operation へ対応付け、現在の目的への自動採用・後続自動開始をしない。到着順が最後であることを受入根拠にしない。

## 27. Avoid over-engineering — 導入しないもの

- exactly-once transport。at-least-once 配送＋`message_id` による重複抑止＋`command_id` による domain idempotency で足りる。
- distributed consensus・global message ordering・universal event log・universal RPC interface。単一 Owner-managed Host の topology では不要であり、per-stream 順序・世代対応・短 commit compare で成立させる。
- schema registry service・custom binary protocol。MessagePack＋versioned DTO＋field 規約で足りる。
- QUIC 等の新 transport の先行導入。必要になれば adapter として追加する。

## 28. 横断レビュー（自己レビュー）

本書完成後に requirements・Step 11・Step 12・CI・PR・CCT・IB・CM へ戻して自己レビューした。観点と結果は次のとおりである。

- **Authority 維持。** Host canonical・Client 非正本・client message 非 authority・Host-local 非公開・Client の Host crate 非依存を維持した。envelope・ID・ack・subscription・view のいずれも authority 化していない。UI ack を presence 成立にせず、受信・送信・表示を効果・完了・許可・報告完了にしていない。
- **Currentness 維持。** identity・revision・generation・typed correspondence・expected current・stale / delayed handling を wire へ落とした。Host 内部 boundary token の全文渡しをせず、Client が保持・返送する最小 correlation（wire ref・generation 写し・round / ticket view・command / stream ID）に限定した。三者（connection・incarnation・generation）を一つの session id へ潰していない。`MessageId` を domain identity に再利用せず、transport 抑止と domain idempotency を分離した。
- **Remote-capable 選別。** IB・CM の 7 群起点で wire 化の要否を再判定し、Host-local（形成・制御確定・秘密利用・割当・予約・認可・作用確定・範囲確定・完了確定・switch・repository compare）を越境させていない。Observer assignment・routing semantic を公開していない。
- **Pattern 分離。** request/response・command+ack・fact・subscription・stream・progress+completion を区別し、generic Event へ統合していない。envelope と payload を分離し、envelope を owner にしていない。
- **Presence / I/O / Observation / Action。** 二重 active 禁止・移行中新規開始禁止・旧 round 付け替え禁止・生成≠提示・旧 stream 非継続・ticket 制・class wipe（本文非再送）・command 到着≠成功・自動 retry 禁止・cancel 四分離をいずれも満たす。
- **Versioning / capability / auth / transport。** claim≠Permission、秘密の通常 payload 非載せ・pairing・revoke・re-auth、version matrix・unknown 拒否・guess 禁止、wire/transport 分離・adapter boundary、per-domain backpressure・global ordering 不要求を満たす。
- **Security / error。** 非露出・untrusted validation・DTO≠boundary、transport/domain error 分離・typed reject・共通 enum 化の禁止を満たす。
- **Crate。** `ene-api` の DTO 限定・mapping の別配置・依存方向の維持を満たす。新規 crate・依存追加をしていない。
- **修正。** レビューで見つけた表現上の不足（retry 時の `command_id`＋新 `message_id` の関係の明記、capability 変化時の既発行 ticket 延命禁止の明記、asset cache の transient 扱い・wipe 参加の明記）は本書へ反映済みである。Step 11・Step 12・CI・PR・CCT・IB・CM・requirements の変更は不要であった。

## 29. Escalation — Requirement / Architecture Issue の有無

- **Requirement 変更。** なし。
- **Step 11 / Step 12 semantic contract の変更。** なし。
- **CI / PR / CCT / IB / CM contract の変更。** なし。
- **semantic owner 変更。** なし。
- **Host / Client boundary 変更。** なし。
- **Security / Privacy semantics 変更。** なし。
- wire / serialization / transport の選択（MessagePack canonical＋JSON-compatible model、local socket＋WebSocket/TLS、adapter boundary）は Issue ではない。

## 30. 意図的に残した Design Freedom

- 具体暗号 library・key format・鍵導出・証明書運用、pairing material の具体形式・保存方式、nonce・proof の具体方式。
- heartbeat / keepalive / timeout / retry 回数・値、`message_id` cache 期間・`command_id` 保持期間、frame 上限値・同時 stream 上限値。
- 具体 TCP port・mDNS 有無・NAT traversal（relay は導入しない）。
- audio codec・capture 画像形式・解像度上限・chunk size、asset chunk size・cache 上限。
- capture 時機・stagger algorithm、費用予約量算定式・集計期間、BodyState hint の粒度・更新頻度。
- view の具体項目・表示文言・UI layout・audit format・提示確認の具体 UI。
- 上記の対応関係から統一 Context layer、Policy Engine、Manager、Service、Coordinator、schema registry、consensus、global ordering の追加を導かない。既存の責務、semantic owner、Host／Client 配置と trust boundary の下で実現方法を選ぶ。

## 31. 次工程への申送り

### 31.1 固定前提として使えるもの

- 第2節の remote-capable 選別（W-1〜W-10 と Host-local の区別）。
- 第3〜5節の layer・pattern・envelope。
- 第6節の identity 分離と発行者規則、第11節の incarnation / stale 拒否。
- 第7節の serialization・version matrix・field 規約、第8節の capability、第9節の pairing / auth / revoke、第10節の transport・adapter boundary。
- 第12〜19節の domain 別 wire semantics。
- 第20・21節の inventory・DTO、第22節の stream semantics、第24節の rejection model。
- 第25節の crate placement（`ene-api::v1`・Host `ipc_map`・Client `ipc`）。

### 31.2 Step 13 でまだ具体化すべき領域（本書の対象外として残したもの）

- 具体 DB schema・index・migration code（PR §4 と IB §13 の repository premise を材料にする）。
- concurrency mechanism の確定（CCT の SD・AU と compare を材料にする）。
- Provider protocol adapter・MCP・Plugin の受入境界の concrete API（IB K-I の extension 種別を材料にする）。
- Client capture・audio・device adapter の crate 分離時期と platform 隔離の確定（CM 第9節の app 内 module 開始を材料にする）。
