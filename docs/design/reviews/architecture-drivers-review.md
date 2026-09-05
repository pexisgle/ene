# Architecture Drivers Review

レビュー対象: [`docs/design/architecture-drivers.md`](../architecture-drivers.md)（分析対象Baseline 2026-09-05）
要件正本: [`docs/requirements/`](../../requirements/README.md)（再構成済みBaseline、最終確認 2026-09-05）

レビュー方法: 要件文書（製品定義、要件、受け入れ条件、参考資料）を先に独立して読み、後続設計へ引き継ぐべき設計圧力を自分で導出したうえで、各Driverの「Driver」「Architectural significance」「Design freedom」と Section 3 の Requirement Issues を要件本文と照合した。照合の観点は次の四つである。

- 要件が固定していない事項を固定事項として書いていないか（過剰拘束）
- 要件が固定している事項をDesign freedomに置いていないか（過少拘束）
- Requirements basis に挙げた見出しの内容と Driver の記述が一致しているか（誤読・捏造）
- System Context、Subsystem Decomposition、State Ownership、Dependency Rules の判断を誤らせる抜け・重複・暗黙の矛盾がないか

既存実装、crate構成、過去設計、参考資料のAgent Harness層分け、一般的なAI Agent architectureは判断基準に用いていない。

## 1. Verdict

**PASS WITH CHANGES**

- 15のDriverは、要件本文に対する事実上の誤読や、要件に存在しない制約の追加を含まない。固定事項とDesign freedomの線引きは[要件定義 README](../../requirements/README.md)「要件と設計の境界」と整合し、受け入れ条件のSupport MatrixやGate、参考資料の層分けを恒久制約へ昇格させていない。Requirements basis に挙げた見出しはすべて実在し、記述内容と対応している。
- 一方、Companion削除・停止に対してTask記録・Schedule・相手側Relationshipがどう扱われるかという、State Ownershipへ直接影響する契約がどのDriverにも反映されていない（F-01、Major）。本書のCompanion中心のOverviewと組み合わさると、TaskをCompanion従属の所有物として設計する誤りを誘発しやすい。
- その他の指摘は、既に本文にある制約の配置（F-02）、要件が例示した拡張点の脱落（F-03）、Section 3 に記録すべき未確定事項（F-04、F-05）であり、Driverの小規模な修正と留保の記録で対応できる。

## 2. Findings

### F-01 — Companion削除・停止に対するTask／Schedule／Relationshipのlifecycle境界

**Severity:** `Major`

**Target**

AD-04、AD-08（関連: AD-02、AD-09、Overview）

**Finding**

要件はCompanion削除時の挙動として、(a) 実行中Taskはbest-effortでCancelするが、残るTask記録と共同Taskは管理面から確認でき、Ownerが別Companionへ引継ぎを依頼できる、(b) 担当Scheduleは削除し、別Companionへ自動で引き継がない、(c) そのCompanionを「主体または相手とする」Relationshipを削除する、と定めている。

このことから、TaskはCompanionの所有物ではなく「担当」を付け替えられる独立した記録であり、Scheduleは担当Companionと共に消え、Relationshipは相手側個体の状態にも削除が及ぶ、という三つの異なるlifecycle関係が読み取れる。

AD-04 は Architectural significance で「削除後もGlobal Learning・グループ発言・共同Taskが残る」ことに触れるが、単独Task記録が残り引継ぎ可能であること、ScheduleとTaskで扱いが異なること、Relationshipの削除が相手側個体にも及ぶことは、いずれのDriverにも書かれていない。AD-08 はTaskとWorkspace・外部file・Clientの関係を扱うが、TaskとCompanionのlifecycle関係を扱っていない。AD-04 の「Relationshipは主体Companionごとの相手への認識」という説明だけでは、相手側削除による自身の状態変化を想定しない設計になり得る。

**Requirements evidence**

- [要件](../../requirements/requirements.md)「CompanionとCharacter／停止と削除」: 「担当Scheduleは削除し、別Companionへ自動で引き継がない」「そのCompanionを主体または相手とするRelationshipも削除する」「残るTask記録と共同Taskは管理面から確認でき、Ownerは必要に応じて別Companionへ引継ぎを依頼できる」
- [要件](../../requirements/requirements.md)「Task、Workspace、成果物／Task」「Schedule」: Task・Scheduleと個体の関係を一貫して「担当Companion」と表現している
- [製品定義](../../requirements/product.md)「主要概念／Task」: TaskはOwnerからCompanionへ依頼された作業単位であり、Companionに所有される単位とは定義されていない

**Why it matters**

Overviewと AD-03 は「その個体から実作業も利用できる」「Companionを窓口とする」とCompanion中心に構成されている。この状態で State Ownership へ進むと、TaskをCompanion配下の集約として所有させ、削除をcascadeさせる設計が自然な既定になるが、これは「残るTask記録…引継ぎ」に反する。逆に、Task記録と同様にScheduleを残す設計も要件に反する。Subsystem Decomposition では、Task管理をCompanion単位のagent内部へ閉じるか、Companionを「担当」として参照する独立した責務にするかの判断を左右する。Dependency Rules では、Companion削除操作がTask記録へ直接到達してよい範囲を決める。

**Recommended disposition**

Driverを修正する。AD-08（またはAD-04）の固定事項に、Task記録がCompanion削除を越えて残り担当を付け替えられること、Scheduleは担当Companion削除で削除され自動引継ぎされないこと、RelationshipはSubject・Targetいずれの削除でもlifecycleが終わることを追加する。Task記録の所有主体をどこに置くかという具体的な決定は Step 2 へ残す。A-01（Companion scope Skill）は Skill に限定された Issue として現状のまま残す。

### F-02 — 「待機のためのLLM問い合わせ禁止」がObservation文脈にだけ置かれている

**Severity:** `Minor`

**Target**

AD-12（関連: AD-01、AD-09）

**Finding**

要件は「所有と実行」で、進行中の作業や外部eventを待つためだけにLLMへ反復問い合わせを行わないことをHost全体の実行契約として定め、「Schedule」でも同じ禁止を繰り返している。AD-12 はこれを「待機だけのLLM pollingを避ける必要があり」と自発性の抑制文脈で一度言及するだけで、Task・Task Agentの待機やScheduleの到来待ちを扱う AD-09、実行の継続を扱う AD-01 には現れない。AD-12 の Requirements basis に「所有と実行」「Schedule」が含まれていることから、意図的にここへ集約されたと読めるが、Driver本文の位置からは Observation 固有の制約に見える。

**Requirements evidence**

- [要件](../../requirements/requirements.md)「所有と実行」: 「Hostは、進行中の作業や外部eventを待つためだけにLLMへ反復問い合わせを行わない」
- [要件](../../requirements/requirements.md)「Schedule」: 「Schedule待機のためにLLMへpollingしない」

**Why it matters**

この制約は、Core loopと実行基盤の駆動方式（eventや時刻を契機にLLMを起動し、LLM側の反復判断で待機しない）を固定する。Task Agentがsubprocessや外部応答を待つ間にLLMを回し続ける設計や、Schedule管理をLLMの反復判断で実現する設計は、要件を直接読めば排除できるが、Driverだけを入力にすると AD-09 側で見落としやすい。費用capや資源上限（AD-06・AD-13）との整合にも影響する。

**Recommended disposition**

Driverを修正する。Host全体の実行契約として AD-01 または AD-09 の固定事項へ移し、AD-12 からは参照にとどめる。駆動方式そのものの選択は Design freedom のまま残す。

### F-03 — 要件が例示するPlugin拡張点の種類がAD-11から脱落している

**Severity:** `Minor`

**Target**

AD-11

**Finding**

要件は、ene固有Pluginを「未対応Provider protocol、Observation adapter、Body renderer等、明確な型と境界を持つ拡張点」に限定する。AD-11 は「明確な型と境界を持つ拡張点に限定し」と抽象化し、例示を落としている。例示は「等」付きで列挙が閉じていないが、どの境界が拡張可能でなければならないかを示す、要件中の唯一の具体的な信号である。

**Requirements evidence**

- [要件](../../requirements/requirements.md)「拡張」: 「ene固有Pluginは、未対応Provider protocol、Observation adapter、Body renderer等、明確な型と境界を持つ拡張点に限定する」
- [要件](../../requirements/requirements.md)「BodyとVoice／Desktop Body」: 「Body表示に失敗しても、テキスト会話、Task管理、設定、復旧操作を利用できる」
- [受け入れ条件](../../requirements/acceptance.md)「障害と安全境界」: 独立して発生させる条件に「Body rendererの停止」が含まれる

**Why it matters**

Body rendererが拡張点であるという事実は、AD-13 の「Body障害時にText・Task管理・復旧を維持する」と結び付いて、Body描画を差替え可能な型境界の外側に置く圧力になる。Observation adapterも同様に、AD-12 の候補検知とメインLLM判断の境界、および外部送信前のCapability割当と関係する。抽象化されたままだと、Subsystem Decomposition で拡張点の候補を設計者が自由に置き直し、要件が想定する三種の境界と食い違う可能性がある。

**Recommended disposition**

Driverを修正する。AD-11 の固定事項に、要件が例示する拡張点の種類をそのまま引き継ぎ、列挙が閉じていないことも併記する。Plugin APIや境界の型の具体化は Step 2 以降に残す。

### F-04 — Observation有効化・自発性上限の適用単位が未確定であることが Requirement Issue に記録されていない

**Severity:** `Minor`

**Target**

AD-12、Section 3

**Finding**

要件は、ObservationのON/OFF、自発会話・通知・内部調査・Companion間交流それぞれの頻度／上限、Quiet hours等をOwnerが設定できると定めるが、その適用単位（Host全体、Client(device)ごと、Companionごと）を定めていない。Provider割当だけは「Hostの既定設定とCompanionごとのoverride」と明示されている。さらに、Remote Clientは「deviceごとに許可された機能」を持ち、候補検知は「同じClientの複数Companionで不必要に重複させない」とされるため、Companion単位の設定とClient単位の共有処理が交差する。

AD-12 はこれらを「明示ON/OFF」「共通上限」と要約し、適用単位が未確定であることに触れていない。Section 3 冒頭の「内部方式を選ぶだけでは決まらず、Ownerから見える挙動…が変わる事項」という本書自身の基準に照らすと、これは Requirement Ambiguity に該当する。

**Requirements evidence**

- [要件](../../requirements/requirements.md)「Observationと自発性／Observation」: 「ObservationはOwnerが明示的にONまたはOFFにでき」「Companionが存在するClientのdesktop全体を観測対象とする」「複数Companionが同じClientにいる場合も、候補検知を不必要に重複させない」「そのCapabilityへOwnerが割り当てるまで送信しない」
- [要件](../../requirements/requirements.md)「Observationと自発性／自発的な発話と行動」: 「それぞれOFFを含む頻度または上限を設定できる」「Quiet hours、Mute、…を常に優先する」
- [要件](../../requirements/requirements.md)「Provider、費用、接続障害／割当と同意」: 「Hostの既定Provider設定とCompanionごとのoverrideを持てる」
- [要件](../../requirements/requirements.md)「Remote Client」: 「Ownerはpairing済みdevice、最終接続、許可された機能を確認し、deviceごとに失効できる」

**Why it matters**

設定の適用単位は State Ownership の直接の入力である。Observation設定がCompanion状態なのか、device(Client)状態なのか、Host全体の設定なのかで、Companion削除・device失効・backup範囲での位置が変わる。また、Client単位で共有される候補検知が画面内容を外部へ送る場合、どのCompanionの割当同意に従うのかは、AD-10 と AD-12 を同時に満たす設計で必ず決めなければならない。要件が固定していない以上、設計で仮決めするとOwnerから見える挙動を設計側が定義したことになる。

**Recommended disposition**

Requirement Ambiguity として扱う。Section 3 に新規Issueとして追加し（本レビューの N-02）、AD-12 の固定事項は「ON/OFFと上限が存在し常時確認できること」までに留めて単位を留保する。Step 2 ではこの単位に依存する所有権判断を保留する。

### F-05 — active Client排他性の外側にある会話・操作の位置付けが未確定であることが記録されていない

**Severity:** `Minor`

**Target**

AD-02、Section 3

**Finding**

要件はBody、Realtime会話、Voice、ambient Observation、自発的interactionをactive Clientへ帰属させるが、テキスト会話、Task管理、管理面の操作はこの列挙に含まれない。Clientは「表示と一時的な操作に必要なdata」を受け取ると定めるだけで、非active Clientから同じCompanionへテキスト入力できるのか、できる場合に「一続きのtimeline」と「入出力roundの安全な区切り」をどう成立させるかは書かれていない。

AD-02 はこれを Design freedom の「複数Clientからの表示・管理操作すべてを禁止することは要件ではない」に含めているが、会話入力の可否はOwnerから見える挙動であり、Section 3 の基準では Requirement Issue に当たる。AD-02 が Driver で列挙している排他対象自体は要件と一致しており、誤読ではない。

**Requirements evidence**

- [要件](../../requirements/requirements.md)「Remote Client」: 「一つのCompanionは同時に一つのClientだけをactive Clientとして持ち、Body、Realtime会話、Voice、ambient Observation、自発的interactionはそのactive Clientに属する」「Clientは表示と一時的な操作に必要なdataだけを受け取り」「Client間の移動時は、現在の入力または出力roundを安全に区切り」
- [要件](../../requirements/requirements.md)「会話と情報提示／一続きの会話」: 「Ownerがsession境界を管理しなくてよい一続きのtimeline」「グループ会話とTask管理は、一対一timelineとは別の空間に置く」

**Why it matters**

非active Clientからのテキスト入力を許すなら、同じtimelineへ複数Clientから同時にroundが到来し得るため、roundの直列化と「Companionが存在する場所」の意味が設計上分かれる。許さないなら、Remote Clientの用途はBody・Voice・Observationの移動に強く寄り、Task管理や管理面の位置付けも変わる。System Context（Clientの役割）と State Ownership（会話roundの調停主体）に影響し、どちらの解釈も要件から一意に導けない。

**Recommended disposition**

Requirement Ambiguity として扱う。Section 3 に新規Issueとして追加し（本レビューの N-01）、AD-02 の Design freedom 記述からは会話入力の扱いを切り出して留保へ移す。調停方式の選択は Design freedom のまま残す。

## 3. Requirement Issues Assessment

### 既存Issueの再評価

#### A-01 — Companion削除時のCompanion scope Skillの扱い

**判定: 妥当**

削除対象の列挙（固有の設定、Experience Summary、Memory、Companion State、一対一Conversation History）にSkillはなく、残す対象の列挙（Global scopeのLearning、グループ発言、共同Task記録、外部file）にもCompanion scope Skillはない。「Global scopeのLearning…は削除しない」との対比からCompanion scope Learning（Skillを含む）は削除されると推測できるが明示ではなく、「同梱またはimportされた原本をExperienceによる変更で破壊しない」との関係（原本とCompanion由来revisionを分離して原本だけ残すのか）も要件から確定できない。Ownerから見える残存範囲が変わるため Requirement Ambiguity として妥当である。

Task記録側は「残るTask記録と共同Taskは管理面から確認でき」により要件から解決可能なので、A-01 の範囲をene内部のCompanion scope Skillに限定している判断も正しい（Task記録は F-01 でDriver側の修正として扱う）。

#### A-02 — 自発的な作業と「Ownerから依頼されたTask」の関係

**判定: 妥当**

Taskは製品定義・要件ともに「OwnerからCompanionへ依頼された作業単位」であり、Scheduleの各回だけが明示的に新しいTaskとされる。自発的な内部調査・外部Actionには「依頼されたActionと同じPermission pipeline」が適用されると明記されるが、Taskとして追跡・表示・Cancel・Host再起動後の扱いに入るかは書かれていない。Task管理面に現れるか、CancelできるかがOwnerから見える挙動として変わるため妥当である。「共通Permission・費用・loop上限が適用されること自体は明確」という本書の限定も正しい。

#### G-01 — Remote利用時にComputer Useが操作するdevice／desktopの範囲

**判定: 妥当（確認すべき契約に一点追加を推奨）**

「Taskとして明示されたcomputer useはambient Observationと区別し、TaskのPermissionと記録を適用する」、Capability境界の「screen、device」、Remote Clientでの「Task実行はHost、Body・Observation等はactive Client」という帰属だけでは、Computer Useの操作対象desktopは確定しない。Gapとして妥当である。

追加で、「Remote接続が切れてもHost上のTaskとScheduleは定義された条件で継続し」の「定義された条件」は要件内で定義されておらず、Client側のscreen・deviceに依存するTaskが切断・失効・Companion移動時に継続できるかは G-01 と同じ問いに帰着する。G-01 の「確認すべき契約」にこの継続条件を明示的に含めることを推奨する。

### 新規Issue

#### N-01 — 非active Clientからの会話・Task管理・管理操作（Requirement Ambiguity）

F-05 を参照。active Clientへ帰属する対象の列挙にテキスト会話・Task管理・管理面が含まれないため、非active Clientからの同一Companionへの入力可否と、その場合のround調停の意味が要件から確定しない。

#### N-02 — Observation有効化・自発性上限・Quiet hoursの適用単位（Requirement Ambiguity）

F-04 を参照。Provider割当以外の設定について、Host全体／Client(device)／Companionのどの単位に属するかが要件から確定しない。deviceごとの「許可された機能」、Client単位で共有される候補検知、Companionごとの送信先割当同意の交差も未整理である。

### 検討したがIssueにしなかった事項

- **Companion削除時の単独Task記録**: 「残るTask記録と共同Task…引継ぎを依頼できる」により残ることは解決可能。Driver側の反映不足として F-01 で扱う。
- **同一機上のClientにpairingが必要か**: pairing要件は「Remote Client」節に属する。Host上のClientを暗黙にpairing済みとしても要件に反しないため Design freedom。
- **高負荷時に下げる「非重要な背景処理」の範囲**: 要件は会話・Owner操作・安全判断の維持だけを固定しており、何を非重要とするかは設計へ委ねられている。ただし許可済みTaskの黙った停止は AD-01・AD-09 に反する（Section 4 参照）。
- **Voice経由の承認可否**: 要件は話者認証を行わないことと明示だけを固定している。承認経路をVoiceから除くかは要件で決まっておらず、追加要件を補って評価しない。
- **Companionが依頼を断れること**: Task状態集合は「等」で開いており、受諾前の応答を会話として扱っても要件に反しない。Design freedom。

## 4. Coverage Assessment

### 全体

要件から独立に導出した設計圧力（Host正本とClient一時性、active Client排他、Companion個体性とCharacter分離とscope、知識状態の役割分離とrevision・時間性、LLM判断と機械的境界、保持と消去の目的別契約、Task・Workspace・成果物の所有、停止・再開・Schedule、Provider・同意・費用・cache、外部標準と拡張隔離、Observationと自発性、Body・Voice・縮退・資源上限、Credential・data最小化・監査、backup・restore・reset）は、15のDriverでいずれも扱われている。F-01 を除き、後続設計で重大な判断を誤らせる抜けは確認できなかった。

固定事項とDesign freedomの線引きは正確である。特に次の点は要件の意図を正しく保っている。

- 「共通pipeline」を製品上同じ制約が適用されることの要求として読み、単一moduleや呼出し列を指定しない（AD-06）
- 候補検知とメインLLM判断の役割区別を、特定のprocess分割へ置き換えない（AD-12）
- 受け入れ条件の数値Gateを実測条件付きの検証制約として扱い、恒久値にしない（AD-13）
- 参考資料のEntry／Core Loop／Tool／Execution／Persistenceの層分けをDriverの根拠にしない（根拠の扱い）
- exactly-once、確実なCancel、全外部作用のrollback、全構成でのOffline推論、model間の完全一致を「要求されていない」と明示する（AD-09・AD-10）

### 重複と粒度の不均衡

内容の誤りではないためFindingにしないが、後続でDriverをtrace単位として使う場合に影響する。

- **AD-05 と AD-13 の粒度**: AD-05 はExperience Summary／Memory／Raw History／Relationship／Companion Stateの役割分離、revisionと時間的有効性、一時状態と持続傾向、一般editor不提供、Skill原本保護と復帰を一つに束ねる。AD-13 はBody・Voice・段階的縮退・keyboard経路・locale一致・資源上限・性能Gateを束ねる。本書自身の「独立して破ることができ、異なる設計判断へ影響する契約をDriverの単位とした」という基準に対して粗い。
- **資源上限の分割**: 要件では「信頼境界」節の一契約だが、AD-06（Promptだけに依存させない）と AD-13（上限適用と到達時の停止・判断要求）に分かれている。役割分担は読み取れるが、資源上限を扱う際は両方の参照が必要になる。
- **Skill契約の分散**: scope（AD-04）、原本とrevision（AD-05）、Workspace内Skill（AD-08）、Agent Skills形式（AD-11）、削除（A-01）に分散している。Character Packageの推奨Skill → import原本 → Companion由来revision → Global昇格 → Workspace内Skill という所属の連鎖を一箇所で確認できないため、State Ownership でSkillを扱う際は横断参照が必要である。
- **軽微な重複**: Rule非triggerが AD-06 と AD-12 に、Remote Client契約が AD-01 と AD-02 に現れる。役割分担（正本・継続 vs 排他・移動）は明確で問題にならない。

### 暗黙の緊張関係

矛盾ではないが、Driver間関係表に加える価値がある。

- **AD-13 の段階的縮退と AD-01・AD-09 のTask継続**: 「非重要な背景処理を段階的に下げる」で何が非重要かは要件が固定していない。許可済みTaskの減速は継続と矛盾しないが、縮退policyが許可済みTaskを黙って停止・破棄する設計は AD-01・AD-09 に反する。
- **AD-13 の「Voice入力に本人認証済みという前提は置けない」**: 要件は話者認証を行わないことと明示だけを固定している。この文を「Voiceでは承認できない」と読み替えて要件を追加しないよう、後続設計で注意が必要である。

### 十分に扱われていると確認した事項

- Capability粒度（Read／Create／Edit／Delete／Execute の分離、識別可能なCapability一覧）は AD-06 が「Permissionと安全境界」全節を根拠に「操作riskの区別」を固定事項としており、参照から到達できる。
- Setupの最小化と追加Capabilityの段階的同意は AD-03・AD-10・AD-12 に分散しているが、いずれも固定事項として保たれている。
- 障害時に誤って成功表示しない契約は、AD-07（削除完了表示）、AD-13（部分障害時の操作可能性）、AD-14（説明と監査）で扱われている。
- Experience SummaryがCompanion間で共有され得ることと、共有根拠の部分削除は AD-04・AD-07 で扱われている。
- targeted deletionが接続中Clientの一時dataへ到達することは AD-07 に含まれている。

## 5. Final Recommendation

**Findingsを修正してから進むべき。**

- Step 2 へ進む前に対応が必須: **F-01**
- 同時に対応することを推奨（Driver本文と Section 3 への小規模な追記で済む）: **F-02、F-03、F-04、F-05**
- 要件定義へ戻って確認すべき事項: **N-01、N-02**、および G-01 への継続条件の追記。ただし A-01・A-02・G-01 と同様に Section 3 の留保として記録すれば Step 2 の開始を妨げない。Step 2 では、これらに依存するClientの役割定義と設定・Observation状態の所有権判断を保留し、要件側の確定後に反映する。
