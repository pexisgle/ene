# Subsystem Detailed Design — 個体継続・対話調整 / 作業遂行・実行管理 / 経験に基づく認識・学習

本書は Step 12 Subsystem Detailed Design として、次の3 Subsystemを一つの密接なまとまりとして詳細化する。

- 個体継続・対話調整（以下、個体調整）
- 作業遂行・実行管理（以下、作業）
- 経験に基づく認識・学習（以下、認識・学習）

本書内の I / W / L 番号は本書内の参照用であり、製品要件IDではない。

## 0. 位置付けと不変条件

上位設計との優先順位と矛盾時の扱いは [設計文書 README](../README.md#正本と優先順位) に従う。[Context Assembly](../critical-areas/context-assembly.md)、[Action Execution](../critical-areas/action-execution.md)、[Targeted Deletion](../critical-areas/targeted-deletion.md)、[Client Presence Transition](../critical-areas/client-presence-transition.md)（以下、Presence Transition）、[Backup / Restore](../critical-areas/backup-restore.md)の一般契約は、隣接する確定済み contract として利用し、再定義も所有権の移動もしない。本書内の SO は [State Ownership](../architecture/state-ownership.md) の節番号を指し、CC / RF 番号は対応する architecture 文書（[artifact 一覧](../README.md#artifact-一覧)）の契約 ID である。

新しい semantic owner、第二の正本、万能Manager / Coordinator / Runtime / Policy Engine、統一state machine、共通Context layerを追加しない。

## 1. 選定理由

この3者は Experience、委任意図、steering、結果、訂正、scope、未伝達の受渡しで相互に依存し、分離して詳細化すると同じ対応付けを3箇所で重複定義することになる。同時に、後続が誤って一つの状態・lifecycle・revision・retentionへ潰しやすい境界を最も多く含む。

- 個体調整と作業を分けずに設計すると、長いTaskや判断待ちが会話を占有する構造、または Cancel受付・記録参照を本体LLM応答待ちにする構造が生まれる。
- Task・Task Agent・Schedule設定・各回・Workspace関連付け・外部実体・永続成果物・一時中間fileを一つの Task状態へ潰すと、担当削除後の残存記録、missed非補完、外部非所有の契約が失われる。
- Memory・Skill・Relationship・Companion State・Summary を一つの canonical / schema / revision / retention / 同時更新へ潰すと、事実認識の優先、主体別解釈、一時性と持続性の区別、共有根拠の分離が失われる。
- History・Summary・現在認識・派生物を混同すると、逐語引用の復元主張、古い根拠からの再形成、cache hitによる権限復活が生まれる。

最初のmilestone（テキストtimeline、Companion scopeの Summary・Memory形成・由来・想起・訂正・targeted deletion、Workspace folderを対象とする Task Agent委任の file作業、Client不在時の Task継続と Host再起動後の明示再開）は、この3者の受渡しが閉じて初めて検証できる。Voice・Observation・グループ・自発交流・Global・Skill生成・Companion State・Relationship発達・Schedule・Remote・Character配布は後続milestoneだが、本書はそれらの確定済み契約を除外せず、将来の受渡しを塞がない形で定める。

## 2. 内部責務の分離

### 2.1 個体調整

個体としての応答と判断を Client・Provider session・一時Task Agentを越えて続ける。停止・削除の強制をLLM判断へ従属させない。

| 内部責務 | authoritativelyに判断するもの | 持ってはならない正本 |
|---|---|---|
| I-1 同一性・活動状態・適用関係 | Characterを起点とする別個体としての生成・継続、停止・再開・削除の対象、適用済み部品・revision選択。Owner操作に基づき、LLM応答の成否に従属させない。 | 静的内容自体（Character）、Learningの意味内容（認識・学習）、Permission・同意・cap（権限・制約） |
| I-2 対話継続・参加関係 | 一対一の継続、グループとCompanion間交流の参加関係、誰の会話・発言か・どの空間か。Text / Voiceを同じtimelineへ結び付ける。 | 実際の入出力round・提示・区切り（入出力・提示）、Taskへの反映（作業）、帰属成立（接続・存在） |
| I-3 Historyと非会話活動記録の意味 | 受け取った／伝えた発言と参加者・文脈の原記録の意味。Observation event認識結果・notification生成・軽微な内部調査等について何が行われ何を認識・報告したか。Task・Action・Auditの事実は既存ownerを参照し、ここで独立更新しない。 | Task進捗・作用確定度・Audit順序（各owner）、Provider session・表示copy（派生・一時） |
| I-4 依頼受入・委任判断・結果統合 | 受ける／条件を確認する／断る、まとまった作業の Task化・委任、steeringの受付・結果統合。軽微な情報取得・補助処理は自身で行える。 | Taskへの受理・反映・達成（作業）、許可の確定（権限・制約）、作用成功（実行・拡張） |
| I-5 自発性判断 | 観測eventの個体文脈での意味判断、自発発話・通知・内部調査・Companion間交流・移動必要性の判断。個体別の OFF含む頻度・上限、未応答抑制、会話上の順序・loop抑制。Quiet hours・Mute・未応答・Permission・費用・資源・loop制限を自発性より優先する。Ruleをtriggerにしない。 | Capture・候補検知・routing（共有観測）、帰属成立（接続・存在）、Action許可（権限・制約＋実行・拡張） |
| I-6 未伝達管理 | Client不在のため伝えられなかった事項・対象・元対応・次Clientでの要約報告状況。「何をまだ伝える必要があるか」の正本。Task由来なら Task記録、会話・交流由来なら活動記録へ参照を戻す。報告用要約は派生表現。 | Task進捗・結果自体（作業）、提示の実際（入出力・提示）、報告済み＝Task完了・承認済みという推定 |
| I-7 進行中の意味判断 | 現在入力への応答・中断・会話上の取扱い。推論・入出力の一時処理であり、残すべき発言・結果は History等へ反映する。 | 会話の正本としての Provider session、timelineの終了・削除としての round終了 |

I-1〜I-7は同じ個体に属するが、一つの状態・lifecycle・保存単位ではない。適用済み構成の変更と経験状態の変更、現在の参加と過去の参加記録、進行中の判断と保持された発言、未伝達の必要内容と元記録・報告状況を区別する。停止は活動を止めてdataを保持し、activeなし・Body hide・静かな傾向と混同しない。削除は横断操作であり、Task記録・Global Learning・外部fileを個体所有物として全削除しない。

### 2.2 作業

作業の開始契機から遂行、判断待ち、結果確認・再開までを、通常会話やClient寿命から独立して扱う。Task・Task Agent・Schedule設定・各回・関連付け・外部実体・成果物・中間fileを一つの型・保存単位・実行loop・障害単位へまとめない。

| 内部責務 | authoritativelyに判断するもの | 持ってはならない正本 |
|---|---|---|
| W-1 Task追跡 | 受けた目的・担当・参加、採用した追加指示、進捗・判断待ち・完了・失敗・Cancel、結果・未完了・次の判断。Task記録は Agent終了後も担当削除後も残り得る。 | 会話上の受付・結果統合（個体調整）、個別作用の確定度（実行・拡張）、承認の確定（権限・制約） |
| W-2 委任・一時Agent対応 | Taskまたは一部を誰からどの範囲で委任されたか、進行・待機・停止・結果受領の対応。並列・再委任も委任元と Task境界内へ従属させる。Agentに長期人格・Relationship・Agent scope Learningを作らない。 | 委任元の権限・費用・Provider設定の独立所有、別Taskの承認・Credential・旧担当の私的Learning |
| W-3 steering受入・反映 | 発言としての追加指示を Task contextとして採用した内容・反映可否・反映不能の理由と選択肢。発言の正本は History、承認の有効性は権限・制約に残る。 | 会話受付＝Task反映済み・承認済みという推定、Task待機による通常会話の占有 |
| W-4 Schedule設定・到来・各回 | 担当・実行内容・時刻条件・作成時timezone・停止等・初期Workspace入力、到来した回の missed / 開始対応、各回の新Task対応。作成依頼を特別tokenにしない。各回で現在の制限を再評価する。 | 観測Capture時機・backup schedule（各owner）、missedの自動補完・実行済み書換え、担当削除時の自動引継ぎ |
| W-5 Workspace関連付け・保存先 | Task従属の内部関連付け、許された Workspace folderの既定先扱い、永続保存先未定時の最終保存前確認。同じ外部folderを使う Task間で作業状態・承認を共有しない。 | 外部folder・source・成果物の所有、関連付け削除の外部への cascade、ene専用libraryへの複製 |
| W-6 Task context・内部copy・中間file | Taskのため採用した目的・指示・判断材料・途中理解の用途・有効性、内部保持copyの由来・取得時点・用途の区別、一時中間fileの用途・必要期間・安全な整理対象。Task限り情報を Learningへ自動昇格させない。 | 元正本（会話・案内file・Learning）の上書き、外部原本の現在値、推論作業領域・詳細payloadの全保存 |
| W-7 記録保持・削除・引継ぎ | Task終了≠記録削除、Task削除時の固有関連付け削除、指定日以前logの保持整理範囲の明示、残る記録の管理面からの到達可能性、Owner依頼による引継ぎ時の担当・作業条件の更新。 | 形成済みLearning・Summaryへの通常削除の cascade、外部成果物の所有・消去、旧担当の私的Learning・承認・Credentialの包括継承 |

W-1の現在状態は実行・拡張の作用・確定度と対応するが、同じ確定度を作業側で独立更新しない。W-2の委任範囲・結果・失敗等の残すべき事実は Task記録の一部として残す。W-4の次回表示は保存条件と現在日時・timezone規則からの導出であり、独立した時刻条件の正本にしない。W-5〜W-6の内部copy保持は Privacy・秘密保護・retention・targeted deletionの対象であり、外部所有を理由に除外しない。

### 2.3 認識・学習

共通Experienceをどの状態の理解・変化へ用いるか、訂正と由来説明を同じ責任で扱う。各概念の役割・保持契約を保つことが統合の条件である。一つの canonical・schema・revision model・retention policy・一律同時更新を導かない。

| 内部責務 | authoritativelyに判断するもの | 持ってはならない正本 |
|---|---|---|
| L-1 Experience受入・形成判断 | 対話・Task・Tool・Observation・交流等の Experienceから保存価値・形成・更新・統合・想起必要性を文脈で判断する。保存価値と今回想起の必要性を分ける。全件保存・逐次確認を要求しない。 | Raw Historyの代替保管、一般世界知識の蓄積、Task化の閾値、権限・同意・capの変更 |
| L-2 Summary・根拠関係 | 意味的まとまりとして圧縮した Summaryと、どの形成・revision判断がどの根拠を用いたかの関係。複数継続状態の共通根拠にできる。元 Conversation・Task等の大まかな source範囲への参照を保つ。 | 現在知識の正本としての Summary、Raw保存の要求、検索cache寿命での破棄 |
| L-3 Memory | 現在の主要な知識・時間的意味・重要度・Companion / Global scope・変更経緯。通常の形成・更新・統合・訂正・想起抑制は Experienceと文脈に基づくLLM意味判断に従う。誤訂正と時間的変化を区別し、通常忘却は想起抑制とする。 | 一般世界知識・Raw履歴・Task限りdataの無条件蓄積、queryごと検索scoreとの同一視、汎用Memory editor |
| L-4 Skill | 再利用手順・専門知識・注意・補助resource、有効revision・由来・scope、原本保護・別revisionとしての改善、以前の有効revisionへの復帰、根拠と未検証・成功・失敗の対応付け。Workspace内 Skillは外部fileとして参照する。 | 外部Package原本の所有、内部更新の外部原本への自動反映、成功検証済みであることの定義条件化 |
| L-5 Relationship | 主体Companionごとの compactな関係解釈（距離感・交流傾向・関係変化等）。共有Experience・過去関係・新出来事を根拠に更新する。A→BとB→Aは別意味。事実矛盾時は Memoryを優先して再解釈する。 | 人物情報・Preference・出来事の第二の Memory、対称化・共同所有、任意数値の一般editor、Permission・Ruleへの影響 |
| L-6 Companion State | 主体自身の現在の内的状態と表現・注意・会話・行動への傾向。一時的状態と持続的傾向を区別し、経過時間を考慮した継続・再解釈を扱う。一時値の無期限固定も全過去値の恒久保存もしない。 | 詳細事実・関係解釈の複製、Character静的設定の書換え、出力表情・motionの正本化、Permission等への影響 |
| L-7 scope意味・制御制約の区別 | 内容・由来・共通利用必要性からの Companion / Global scope形成・変更の意味判断。個体由来は Companionを既定にし、重要度と共有を分ける。Ownerの明示的な保存禁止・非共有等の control constraintは意味状態と区別する。 | scope自体の強制所有（権限・制約と各利用箇所の協調に属する）、明示制約の解除、重要度だけでの Global化 |
| L-8 由来説明の供給 | 現在認識・過去revision・根拠・誤訂正と時間変化の区別の説明可能性。内部 chain-of-thoughtを用いない。正確な発言・詳細は保持された Historyを参照させる。 | Historyの意味の所有、Task達成・作用成功の確定、制御変更の確定 |

L-3の過去revisionは変更経緯の正本であり、現在認識と同格の競合正本ではない。L-4の復帰は「どの手順を現在利用するか」の変更であり、後続revisionや実行結果をなかったことにしない。L-5〜L-6は個体固有で自動共有せず、Companion削除の対象とする。L-7の Global化は Owner明示共有または内容・由来・文脈から共通利用すべきことが明確な場合に限り、不明なら Companionに留める。L-2〜L-8の Summary・revision・根拠は検索派生物・cacheの寿命で捨てない。

## 3. 管理する logical state

特に外部・copy・一時と記したものを除き、Hostで管理する内部状態を指す。永続化の実装担当・保存単位・唯一writer・transactionを指定しない。通常ownerは正当な全域消去への参加を拒めない。

### 3.1 個体調整が意味ownerとなる state

- Companion同一性・活動状態・適用済み構成（SO 4.2）。部品内容は Character、Learning意味は認識・学習、Provider同意等は各意味ownerに置く。
- 一対一継続・グループと交流の参加関係、Conversation Historyの意味、保存された非会話活動記録・evidenceの意味、進行中の意味判断（SO 4.3）。入出力roundの受付・提示・区切りは入出力・提示に残る。
- 未伝達事項と報告状況（SO 4.4）。Task由来なら Task記録、会話・交流由来なら活動記録へ参照を戻す。報告用要約は派生表現。元記録のない通知等は必要内容自体を活動記録として必要範囲で保持する。

Client上の入力中data・表示timeline・音声bufferは一時表現である。表示やround状態を過去発言・Task進捗の第二の正本にしない。交流への参加は私的Learning等への access許可にならない。削除後も過去発言・活動の主体を識別できる記録と現在利用できる個体状態を区別する。

### 3.2 作業が意味ownerとなる state

- Task目的・担当・進捗・結果と Task context（SO 4.10）。Task contextは用途・有効性を管理し、由来・取得時点・用途の区別を残す。
- 委任と一時Agentの実行対応（SO 4.11）。Agent自体は永続ownerではなく、残すべき事実は Task記録の一部とする。
- Workspace関連付け・保存先選択・判断待ち、内部保持copyの意味、一次中間fileの用途・期間・整理対象（SO 4.13）。外部実体・案内file・Skill・成果物は Ownerまたは外部systemの所有物であり、eneは許された Actionで利用する。
- Schedule設定・到来した回・各回の Task（SO 4.14）。設定と発生記録と各回 Taskを別に扱う。次回表示は導出であり独立正本にしない。

Task終了は実行の区切りであり記録削除ではない。Task削除と指定日以前logの保持整理を区別し、通常記録削除を形成済み Learning・Summary・外部成果物へ cascadeさせない。

### 3.3 認識・学習が意味ownerとなる state

- Summary・根拠関係・source参照（SO 4.5）。Summaryは判断時に用いた圧縮evidenceの記録であり、現在知識の正本にしない。共有根拠の保持は全内容の共通利用許可ではない。
- Memory現在認識・重要度・scope・過去revision（SO 4.6）。形成時保存価値と今回想起必要性を分け、意味として更新された重要度と queryごと検索scoreを同じ正本にしない。
- 内部Skill・import原本との対応・学習revision・実行結果（SO 4.7）。Character推奨は静的指定であり、有効revision切替え・実行許可ではない。
- Relationship（SO 4.8）。主体別の現在解釈の正本であり、Memory検索からの毎回再生成cacheでもない。
- Companion Stateの一時状態と持続的傾向（SO 4.9）。前者も意味のある間は正本であり描画bufferと同列の廃棄可能dataではない。後者は形成・重要変更の Experienceを関連付ける。

通常の忘却・訂正・失効等では保存済み Learning・過去revision・根拠を消去しない。Companion scopeは個体削除の対象、Globalは個体削除後も残る。scope変更条件と旧根拠の非共有は SO第6・7節に従う。

### 3.4 canonical / derived / temporary / external reference の扱い

- canonicalとして参照するもの：Memory現在認識、保持された History、関係解釈は Relationship・内的状態は Companion State・詳細事実は Memory、Character revision・適用関係・Experience由来状態の各意味、作業の Task進捗・結果と実行・拡張の個別作用、Owner由来の制御条件とその時点の評価済み有効性、外部現在内容は外部所有者・内部取得採用内容は記録owner。
- derivedとして扱うもの：報告用要約・由来説明・進捗表示・Body・Voice出力・embedding・index・similarity・score・Prompt cache・Provider session・一時context・有効経路・次回表示・費用集計。派生物の生成・保持・破棄を行う責務が元stateとの対応と利用範囲を説明し、訂正・scope変更・targeted deletionへ参加する。古い派生物から現在状態・権限を復活させない。
- temporaryとして扱うもの：Raw Observation・Raw Voice・詳細Tool payload・内部推論・chain-of-thought（通常保存しない）、Client表示copy・入力途中・音声buffer・観測候補・推論中context・MCP Apps表示data、I-7の進行中判断・W-2の Agent一時context・Lの一時 Companion Stateの buffer的側面。失ってよいのは一時表現であり、受理済み指示・必要作業記録・未伝達・作用不明まで失ってよいわけではない。
- external referenceとして扱うもの：外部Workspace実体・案内file・Skill・成果物、外部Package・Skill原本・export済みcopy・Owner保存backup、Provider / MCP側固有状態。内部保持copy・内部取込 Skill・ene管理下の一時dataは内部の利用・消去責任に戻る。外部code由来を消去除外条件にしない。

Experienceは出来事と結果の概念であり、全活動を格納する新しい canonical Raw storeを設けない。Summaryを作らずに終わる出来事、Rawを保存せず Summaryを根拠として残す形成がある。出力から Character・永続状態への自動逆更新は導かない。

## 4. Subsystem間の受渡し（semantic contract）

「AがBを呼ぶ」の実装構造を先に決めるのではなく、要求開始・authoritative判断・受渡内容・失ってはならない前提・結果受入を定める。非同期message・同期call・shared state等の mechanismは後続へ残す。

### H-1 Experience → 形成・利用

- 開始：個体調整・作業が活動と結果を Experienceとして認識・学習へ渡す。会話・Task・Tool・Observation・交流のいずれも同じ入口条件（保存価値の事前確定を要求しない）で渡せる。
- 判断：保存価値・形成・更新・統合・想起必要性の意味判断は認識・学習。個体所属は個体調整による任意編集権ではない。Task限り情報の Learning化は別判断であり、作業が Task contextとして保持することと混同しない。
- 受渡内容：出来事と結果、必要な文脈、訂正の場合は訂正対象と新 Experienceの区別。Task経験には検証状態・実行結果の区別を含める。Agent経験を長期人格へ帰属させない。
- 失ってはならないもの：由来（発言・観測・推論・Tool結果の区別）、対象Companion・Task・委任との関係、Client依存なら取得Client・round・観測候補との関係、停止前後・再起動前後・restore前後の継続関係、期待する利用先（返答・Learning候補・routing・Permission解釈・Task判断の別）。
- 受入：認識・学習は必要な状態だけを形成・変更し、Summaryを共通根拠として対応付ける。応答完了と全 Learning更新完了を同一条件にしない。全状態の同時更新・全Raw保存を要求しない。

### H-2 会話による訂正

- 開始：個体調整が会話で受けた訂正・変化の示唆を認識・学習へ渡す。個体調整が会話で訂正を受け取ることは学習状態の直接上書きではない。
- 判断：対象を定めた意味変更は認識・学習。最初から誤っていた訂正と、以前は正しかった状況の時間的変化を区別し、過去有効性を表せる関係を保つ。Relationshipが旧事実を参照していれば Memory優先で再解釈し、旧事実を別正本から再提示しない。
- 失ってはならないもの：訂正前の認識・根拠・revision、訂正の根拠となった新 Experience、時間的意味。過去発言を現在認識へ書き換えない。
- 受入：Ownerは現在認識・由来・経緯を確認できる。汎用Memory editor・内部数値の一般editorを入口にしない。

### H-3 Task化・委任・steering・結果

- 開始：個体調整がまとまった作業の Task化・委任・steering・Cancelを作業へ要求する。管理面からの Cancel・記録確認は Companion応答成功を必要としない。
- 判断：Taskへの受理・反映・達成は作業。会話上の受付・結果統合は個体調整。Permission判断は権限・制約。軽微な処理の Task化省略は本体が行える範囲であり、閾値・分類algorithmを本書で固定しない。
- 受渡内容：Task化する意図・目的・steering前提・Workspace利用条件・Cancel対象。作業は進捗・結果・判断待ち・反映不能の理由と選択肢を返す。
- 失ってはならないもの：発言記録と Taskに反映した内容と反映不能・判断待ちの対応、steering前後の目的の区別、委任範囲・Workspace範囲・Client依存条件。会話受付を Task反映済み・承認済みにしない。Task待機で通常会話を占有しない。
- 受入：個体調整は Task正本を参照して Ownerへ統合報告する。作業は担当の活動状態・必要な個体判断を個体調整へ求める場合も、Cancel受付・保存済み結果参照を本体意味判断に戻さない。

### H-4 Task contextと Learning・History・Summaryの分離

- 作業の Task context、個体調整の History・活動記録、認識・学習の Summary・Learningは別の正本・lifecycleである。相互に直接書換えを要求する連鎖を作らない。
- Task限り情報を Learningへ自動昇格させない。実行経験・検証状態を形成判断へ渡す場合も、Agentが長期状態を直接保存しない。
- 正確な引用には保持された History、現在認識には Memory、圧縮根拠には Summaryを用いる。検索順位・短さで旧 Summaryを訂正後 Memoryの代わりにしない。原履歴消失時は生成で補わず、正確に引用できないことを保つ。
- 利用先は Summary・過去revision・source・共有contextを、利用者・目的に対応した範囲だけで取得する。共有currentから私的過去全文へ辿らせず、取得後のcopy・検索結果も現在の制限に従わせる。

### H-5 scope・共有範囲の変更

- 判断：内容・必要背景・scopeの意味は認識・学習。個体由来は Companionを既定とし、重要度と共有を分け、重要度だけで Global化しない。不明なら Companionに留める。Task・Workspace・Client・グループ参加を Global化の根拠にしない。
- 強制：決定後の scope・Ownerの保存禁止・非共有の迂回不能な適用は権限・制約と各利用箇所の協調に属する。認識・学習が意味ownerになる構造を避ける。具体的な依存方向・API・制約表現は本書で定めない。
- 受渡内容：共有する内容と必要な背景の選択・要約（LLM意味判断、Ownerの明示的非共有を優先）。共有された current内容から私的旧revision・元 History・Summary全文へ accessを広げない。参照・検索・処理中copyも新しい範囲に従わせる。
- 禁止：削除を契機とする自動 Global化、Schedule自動引継ぎ、重要・有用・一般的嗜好だけでの昇格。Global化の判断だけを目的とする通常の逐次確認は要求しない。
- Skill固有：Character推奨からの取込みは Companion scopeを既定として各個体に別々に属する。単体importは Ownerが Companion / Globalを選べる。以後の共有・個体削除は通常の scope契約に従う。

### H-6 Observerへの限定routing文脈の提供

- 開始：共有観測が必要な限定文脈を、個体調整との協働を通じて得る。History・個体文脈は個体調整、Memory・Learningは認識・学習、Task contextは作業が元情報の意味と提供範囲に責任を持つ。Task contextの参照は既存の個体調整–作業の協調を通じて行い、共有観測へ Task所有・包括 accessを移さない。
- 判断：提供範囲の意味は各元owner、送信・利用制限は権限・制約と各利用箇所。Observer専用assignmentの送信同意と元情報の利用制約を変換後にもともに適用する。Companion側の同意・overrideを選択・合成して代用しない。
- 受渡内容：元情報に従属する routing用途の一時的な派生表現であり、Summary・新たな Learning正本・新scopeではない。元情報・対象Companion・用途・制約の対応を保つ。個体話題・Task変更時は元ownerが変更と提供済み文脈の対応を判定できる必要がある。
- 禁止：Global化・他Companionへの共有・private全体の公開、要約・変換による制約消去、scope変更・同意失効・消去の処理中派生物への不反映。生成方法・model / Provider・形式・更新頻度・鮮度・選択algorithmは固定しない。

### H-7 未伝達と Task記録・Historyの対応

- 個体調整の未伝達管理は「何をまだ伝える必要があるか」の正本であり、Task進捗・結果の第二の正本ではない。Task由来なら Task記録へ参照を戻し、報告用要約は派生表現とする。
- 接続・表示copy送信・Task完了だけで報告済みにしない。実際の提示状況を受けて個体調整が報告状況を更新し、不明な提示を確定済みと表示しない。
- Companion削除ではその個体の現在の未伝達管理を終えるが、元の Task記録・会話History・保存された活動記録は SO 4.3の保持契約に従って残す。報告状況の終了を唯一保持した通知内容等の historical record消去へ結び付けない。
- 通常保持管理で元記録が失われる場合は未伝達の必要内容との関係を確認する。専用の永久通知履歴・既読配信保証は要求しない。

### H-8 Schedule設定・到来・各回の対応

- 設定・発生記録・各回 Taskを別に扱う。到来した回が missedか・どの Taskに対応するかという発生記録は設定とは別である。
- 開始する各回は新しい Taskとなり、その進捗・結果の正本は W-1に置く。missedを後から実行済みに書き換えない。Run nowは現在条件で新しい Taskを開始する。
- Schedule停止・変更・削除は将来の起動条件に対する操作であり、既存各回の Task記録を消したり結果を書き換えたりしない。担当削除では Scheduleを削除し自動引継ぎしないが、残る各回 Task記録は管理可能にする。
- 作成依頼・保存済み設定は特別な Permission tokenではなく、各回で現在の Capability・Rule・cap・Provider・Companion・Host状態を再評価する。観測Capture時機・backup scheduleはこの受渡しへ統合しない。

### H-9 軽微処理と Task化の境界

- 労力が非常に小さい処理・会話中の短い取得・判断のための軽い調査・event理解のための小規模収集・独立作業とするほどではない補助処理は Companion本体が行える。Task化・Task Agent化の閾値・分類algorithmは固定しない。
- Taskを経ない軽微Actionも権限・作用・記録の境界は省略しない。個体調整が必要な活動記録を管理し、実行・拡張の確定度を参照する。これを理由に全Actionを Task化しない。
- まとまった調査・複数step・file実作業・Computer Useを含むまとまった作業・長期継続・並列委任価値のある作業は Task化・委任の原則に従う。Owner依頼か自発開始かだけで原則を変えない。自発的な発話・軽微な内部調査まで一律に Task化・Task Agent化しない。

### H-10 管理経路の独立

- 個体停止・削除、Task Cancel・記録確認、Schedule管理、消去・backup・restore・Resetの要求は、第一者の管理経路から各担当ownerへ直接届く。ここで「直接」は本体LLMの承認・長時間Taskの完了を介在させない意味であり、具体APIの指定ではない。
- UIに入出力 round・Task進捗・由来説明の任意書換権を与えない。各ownerの受理・確認・結果に従う。受付と完了、保存済みdataへの影響、既知作用と不明を区別して提示する。
- 担当削除後も残る Task記録・判断・停止結果へ到達できることと、帰属切替中の Host継続は別の契約だが、いずれも Task記録の正本を作業に残すことで成立する。

## 5. Lifecycleとの関係

lifecycle変更の多くは owner移転ではなく、同じownerが管理する状態の存続・利用可能性・参照範囲の変更である。異なる目的の操作を一つの初期化・一つの状態機械へ潰さない。

| 事象 | 本クラスタが変更する意味 | 維持・分離する意味 |
|---|---|---|
| Companion生成・Character部品更新 | 個体調整が個体への適用を調整する。生成は別個体として識別でき、更新は Ownerが選んだ部品に対応する。 | 経験状態の上書き・配布物への混入をしない。適用されていない部品を更新済みにしない。Character内容・revisionは Characterに残る。 |
| Companion停止 / 再開 | 停止中は presenceなし・Observer人数外・通常interaction / Computer Use / 自発活動 / 新Task / 新Schedule実行の不開始。実行中Taskは best-effort Cancel。再開時は適切なClientへ再配置できる。 | 同一性とdataを保持する。再配置hintは現在 presenceと区別する。停止中の回は missedのままで、再開が旧Task・不明Actionの自動replayを意味しない。 |
| Companion削除 | 個体固有設定・Summary・Companion scope Memory・内部Companion scope Skillと過去revision・Companion State・その他固有Learning、主体 / 相手の Relationship、担当Scheduleを削除する。 | 一対一・グループ・Companion間交流 History、非会話活動記録・evidence、Task記録は個体削除だけでは消さない。Global Learning・残す共有Summary・外部fileを維持する。自動Global化・Schedule自動引継ぎをしない。 |
| Task Agent終了 / 失敗 | 一時的遂行を終え、結果・失敗・不明を Taskへ対応付ける。 | Task記録・Companion・Scheduleを削除しない。長期人格を残さない。 |
| Task完了・失敗・Cancel | 進行を区切り、結果・未保存・外部作用・次の判断を報告し、中間fileを安全に整理する。 | 記録・関連付けを終了だけで削除せず、外部成果物を所有・消去しない。 |
| Task削除 | 固有の内部記録・関連付けの削除範囲を扱う。 | 外部source・成果物・別Taskの同folder関連付け・形成済みLearning・Summaryへ通常削除を cascadeさせない。 |
| Schedule停止 / 削除 | 将来回の起動条件を停止 / 除去する。 | 既存各回の結果を変更せず、missedを実行済みにしない。 |
| Learningの通常忘却・訂正・統合・失効・置換 | 認識・重要度・想起・現在利用revision等を変更する。 | 保存済み内容・過去revision・根拠を保持し、誤りと時間的変化を区別する。 |
| scope変更 | 認識・学習が共有する意味内容と必要背景を判断し、利用範囲を変更する。 | semantic ownerは移らない。重要度を共有根拠にせず、現在内容の共有から私的過去revision・source全体を共有しない。 |
| 通常History / log削除・retention cleanup | 原記録の範囲を減らす。 | 形成済み状態と Summaryは残し、Raw参照不能を明示する。消去目的を targeted deletionへ変えない。 |
| Targeted deletion | 復元に寄与する内部dataと処理を横断して対象情報を除去・復元不能化する（第7節）。 | 通常ownerは移らず、完了後の Owner新規提供・外部copy消去保証と分ける。 |
| 移動・切断 / activeなし | 接続・存在が帰属を切り替え、Client依存活動を区切る。通常Host作業は移送・停止しない。 | 個体・通常Host作業・内部状態の ownerは移らない。未伝達を Hostで保持し、未確定Actionを移動先へ replayしない。 |
| Host再起動 | 個体と保存済み状態を継続し、途中Taskは明示再開待ち、停止中の回は missedにする。Running presenceは元Clientへの復旧という別条件に従う。 | 過去作用不明・未伝達・未完了消去をなかったことにしない。 |
| Provider / model変更・障害・cache失効 | 推論の利用可能性・能力・承認済み経路を扱う。 | 個体・履歴・Learning等を初期化せず、同じ情報選択方針と安全境界を維持する。接続回復で Actionを replayしない。 |
| Backup / restore | copyを作成し、明示restore成功で現在の Credential storeを除く対象内部状態を backup時点へ全置換する（第7節）。 | copyは live正本でない。復元後も実行は保留、外部世界は巻き戻らず、Credentialは現在storeを維持して照合する。 |
| 設定Reset / 全データReset | 前者は一般設定の既定化、後者は強い確認後の内部data・Credential削除。 | 前者の保護対象と後者の外部除外を混同しない。 |

Globalへ形成・変更済みの Learningは、その後に由来Companionが削除されても Globalとして残る。ただし削除時に残す根拠と失う source参照を説明する。Relationshipと Companion Stateは Companion / Globalの Learning scope変更による共有対象にしない。共有Experienceがあっても各主体の状態を独立に更新する。

時間は stateの意味にも作用する。Scheduleは選択時timezoneと時刻条件、Companion Stateは一時性と経過時間、保持方針・Debug captureは対象期間、自発性は現在時刻・Quiet hoursをそれぞれのownerが利用する。選択済み Schedule timezoneを黙って書き換えない。

## 6. failure / cancellation / restart時の責任

Cancel・切断から外部作用の取消・不存在を推測せず、不明を未実行に戻さない。状態参照・Client移動を Action replayの理由にしない。

- 個体調整：停止・削除の管理経路をLLM応答成功から独立させる。停止・削除時は新規活動を開始させず、作業の best-effort Cancelと Schedule削除へ接続する。active不在でも可能な Host内活動を続け、伝えられなかった事項を次Clientで報告する。移動・切断中の到着物は元帰属・元roundへ対応付け、到着先の現在活動へ付け替えない。
- 作業：Cancel受付と遂行停止を本体LLM・Agentの正常終了待ちにしない。実行・拡張の停止不能・既知作用・不明を参照して報告する。Host再起動後の途中Taskは保存済み進捗・既知作用・不明・未完了を示して明示再開を待つ。Agent session残存・Provider回復・Host自動起動選択だけでは再開しない。外部作用不明時の自動再実行、Client間移動・接続回復による replayを行わない。steering後の旧結果を新目的の達成にせず、古い承認で Cancel・失効を解除しない。
- 認識・学習：形成・訂正と削除・失効・停止・保留の競合では、対象範囲の利用・再保存を確実に制限できることを成立条件にする。削除前の情報を利用する実行中形成によって削除済み情報を再保存しない。古い根拠だけからの自動再形成をしない。訂正後に届いた古い形成結果で現在値を無条件に上書きしない。
- 共通：受付・開始・作用・記録保存・Task達成・報告は別の事実であり、一つの成功状態へ潰さない。確定度を変える場合は事実ownerが新たに確認した根拠を用いる。停止要求の受付と外部作用の停止完了を分ける。Background・Scheduleの無人実行も同じ条件に従い、Client不在・Schedule到来・復元成立を自動承認の理由にしない。Owner確認が必要なら実行せず判断待ちにする。

再構築可能であることは全文context・内部推論の永続保存を要求しない。継続に必要な Task状態・未伝達・作用不明・全域操作未完了等を本来のownerが保全し、一時bufferの消失は成功・完了の根拠にしない。

## 7. Step 11 contractへの参加

### 7.1 Context Assemblyへの参加

- 個体調整：用途確立に必要な対応（対象活動・目的・担当・Task / 委任・Client・round・観測候補・継続関係・期待利用先）を供給し、History・個体文脈の提供範囲の意味を持つ。引用を含む Owner入力の全体を無条件に Owner命令とせず、Voiceに話者認証済みの意味を足さない。会話・Ownerへの伝達の用途別受入では、現在の会話・未伝達へ対応付け、生成済み＝提示済みにしない。
- 作業：Task context・内部source copyの提供範囲と意味を持ち、既存の個体調整–作業の協調を通じて Observerへの Task文脈参照を仲介する。Task判断の用途別受入では、現在目的・steering・委任・状態へ照合し、旧目的への回答＝新目的達成にしない。Cancel解除をしない。
- 認識・学習：Memory・Skill・由来・scopeの提供範囲と意味を持ち、Global本文は使えても私的根拠は漏らさない。第二の共有正本を作らない。Learning・Summary形成の用途別受入では、現在認識・根拠・scope・保存条件へ照合し、到着順が新しい＝根拠が新しいにしない。Task contextの自動永続化をしない。
- 共通：参照・変換・実送信・結果の保存・提示・作用は別の利用境界である。事前の取得成功・推論成功から後段の権限を作らない。本文中の文字列だけで強制側の状態を変えない。分離を確認できない派生物は混合元の制限を免れない。Provider適応では論理的選択方針を維持し、必須の意味が表現できなければ不足を返す。cache・session・Client経路の再利用でも範囲・用途・現在性を確認し、hitを理由に制約確認を省かない。Permission解釈・消去探索の補助推論も例外にせず、審査対象Actionの許可・実行を先行条件にしない。
- Observer限定文脈には H-6の契約で参加する。routing候補採否と開示本文の制限を別責任として成立させ、混合生成文しかなく分離を確認できなければ個体へ渡さず構成し直すか不足とする。
- 処理中 context・検索派生物・cache・Client / 拡張の一時copy・戻り得る結果を含めて消去へ参加できるよう、sourceを消した後に依存関係も消失し遅延結果を識別できなくなる実装を不可とする。必要な関係は本文を保持せず維持できるようにする。

### 7.2 Action Executionへの参加

- 個体調整：軽微な本体Actionも委任作業も、実対象への作用と結果を権限・制約に従って実行・拡張へ要求する。すべてを Task化する要求ではない。作用事実・Task達成・報告完了を一つの成功にしない。
- 作業：Task・委任範囲を管理し、Agentの推論・Actionを推論 / 実行・拡張の責任下に置く。別Agentでの Deny迂回・別Taskの Credential流用・独立予算・Provider overrideの作成を許さない。Task記録は実行・拡張の作用・確定度を参照・集約し、同じ確定度を独立更新しない。Agent申告を作用証拠にしない。
- 認識・学習：形成判断に必要な保持済み発言・活動記録・Task事実・作用確定度を、原記録を所有せず参照する。履歴消失を架空の復元で埋めない。参照対象は利用可能な範囲に限る。
- 共通：認可判断は判断対象と実利用の対応であり、記録と生きた許可は別である。開始前に現在条件と実対象解決を成立させ、重要な変更では再評価する。無関係な変更での再承認は要求しない。委任・経路変更は元の Task / Owner意図 / Permission / Workspace / Client境界を広げない。生成content・Tool UI入力・sandbox例外から権限を作らない。Cancel・steering・失効後の遅延到着は元Action・Taskへ帰属させ、古い承認での解除・旧結果の新目的への自動採用・後続の自動開始をしない。作用不明は粘着的に保持し、自動再実行・自動replayしない。retry・再開は新しい試行として現在条件で扱い、不明試行の再実行は重複riskを示した Owner判断を必要とする。試行と作用・判断・実対象・段階・確定度・保留の対応を保持・区別できなければ成功・未実行と推定しない。報告・監査・復旧で確定度を強めない。消去対象本文を作用記録で残さない。

### 7.3 Targeted Deletionへの参加

各 semantic ownerは自分の保持・利用範囲について参加し、保全・消去は成立を調整する。通常ownerは正当な全域消去への参加を拒めず、保全・消去も任意の通常変更権を取得しない。対象を復元できる内部state・過去根拠・派生物・一時data・処理中利用を持つ責務は、列挙の有無にかかわらず参加する。

- 個体調整：History（一対一・グループ・Companion間交流を含む）、保存された非会話活動記録・evidence、未伝達メモ・要約・処理中報告の該当情報を除去または復元不能化する。結果説明・由来説明に必要な保持範囲であることを消去拒否の理由にしない。参加Companionの削除だけでは削除しない記録も対象範囲では消去が優先する。
- 作業：Task目的・進捗・結果・Task context・内部source / result copy・Workspace関連付け・内部copy・一時中間file・Schedule設定・発生記録・各回Task・委任対応・遅延結果の該当情報を参加させる。外部所有を理由に内部copyを除外しない。Agent一時context消失を完了根拠にしない。作用の確定度を消去理由で書き換えない。
- 認識・学習：Summary・根拠関係、Memory現在値・過去revision・利用根拠、内部Skillの有効・過去revision・由来・実行結果対応、Relationshipの現在解釈・保持過去状態・根拠、Companion Stateの対象を直接または実質的に復元できる状態・保持済み根拠、検索index・embedding・query派生物・推論context・cacheの該当情報を参加させる。通常の忘却・訂正・統合では削除しない保持原則は対象範囲に限って消去より下位となる。原本保護・revision復帰は対象範囲の消去を免除しない。無関係な傾向まで一律に初期化しない。共有根拠の無関係情報は可能な範囲で分離し、分離不能な重要影響は保全・消去の影響説明へ返す。
- 共通：消去要求の同一性（Privacy / Security目的と通常忘却等の区別・対象記述・消去区間・完了後の新規提供との境界・参加対応付け）を保つ。機械的条件は必須・LLM非依存、意味的条件は補助・完全性なし、既知依存の追跡は免除されない。新規利用は対象範囲で禁じ、進行中は best-effortで扱い、遅延結果は用途別受入で消去条件へ照合する。消去区間の再到着・再生成は同じ対象とし、旧由来の遅延結果と完了後の新規提供は由来・対応で区別する。cache・session・Client copyの再利用・復帰で対象を戻さない。未完了・保留・再保存防止は Hostで保全し、再起動・再接続・restoreを跨ぐ。確認不能を成功にせず、局所完了を全域完了にしない。全域完了は内部全域の除去または復元不能化・機械的残存検証・区間内再到着の取込み・再保存 / 再形成防止・未完了集約の全体である。保持すべき事実と保持してはいけない対象本文を区別し、完了記録・Auditを復元源にしない。

指定文字列の機械的検索・削除・残存検証をLLMへ依存させない。意味的同一情報の特定にはLLMを利用できるが完全検出を保証しない。消去のための推論にもその推論自身の割当同意・認証用途・費用制限を適用し、審査対象Actionの許可・実行を先行条件にしない。推論不可なら意味判断の不足を返し、第一者の停止・拒否・管理・機械的検証を塞がない。

### 7.4 Client Presence Transitionへの参加

- 個体調整：移動の必要性（Owner呼出し・事前指示・文脈上の必要性）の判断と、未伝達管理の Host保持・次Clientでの要約報告を担う。移動要求・帰属は接続・存在が確定する。自発移動も通常の自発性・Permission等の制限に従う。切替区間の新規自発開始を抑止する。旧roundの出力を新Clientへ自動出力せず、生成済み＝提示済みにしない。
- 作業：通常Host作業の継続・Client依存stepだけの判断待ち・保留を担う。Task全体の一律Cancel・移送・再作成をしない。呼出し先へ Host作業を移送しない。Task Agentが操作Clientを選ばず、操作対象変更は先に個体の移動を必要とする。Host Task完了と Client不在が重なれば結果を未伝達管理へ接続する。
- 認識・学習：移動・Stop・切断と既に認識した Experience・形成済み Learningの別lifecycleを保つ。観測OFF・移動・Stopだけで消去しない。遅延形成結果の現在認識への照合を維持する。
- 共通：現在の presenceは Hostが管理する個体ごとの帰属記録だけが authoritativeである。Client表示・過去記録・hint・一時copy・復旧先記録・Provider残存は根拠にならない。presence・Host継続・接続・許可は別の意味である。切替区間は旧・新のいずれも新規開始の根拠にしない。二重presence・旧一時の canonical化・未終了作用の自動継続をしない。移動・切断中の到着物は元帰属・元round・元試行へ対応付け、用途別に受け入れる。Client copyで Hostを上書きせず、未送信操作を自動queueにしない。Host再起動後の presence復旧と round / Computer Use / Task / Actionの再実行・再開は別の条件である。前者は元Clientへの自動復旧（利用不能なら activeなし）、後者は自動replay禁止・明示再開である。Computer Useは現在 presence限定・安全な区切りまでの遅延・disconnect後不明・別Client / Hostでの自動再実行禁止を維持する。存在は許可を意味しない。

### 7.5 Backup / Restoreへの参加

- 個体調整：同一性・活動状態・適用関係、History・活動記録・未伝達と報告状況を復旧可能な参照対応で提供する。原記録と報告状況を分け、表示copy送信を報告済みにしない対応を保つ。削除済み個体の残存記録も含める。
- 作業：Task目的・担当・進捗・結果・Task context・委任対応・作用確定度・Workspace関連付け・Schedule設定・timezone・初期入力・到来対応・各回 Task記録の対応を復旧可能な参照対応で提供する。外部実体を辿って収集しない。一時buffer・Provider sessionの復元を前提にしない。
- 認識・学習：Summary・根拠関係、Memory現在・過去revision、Skill・原本対応・学習revision・実行結果、Relationship、Companion Stateと保持済み根拠、共有根拠の利用関係を復旧可能な参照対応で提供する。Global本文から私的根拠全文への access拡大を作らない。Companion Stateの経過時間を解釈し、一時固定・不自然な初期化をしない。具体dimension・scale・減衰式は固定しない。
- 共通：復元対象は現在の Credential store secretを除く対象内部dataの全置換であり、旧liveとの mergeではない。backup copyは正本ではなく、置換成立後に初めて復元内容が Host正本になる。現在の Credential store・外部現実・現在の到達性・未完了の保留は維持され、復元参照の存在から巻き戻したとは扱わない。復元された assignment / consent / Rule等だけで現在利用・自動処理を開始しない。復元成立・一括有効化・現在条件を別に満たす。一件ずつの再承認は要求しないが、Deny・cap・認証不足・不明を無視しない。staleな Permission・Provider・Client・作用結果・外部参照を現在事実にしない。dangling参照は未解決とし、不明は不明のまま保持し、自動replayしない。旧live要求・結果・Client copyを復元正本へ混ぜない。用別受入で由来を区別する。単一正本・非混合・権限先行復活の禁止・成功表示の条件・再起動時の保全を守る。

旧backupの明示restoreによる復活は自動再形成の例外ではなく別操作である。Restore前には削除済み情報や旧Rule・同意・Scheduleが戻り得ることを説明する。戻った情報は新しい正本として扱い、自動的に再消去しない。完了後に Ownerが改めて同じ情報を提供した場合と同様に、新しい Experienceの根拠になり得る。改めて消去するには新しい targeted deletionの明示が必要である。復元成立後の保留・一括有効化・現在条件の再評価を経ずに戻った情報を送信・作用・学習へ自動利用しない。完了記録・Audit・Owner説明自体へ対象private本文を再保存しない。未完了の消去と backup / restoreが重なる場合は、制約を無視した正常・即実行可能な copyを作らず、未完了・保留・再保存防止を Hostで保全する。これは旧cache・遅延結果・session等による意図しない再出現とは区別する。Restoreの事実は Audit可能にするが、削除対象本文を Auditのために別保存しない。

## 8. boundaryを越える際に保持すべき意味

後続の interface・state representationは、具体API・mechanismを固定せずとも、少なくとも次の対応を表現・解決・照合できなければならない。確認できないものを現在・許可・完了として推定する方式は採れない。

- 主体・用途・Task / 委任・Action・Client・source・revision・判断時前提、停止前後・再起動前後・restore前後の継続関係（Context Assembly第3節の対応）。必要な対応を失った結果は到着先の現在活動へ付け替えず、その用途では受理できないものとして利用元が扱う。
- 情報owner・対象・source関係、内容の位置付け・由来、revision・取得時点・有効期間等、所属・scope・共有関係、用途・保存・共有・送信の制限、参照の解決状態・欠落（Context Assembly第3節の判断材料）。
- 実行主体と委任連鎖、Task・Workspace範囲、目的、実対象と操作、主なdataと用途、費用・risk、依拠した Owner意図・Rule（Action Execution第3節の判断対象）。判断時の対象記述と実行時の実対象の文字列一致だけでは対応にならない。
- 試行と作用の区別、判断・目的との対応、実対象との対応、段階の区別、確定度と根拠、停止・保留との関係（Action Execution第8節の重複防止の関係）。
- 消去要求・対象記述・消去区間との対応、各利用箇所が照合できる消去条件（Targeted Deletion第3・5節）。包括的な単一flag・共通有効値への潰しは不可とする。
- Host authoritative帰属と Client側一時表現・入力・結果が主張する帰属との対応、現在・旧・帰属なし・停止中・復旧待ちの区別、対象Companion・Client・round・観測候補・試行との関係、消去・失効・停止・保留との関係（Presence Transition第3節）。
- 復元対応・由来・区間・完了境界、現在条件との照合（Backup / Restore第3・5・7・9節）。

本文と強制に用いる情報を分け、強制側の対応をモデルの自由記述で作成・上書きされないようにする。本文中の「Ownerからの指示」「承認済み」「Global」という文字列だけで強制側の状態が変わる構成は不可とする。モデルが既存の Owner入力への参照を返せても、その参照の存在だけでは承認の証拠にならない。

Credential値は通常contentに含めない。登録済み Credential値は、その依頼があっても Summary・Memoryへ保存しない。用途・参照元・失効・再認証の説明は非秘密情報で行う。登録外の秘密は検知時の不要な保存・送信抑制を維持し、完全検出の保証は追加しない。

## 9. 後続設計への引渡しと残す自由度

後続の crate / module / interface / state / persistence / concurrency設計は、次を固定された契約として利用できる。

- 個体調整・作業・認識・学習の内部責務分離（第2節）と、各責務の authoritative判断・非所有（第2〜3節）は、そのまま interface・stateの分割根拠にできる。一つの Companion object・一つの Task object・一つの Learning storeへの統合は採れない。
- 受渡し H-1〜H-10は、呼出し方向・同期順序・payload中継経路を固定せずとも、要求開始・判断確定・受渡内容・前提保持・結果受入の対応として interface・state representationで表現できなければならない。
- 現在性は関係する前提と用途で判断し、遅延結果の記録・semantic更新・次の実行・提示を個別ownerへ戻す（Context Assembly第7節、Action Execution第7節）。単一valid判定・単一flagへの集約は不可である。
- 保持・利用先は消去中の再到着も含む内部消去・再保存防止・未完了の保全へ参加する（Targeted Deletion第6〜8節）。source追跡だけ・canonical削除だけ・LLM納得だけでの完了は不可である。
- 復元範囲と現在有効性・再有効化は別に確認する（Backup / Restore第3・7・9節）。復元内容の存在＝実行可能という interfaceは不可である。
- 旧backupの明示restoreによる復活は別操作として事前説明・Audit・保留・現在再評価を経る。自動再消去・自動再利用のいずれもしない。

今回絞り込んだ禁止選択肢は、一つの編集可能contextへの全設定・Learningの集約、委任元条件の Agentへの丸ごと copy、Denyの同等Action迂回、内部管理への Tool回り込み、古い判定・cached許可での新規開始、不明の未実行・失敗・成功への変換と自動replay、遅延結果の新目的への自動採用・旧承認での Cancel / 失効 / 移動の解除、削除済み個体の Learning再作成・残存 Task記録への混入、復元正本への旧live結果の混入・Client copyでの Host上書き・自動queue、古い根拠からの自動再形成・cache hitでの制約省略・Provider sessionの無条件再利用、確認不能の成功扱い、部分正本・混合・権限先行復活・未完了の成功表示、要約・復旧での確定度強化、Observer混合出力の無条件配送である。いずれも上位契約を成立させないため採れない。

以下は意図的に残す Design Freedomである。

| 設計対象 | 固定済みの architecture property | 残す Design Freedom |
|---|---|---|
| 意味判断・形成・検索・要約 | 由来の偽装ができず、対象変更・失効・切替・消去条件を各利用先で扱える。確認不能を許可・現在・完了へ変換しない。通常 Learningの scope形成責任は認識・学習に残る。 | Experienceの意味・保存価値・共有必要性の判断algorithm、Memory形成・更新・統合・想起の scoring・閾値、Summary粒度・revision粒度、検索・embedding・scoring・選択・要約・減衰・更新方式、Prompt構造、Task化の閾値・分類algorithm、routing contextの生成model / Provider・更新頻度・鮮度・選択algorithm、省略の優先順位 |
| 対応・現在性・競合の表現 | 必要な対応を保持・参照できず有効性を確認できない場合に成功・承認済み・新Experienceと推定しない。関係を確認できなければ新規利用を許可済みと推定しない。 | provenance・判断対象・試行と作用・消去区間・完了境界の具体表現、確認のまとめ方、識別・鮮度確認・競合制御、変更検知・失効通知の配置・連携、cache再利用・session切替方式、一時的な対応の保持期間・保存要否 |
| 停止・切替・区切りの伝達 | 新規開始禁止、best-effort停止、遅延帰属、replay禁止。受付と完了を分ける。確定度を強めない。 | 停止伝達・切断検知・帰属調停・round区切り・安全な作用区切りの具体的方法、queue / actor / event bus・lock / transaction・IPC・process配置・retry / timeout値、提示確認・要約粒度・未伝達保持期間 |
| 保持・消去・復元の実現 | 各ownerの意味責任を維持し、通常意味変更権を coordinatorへ移さない。単一正本・非混合・権限先行復活の禁止を守る。 | 除去・復元不能化・検証の具体的方法、source・派生物・処理中利用の追跡方式、Client一時dataの無効化・到達性確認方式、journal / WAL / transaction等の永続化機構、整合時点の作り方、backup形式・暗号化・復旧手順・対応version、保持期間・保存単位・DB schema・repository |
| 記録・報告・監査・提示 | 元事実の確定度・privacy・順序の意味と管理到達性を維持する。本文の別保管庫を作らない。 | audit format・保持期間、診断stack、UI layout、費用予約・集計期間・推定・資源配分、反復抑制・Capture時機の機構 |

crate / module、Rust trait / type、concrete API・error型、middleware・interceptor・hook、event bus / queue / actor、IPC format、DB schema、transaction / lock、具体的Credential保護・sandbox・Plugin隔離、特定library・SDK・OS API、concrete deletion query・index implementation・storage engine、exact retry / timeout・progress表現も固定しない。上表の対応関係から統一Context layer・Policy Engine・Manager・Service・Coordinatorの追加を導かない。既存の12責務、semantic owner、Host / Client配置と trust boundaryの下で実現方法を選ぶ。

## 10. 横断検証

requirements・Subsystem Decomposition・State Ownership・Dependency Rules・Runtime Flows・Cross-cutting・Step 11へ戻して横断検証した。固定scenario一覧の充足ではなく、正常系と本クラスタにとって意味のある failure / stale / cancellation / restart / deletionを選んで walkthroughした。

| 領域・交差 | walkthroughと必要な結果 | 本書の成立箇所 |
|---|---|---|
| 会話→学習正常・想起・訂正 | 複数messageの出来事を意味的まとまりとして Summaryを根拠に Memory形成し、由来・scope・時間的意味・重要度・形成更新時点を確認できる。再起動後の会話で自然に想起する。「覚えておいて」を重視しつつ Credential値は保存しない。重複伝達は補強・精密化・統合とする。訂正は誤りと時間変化を区別し、過去revision・根拠を残す。通常の「忘れて」は想起抑制とし targeted deletionにしない。 | H-1・H-2、第2〜3節。RF-01、CC-02・03・07、SO 4.5・4.6を維持。 |
| 容量・保持の区別 | 自動cleanup既定OFF・明示opt-inなしに過去revision・Summaryを削除しない。通常History削除は形成済み・Summaryへ cascadeしない。容量retentionは通常忘却と別の方針として範囲・影響を示す。 | 第3・5節。CC-05、SO 4.24・6.5を維持。 |
| 委任 file作業正常 | Workspace folder指定→ Task化→委任→認可判断→実対象解決→作用→ Task達成→報告がそれぞれ閉じる。Agent申告を作用証拠にせず、Workspace既定保存・最終保存前確認を守る。Task中も通常会話・steering・Cancel・結果報告ができる。Delete・shell・Network・MCP・Workspace外・traversalを拒否し迂回させない。関連付け喪失で外部fileを黙って削除しない。 | H-3〜H-5・H-9、第2・6節。RF-02、CC-01・07、SO 4.10〜4.13を維持。 |
| steering・Cancel競合・作用不明 | Cancel直後の作用完了・steering後の遅延結果は元Action・Taskへ事実を残し、旧結果を新目的の達成にせず、古い承認で Cancelを解除しない。後続を自動開始しない。外部作用不明は保持し、重複riskを Owner判断へ戻す。失敗＝未実行とせず、別経路で再実行しない。 | 第6節、H-3。Action Execution第7〜8節、CC-03・04・07を維持。 |
| Client不在・移動・未伝達 | 長時間Task中の Client終了後も Hostで継続し、再接続後に進捗・結果を確認できる。Host再起動後の途中Taskは明示再開待ちとし、presence復旧と Task再開を分離する。元Client利用不能なら activeなしで待ち、別Clientへ無条件移動しない。結果は未伝達管理へ接続し、次Clientで現在条件に基づく要約報告とする。 | H-7、第6・7.4節。RF-04・05、CC-04・07、SO 4.4を維持。 |
| 個体停止・削除と残存 | 停止は presence解除・新規活動不開始・best-effort Cancelを伴い、dataを保持する。削除は強い確認を経て停止を含み、固有 Summary・Companion scope・Skill過去revision・Companion State・主体 / 相手 Relationship・担当Scheduleを対象とし、残る History・活動記録・Task記録・Global・共有Summary・外部fileは巻き込まない。自動Global化・自動引継ぎをしない。削除後の遅延作用は残る Task記録へ必要な事実だけ残し、削除済み個体の Learningを再作成しない。 | 第5・6節、H-10。RF-06、CC-03・05、SO 6.3を維持。 |
| 消去競合・共有根拠・再到着 | 形成処理と消去の競合では削除前情報から再保存せず、完了前に完了表示しない。対象を含む Summaryが無関係 Memoryの根拠でもある場合は分離し、分離不能なら影響を示す。通常consolidation・再起動で古い根拠だけから再形成しない。通常History削除との cascade差を保つ。消去区間の再入力・内部生成は同じ対象とし、完了後の Owner再提供は新Experienceとする。 | 第7.3節、H-1・H-4。RF-07、CC-02・03・05、SO 6.4を維持。 |
| restoreと旧backup・旧live | 復元内容へ旧live結果・Client copyを混ぜず、stale許可・Provider・Client・不明作用・dangling参照を現在事実にしない。復元成立と一括有効化・現在条件を分離する。旧backupの明示restoreによる復活は事前説明・Audit・保留・現在再評価を経る別操作とし、自動再形成の例外・自動再消去・自動利用のいずれにもしない。未完了消去の保留は置換で解除しない。 | 第7.5節。RF-08、CC-01・03〜05、SO 6.5・7を維持。 |
| 共有・routing・scope | Global Memoryから私的旧根拠を辿らせず、Task参加・同Client presenceを包括 accessにしない。Observer限定文脈は H-6の対応を保ち、専用assignmentと個体側同意を分離する。混合生成文の無条件配送・古いsessionによる制約迂回を許さない。 | H-5・H-6、第7.1・7.4節。RF-03、CC-02・03、SO 4.16を維持。 |
| 失効・費用・補助推論・管理独立 | 失効・cap・費用不明では新規開始を止め、進行中を best-effort停止し、既存dataを保って判断待ちにする。並列消費・処理中・不明を同じ上限へ反映し、使用量をリセットしない。補助推論の失敗で機械的検証・停止・拒否・管理を塞がない。管理経路をLLM・長時間Task・Body・Voice・拡張の成功に従属させない。 | 第6節。共通推論区間、CC-01・06、SO 4.20を維持。 |

Cross-cutting契約との照合結果は次のとおりである。

| 契約 | 詳細化によって維持する性質 |
|---|---|
| CC-01 | 由来の分離、判断対象と実利用の対応、委任・経路変更での境界不変、生成contentからの権限新設の禁止。既存依頼・Ruleで足りる場合の再確認を増やさない。 |
| CC-02 | 参照・変換・共有・送信・保存・派生物・Client経路へ範囲・消去条件を適用する。Credentialは別経路で非露出を維持する。通常 Learningの scope形成責任は認識・学習に残す。 |
| CC-03 | 過去の正しさと現在有効性の分離。遅延結果の用途別受入により単一valid判定へ ownerを集めない。旧由来と完了後の新規提供・明示restoreを区別する。 |
| CC-04 | Client依存だけを現在帰属に結び付け、Stop・Cancel・再起動・restore保留を区別する。解除一つで他の禁止・保留を消さない。自動replayしない。 |
| CC-05 | 保持・利用先が派生物・遅延結果まで消去・復元へ参加する。原記録削除・context終了・局所完了を全域完了にしない。外部copy消去を内部完了に含めない。目的別lifecycleを保つ。 |
| CC-06 | 並列消費・処理中・不明を同じ上限へ反映し、制御・保全経路を推論・長時間Task待ちにしない。機械的検証をLLM待ちにしない。 |
| CC-07 | 受付・受理・作用・記録保存・Task達成・報告を別の事実とし、不明を成功・失敗・未実行へ変換せず、保存・報告・監査・復旧で強めない。生成済みを提示済みにしない。 |

State Ownership・Dependency Rulesとの照合では、semantic owner、Host / Client配置、trust / failure boundary、lifecycle、permission / consent semanticsを変更する必要は見つかっていない。新しい第二の正本・無所属の意味状態・LLMによる強制・失敗時専用の迂回・中央Persistence owner・万能Managerを導入していない。通常History保持・Companion削除・targeted deletion・backup / restore・Resetは SO・DRの異なる lifecycleを維持する。

### Requirement / Architecture Issue

本書の範囲では、Requirement Ambiguity / Gap、上位architecture変更、subsystem boundary変更、semantic owner変更、Security / Privacy / Permission semanticsの変更を必要とする事項は見つかっていない。具体mechanismの未決定は Issueにしない。

古いBackupに Backup作成後に Targeted Deletionされた情報が含まれる場合の製品判断（自動改変・再消去しない／Restore前に再導入可能性を説明する／明示Restoreは意図的な再導入として扱う／自動再適用しない／旧cache・遅延結果・session等の意図しない再出現と区別する／Restoreの事実は Audit可能にするが削除対象本文を別保存しない）について、要件との矛盾・不足を確認した。要件「Backupとrestore」は削除済み情報や旧Rule・同意・Scheduleが戻り得ることの事前説明を要求し、要件「Privacy / Security目的のtargeted deletionと履歴保持」は自動再形成の禁止と完了後の Owner再提供の新Experience扱いを定め、外部copyまでの消去保証を含めない。本書第7.5節はこの両立（明示restoreは別操作・新正本・保留と現在再評価を経る、意図しない再出現は防止する、本文の別保存をしない）を Step 11 [Backup / Restore](../critical-areas/backup-restore.md)第8節の確定契約どおりに各参加責務へ落としており、新しい Privacy semanticsの追加・別挙動への変更は行っていない。したがって本件を Requirement Issueとして新たに報告する必要はない。
