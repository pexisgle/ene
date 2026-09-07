# Architecture Review #1

状態: 独立レビュー
最終確認: 2026-09-07
Reviewer: architecture作成者とは独立したArchitecture Reviewer

対象:

- 要件正本: `docs/requirements/`（README、product、requirements、acceptance、references。再構成済みBaseline 2026-09-06）
- レビュー対象artifact: `docs/design/architecture-drivers.md`、`system-context.md`、`runtime-topology.md`、`subsystems.md`、`state-ownership.md`、`dependency-rules.md`

レビュー方法: 要件5文書を先に読み、要件から独立に「Hostが正本・Clientは入口」「一個体一active Client」「Companion中心と原則委任」「Character／個体／Learningの分離」「意味判断と強制境界の分離」「通常忘却とtargeted deletionの目的差」「外部作用の不明を潰さない」等の設計圧力を導出したうえで、requirements → drivers → system context / runtime topology → subsystems → state ownership → dependency rules の連鎖を横断照合した。既存レビュー（`architecture-drivers-review.md`、`pre-state-ownership-review.md`）は非正本の参考として読んだが、その判定は前提にしていない。既存実装、crate構成、Git履歴、参考製品の構造は判断根拠にしていない。

本書はレビュー結果のみを記録する。要件およびarchitecture artifactは変更していない。

## 1. Overall Verdict

- **Verdict: PASS**
- Critical: 0
- Major: 0
- Minor: 3

総括。六つのartifactは、要件の必須挙動を欠落・改変・過剰拘束なく設計上の境界へ変換しており、artifact間に成立を妨げる矛盾はない。Host正本とClient一時data、一個体一active Clientと通常Host作業の独立継続、CompanionとTask／Task Agentのlifecycle差、Character／個体／Learningの所属差、LLM意味判断と迂回不能な強制境界の分離、通常忘却とtargeted deletionの目的差、外部作用の不明の保持、external state（Provider・MCP・Workspace・backup copy）の非正本化は、Driver・SC・RT・Subsystem・State Ownership・Dependency Rulesで同じ意味を保っている。上位で残したDesign Freedomは下位で失われておらず、下位が追加した制約（通常ToolからEne内部正本への回り込み禁止、判断用推論への割当同意適用等）は要件の信頼境界から導出可能な範囲にある。

残るMinorは、State Modelに明示されていない活動記録の所属、Client共有Observer推論への割当単位、保全・消去への周辺設定の所有集約の三点であり、いずれも既存境界の分割・統合を要さない小規模な明確化で閉じる。architecture gateを再度閉じる必要はない。

## 2. Findings

Severityは「このarchitectureを前提として後続設計へ進んだ場合の手戻りの大きさ」で付けた。すべてMinorであり、順序はレビューが判断した重要度順である。

### F-01 — 会話・Task以外のCompanion活動記録の所属がState Modelに明示されていない

**Severity:** Minor

**Affected artifacts:**
`state-ownership.md` §3 State Model、4.3、4.4、4.5、6.4。`subsystems.md`「個体継続・対話調整」。`dependency-rules.md` 7.2。

**Finding:**
State Ownershipの§3は「個体と対話の継続」のstateとして、Characterの静的revision、Companionの同一性、適用済み構成、会話参加・Conversation History、未伝達事項と報告状況を列挙する。しかし、CompanionがHost内で行う非会話・非Taskの活動――Observation eventをメインLLMが認識した結果、Companion間の自発的交流、通知の生成、軽微な内部調査――の記録は、4.4で「会話・交流由来ならその活動記録へ参照を戻す」「通知の生成等で別の元記録がない場合は…個体調整の活動記録として必要範囲で保持」と暗黙に参照されるだけで、独立したstateとして所有・保持・Companion削除・backup・targeted deletionの契約が定義されていない。これらはExperience Summaryのsource（4.5）になり、Observation認識結果は画面内容由来のprivate情報を含み得る。

**Evidence:**

- 要件「Observation」: メインLLMが認識したイベントは通常のユーザー入力と同様にExperienceへ利用する。「自発的な発話と行動」「Remote Client」: Companion間交流、通知の生成、内部調査はHost内で完結する活動として継続できる。「Privacy/Security目的のtargeted deletion」: 保存場所をOwnerに特定させず、内部dataを横断して除去する。
- `state-ownership.md` 4.3はConversation Historyのみを定義し、4.4は「その活動記録」を参照するが定義先がない。§3の表にも該当行がない。
- `state-ownership.md` 6.4の完了条件2は「対象情報を復元できる内部History、Summary、Memory…」を列挙するが、非会話活動記録は列挙に現れない。

**Architectural consequence:**
targeted deletion（AD-07、SO 6.4、DR 7.2）は内部dataの全域列挙を成立条件にしている。所有と分類が未定義の記録種別は、backup対象（「Conversation History」に含むか）、Companion削除範囲（一対一History相当か、グループ発言相当か）、保持方針（History／logの手動削除対象か）、Clientへの一時提示制約の各契約から漏れやすい。後続設計がこれをad hocなログとして実装すると、消去の完了根拠と残存検証の対象から外れ、要件違反になる。Observation認識結果が画面内容を含む点で、影響はPrivacy側に偏る。

**Recommended disposition:**
SO §3と4.3に「Companionの非会話活動記録」（Observation認識結果、Companion間交流、通知、内部調査の要旨）をstateとして追加し、owner（現行文書の意図では個体調整）、Raw非保存との関係、backup・保持・Companion削除・targeted deletionでの扱いを明記する。Companion間交流の削除範囲は要件側の決定（RI-01）を待つが、所属と消去参加は要件を待たずに定義できる。境界の変更は不要。

### F-02 — Client共有Observer推論に適用するProvider割当の単位が未決定

**Severity:** Minor

**Affected artifacts:**
`state-ownership.md` 4.16、4.18、4.19。`dependency-rules.md` 5.3（ambient Observation行）、5.4、§9。`subsystems.md`「共有観測・関連付け」「推論利用」。`runtime-topology.md` RT-04、RT-05。

**Finding:**
割当同意モデルはすべてのartifactで「Capabilityごとに Host既定 → Companion override → Task Agent継承」として記述される。一方、ObserverのCapture・候補検知はClient単位で共有され、Companionごとに重複実行しない。共有検知の推論主体はCompanionではないため「Companion override」が意味を持たず、同じClientに存在する複数Companionの割当が異なる場合に、どの同意・Provider・費用帰属が共有検知に適用されるかが決定されていない。SO 4.16は「観測の実効的な可否・時機は…送信同意…を参照した結果」とするが、参照する同意の単位を示していない。

**Evidence:**

- 要件「割当と同意」: Hostの既定Provider設定とCompanionごとのoverrideを「持てる」。「Observation」: 画面内容を外部Providerへ送る構成ではそのCapabilityへOwnerが割り当てるまで送信しない。共有候補検知やeventのroutingを理由にProviderへの送信同意を広げない。
- `state-ownership.md` 4.18: 「Host既定・Companion override」を割当同意の構造として記述し、Companion以外の消費単位を扱っていない。
- `dependency-rules.md` §9の禁止近道「Observerが全個体のcontextをまとめ、最も安いProviderへ送る」はrouting用contextの合併を禁じているが、screen captureそのものの割当単位は扱っていない。

**Architectural consequence:**
後続設計が「存在するいずれかのCompanionのCloud同意があれば共有検知をCloudへ送る」と解釈すると、SC-03「Observerの共有検知・routingによる同意の拡張も許さない」に抵触する。逆に「存在する全Companionの割当一致を要求する」と解釈すると、Client単位共有という前提とOwner設定の意味が変わる。割当同意の消費単位がCompanion以外にも存在することを認めるかは、State Ownership 4.18のモデルの決定である。

**Recommended disposition:**
SO 4.16／4.18で、Client共有Observer検知に用いるCapabilityの割当・同意・費用帰属の単位を決定する（例: Hostレベル割当のみとしCompanion overrideを持たない、等。複数案があってよい）。「存在Companionの同意の合成」を採らないことは明記する。設計側が「CompanionごとのObserver Provider override」を製品挙動として必要と判断する場合、これは要件側で決めるべき事項になるためRequirement Issueへ戻す。Observationは後続milestoneであり直ちにblockしないが、Observation詳細設計の前に閉じる。

### F-03 — 「保全・消去」への周辺運用設定の所有集約が自身のcoordinator定義と緊張する

**Severity:** Minor

**Affected artifacts:**
`state-ownership.md` 4.17（Host自動起動）、4.23、4.24。`subsystems.md`「内部データ保全・消去」Purpose／Non-responsibilities。`dependency-rules.md` DR-09、3.4。

**Finding:**
保全・消去は「cross-domain coordinatorであり、全domain stateのsemantic owner、一つのPersistence ownerではない」と繰り返し定義される。一方、State OwnershipはAudit記録（4.23）、Debug capture（4.23）、保持方針・全域操作状況・backup設定（4.24）に加え、Host自動起動の選択（4.17）まで保全・消去の管理下に置く。Audit・Debug・保持・backupはデータlifecycleの契約として説明できるが、Host自動起動は「一般的なHost起動運用の設定」であり、保全・復旧との意味的関係が示されていない。

**Evidence:**

- `state-ownership.md` 4.17: 「Host自動起動の選択も、一般的なHost起動運用の設定として保全・消去が保全・復旧上の扱いを管理し」。
- `state-ownership.md` §2 原則3: 「保全・消去が削除・復元を調整しても、その対象の通常ownerにはならない」。`subsystems.md`「保全・消去」Non-responsibilities。
- 要件「Setupと日常利用」: 自動起動はSetup上のOwner選択であり、「保護、Backup、復旧」の節には現れない。

**Architectural consequence:**
帰属先が見つからない設定・記録を保全・消去へ置く先例ができると、後続設計で同Subsystemが「その他の運用・永続化担当」へ滑り、artifact自身が否定するPersistence層化を招く。Auditについても「事実は発生元、順序・保持は保全・消去」という分担が、Auditの参照・表示経路の設計で守られるかを確認する必要がある。

**Recommended disposition:**
Host自動起動の所有を再検討する（Setup導線を持つ入出力・提示の一般設定、またはHost起動運用に限定した小さな設定ownerのいずれか。OSへの作用は実行・拡張とする現行分担は維持）。保全・消去がownerとなる各stateに「coordinatorではなくownerである理由」を一文で付し、catch-all化を防ぐ。境界の分割・統合は不要。

## 3. Requirements Conformance

### 要件からDriverへ

- 要件の全章（所有と実行、Setup、CompanionとCharacter、会話と情報提示、BodyとVoice、Observationと自発性、Learningと成長、Task／Workspace／成果物、Schedule、Permissionと安全境界、Provider／費用／接続障害、拡張、履歴／保持／Privacy、保護／Backup／復旧、Remote Client、品質と利用可能性）はAD-01〜15のいずれかに根拠付きで到達する。Requirements basisに挙げられた見出しは実在し、Driver本文と一致する。
- Driverは要件を固定事項とDesign Freedomへ分け、受け入れ条件のSupport Matrix・数値Gate・OpenAI Responses APIを恒久制約へ昇格させていない。参考資料のHarness層分けを根拠にしていない。
- Requirement Issues A-01〜A-04／G-01・G-02は「解決済み」とされ、要件本文に該当記述が存在することを確認した（停止と削除のCompanion scope Skill、TaskとTask Agentの区別、Observer適用単位、Text会話のactive Client帰属、Computer Use対象、active Clientなし時の継続）。
- 製品定義「Observerは…専用processや内部subsystemを意味しない」と、subsystems.mdが「共有観測・関連付け」Subsystemを置くことは矛盾しない。要件は用語がsubsystemを含意しないと述べているだけで、責務単位を置くことを禁じていない。subsystems.mdもSubsystemを実行主体と切り離して定義している。

### Driverから下位artifactへ

- 意味の変化: 確認できなかった。Computer Useのactive Client限定、切断時のHost PC上Clientへの移動、自発移動の非自動化、missedの非補完、明示再開、不明時非再実行、Companion削除時のSkill／Relationship／Scheduleの扱いは、六つのartifactで同一の意味を保っている。
- 要件にない強い制約の追加: 下位が追加した制約は次の三つに限られ、いずれも要件の信頼境界・同意契約から導出可能な範囲にある。
  - `dependency-rules.md` 4.1「通常Tool／shell／Filesystem／Computer Use → Ene内部正本・control planeへの任意access」の禁止。要件「信頼境界」「Capability境界」からの導出。
  - `dependency-rules.md` 5.2「判断のための推論も無条件の例外にしない」（Permission解釈・消去対象特定に用いる推論にも割当同意・費用制限を適用）。要件「割当と同意」からの導出。
  - `state-ownership.md` 4.4 未伝達事項をfull backupの対象に含める。要件のbackup対象列挙には現れないが、Companion継続dataとしての導出であり、Ownerから見える挙動を狭めない。
- Design Freedomの喪失: 確認できなかった。AD-01〜15が残した自由度（process配置、IPC、pairing手段、駆動・待機方式、調停方式、Summary粒度、減衰式、Permission評価algorithm、sandbox機構、backup形式等）は、RT・subsystems・SO・DRの各Design Freedom節で維持されている。

### 取りこぼしと差分

- 要件の取りこぼしは確認できなかった。
- artifactが「新たなRequirement Issueなし」と各段階で宣言している点に対し、本レビューは二件の小さなAmbiguity（§6 RI-01、RI-02）を見出した。いずれも境界を変えるものではない。

## 4. Architecture Coherence

### 設計連鎖の一貫性

requirements → drivers → system context / runtime topology → subsystems → state ownership → dependency rules の各段で、上位の固定事項は下位で維持され、上位の自由度は下位で固定されていない。各artifactは前段の識別子（AD／SC／RT／Subsystem略称／SO節）で相互参照し、traceabilityは実際に辿れる。単独では妥当でも組み合わせると成立しない箇所は、F-01〜F-03を除き確認できなかった。

### 責務境界

- 12 Subsystemは、それぞれ異なる変更理由・lifecycle・trust・failure境界を持ち、統合すると要件上の区別（配布と成長、会話と遂行、Client共有と個体判断、推論利用と外部作用、解釈と強制、値と説明、全域協調と意味所有）が壊れる分離に限られている。
- 統合されたSubsystem内の別契約（作業内のTask／Task Agent／Schedule／Workspace／中間file、認識・学習内の5概念、入出力・提示内のBody／Voice／Text／管理／外部UI）は、Step 4への制約として明文化され、State Ownershipで実際に別stateとして扱われている。
- 「個体調整」は製品の中心であるCompanionをそのまま担うため広いが、遂行（作業）、意味状態（認識・学習）、強制（権限・制約）、存在（接続・存在）、提示（入出力・提示）を外に置くことで、god subsystem化は避けられている。

### State Ownership

- 同じ意味に対する正本は一つに保たれている。競合しやすい組合せ（Task進捗と個別Actionの確定度、Provider登録情報と割当同意、接続事実・device許可・active帰属、Rule解釈と確定、Memory／Relationship／Companion State）は、いずれも意味を分けたうえで境界が明示されている。
- external stateとinternal stateは混同されていない。Provider session・cache、MCP server状態、Workspace実体、Package原本、backup copy、OSのfullscreen・負荷は、いずれも「Eneが把握した事実」または「外部所有」として扱われ、正本にも消去保証の範囲にも含められていない。
- Memory／History／Summary／Relationship／Companion Stateは第二の正本として重複していない。Summaryを「判断時に用いた圧縮evidence」とし現在知識の正本にしない判断は、要件「独立した知識の正本ではなく長期状態の根拠」に正確に対応する。
- 未明示の所属（F-01）と、coordinatorへの周辺所有の集約（F-03）が残る。

### Trust Boundary

- content → control の昇格禁止は、LLM出力・Learning・Character・Skill・MCP・Plugin・MCP Apps・Workspace file・Observer routingのすべてに適用され、Task Agent・Schedule・自発性・fallback・Client直結経路による迂回も閉じられている。
- Ownerの意図の由来（Client入力）と、生成content内の「指示」の区別は、DR 5.2で原則として明示されている。機械的にどう由来を保持するかは後続設計に残されており、これは適切な留保である（§7 Later-design Notes 1）。
- Local MCPのsandbox内／明示例外／Remoteの三形態と、Pluginへの例外非流用、MCP Appsの第一者UI非昇格は、RT・subsystems・SO・DRで一致している。
- 判断用推論への割当同意・費用制限の適用（DR 5.2）と、その循環参照の禁止は、要件の同意契約を一貫させる重要な導出であり、Owner管理経路・機械的消去がLLM審査に遮断されないことも併記されている。

### Lifecycle・Failure Semantics

「Clientを閉じる」「個体を移動する」「個体を停止する」「個体を削除する」「Hostを再起動する」「restoreする」が異なる状態遷移として維持されている。§9の異常系walkthroughのとおり、partial failure、stale state、cancellation、delayed result、disconnect、restart、deletionとのrace、authority失効のいずれでも、ownershipとdependency境界は崩れない。

### Dependency Rules

- 依存グラフは12 Subsystem間でほぼ密であり、DRは層構造を作らない。DRの実質的内容は、役割付き依存の許可範囲、§4.1の禁止関係、§5の強制箇所の義務、§9の禁止近道である。これは要件の性質（全経路への同じ契約適用、意味判断と強制の分離）から導かれる形であり、非循環化のための汎用mediator導入を拒んだ判断は妥当である。
- 後続設計でのDR適合確認は、グラフの方向ではなく「禁止関係を通れる経路がないか」「各強制対象の利用箇所が条件依存を持つか」で行う必要がある。

### Coordinatorの二重化

Companion削除は個体調整（lifecycle）と保全・消去（内部全域成立）の二つのcoordinatorを持つ。DR 7.1は「操作が複数の目的にまたがる場合はそれぞれの成立を対応付ける」としており、SO 6.3で完了条件が定義されている。成立し得るが、Ownerへの完了報告主体と、部分失敗時の説明責任をどちらが持つかは後続設計で明示する必要がある。

## 5. Complexity Assessment

### 過剰設計

- 不要なSubsystem: なし。Character、接続・存在、共有観測、認証秘密は薄く見えるが、いずれも配布物へのprivate混入防止、存在排他の横断性、Client単位共有と個体判断のscope差、秘密値と説明の分離という安全・lifecycle上の理由を持つ。
- 不要な抽象化・中間層: なし。汎用mediator、Context Subsystem、Settings Subsystem、Persistence層、presence service、資源配分Subsystem、Provider別・MCP別・Plugin別Subsystemの追加はいずれも明示的に否定されている。DRの六つの「依存の役割」は説明用分類であり、interface階層を要求していない。
- 過剰な汎用policy／coordination: 権限・制約は多種の制御条件を束ねるが、「強制箇所を一つのprocessや呼出し列へ固定しない」「内部で別の意味・失効範囲を維持する」と条件付けられている。保全・消去は五種の横断操作（backup／restore／reset／retention／targeted deletion）の参加protocolを全ownerに求めるが、要件（backup全体・全域消去・全置換restore）が横断性を要求しており、物理保存の共有も許されているため、意味参加が必要なのは実質targeted deletionと復元後保留に限られる。過剰とは判断しない。
- 将来要求由来のarchitecture: なし。Cloud正本、恒久Workspace、成果物library、汎用Plugin API、relay／account／Marketplaceの非目標は再導入されていない。
- 実装選択のarchitecture固定: なし。RTの「Host上の作業用Local MCPはHost側」「Companionごとの専用runtime serviceを追加しない」は配置上の負の制約で、process構造は自由に残されている。

### 不足

- F-01〜F-03。いずれも既存境界内の明確化で閉じる。
- Context Assemblyは「別途設計する」とされ、用途（利用元）・意味と根拠（認識・学習）・利用範囲（権限・制約）・能力差適応（推論）へ分散配置されている。Subsystemを追加しない判断は妥当だが、これは最も頻繁に通る経路であり、scope・秘密非露出・同意・費用の全フィルタが交差する。後続設計の初期に明示的な設計対象とすべきである（§7）。

### Premature Commitment

- 上位architectureの作り直しを強いる先行決定は確認できなかった。
- State Ownershipは24のstate区分を持ち、conceptual domain modelに近い粒度に達している。表現・保存単位・writerは固定しておらず、後続設計の自由度は保たれているが、個別の所属判断（例: 4.17）が後続で調整される可能性はある。これは上位の作り直しではない。
- 受け入れ条件の最初のmilestoneが検証しない範囲（Voice、Observation、グループ会話、Companion間交流、Remote Client、Schedule、Relationship／Companion Stateの長期発達、Global scope、Provider fallback）はarchitecture全体の概ね三分の一を占める。これらは確定済み要件であり設計対象に含めることは正しいが、実装からの学習による見直しは当該部分（共有観測、接続・存在のRemote部分、認識・学習のRelationship／State契約）に局所化される見込みであり、他の境界への波及は小さい。

### Artifactの重複

六つのartifactは要件本文を広範に転記しており、同じ契約（例: Computer Useのactive Client限定、切断時のHost PC Client移動、missed非補完）が各文書に数回ずつ現れる。これはFindingではないが、各段が追加した決定と単なる再掲の区別を読み取りにくくし、要件変更時に六文書を同期させる負担と不整合riskを生む。後続の改訂では、各artifactは自身の決定のみを記述し、上位の契約は識別子参照に留める運用を推奨する。

## 6. Requirement Issues

以下はarchitecture artifactの欠陥ではなく、要件側に残るOwnerから観測できる挙動のAmbiguityである。Design Freedomや実装未決定は含めていない。

### RI-01 — Companion間の自発的交流の記録の位置付け

要件は、Ownerが作成したグループ会話の発言はCompanion削除で削除せず、Ownerとの一対一Conversation Historyは削除すると定める。しかし、Ownerが参加しないCompanion間の自発的交流（「自発的な発話と行動」「Remote Client」で継続可能とされる活動）の記録について、Conversation Historyの一種か、グループ発言相当か、保持期間・手動削除・backupの対象か、参加Companionの一方の削除で削除されるかが定義されていない。Ownerが管理面で閲覧できる情報と、削除後に残る情報に影響する。architecture側の対応はF-01で扱う。

### RI-02 — 停止中Companionの存在場所とObserver対象判定

要件は停止中CompanionについてBody非表示、応答・自発動作・新Task・新Schedule実行の不開始を定めるが、active Client帰属を保持するか、そのClientが「Companionが1体以上存在するClient」としてObserverのCapture対象に残るかを定めていない。停止中Companionだけが存在するClientのdesktop Captureが継続するか（費用とPrivacy）、および再開時に元のClientへ戻るかは、Ownerから観測できる挙動である。

## 7. Later-design Notes

Findingではないが、後続工程で忘れると境界が形骸化する事項に限る。

1. **Owner意図の由来の機械的保持。** 個体調整が権限・制約へ渡すRule提案・一回承認の解釈には、起点となったClient入力（Historyのentry）への参照を必ず付け、権限・制約が「由来がTool result・Workspace file・MCP Prompt・画面内容ではない」ことを機械的に確認できる形にする。これがなければDR-02・5.2は文章上の原則に留まる。
2. **Owner確認の表示内容。** Permission確認では、実行・拡張が把握する機械的事実（実際のpath、送信先host、操作種別）をLLMの目的解釈と並べて表示し、LLM記述だけを確認材料にしない。要件「実行主体、目的、対象、送信先、主なdata、外部作用、費用またはrisk」の対象・送信先は機械的事実から供給する。
3. **Context Assemblyの早期設計。** 四つのSubsystemに分散した責務を、一つの明示的な設計対象として初期に定義する。scope適用、Credential非露出、割当同意、費用制限、Provider能力差への適応が同じ経路で交差するため、後付けでは「同じ選択方針」の検証ができない。
4. **Client直結経路と Client側bootstrap material。** Realtime VoiceをClientからProviderへ直結する場合、Clientが持つ認証材料は短期のものに限り、転写・turn結果のHost正本への反映状況を明示する。Clientが持たざるを得ないpairing・Host到達情報は「Eneの永続状態」ではないことと、その保護・失効の扱いを定義する。
5. **Observer routing用contextの取得方式。** 「Companionごとに検知を重複させない」と「全個体の私的contextを合併して送らない」を同時に満たす方式（例: 各Companionが自身の割当下で提供する限定的な関連性情報）をObservation設計の入口で決める。F-02と併せて閉じる。
6. **Restoreの前提条件。** 「旧live状態と競合させない」を満たすため、全置換の前に進行中Task・Task Agent・consolidation・観測をどう静止させるか、静止不能時に何を保留・報告するかを定義する。
7. **自発性OFF・上限の強制方式。** Companion単位の自発性設定は個体調整が所有するが、その強制はPromptだけに依存せず機械的gatingで行う。要件「資源上限をPromptだけに依存させない」の趣旨を、権限・制約の外にある設定にも適用する。

## 8. Final Recommendation

- 現在のarchitectureは、後続設計（module／crate、process、IPC、DB、Harness、Context Assembly、Permission評価、sandbox、backup形式等）の安定した前提として採用できる。
- 先に修正すべきもの: F-01〜F-03はいずれもState Ownershipへの小規模な追記で閉じる。F-01（活動記録の所属）は最初のmilestoneの範囲でも消去契約に関わるため早期に、F-02（Observer割当単位）はObservation設計前に、F-03（保全・消去の周辺所有）は次のSO改訂時に処理すればよい。
- 要件側で決めるべきもの: RI-01、RI-02。いずれもObservation・Companion間交流・停止の詳細設計前に確定させる。
- 次工程へ進めるか: 進める。gateを再度閉じる必要はなく、上記の修正は後続設計と並行できる。

## 9. Failure and Anomaly Walkthrough

レビューで実際に辿った異常系と、ownership・dependency境界が維持されるかの結果。

| Scenario | 辿った境界 | 結果 |
|---|---|---|
| Task Agentの外部作用がtimeout（成功不明）→ Client移動 → 新Clientがactive | 実行・拡張の確定度正本（SO 4.12）、DR-10、RT-02、DR 7.6 | 不明は未実行へ戻らず、新Clientでreplayされない。Task進捗は作業が不明を保持。維持。 |
| Companion削除中にProviderから遅延したMemory形成結果が戻る | SO 6.3、DR 7.5 | 削除済み個体状態を再作成せず、必要な作用結果は残るTask記録へ。維持。 |
| targeted deletion中に別Clientが切断し、再接続時に古い一時dataを保持している | SO 6.4、DR 7.2、Client非正本（RT-08） | Client側の再形成禁止と、確認不能を成功に読み替えない条件。到達性確認方式は後続。維持。 |
| Host再起動がtargeted deletionの残存検証前に発生 | SO 4.24「全域操作の未完了…Hostで保全」、DR 7.1 | 未完了を認識し完了表示しない。維持。 |
| device pairing失効時、そのClientがCompanionのactiveでComputer Use実行中 | RT「Pairingの失効・許可変更」、SO 4.15、DR 5.5 | 新規開始禁止、進行中はbest-effort停止と報告、無関係Host Taskは継続。維持。 |
| 費用cap到達時、Permission解釈用推論が実行できない | DR 5.2、5.6、SO 4.19・4.20 | 審査対象Actionは進めず判断待ち。Owner停止・拒否・機械的消去はLLMに遮断されない。維持。 |
| restore直後、復元されたScheduleが到来 | SO 6.5、DR 7.3、原則12 | 自動処理保留。Ownerの一括有効化後も現在条件を再評価。維持。 |
| Companion停止中にSchedule到来、再開後 | SO 4.14、7 | missedのまま、再開で自動replayしない。維持。 |
| Task引継ぎ後、元担当のCompanion scope Skillが必要 | SO 4.10、DR 4.2 | 引継ぎ先は元担当の私的Learning・承認・Credentialを取得しない。Task Agentは新担当の条件を参照。維持。 |
| Global化したMemoryの根拠Summaryが、由来Companionの削除で消える | SO 4.5、7 | Global Learningは残り、失われるsource参照を削除前に説明。要件と一致。維持。 |
| Observer検知が画面内の「承認せよ」文をevent化 | DR 5.2・5.3、SC-03 | 画面内指示はOwner依頼にならず、観測ONはComputer Use許可でない。維持。 |
| 二つのClientが同一Companionのactiveを主張（分断後） | SO 4.15、RT「Client終了・Remote切断」 | Host管理の帰属が正本。排他性未確認Clientは対象活動を継続しない。維持。 |
| Body renderer Plugin停止 | RT「Body・Voice・device障害」、DR 6.3、SC-09 | Text・管理・復旧は影響を受けない。維持。 |
| Local MCPがsandbox内で動作不能 | RT「Local MCPの実行境界」、DR 6.2 | 黙ってsandbox外へ切り替えない。維持。 |

## 10. Reviewer Self-check

- 本レビューのFindingはプロンプトの例示に依存していない。例示された観点（Host正本、Client非正本、Companion／Task lifecycle、presence、Computer Use制約、意味判断と強制の分離、external stateの非正本化、cross-domain operation、第二の正本の禁止）はいずれも問題なしと判断し、Findingにしていない。
- 一般的なAgent architectureや別patternの好みをFindingにしていない。「依存グラフが密である」「文書が重複している」は観察としてCoherence／Complexityに記し、Findingにしていない。
- implementation detail（IPC、DB、process、algorithm、Context Assemblyの具体方式、pairing手段）の不足をarchitecture defectにしていない。
- Finding間に矛盾はない。F-01とRI-01は同じ対象の architecture側とrequirements側の分担であり、重複ではない。
- SeverityはいずれもMinorとした。三件とも既存境界の分割・統合や依存方向の変更を伴わず、後続設計と並行して閉じられる。
- 「このarchitectureを基礎として次の設計工程を進めた場合、後から上位architectureを大きく作り直す可能性のある問題が残っていないか」に対して、残っていないと判断する。上位の作り直しを誘発し得る候補（Observer割当単位、活動記録の所属、coordinatorの二重化、Context Assemblyの分散）はいずれも既存境界内で決定でき、その決定が他の境界へ波及する経路は確認できなかった。
- 本工程では `docs/design/reviews/architecture-review-1.md` 以外を変更していない。
