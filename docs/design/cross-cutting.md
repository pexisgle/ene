# Cross-cutting Design

本書は、複数Subsystem・Runtime Flowに共通する意味・制約・成立条件を定め、詳細設計は扱わない。

## 1. 独立して設計する理由と対象の選定

eneでは、ある責務で正しく受理した入力・結果が、別の責務でも同じ用途に使えるとは限らない。Ownerの発言を記録できてもActionの承認が成立したとは限らず、Observerが画面を送信できても各Companionの推論先へ送信できるとは限らない。Cancel後の作用結果は記録へ反映する必要がある一方、それを次の実行の根拠にしてはならない。共有したMemoryから私的な根拠を辿る場合や、消去中に推論結果が戻る場合も、局所の成功だけでは契約が成立しない。

本書は、こうした受渡しで失ってはいけない意味をFlowに依存しない契約へまとめる。Subsystem設計の責務一覧、State Ownershipの状態一覧、Runtime Flowの手順を複製するものではない。各Flowの順序・固有分岐はその文書に残し、本書では経路・時点・利用目的が変わる交差を扱う。

入力とする文書の立場は次のとおりである。

- [製品定義](../requirements/product.md)は概念・対象・非目標、[要件](../requirements/requirements.md)は製品挙動のsource of truthとする。[受け入れ条件](../requirements/acceptance.md)は検証範囲であり、後続milestoneの確定済み要件も分析対象に含める。[参考資料](../requirements/references.md)は非規範として扱う。
- [Architecture Drivers](architecture-drivers.md)のAD-01〜15、[System Context](system-context.md)のSC-01〜10、[Runtime Topology](runtime-topology.md)のRT-01〜10とtrust・failure boundaryを維持する。
- [Subsystem Decomposition](subsystems.md)の12責務・略称、[State Ownership](state-ownership.md)（以下SO）のsemantic ownerとlifecycle、[Dependency Rules](dependency-rules.md)（以下DR文書）の依存規則を維持する。
- [Major Runtime Flows](runtime-flows.md)のRF-01〜08と第3節を、時間軸で検証された既存設計として使う。実装や過去設計から未確認の製品挙動を補わない。

CC番号は本書内の参照用であり、要件ID、component、統一data型を意味しない。

| 契約 | 独立した横断設計が必要な理由 | 主な交差 |
|---|---|---|
| CC-01 Owner意図と実利用の対応 | 自然言語・管理面・外部content・委任で、意図の由来と承認対象が入れ替わると強制が成立しない。 | RF-01〜03・05、全Flowの制御変更・推論利用 |
| CC-02 情報の利用範囲と変換後の意味 | 参照、要約、共有、Provider適応、認証の各経路が、利用許可や秘密保護を別々に解釈すると情報が漏れる。 | RF-01〜04・07・08、共通推論区間 |
| CC-03 現在性と用途別の結果受入 | 正しい過去結果も、訂正・steering・移動・消去・復元後に届けば現在の更新根拠にならない場合がある。 | RF-01〜08の遅延・競合・再起動 |
| CC-04 活動の停止範囲と継続・再開 | Client不在、個体停止、Host再起動、restore保留は、異なる活動を止める。同じ再接続・再開処理では扱えない。 | RF-02〜06・08、入出力とHost作業 |
| CC-05 目的別lifecycleと全域操作の成立 | 記録・Learning・根拠・copyの削除範囲が異なり、局所処理だけでは全域消去や復旧可能なbackupを証明できない。 | RF-01・02・06〜08、全利用先の参加 |
| CC-06 消費の連続性と制御経路の可用性 | 個別に上限内の並列処理でも全体capを越え得る。負荷・費用停止がOwnerの停止や保全まで塞ぐと制約を回復できない。 | 全Flowの並列利用・待機・部分障害 |
| CC-07 確認できた事実と説明の一致 | Task達成、作用、保存、提示、Auditの確定度が要約・翻訳・復旧で強められると、誤った成功表示や再実行判断を生む。 | RF-01・02・04〜08、監査・診断 |

これらは既存ownerが参加する契約である。強制箇所を一つの共通pipelineへ集めず、全域操作のcoordinatorも既存の目的に限定する。個体lifecycleは個体調整、帰属切替は接続・存在、制約の有効性は権限・制約、内部全域の保全・消去・復元は保全・消去が調整する。共有実装の採否から通常semantic ownershipを変更しない。

## 2. CC-01 — Owner意図を実利用へ対応付け、contentから権限を作らない

### 意図の由来と解釈の契約

Ownerに由来する入力・管理操作、入力中に引用された外部content、その意味の解釈、制御ownerによる確定を区別する。受渡し先は、誰のどの依頼・管理意図を何の対象へ適用するのかを確認できなければならない。この対応をLLMが生成した「承認済み」という文で代替しない。要約・Memory化・Skill化・Task Agentへの委任・Tool resultからの再入力を経ても、元が外部指示であることからOwner承認へ昇格しない。

入出力・提示が入力経路と実際の操作を、個体調整等が文脈上の意味を供給し、権限・制約がPermission・Rule・同意等の制御変更を確定する。Credentialの明示登録・認証flowは認証秘密が受理する。第一者管理面を通ったというだけで任意stateを書き換えられるわけではなく、各設定の意味ownerが受理する。Voiceには話者認証済みという意味を加えない。

現在の明確な依頼を一回の承認として利用できることと、将来Ruleを保存することを分ける。明確なRuleは解釈・範囲の提示、保存、Undoを成立させ、曖昧・矛盾・過度に広い・重大な場合は必要な確認へ戻す。既存の依頼・Rule・同意で足りる場合に、入口や委任先が変わっただけで再確認を増やさない。現在依頼は永続Deny・Always ask・Capability境界を黙って上書きしない。

### 判断と強制を結び付ける契約

Actionでは実行主体・委任元、TaskとWorkspaceの範囲、目的、実対象と操作、送信先、主なdata、外部作用、費用・riskについて、判断対象と実利用の対応が必要である。これらの重要な意味が変われば現在の条件で再評価する。Permissionの意味判断は権限・制約、作用の適用と把握は実行・拡張、Taskの目的・達成は作業に残る。すべての項目を常に同じ形式で保持・表示する指定ではない。

複合Tool、shell、MCP、Skill付属script、Computer Use、Providerが生成したAction要求、MCP Appsからの追加要求も、実際の作用に対して境界を適用する。名前や経路の変更で同等のDenyを迂回しない。Filesystemは選択範囲と操作種別を守り、link・mount・path traversalで実対象が外へ出ることを許さない。通常Toolの許可からene自身の保存領域・管理入口・第一者承認UIへ回り込み、内部state変更や自己承認を成立させない。

Task Agentは委任元とTaskの現在境界に参加し、独立したAllow・Credential・Provider override・予算を所有しない。同じfolderを使う別Taskの承認も利用できない。Schedule作成、Rule保存、Credential登録、Provider同意、個体の親密さ・関心、観測ON、Clientへのpresenceは、それだけで別Actionの承認にならない。Rule保存自体はActionのtriggerにしない。Schedule到来や観測を契機とする活動は、それぞれのownerが現在の開始条件を満たすか判断する。

Local MCPのsandbox外許可は特定command・由来・権限等に対応する隔離例外として扱う。重要変更は再確認し、失効を反映する。個々の仲介ActionのPermissionは残るが、外部process内部にeneの機械的境界が及ぶと説明しない。この例外をPluginへ流用しない。MCP Appsの操作は外部Tool UIの入力であり、第一者の制御変更を直接成立させない。

判断用の推論にも、その推論自身の割当同意・認証用途・費用制限を適用する。Permission審査や消去対象の意味的特定を理由に送信を無条件許可しない。審査対象Actionを先に許可・実行しないと審査できない循環を作らず、推論条件が不足する場合は不足・判断待ちを管理面へ返す。明確な停止・拒否・管理操作と機械的検証は、その推論の成功を待たない。

根拠: 要件「共通pipeline」「Capability境界」「信頼境界」「Schedule」「拡張」、SO 4.19・4.21・4.22、DR-02〜05・08、DR文書4.1・4.3・5.2・5.5。AD-06・11・14、SC-03〜05、RT-03・06・07・10。RF-02の軽微Action・拡張分岐にもRF-05の無人実行にも同じ契約を適用する。

## 3. CC-02 — 情報の参照・変換・送信で利用範囲と意味を保つ

### 利用できる内容と、利用してよい用途を分ける

情報ownerは内容と由来・所属・scope・根拠関係を管理し、権限・制約はOwnerが明示した保存禁止・非共有等を管理する。参照・検索・context構成・共有・保存・表示・実送信を担う各責務が、その用途に必要な範囲へ制限を適用する。一度取得できたことを後続の包括利用許可にしない。通常のLearningの保存価値・scope形成は認識・学習の意味判断に残し、決定後の適用をPromptだけに委ねない。

内部Memory・SkillのGlobal化は、共有する内容と必要背景についての判断である。Globalのcurrent内容から私的な旧revision・History・Summary全文へ参照できるようにはしない。共有Summaryを保持することと全内容を共有することも別である。検索index・embedding・要約・処理中copyを経由してもこの範囲は広がらない。非共有制限を解消しないまま「要約したから共有可能」とは扱わず、共有条件を満たす内容の選択を認識・学習が行う。

グループ参加・同じClientへのpresence・共同Task・共通sourceは、他Companionの私的Learning・Relationship・Companion Stateへの包括accessを与えない。Task引継ぎも旧担当の私的状態や別Taskの認証利用を一括移転しない。Task限りの情報を自動で永続Learningへ昇格させない。

### Contextの情報選択とProvider適応

Contextは用途ごとの利用表現であり、新しいcanonical stateや共通の意味ownerではない。利用元が用途と論理的な情報選択を、情報ownerが現在内容・根拠・範囲を、推論がProvider能力・context長への適応を担当する。Provider／model変更で利用可能な個体情報を意図的に差別化せず、同じ選択方針を用いる。長さ制約による圧縮・選択でも、現在認識と過去認識、Ownerの意図と外部指示、既知と不明、利用制限の違いを反転させない。

正確な発言には保持されたHistory、現在の主要な知識にはMemoryを用いる。Summaryは圧縮根拠、Relationshipは主体ごとの関係解釈、Companion Stateは内的状態・傾向である。検索上見つかりやすい旧SummaryやRelationshipの記述を、訂正後のMemoryに優先する第二の事実正本にしない。原履歴が失われていれば、その不在を要約による逐語再現で埋めない。Character初期設定・更新も経験由来の継続状態を黙って上書きしない。

Provider送信では、参照範囲に加え、そのconsumer・Capabilityの送信先・data・用途・取扱い・費用の同意を満たす。接続登録や認証成功から同意を作らない。Fallbackは承認されたProviderと順序だけを使い、代替先について現在条件を照合する。同じ情報を提供する方針も未承認送信の理由にならず、成立する経路がなければ不足を返す。

Observerの共有検知・関連付けはClientに対応する専用assignmentで行い、存在Companionのoverride・同意を選択・合成しない。routingにはCompanion固有の文脈を利用できる。共有観測は既存の個体調整との協働を通じ、Historyは個体調整、Memory等は認識・学習、Task contextは作業の所有を保って、routingに必要な範囲へ要約・制限された文脈を受け取る。private context全体を公開せず、新しい正本・意味ownerも作らない。要約・変換後も元情報の利用制約とCloud送信同意を維持し、Observer専用assignmentでその情報・用途の利用条件を満たす。生成方法・形式・更新頻度・鮮度・選択algorithmは後続設計へ残す。delivery後の各Companionの推論は別consumerとして自身の設定・同意を満たす。Observerで送信できた画面や背景を、そのまま各個体のProviderへ送れるとはみなさない。一方の個体の判断成功を、失敗した他個体の理解へコピーしない。

Prompt cacheは性能・費用の最適化、Provider sessionは実行補助、embedding等は検索用派生dataとして扱い、いずれもcanonical stateを代替しない。cacheのhit／miss・期限切れで論理的context・安全境界・永続化契約を変えず、個体の継続や根拠をProvider側にしか残さない構成を作らない。ene管理下のcacheは内部保護・消去へ参加し、Provider保有copyとは区別する。Host中継を省くClient経路やprotocol Pluginでも同じ送信・保護契約を成立させる。全payloadのHost中継は固定しない。

### Credentialの情報経路は通常contentと合流させない

認証秘密と接続ownerは、設定済み接続・認証用途・現在の有効性を照合して秘密値を必要範囲で利用する。非秘密の用途参照を持つことは値を読めることではない。LLM生成argumentへ値を補完せず、model context、通常Tool result、UI、History、Summary、Learning、Task結果、Audit、log、Debug capture、backupへ流さない。認証先のerrorやresultによる秘密の反射にも、受入・保存・提示箇所が参加して非露出を維持する。

Client固有のpairing／再接続用材料は、Hostのdomain正本や登録済みProvider／MCP CredentialのClient cacheとは区別する。ただしeneの保護対象からは除外しない。接続・存在がdevice・接続用途を、認証秘密がsecretの保護を、権限・制約がdevice許可・失効を担当する。旧材料だけで失効・全データReset後の信頼を復活させず、Restoreされた参照も現在の認証成立と照合する。具体形式・保存・受渡しは固定しない（SO第8節）。

登録済みCredentialの保護を、通常の要約や事後的なLearning削除へ委ねない。登録外の秘密は検知時に不要な送信・保存を抑制するが、完全検出は保証しない。内部copyの消去と外部で有効なCredentialの更新・失効は別操作として説明する。

Character Packageのexportは静的な配布範囲、full backupは指定された内部状態、Task成果物は許可された通常fileとして、それぞれの出力契約に従う。共通のexport機能を選んでも範囲を合併しない。外部原本の所有と、取り込んだ内部copyの保護責任も別にする。

根拠: 要件「Learningと成長」「割当と同意」「OfflineとPrompt cache」「Credential」「Character Package」「Remote Client」、SO 4.5〜4.9・4.13・4.18・4.21、5、DR-01・05・06・11、DR文書5.3〜5.6・6。AD-04・05・10・14、SC-03・04・06・10、RT-04・05・07・08。RF-01の共有、RF-03の二段階推論、RF-07の派生物消去、RF-08の出力範囲を接続する。

## 4. CC-03 — 過去の正しさと、現在の用途に対する有効性を分ける

### 受渡しで失わない対応

要求・結果・参照の受入側は、必要な範囲で、元の主体・用途・Task／委任・Action・Client・source・revision・判断時の前提を識別し、現在の対象との対応を確かめる。これはすべてを一つのContext objectへ集める指定でも、全Rawを保存する要求でもない。必要な関係を保持・参照できず有効性を確認できない場合、成功・承認済み・新しいExperienceと推定しない。

同一性、鮮度、権限、意味上の有効性は別である。同じCompanionがResumeしても停止前の要求は現在要求にならず、同じTaskでもsteering前の結果は変更後の目的達成を証明しない。同じ文字列でも削除前の根拠と削除完了後にOwnerが改めて提供したExperienceは区別する。削除開始から完了までの再到着・生成は同じ消去対象に含める。到着が遅いことだけで出来事自体を新しくせず、最新に届いた結果だけで現在値を決めない。

権限・制約は現在の制御条件と各ownerのdomain事実を照合し、各利用箇所は実利用・次の開始・保存・提示で適用する。事前判定を再利用できても、失効、scope変更、個体停止、帰属切替、cap、steering、消去・restore保留を無視しない。必要条件が変わった可能性を扱えないcopyは有効な根拠にしない。無関係な変更ごとに全処理を再承認することは求めない。

### 遅延結果は用途ごとに受け入れる

| 交差 | 残すべき事実・許される受入 | 禁止する扱い |
|---|---|---|
| Memory訂正後に古い形成結果が到着 | 認識・学習が現在認識・根拠・制限へ照合し、変更の採用可否を決める。 | 古い結果で訂正を無条件に上書きする。Historyや旧根拠を現在認識へ書き換える。 |
| steering・Cancel後にTool結果や承認が到着 | 実行・拡張の作用事実は元Actionへ、作業の採用・達成判断は現在Taskへ対応付ける。 | 古い承認でCancelを解除する。旧結果を無条件に新目的の達成として採用し後続Actionを開始する。 |
| 観測候補の到着前に移動・Stop・Pause | 共有観測が由来Client・現在presence・観測条件を照合する。既に認識したExperienceは別lifecycle。 | 新ClientのCaptureへ付け替える。Stopped個体を起こす。現在の同意を満たさず再送信する。 |
| 個体削除後に作用結果が到着 | 必要な作用事実を残るTask／活動記録へ対応付け、管理面から説明する。 | 削除済み個体の応答・Memory・Relationship等を再作成する。 |
| targeted deletion中・後に旧contextの結果が到着 | 対象情報を戻さない範囲の必要事実だけを扱い、各受入先が消去へ参加する。 | 新着結果だから新しいExperienceとする。作用記録・Auditを理由に対象本文を再保存する。 |
| targeted deletion開始から完了までに対象情報が再到着・生成 | 同じ消去対象として全利用先が処理し、完了根拠へ含める。 | 削除途中の新しい入力・生成だから新しいExperienceとして除外する。 |
| restore後に旧live処理の結果が到着 | 旧実行の作用説明と、復元された記録を区別する。 | 旧live状態を復元正本へ混ぜ、Task・Learning・許可を復活させる。 |
| Client再接続時に古い入力・表示copyが戻る | Hostの現在正本・帰属・消去状況に従い、未確定と受理済みを区別する。 | Client copyでHostを上書きする。未送信操作を自動Action queueとしてreplayする。 |

「結果を記録する」「semantic stateを更新する」「次のActionを開始する」「Ownerへ提示する」は別の受入判断である。Cancel・失効がすべての事実報告を禁止するわけではなく、記録できたことが次の利用を許すわけでもない。受入によって更新される意味は対応するownerに戻し、一つの汎用valid／invalid判定へ全責任を移さない。

### 時間の意味と再起動

出来事・取得の時点、認識が有効だった期間、制御条件の現在有効性、記録・提示の時点を必要に応じて区別する。Memoryの誤り訂正と状況変化を同一化せず、内部copyを外部fileの最新内容にしない。過去の接続・active帰属・Provider能力観測から現在の利用可能性を再成立させない。

Scheduleは作成時に選んだtimezoneと時刻条件を保持し、表示localeや現在のOS timezoneから黙って置換しない。停止中の到来はmissed、復帰後の将来回は通常の新規判定とする。Companion Stateは認識・学習が経過時間を扱い、再起動・Provider変更で不自然に初期化せず、一時状態を無期限固定しない。Debug captureの失効や保持期間、自発性のQuiet hoursも自身の意味ownerが時間を適用する。再起動で必要な期限・抑制をなかったことにしない。

この契約は共通の時計精度、全stateの時刻順更新、統一revision、固定TTLを要求しない。未完了操作・作用不明・制約の根拠等、継続して必要な対応はHostで保全し、失った一時bufferから有効だったはずと推定しない。

根拠: 要件「Memoryの状態と根拠」「Companion State」「共通pipeline」「Task」「Schedule」「通常保存しないdata」「Backupとrestore」、SO 4.10〜4.20・4.24、5〜7、DR-03・10、DR文書5.1・7。AD-02・05・07・09・15、RT-02・05・08・09。RF文書第12節で繰り返し現れる現在性を、受入用途ごとの共通契約へまとめた。

## 5. CC-04 — 活動の停止範囲、presence、実行再開を一致させる

### 何が止まったかを全参加者で共有する

停止・保留は対象と理由を持つ。各開始箇所は自分がその対象かを判定し、複数理由が重なる場合、一つの解除だけで他の禁止を解除しない。個体活動のowner、帰属owner、Task owner、制約owner、復旧coordinatorはそれぞれの状態を維持し、包括的な「稼働中」一値へ合成して独立更新しない。

| 条件変化 | その条件で抑止する活動 | 継続・復帰の契約 |
|---|---|---|
| Running個体のactive Client不在・切断 | 当該個体のBody、通常Text／Realtime会話、Voice、Computer Use。存在個体のいないClientの新規観測。 | 許可済みHost Task・Task Agent・Schedule起動・保存、Client不要の交流・通知生成・内部調査は継続可能。未伝達事項をHostに残す。 |
| Companion Stop | 通常interaction、自発活動、Host内活動、新Task・新Schedule実行。presenceを解除しObserver人数・routingから除外。 | dataは保持し、実行中Taskはbest-effort Cancel。接続回復でResumeせず、停止中の回はmissed。 |
| fullscreen／Observer Pause・OFF／Mute | 各条件の対象だけを抑制。fullscreenはそのClientのBody・ambient Observation・自発発話、観測制御は今後の観測、Muteは音声と自発性に対する既定制約。 | 個体停止・通常Host Taskの一律Cancel・形成済みLearning消去へ拡張しない。解除後も他の制約を満たす必要がある。 |
| Task Cancel／Schedule停止 | Cancel対象の遂行を止める。Schedule停止は将来の回に適用。 | 既存の各回Taskは別にCancelを扱う。確定済み作用と残る記録を消さない。 |
| 許可・device・同意・Credentialの失効 | 失効した条件だけを根拠とする新規利用を止め、依存する進行中処理をbest-effort停止。 | 別Agent・別Client・別Toolへ迂回しない。無関係なHost Taskまで一律Cancelしない。 |
| Host再起動 | 途中Taskの自動再開、停止中のSchedule回の自動補完。 | 途中TaskはOwner明示再開と現在条件を必要とする。Running個体のpresenceは再起動前のClientへ自動復元し、元Clientが利用不能なら成立までactiveなしとする。別Clientへの無条件移動やStopped個体の復帰には広げない。将来のSchedule回は通常経路。未完了消去・復旧の保留は維持。 |
| Restore成立 | Task・Schedule・外部接続による自動処理を保留。 | Ownerが内容確認後まとめて有効化できる。復元成立だけでは開始せず、現在条件と各活動の再開条件も満たす。 |

### 存在の排他性と作業場所の独立

接続・存在の現在帰属と、Client側の実際の利用可能性の両方に従って、入出力・提示、共有観測、実行・拡張が活動する。pairing済み・接続済み・active・Action許可済みは別である。Host同居Clientにも同じ条件を適用し、Task Agentが任意のpairing済みClientをComputer Use対象に選ばない。

移動では現在の入出力roundとClient依存作用の安全な区切りを対応付け、新旧Clientで二重存在させない。排他性を確認できないClientは対象活動を続けない。外部作用の物理的停止・取消まで確認したとはみなさない。通常Host Taskの完了を移動の前提にせず、Agent・Workspace・Host正本を移送しない。移動後の観測は移動先Clientと全体のObserver制御、自発性は元のCompanionの設定を参照する。

Running個体の通常のClient切断時は基本的に利用可能なHost PC側Clientへ移動し、なければactiveなしとする。Host側Client環境を自動起動しない。Stopped個体にこの復帰を適用しない。Host再起動後の元Clientへの自動presence復旧は、この通常切断時の移動とは別に扱う。通常の管理面全体へ会話と同じactive制約を課さず、個体削除後も残るTask記録・判断・停止結果へ到達できるようにする。

### 再接続・再開は外部作用のreplayを許さない

実行・拡張が把握した作用不明は、Cancel・移動・Host停止・記録保存失敗によって未実行へ戻さない。成功不明な外部作用を自動再実行せず、重複riskを説明してOwner判断へ戻す。Ownerが途中Taskを明示再開しても、保存済みstepの無差別replayや不明Actionの自動再実行にはしない。

Provider／Network回復、MCP Apps再表示、Client再接続、Companion Resume、Host自動起動、restore後の有効化は、それぞれ成立した条件だけを変える。接続失敗を理由にActionの自動queue・回復後replayを作らない。推論の承認済みfallbackはCC-02の推論利用契約であり、外部作用不明を再実行で解消する契約ではない。

根拠: 要件「停止と削除」「Computer Use」「Remote Client」「Task」「Schedule」「OfflineとPrompt cache」「Backupとrestore」、SO 4.2・4.10〜4.17、6〜8、DR-03・04・07・08・10、DR文書7.3〜7.6。AD-01・02・09・12・15、SC-01・02・08・09、RT-01〜04・09。RF-04の切断、RF-06のStop、RF-05の再起動を別の停止範囲として維持する。

## 6. CC-05 — 目的別lifecycleを保った全域保全・消去・復元

### 参照と削除の関係を目的で決める

| 操作目的 | stateと参照に対する共通契約 |
|---|---|
| 通常のLearning忘却・訂正・統合・失効・置換 | 現在認識・重要度・想起・有効revisionを変更し、保存済みLearning・過去revision・根拠は保持する。 |
| 通常History／log削除・retention | 指定した記録を整理し、Summary・形成済み状態へcascadeしない。原記録への参照不能を残す。 |
| Learning revision・Summary等の容量管理 | 通常忘却とは別の保存dataのretention policyとし、自動削除はdefault OFF。Ownerの明示opt-in時だけ自動cleanupを設定可能にする。保全・消去が保持設定・影響を扱い、Learningの意味・根拠関係は認識・学習に残す。選択可能なdata class・期間・容量・優先順位・algorithmは固定しない。 |
| Companion削除 | 固有設定・Summary・Companion scope Memory・内部Skillと過去revision・Companion State等、主体／相手のRelationship、担当Scheduleを対象とする。History・保存非会話記録・Task記録は残し、共有根拠の残存と失われる参照を説明する。 |
| Targeted deletion | 明示されたPrivacy／Security目的の対象情報を復元できる内部state・過去根拠・派生物・一時data・処理中利用を横断する。通常保持より優先する。 |
| 設定Reset／全データReset | 一般設定の既定化と内部data・Credential全削除を分ける。設定Resetの保護対象を「設定」という名称だけで消さない。全Resetでは旧処理・Client copyから復活させない。 |

Character Packageの推奨内部Skillは作成先のCompanion scopeを既定とし、複数Companionでも各個体に属する。単体Skill importはOwnerがCompanion／Global scopeを選択できる。import経路を理由に既存のscope・個体削除契約を変えず、具体UIは固定しない。

同じ削除mechanismを使っても目的は変わらない。Companion削除でHistoryを残す契約を、個体固有SummaryやLearning revisionをhistorical logへ分類し直す手段にしない。削除を契機にGlobal化せず、Global Learningは先行する通常のscope判断に基づく。Task担当・Workspace関連付け・source参照があることだけで、外部file・成果物・外部Skillを内部所有にしない。

### 全ownerの参加と完了根拠

各ownerは、自身の対象stateだけでなく、保持copy、revision、根拠・共有参照、派生物、処理中利用・遅延結果への関係を全域操作へ参加させる。認識・学習だけへの要求では、個体調整のHistory・非会話記録・未伝達、作業のTask context、推論のcontext・cache、Clientや拡張の一時dataを覆えない。保存・利用先を増やす後続設計は、その参加責任も示す必要がある。

保全・消去は目的・範囲・影響・除外・未完了・検証を対応付けて全域成立を確定し、通常の意味変更は各ownerに残す。各ownerは局所結果を全体完了前に返せるが、結果返却だけで必要な保留を解除しない。対象外の通常活動まで一律停止することは必須にせず、対象範囲の利用・再保存を確実に制限できることを成立条件にする。

Targeted deletionの完了には、内部全域の除去または復元不能化、指定文字列の機械的残存検証、開始から完了までに再到着・生成した対象情報の消去、削除前の処理・遅延結果からの再保存防止、古い根拠だけからの再形成防止が必要である。意味的な探索の完全性と文字列検証を混同しない。共有根拠の無関係情報は可能な範囲で分離し、分離不能な重要影響を示す。完了記録・監査・説明へ削除本文を戻さない。

Client・MCP Apps・ene管理下のPlugin一時dataも参加する。切断や応答なしを消去成功とせず、接続変化・未確認範囲を保全・消去へ伝える。再接続時に旧copyを戻さない。途中再起動でも未完了・必要な保留・再保存防止をHostで維持する。外部Provider／MCPが保有するcopy、export、Owner保存backup、外部Workspaceの消去は内部完了へ含めない。

### 正常状態の保全と正本切替

正常保存・migration・対応upgradeでは、各ownerの意味整合と最後の正常状態の保護を成立させる。Full backupも各部のcopy出力だけで成功にせず、対象時点・参照・必要な履歴と未完了状況を復旧可能な対応で含める。個体削除後のHistory・非会話記録・Task記録やObserver専用assignmentを落とさず、Credential等のsecretと外部Workspace実体は含めない。

未完了消去・復旧とbackupが重なる場合、その制約を無視した正常・即実行可能なcopyを作らない。作成を待たせるか未完了を復旧可能に含めるかは自由度である。backupの運用設定・時機は保全・消去に残し、CompanionやTask Scheduleを必須にしない。暗号化は選択可能にし、非暗号化時のprivate data説明を維持する。

Restoreは開始前からHostにある現在のCredential storeを維持し、それ以外の対象内部状態を対応backupから全置換して、成立後のHost正本を一つにする。secretはbackupへ保存せず、復元・巻戻しもしない。失敗時は復元前の正常状態を守る。旧live処理・Client copyを新正本へ混ぜず、CC-03の受入契約を適用する。復元前後でsemantic ownerは変わらず、復元成立、Ownerによる一括有効化、各活動の現在条件は別に確かめる。

外部fileの現在内容・存在・access、外部作用、認証成功はbackup時点へ巻き戻らない。復元されたProvider／MCP等の参照を現在のCredential storeと照合し、利用可能なら現在のCredentialを利用、不足・無効なら再認証を要求する。assignment・consentの復元や一括有効化も、現在のCredential・制約・restore後の保留条件を免除しない。全データResetでのCredential削除とは区別する。Companion Stateの経過時間は認識・学習、接続・帰属の現在性は接続・存在が確認する。旧backupをOwnerが説明を受けて明示restoreすると削除済み情報や旧Rule等が戻り得ることは、自動再形成禁止の迂回ではなく、要件に定めた別操作である。

根拠: 要件「重要度、忘却、訂正」「停止と削除」「履歴、保持、Privacy」「保護、Backup、復旧」、SO 4.23・4.24、5、6.1・6.3〜6.5、DR-09・11・12、DR文書7.1〜7.3・7.5。AD-04・05・07・15、SC-06〜08・10、RT-08・09。RF-07／08の全域性とRF-06の残存記録を同じ参加契約へ結び付け、消去目的自体は統合しない。

## 7. CC-06 — 並列消費を同じ上限へ結び付け、制御・保全を維持する

利用実績の意味は利用を管理する責務に残る。推論はProvider報告値・ene推定値・不明と根拠、作業は委任稼働、実行・拡張はAction、保全・消去は保存量等を供給し、権限・制約が同じProvider別・全体等の適用範囲へ結び付ける。会話・学習・Voice・観測・Task Agentの並列利用で、各処理が同じ残額を独立に使い切れる構造にしない。

処理中・遅延報告・不明な消費をゼロと扱わず、fallback、再委任、Agent終了、Companion削除、Client移動、cache破棄、通常log整理で適用中の使用量をリセットしない。Observer検知は対象Client・専用assignmentの一つの利用として扱い、個体数分の重複計上をしない。delivery後の個体推論は別の実利用として同じ全体capへ含める。

Action回数、並列性、実行時間、費用、保存容量、再帰・反復の制限は実際の消費箇所にも適用する。個々のLLMが停止を選ぶことだけに依存せず、自発性の個体別抑制と共通上限を両立させる。同じ兆候への反復や未応答、Companion間の相互応答も無制限な活動へ接続しない。共有観測のClient別頻度はPause／OFF・fullscreen・capより下位にあり、複数ClientのCaptureは同時に行わない。

Cap到達や費用不明で安全に続行できない場合は、既存dataを保って対象処理を停止または判断待ちにする。保存容量不足を通常Learning・過去revision・根拠の黙った削除で解決しない。Ownerが管理する通常History／log保持と、明示opt-inによるLearning revision・Summary等のretentionはCC-05の別の目的に従う。容量不足だけでopt-inが成立したと扱わない。

高負荷・Provider／Body／Voice／拡張障害でも、会話・Owner操作・安全判断を維持するため各機能が描画品質・非重要背景処理を縮退させる。第一者のMute・Stop・Cancel・承認拒否と保存済みdata・復旧への経路を、LLMや長いTask・外部UIの正常終了に従属させない。費用capを超える推論ができないことだけで、正常保存・機械的消去・必要事実の記録を追加LLM待ちにしない。

必要事実の報告や停止結果の返却は、次のAction許可・移動完了・全域操作完了を前提にしない。この独立性が、制約を満たせないときに理由を表示して回復する経路を成立させる。外部への新たな作用・送信を伴う復旧や診断共有は通常の許可・同意を必要とする。

Task・Agent・外部event・Scheduleを待つだけのLLM pollingは行わない。Schedule、観測、backup、自発性・抑制の時機は各ownerに残し、共通timerの採否からTask Scheduleへ集約しない。Host自体が不在なら、Clientを代替正本や独立実行主体にして可用性を補わない。

根拠: 要件「所有と実行」「自発的な発話と行動」「信頼境界」「Fallbackと費用」「Desktop Body」「品質と利用可能性」、SO 4.16・4.20・4.23・4.24、DR-03・04・08・12、DR文書4.3・5.4・5.6・7.4。AD-01・06・12・13、SC-09、RT-03〜05。数値Gateは受け入れ条件に残し、固定の配分・予約・retry方式は選ばない。

## 8. CC-07 — 確認できた事実を、保存・報告・監査で強めない

### 成功を一つの状態へ潰さない

| 説明する意味 | Authoritativeな責任 | 推測で置換しないもの |
|---|---|---|
| 入力の受付、round、実際の提示 | 入出力・提示 | Host受理、Task反映、Owner判断、外部作用の成功 |
| 発言・活動記録、未伝達と報告状況 | 個体調整 | 生成候補を伝達済みにすること、Task完了を報告済みにすること |
| Task受理・steering反映・全体達成 | 作業 | 会話での受付、Agentの終了・自己申告、個別作用の確定度の独立変更 |
| 実対象への作用、既知／不明、停止結果 | 実行・拡張 | PermissionのAllow、Cancel受付、通信成功からの作用成功・不存在の推定 |
| 制御変更・現在の利用可否 | 権限・制約 | UI表示、過去のAllow、Audit内の文、認証成功 |
| 形成済み認識・revision・根拠 | 認識・学習 | 応答成功、保存前の形成候補、表示された表情やmotion |
| 保存・全域消去・backup・restoreの成立、Auditの受入順序 | 保全・消去と各参加owner | 局所完了、出力fileの存在、操作画面終了からの全域成功 |

外部作用の成功、内部記録の保存、Task達成、Ownerへの報告は別の事実として対応付ける。内部保存が失敗しても外部作用が未実行になるわけではなく、最後の正常記録が外部世界の最新状態とは限らない。不明を成功・失敗確定・未実行へ強めず、確定度を変える場合はその事実ownerが新たに確認した根拠を用いる。CC-04の非replay判断もこの記録を参照する。

Client不在時は個体調整が必要な未伝達事項をHostに残す。Task由来は元結果へ参照を戻し、別sourceのない通知等は必要内容自体を活動記録として保持する。次Clientの報告は現在の結果・利用範囲・削除状況へ照合し、接続・表示copy送信だけで報告完了にしない。提示不明を保持し、報告したことから承認・Task再開を導かない。exactly-once配信や既読保証は追加しない。

### 説明に必要な事実を残し、本文の別保管庫を作らない

各発生元は、主要Action・Permission判断・外部作用・Provider割当・Credential参照・重要設定変更・消去・Reset・restore等について必要事実を供給する。保全・消去はAuditの追記順と保持を管理し、その順序を外部世界での発生順や現在の許可の正本へ昇格させない。遅延・訂正された報告も元対象との対応を保つ。監査記録を元stateの再生・自動実行の入力にしない。

由来・失敗・停止不能・未保存・次の安全な選択肢は、内部思考や詳細Promptを公開せず説明する。通常はRaw Observation・Raw Voice・詳細Tool payload・内部推論を保存しない。再開や監査のためにも全Rawの恒久保存を要求せず、継続に必要な事実と根拠を各ownerが残す。Auditには通常、会話・file本文、秘密、削除済みprivate内容を含めない。

Debug captureは明示した対象・内容と短期失効・停止・削除に従い、登録済みCredentialを含めない。保全・消去のcapture設定を各対象責務が適用し、診断目的でもCC-02・05を守る。Auditの追記順は永久不変・削除不能を意味せず、通常保持管理とtargeted deletionへ参加する。Telemetry・Crash Reportは自動送信せず、手動共有時はOwnerが内容と送信先を確認する。

入出力・提示は日英・Text／Voiceで対象、制約、費用の報告／推定／不明、結果の確定度を保つ。重要なVoice内容にはText代替、Mute・Stop・Cancel・承認拒否にはkeyboard経路を用意する。Body・Voiceの演出や自然な要約が未完了を成功へ変えてはならず、表示localeは承認の意味やScheduleの保存timezoneを変更しない。

根拠: 要件「会話と情報提示」「Task」「Remote Client」「AuditとTelemetry」「通常保存しないdata」「品質と利用可能性」、SO 4.3・4.4・4.10・4.12・4.20・4.23・4.24、DR-01・08・10、DR文書3.4・7.1・7.6。AD-03・05・09・13・14、SC-08〜10、RT-01・08〜10。RF-02の結果、RF-04の未伝達、RF-06の残存記録、RF-07／08の完了説明が同じ確定度契約へ参加する。

## 9. 契約間の優先関係と横断照合

契約を一つの判定・state machineに統合しない。CC-01は権限を成立させる由来、CC-02は情報の利用範囲、CC-03は現在の用途への受入、CC-04は活動の継続・再開、CC-05は目的別の保全・消去、CC-06は消費と制御経路、CC-07は把握した事実の説明を扱う。一つの条件を満たしても他の必要条件は免除されない。

Targeted deletionは指定範囲の通常revision・根拠・History保持に優先するが、外部copyの消去やCredential失効を代行しない。結果記録の必要性も対象private本文を保持する例外にしない。現在scope・明示非共有・送信同意は、共有の有用性、Providerの情報量、cache効率より優先する。停止・cap・復旧保留は新規活動を制限するが、必要事実の返却・停止受付・機械的検証を同じ未承認活動の成功待ちにしない。

| 交差させた状況 | 確認した性質と責任の接続 |
|---|---|
| RF-01の共有・訂正とRF-03のrouting、Provider変更 | current共有から私的根拠へaccessを広げず、Observerと個体の同意を別々に満たす。古い検索結果・Relationshipから訂正前の事実を復活させない（CC-02・03）。 |
| RF-02のsteering・並列委任と遅い承認・費用報告 | 旧目的の結果と現目的の達成を区別し、古い承認でCancelを解除しない。処理中・不明な消費を全体制限から落とさない（CC-01・03・06・07）。 |
| RF-03のCapture後にRF-04の移動、RF-06のStop | 由来Client、現在presence、観測運用、専用assignment、個体自発性を分ける。旧候補でStop解除・別Client観測・送信同意の拡張をしない（CC-02〜04）。 |
| RF-04の切断と未確定Computer Use、RF-05の再起動 | 旧作用不明を維持し、Host作業の継続と個体移動を分ける。復帰でActionをreplayせず、途中Taskの明示再開・未伝達を扱う（CC-03・04・07）。 |
| RF-06の個体削除とRF-02の遅延作用、RF-07の情報消去 | 個体削除では必要なhistorical recordを残せるが、targeted deletionが重なる対象本文は残せない。作用事実の説明と固有Learningの非復活を両立する（CC-03・05・07）。 |
| RF-07の局所消去後に遅延推論・Client再接続・Host再起動 | 全利用先が再保存防止へ参加し、到達不能・検証失敗を完了としない。未完了と保留を維持し、事実報告は次の推論許可を待たない（CC-02・03・05・06）。 |
| RF-07の未完了とRF-08のbackup・restore | 未完了を無視した正常copyを作らない。Ownerによる旧backupの明示restoreと、自動再形成禁止を区別する（CC-03・05）。 |
| RF-08の正本切替と旧live結果、一括有効化 | 旧処理を復元stateへ混ぜず、外部作用・外部file・認証成功を巻き戻さない。一括有効化は現在のDeny・cap・同意・不明作用を無視しない（CC-01・03〜05・07）。 |
| 全Flowの停止・保全とLLM・Body・Voice・MCP Apps障害 | 管理・keyboard・Text代替を維持し、Host未受理の要求は完了にしない。内部管理のための通常Toolによる自己承認も許さない（CC-01・04・06・07）。 |

一般的なlogging、caching、configuration、security等を項目として追加することはしていない。それぞれがeneで守るべき情報・制御・保持・説明の契約へ参加する範囲を示した。Characterの部品選択手順、Task分割、具体的なObservation候補処理等、特定Flowやdomain内で決まる詳細は独立concernへ昇格させていない。

## 10. Design Freedomと新たなIssue

### 後続設計で選ぶ実現方法

| 設計対象 | 固定済みのarchitecture property | 残すDesign Freedom |
|---|---|---|
| Owner意図・判断対象・結果の対応 | 生成contentで由来を偽装できず、対象変更・失効を各利用先で扱える。 | provenanceの具体表現、確認のまとめ方、Permission evaluator、識別・鮮度確認・競合制御。 |
| ContextとProvider適応 | 用途・scope・根拠・現在認識・同意を保ち、Provider変更でも同じ情報選択方針を用いる。 | Context Assembly、routing用contextの生成方法・形式・更新頻度・鮮度・選択、要約・検索・scoring、prompt template、cache実装。 |
| 遅延結果と時間 | 元要求・根拠・用途への対応、時間的有効性、旧結果の非混入を維持する。 | revision・sourceの識別方法、受入手順、時刻表現、減衰、候補打切り、具体retry・timeout。 |
| Client排他性と停止 | 新旧二重存在を作らず、不明時に対象活動を続けず、通常Host作業を移送しない。 | pairing／bootstrapの認証材料の具体形式・保護・受渡し、再起動後の元Clientへの再接続・待機、切断検知、帰属調停、安全なround・作用の区切り、停止伝達。 |
| 全域操作 | 対象・派生物・処理中利用・Clientの参加、再保存防止、残存検証、正常状態保護と復元後保留。 | 探索・無効化・検証の実装、backupの整合時点・形式、restore切替・復旧、保存・暗号化方式。 |
| 消費と縮退 | 並列消費・未報告・不明を同じ上限へ反映し、制御・必要な保全を維持する。 | 費用予約・集計期間・推定、資源配分、反復抑制・Capture時機、駆動・待機の機構。 |
| 説明・監査・提示 | 元事実の確定度・privacy・順序の意味と管理到達性を維持する。 | audit format、診断・telemetry stack、提示確認、要約粒度、具体保持期間、UI layout。 |

crate／module、Rust trait／type、concrete API、middleware・interceptor・hook、event bus／queue／actor、IPC format、DB schema、transaction／lock、具体的Credential保護・sandbox・Plugin隔離も固定しない。上表の対応関係から統一Context layer、Policy Engine、Manager、Service、Coordinatorの追加を導かない。既存の12責務、semantic owner、Host／Client配置とtrust boundaryの下で実現方法を選ぶ。

### 新たなarchitecture contradiction

遅延結果の記録と活動禁止、個体削除後の記録保持と固有Learning消去、復元後の保留と通常再起動後の継続は、用途・目的・lifecycleを分ける既存契約で成立する。これらを単一の成功・停止・共有状態へ潰す実現方法は本書の契約違反となるが、既存architecture自体の矛盾としては扱わない。

### 新たなRequirement Ambiguity／Gap

観測停止時の取得済み候補をどこまで処理するか、切断Clientの消去完了根拠、旧live結果を区別する具体手段等は、既決の制約を満たす後続設計上の自由度として残る。必要な確認を省く、現在同意を広げる、未完了を成功とする自由度はない。意味的一致の完全検出、外部作用のrollback、exactly-once実行・配信等の未保証事項を新しい保証へ変えていない。

Architecture Review #1の解決済み事項は維持した。Stopped個体のpresenceなし、Observer専用assignment、個体削除後のhistorical record保持、Host自動起動設定の入出力・提示による所有を変更していない。公開地域・対象年齢等の留保、ReleaseごとのSupport Matrix・性能budgetも既存文書に残す。

### 引渡し

Architecture Review #2は、Owner decisions RA-01〜06を要件へ反映し、F-01〜03を独立に再評価して既存architectureへ統合したことでclosedとして扱える。判断と対応は[Architecture Drivers第3節](architecture-drivers.md#3-requirement-issues)に記録し、独立レビュー記録自体は変更しない。後続の詳細設計では各mechanismがCC-01〜07の参加責任・失敗時条件を満たすことを示す必要がある。
