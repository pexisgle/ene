# Architecture Review #3

状態: 独立レビュー（Walking Skeleton 着手前）
最終確認: 2026-09-08
Reviewer: 各 artifact の作成者とは独立した Architecture Reviewer

対象:

- 要件正本: `docs/requirements/`（README、product、requirements、acceptance、references）
- Upper Architecture: `docs/design/architecture/`（architecture-drivers、system-context、runtime-topology、subsystems、state-ownership、dependency-rules、runtime-flows、cross-cutting）
- Critical Area Detailed Design: `docs/design/critical-areas/`（context-assembly、action-execution、targeted-deletion、client-presence-transition、backup-restore）
- Subsystem Detailed Design: `docs/design/subsystems/`（companion-task-learning、permission-credential-execution-inference、client-presence-io-observation、data-preservation-erasure、character-distribution）
- Concrete Design: `docs/design/concrete/`（crate-module-decomposition、interface-boundaries、host-client-ipc、persistence-recovery、concurrency-control、correspondence-identity）

参照（非正本・経緯確認のみ）: `reviews/architecture-review-1.md`、`reviews/architecture-review-2.md`、`reviews/architecture-drivers-review.md`。これらの判定、および各 artifact 末尾の「新たな Issue なし」宣言は前提にしていない。既存実装 crate・Git 履歴は判断根拠にしていない。

略号: AD（architecture-drivers）、SC（system-context）、RT（runtime-topology）、SS（subsystems）、SO（state-ownership）、DR（dependency-rules）、RF（runtime-flows）、CC（cross-cutting）。CA-CTX / CA-AE / CA-TD / CA-CPT / CA-BR（critical-areas 各書）。SS-CTL / SS-PCEI / SS-CPIO / SS-DPE / SS-CD（subsystems 各書）。CM（crate-module-decomposition）、IB（interface-boundaries）、IPC（host-client-ipc）、PR（persistence-recovery）、CCT（concurrency-control）、CI（correspondence-identity）。`R L472` は `requirements.md` の行番号。

レビュー方法: 要件 5 文書を先に読み、要件から独立に「concretization で崩れやすい意味」を評価軸として立てた（第2節）。次に precedence 順（Requirements → Upper → CA → SS → Concrete）に全 artifact を読み、Concrete 層の table・DTO・crate 依存・SD 順序・message inventory の一つ一つが、上位のどの ownership・authority・trust boundary・lifecycle 規則を実現しているかを逆引きした。上位規則の「担い手」が Concrete 層に存在しない箇所、Concrete 層が上位と異なる規則を導入している箇所、Concrete artifact 同士が互いを根拠に引いているが実際には閉じていない箇所を探した。正常系に加えて、切断・再接続・Host 再起動・Restore・失効・crash・cap 到達・削除の異常系を、要件から Concrete まで縦に辿った（第5節）。

本書はレビュー結果のみを記録する。要件・design artifact・実装 code は変更していない。

## 1. Overall Verdict

- **Verdict: CHANGES REQUIRED**（修正は局所的。Walking Skeleton は現在の design をほぼ固定したまま着手できる）
- Critical: 0
- Major: 3
- Minor: 4
- Requirement Ambiguity: 1（RA-01）
- Upper Architecture Issue: 0

総括。Concrete 層への具体化は、上位設計の中核的な意味を保っている。Host 正本と Client 一時 data、caller ≠ authority、保存 Allow の非再利用、秘密値の非露出（`SecretValue` 非公開・E 分類・backup 除外）、作用不明の粘着、targeted deletion の durable-before-enforce、restore generation による旧世代参照の遮断、presence generation による帰属 lifecycle、identity / revision / generation の分離は、IB の command / outcome、PR の table 群、CCT の SD 順序、CI の correlation、CM の crate 依存、IPC の envelope / DTO まで一貫している。crate 依存グラフは DR の allowed / prohibited を一方向に写しており、中央 orchestrator や dumping ground は導入されていない。

一方、**Remote Client 周辺の lifecycle と trust boundary** に限って、Concrete 層（主に IPC、一部 PR）が上位規則と異なる規則を導入し、あるいは上位が要求する security property の担い手を欠いている箇所が三件ある（F-01〜F-03）。三件とも要件と Upper〜Subsystem 層は一貫しており、逸脱は Concrete 層で発生している。修正はいずれも IPC / PR / CM の局所記述の追記・書換で閉じ、crate 分割・依存方向・IB の interface 群・table 群・SD 順序を変えない。Walking Skeleton の主経路（Host 同居 Client のテキスト対話・Task Agent の file 作業・Host 再起動後の presence 復旧と明示再開・Text 管理面）に触れるのは F-01・F-04 と表記訂正の F-06 であり、いずれも数行の修正である。F-02・F-03 は Remote Client / Backup-Restore の実装前に閉じればよい。

## 2. 評価軸 — concretization で起きやすい意味変化の型

要件と上位設計から独立に、以下六つの型を立て、Concrete 層の全記述をこの型で検査した。

| 型 | 内容 | 本レビューでの検出 |
|---|---|---|
| (a) lifecycle drift | 上位で「意味」として置かれた遷移規則が、下位で mechanism の都合により別規則に置換される | F-01 |
| (b) authority 合流 | 上位で分離されていた authority が、DTO / table / crate の共有を通じて事実上合流する | 検出なし（第5節 S-09〜S-11） |
| (c) security property の担い手欠落 | 上位が性質として述べた secret / trust の property に、下位のどの table・class・message も担い手として対応していない | F-02 |
| (d) trust boundary の拡張 | 上位で Host 側だった判断入口が、wire 化により remote へ広がる | F-03 |
| (e) bootstrap gap | protocol / flow の起点または終点が inventory から抜ける | F-04、F-05 |
| (f) artifact 内不一致 | 同一 artifact の規則部と walkthrough 部が食い違う | F-06、F-07 |

## 3. Findings

Severity は「この design を固定して Walking Skeleton とその後の実装へ進んだ場合の手戻りの大きさ」で付けた。各 Finding に、上位設計の欠陥か、Concrete 層の逸脱か、Requirement Ambiguity かを明記した。

### F-01 — 通常 Client 切断時の presence 契約が、IPC で「保持＋再接続待ち」へ置換されている

**Severity:** Major

**分類:** Concrete artifact（IPC）の上位契約からの逸脱（型 (a)）。要件・Upper・CA・SS は一貫している。

**Affected artifacts:**
IPC §12.2 第4項（L415）、§12.3 `MoveReason`（`OwnerSummon | PriorInstruction | SpontaneousNeed | ReconnectRecovery`、L425）、§21 同 DTO、§26 V-5（切断中の Action のみ扱い presence の遷移先を描かない）。CCT §10.2 disconnect 項（切断検知の一時状態のみを扱い、通常切断が確定した後の遷移先を定めない）。PR Group G 末尾（L200-201。再起動復旧のみ記述）。Concrete 層六書のいずれにも「Host PC 上の Client」に相当する概念・判定材料がない。

**上位の規則:**
R L472「通常のClient切断では、Running Companionを基本的にHost PC上のClientへ移動する。Host再起動時は…presence復旧契約を適用する」。RT-02（L63）・RT「Client終了・Remote切断」（L86）、AD L61・L207・L399、SS「接続・存在」（L216）、CA-CPT §4.3（L111）・§8.1 表（L174）、SS-CPIO（L199）のすべてが「通常切断 → 利用可能な Host PC 上の Client へ移動、なければ active なし。Host 側 Client 環境の自動起動はしない。再接続待ちは Host 再起動の復旧契約に限る」を同じ意味で保っている。CA-CPT §8.1 はさらに「一時的な network loss・短時間の到達不能（現在帰属を直ちに捨てない）」と「Client process 終了・通常の Client 切断（Host PC 上の Client へ移動）」を別行で区別している。

**Finding:**
IPC §12.2 は「通常の Client 切断では Running Companion の帰属は Host 側の current として保持し、再接続・復旧の確認を経て確定する」と規定し、`MoveReason::ReconnectRecovery` を Client 起点の `MoveIntent` の理由として置いている。これは「切断した Client が戻るまで帰属を保持し、戻ったら復旧する」model であり、CA-CPT §8.1 が一時的な到達不能に限って認めた「帰属を直ちに捨てない」扱いと、要件が Host 再起動時に限定した復旧契約を、通常切断へ広げている。CCT §10.2 の disconnect 項も一時状態の扱いに留まり、切断が確定した後の遷移先を持たない。要件の「Host PC 上の Client へ移動、なければ active なし」経路は IPC・CCT・PR のどこにも現れず、Host 発の切断 fallback 移動を表す `MoveReason` も `presence_transition_log` の理由区分もない。Concrete 層には「Host PC 上の Client」を識別する材料（IPC §10.1 の same-machine transport 区分は存在するが、presence 側の判定材料としては結び付けられていない）もない。

**影響:**
Remote Client が切断した瞬間の Owner 体験が要件と異なる。要件では Companion は Host PC の Client へ現れ、Owner は Host PC で会話を続けられる。IPC の規則では Companion は「切断した Client の current」として見えないまま留まり、切断 Client の再接続を待つ。さらに、`ReconnectRecovery` を Client 起点の `MoveIntent` として受けると、再接続 Client の主張が帰属復帰の trigger になり、R L481「接続済み Client への自発的な移動は…自動化・義務化しない」および CA-CPT の「再接続した Client の古い一時 state だけで現在 presence を成立させない」と摩擦する。Walking Skeleton（Host 同居 Client のみ）でも、Client process の crash → 再起動時に「Host PC 上の他の Client がないので active なし → 再起動後は呼出し・事前指示・自発判断で復帰」（要件）と「保持 → 再接続で自動復帰」（IPC）の二通りの実装が可能になり、presence 実装の初期段階で分岐する。

**修正方向（最小）:**

1. IPC §12.2 第4項を書き換える。「通常の Client 切断が確定したとき、Host は利用可能な Host PC 上の Client（same-machine transport で認証済み・device 許可あり・排他性確認可）があれば SD-Presence の CAS で `旧 → 移行中 → Host PC Client` へ遷移し、なければ `NoActive` へ確定する。Host 側 Client 環境を自動起動しない。切断 Client の再接続を待つ復旧は Host restart restoration（`RecoveryWait`）に限る」。
2. `MoveReason` に Host 発の `DisconnectFallback`（または同義）を追加し、`ReconnectRecovery` は Host restart 後の `RecoveryInvite` に対する応答としてのみ使うことを明記する。Client 起点の `MoveIntent` で `ReconnectRecovery` を送れないようにする（Host → Client の `TransitionAck` / `PresenceAttributionFact` の理由としてだけ現れる）。
3. CCT §10.2 の disconnect 項に「切断確定（一時的な到達不能と区別）→ fallback 移動または `NoActive`」を追記し、SD-Presence の CAS 対象であることを示す。一時的な到達不能から切断確定への境界（timeout 等）は CA-CPT §12 の Freedom のまま残してよい。
4. 「Host PC 上の Client」の判定材料を一箇所に置く。最小案は PR Group G `client_last_connection` に transport class（same-machine / remote）を持たせ、`ene-presence` が fallback 先の候補判定に使う。判定の authority は `ene-presence`（帰属）と `ene-permission`（device 許可）に留め、Client の自己申告を根拠にしない。

crate 分割・IB の command 群・table 追加は不要である。

### F-02 — device 失効が Restore で巻き戻り得る（PR と IPC の相互参照が閉じていない）

**Severity:** Major

**分類:** Concrete artifact 間の不整合（型 (c)。security property の担い手欠落）。上位は一貫している。

**Affected artifacts:**
PR Group F `device_permission`（`(device_id, function)`、「許可機能・失効状態」、**D1**、L183）。PR §9.1「含める」行に「device」（L439）。PR §9.2「現在 environment から維持するもの」（L447-452）に device 失効・Host 側 device 検証材料がない。PR §9.3 L461「復元された…Client device 参照…は、現在の Credential・制約・保留・到達性・排他性と照合して初めて利用できる」。IPC §9.2 第2項「device record（descriptor・許可された機能範囲・失効 flag・最終接続）を durable に保持する」（L316）。IPC §9.4 第3項「Host restart・Restore 後も失効は維持する。Restore で過去の pairing・許可を復活させない（PR §9）」（L328）。CM §7.2 表「Client 接続材料の秘密部分 → `ene-credential` + `ene-presence` の非秘密参照」（L205）。PR には device record 本体（descriptor・`DeviceWireId` 対応・Host 側の所有証明検証材料）に対応する table も durability class もない。

**上位の規則:**
R L462「deviceごとに失効できる」、R L464「古い接続材料だけでHost側のpairing・許可を復活させない」。SO §8（L525-527）「Device失効・全データResetではHost側の信頼を失効させ、旧Client材料だけで再接続の信頼を復活させない…Restoreも復元されたdevice参照・許可と現在の接続・認証の成立を照合し、secretをbackupから戻さない」。CC L82。CA-BR「secret を backup から戻さない」。

**Finding:**
失効状態は `device_permission`（D1）に置かれ、D1 は backup に含まれ、Restore で backup 時点へ全置換される。失効前に取った backup を Restore すると `device_permission` は失効前の値へ戻る。IPC §9.4 は「Restore で過去の pairing・許可を復活させない」の根拠を PR §9 に求めているが、PR §9.2 の維持対象は Credential store の秘密値本体（E）だけで、device 失効も Host 側の device 検証材料も含まれていない。

SO §8 の安全弁は「復元された device 参照を現在の認証成立と照合する」ことである。この安全弁は、**失効時に Host 側の検証材料が無効化されている**場合にだけ働く。しかし IPC §9.4 の失効手順は「device record に durable に記録し、現 connection を切断し、以後の auth を拒否する」であり、検証材料の無効化を含まない。IPC §9.3 が認証方式を Freedom としているため、Host 側検証材料が非秘密（例: Client 公開鍵）として DB に置かれる実装が自然に選ばれ得る。その場合、材料も `device_permission` も backup に入り、Restore で両方が戻り、失効済み Client が旧材料で再認証に成功する。これは R L464・SO §8 が禁じる「旧材料だけで Host 側の pairing・許可を復活させる」経路である。

**影響:**
Owner が「device を失効させた」と理解した後に Restore を行うと、失効した端末が再接続できる。失効は通常、端末紛失・侵害の疑いで行うため、security 上の影響が大きい。Walking Skeleton の範囲（Host 同居 Client のみ・Backup/Restore は後続）には含まれないが、pairing / Restore の実装前に設計として閉じておかないと、認証方式の選択次第で property が成立しなくなる。

**修正方向（最小）:**

1. PR に device record（pairing identity・descriptor・Host 側の所有証明検証材料・失効）の置き場所を明記する。Host 側の検証材料は非秘密であっても **E（Credential store 側）または §9.2「現在 environment から維持するもの」に分類**し、backup に含めず Restore で置換しない。`device_permission`（D1）は許可機能の記録として backup に残してよいが、「復元された device 行は現在の E 側材料と対応が取れて初めて有効」と PR §9.3 に明記する（既存 L461 の「現在の Credential」を「現在の device 検証材料」まで明示的に広げる）。
2. IPC §9.4 の失効手順に「Host 側検証材料（E）の無効化・削除」を追加し、「以後の auth を拒否する」の担い手を DB の失効 flag ではなく E 側材料の不在にする。§9.4 第3項の根拠を「PR §9.2 の維持対象」へ具体的に向ける。
3. crate 配置は SO 4.15 L248 / CM §7.2 L205 に従い、検証材料の保持を `ene-credential`（device-auth 用途の store）、失効の意味判断を `ene-permission`、接続への適用を `ene-presence` に置く。既存の allowed dependency で成立し、新規依存は不要である。

### F-03 — pairing 承認・device 失効・Credential 設定・Restore / Reset 要求が、remote Client の管理面から送れる

**Severity:** Major（RA-01 の解決に依存。解決結果が「remote からも可」であれば Minor へ降格し、IPC に明示的な決定記述を残すだけでよい）

**分類:** Concrete artifact（IPC）による trust boundary の拡張（型 (d)）＋ Requirement Ambiguity（RA-01）

**Affected artifacts:**
IPC §9.2 第1項「Host は Owner 確認待ち（管理面）にする」（L314）。IPC §2.1 W-9「Owner 管理面の入口と表示 — remote-capable」（L71）。IPC §18 `ManagementIntentKind`（`ManageDevice | ConfigureCredentialIntent | RequestDeletionBackupRestoreReset` を含む、L580-582）。IB §15 remote-capable 行（L1404）。

**上位の規則:**
R L461「新しいClientはOwnerがHost側で確認できるdevice pairingを必要とする」。SO 4.15 L248「Host側の確認に基づくpairingの許可」、SO §8 L527「再pairingは現在のHost側確認に基づき」、CA-CPT §8.2「再pairingは現在のHost側確認に基づく」。SC L33「pairingによってClientのOS・周辺環境全体まで信頼するものではない」、SC L53「ネットワーク内にいるだけで信頼せず、新ClientのHost側で確認できるpairing…をeneが担う」。AD L41。

**Finding:**
IPC は Owner 管理面を remote-capable とし、pairing 承認の入口を「管理面」と定めた結果、任意の pairing 済み Client（remote を含む）から `ManageDevice` を送って新 device の pairing を承認し、他 device を失効させ、`ConfigureCredentialIntent` で Provider / MCP Credential を設定し、`RequestDeletionBackupRestoreReset` で Restore / 全データ Reset を要求できる protocol になっている。IPC §18 は「Client 送信は intent であり Host owner が確定する」と authority の位置を守っているが、確定の入力である Owner 確認そのものが remote Client 上で行われることを妨げていない。

上位設計は pairing 確認を一貫して「Host 側」と表現している。この語が「Host に到達し Host が記録する確認」なのか「Host PC 上で行う確認」なのかは要件側で曖昧である（RA-01）。しかし SC L33 の「pairing は Client の周辺環境全体を信頼することではない」を前提にすると、pairing 済み remote Client 一台の侵害が、(1) 攻撃者 device の永続的な pairing、(2) Owner の他 device の失効、(3) Credential の差替え、(4) Restore / Reset による data 破壊、へ推移的に拡大する経路を protocol が用意していることになる。少なくとも (1) は「pairing の承認を pairing 済み Client に委ねる」構造であり、Host 側確認という上位の意図から外れている可能性が高い。

**影響:**
Remote Client の実装前に決めなければ、`ManagementIntent` の受入判定に「送信元 Client の transport class・device 種別」という条件を後付けすることになり、IPC §2 の remote-capable 選別と IB §15 の表を書き直す。Walking Skeleton（Host 同居 Client・Text 管理面）には影響しない。

**修正方向（最小）:**

1. RA-01 を解決する（第4節）。推奨は「pairing 承認・device 失効・全データ Reset・Restore の確認は Host PC 上の Client（same-machine transport）に限定する。remote Client は device 一覧・最終接続・許可機能の filtered view と、**自身の** device の失効要求のみ送れる」。
2. 決定を IPC §2（remote-capable 選別）と §18 に反映する。最小案は `ManagementIntentKind` のうち `ManageDevice`（pairing 承認・他 device 失効）・`ConfigureCredentialIntent`・`RequestDeletionBackupRestoreReset` を Host-local surface へ移すか、Host が送信元の transport class を照合して remote からは `DeniedByBoundary` で返す規則を §18 に追加する。DTO 追加は不要である。
3. IB §15 remote-capable 行の「管理面の表示・操作」に同じ限定を追記する。

### F-04 — 初回 `SubmitTextInput` に必要な `RoundWireId` を Client が得る手段が message inventory にない

**Severity:** Minor（ただし Walking Skeleton の最初の実装対象であり、着手前に埋めるべき）

**分類:** Concrete artifact（IPC）の bootstrap gap（型 (e)）

**Affected artifacts:**
IPC §21 `SubmitTextInput { round: RoundWireId, ... }`（L670-675）、`RoundIntakeOutcomeWire::StaleRound { current_round: Option<RoundWireId>, .. }`（L679）、`PresenceAttributionWire`（round を含まない、L662-667）。IPC §6.3 L223・§13.1 L453「round は Host 発行の `RoundWireId`」。§12.1 表・§20 inventory・§21 DTO に round を開始・通知する message がない。CM §12 walkthrough 1（L420）。CM §5.2 L168（`RoundId` の実際は `ene-presentation`）。

**Finding:**
round は Host 発行であり、Client は `SubmitTextInput` に必ず `RoundWireId` を載せる。しかし Host が Client へ「現在の round」または「新規 round」を伝える message が inventory にない。`StaleRound` の `current_round` は失敗応答でしか届かない。Client が最初の入力を送る正当な手段がなく、実装者は「ダミー round で送って `StaleRound` から拾う」「Client 側で round を生成する（Host 発行の原則に反する）」「presence fact に round を勝手に足す」のいずれかへ流れる。

**影響:**
Walking Skeleton の第一歩（Owner のテキスト入力）で最初に踏む。設計上の欠落は小さいが、実装者の場当たり対応が「Client が round identity を作る」方向へ進むと、CI §3.5 の「旧 round の入力を新 round へ付け替えない」と IPC 原則 2「Client-originated は authority ではない」に触れる。

**修正方向（最小）:**
次のいずれか一つ。(a) `SubmitTextInput.round` を `Option<RoundWireId>` とし、`None` を「現在の presence generation の下で新規 round を開く要求」と定義し、Host は `AcceptedForRound { round }` で発行 round を返す。(b) `PresenceAttributionFact` または `AuthResult` に `current_round: Option<RoundWireId>` を含め、Host 側 `ene-presentation::round` が presence 確定時に round を開く。(a) は変更が DTO 1 箇所で済み、round 発行 authority を `ene-presentation` に保つ。

### F-05 — crash で孤立した `Reserved` 予約の遷移主体と遷移先が記述されていない

**Severity:** Minor

**分類:** Concrete artifact（PR / CCT / IB）間の記述欠落（型 (e)）。上位契約との矛盾ではない。

**Affected artifacts:**
IB K-G（`ReserveUsageCommand` / `CommitUsageCommand` / `ReleaseUsageCommand`、`ReservationOutcome::Reserved`、L514-543）。CCT §9 SD-Cap（三状態）。PR L78「処理中・未報告・不明消費の保全（ゼロ化・リセットしない）」、Group F `usage_fact_*`（L187-188）。PR §6.2 の再起動時再評価。

**Finding:**
予約は「Reserved → Committed（報告 / 推定 / 不明）または Released」の三状態で、commit / release は予約した利用 owner の in-flight 処理が行う。Host process の crash や Task Agent の停止で in-flight 処理が消えると、`Reserved` 行は commit も release もされない。PR の「処理中をゼロにしない」原則からこの行は cap 集計に含まれ続けるべきであり、Released にしてはならない。ここまでは原則から導ける。しかし、(1) 再起動時にこの行を「処理中」から「不明」へ確定する主体（各利用 owner の再起動再評価か、`ene-permission::cap` か）、(2) その操作の interface（K-G には元 caller 以外が呼ぶ「不明として確定」がない）、(3) Owner が管理面で不明消費を確認し扱う経路、が記述されていない。

**影響:**
実装者が「再起動時に Reserved を release する」clean-up を書く誘因があり、書けば「不明をゼロ化しない」契約に反する。逆に何もしなければ crash ごとに cap が永続的に食われ、Owner に理由が示されない。いずれも実装を止めないが、Walking Skeleton の Task Agent 委任経路で発生し得る。

**修正方向（最小）:**
PR §6.2（または CCT §9）に一文を追加する。「再起動時、所有 in-flight を失った `Reserved` は、当該利用 owner の再起動再評価で `CommitUsageCommand(actual = 不明)` により確定する（release しない）。不明消費は費用管理面に区別して表示し、Owner の扱いは Design Freedom」。IB K-G の `CommitUsageCommand` が再評価経路からも呼べることを注記する。

### F-06 — CM 内で `ene-task` から `ene-permission` への呼出主体が食い違う

**Severity:** Minor

**分類:** Concrete artifact（CM）内の不一致（型 (f)）

**Affected artifacts:**
CM §10.1 L271「`ene-task` … `ene-permission` 具体への依存禁止（premise 供給で満たす）」、L273「`ene-action` → `ene-permission`（K-B live check…の caller）」、L289 prohibited reverse。CM §12 L421「`ene-task`（受理・`CreateDelegationCommand`）→ `ene-permission`（live check。**Task が caller**）→ `ene-credential` → `ene-action`」。

**Finding:**
§10.1 は `ene-task` → `ene-permission` を禁止し、K-B live check の caller を `ene-action`（および `ene-inference`）に置く。§12 の walkthrough はその直後に「Task が caller」として `ene-task` → `ene-permission` の直接呼出を描く。§12 の同じ段落は `ene-task` → `ene-credential` の順路も描くが、§10.1 では `ene-task` は `ene-credential` に依存しない。

**影響:**
Walking Skeleton の Task Agent file 作業を実装する際、walkthrough を読んだ実装者が `ene-task` に `ene-permission` / `ene-credential` 依存を追加すると、DR「作業 → 権限・制約」の premise 供給原則と CM の依存グラフが崩れる。

**修正方向（最小）:**
L421 を「`ene-task`（受理・`CreateDelegationCommand`・cost 予約は自 crate の gate trait 経由）→ `ene-action`（`ExecuteActionCommand`。内部で `ene-permission` live check と `ene-credential` の限定利用を caller として行う）→ …」に書き換える。

### F-07 — Observer routing 文脈の「収集」主体の表現が、composition root を意味選択者に見せる

**Severity:** Minor

**分類:** Concrete artifact（CM）の表現の曖昧さ（型 (f)）。上位（SO 4.16、Review #2 F-01 の解決）とは整合し得るが読み方が二つある。

**Affected artifacts:**
CM §12 L425「Host 供給の routing 文脈（`ene-companion` History・`ene-learning` Memory・`ene-task` context を **`apps/ene-core` が premise として収集**。`ene-observer` は三者に依存しない）」。CM §6 L191「`apps/ene-core` は wiring・lifecycle・storage init・route 配線に限定し、採否・達成・許可・確定度を決めない」。CM §2 原則 7・8。SO 4.16。IB H-F・§10 field matrix（L1072。元 owner・対象 Companion・用途・選択前提を持つ）。CI §5.7・§8.2 L591 `RoutingContextRef`。

**Finding:**
「収集」が「各 owner が自ら範囲を限定して生成した `RoutingContextRef` を `ene-core` が中継・束ねる」意味なら、SO 4.16（各 owner が自身の選択・範囲に責任を持つ）と CM 原則 7・8 に整合する。しかし「`ene-core` が History・Memory・Task context を読み、routing に使う部分を選ぶ」と読むと、composition root が Companion 固有情報の利用範囲を決める意味選択者になり、CM が禁じた中央 orchestrator に近づく。IB H-F の `RoutingContextOffer` は `source_owner`・`selection_premise` を持つ「供給（offer）」であり、生成主体が各 source owner であることを示している。CM の walkthrough だけがこれを「`ene-core` が収集」と表現しており、IB と CM の間で読み方が分かれる。

**影響:**
Observer は Walking Skeleton の範囲外であり、実装を止めない。Observer 実装時に `ene-core` に選択 logic が置かれると、後から各 owner crate へ移す手戻りになる。

**修正方向（最小）:**
L425 を「各 source owner（`ene-companion` / `ene-learning` / `ene-task`）が IB H-F の `RoutingContextOffer` を自身の範囲限定で生成し、`apps/ene-core` はそれを `ene-observer` へ中継する。選択・範囲判断は各 owner の pub API に置き、`ene-core` は内容を読まない」に書き換える。

## 4. Requirement / Architecture Issue

### RA-01 — 「Host側で確認できる device pairing」の確認場所

R L461「新しいClientはOwnerがHost側で確認できるdevice pairingを必要とする」。「Host側で確認できる」は、(i) 確認が Host に到達し Host が正本として記録する、(ii) 確認操作を Host PC 上で行う、の二通りに読める。SO L248・L527、SC L33・L53 は同じ語を使い、いずれの読みも排除していない。Concrete 層（IPC §9.2・§18・W-9）は (i) を採り、確認操作を任意 Client の管理面に置いた（F-03）。

確認すべき点: pairing 承認、他 device の失効、Credential 設定、Restore / 全データ Reset の確認を、remote Client の管理面から行えるようにするか。SC L33「pairing は Client の周辺環境全体を信頼することではない」を前提とすると、少なくとも pairing 承認は Host PC 上の Client に限定するのが自然である。Owner 体験としては「新しい端末を追加するときは Host PC で承認する」という一回限りの操作であり、日常会話・Task 開始に管理画面操作を要求しない R L24 と衝突しない。

### Upper Architecture Issue

なし。F-01〜F-03 の原因となる矛盾・欠落は AD / SC / RT / SS / SO / DR / RF / CC、および CA・SS 各書の間に見つからなかった。上位は今回逸脱が起きた三点（切断時の Host PC Client への移動、device 失効の Restore 非復活、Host 側 pairing 確認）を一貫した意味で保持している。逸脱はすべて Concrete 層で発生しており、上位を書き直す必要はない。

## 5. 意味の維持を確認した scenario

要件から Concrete まで縦に辿り、ownership・authority・security・lifecycle が変化していないことを確認した scenario。「維持」は本レビューが独立に確認した結果である。

| # | scenario | 辿った artifact | 結果 |
|---|---|---|---|
| S-01 | Owner が Host 同居 Client からテキスト入力 → 推論 → 応答 → Learning 形成 | CM §12 walkthrough 1、IB H-A/X-B/K-D/K-E、IPC §7・§13・§21、PR Group B、CCT SD-Round | 維持。round 発行 authority は `ene-presentation`、対話意味は `ene-companion`、送信条件は `ene-inference` が `ene-permission` を caller として照合。初回 round の取得手段のみ欠落（F-04） |
| S-02 | Task Agent が file 作業を開始 → cap 近傍で予約 → 途中 crash → Host 再起動 | IB K-G/K-H、CCT SD-Cap/SD-Task、PR Group D/F・§6.2、CM §12 walkthrough 2 | 維持。作用不明は `Unknown` のまま、Task は明示再開待ち、保存 Allow は再利用されない。孤立予約の遷移主体のみ未記述（F-05）、walkthrough の caller 表記のみ不一致（F-06） |
| S-03 | Remote Client で Running 中に通常切断 | R L472、RT-02、CA-CPT §4.3・§8.1、SS-CPIO、IPC §12.2・V-5、CCT §10.2、PR Group G | **不一致**（F-01）。上位は Host PC Client への移動、IPC は保持＋再接続待ち |
| S-04 | Host 再起動 → 再起動前 Client への presence 復旧 | RT-02、SO 4.15 L251、RF L196、CA-CPT §8.3、PR Group G、IPC §12.2 第5項・V-10、CCT §10.2 | 維持。`RecoveryWait` → 現接続・許可・排他性の確認 → `Present` / `NoActive`。別 Client への無条件移動・Stopped への適用・Task 再開権限化なし |
| S-05 | 失効前 backup → device 失効 → Restore | R L462・L464、SO §8、CA-BR、PR §9、IPC §9.4 | **不一致**（F-02）。失効状態が D1 として backup に含まれ、Host 側検証材料の class が未定 |
| S-06 | Remote Client の管理面から新 device の pairing を承認 | R L461、SO L248・L527、SC L33、IPC §9.2・§18・W-9 | **要決定**（F-03 / RA-01） |
| S-07 | targeted deletion 進行中に in-flight 推論結果・Tool 結果が遅延到着 | CA-TD、IB D-A〜D-E・`erasure_check`、CCT SD-Deletion、PR Group J、CI §6.4 | 維持。durable-before-enforce、消去条件を用途別受入で照合、遅延再保存を防止。完了確定は `ene-preservation` の集約のみ |
| S-08 | Restore 途中で旧 live の Task 結果が到着 | CA-BR、PR §9.3・§9.5、CI `RestoreGeneration`、IB `restore_premise` | 維持。旧世代 tag の参照は利用・実行・送信されない。置換前後で正本は常に一つ |
| S-09 | 保存された Allow / 委任時 copy / context 内許可文による Action 開始 | R（Permission）、SS-PCEI、IB K-B/K-H、PR `permission_evaluation`（D2「生きた許可ではない」）、CCT SD-Action、CM 原則 6 | 維持。開始前 atomic compare で現在条件を再照合。Allow copy が authority に昇格する経路なし |
| S-10 | Client からの ack・view revision・intent による状態確定 | IPC 原則 2、§12.2 第3項（`PresencePresentedAck`）、§18 `base_view`、IB §15 | 維持。Client-originated は premise / candidate のみ。Host 側 owner が確定 |
| S-11 | Credential 値の露出経路（context・Tool 引数・History・Learning・backup・wire） | R、SO 4.21、DR-05、IB K-C、CM 原則 6（`SecretValue` 非公開）、PR E 分類・§9.1、IPC 原則 3 | 維持。値を返さない closure 利用、E は backup 除外、wire へ出さない。Provider plugin が別 process の場合の受渡しは Design Freedom（第7節） |
| S-12 | Companion 削除後の usage・Task 記録・Global Learning 根拠 | R（停止と削除）、SS-CTL、CA-TD、PR Group B/D/F の deletion 列 | 維持。費用 log・使用量をリセットしない、Task 記録は残る、共有根拠の利用関係を保ち全文 access を広げない |
| S-13 | Character revision 適用と Companion 固有 state | SS-CD、CI §5、IB C-A〜C-D、PR Group A | 維持。適用禁止種別の除外、Package 更新を成長の初期化にしない |
| S-14 | crate 依存グラフと DR の allowed / prohibited | DR §3〜§7、CM §10 | 維持。owner → caller の逆依存なし。双方向に見える依存は trait 反転・premise 供給・Host 媒介で一方向。`ene-store` → owner のみ（adapter → domain）。`ene-api` は domain に依存しない |
| S-15 | identity / revision / generation の混同 | CI 全体、IPC §7、PR §10 | 維持。`MessageId` を domain identity に流用しない。connection / incarnation / generation を一つの session id に潰さない。revision と generation を代替しない |

## 6. Walking Skeleton 前に必要な修正と進行判定

### 6.1 着手前に修正（Walking Skeleton の主経路に触れる）

| Finding | 修正箇所 | 規模 |
|---|---|---|
| F-01 | IPC §12.2 第4項の書換、§12.3 / §21 `MoveReason` の variant 追加と `ReconnectRecovery` の用途限定、CCT §10.2 への切断行追加、PR Group G `client_last_connection` への transport class 追記 | 各数行。table・crate・IB command の追加なし |
| F-04 | IPC §21 `SubmitTextInput.round` を `Option` 化し意味を定義（または presence fact に `current_round`） | DTO 1 箇所 |
| F-06 | CM §12 L421 の書換 | 1 行 |

### 6.2 Remote Client / Backup-Restore の実装前に修正

| Finding | 修正箇所 | 規模 |
|---|---|---|
| F-02 | PR に device record・Host 側検証材料の class（E または §9.2 維持対象）を追記、§9.3 L461 の照合対象を明示、IPC §9.4 に E 側材料の無効化を追加 | 各数行。crate 依存の追加なし |
| F-03 | RA-01 の決定 → IPC §2 / §18・IB §15 の remote-capable 区分に限定を追記 | 決定 1 件＋数行 |

### 6.3 任意（Observer / 費用管理面の実装前）

F-05（PR §6.2 または CCT §9 に一文）、F-07（CM §12 L425 の書換）。

### 6.4 進行判定

**現在の design をほぼ固定したまま Walking Skeleton へ進める。** 7 件の修正はすべて IPC / PR / CM / CCT の局所記述であり、以下を変更しない: 12 subsystem と semantic owner、SO の ownership 表、DR の依存規則、CA・SS の契約、IB の command / outcome / repository trait 群、PR の table 群と durability class（device record の追記を除く）、CCT の SD 順序、CI の identity / revision / generation 体系、CM の crate 分割と依存グラフ、IPC の envelope・pattern・transport 選択。6.1 の三件は Walking Skeleton の presence・round 実装に着手する時点で反映されていればよく、修正と実装準備を並行できる。

## 7. Design Freedom として finding にしなかった事項

以下は未決定だが、上位契約が明示的に Freedom としており、かつ Concrete 層が満たすべき property を記述しているため finding としない。

- pairing / connection auth の暗号方式・鍵形式（IPC §9.3 の property「秘密の非露出・nonce 単発・旧 proof 再利用禁止」が定義済み）。ただし F-02 の通り、Host 側検証材料の**分類**は Freedom ではない。
- 切断検知・heartbeat・timeout 値・reconnect interval、および一時的な到達不能から切断確定への境界（CA-CPT §8.1・§12、CC L273）。
- 費用予約量の算定式・集計期間・推定方式（IB K-G、PR L565）。
- Provider adapter が別 process（`plugins/provider/*`）で動く場合の認証秘密の受渡し。K-C「値を返さない」と DR-05 を満たす限り Freedom だが、実装時に「秘密値を通常 payload に載せない」を plugin IPC frame にも適用することを確認すること。
- `ene-kernel` shim の残置期間（CM §3 は Walking Skeleton 完了時の廃止を明記）。
- Context Assembly の選択 algorithm・prompt template・cache（CA-CTX、CM 原則）。

## 8. Observations（finding ではない参考事項）

- IPC §10.1 の same-machine transport 区分は、F-01 の「Host PC 上の Client」判定と F-03 の「Host-local 管理面」判定の両方に再利用できる。二つの修正を同じ材料で閉じると、presence と管理面で「Host PC 上」の定義が分岐しない。
- PR §9.1 は `client_last_connection` を含む Group G を backup に含めるが、同表は「現在接続として再成立させない」と明記しており、F-02 のような復活経路にはならない。
- Review #2 の F-02（Client 側接続材料）・F-03（restore 時 Credential store）は Concrete 層で解決されているが、その解決は「Client 側 secret を backup に含めない」「Provider / MCP Credential store を E として維持する」に留まり、Host 側 device 検証材料は両者の隙間に落ちている。F-02 はこの隙間の指摘である。

## 9. 修正後の独立再判定（2026-09-08）

本節は第1〜8節の初回レビューと区別した closure review であり、現在の再判定は本節による。新規 Architecture Review の全域検査ではなく、F-01〜F-07・RA-01 の解消と、修正に起因する Critical / Major regression を対象とした。

### 9.1 対象と基準

- 修正対象の Concrete 五書（IPC・PR・CCT・IB・CM）の差分全体と現行の該当規則・DTO・inventory・mapping・walkthrough を照合した。Git diff は修正範囲の特定にだけ使用し、Git 履歴・既存実装から製品要件を補っていない。
- 基準は現行 Requirements「Remote Client」「Backupとrestore」「Reset」「費用と資源」に関する契約、SO 4.15・4.16・§8、DR 3.4・4.1・4.2・5.1、RF-04 / RF-05、および CA-CPT §8 の関連契約。前回レビューの修正案への字面の一致ではなく、責任主体・現在性・失敗時の帰結が閉じるかで判定した。
- 正本の優先順位は requirements/README「要件と設計の境界」と design/README「requirements と design の関係」「precedence / conflict rule」に従う。Design 内の「Owner decision」というラベルだけを要件正本への反映済みの証拠とは扱わない。

### 9.2 Verdict

- **Verdict: CHANGES REQUIRED**
- **未解消件数: Critical 0 / Major 0 / Minor 1**（F-03 の要件正本との接続のみ。従来の Major な remote 単独成立経路は解消）
- **Requirement Ambiguity: 1（RA-01、正本への反映待ち）**。F-03 と同じ残件であり、独立した追加 finding として二重計上しない。
- **修正に起因する新規 Critical / Major: 0。新規 finding は追加しない。**

### 9.3 Finding ごとの解消状況

| ID | 再判定 | 確認した根拠と帰結 |
|---|---|---|
| F-01 | **Closed** | IPC §10.1・§12.2〜12.3・V-5/V-10、PR Group G、CCT §10.2、IB X-A が通常切断と Host restart を分離する。Host 確定の live SameMachine・認証・device 許可・排他性を条件に CAS で fallback し、候補なし・確認不能なら NoActive。再接続だけでは元へ戻さず、Host 専用理由を Client の MoveIntentReason から排除している。Stop 優先・Host Client 非自動起動・旧作用非再実行を維持。 |
| F-02 | **Closed** | PR Group F/G/K・§9.1〜9.3、IPC §9.2〜9.4・V-13、CM §7.2 により、Host device-auth の材料・現在 trust 範囲・失効は非秘密部分も E として backup 除外・Restore 非置換。復元 DB 行では E を再生成できず、認証と機能利用は現在 E との照合を必要とする。機能のみの失効、Full Reset 後の旧 backup、新 Host の材料不在にも復活経路がない。E の保管と Permission の意味 ownership は分離され、部分失敗を完了とせず利用保留にする。 |
| F-03 | **技術的解消、正本反映待ち（Major → Minor）** | IPC §2・§9・§18.1・M-18・V-14、IB §9・§15、CM §11、PR §9 は remote intent と trusted Host-local 最終確認を分離する。same-machine / pairing のみで管理入口資格を与えず、対象・内容・現在前提への確認の結付け、stale / replay 拒否、汎用 kind・Tool 等による迂回防止も一致。残件は下記 RA-01 のみ。 |
| F-04 | **Closed** | IPC §13.1・M-8・§21・§23・V-2、IB X-B が None による正当な初回開始を定義。ene-presentation が現在条件を照合して round を発行し、非 optional の domain RoundId へ解決する。Client / ingress mapping に発行 authority を移さず、同一 command retry の二重発行・旧 round 拒否の None 自動再送を禁止している。 |
| F-05 | **Closed** | PR §6.2、CCT §9.2、IB K-G が当該利用 owner の再評価経路から CommitUsageCommand(actual = 不明) を呼ぶ責任を明記。短 transaction で Reserved を照合し、二重計上・確定済み報告の巻戻しを防ぐ。引当は不明として一度だけ cap に残り、後着報告は元利用への更新。不明と孤立理由の管理面表示まで閉じている。 |
| F-06 | **Closed** | CM §12 の Task walkthrough は ene-task → ene-action とし、Permission live check / Credential 限定利用の caller を Action に訂正。Task の cost 予約は自 crate の gate trait 経由であり、CM §10 の禁止依存と DR の premise 供給原則を崩さない。 |
| F-07 | **Closed** | CM §12 は source owner が自身の範囲限定で RoutingContextOffer を生成し、ene-core は中継すると明記。Task 文脈は既存の個体調整–作業協調経由で、SO 4.16 の提供範囲責任と一致する。composition root への意味選択権移転はない。 |

### 9.4 RA-01 — Design 上の決定は明確、要件正本への反映が未完了

IPC §18.1（現行 L589〜599）は「RA-01 Owner decision」として、Host PC 上の trusted first-party management surface に最終確認を限定している。したがって、**修正後 Design 自体に確認場所の二義性は残っていない**。

一方、行動要件正本 `docs/requirements/requirements.md` L461 は依然として「OwnerがHost側で確認できるdevice pairing」である。L439〜442 の Restore、L454 の Full Reset、L462 の device 失効にも Host PC での確認限定はない。現行 requirements 五書にも、この決定を確認場所・対象操作の製品契約として取り込んだ記述は見つからない。SO 4.15・§8 も「Host側確認」のままである。

確認場所の限定は、Owner が remote だけで device を失効できるか、Restore / Reset を完了できるかを変えるユーザー可視の安全契約であり、OS API や wire format の選択だけではない。requirements/README L5・L12・L20〜25、および design/README L7〜8・L84〜90 の規則上、Concrete の宣言だけで RA-01 を要件として closed にはできない。ただし安全な確認経路と各 artifact の整合は既に成立しているため、従来の F-03 Major を維持する理由もない。

**最小の closure 条件:** 記載済み Owner decision の確認場所・対象操作・remote は要求までという製品契約を要件正本へ反映する。OS peer 認証や入口の保護方式など内部 mechanism は Design に残す。決定内容自体の再設計は今回の確認からは要求しない。本レビューでは要件・Design artifact を変更していない。

### 9.5 修正による regression と Walking Skeleton

修正の交差を、(1) fallback と Stop / 同時呼出し / Host restart、(2) device 失効と Restore / Full Reset / 機能許可の巻戻し / store 部分失敗、(3) 高権限要求と入口偽装 / 確認 replay / 管理面の active 非依存、(4) 初回 round と retry / stale、(5) 孤立予約と重複再評価 / 後着報告、(6) crate caller と routing scope の観点で辿った。新規 Critical / Major 問題は確認できなかった。具体的な OS 入口保護、timeout、費用算定等の既存 Design Freedom を追加 finding にしていない。

**Walking Skeleton の主経路へは進める。** 着手前対象だった F-01・F-04・F-06 は解消し、F-05 も recovery 経路まで閉じた。RA-01 の要件正本反映は並行して行えるが、高権限管理の製品契約を確定済みとして固定する前、および本 closure review を PASS にする前に完了させる必要がある。現在の CHANGES REQUIRED はこの一残件によるものである。

## 10. 最終判定（RA-01 Requirements 反映後）

本節は §9 の CHANGES REQUIRED に対する closure の最終判定であり、§9 までの記録は変更しない。対象は §9 の唯一の残件（RA-01 の要件正本反映）の確認と、未解決 Critical / Major / Minor の有無、新規問題の有無に限定する。Design / Requirements は変更していない。

- **Verdict: PASS**
- **Critical: 0 / Major: 0 / Minor: 0**
- **RA-01: Closed**。行動要件正本 `docs/requirements/requirements.md` の「信頼境界」に、確認場所（Host PC 上の trusted first-party management surface）、remote は要求まで（Remote Client だけでは成立しない）、対象操作（pairing／再 pairing、device trust・許可機能の変更・失効（自身の device を含む）、Credential の登録・更新・差替え・失効、Restore の実行確認と復元後の一括有効化、全データ Reset の強い確認、同種の高権限操作）、および代理入力の否認が製品契約として記載され、「Remote Client」節の pairing 行から同節へ参照している。旧来の「Host側で確認できる」という確認場所の二義性は pairing 行に残っていない。OS peer 認証・transport 区分・DTO・outcome 等の内部 mechanism は Design 側に残されており、要件と設計の境界を越えていない。
- **F-01〜F-07: すべて Closed**。§9 の判定に対する Requirements 側の変更は確認場所の製品契約の追加に留まり、presence fallback、device-auth、round、孤立予約、crate caller、routing scope の各 closure を崩さない。F-03 の残件だった正本反映が解消されたため、Minor 1 は残らない。
- **Design との整合:** 上記要件は IPC §18.1 の確認場所・対象・remote 要求扱い・迂回防止と一致し、IB §9・§15、CM §11、PR §9 の同境界とも矛盾しない。SO の「Host側確認」は抽象表現として残るが、IPC §18.1 がその具体化であることを明示しており、上位書換えを要する drift ではない。
- **新規問題: なし**。今回の Requirements 追加による Critical / Major / Minor の新規 finding はない。
- **Walking Skeleton へ進める。** §9 の進行条件は満たされた。
