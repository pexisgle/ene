# Client Presence Transition — Client帰属切替とClient依存活動の区切りの詳細設計

本書は、CompanionがClientへ存在し、呼び出され、移動し、切断され、再接続し、Host再起動後にpresenceを復旧する過程で、**presenceとClient依存活動の帰属が曖昧にならず、二重存在・古いClientでの作用・誤った再実行を起こさないためのlogical contract**を詳細化する。後続のstate representation／concurrency／persistence／IPC／interface設計が安全に依存できる水準まで定め、具体的なmechanismは固定しない。

## 1. 詳細化する問題と範囲選定

Client移動を「active Client fieldの瞬間的な代入」へ落とすと、次の帰属が失われる。後続設計が誤りやすい点と、本書で定める契約は次のとおりである。

| 誤りやすい点 | 本書で定める契約 |
|---|---|
| 新旧どちらのClientも「現在」と扱い、二つのClientで通常presenceが並立する。 | Hostが管理する単一のauthoritative帰属と、切替区間の新規開始禁止。 |
| 切替・再接続・再起動でClient側の表示・一時stateをcanonical presenceとして採用する。 | Host正本とClient側一時表現の区別、stale識別に必要なlogical property。 |
| 移動前の入力・出力round・Capture・作用結果を、新Clientの現在活動へ付け替える。 | 到着物の元帰属・元round・元試行への対応付けと用途別受入。 |
| すべての活動へ一律の「移動時停止」または一律の「自動継続」を適用する。 | 活動種別の区切り。旧で完了・中断・新round・Host継続・unknown残留の区別。 |
| 通常Host作業の継続をClient依存と混同し、移動でTaskを移送または停止する。 | Host継続の判断基準（Clientが必要かどうか）と非移送。 |
| presence復旧を会話round／Computer Use／Task／Actionの再実行・再開と混同する。 | 復旧できるものとreplayしてはいけないものの分離。 |
| 切断・再起動で不明だった外部作用を、回復・移動・再開で再実行可能へ変える。 | 不明の粘着的保持と自動再実行の禁止。 |
| Observerの対象判定を移動・Stop・切断と連動させず、旧Clientで観測・routingを続ける。 | 存在人数・routing対象の現在帰属への連動。 |

本書の範囲は、authoritative presenceの判断、切替区間の新旧の扱い、活動種別の区切り、到着物の帰属、Host継続、disconnect／reconnect／restart、Observer eligibility、Computer Useの固有条件までとする。Contextの情報選択・Provider適応の一般契約は[Context Assembly](context-assembly.md)を、認可判断と実作用・確定度・不明の一般契約は[Action Execution](action-execution.md)を、全域消去の参加・完了の一般契約は[Targeted Deletion](targeted-deletion.md)を利用し、再定義しない。Observer routing contextやProvider assignmentそのもの、具体的なComputer Use protocol、UI animation・audio buffering・capture intervalは対象外である。

## 2. 上位architectureとの位置関係

上位architectureとの優先順位は[設計文書 README](../README.md#正本と優先順位)に従う。本書内のSOは[State Ownership](../architecture/state-ownership.md)、DRは[Dependency Rules](../architecture/dependency-rules.md)の節番号を指し、CC／RF／RT番号は対応するarchitecture文書（[artifact 一覧](../README.md#artifact-一覧)）の契約IDである。新しいsemantic owner、万能Presence Manager、統一presence state machine、共通Client session layerを追加しない。

| 本書内の役割 | 既存の責任とauthoritativeな判断 | 本書が持ってはならない正本 |
|---|---|---|
| 帰属の調停 | 接続・存在。個体ごとの排他性と移動成立・activeなしの扱いを確定する。 | 移動の会話上の必要性、Task達成、Action許可の確定 |
| 個体の活動状態 | 個体調整。同一性、停止・再開・削除、移動の必要性、未伝達管理を扱う。 | 帰属そのものの成立、作用成功の確定 |
| round・提示の実際 | 入出力・提示。受付・提示・安全な区切りの実際を管理する。 | timelineの意味、帰属の成立、報告完了の代行確定 |
| 作用と確定度 | 実行・拡張。実対象への作用、停止結果、把握できた作用と確定度を管理する。 | 許可の付与、Task達成の判定、移動可否の判定 |
| Taskの目的・達成 | 作業。Taskへの受理・反映、委任、steering、進捗・達成・未完了を確定する。 | 個別作用の確定度の独立更新、帰属の確定 |
| 共有観測の対象・時機 | 共有観測。対象Client・時機・候補検知・routingを扱う。 | 個体の最終意味判断、Action許可の確定 |
| 制約の有効性 | 権限・制約。pairing許可・device許可・同意・cap・失効の現在可否を確定する。 | 帰属の成立、Task達成、作用成功の確定 |
| 秘密の扱い | 認証秘密。接続材料・Credentialの用途・有効性・除外を扱う。 | 帰属・許可・実行可否の確定 |
| 保全・復旧の調整 | 保全・消去。全域操作の成立・未完了・保留を対応付ける。 | 帰属の成立、通常意味変更権 |

[Cross-cutting Design](../architecture/cross-cutting.md)のCC-03（現在性と用途別受入）、CC-04（停止範囲と継続・再開）、CC-07（確定度）を、帰属切替固有の意味へ詳細化する。CC-01の意図対応、CC-02の利用範囲、CC-05の消去参加、CC-06の消費連続性は前提として利用し、再定義しない。Context Assemblyで確定した由来・用途・現在性・用途別結果受入、Action Executionで確定した判断対象と実対象の対応・委任不変・確定度・不明保持へ、帰属のsemantic ownershipを移さない。

## 3. Authoritative presenceの契約

### 3.1 現在のpresenceとして判断するもの

Running Companionの現在のpresenceは、**Hostが管理する個体ごとの帰属記録**だけをauthoritativeとする。管理責任は接続・存在にあり、個体調整の活動状態と権限・制約のpairing・device許可を参照して成立する。次のものは、それだけでは現在のpresenceの根拠にならない。

- Client側の表示（Bodyが見えること、timelineが残ること、音声sessionが残ること）。
- 過去にactiveだった記録、最後のClient・復帰候補等の再配置hint。
- Client側に残る入力中data・表示用timeline・音声buffer・未送信操作。
- 再接続したClientが持つ古い一時stateや進行中Actionの存在。
- Host再起動前に保存された復旧先の記録そのもの。
- Provider session・cache・検索indexに残る過去の帰属情報。

復旧先の記録と再配置hintは接続・存在が管理するが、記録だけで現在presenceが成立したとは扱わない。現在の接続・許可・排他性を確認できて初めてpresenceが成立する。再接続・待機・timeout・pairingの具体方式は固定しない。

### 3.2 混同してはいけない四つの意味

presence、Host継続、接続、許可は別の意味である。

- **presence（帰属）。** どのClientにそのCompanionが存在し、Client依存活動をどこへ結び付けるか。接続・存在が確定する。Runningは最大一つ、StoppedはClientにもHostにも持たない。
- **Host継続。** 個体がHost正本で同じ個体として存続し、Hostで完結する活動を続けられること。Runningのままactiveなしでも存続する。presenceとは別であり、activeなしを停止とみなさない。
- **接続。** ClientがHostへ到達し、機能を利用できるという観測事実。接続・存在が管理する。pairing済み・接続済みはactive・許可済みと同義ではない。
- **許可。** そのClientでその活動をしてよいという制御上の有効性。権限・制約が確定し、各利用箇所が適用する。presence・接続はAction許可を意味しない。device許可・Capability・同意・cap・保留も別に満たす。

Hostがcanonical stateと継続実行を持つことと、CompanionがHostへpresenceを持つことは別である。Stoppedの保存data・hintをHost上のpresenceとして扱わない。管理面全体へ会話と同じactive制約を拡張しない。Host同居Clientにも同じ帰属制約を適用する。

### 3.3 Staleを識別できるlogical property

具体的なsession token・lease・heartbeat・epoch・sequence numberは固定しない。ただし後続設計は、少なくとも次の論理的な対応を保持・照合できる必要がある。確認できないものを現在として推定する方式は採れない。

- Hostのauthoritative帰属と、Client側一時表現・入力・結果が主張する帰属との対応。どのHost帰属に対応する一時data・入力・結果かを解決できること。
- 現在帰属・旧帰属・帰属なし・停止中・復旧待ちの区別。過去の帰属を現在の利用可能性として再成立させないこと。
- 対象Companion・対象Client・入出力roundや観測候補・試行との関係。Context Assembly第3節のClient依存の対応を帰属切替を越えて維持できること。
- 消去・失効・停止・保留との関係。古い一時dataから現在許可・同意・scopeを復活させないこと。

Clientの古い設定copyや外部codeが独立に同意・現在性を決めてはならない。Hostの現在条件を確認できないClientは該当活動を継続せず、一時copyを第二の正本にしない。到達不能・確認不能を成功・現在へ読み替えない。

## 4. 切替途中の区別

### 4.1 必要な論理的な区別

具体的なstate machineやhandshake protocolは固定しない。ただし後続設計は、少なくとも次の論理的な区別を保つ。すべてを一つのbooleanや単一active fieldへ潰す実装は不可とする。

- **旧presence。** 切替前にauthoritativeだった帰属。旧Clientでの新規Client依存開始の根拠だったが、切替決定後は新規開始の根拠にしない。
- **移行中（切替区間）。** 移動が決定・要求されてから新presenceが成立するまでの論理的な区間。旧も新も新規開始の根拠にしない。旧でのin-flightの安全な完了と、Host継続だけが許される。
- **新presence。** 新Clientについて現在の接続・許可・排他性を確認して成立した帰属。以後のClient依存活動だけがここへ帰属する。
- **activeなし。** RunningのままどのClientにも帰属しない正当な状態。Client依存活動は行わず、Host継続・未伝達管理は行う。
- **停止中。** 個体調整の停止状態に従い、どのClientにもHostにもpresenceを持たない。activeなしとは別の意味であり、Host継続・disconnect復帰・再起動復旧を適用しない。

Host再起動後の復旧待ち（復旧先はあるが現在接続・排他性が未確認）はactiveなしの一種であり、旧presenceの自動延長ではない。別Clientへの無条件移動へ広げない。

### 4.2 切替区間で成立させる排他性

どの途中状態でも次を成立させる。

1. **二つのClientで同時に通常presenceを成立させない。** 旧帰属による対象活動を残したまま新をactiveにしない。新旧Clientで同一個体が同時に対象活動（Body・通常Text／Realtime会話・Voice・観測関係・自発的interaction・Computer Use）を行わないことが成立条件である。
2. **古いClientの一時状態をcanonical presenceとして扱わない。** 旧Clientの表示・buffer・未送信操作・進行中Actionの残存は、切替成立・再接続・再起動の根拠にならない。Hostの現在帰属へ上書きしない。
3. **安全に終了できていないClient依存作用を別Clientで自動継続しない。** 旧試行の作用・不明は元の試行・Taskへ対応付け、新Clientでの新しい試行として自動再実行しない。移動と未確定Actionの再実行は別に扱う。

切替決定から新presence成立まで、新規のClient依存開始を旧Clientでも新Clientでも行わない。旧ではin-flightを安全な区切りまで完了させる範囲だけを許し、新では成立後の新しいround・試行だけを許す。通常Host Taskの終了・移送を切替の前提にせず、Agent・Workspace・Host正本を呼出し先へ移送しない。移動の必要性（Ownerの呼出し・事前指示・文脈上の必要性）の判断は個体調整、成立は接続・存在に残る。自発移動も通常の自発性・Permission等の制限に従う。

入出力・提示は両側に把握できた移動状態を示す。区切りの報告のために新Clientへの移動完了を待たない。排他性を確認できないClientは対象活動を続けない。Hostも現地の物理的停止・外部作用取消を確認なしに成功扱いしない。

### 4.3 呼出し・移動の基本形

別Clientから会話・操作する場合は、先にそのClientへの呼出し・移動を経る。元Clientに残したまま別ClientからTextだけを送って応答させることを基本モデルにしない。呼出し先Clientは許された接続・機能を持ち、pairing済みだけでは足りない。移動後のambient Observationは移動先ClientのObserver設定と全体制御に従い、自発性設定はCompanion単位で引き継ぐ。

通常のClient切断では、Running個体を基本的に利用可能なHost PC上のClientへ移動する。利用可能なClientがなければactiveなしとする。Host側Client環境を自動起動しない。Stopはdisconnectと異なり、停止中個体をHost側へ移動してpresenceを残さない。Host再起動時は次章とは別の復旧契約（第8節）を適用する。

## 5. Client依存活動別の区切り

すべての活動へ一律の移動時停止を適用しない。各活動について、既存requirementsから必要な範囲で区別する。具体的なUI animation・audio buffering・screen capture intervalには進まない。

| 活動 | 帰属と区切りの契約 |
|---|---|
| **Text（一対一・グループの発話を含む）** | 現在存在するactive Clientに属する。round単位で安全に区切る。旧roundの入力・生成途中・未提示は新roundへ自動継続しない。旧roundの結果は元round・Historyへ対応付け、未伝達は個体調整の未伝達管理へ接続し、次Clientで現在の結果・利用制限に基づいて要約報告する。逐語の自動転送・旧roundへの自動出力をしない。グループでも各Companionの参加は自身のpresenceを必要とし、参加だけを他個体の私的状態へのaccessにしない。 |
| **Voice（Realtime・turn-based）** | active Clientに属するsession的な利用であり、中断可能だが同一streamとして再開しない。旧Clientの待受・発話・認識途中は区切り、Mute・停止・承認拒否のkeyboard経路は維持する。新Clientでは新しいsession・roundとして開始し、現在の割当同意・device許可・費用条件を満たす。話者認証済みの意味を足さず、周囲発話の扱いの説明を維持する。Voice障害はTextへの段階的切替であり、帰属切替の迂回にしない。 |
| **Body（Desktop overlay）** | active Clientに属する表示であり、旧Clientではhide・休止し、新Clientでは成立後に表示する。二重表示しない。位置・size・hide等の一般設定はHost正本だが、実際の表示は現在帰属に従う。描画失敗・fullscreen・高負荷はText・管理・復旧へ波及させない。出力した表情・motionをCompanion Stateの正本や永続変化の根拠にしない。 |
| **Observationとの関係・自発的interaction** | 対象Clientの適格性と、個体の自発判断は別である。旧ClientのCapture・候補・routing結果を新Clientへ付け替えない。移動・Stop・Pause／OFF・fullscreenが重なれば、由来Client・現在presence・観測条件を照合し、旧候補で新規Capture・deliveryを続けない。Stopped個体を起こさず、存在人数に数えない。自発発話・通知等の最終判断は各Companionが現在の帰属・Mute・Quiet hours・費用・Permissionの下で行い、切替区間の新規自発開始は抑止する。 |
| **Computer Use** | 最も強い帰属制約を持つ。第10節の固有契約に従う。旧で安全な区切りまで完了またはbest-effort停止し、新で新しい試行として開始する。旧試行の自動再実行をしない。 |
| **通常のHost-side Task・Task Agent・Schedule** | active移動から独立して継続できる。判断基準はClientが必要かどうかである。Clientが必要なstepだけを待たせ、Task全体を一律Cancel・移送・停止しない。呼出し先へHost作業を移送しない。Owner確認が必要なActionは判断待ちのまま自動承認しない。 |
| **Companion間交流・通知生成・Client不要の内部調査** | RunningならactiveなしでもHost内で継続できる。Ownerへの提示・伝達は次Clientへ延期する。Stopでは開始しない。 |
| **未伝達事項の報告** | Hostに残し、次Clientで要約報告する。接続・表示copy送信・Task完了だけで報告済みにしない。提示不明を保持する。報告済みは承認済み・Task再開済みではない。 |

軽微な本体ActionもTask化の省略はできるが、権限・作用・記録の境界は省略しない。まとまった作業・file実作業・Computer Useを含むまとまった作業・長期継続・並列委任価値のある作業はTask化・委任の原則に従う。

Permission・device・同意・capの失効が重なれば、その条件だけを根拠とする新規利用を止め、別Client・別Agent・別Toolへ迂回しない。無関係なHost Taskまで一律Cancelしない。

## 6. 移動・切断中に到着した入力・出力・Capture・作用結果の帰属

Context Assembly第7節とAction Execution第7節の用途別受入に、帰属の照合を追加する。到着が遅いことだけで出来事を新しくせず、最新に届いた結果だけで現在値を決めない。必要な対応を失った結果は、到着先の現在活動へ付け替えず、その用途では受理できないものとして利用元が扱う。

| 到着物 | 残すべき事実・許される受入 | 禁止する扱い |
|---|---|---|
| 切替区間に旧Clientで到着した入力 | 元帰属・元roundへ対応付ける。切替決定後の新規受付として旧で実行せず、新へ自動転送もしない。必要なら新Clientで新しいround・依頼としてOwnerが改めて示す。 | 旧入力を新Clientの現在要求とする。未送信操作を自動Action queueとしてreplayする。 |
| 切替区間に新Clientで到着した入力 | 新presence成立前の現在要求にしない。成立後に新しいroundとして受理・判断する。成立前の先行実行をしない。 | 成立前の入力で新presenceを成立させる。旧roundの継続として実行する。 |
| 旧Clientで生成済みだが未提示の出力 | 生成済み＝提示済みにしない。元roundへ対応付け、未伝達管理へ接続し、次Clientで現在条件に基づく要約報告とする。 | 旧roundの出力を新Clientへ自動出力する。表示copy送信だけで報告完了にする。 |
| 旧Clientの遅延message・表示copy | Hostの現在正本・帰属・消去状況に従い、未確定と受理済みを区別する。Client copyでHostを上書きしない。 | 遅延messageで現在帰属・Permission・Task状態を復活させる。 |
| 旧Client由来のCapture・候補・routing結果 | 由来Client・取得時点・候補との対応を維持する。新Clientのものに付け替えない。移動・Stop後のdeliveryは現在帰属・活動状態・観測条件へ照合し、Stoppedを起こさない。 | 旧Captureを移動先の観測として利用する。旧候補で別Clientの観測・送信同意を拡張する。 |
| Cancel・steering・失効・移動・削除・復元後のTool結果・承認・推論結果 | 元Action・Taskへ必要な作用事実だけ対応付け、現在のTask・目的への採用は再評価する。後続を自動開始しない。古い承認でCancel・失効・移動を解除しない。 | 旧結果を新目的の達成とする。旧承認で新試行を開始する。 |
| Client依存の作用結果（既知／不明） | 実行・拡張の作用事実は元試行へ、作業の達成判断は現在Taskへ対応付ける。不明は不明のまま保持する。 | 不明を未実行・成功へ変換する。新Clientでの自動再実行で解消する。 |
| 消去区間・復旧保留と重なる到着物 | 各受入先が消去条件・保留へ照合し、対象情報を戻さない。旧live結果を復元正本へ混ぜない。 | 新着だから新しいExperienceとする。作用記録・Auditを理由に対象本文を再保存する。 |

Streaming・部分結果も、新たに保存・送信・提示・作用へ渡す部分ごとに帰属・消去・許可の受入条件を満たす。全文到着を常に待つ指定はしないが、未検証の部分を先に露出して後で無効化することで制約を満たしたことにしない。すでに起きた露出・作用は取り消せたと推定せず、把握できた事実を維持する。

推論結果は少なくとも実際に入力した情報範囲・帰属に依存し得るものとして扱う。旧帰属のcontext・cache・sessionを新帰属の現在contextとして再利用しない。Provider sessionに旧帰属情報が残る場合、現在使えない情報を含むsessionへの新規依頼は、現在のcontextを短く渡すだけでは成立しない。

## 7. Host-side Taskが継続できる理由と範囲

判断基準はClientが必要かどうかであり、Taskの重要度・進行度・担当の存否ではない。Clientが必要なこと以外のTask等は可能、Clientに依存することは不可能とする。この基準により、通常Host作業はClientの存在・表示・移動完了を存続条件にしない。

- 許可済みHost上のTask・Task Agent・Schedule起動・保存は、activeなしでも継続できる。待機だけのLLM pollingは行わない。
- Client依存step（Computer Use等の現地作用、現地での入出力・提示を要する確認）は、そのstepだけを判断待ち・保留とし、Task全体のCancel・移送・再作成にしない。
- 作業中であることだけでは移動を妨げない。Client移動は通常のHost-side Task実行を別Clientへmigrationすることではない。
- Task AgentはCompanionの継続人格や任意Clientの選択者にならない。委任されたComputer Useにも委任元Companionの存在場所の制約を適用する。Task Agentが独自に操作Clientを選ばない。操作対象の変更は先に個体の移動を必要とする。
- Host Task完了とClient不在が重なれば、結果は未伝達管理へ接続し、次Clientで報告する。Task完了を報告済みにしない。
- Background・Scheduleの無人実行も同じ条件に従い、Client不在・Schedule到来を自動承認の理由にしない。Owner確認が必要なら実行せず判断待ちにする。
- fullscreen・Observer Pause／OFF・Mute・高負荷・費用capは、それぞれの対象だけを抑制し、通常Host Taskの一律Cancel・形成済みLearning消去へ拡張しない。

管理面全体へ会話と同じactive制約を課さない。担当Companionの削除後も残るTask記録へ到達できることと、帰属切替中のHost継続は別の契約だが、いずれもTask記録の正本を作業に残すことで成立する。

## 8. Disconnect／reconnect／restart

### 8.1 切断の種別の区別

いずれも「不明を未実行へ戻さない」「自動replayしない」は共通だが、帰属の扱いは異なる。具体的な検知・timeout・reconnect intervalは固定しない。

| 事象 | 帰属・活動の扱い |
|---|---|
| 一時的なnetwork loss・短時間の到達不能 | 現在帰属を直ちに捨てないが、排他性・利用可能性を確認できない間は新規Client依存開始をしない。in-flightはbest-effort停止の準備をし、確認不能を成功にしない。回復後に旧帰属の自動延長として無条件継続せず、現在の帰属・許可へ照合する。 |
| Client process終了・通常のClient切断 | Runningは基本的に利用可能なHost PC上のClientへ移動し、なければactiveなしとする。旧Clientの未確定ActionをHostで自動再実行しない。旧Actionの停止はbest-effortとし、既知／不明を作業へ返す。 |
| Client deviceの再起動・OS再起動 | 同上だが、再接続したClientの古い一時stateだけで現在presence・Permission・実行再開を成立させない。表示・帰属・未送信操作を正本に戻さない。 |
| device失効・許可変更 | 失効した機能の新規利用を止め、現在条件へ反映する。別device・別Clientへの切替で迂回しない。無関係なHost TaskすべてをCancelしない。旧材料だけで失効後の信頼を復活させない。 |
| Host再起動 | 第8.3節の復旧契約に従う。通常切断時のHost PCへの移動とは別に扱う。 |
| Stopとの競合 | Stopが確定すればdisconnect復帰・reconnect・再起動復旧より優先する。停止中個体をHost側へ移動してpresenceを残さない。接続回復だけでResume・再配置しない。 |

切断検知・排他性の実現mechanismは後続に残す。必要なのは、排他性を確認できないClientは対象入出力・観測・自発的interaction・Computer Useを続けないことである。

### 8.2 Reconnect

再接続したClientが古い一時stateや進行中Actionを持っていても、それだけで現在presence・Permission・実行再開を成立させない。Hostの現在正本・帰属・消去状況に従い、未確定と受理済みを区別する。Client copyでHostを上書きせず、未送信操作を自動Action queueとしてreplayしない。

- 古い表示copy・入力・Tool UI dataをHostへ戻して対象・Task・Learningを再形成しない。
- 古い承認・判定copy・解決済み経路だけで新規利用を始めない。現在の条件（Permission・委任・Workspace・device・費用・停止・保留・消去）を照合する。
- 消去中の再接続では、確認不能を消去成功に読み替えず、古い一時dataをHostへ戻して対象を再形成しない。必要な確認を省いて完了としない。
- Device失効・全データReset後は旧材料だけでHostの信頼を復活させない。再pairingは現在のHost側確認に基づく。

再接続は、Hostの進捗・結果へ到達できるようにする操作であり、旧live状態の復活ではない。切替前に開始した推論・Tool結果が切替後に届いても、旧live状態を現在の正本へ混入させない。

### 8.3 Host再起動後のpresence復旧と再実行の分離

Host再起動後は、保持された正本から現在用途に必要なcontextを再構成する。保存した派生物を材料にできても、旧要求の実利用許可や旧実行をそのまま再開しない。次の三つを区別する。

1. **presenceの復旧。** Running Companionは再起動前に存在していたClientへ自動的にpresenceを復元する。これは別Clientへの自発移動とは別の復旧であり、元のClientが利用可能になるまでactiveなしとして扱える。別Clientへの無条件の自動移動へ広げず、Stopped Companionには適用しない。Host側Client環境の自動起動は行わない。再接続・待機・timeout・pairingの具体方式は残す。
2. **会話round・Computer Use・Actionの再開。** 自動再実行・再開しない。旧roundは終了し、旧試行の不明は不明のまま保持する。新しいround・試行は現在の帰属・許可・目的で開始し、Owner判断を要する不明は重複riskを示して判断を求める。presence復旧はTask再開・Action開始の権限にならない。
3. **途中Task・Scheduleの扱い。** 途中Taskは自動再開せず、保存済み進捗・既知作用・不明・未完了を示してOwnerの明示再開を待つ。停止中に到来したSchedule回はmissedとし、自動補完しない。将来回は新Taskとして現在条件で組み立てる。Agent sessionの残存・Provider回復・Host自動起動の選択だけでは再開しない。

通常再起動は、restoreのように全自動処理を一律の復元内容確認へ戻す操作ではない。未完了消去・復旧の保留があればそれを維持する。Host shutdownでも必要な進捗・作用不明・未伝達・全域操作の未完了を保全し、外部作用がHostと同時に消えると推定しない。Clientは独立実行・代替正本にならず、停止要求をHostへ届けられなければ受付と停止完了を混同しない。

Restoreとの区別も維持する。Restoreは対応backupからの全置換と復元後保留・Ownerの一括有効化を必要とし、再起動時の自動presence復旧のような自動復元ではない。復元成立と実行再有効化は別々に確認する。

### 8.4 Stopとreconnectの競合

Stop成立後はClientにもHostにもpresenceがない。保存済み同一性・経験状態・hintは残せるが、hintは再配置候補にすぎない。接続回復だけではResumeしない。停止前の遅延結果を、新しく再開した活動の指示・許可と混ぜない。停止中のSchedule回はmissedのままである。

削除後は、必要な作用事実を残るTask／活動記録へ対応付け、管理面から説明する。削除済み個体の応答・Memory・Relationship等を再作成しない。残るHistoryが個体の過去の存在を示していても、そこから個体・固有Learningを再作成しない。

## 9. Observation

ObserverはClientに紐づく共有componentであり、対象Clientに少なくとも一つRunning Companionが存在するときだけ観測対象になる。Companion移動・Stop・切断によってObserver eligibilityは次のように変わる。Observer routing contextやProvider assignmentそのものは設計済みであり、やり直さない。

- 存在人数の数え方はauthoritative帰属に従う。Stopped個体を人数・routing対象に数えない。Client側の表示残存・旧帰属の一時dataを人数に数えない。
- 移動では、旧Clientの対象性と新Clientの対象性を現在帰属で見直す。旧Clientで最後の個体が抜ければそのClientの新規観測対象性がなくなる。新Clientでは移動先のObserver設定と全体制御に従う。自発性設定はCompanion単位で引き継ぐ。
- 切替区間は、旧Clientの旧Capture・候補で新規Capture・deliveryを続けない。移動先のObservation設定へ古いCaptureを自動的に付け替えない。
- activeなしの間、その個体についての新規観測は発生しない。他個体が残るClientの対象性は、その他個体の存在で判断する。
- Stopでは共有観測の人数・routingから除外し、最後の個体ならそのClientの新規観測対象性がなくなる。Host内の自発交流・通知・内部調査の停止とは別の判定だが、同じ停止状態を参照する。
- disconnectでは、到達不能Clientの旧人数で観測・routingを続けない。確認不能を対象性ありに読み替えない。
- Pause／OFF・fullscreenが途中で成立した場合は、それが制限する今後の観測・提示を止め、未完了Capture・検知・deliveryの扱いも現在条件に照合する。取得済み候補を一律破棄するか条件を満たす範囲で処理を終えるかの細部は固定しない。ただし旧設定だけで次の観測・送信を始めず、fullscreen中の自発発話へ流さない。
- 候補検知・関連付けはObserver専用assignmentで行い、Companion overrideを適用・合成しない。限定private文脈を使う場合も、元情報の利用制約と専用assignmentの送信条件をともに満たす。delivery後の個体推論は別consumerとしてそのCompanionの通常Provider設定・同意を満たす。
- eventを受けたCompanionのメインLLMが自身の文脈で意味判断する。Observerによるroutingは最終判断を置き換えない。すでに個体が認識したExperienceは別lifecycleであり、観測OFF・移動・Stopだけで消去しない。delivery後の個体側推論条件を満たせなければ、その個体の理解・活動が成立したと扱わない。
- 同意失効・targeted deletionが重なる場合は、利用禁止と再保存防止が優先する。生成済み要約・処理中結果にも適用する。混合生成文しかなく分離を確認できなければ個体へ渡さず、許された範囲で構成し直すか不足として扱う。

複数対象Clientは同時にCaptureせず、指定頻度に対し制限を優先しつつ時機をずらすことは既存契約のままである。本書はその時機algorithmを決めない。

## 10. Computer Use

Computer Useでは特に次の既存contractを維持する。具体的なComputer Use protocol・UI automation backendは設計対象外である。

- **現在presenceのあるClient限定。** 操作対象は委任元Companionの現在のactive Clientだけである。TaskやTask Agentから任意のpairing済みClientを独立に選ばない。Host PCを対象にする場合もHost上のClientへの存在を必要とする。存在場所と操作対象を分離しない。
- **安全な区切りまで移動を遅延できる。** Client依存Action実行中の移動は、外部作用の結果を曖昧にしないよう安全に区切れるところまで遅らせられる。待つのは対象の入出力round・Client依存作用に必要な区切りであり、無関係なHost Task完了を要求しない。切断・停止不能を「待ち続ければ成功する」と扱わない。
- **disconnect後の作用結果不明。** Client切断による停止はbest-effortとし、停止できなかった処理・既知の作用・結果不明の作用を示す。現地の物理的停止・外部作用取消を確認なしに成功扱いしない。排他性不明のClientで活動を続けない。
- **別Client／Hostでの自動再実行禁止。** 移動を理由に元Clientで実行していたActionを別Client・Hostで自動再実行しない。成功不明な外部作用はHost PCその他のClientで自動再実行せず、重複の可能性がある場合はOwnerの判断を求める。Companionの移動と未確定Actionの再実行は別に扱う。Ownerの明示再開も不明Actionの無差別replayにしない。

現在のClientに存在することはActionの許可を意味しない。通常のCapability・Permission・device許可・外部作用の記録を適用し、ambient Observationの有効化を操作の承認として扱わない。許可済みComputer Useでene自身の承認ボタンや設定画面を操作してOwner操作を偽装しない。

試行と作用の区別、判断・目的・実対象・段階・確定度・停止保留との対応の保持・区別はAction Execution第8節の契約を利用する。具体的なidempotency key・transaction・retry実装・queue／actor・DB schema・lock方式は固定しない。重複し得る再送を「同じ試行の継続」として不明・重複管理から除外する方式は採れない。retryは新しい試行として現在の認可・目的・実対象・条件で扱い、不明試行の再実行は重複riskを説明したOwner判断を必要とする。

## 11. Runtime Flows・Cross-cutting Design・他のcritical-area契約に対する横断検証

[Major Runtime Flows](../architecture/runtime-flows.md)第3節とRF-01〜08、[Cross-cutting Design](../architecture/cross-cutting.md)CC-01〜07、[Context Assembly](context-assembly.md)、[Action Execution](action-execution.md)、[Targeted Deletion](targeted-deletion.md)に対する、正常系と意味のある競合・障害のwalkthroughは次のとおりである。

| Flow・交差 | Walkthroughと必要な結果 | 本書の成立箇所 |
|---|---|---|
| RF-04正常：A→B呼出し移動 | 移動意図→帰属照合→round・作用の区切り→排他切替→新帰属での利用→観測見直しが閉じる。Host Taskを移送せず、新activeを旧Action再実行の許可にしない。 | 4、5、7。CC-04、Context 8.1、Action 8.4を維持。 |
| 移動要求と入力到着の競合 | 切替区間に旧・新へ入力が到着。いずれも現在要求として自動実行せず、旧は元roundへ、新は成立後の新roundとして扱う。未送信操作を自動queueにしない。 | 4.2、6。Context 3.1・7.2、CC-03を維持。 |
| Computer Use中の切断 | 結果返答前に切断。best-effort停止と既知／不明を作業へ返し、移動完了待ちにしない。Host PC移動しても旧Actionを自動再実行しない。不明は保持する。 | 8.1、10。Action 6.2・8.4、CC-04・07を維持。 |
| 新Clientへの移動途中でのfailure | Bが途中で利用不能。Bへの移動を成功表示せず、成立済み帰属またはactiveなしを基に扱う。二重存在を作らない。戻り先選択の具体algorithmは残す。 | 4.2。CC-04、RT-02を維持。 |
| 旧Clientの遅延message・表示copy | 再接続・遅延で旧copyが戻る。Host現在正本・帰属・消去へ照合し、未確定と受理済みを区別する。Client copyでHostを上書きしない。 | 6、8.2。Context 7.3・8.1、Action 7.3、CC-03を維持。 |
| Host Task完了とClient不在 | Task完了時にactiveなし。結果を未伝達管理へ接続し、次Clientで現在条件に基づく要約報告とする。接続・送信だけで報告済みにしない。 | 5、7。Context 7.3、Action 9、CC-07を維持。 |
| Host restartとClient再接続 | 途中Task＋in-flight不明を抱えて再起動。presenceは元Clientへ自動復旧、Taskは明示再開待ち、旧round・旧試行はreplayしない。元Client利用不能ならactiveなしで待つ。 | 8.3。Context 8.1、Action 8.4、CC-04を維持。 |
| Stopとreconnectの競合 | Stop確定後に旧Clientが再接続。接続回復でResume・再配置せず、停止前の遅延結果を新活動の指示・許可に混ぜない。停止中回はmissedのまま。 | 8.4。CC-04、SO 6.3を維持。 |
| RF-03交差：Capture後に移動・Stop | 候補戻り前に最後の個体が移動・停止。旧人数・routing候補で新規Capture・deliveryを続けない。旧Captureを新Clientへ付け替えない。Stoppedを起こさない。認識済みExperienceは観測OFFだけで消さない。 | 9。Context 6.3、CC-02〜04を維持。 |
| RF-01交差：応答生成と移動 | 応答生成後に移動。生成済み＝提示済みにせず、元roundへ対応付けて未伝達とする。新Clientで現在帰属の提示条件を満たして報告する。 | 5、6。Context 7.3、CC-03・07を維持。 |
| RF-02交差：steering・Cancelと移動 | steering・Cancel後に移動・遅延結果が到着。元Action・Taskへ事実を残し、旧結果を新目的の達成にせず、古い承認でCancelを解除しない。後続を自動開始しない。 | 6。Action 7、CC-03・04を維持。 |
| RF-05交差：Schedule到来とactiveなし | activeなしでSchedule到来。Host完結作業は開始し、Client依存確認は判断待ちにする。到来を自動承認にしない。各回を新Taskとしmissedを補完実行しない。 | 7。CC-01・04を維持。 |
| RF-06交差：個体削除後の遅延作用 | 削除後に対象を含む作用結果が到着。残るTask記録へ必要な事実だけ残し、削除済み個体のLearningを再作成しない。 | 8.4。Action 7.3、CC-03・05を維持。 |
| RF-07交差：消去中の切断・再接続・再起動 | 消去中に切断・再起動。確認不能を成功にせず、古い一時dataをHostへ戻さない。未完了・保留・再保存防止をHostで維持する。新ClientへのCapture付け替え・旧Actionのreplayをしない。 | 8、9。Targeted Deletion 7.2、Context 8.2、CC-05を維持。 |
| RF-08交差：restore後の旧live結果 | 切替前に開始した推論・Tool結果がrestore後に到着。旧live状態を復元正本へ混ぜず、Task・Learning・許可を復活させない。 | 8.3。Context 8.1、Action 7.3、CC-03・05を維持。 |
| 全Flow：補助推論・費用不明との競合 | 帰属判断・移動可否の推論が不通・費用不足。未承認Action・無条件移動を先に実行せず、不足を管理面へ返す。停止・拒否・機械的検証は継続可能。 | 3〜5。Context 5.2、CC-06を維持。 |

CC-01〜CC-07の一般条件は[Cross-cutting Design](../architecture/cross-cutting.md)が持つ。本書が加えるのは次の点である。

- CC-01：移動要求・呼出し・自発移動の由来と、帰属成立・Action許可を区別する。画面内指示・観測ON・presenceから権限を新設しない。
- CC-02：旧帰属のcontext・cache・sessionを新帰属で再利用しない。
- CC-03：過去の帰属の正しさと現在の用途への有効性を分離する。
- CC-04：自動replay・自動復帰の拡張をしない。
- CC-05：Client一時dataの古いcopyからの再保存・再形成を防ぐ。round終了を全域完了にしない。
- CC-06：制御・保全経路を移動完了待ちにせず、機械的な帰属確認をLLM待ちにしない。
- CC-07：生成済みを提示済みにしない。

## 12. 本書が固定する契約と残す Design Freedom

下位設計は、次を本書が固定した契約として利用できる。

- 現在のpresenceはHostが管理する個体ごとの帰属記録だけがauthoritativeである。Client表示・過去記録・hint・一時copy・復旧先記録・Provider残存は根拠にならない。presence・Host継続・接続・許可は別の意味である。
- 切替区間は旧・新のいずれも新規開始の根拠にしない。旧は安全な区切りまでの完了だけ、新は成立後の新しいround・試行だけを許す。二重presence・旧一時のcanonical化・未終了作用の自動継続をしない。
- Text／Voice／Body／Observation関係／自発interaction／Computer Use／Host作業／Host内活動／未伝達報告は第5節の区切りに従う。一律停止・一律継続にしない。
- 移動・切断中の到着物は元帰属・元round・元試行へ対応付け、用途別に受け入れる。到着先の現在活動へ付け替えず、Client copyでHostを上書きしない。未送信操作を自動queueにしない。
- Host継続の判断基準はClientが必要かどうかである。通常作業は移送・停止せず、Client依存stepだけを待たせる。Task Agentが操作Clientを選ばない。
- 切断種別（一時loss・process終了・device再起動・失効・Host再起動・Stop競合）で帰属の扱いを変える。再接続の古い一時stateだけでpresence・Permission・再開を成立させない。
- Host再起動後のpresence復旧と、round／Computer Use／Task／Actionの再実行・再開は別の条件である。前者は元Clientへの自動復旧（利用不能ならactiveなし）、後者は自動replay禁止・明示再開である。
- Observer eligibilityはauthoritative帰属の存在人数に従い、移動・Stop・切断で見直す。旧Captureの付け替え・Stoppedの覚醒・同意拡張をしない。専用assignmentと個体側推論の条件を分離する。
- Computer Useは現在presence限定・安全な区切りまでの遅延・disconnect後不明・別Client／Hostでの自動再実行禁止を維持する。存在は許可を意味しない。
- 試行と作用・判断・実対象・段階・確定度・保留の対応を保持・区別できなければ、成功・未実行と推定しない。不明は粘着的に保持し、重複riskを示したOwner判断なしに再実行しない。

本書が採れない選択肢として除外するのは、新旧二重presence、Client表示・hint・一時copy・復旧先記録のcanonical化、旧round・旧Capture・旧作用の新活動への付け替え、Client copyでのHost上書き・自動Action queue、Host作業の移送・一律停止、Task Agentによる操作Client選択、再接続・復旧によるpresence・Permission・実行の自動成立、別Clientへの無条件自動移動・Stoppedへの復旧・Host側Client環境の自動起動、presence復旧によるTask・Actionの再開権限化、不明の未実行・成功への変換と自動replay、要約・復旧での確定度強化、Observer混合出力の無条件配送・古いsessionによる制約迂回、消去対象の旧Client copyからの復活である。いずれも上位契約を成立させないため採れない。

以下は意図的に残すDesign Freedomである。

| 設計対象 | 固定済みのarchitecture property | 残すDesign Freedom |
|---|---|---|
| 帰属・対応・現在性の表現 | 由来の偽装ができず、対象変更・失効・切替・消去条件を各利用先で扱える。確認不能を許可・現在へ変換しない。 | 帰属記録・由来・round・試行・区間・完了境界の具体表現、確認のまとめ方、対象探索・帰属調停の配置、識別・鮮度確認・競合制御。 |
| 切替・区切りの伝達 | 新規開始禁止、best-effort停止、遅延帰属、replay禁止。受付と完了を分ける。確定度を強めない。 | 切断検知・帰属調停・round区切り・安全な作用区切りの具体的方法、queue／actor／event bus・lock／transaction・IPC・process配置・retry／timeout値。 |
| 再接続・復旧の実現 | 古い一時のcanonical化・自動再開の禁止、元Clientへの復旧とactiveなし待機、別Clientへの無条件移動の禁止。 | 再接続・待機・timeout・pairing・再配置hint選択・復旧の具体方式、切替未成立時の戻り先選択、提示確認・要約粒度・未伝達保持期間。 |
| Observerの実現 | 存在人数・routing対象の現在帰属連動、旧Capture非付け替え、専用assignmentと個体側条件の分離。 | Capture時機・分散・候補打切りの実装、routing context生成・更新の方式、到達性確認方式。 |
| 重複防止の保持 | 試行と作用・判断・実対象・段階・確定度・保留の対応を保持・区別し、再起動・移動・再接続を越えてreplayしない。 | 識別子・idempotency表現・保存単位・DB schema・repository・保持期間・永続化を必要とする未完了状態の範囲。 |
| 記録・報告・監査 | 元事実の確定度・privacy・順序の意味と管理到達性を維持する。本文の別保管庫を作らない。 | audit format・保持期間・提示確認・要約粒度・UI layout・診断stack。 |
| 消費・縮退・費用 | 並列・処理中・不明を同じ上限へ反映し、制御・保全を維持する。 | 費用予約・集計期間・推定、資源配分、反復抑制の機構。 |

crate／module、Rust trait／type、concrete API・error型、middleware・interceptor・hook、event bus／queue／actor、IPC format、DB schema、transaction／lock、具体的Credential保護・sandbox・Plugin隔離、特定library・SDK・OS API、session token・lease・heartbeat・epoch・sequence numberの具体形式、timeout／reconnect interval値、具体retry algorithm、UI animation・audio buffering・capture intervalも固定しない。上表の対応関係から統一Context layer、Policy Engine、Manager、Service、Coordinatorの追加を導かない。既存の12責務、semantic owner、Host／Client配置とtrust boundaryの下で実現方法を選ぶ。

切替調停・検知・区切りの具体mechanism、再接続・復旧の待機方式、戻り先選択、到達性確認方式等は下位設計の自由度として残る。必要な確認・Owner判断の省略、現在同意の拡張、未完了・不明の成功扱い、外部作用rollback・exactly-once実行の新保証はいずれも自由度に含めない。
