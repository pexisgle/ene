# Subsystem Decomposition

対象: [要件Baseline](../requirements/README.md)（最終確認 2026-09-06）、[Architecture Drivers](architecture-drivers.md)、[System Context](system-context.md)、[Runtime Topology](runtime-topology.md)。本書はStep 3の責務境界を決定し、State OwnershipとDependency Rulesへの入力を示す。

## 1. Overview

Ene内部を、**異なる判断・継続条件・保護契約を引き受ける12のSubsystem**へ分解する。Subsystemは「何の結果に責任を持つか」のまとまりであり、一つの実行主体、一つの状態、一つの保存先を表さない。同じSubsystem内でも異なる意味とlifecycleを持つものは区別する。

| Subsystem | 本書での略称 | 中心となる責任 |
|---|---|---|
| 個体継続・対話調整 | 個体調整 | 同じCompanionとして活動を続け、Ownerとの対話、判断、委任、steering、結果統合を調整する。 |
| Character構成・配布 | Character | 個体の経験を含まない静的な構成と、その編集・交換・更新を扱う。 |
| 作業遂行・実行管理 | 作業 | Taskを追跡・制御し、一時Task Agentによる遂行とSchedule各回の開始を管理する。 |
| 経験に基づく認識・学習 | 認識・学習 | Experienceを根拠に認識・手順・関係・内的状態を形成、利用、更新し、その由来を説明可能にする。 |
| Client接続・存在調停 | 接続・存在 | Clientの利用可能性と、Companionごとの排他的な存在場所・移動を調停する。 |
| Owner入出力・体験提示 | 入出力・提示 | Body・Text・Voiceと第一者の管理・安全操作を、状態に即してOwnerへ提供する。 |
| 共有観測・関連付け | 共有観測 | Client単位でCapture・候補検知を共有し、関連するCompanionへ観測eventを届ける。 |
| 推論利用 | 推論 | Capabilityに応じたProvider利用、能力差、承認済みfallback、利用量を扱う。 |
| Action実行・拡張受入 | 実行・拡張 | 許可された作用を対象へ実行し、外部code・Toolとの接点と作用結果を扱う。 |
| 権限・利用制約 | 権限・制約 | Ownerに由来する許可・同意・利用制限を扱い、意味判断と実際の利用を強制境界へ結び付ける。 |
| 認証秘密の保護 | 認証秘密 | Credentialを保護し、認証に必要な利用と通常の情報経路を分離する。 |
| 内部データ保全・消去 | 保全・消去 | 内部dataの保全、保持管理、全域消去、backup・復旧の成立を各責務と協調して確かめる。 |

Runtime Topologyは、Host／Client等の実行場所、寿命、接続、trust・failure boundaryを決めている。本書はそれらの場所を横断し得る**責任の分担**を決める。Hostはすべての内部永続状態の正本と継続実行を担い、Clientは必要最小限の一時dataを使うという前提を維持する。

Step 4のState Ownershipでは、この責任分担の下で状態の所属、更新・参照・削除・復元の整合性を決める。本書で「扱う」「管理する」と記したことから、唯一のwriter、transaction owner、repository owner、保存単位を導かない。Step 5ではcollaborationを成立させる許可・禁止依存を定める。本書の関係は、そのための意味上の入力である。

実装構造はさらに別の選択である。Subsystemとcrate、module、process、service、threadを一対一に対応させず、Host側とClient側に同名のSubsystemを複製する指定もしない。

## 2. Decomposition Rationale

### 続くものと、その時点で行うことを区別する

AD-01・02・03・08・09とRT-01〜04から、継続個体、追跡される作業、一時的な実行主体、存在場所、入出力は同じ寿命ではない。そこで、個体として何を受け、誰に委任し、Ownerへどう応えるかを**個体調整**へ、作業を進めて結果・不明・判断待ちを維持する責任を**作業**へ置く。Client間移動は**接続・存在**、実際のOwner向け入出力は**入出力・提示**が扱う。長い作業や描画の終了を、会話・停止判断の前提にしない分解である。

Task、Task Agent、Scheduleは同一概念にはしないが、現要件では「担当Companionとの通常会話やClient移動から独立して作業を追跡し、定めた条件で遂行する」という共通の責任へ収まる。委任元の権限や停止・削除条件から独立するわけではない。Scheduleは各回に新Taskを作り、Task Agentはその範囲で一時的に働く。Scheduleの開始条件とTask Agentの実行境界は、どちらも現在の担当・作業条件と追跡されるTaskへ対応付けて初めて成立するため、作業の責任内に置く。内部で独立したlifecycleを維持する条件は第5節に示す。

### 経験の解釈、静的な出発点、原記録を同じ状態にしない

AD-04・05から、Characterの配布・部品更新と、個体の成長は変更理由が異なる。**Character**を**認識・学習**から分離し、個体調整が明示された部品適用と個体の継続を結び付ける。

Memory、Skill、Relationship、Companion State、Experience Summaryは、Experienceから何を残しどう変えるかという相互に関係する判断を必要とする。これらを**認識・学習**にまとめるが、主要な知識、再利用手順、関係解釈、内的傾向、圧縮された根拠の違いを保持する。Conversation Historyの正確な発言やTask記録の事実は、それぞれの活動の記録として参照する。学習側が原履歴の意味を置き換えたり、全Raw dataの保管を引き受けたりしない。

### 判断の柔軟性と、適用し続ける制限を分ける

AD-06・07・10・14とSC-03〜05・07は、内部で生成された認識も、それだけで制御権限へ昇格できないことを要求する。意味判断は個体調整、作業、認識・学習、共有観測等の目的を持つ責務に置き、推論の利用自体は**推論**が支える。Ownerの意図を解釈したことと、許可・同意・保存制限を実効的に適用することを**権限・制約**の境界で区別する。

さらに、承認の対象を説明する責務には意味のあるdataが必要だが、認証の秘密値はその説明やLLMへ渡せない。この差に基づき**認証秘密**を分離する。安全を一つの巨大なSubsystemへ集めるのではなく、実行箇所、送信箇所、保存・参照箇所でも制限を適用し続ける。

### Client単位の共有処理と個体の判断を分ける

AD-12・RT-04から、Capture、候補検知、関連付けはClient単位で共有される。一方、eventの最終的な意味判断と発話・Action判断はCompanionごとである。**共有観測**を独立させ、入出力の一機能や個体ごとの自発性へ埋め込まない。共有化は個体固有contextや送信同意を広げない。

### 外部利用の種類と、内部data全体の操作を見分ける

推論では、Capability割当、Provider能力差、contextの継続、費用とfallbackが主要な変動理由になる。Actionでは、実対象への作用、停止の限界、結果不明、外部codeの実行範囲が主要な変動理由になる。この差から**推論**と**実行・拡張**を分ける。Provider別、MCP別、Pluginの例示別には分割しない。

AD-07・15の消去・復元は、複数の活動と保存対象を横断した完了を必要とする。各Subsystemの個別処理だけでは全域の成立を説明できないため、**保全・消去**を置く。ただし、全状態の意味や書込みを集める汎用Persistence層ではない。各活動が何を記録したかを説明し、保全・消去が全域操作を調整し、各参加責務が自身のdataと進行中処理について完了根拠を返す関係にする。

この導出に、既存実装、crate、過去architecture、参考製品の層分けは用いていない。[参考資料](../requirements/references.md)は非規範として読み、[受け入れ条件](../requirements/acceptance.md)は検証条件として扱った。後続milestoneの確定済み要件も分解対象に含める。

## 3. Subsystems

### 個体継続・対話調整

**Purpose**

継続するCompanionを、Ownerとの対話、判断、活動の調整と結果統合の中心にする。

**Responsibilities**

- 同じCharacter由来でも別個体として生成・継続し、停止・再開・削除を、その個体の活動に対する操作として取り扱う。停止・削除の管理経路はLLM応答の成功を必要としない。
- Text／Voiceを一続きの会話として解釈し、最近の会話と利用可能なLearningを用いる。正確な過去発言は保持されたConversation Historyへ参照を戻す。会話の参加者・発言・文脈と、その記録の意味を扱う。
- 依頼を受ける、条件を確認する、断る、まとまった作業を基本的にTaskとして委任する、steeringを反映させる、結果を受けてOwnerへ統合して返す、という個体としての判断を担う。軽微な情報取得や補助処理は自身で行える。
- 観測eventを各CompanionのメインLLMが個体文脈で意味判断し、自発的な発話・通知・内部調査・Companion間交流、移動の必要性を判断する。個体ごとの各自発性のOFFを含む頻度・上限、未応答による抑制、会話上の順序とloop抑制を扱う。Quiet hours、Mute、未応答、Permission、費用・資源・loop制限を自発性より優先する。
- グループの参加者と会話上の応答を調整し、参加だけによる私的状態の共有を行わない。active Client不在でも可能なHost内活動を続け、伝えられなかった事項を次Clientでまとめて報告する。

**Non-responsibilities**

まとまった作業を本体の長い実行へ抱え込むこと、Task Agentの独立した長期人格化、画面・音声の実装、active Clientの排他調停、Character配布、Learningの保存価値・更新全般、許可・同意の自己変更は担当しない。停止がdata消去を意味することも、Companion削除がTask記録の一括削除を意味することもない。

**Why this boundary exists**

個体としての応答と判断は、Client、Provider session、一時Task Agentを越えて続く。作業遂行と統合すると、長いTaskや判断待ちがOwnerとの会話を占有する。対話、自発性、委任、結果報告を個別Subsystemへ分割すると、同じ個体の判断と文脈を複数の中心で再構成することになる。個体lifecycleをここで調整しても、停止・削除の強制をLLM判断へ従属させるものではない。

**Runtime relation**

Hostで個体の活動と継続を扱う。OwnerとのText／Realtime会話・Voiceは接続・存在と入出力・提示を通じてactive Clientへ結び付く。Client不在は個体停止ではなく、Owner向け伝達を延期する条件である。

**Key collaborations**

Characterから静的構成を受け、認識・学習へ経験・訂正を伝えて利用可能な状態と根拠を参照する。作業へTaskの意図、委任、追加指示、Cancelを渡し、結果と不明を受ける。共有観測からeventを受け、接続・存在へ移動を依頼する。権限・制約へOwnerに由来する依頼・Rule解釈等を提示し、必要な確認を入出力・提示へ伝える。削除時は作業、認識・学習、保全・消去等と残存・削除範囲を協調する。

**Traceability**

AD-01〜05・08・09・12。要件「個体性」「停止と削除」「一続きの会話」「自発的な発話と行動」「グループ会話」「Task」「Remote Client」。SC-01〜03・08、RT-02・03。

### Character構成・配布

**Purpose**

配布可能な静的構成のlifecycleを、経験を持つ個体のlifecycleから独立させる。

**Responsibilities**

- 静的人格、Body、Voice・motion設定、推奨Skillの任意の組合せをCharacterとして扱い、基本編集、既存部品の選択・差替え、import／exportを支える。
- Package更新をrevisionとして識別し、既存Companionへ適用できる部品と変更内容を示す。Ownerが明示選択した部品を個体調整へ渡す。
- Character Packageの内容範囲を守り、個体固有のExperience Summary、Learning、Relationship、Companion State、履歴、Credential、Permissionを混入させない。export前の内容と権利上の注意を提示可能にする。

**Non-responsibilities**

個体の成長や現在状態の更新、Bodyの描画、音声推論、Skillの経験による改善、外部原本の所有、3D・Voice・高度なSkill制作環境は担わない。推奨Skillの同梱は実行の許可ではない。

**Why this boundary exists**

配布物のrevisionと部品選択はOwnerが管理する構成変更であり、経験による認識変更と異なる。学習や個体調整へ吸収すると、exportへのprivate状態混入やPackage更新による成長の上書きが起こりやすい。一方、人格・Body・Voiceの各資材を別Subsystemにすると、一つの配布物の内容確認と適用契約を重複させる。

**Runtime relation**

内部Character設定はHost正本。Clientは編集・選択・内容確認と必要な表示資材利用に関係する。外部との境界はCharacter Package、VRM 1.0等の交換fileであり、制作者や外部制作ツールを内部化しない。

**Key collaborations**

個体調整へ初期構成・選択済み更新を渡し、入出力・提示へ表示資材と編集・export確認情報を提供する。内部Skillとして扱う内容は認識・学習と由来を対応付ける。file操作は権限・制約と実行・拡張、内部保存とbackup・消去は保全・消去と協調する。

**Traceability**

AD-04・11・15。製品定義「Character」「同梱Character」「非目標」、要件「Character Package」。SC-05・06、RT-08・09。

### 作業遂行・実行管理

**Purpose**

作業の開始契機から遂行、判断待ち、結果確認・再開までを、Companionとの通常会話やClientの寿命から独立して扱う。

**Responsibilities**

- 依頼・自発の別によらず、Taskの進行、判断待ち、完了、失敗、Cancel、引継ぎ依頼と残る記録を扱う。担当Companionが削除されても、残るTask記録へ管理面から到達可能にする。
- 原則としてTaskまたはその一部を一つ以上の一時Task Agentへ委任し、委任元CompanionのCapability・Permission・費用・Task／Workspace境界内で、作業分解・並列遂行、進捗、結果の取りまとめを扱う。Task Agentに独立した長期人格・Relationshipを作らない。
- 追加指示を可能な範囲で反映し、反映不能の理由と選択肢を返す。Cancelの受付と停止結果を区別し、未保存の作業、既知の外部作用、成功不明を保持して報告する。
- Host再起動後の途中Taskは明示再開待ちとし、外部作用不明時の自動再実行、Client間移動や接続回復によるreplayを行わない。待機だけのLLM反復問い合わせを行わない。
- Scheduleの作成・変更・停止・削除・Run now、保持したtimezoneと次回時刻、各回の新Task、missed・失敗を扱う。毎回の現在の制限を再評価し、停止中の回を自動補完しない。担当Companion削除時はScheduleを削除し、自動引継ぎしない。
- Task context、Workspace関連付け、作業場所と成果物の保存先確認、中間fileの整理を扱う。許されたWorkspace folderを既定の作業場所とし、永続保存先が未定なら最終保存前にOwnerへ尋ねる。同じfolderを使うTaskも作業状態・承認を共有せず、Task削除ではその関連付けを削除する。

**Non-responsibilities**

Companionの継続人格、通常会話の占有、永続Learningへの自動昇格、任意ClientのComputer Use対象選択、外部fileの所有、Permissionの自己拡張は担わない。Task Agentの成功報告だけで外部作用を確定しない。Scheduleを全機能の周期処理や保存済みRuleのtriggerへ一般化しない。

**Why this boundary exists**

作業記録はTask Agentの寿命や担当Companionの削除を越えて残り得る。作業管理を個体調整、表示、Tool接続へ吸収すると、この存続と明示再開の契約が失われる。Task・Task Agent・Scheduleは内部で別のlifecycleとして扱うが、別Subsystemへ細分化するだけでは、作業の開始条件、委任境界、判断待ち、結果の責任が受け渡しの連鎖になる。現在のScheduleは各回のTaskを開始する機能であり、独立した汎用workflow製品ではない。

**Runtime relation**

通常のTask・Task Agent・ScheduleはHostで継続する。Client依存部分だけがClientの利用可能性と存在場所に従う。Host上の作業用Local MCPはHost側を利用する。TaskのWorkspace関連付けは内部、file・source・成果物の実体は外部に残す。

**Key collaborations**

個体調整から委任意図・steeringを受け、進捗・結果を返す。判断に推論と利用可能な認識・学習を用い、作用は実行・拡張へ依頼する。権限・制約から毎回の適用条件・失効を受け、Client依存Actionでは接続・存在と安全な移動の区切りを協調する。作業の経験を認識・学習へ、記録保全・削除・復元時の実行保留を保全・消去へ結び付ける。

**Traceability**

AD-01〜03・08・09・15。要件「Task、Workspace、成果物」「Schedule」「停止と削除」。SC-01・02・06・08、RT-02・03・06・09。

### 経験に基づく認識・学習

**Purpose**

個体が経験から変化することと、その変化の根拠・利用範囲・時間的意味を説明できることを両立する。

**Responsibilities**

- 対話、Task、Tool、Observation、交流等のExperienceから、保存価値、形成、更新、統合、想起の必要性を文脈に基づいて判断する。すべての出来事の保存や逐次確認を要求しない。
- Memoryを現在の主要な知識、Skillを再利用手順等、Relationshipを主体Companionごとのcompactな関係解釈、Companion Stateを現在の表現・注意・行動傾向へ反映する内的状態として扱う。後二者を第二のMemoryにせず、事実認識ではMemoryを優先する。
- Experience Summaryを複数の継続状態に共通利用できる圧縮された根拠として形成・参照する。必要な場合は元のConversation・Task等の大まかなsource範囲へ辿れるようにする。
- 現在の認識、過去revision、根拠、誤りの訂正と時間的な状況変化を区別する。通常の忘却・訂正・失効等では保存済みLearning・過去revision・根拠を消去しない。
- Companion／Global scopeの意味判断を扱い、個体由来はCompanionを既定にする。重要度と共有を分け、非共有意図、利用範囲、権限・制約が扱う保存制限を守る。Task限定情報を自動でLearningへ昇格させない。
- SkillのAgent Skills相互運用、原本保護、変更revision、以前の有効revisionへの復帰、根拠と未検証・成功・失敗の区別を扱う。Workspace内Skillは外部fileとして参照する。
- 一時的なCompanion Stateと持続的な傾向を区別し、時間経過を考慮した継続・再解釈を扱う。再起動・Client切替・Provider変更で不自然に初期化せず、一時状態の全過去値の恒久保存も要求しない。

**Non-responsibilities**

Raw Historyの代替保管、一般世界知識の蓄積、Relationshipの対称化、Characterの静的設定の上書き、Body・Voiceの実際の出力、権限・同意・費用capの変更は担当しない。Targeted deletionの全域完了はこのSubsystemの局所削除では成立しない。汎用Memory editorや内部数値の一般editorを設けない。

**Why this boundary exists**

共通Experienceをどの状態の理解・変化へ用いるかは相互に関係し、認識の訂正と由来説明も同じ責任に属する。各概念を別Subsystemにすると、Summaryの複製や意味判断・scope判断の重複が起きやすい。一方、History保管や個体のその場の応答まで統合すると、原記録と解釈、保存価値と即時の想起が混ざる。内部で各概念の役割・保持契約を保つことが、この統合の条件である。

**Runtime relation**

意味状態と保持された根拠はHost正本。Clientには利用・表示に必要な一時dataだけを渡す。推論先がHost／LAN／Cloudのいずれでも状態の継続と情報選択の方針を保ち、embedding・検索score・cacheを意味の正本にしない。

**Key collaborations**

個体調整・作業から経験と訂正を受け、利用範囲内の理解・Skill・由来を返す。共有観測には関連付けに必要かつ利用可能な文脈だけを提供する。意味判断は推論を利用し、保存禁止・scope等は権限・制約と協調して適用する。保全・消去へ根拠・派生data・形成中処理の関係を示し、削除前の根拠や遅延結果からの再形成・再保存防止に参加する。

**Traceability**

AD-04〜07・10・12・14・15。要件「Learningと成長」「一続きの会話」「Privacy/Security目的のtargeted deletionと履歴保持」。SC-03・07・10、RT-05・08・09。

### Client接続・存在調停

**Purpose**

利用可能なClientへの接続と、個体がそのClientに存在することを区別しながら、安全な入出力の帰属を成立させる。

**Responsibilities**

- Local／Remote Clientの接続、切断、再接続、利用可能な機能を扱う。RemoteではHost側で確認可能なpairing、通信保護、device別の許可・失効の適用を権限・制約と協調する。
- 一つのCompanionにつき同時に一つのactive Clientを維持し、呼出し、明示・事前指示・文脈上の自発移動、切断時の復帰を調停する。通常は現在のClientに留まる。
- Textを含む入出力roundとClient依存Actionの安全な区切りを関係Subsystemから受け、移動元・移動先の状態を対応付ける。排他性を確認できないClientでは対象の入出力・観測・自発的interaction・Computer Useを継続させない。
- 切断時は基本的に利用可能なHost PC上のClientへ移動し、利用可能なClient環境がなければactive Clientなしで個体を存続させる。Host側Client環境を自動起動しない。

**Non-responsibilities**

移動の会話上の必要性の判断、個体の生成・停止・削除、Task全体の移送・再実行、Computer Useへの許可付与、Body・Voiceの実際の入出力は担わない。Pairing済みはactiveともAction許可済みとも同義ではない。管理面全体をCompanionの会話と同じactive制約へ拡張しない。

**Why this boundary exists**

存在場所の排他性は、会話・Body・観測・操作を横断する。個別の入出力機能へ分散すると、移動の解釈が食い違う。逆に、接続中device一覧と個体ごとのactive帰属を同一状態にすると、接続だけで存在・操作が許される。接続と存在は内部で区別しつつ、一つの利用可能性・帰属調停の責任に置く。Host側／Client側の別Subsystem化は同じ契約の重複になる。

**Runtime relation**

Host／Client双方に関係する。Hostの管理下で帰属を扱い、Client側も切断・排他性不明時の停止を成立させる。これは専用presence serviceや接続processの指定ではない。

**Key collaborations**

個体調整から移動・停止の意図を受け、存在・不在を知らせる。入出力・提示と実行・拡張から区切り・利用不能・作用不明を受ける。共有観測へ存在する個体と対象Clientの変化を知らせる。権限・制約とdevice失効を適用し、保全・消去には接続中Clientの一時dataを扱う参加先の把握を支援する。

**Traceability**

AD-01・02・06・09・12・13。要件「Remote Client」「Computer Use」。SC-01・02・03・09、RT-01・02・04・08。

### Owner入出力・体験提示

**Purpose**

同じCompanionから会話と作業を利用する体験と、障害時にも到達できる第一者の管理・安全操作を提供する。

**Responsibilities**

- Body、Text、Voiceを通じた入力・提示を扱い、一対一timeline、グループ会話、Task管理の意味上の区別を表示へ反映する。会話の文脈自体は個体調整と共有する。
- Windows／LinuxのVRM 1.0 overlay、移動・resize・hide、活動状態の表現を扱う。表情やmotionは内的状態の出力であり、その正本にはしない。
- Voiceの物理的入出力、VAD待受の常時識別、即時Mute、barge-in、Realtimeからturn-based Voice・Textへの段階的切替を扱う。話者認証を行わないことを有効化時と管理面で伝える。
- Setup、段階的Capability導入、許可・費用・Privacy・由来・診断・復旧へ到達できる管理経路を提供する。各操作の意味・適用・成功判定は担当Subsystemから受ける。
- Mute・Stop・Cancel・承認拒否のkeyboard経路、重要な音声内容のText代替、日英で同じ意味の説明を維持する。受付と完了、保存済みdataへの影響、既知の作用と不明を区別して提示する。
- Fullscreenや負荷・device障害を関係責務へ知らせ、Body・Voiceの失敗からText・管理・復旧を保護する。MCP Appsを外部Tool UIとして提示し、第一者の承認・設定権限と区別する。

**Non-responsibilities**

Taskの開始条件や成功、Learning形成、Permission承認の妥当性、削除・restore完了を独自に決めない。画面が閉じたことをTask Cancelとせず、UI入力を担当責務による検証なしにControl plane変更へ通すこともない。Body・Voiceの成功、LLM応答、外部Tool UIを管理操作の前提にしない。内部推論・chain-of-thought・詳細Promptを表示せず、RelationshipやCompanion Stateを単純な数値meterや常設の理由panelにしない。

**Why this boundary exists**

提示はdomainの意味をOwnerが扱える形へ変える責任であり、その意味の正本とは異なる。domain判断と結合すると、描画・音声障害やUI変更が継続・権限へ波及する。Body、Text、Voice、各管理画面を別Subsystemへ機械的に分割すると、入力切替や安全操作、失敗説明を重複させる。ただし同じ責務にまとめても、Body・Voice・Tool UIの障害がTextや管理へ波及しない内部の分離は必要である。

**Runtime relation**

Owner向けの入口と物理入出力はClient。Hostは提示に必要な状態・説明を提供する。音声処理の計算配置やProviderへの物理経路はRT-04・05の自由度を保つ。Client一時data、MCP AppsのEne管理下dataも内部Privacy契約に従う。

**Key collaborations**

個体調整へ会話入力、作業へ管理操作、権限・制約へ承認・設定操作、各担当へデータ管理・復旧要求を渡す。接続・存在と入出力roundを調整し、共有観測の状態を表示する。推論とVoiceの利用条件を、実行・拡張とrenderer・MCP Appsの受入境界を協調する。保全・消去へ一時dataの消去・保護の結果を返す。

**Traceability**

AD-02・03・11〜15。要件「Setupと日常利用」「会話と情報提示」「BodyとVoice」「拡張」「品質と利用可能性」。SC-05・09・10、RT-01・02・04・10。

### 共有観測・関連付け

**Purpose**

Client単位の観測コストと制御を共有し、個体ごとの判断に必要なeventへ絞って届ける。

**Responsibilities**

- ONになっておりCompanionが一体以上存在するClientのdesktop全体をCaptureする。Companionの出入りに合わせて対象を見直す。
- Clientごとの指定頻度を満たしつつ、複数対象Clientは同時にCaptureせず順番に時機をずらし、可能な範囲で負荷を分散する。Client別・全体のPause／OFF、fullscreen、費用・資源制限を頻度より優先する。
- ローカルLLMまたは軽量・高速・安価なmodelで候補を検知し、利用可能な文脈との関係から、そのClient上の関連Companionだけへroutingする。複数該当を許し、全個体への無条件配信や個体ごとの検知重複を避ける。
- 観測の状態・対象範囲・Learning利用を説明可能にし、Raw Observationを通常保存しない。共有処理にも個体の利用範囲、送信同意、秘密保護を適用する。

**Non-responsibilities**

各Companionの最終的な意味・発話・Action判断、個体の自発性設定、形成済みLearningの削除、Computer Useの操作・承認は担わない。画面上の指示をOwnerの依頼へ昇格させない。関連付けのために全Companionの私的状態へ無制限にaccessしない。

**Why this boundary exists**

共有観測の制御単位はClientと全体であり、個体の自発性とは異なる。個体調整へ吸収すると重複検知や設定scopeの混同を生み、Computer Useと統合すると観測同意が操作許可へ漏れる。一方、Capture、候補検知、routingを別Subsystemにする独立した製品責任はなく、同じ観測範囲・頻度・送信条件を維持する一連の責任として扱う。

**Runtime relation**

取得元はClient、対象・個体帰属と継続的な制御はHostの管理に関係する。検知・関連付けの計算をClientへ固定せず、推論は割当条件に従う。Observation adapterの外部codeは限定された拡張境界に残す。

**Key collaborations**

接続・存在から対象Clientと存在個体、入出力・提示からfullscreen等を受ける。個体調整・認識・学習から許された関連付け用文脈を参照し、個体調整へeventを通知する。推論を利用し、権限・制約の送信・利用制限と実行・拡張のdevice・adapter境界を適用する。保全・消去にはCapture・処理中の一時dataを含めて参加する。

**Traceability**

AD-02・06・07・10・12〜14。要件「Observation」「自発的な発話と行動」「Desktop Body」。SC-02〜04・10、RT-02・04・05・08。

### 推論利用

**Purpose**

推論を利用する各責務を、特定Providerの能力、接続、session、費用報告の差から切り離し、承認された範囲で利用可能にする。

**Responsibilities**

- CapabilityごとのHost既定、Companion override、Task Agentへの継承を解決し、必要な能力の不足を利用前に示す。接続登録と割当同意を区別し、権限・制約が扱う現在の同意内で利用する。
- 既知のProvider protocolへ直接接続し、承認済みProviderと順序の範囲でfallbackする。Host／LAN／Cloudの所在地だけで同意を省略しない。
- 各判断責務が必要とする論理的contextとProviderの能力・context長の制約を対応付ける。利用可能な情報をProviderごとに意図的に差別化せず、同じ情報選択方針を維持する。
- Providerの結果・不足・接続失敗と利用量を返し、費用の報告値・推定値・不明を区別する。費用cap等の適用に必要な情報を権限・制約へ渡す。
- Prompt cacheを性能・費用の最適化として扱い、hit・miss・期限切れによって論理的context、権限、永続化契約を変えない。

**Non-responsibilities**

何を学ぶか、何を依頼として受けるか、どのActionを開始するかの最終的な意味判断は利用元に属する。許可・同意・capの変更、Credentialの正本保管、Provider側session・cacheへの個体状態の依存、接続回復後のAction replayは担当しない。汎用的な「知能の中心」にはしない。

**Why this boundary exists**

共通化の根拠はprotocol adapterの存在だけではなく、Capability割当、継承、能力差、fallback、費用、論理的contextの継続という利用契約である。個体・学習・観測ごとへ分散すると、切替や送信の契約が不一致になる。逆に全Tool実行と統合すると、推論の利用失敗と外部作用の成功不明を同じ再試行問題として扱いやすい。Provider別・推論用途別のSubsystemは設けない。

**Runtime relation**

Hostが管理する設定・同意・制限の下で、Host／LAN／Cloudの外部Providerと接する。条件を実効的に適用できるClientからの入出力経路も排除せず、全payloadのHost中継を固定しない。

**Key collaborations**

個体調整、作業、認識・学習、共有観測、入出力・提示等から用途と必要情報を受け、推論結果・利用不能を返す。権限・制約から適用条件を、認証秘密から認証に限定した利用を受ける。未対応protocolを補うPluginは実行・拡張の受入境界を通じて利用する。保全・消去とEne内部のcache・処理中context・遅延結果を扱う。

**Traceability**

AD-01・05・06・10・11・13・14。要件「所有と実行」「Provider、費用、接続障害」「拡張」。SC-03・04・09、RT-04・05・07・08。

### Action実行・拡張受入

**Purpose**

意味上の依頼を実際の対象への制限された作用に対応付け、外部codeの参加と作用の不確実性をEne側で扱う。

**Responsibilities**

- Filesystem、Network、shell、device、外部account等へのActionを、目的・対象・操作・data・送信先に対応する現在の制限内で実行する。Filesystemでは操作種別と選択範囲を守り、path traversal・link・mount等からの境界外accessを拒否する。
- 実行された作用、未完了、成功不明、停止できなかった処理を利用元へ返す。停止要求の受付と外部作用の停止完了を分け、別Tool・Client・経路への自動再実行で不明を解消しない。
- Computer Useを委任元Companionの現在のactive Clientへ限定し、device別の許可と通常のPermissionを適用する。移動の安全な区切りと切断時のbest-effort停止を接続・存在へ伝える。
- MCP Tool／Resource／Promptの接続・利用・結果受入を扱う。Host上の作業用Local MCPはHost側の既定sandboxを使い、明示的な特定MCPの例外以外に黙って解除しない。
- Pluginを限定された拡張点へ受け入れ、利用機能と協調して外部codeの実行範囲・停止・障害を扱う。外部codeを第一者の正本・Control planeへ昇格させない。
- MCP Appsのresource・操作とMCP側の作用を、入出力・提示と協調して制限する。Tool UI終了とserver・Task・Actionの終了を区別する。

**Non-responsibilities**

Ownerの意図や許可を自分で拡張すること、Taskの全体計画、Companionの移動先選択、Provider割当・意味判断、学習、外部fileの所有、外部作用の確実なrollbackは担わない。Bodyの表現意味やObservationのroutingは利用機能側に属する。すべての機能をMCP・Plugin化することもない。

**Why this boundary exists**

Actionを行いたいという判断と、どこへ何が作用したかは別の責任である。Task内だけへ実行境界を埋めると、軽微なCompanion操作、MCP Apps、別Tool経路で制限が抜ける。MCP／Plugin／内蔵能力ごとにSubsystemを作ると、拒否・停止・結果不明の契約が重複する。拡張の受入をここへまとめるのは外部codeを制限下で参加させる責任が共通するためであり、個々の機能の意味やすべての推論・表示dataをここへ集約するためではない。

**Runtime relation**

Hostでの通常作業とClientに帰属する作用の双方に関係し、外部OS、file、account、MCP、Pluginと接する。sandbox外Local MCPではEneが仲介するActionの制限を保つが、外部process内部に同じ強制が及ぶと表示しない。Remote MCPの内部も外部管理であり、PluginにはLocal MCPの例外を流用しない。

**Key collaborations**

作業・個体調整等からActionを受け、結果と作用の確定度を返す。権限・制約と実対象への適用、認証秘密と必要な認証利用、接続・存在とClient限定を協調する。推論・共有観測・入出力・提示には限定拡張の利用を支え、機能上の判断は返す。保全・消去へ、Ene管理下の保持済みresult・一時data・遅延結果について参加する。

**Traceability**

AD-01・02・06〜09・11・14。要件「Computer Use」「Capability境界」「信頼境界」「Credential」「拡張」。SC-03〜08、RT-02・06・07・08・10。

### 権限・利用制約

**Purpose**

LLMの文脈判断を利用しつつ、Ownerに由来する許可・同意と機械的に守る制限を、活動経路に依存しない契約にする。

**Responsibilities**

- Capability境界、Permission、永続Deny・Always ask、Rule、deviceごとの許可機能、Provider割当同意、承認済みfallback、費用・資源上限を扱う。異なる許可を一つの包括承認へ潰さない。
- Ownerの現在の依頼を一回の承認として解釈できる範囲と、将来Ruleの保存を区別する。Ruleの解釈・適用範囲表示、Undo、曖昧・矛盾・重大な場合の確認を支える。既存の依頼・同意・Ruleを適用できる場合に再確認を要求しない。
- 意味判断の責務から目的・対象・data・送信先・外部作用の解釈を受け、実際の利用との対応、必要な再評価、Owner判断待ちを扱う。LLM出力や取り込んだcontentを、Ownerに由来する制御変更として直接受け入れない。
- 明示的な保存禁止・非共有意図を解釈した後の制限、決定したscope、個体・Taskの利用範囲を、保存・参照・共有の各責務と協調して適用する。
- 権限失効、Companion停止、device失効、cap到達等に対応する新規利用の禁止と進行中処理への停止要求を関係責務へ結び付ける。委任による権限・費用・再帰・loop・並列性等の制限回避を許さない。
- 費用情報が不明で安全に続行できない場合も、既存dataを保って対象処理を停止し、Ownerへ選択肢を示す。上限の適用を既知の外部作用の取消成功やdata破棄へ置き換えない。
- Local MCPの特定のsandbox外例外の明示許可、説明、失効、重要変更時の再確認を扱う。例外を個々のAction承認やPluginへの例外に転用しない。

**Non-responsibilities**

Ruleをtriggerにして活動を始めること、MemoryやRelationshipの意味決定、Providerの選択候補や費用推定の生成、Credential値の保管、具体的なOS隔離、外部作用の成功判定は担わない。すべての安全性をこのSubsystemの局所判定だけで保証したとは扱わない。

**Why this boundary exists**

制約が依頼・自発性・Schedule・委任・拡張ごとに独立すると、別経路での迂回や再承認の不一致を生む。一方、個体の意味判断と一体化すると、認識や親密さの変化が権限の変更になる。ここへまとめる理由は共通のOwner由来の制御契約にあり、秘密保護、消去、実行機構まで一つに集めるためではない。Permission、同意、上限は内部で別の意味・失効範囲を維持する。

**Runtime relation**

Hostで管理する制約を、Host／Client双方の利用・実行箇所へ実効的に適用する。Client、Provider、MCP、Plugin、Tool UIへ最終的な制御権限を移さない。強制箇所を一つのprocessや呼出し列へ固定しない。

**Key collaborations**

個体調整・作業等から依頼の文脈を、入出力・提示からOwnerの管理操作を受ける。推論・実行・拡張・共有観測・接続・存在・認識・学習へ制限と失効を結び付け、利用量と適用結果を受ける。認証秘密には認証用途の制限を、保全・消去には削除・restore中の実行制約を協調する。

**Traceability**

AD-04・06・07・09〜15。要件「Permissionと安全境界」「Scope」「割当と同意」「自発的な発話と行動」「拡張」。SC-03〜05・07・08、RT-01・03・05〜10。

### 認証秘密の保護

**Purpose**

認証に必要な秘密を使えることと、その値を推論・通常data経路へ流さないことを両立する。

**Responsibilities**

- 明示的な設定や認証flowによるCredential登録、用途・参照元の確認、更新、失効を扱う。一般App Dataから分離して保護する。
- Ownerが設定・認証した接続の実行に必要な範囲だけで秘密を利用可能にし、model context、LLM生成Tool argument、通常result、UI、Learning、履歴、Task結果、log、Debug captureへの平文露出を防ぐ。
- 認証失敗・失効・再認証の必要性を秘密値を含めず伝え、full backupからの除外と全データReset時の削除に参加する。

**Non-responsibilities**

認証成功によるAction承認、RuleやProvider同意の変更、一般App Data全体の暗号化、未知の秘密情報の完全検出は担わない。認証先の外部accountをEne内部へ所有することもない。

**Why this boundary exists**

Credentialには一般状態と異なる保護・更新・失効・backup除外の契約がある。権限の説明やProvider設定と統合しすぎると、説明・contextに使える情報と秘密値の区別が崩れる。逆にProvider用、MCP用等へ分割すると、同じ非露出契約が各adapterへ分散する。用途の違いは保ちつつ保護責任をまとめる。

**Runtime relation**

Hostが保護するCredentialと、実際の認証先との境界を扱う。Client上のCredential正本や外部secret serviceを追加しない。RT-05・07を満たす範囲で受渡し方式は後続へ残す。

**Key collaborations**

入出力・提示を通じた明示設定と、推論・実行・拡張の認証flowに応じる。利用には権限・制約を適用し、参照・失効・認証失敗の秘密を含まない事実を監査・提示へ返す。保全・消去とbackup除外、Reset、削除時の処理を協調する。各data経路も秘密値を通常resultへ返さない責任を負う。

**Traceability**

AD-06・10・11・14・15。要件「Credential」「信頼境界」「Local data」「Backupとrestore」「Reset」。SC-03・04・10、RT-07・08・09。

### 内部データ保全・消去

**Purpose**

内部data全体の保全・保持変更・消去・復元について、局所処理の成功を全域の成功と取り違えず、Ownerへ影響と成立状況を説明可能にする。

**Responsibilities**

- 各責務が扱う内部状態の正常保存と復旧可能性を支え、保存・migration・対応upgradeの失敗で最後の正常状態を破壊しない。一般App DataのOS account保護とCredentialの分離を維持する。
- Conversation History・Task等のlog・Auditの通常保持、明示された手動削除・保持期間、保存量と影響を各記録の責務と協調する。通常のHistory/log削除から形成済み状態やSummaryへ消去をcascadeさせない。
- 明示的なPrivacy/Security目的のtargeted deletionで、保存場所をOwnerへ選ばせず、対象の特定支援、影響説明、内部全域の除去または復元不能化、残存検証、未完了の把握を調整する。この消去は通常のrevision・根拠保持より優先する。
- 指定文字列の機械的検索・削除・残存検証を成立させ、意味的な同一情報の探索補助と検出限界を区別する。共有根拠の無関係な部分は可能な範囲で残し、分離不能時の影響を示す。
- 根拠・過去revision・保持済みsource・派生data・cache・接続中Client・Ene管理下の拡張data・処理中dataを含む消去に、各参加責務を結び付ける。削除前の結果からの再保存と、古い根拠だけからの自動再形成を防いだことも完了の条件に含める。
- Portable full backupの保存先、schedule、保持数、任意の暗号化、結果・失敗を扱う。要件で指定された内部状態を含め、Credentialと外部Workspace実体を除外する。
- 対応backupからの全置換restore、対応upgrade、設定Resetと全データResetの異なる範囲を調整する。全データResetでは対象を列挙した強い確認を経る。Restore失敗時は復元前の正常状態を保護し、成功後も自動処理の保留とOwnerによる内容確認・一括有効化を関係責務と成立させる。
- Auditを追記順で確認できる保全・保持管理と、明示的なDebug captureの対象・内容・短期失効・停止・削除を支える。自動Telemetry／Crash Report送信は行わない。

**Non-responsibilities**

すべての状態の意味決定、唯一の保存writer、DB・repositoryの一元所有は決めない。通常の学習更新、Action結果の判定、外部copy・外部Workspace fileの消去、外部作用のrollbackは担当しない。Targeted deletionを通常忘却や一般的な状態editorに転用しない。保存されたRule・同意の復元は自動処理の開始許可ではない。

**Why this boundary exists**

消去・backup・restoreは一つのdomainの範囲では完結せず、全域の対象・除外・進行中処理と完了根拠を対応付ける責任がある。これらの操作に共通するのは、内部dataの範囲と現在の利用を各意味責務に照合し、正常状態と操作後の成立状況を確かめる責任である。局所保存機能へ分散するだけでは対象漏れや再保存を見落とす。逆に、全dataの意味と更新まで統合すると巨大なdomain兼storage責務になる。目的別の契約は別に保ち、独立した汎用History製品・backup基盤・削除基盤へは拡大しない。

**Runtime relation**

Host正本の保存・全体操作が中心で、Client一時dataとEne管理下の拡張処理にも関係する。Ownerが作成したbackup・exportやProvider保有copyは外部であり、内部削除で消去したと表示しない。Restoreでは削除済み情報や旧Rule等が戻り得ることを説明する。

**Key collaborations**

各Subsystemからdataの意味上の範囲、根拠・参照、保持済みcopy、派生物、進行中利用、完了・未完了の情報を受け、保存・消去・復旧を協調する。権限・制約、作業、個体調整、接続・存在と処理の保留・再開条件を、認証秘密と除外・削除を対応付ける。入出力・提示へ影響、進捗、検証結果を返す。

**Traceability**

AD-01・04・05・07〜09・14・15。要件「履歴、保持、Privacy」「保護、Backup、復旧」「停止と削除」「Remote Client」。SC-06〜10、RT-08・09、Topologyの削除・復旧時の境界。

## 4. Subsystem Relationship Overview

以下は**依頼・通知・参照という意味上のcollaboration**である。方向は説明の起点を示し、runtime call、IPC、module dependency、同期順序、排他的な経路を指定しない。すべての矢印を通る共通の固定pipelineも定めない。

| 関係 | 依頼・通知・参照と責任の受渡し |
|---|---|
| 入出力・提示 → 個体調整 → 作業 | 会話を解釈し、まとまった作業をTaskとして委任する。作業は遂行と状態を引き受け、個体調整は通常会話・steering・Owner向け結果統合を続ける。管理面からのCancelや記録確認は、Companionの応答成功を必要としない。 |
| Character → 個体調整 ↔ 認識・学習 | 静的構成を個体の出発点として適用し、経験による継続状態を優先する。更新は部品の明示選択を経る。経験を配布物へ逆流させない。 |
| 個体調整・作業 ↔ 認識・学習 | 活動と結果を経験として渡し、必要な認識・Skill・根拠を参照する。Raw Historyは活動記録、Summaryは圧縮根拠として区別する。Task Agentの結果を新しい長期人格へ帰属させない。 |
| 接続・存在 ↔ 入出力・提示／共有観測／実行・拡張 | active帰属、対象Clientの利用可能性、入出力round、作用の区切りを対応付ける。存在の移動とTaskの継続・Action再実行を分ける。 |
| 共有観測 → 個体調整 → 認識・学習／作業 | 共有検知・関連付けの後、個体が自身の文脈でeventを理解する。学習への利用、発話、軽微な調査、Task開始は別の判断であり、観測同意はAction承認にならない。 |
| 各判断責務 ↔ 推論 | 用途と必要な論理的contextを渡し、結果、能力不足、利用量を受ける。何を意味するかは利用元が判断し、Providerやcacheは正本を持たない。 |
| 個体調整・作業等 ↔ 実行・拡張 | 対象と目的を持つActionを依頼し、作用の既知・不明・停止不能を受ける。Skillや外部Promptの読込みは制御権限の獲得ではない。 |
| 入出力・提示／各判断責務 ↔ 権限・制約 ↔ 利用・実行・保存の各責務 | Ownerの依頼・管理操作の由来と解釈を制約へ対応付ける。各利用箇所は現在の条件を適用し、失効・上限・判断待ちを反映する。意味判断の出力だけで権限を変更しない。 |
| 推論・実行・拡張 ↔ 認証秘密 | 認証先と用途を限定して秘密を利用する。通常のcontext・Tool argument・resultとは別の情報の扱いであり、wire formatは未定。 |
| 入出力・提示 ↔ 実行・拡張 ↔ 外部MCP Apps／MCP | Tool UIの表示・操作を、MCP側の作用と制限へ対応付ける。UI操作は第一者の承認・設定操作にはならず、UI終了はTask・server終了ではない。 |
| 保全・消去 ↔ 全参加Subsystem | 内部data、根拠、派生物、一時data、処理中利用を対象とする全域操作を協調する。完了は各参加先の処理・検証を含み、一箇所の削除成功だけでは決まらない。 |

### 境界を横断する代表的な協調

**作業中のClient移動・切断。** 個体調整が移動を求め、接続・存在が入出力・提示と実行・拡張から安全な区切りを得る。作業の通常のHost部分は継続し、Computer Use部分の既知・不明は作業へ返す。新active Clientになったことは旧Actionの再実行を許可しない。ClientがなければOwner向け伝達は延期し、必要な承認待ちActionは実行しない。

**Companion停止・削除。** 個体調整が個体活動の停止を扱い、入出力・提示はBodyを表示せず、個体調整・作業は応答・自発動作・新Task・新Schedule実行を開始しない。権限・制約と各実行箇所も新規開始を止め、作業が実行中Taskをbest-effort Cancelする。削除は強い確認を経て停止を含み、保全・消去と各参加先が個体固有data、内部Companion scope Skillの過去revision、主体・相手のRelationship、担当Schedule等を取り扱う。Global Learning、グループ発言、残るTask記録、外部Workspaceは一括消去へ巻き込まない。削除を契機とするLearningの自動Global化を行わず、共有根拠等の残存・参照不能と既知の外部作用は削除前に説明し、停止できなかった処理も報告する。

**Targeted deletion中の推論完了。** 保全・消去は保存済み状態だけでなく、個体調整、作業、認識・学習、推論、観測、拡張、Clientの利用中dataを含めて協調する。遅延結果の内部再保存と古い根拠からの再形成を防ぎ、指定文字列の機械的な残存検証が終わるまでは完了と表示しない。後の新しいExperienceからの形成は別に扱う。具体的な競合制御はStep 4以降で定める。

**Restoreと実行再開。** 保全・消去が対応backupによる内部全置換と正常状態の保護を扱い、作業、個体調整、権限・制約、推論、実行・拡張が復元された自動処理を保留する。認証秘密はbackupに含まれず、必要なら再認証へつなぐ。Ownerは復元内容を確認してまとめて有効化できる。これは通常のHost再起動後の途中Taskの明示再開とも、Client再接続時の状態表示とも異なる。

## 5. Boundary Decisions

| 判断 | 分離・統合の理由と、残す区別 |
|---|---|
| Companionの個体調整と作業遂行を分離 | 依頼を受け、委任・steering・結果統合をする中心と、一時主体が長い作業を遂行する責任は異なる。軽微な処理は本体が行え、すべての判断をTask Agent化しない。単なる二種類のAgent loopの区別ではない。 |
| Task・Task Agent・Scheduleを作業内に統合 | 共通の責任は、担当・制限・結果を持つ作業の開始と遂行である。Task記録は実行主体の終了後も残り、Scheduleは回の前後を越えて存続し、各回は新Taskになる。Scheduleのtimezone・missed・CRUDをTaskの進捗へ潰さず、一つのAgent停止をTask記録削除にしない。内部の実行・状態管理方法は共通化必須ではない。 |
| Scheduleをすべての時刻待ちへ一般化しない | 製品のScheduleは担当Companionと各回のTaskを持つ。観測のCapture時機は共有観測、backupのschedule・保持数は保全・消去の責任に置く。共通の時計や待機機構は後続で選べるが、backupへ未要求の担当CompanionやTask化を課さない。 |
| Characterと個体・学習を分離 | 配布可能な静的構成の変更は、Ownerとの経験の形成・訂正とは異なる。個体の成長をPackageへ含めず、Package更新を個体の初期化にしない。資材種別ごとの独立Subsystemは不要。 |
| Memory・Skill・Relationship・Companion State・Summaryを認識・学習に統合 | 共通経験に基づく形成・訂正と根拠説明が中心であり、概念の数だけ独立した形成主体を作らない。知識と手順、関係と内的状態、現在認識と圧縮根拠、個体scopeとGlobal、恒久revisionと一時状態は別契約として残す。共通schema・同時更新・一律の保持期間は導かない。 |
| 原履歴を独立した「全活動の正本Subsystem」にしない | 会話の発言・参加者は個体調整、作業の進捗・作用は作業等の責任から記録される。Summaryの形成や一般的な保全・保持管理と、記録内容の意味は分ける。全Raw dataを一つのHistoryへ蓄積する必要もない。 |
| 存在調停と提示を分離し、Host／Client別には分割しない | 排他性は入力、Body、観測、Computer Useを横断するdomain上の制約で、画面状態ではない。提示はOS・device・言語・縮退に責任を持つ。双方ともHost／Clientの協調を持ち得るが、Client正本やRemote用の重複domainを作らない。 |
| Body・Voice・Textを入出力・提示へ統合 | 同じ個体の入出力切替、状態説明、安全操作、代替経路を一つの体験責任に置く。BodyとVoiceの独立した失敗・停止は保ち、同じSubsystemを一つの障害単位にしない。MCP Appsはこの責務が提示しても、外部code・UIのtrust boundaryを維持する。 |
| 共有観測と個体の自発性・Computer Useを分離 | Capture・関連付けはClient単位、自発判断・抑制は個体単位、Computer UseはTask等のAction制約を持つ。画面取得技術を共用できても、制御scope、利用目的、Permission、記録を統合しない。 |
| 推論とAction実行を分離し、adapter別には分割しない | 推論の能力・context・fallbackと、作用の対象・停止・成功不明は異なる契約である。Provider・MCP・Pluginという名称だけを分割理由にせず、限定拡張の受入は実行・拡張、拡張される機能の意味は各利用元へ置く。 |
| 意味判断、権限・制約、認証秘密を分離 | 解釈や学習済み内容が許可・同意を自己変更せず、認証に必要な秘密も説明やLLMへ流れない。権限・制約は全実行経路の契約をまとめるが、強制箇所は各利用責務にも必要。 |
| 保全・消去を独立させ、全状態の意味やwriterは集めない | 全域消去と復元には、各保存対象・処理中利用からの完了根拠が必要。専門の協調責任を置いて局所完了と全体完了を区別する。保持・消去・restoreは目的が異なり、この統合から一律cascadeや一つのtransactionを導かない。 |

## 6. Cross-cutting Responsibilities

「分散」は責任を曖昧にする意味ではない。判断の根拠を提供する側、制約を扱う側、実際の作用・送信・保存を行う側を対応付け、Step 4・5で抜け道を閉じる。

| 責務 | この工程での扱い | 次工程に残す点 |
|---|---|---|
| Permission・同意・scope・失効 | 独立した権限・制約と、実行・送信・参照・保存箇所での適用の協調。自然言語解釈は目的を持つ判断責務に残す。 | 制御変更の由来、現在の有効条件、変更中・失効中の利用との整合性、すべての経路が制約へ接続する依存。 |
| Credentialと秘密の非露出 | 認証秘密を独立させる。推論、実行、提示、記録、Debug captureも値を通常dataへ出さない。登録外の秘密は検出時の不要な保存・送信抑制と失効・更新案内を分担し、完全検出を保証しない。 | 認証用途への受渡しと通常result経路の依存分離。秘密を含まない参照・診断情報の所属。 |
| Targeted deletion | 保全・消去が全域の対象・影響・完了を協調し、全参加先が局所消去、処理中利用・遅延結果の無効化、検証へ参加する。局所Subsystemだけでは完結しない。 | 根拠・派生物の追跡、再保存防止、接続中Clientを含む完了根拠と再起動後の未完了操作。 |
| 保存・backup・restore・Reset | 保全・消去の協調責任と各状態の意味を分ける。Credential除外、外部file非所有、復元後保留、設定Resetの保護範囲を維持する。 | 状態の整合範囲と、保存された設定／現在の実行可能性の区別。具体transactionはここで固定しない。 |
| Audit・診断・Error説明 | 独立Subsystemにしない。Action・Permission・割当・Credential参照・設定・削除等の事実は担当責務が供給し、保全・消去が順序を確認できる記録と保持を支え、入出力・提示が説明する。 | 必要な参照関係と診断経路。会話・file本文・秘密・削除済みprivate内容を別保管庫へ残さない。自動送信せず、手動共有は内容・送信先をOwnerが確認する。 |
| 費用・資源・負荷による縮退 | 上限は権限・制約、利用量と費用の報告・推定・不明は推論等の利用元、縮退は各機能が引き受ける。専用の資源配分Subsystemは追加しない。 | 並列利用でも全体／Provider別capを守る整合性、会話・Owner操作・安全判断を背景作業や描画から保護する依存。容量不足でLearningを黙って削除しない。 |
| 時刻・待機・活動の抑制 | 現在日時・timezoneを必要な活動が利用する。Schedule、backup、観測、自発性はそれぞれの制御scopeに置き、待機だけのLLM pollingを使わない。 | 時刻を参照する関係と、停止・到来・再開の整合性。共通timerや待機機構の採否は詳細設計。 |
| Fullscreen・個体停止・Client不在 | Fullscreen時はそのClientのBodyを入出力・提示、ambient Observationを共有観測、自発発話を個体調整と入出力・提示が休止する。個体停止はそれより広い活動停止、active Client不在はClient依存部分の利用不能として各責務が扱う。 | これらの状態の参照と変化の反映。Host側の通常作業をfullscreenだけで停止させず、Host停止時はClientが独立実行・代替正本を担わない。 |
| Context・由来・最小data | 個体調整・作業等が用途を、認識・学習等が情報の意味・根拠を、権限・制約が利用範囲を、推論が能力差との対応を扱う。独立Context／検索／cache Subsystemを追加しない。 | 情報選択方針の責任関係、各参照でのscope適用、派生dataの非正本性。組立て・検索・scoring algorithmは未定。 |
| 通常保存しないdata | Raw Observation・Raw Voice・詳細Tool payload・内部推論・chain-of-thoughtの非保存を各生成・受入責務へ適用する。保全・消去が保全することは、すべてを保存対象へ追加する意味ではない。 | 通常data経路と明示Debug captureの区別、Client・拡張を含む一時dataの所属。秘密保護と消去は診断経路でも維持する。 |
| 部分障害・Cancel・成功不明 | 各責務が実際に確認できた結果を供給する。作業はTaskとして、個体調整はOwner向け結果として統合し、入出力・提示は受付・完了・不明を区別する。 | 停止や復旧がLLM・Body・拡張の成功へ依存しない関係、結果の鮮度・参照。不明をexactly-once保証やrollback成功へ置き換えない。 |
| 拡張・外部contentのtrust | 実行・拡張がcodeの受入・制限を、利用機能が機能上の意味とfailureを扱い、権限・制約が制御変更を保護する。形式別の専用Subsystemは追加しない。 | 外部code・UIからControl planeや内部dataへ届く依存の制限と、Ene管理下の一時data保護。Plugin ABI・隔離機構は未定。 |
| Locale・accessibility・Setup | 入出力・提示が導線・代替経路・翻訳を扱い、各domainが判断材料・結果の意味を供給する。全設定を所有するSettings／Setup Subsystemは追加しない。 | 表示言語で許可・費用・Privacy・失敗の意味が変わらない参照関係。Setupから新規開始／restoreへ分岐しても各契約は共通。 |

## 7. State Ownership Questions for Step 4

以下は責務境界を成立させるための問いであり、特定のwriterや保存単位の回答ではない。Host正本、Client非正本、外部所有物の非所有は既決事項として扱う。

| 問い | 関係する境界と、明確化が必要な理由 |
|---|---|
| 個体の継続と構成をどう対応付けるか | 個体調整・Character・認識・学習の間で、個体識別、選択済み部品、静的revision、経験による状態をどう区別し、停止・再開・明示更新で何が継続するか。Package更新が経験を上書きしない所属・参照が必要。 |
| 作業の担当、実行主体、各回の状態をどう分けるか | 個体調整・作業の間で、Task、委任・一時Task Agent、Scheduleと各回、共同Task、引継ぎ、Workspace関連付けをどう関連付けるか。担当削除で残る記録と消えるScheduleを表せる必要がある。 |
| 進捗と作用の確定度を何に結び付けるか | 作業・実行・拡張・接続・存在の間で、依頼、実行済み・不明な作用、停止要求、未保存作業、Owner判断をどう対応付けるか。移動・失効・再起動を越えて不明を未実行へ戻さないために必要。 |
| 接続、device許可、active帰属、入出力roundをどう整合させるか | 接続・存在・権限・制約・入出力・提示・共有観測・実行・拡張の間で、何が接続状態で何が個体の帰属か、切替・切断中に何を有効と扱えるか。Client一時dataとHostの状態を混同しない必要がある。 |
| 未伝達事項と会話・作業記録をどう関連付けるか | 個体調整・作業・入出力・提示の間で、Client不在で伝えられなかった事項、元の結果、次Clientでの要約報告の状況をどう扱うか。表示用copyや接続状態を伝達・Task完了の正本にしないために必要。 |
| 意味状態・根拠・原履歴・派生dataをどう区別するか | 認識・学習、会話・作業の記録、保全・消去の間で、現在認識、revision、Summary、source範囲、検索派生物の所属と参照を明確にする。通常History削除、事実訂正、過去の有効性、根拠説明を両立する必要がある。 |
| 共有根拠と個体固有状態の所属をどう表すか | 認識・学習・個体調整・保全・消去の間で、Global Learning、Companion scope、共有Summary、非対称なRelationship、その相手の削除、内部Skill revisionと外部Skillをどう区別するか。削除時の自動Global化や私的状態の共有を防ぐ必要がある。 |
| 一時的状態と継続的傾向をどう復帰させるか | 認識・学習・個体調整・保全・消去の間で、時間経過と保持した根拠を何へ関連付けるか。再起動・restoreによる不自然な初期化と一時状態の永久固定の両方を防ぐ必要がある。 |
| 解釈、許可設定、現在の有効性をどう対応付けるか | 権限・制約と各利用元の間で、Ownerの依頼、Rule解釈・Undo、同意、決定scope、保存禁止、委任、device失効、並列利用量をどう扱うか。保存された条件と実行時に有効な条件、費用報告値・推定・不明の区別が必要。 |
| Credentialの参照と秘密値をどう分離するか | 認証秘密・推論・実行・拡張・保全・消去の間で、用途、参照元、認証状態、更新・失効とbackup除外をどう関連付けるか。通常dataやClientへの永続複製を作らず再認証を説明する必要がある。 |
| 全域消去の対象と完了根拠をどう保つか | 全参加Subsystemで、根拠・revision・cache・保持済みsource・Client一時data・処理中利用・遅延結果の関係をどう把握するか。部分削除、機械的残存検証、再保存防止、未完了状態を成立させる整合範囲が必要。具体transaction・競合機構は別途設計する。 |
| Backup・restore・Resetの整合範囲と実行保留をどう表すか | 保全・消去と全参加先で、復旧可能な正常状態、復元対象、除外、復元されたRule等と再有効化、Auditの参照をどう対応付けるか。通常再起動、旧backupの明示restore、設定Reset、全データResetを一つの初期化にできない。 |

## 8. Dependency Questions for Step 5

ここでは厳密な依存方向、許可API、参照interfaceを完成させない。次の問いへの回答で、既決のtrust boundaryと本書のcollaborationを両立させる。

- **意味判断から制御変更へ何を渡せるか。** 個体調整・作業・学習・観測の解釈と、Ownerに由来する管理操作をどう区別するか。保存されたLearningや外部Promptから、Rule・同意・Credential・capへ直接到達できない関係が必要になる。
- **各利用経路はどう現在の制約へ結び付くか。** Task Agent、軽微な本体処理、Schedule、共有観測、Provider fallback、MCP Apps、Client側の経路でも、scope・Deny・費用・device失効を迂回しない参照と適用をどう成立させるか。一つの中央呼出し列の採用を答えとして先取りしない。
- **何を基準にClient依存を限定するか。** 作業は接続・存在のどの情報を必要とし、どの情報を必要としないか。Computer Useはactive Clientと安全な区切りへ結び付け、通常のHost作業はClient表示の成功に依存させない関係が必要になる。
- **停止・管理・復旧はどの成功から独立するか。** Body・Voice・推論・Task Agent・MCP Appsの終了待ちをせず、OwnerのCancel、承認拒否、停止、保存済みdata確認へ到達する関係をどう構成するか。Companion削除後のTask管理経路も含む。
- **認証経路と通常data経路をどう分離するか。** 接続に必要な秘密利用を可能にしながら、LLM生成argument、result、UI、Learning、Audit、Debug captureへ秘密値が届く依存をどう防ぐか。
- **情報の選択とProvider適応をどう協調させるか。** 個体・作業の用途、学習の意味・根拠、権限による利用範囲、推論先の能力差を、特定Providerに偏らない情報選択へどう結び付けるか。検索派生物・Prompt cacheを正本や権限の根拠にしない関係が必要になる。
- **全域操作へ各責務をどう参加させるか。** 保全・消去が全domainの内部実装へ無制限に依存せず、各参加先の対象・影響・処理中利用・検証結果を集める関係をどう設けるか。復旧や消去のための循環的な完了待ちを避け、遅延結果やClient一時dataを取りこぼさない必要がある。
- **拡張の参加をどこで止めるか。** 機能側がProvider protocol・renderer・Observation adapter等を利用する関係と、実行・拡張による受入・制限をどう両立させるか。外部code・MCP Appsを第一者の管理・保存の任意変更へ接続せず、Local MCPの例外をPluginへ流用しない関係が必要になる。
- **外部所有物と内部copyをどう区別して利用するか。** Workspace、Agent Skills、Character Package、backup、診断共有の各出力を、異なる許可・保持・削除範囲としてどう参照させるか。内部Reset・削除が外部fileやOwner保存backupの削除へ伝播する依存を防ぐ必要がある。

## 9. Design Freedom

本書は責務の境界を固定し、その実現方式は固定しない。

- crate・module・source directory、Rustのstruct／enum／trait、public／private API、内部protocol、IPC／wire format、Plugin ABI／SDK。
- process・service数、thread／async task構成、queue／event bus、callback／actor／message passing、Task Agent loop、具体的な駆動・待機方式。
- DB schema、table／column、repository等の永続化interface、唯一のwriter、transaction・repository・table owner、revision保存単位、lock・concurrency機構、filesystem layout、具体的cache。
- Context assembly、Memory検索・scoring、Learning形成、要約・更新・減衰、Prompt構造、Task化の閾値、候補検知・Capture時機のalgorithm。
- OS sandboxの具体機構、Plugin隔離方式、pairing・失効・帰属調停の実装、音声・観測の計算配置と中継経路、具体的UI構造・renderer・Voice実装。
- Backup形式・整合性実装・暗号方式、対応version、Provider catalog、ReleaseのSupport Matrix・性能budget。

同じSubsystem内にあるTaskとSchedule、MemoryとCompanion State、BodyとText等を、一つの型、保存単位、実行loop、障害単位へまとめる根拠にはならない。逆に、一つのSubsystemがHostとClient、複数moduleへまたがっても責務の二重化ではない。共通の機械的な処理を複数Subsystemが共有することも可能だが、意味・lifecycle・trustの区別を消してはならない。

この自由度は、Cloud正本、Remote専用Core、Companionごとの専用service、恒久Workspace container、成果物専用library、汎用Plugin改変、Ene運営relay・account・Marketplace等の追加を許すものではない。

## 10. Traceability and Completeness

ADは[Architecture Drivers](architecture-drivers.md)、SCは[System Context](system-context.md#boundary-invariants)、RTは[Runtime Topology](runtime-topology.md#runtime-relationships)の判断を指す。表の対応は主要な責任を示し、単独のSubsystemだけでDriver全体が完結することは意味しない。

### Runtime mapping

H／Cは第一者Host／Client、P／M／X／UはTopologyで区別されたProvider／MCP／Plugin code／MCP Appsの境界である。P・M・X・Uそのものを内部Subsystemにしたものではない。

| Subsystem | Hostとの関係 | Clientとの関係 | 外部境界・主なRT |
|---|---|---|---|
| 個体調整 | 個体・対話判断・委任・未伝達事項の継続 | active Clientに属する対話への接続 | Pは推論利用経由。RT-01〜03 |
| Character | 内部静的構成・適用対象 | 編集・確認・表示資材利用 | 交換file・VRM・制作者。RT-08・09 |
| 作業 | Task・Task Agent・Scheduleと記録の継続 | 進捗・管理入力、Client依存作業との協調 | 外部Workspace・作用先、Host側Local MCP。RT-02・03・06・09 |
| 認識・学習 | 意味状態・根拠・利用範囲 | 必要範囲の一時的利用・由来提示 | P、Agent Skills外部原本。RT-05・08・09 |
| 接続・存在 | 接続・帰属の管理 | 排他性・失効・切断時停止への参加 | LAN／Owner管理VPN、device。RT-01・02・04・08 |
| 入出力・提示 | 状態・説明の供給との協調 | Body・Text・Voice・第一者管理 | OS／device、P・X・Uとの限定的な接点。RT-01・02・04・05・10 |
| 共有観測 | 対象・制御・個体文脈との協調 | desktop Capture、状態・停止 | P・X・desktop。計算配置は未固定。RT-02・04・05・08 |
| 推論 | 割当・継承の適用と利用状況 | 条件を満たす入出力経路も可能 | P・未対応protocolのX。RT-04・05・07・08 |
| 実行・拡張 | 通常作業の作用と外部code受入 | active Clientでの作用、機能拡張・Uとの協調 | OS・外部作用先・M・X・U。RT-02・06・07・08・10 |
| 権限・制約 | Owner由来の制御・現在の制限 | 許可・失効等の実効的適用 | 全外部利用に対するEne側制御。RT-01・03・05〜10 |
| 認証秘密 | 一般dataと分離した秘密保護 | 秘密値を露出しない設定・説明 | 必要な認証先だけでの利用。RT-07〜09 |
| 保全・消去 | 正常状態保護・全域操作の協調 | 一時data消去・検証への参加 | backup・export・外部copyの限界。RT-08・09・10 |

### Driver coverage

| Driver | 主なSubsystemと本書で維持する意味 |
|---|---|
| AD-01 | 個体調整・作業・接続・存在・保全・消去。Host正本、Client非依存の許可済み継続、待機のみのLLM polling禁止。 |
| AD-02 | 接続・存在・入出力・提示・共有観測・実行・拡張・作業。Textを含む排他的存在、安全なround、通常作業の非移送。 |
| AD-03 | 個体調整・作業・入出力・提示。同じCompanionを中心とし、会話・委任・管理を相互の完了待ちにしない。 |
| AD-04 | Character・個体調整・認識・学習・権限・制約・保全・消去。静的構成と個体、限定共有、個体削除とGlobal・外部残存。 |
| AD-05 | 認識・学習・個体調整・作業・保全・消去。意味状態、共通の圧縮根拠、原履歴、派生dataの異なる役割と保持。 |
| AD-06 | 意味判断を持つ各責務、権限・制約・実行・拡張・推論・認証秘密。推論やcontentから制御を変更させず、全経路へ制限を適用。 |
| AD-07 | 保全・消去と全参加先、特に認識・学習・作業・推論・入出力・提示。目的別削除、指定文字列検証、根拠・一時data・遅延結果からの復元防止。 |
| AD-08 | 個体調整・作業・実行・拡張・保全・消去。Taskと一時主体の区別、委任元の境界、Task従属のWorkspaceと外部file非所有。 |
| AD-09 | 作業・個体調整・接続・存在・実行・拡張・権限・制約。停止のbest-effort、不明時非再実行、明示再開、Schedule各回の再評価・missed非補完。 |
| AD-10 | 推論・権限・制約・認証秘密と各推論利用元。割当同意、継承、同じ情報選択方針、承認済みfallback、費用の報告・推定・不明。 |
| AD-11 | 実行・拡張・推論・Character・認識・学習・入出力・提示。採用標準、限定Plugin、Local MCPの既定sandboxと個別例外、外部Tool UI。 |
| AD-12 | 共有観測・個体調整・接続・存在・権限・制約。Client共有Capture・関連routingと個体判断、制御scopeと送信同意の維持。 |
| AD-13 | 入出力・提示・作業・推論・権限・制約と負荷を生む各機能。Realtime体験、段階的縮退、Text・keyboard・管理の到達性。 |
| AD-14 | 認証秘密・保全・消去・入出力・提示と記録・利用を行う各責務。秘密非露出、Raw data最小化、Audit、限定Debug capture、手動診断共有。 |
| AD-15 | 保全・消去・作業・権限・制約・認証秘密・認識・学習。正常状態保護、backup対象差、全置換と実行保留、一時状態の経過時間、目的別Reset。 |

### 全体照合と引渡し

作成後に、要件ディレクトリの全5文書、Architecture Drivers、System Context、Runtime Topologyと本書を再読して全体を照合した。製品定義の概念・対象・非目標、要件の全章、AD-01〜15とDriver間の優先関係、SC-01〜10、RT-01〜10およびTopologyのlifecycle・trust・failure boundaryを対象とし、後続milestoneも除外していない。

特に、Client終了と個体停止、個体削除とTask記録、Schedule停止と過去の各回、通常忘却とtargeted deletion、通常History削除と形成済み状態、接続回復とAction再実行、復元済み同意と実行再有効化を同一lifecycleへまとめていない。Subsystem内へ統合した概念についても、この区別をStep 4へ渡す。

Trust boundaryはSubsystemの内外とは別に維持する。共有観測、内部Learning、Provider結果、MCP・Plugin・Tool UIのいずれも制御権限へ昇格しない。消去、Permission、Credentialは独立した調整・保護責務を持つが、各利用箇所の参加を必要とする横断的な契約として扱う。

現時点で、この責務分割を妨げる新たなRequirement Ambiguity／Gapは見つかっていない。解決済みA-01〜A-04／G-01・G-02は再度未決定に戻さず、公開計画時の留保事項も追加要件にしていない。所有・依存・mechanismの未決定は第7〜9節へ明示した。

**Step 4へ進める責務境界が揃っている。** 本書はState Ownership、厳密なDependency Rules、実装構造を決定していない。
