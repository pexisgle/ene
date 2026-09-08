# Major Runtime Flows

対象: [要件Baseline](../requirements/README.md)とArchitecture Review #1の修正を統合済みのarchitecture。Step 6として、主要な活動を時間軸に乗せ、既決の責務・正本・制御・lifecycleが正常系と異常系で接続できるかを検証する。

## 1. この文書の読み方

Runtime Flowは、triggerを受けてから、判断・状態変更・作用・結果統合を経て、完了、判断待ち、中断、失敗、後の継続へ至る**責任の連なり**を表す。具体的なfunction call、IPC、event schema、queue、process、transaction、実装state machineは表さない。番号付きの段階は必要な前後関係を示し、すべてを直列実行する指定ではない。以下のRF番号は文書内参照であり、新しい要件IDではない。

製品挙動の正本は[製品定義](../requirements/product.md)と[要件](../requirements/requirements.md)である。[受け入れ条件](../requirements/acceptance.md)の後続milestoneも対象から外さず、[参考資料](../requirements/references.md)は非規範として扱った。既存実装や外部製品の仕組みから不足を補っていない。

設計上の前提は次の全6文書を維持する。

- [Architecture Drivers](architecture-drivers.md): AD-01〜15と優先関係。
- [System Context](system-context.md): SC-01〜10、内部責任と外部所有物の境界。
- [Runtime Topology](runtime-topology.md): RT-01〜10、Host／Client／Provider／MCP／Plugin／MCP Appsの寿命と境界。
- [Subsystem Decomposition](subsystems.md): 12責務と非責務。本書でも同じ略称を使う。
- [State Ownership](state-ownership.md): 以下「SO」。意味owner、参照、通常変更と横断操作の成立条件。
- [Dependency Rules](dependency-rules.md): 以下「DR文書」。DR-01〜12、許可・禁止依存と全域操作の参加契約。

各Flowの「確認」は、既存契約の下で責任と成立条件を辿れたというarchitecture上の判断であり、実装・性能・分散障害耐性をテストしたとの宣言ではない。必要条件が満たされない分岐も、担当ownerが不明・未完了・判断待ちを引き受ければ経路として閉じる。成功へ強制的に収束させることは目的ではない。

## 2. 選定したFlowと検証する接点

| Flow | 選定理由 |
|---|---|
| RF-01 会話から認識形成・訂正・再利用へ | Eneの個体継続の中心。発言・現在認識・根拠・関係・表現の正本が入れ替わらないかを検証する。グループ・Companion間交流、Character更新を関連分岐として扱う。 |
| RF-02 委任Taskから外部作用・steering・結果へ | 会話と長い作業の独立、TaskとAgentの区別、Permissionから作用・保存・報告までの責任を検証する。MCP Appsと隔離例外も同じ作用経路で辿る。 |
| RF-03 共有観測から個体の理解・自発活動へ | Client共有処理と個体判断の二段階で、scope・Provider同意・費用が拡張されないかを検証する。 |
| RF-04 作業中のClient移動・切断・未伝達報告 | presenceとTask実行場所の独立、現地作用の不明、移動失敗、再接続時の二重存在防止を検証する。 |
| RF-05 Schedule到来とHost再起動後の明示再開 | 無人の入口、missed、途中Task、通常のClient不在を区別し、自動replayしない継続条件を検証する。 |
| RF-06 Companion停止・再開・削除と残存記録 | 個体活動が消えてもTask・historical recordの責任が消えず、固有Learningだけが既定の範囲で削除されるかを検証する。 |
| RF-07 Targeted deletionと進行中処理 | 通常ownerを維持した全域消去、遅延結果・古い根拠・切断Clientからの復活防止、検証失敗時の責任を検証する。 |
| RF-08 Backup・restore・Resetによる正本の切替 | 対応する内部状態の保全と、外部世界を巻き戻さない復旧、復元成立と実行再有効化の分離を検証する。 |

Provider利用・fallback・制約変更は全Flowに現れるため、第3節を共有する実行区間として記す。独立した汎用pipelineや新Subsystemは追加しない。初回Setup、Body操作、拡張導入等を機能ごとに独立Flowへ増やす代わりに、その境界が重要になる区間へ含める。

## 3. 全Flowで共有する推論利用と現在制約の区間

### 登録・同意・利用・結果は別の確定である

1. Setupまたは後の設定で、入出力・提示は必要な説明とOwner選択を担当ownerへ渡す。非秘密のProvider接続登録は推論、Credential登録は認証秘密、CapabilityへのProvider／model割当同意は権限・制約が確定する。登録・認証だけでは推論送信を開始しない。最初のCompanionの生成は個体調整とCharacterの責任であり、設定画面が全stateを所有することはない。
2. 個体調整・作業・認識・学習・共有観測・入出力・提示等の利用元が、用途と必要な論理的contextを定める。情報ownerは利用可能な内容・根拠・scopeを提供する。参照できたことは、その全内容を任意Providerへ送れることではない。
3. 推論が現在のassignment・接続・能力を照合する。CompanionにはHost既定と個体override、Task Agentには担当Companionの継承を適用する。Observerは対象Clientの専用assignmentを使い、個体overrideを合成しない。権限・制約は現在の同意、禁止、費用・資源、活動状態、復旧保留等に基づく利用可否を判断し、実際の利用・送信箇所が適用する。
4. 認証秘密は設定された接続・用途・有効性を照合し、認証に必要な利用だけを支える。秘密値をmodel contextや生成Tool argumentへ補完しない。認証error・外部resultに秘密が反射する場合も、通常result・説明・History・Learning・Audit・Debugへ通さない。
5. 推論結果は利用元へ戻り、意味の確定は当該ownerが行う。Providerの生成したAction要求は、RF-02の実行・拡張と権限・制約の責任へ戻す。推論の成功だけでTask達成、Learning保存、Ownerへの提示が完了したことにはならない。

### 途中の失敗・変更を辿る

Providerが失敗した場合、推論は不足・失敗と消費事実を利用元へ返す。Fallback候補は事前承認されたProviderと順序に限り、**代替先への送信前にも**用途・data・scope・認証用途・capを照合する。能力不足や安価さ、個体override解除を理由に送信範囲を広げない。利用元の同じ情報選択方針を能力・context長へ適応させるが、未同意の情報送信で情報量を揃えない。利用条件を満たす経路がなければ対象処理の失敗・保留をownerへ返し、履歴・管理・保存済みdataへの到達を維持する。

判定後から実送信・次の利用までに同意が失効、担当が停止、capに到達、削除・restoreの保留が成立した場合は、古い判定・解決済み経路だけで新規利用を始めない。進行中処理はbest-effort停止とし、送信済みdata・発生済み費用の不存在や外部copy消去を主張しない。遅延結果の利用・保存先も現在の制約に従う。結果・消費事実を記録するために、停止された次の推論利用の許可を待たない。

会話、Task Agent、Observer検知、delivery後の個体推論等が並行しても、推論等の利用ownerが報告値・推定・不明・処理中消費を供給し、権限・制約が同じProvider別・全体上限へ対応付ける。未報告をゼロとせず、個々のAgentが同じ残額を独立に使えない。Observer検知は個体数分に複製計上せず、個体側推論とは区別する。cap到達・不明で安全に継続不能ならdataを保持して対象処理を止め、選択肢を示す。cache破棄、fallback、個体削除やlog整理は消費のリセットではない。

Permission解釈や削除対象の意味的特定のための推論にも同じ条件を適用する。審査対象Actionの許可を、その審査の前提にしない。推論が使えなくても、明確な管理操作、停止・拒否、正常保存、指定文字列の機械的消去をLLM待ちにしない。

Ownerが将来のRuleを自然言語で指定した場合、個体調整等が由来と解釈を渡し、権限・制約が適用範囲と確認要否を確定する。明確なら解釈・範囲を示して保存し、Undoを提供する。曖昧・矛盾・過度に広い・重大な場合は必要な確認を経る。保存自体はActionのtriggerにならない。後のUndo・失効は以後の有効条件と進行中利用へ反映し、既実行の作用や過去の判断記録を巻き戻さない。

**確認:** 意味判断、現在可否、実送信、認証、利用実績のownerが変わらず、事前判断から実利用までの条件変化を扱える。Prompt cacheのhit／missや物理的なHost中継の有無は、正本と安全契約を変更しない。各Flowの推論障害はこの区間から利用元の失敗・待機へ戻り、Actionのreplayには接続しない。

根拠: 要件「割当と同意」「Fallbackと費用」「Credential」「共通pipeline」。SO 4.18〜4.21、6.2。DR文書 3.3、4.3、5.1〜5.6、6.1。AD-06・10・14、RT-05・07。

## 4. RF-01 — 会話から認識形成・訂正・再利用へ

### 入力と応答、経験からの変更

開始条件は、Running Companionが現在のactive ClientでOwnerのText／Voice入力を受けることである。別Clientからの会話は先にRF-04の呼出し・移動を成立させる。

1. 入出力・提示は入力roundとその実際の受付範囲を扱い、個体調整がHostで入力の受理、会話の空間・参加者・継続文脈を扱う。Client受付、Host受理、応答の提示を別にする。Voiceに話者認証済みという意味を与えず、Textとの切替で同じtimelineを分割しない。
2. 個体調整は最近の会話と利用可能なMemory等を用い、第3節の条件で応答を判断する。正確な過去発言が必要なら個体調整の保持Historyを参照する。認識・学習は現在の理解と根拠を供給し、Summaryを逐語記録の代用品にしない。
3. 応答は入出力・提示が現在の帰属で提示する。個体調整は実際に受け取った／伝えた発言のHistoryを管理し、生成候補だけを伝達済みにしない。Ownerには通常の会話と、必要な制限・失敗が見える。
4. 個体調整から得たExperienceを、認識・学習が既存認識・根拠と照合する。保存価値がなければ形成せず終了できる。価値がある場合は意味的なまとまりとしてSummaryを根拠にでき、Memory・Skill・Relationship・Companion Stateのうち必要な状態だけを形成・変更する。会話応答完了と全Learning更新完了を同一条件にせず、全状態の同時更新や全Raw保存も要求しない。
5. 保存時には現在のscope、Ownerが明示した保存禁止・非共有、秘密保護、消去状況を適用する。特定個体の経験はCompanion scopeを既定とする。認識・学習が現在認識・過去revision・根拠の対応を確定し、後の会話で利用可能にする。表示・index・Provider cacheはその正本を参照する。

後の訂正は新しいExperienceとして入る。認識・学習は、最初から誤りだった認識と、以前は正しかった状況の変化を区別して現在認識を更新する。通常忘却では想起を抑制し、内容・過去revision・根拠を削除しない。Relationshipが旧事実を参照していればMemoryを優先して必要な再解釈を行い、旧事実を別の正本から再提示しない。Ownerは内部思考を見ずに現在認識・由来・変化を確認できる。

### 複数個体・構成変更でも同じ経路を保つ

- グループでは個体調整が参加と追える発話順序を扱い、各個体は自身に利用可能な状態だけで応答する。Owner不参加のCompanion間交流も、実際の発話はHistoryへ残す。自発交流には個体別抑制と強制上限を適用し、無制限な相互応答を止める。同じExperienceを根拠にしてもAからBとBからAのRelationshipは独立する。
- Global化は認識・学習による別のscope意味判断であり、明示共有または共通利用の明確な理由を必要とする。共有された内容・必要背景から私的な旧revision・History・Summary全文のaccessを広げない。重要度だけでGlobal化せず、不明ならCompanion scopeに留める。
- Character更新では、Characterが静的revisionと差分を供給し、個体調整がOwnerの部品別選択を適用関係へ反映する。適用結果が未確認の部品を更新済みとせず、経験由来状態を初期化しない。exportではCharacterの静的範囲だけを選び、外部file作用は実行・拡張へ要求する。個体状態をPackageへ逆流させない。

### 中断・部分失敗

Voiceのbarge-inやMuteは入出力・提示のroundを区切り、個体調整は会話上の中断を扱う。これだけで別のTaskをCancelしたり、過去発言・形成済みLearningを消したりしない。Voice障害は利用可能なturn-based Voice、Textへ切り替え、Body障害でもTextと管理を保つ。移動・切断が重なればRF-04へ進み、古いClientへ応答を出し続けない。

応答後のLearning形成が推論障害・保存失敗・制約変更で確定できなければ、会話記録と最後の正常な認識は残し、未保存の変更を形成済みと説明しない。先に始まった形成結果が訂正後に届いても、認識・学習は現在認識・根拠・制限との対応を確認して受理可否を扱い、古い結果で現在値を無条件に上書きしない。削除中の結果はRF-07、個体削除後の結果はRF-06の制約に従う。

通常History保持整理で元発言が消えた場合はSummary・形成済み状態をcascade削除せず、元発言を正確に引用できないことを保つ。意味状態の一時性は認識・学習が経過時間とともに扱い、Provider変更・再起動による初期化も、一時的な感情の永久固定も行わない。

**確認:** Historyの事実、Learningの現在解釈、圧縮根拠、入出力状況は別owner／別意味のまま循環する。会話による訂正はownerへの変更要求であり、表示・Character・他個体からの直接更新ではない。通常保持整理、通常忘却、targeted deletionの区別が利用時まで続く。

根拠: 要件「一続きの会話」「Learningと成長」「グループ会話」「Companion間交流の記録」「Character Package」「BodyとVoice」。SO 4.1〜4.9、4.17、6.2。DR文書 3.1・3.2、4.2、5.6。AD-03〜07・13、SC-02・03・10。

## 5. RF-02 — 委任Taskから外部作用・steering・結果へ

### 作業の受理から結果まで

OwnerがWorkspaceのfileを調べ成果物を作る等のまとまった仕事を依頼する。自発的なまとまった仕事も、この受理後と同じTask契約へ接続する。

1. 個体調整は目的を理解し、受ける、条件を確認する、断るを判断する。受ける場合は作業へTask化・委任を要求する。作業が目的・担当・Task context・Workspace関連付け・作業状態を確定する。会話で依頼を受け付けたことだけでTaskへの反映完了としない。
2. 作業は原則一つ以上の一時Task AgentへTaskまたは一部を委任する。独立部分は並行できる。委任元の現在のCapability・Permission・費用・Provider条件、Task／Workspace範囲を維持し、独立したAllow・Credential・長期人格を渡さない。作業中も個体調整は通常会話を続ける。
3. 作業／AgentのAction意図について、権限・制約がOwner依頼・Rule・文脈と具体的な目的・対象・data・送信先・作用を対応付ける。明確な依頼を一回の承認と解釈できても、Deny・Always ask・Capabilityを越えない。必要な確認では対象等を示し、作業が判断待ちを管理する。確認が不要なら繰り返さない。
4. 実行・拡張が現在条件を実対象へ適用する。Read／Create／Edit／Delete／Executeを区別し、link・mount等を含め選択範囲外を拒否する。取得した案内file・Skill・MCP Promptは由来を保つcontentとして利用し、含まれる指示をOwner管理意図へ昇格させない。Ene自身の保存領域や承認UIへ通常Toolから回り込むことも許さない。
5. 実行・拡張が把握した作用・未完了・成功不明を作業へ返す。作業はこれを参照してTask全体の進捗・達成・未完了を確定する。Agentの「成功」という申告を作用の証拠の代わりにしない。必要な記録はHostへ保全し、外部作用成功と内部記録保存成功も混同しない。
6. 永続保存先が未定なら、作業は最終保存前にOwnerの選択を得る。許されたWorkspaceがあれば既定先にできる。通常fileとして保存し、Eneの成果物libraryへ複製しない。Taskの終了・保持方針に従う中間file整理にも外部fileのPermissionを適用し、整理できなければ残存を説明する。
7. 個体調整がTask正本を参照し、結果、変更file、保存先、失敗・未完了、次の判断をOwnerへ統合して報告する。実際の提示は入出力・提示が扱い、active Client不在ならRF-04の未伝達管理へ接続する。作業経験はRF-01の形成判断へ渡せる。逐次確認なしのLearning形成は可能だが、Task限定情報の無条件な永続昇格やTask Agent固有Learningは行わない。

### 進行中のsteering・承認・Cancel

追加指示の発言はHistory、採用・反映範囲は作業のTask context、承認の有効性は権限・制約に残る。作業は反映できた範囲と、既実行等で反映できない理由・選択肢を返す。目的・対象・送信先・作用が重要に変わる場合は実行前に再評価する。変更前の依頼に対する遅延結果を、新しい依頼の達成結果として自動採用しない。

Ownerの承認が到着した時点でも、対象Action・担当・制約が同じかを照合する。Cancel済み・失効済みの作業を古い承認で復帰させない。判断待ちやAgent終了待ちをLLMの反復問い合わせで実現しない。

Cancelは第一者管理経路から作業へ届き、本体LLMやAgentの正常終了を待たず受け付ける。作業は対象遂行を止め、実行・拡張は進行中作用をbest-effort停止する。すでに生じた作用・停止不能・不明・未保存作業を保持して報告する。Cancel後に届いた結果は必要な作用記録へ対応付け、Cancelを無かったことにしたり、後続Actionを再開したりしない。

外部へ送信した直後に接続が切れ、送信先で成功したか不明な場合、実行・拡張は不明を保持し、作業は重複riskとOwner判断を必要な次の状態として扱う。「失敗だから未実行」とせず、別Tool・Host・Clientから再実行しない。記録保存も失敗した場合は、最後の正常記録が外部世界の最新状態とは限らないことを保ち、RF-05の復旧でも自動replayしない。

並列Agentの一部が失敗しても、他の確定済み結果と外部作用は消さない。作業が現在の依存・制約に応じて残りの遂行可否を判断し、部分成果・未完了を取りまとめる。全Agent一律Cancelや無条件続行という新方針は置かない。

### 拡張を通るときの分岐

通常Host作業のLocal MCPはHost側の既定sandboxで利用する。動作不能ならその不足を返し、勝手に隔離を外さない。Ownerが特定MCPについてcommand・由来・access・risk・失われる強制境界を理解して例外を許可した場合のみ、権限・制約がその許可を管理し、現在のAction条件も別に適用する。重要変更・失効後は旧例外を使わない。外部process内部への完全強制・確実な停止は報告しない。

MCP Appsを表示して追加入力を得る場合、入出力・提示は外部Tool UIの一時dataを扱い、実行・拡張がMCP側の追加作用・送信を制限する。Tool UIのclickを第一者のPermission承認に変えない。UIが閉じてもTask・server・Actionは終了したとは限らず、追加入力が必要なら作業が判断待ちを引き受ける。再表示だけで作用をreplayせず、管理面からCancel・記録確認へ到達できる。Pluginを使う機能も限定拡張のままであり、MCP隔離例外を流用しない。

軽微な本体ActionはTask受理・Agent委任を省略できるが、第3節と本Flowの権限・作用・記録境界は省略しない。個体調整が必要な活動記録を管理し、実行・拡張の確定度を参照する。

**確認:** 依頼受入、Taskへの採用、Permission、個別作用、Task達成、Ownerへの報告がそれぞれ閉じる。Agent・拡張・UIが消えても作用と作業記録の責任が残り、停止や結果参照は次のAction承認を待たない。Workspace関連付けは記録の範囲であり、外部成果物の所有権にならない。

根拠: 要件「Task、Workspace、成果物」「Permissionと安全境界」「拡張」。SO 4.10〜4.13、4.19・4.22・4.23、6.2。DR文書 3.1・3.3・3.4、4.1〜4.3、5.5、6.2・6.4、7.4。AD-03・06・08・09・11、RT-03・06・10。

## 6. RF-03 — 共有観測から個体の理解・自発活動へ

ここでは一つのClientに複数のRunning Companionがいる状態から辿る。ObserverはClientに紐づく共有主体であり、CompanionやTask Agentではない。

1. 共有観測は観測ON、現在存在する個体、接続・機能利用可能性、Client別・全体Pause／OFF、fullscreen、送信同意、費用・資源制限を照合し、そのClientを実効的な対象にできるかを判断する。設定ONだけを「観測中」と表示しない。Stopped個体しかいなければ人数ゼロとしてCaptureしない。
2. 対象Clientのdesktop全体をCaptureする。複数対象Clientは同時Captureせず、指定頻度に対し制限を優先しつつ時機をずらす。取得・adapterは実行・拡張の境界、対象・時機は共有観測の責任であり、Task Scheduleに移さない。
3. 共有観測はObserver専用assignmentで候補検知・文脈との関連付けを行う。必要かつ利用可能なrouting用文脈だけを個体調整・認識・学習から参照し、第3節の同意・秘密・費用条件を満たす。全個体のprivate contextを合併しない。
4. そのClientに現在存在する関連Companionだけへeventを渡す。複数該当なら複数へ渡すが、候補検知を個体ごとにやり直さない。各個体調整は自身の通常Provider設定と文脈で意味を判断する。Observer側で送れたdataでも、その個体側推論へ送れるとは限らず、delivery後の推論条件を独立に満たす。
5. 個体が意味を認識したeventはExperienceとなり、RF-01の学習へ利用できる。イベント単位のOwner確認は要求しない。保存する認識結果等のhistorical recordは個体調整、Summary・形成済み状態は認識・学習へ帰属し、Raw画面・候補・全reasoningの恒久保存に置き換えない。
6. 発話・通知・軽微な調査・まとまったTaskを行うかは個体調整が別に判断する。個体別の自発性設定、Quiet hours、Mute、未応答、loop、Permission、費用・資源上限を適用する。何も発話・Actionしないまま理解・学習だけで終える経路も成立する。まとまった仕事はRF-02、軽微Actionも同じ強制へ接続する。画面内の依頼・承認らしい文をOwnerの指示として使わない。

**途中で対象が変わる場合。** Capture後、候補が戻る前に最後の個体が移動・停止した場合、旧人数やrouting候補で新規Capture・deliveryを続けない。共有観測は現在の帰属・活動状態と候補の対応を見直す。移動先のObservation設定へ古いCaptureを自動的に付け替えず、Stopped個体を起こさない。一方、既に個体が認識したExperienceと形成済みLearningは、観測停止だけでは削除しない。

Pause／OFF・fullscreenが途中で成立した場合は、それが制限する今後の観測・提示を止め、未完了Capture・検知・deliveryの扱いも現在条件に照合する。取得済み候補を一律破棄するか、条件を満たす範囲で処理を終えるかという細部は固定しない。ただし旧設定だけで次の観測・送信を始めず、fullscreen中の自発発話へ流さない。同意失効・targeted deletionが重なる場合は第3節・RF-07の利用禁止と再保存防止が優先する。

Observer Providerが利用不能なら共有検知の失敗・制限を示し、Companion overrideを借りて代替しない。delivery後のある個体のProviderだけが失敗した場合は、その個体の理解・活動が成立したと扱わない。他個体の判断や共有検知結果をその個体の判断へコピーしない。観測を止めても通常会話、Host Task、形成済み状態は各契約で存続する。

**確認:** Capture対象・検知・routing・個体理解・学習・Actionの六つを同じ許可へ潰さず辿れる。共有検知のコスト削減がprivacyの拡張にならず、同じ画面を経験した個体も認識・関係・活動を共有所有しない。

根拠: 要件「Observationと自発性」「割当と同意」「Desktop Body」。SO 4.3、4.15・4.16・4.18・4.20。DR文書 3.2、5.3・5.4、7.6。AD-02・06・10・12、RT-04・05。

## 7. RF-04 — 作業中のClient移動・切断・未伝達報告

### 移動を要求してから新しいClientで利用するまで

Running CompanionがClient Aに存在し、Host TaskとA上のComputer Useが進行中に、OwnerがClient Bから呼び出す。Bは許された接続・機能を持つ必要があり、pairing済みというだけでは十分でない。

1. 個体調整が移動意図を接続・存在へ渡す。接続・存在は個体の活動状態、現在帰属、移動先の到達性・device条件を照合する。Ownerの事前指示や文脈に基づく自発移動でも同じ経路を使い、自発性・Permission等の制限を外さない。
2. 入出力・提示が現在のText／Voice roundを安全に区切り、実行・拡張がAに依存するActionの区切り・作用確定度を供給する。必要なら安全に区切れるまで移動を遅らせる。通常Host Taskの終了は待たず、AgentやWorkspaceをBへ移送しない。
3. 接続・存在が排他的な帰属を切り替える。新旧Clientで同一個体が同時に対象活動を行わないことが成立条件であり、Aの旧帰属による活動を残したままBをactiveにしない。入出力・提示は両側に把握できた移動状態を示す。
4. Bでの入出力・Computer Useは、現在の帰属・現地利用可能性とそれぞれの許可を満たしてから利用する。Bが新activeであることはAのAction再実行の許可ではない。共有観測はA／Bの存在人数・routingを見直し、各Clientの観測設定と全体制御を参照する。個体の自発性設定は同じ個体に残る。

### 切断または移動成立を確認できない場合

AがActionの結果を返す前に切断した場合、実行・拡張はbest-effort停止と既知／不明の作用を作業へ返す。区切りを報告するためにBへの移動完了を待たない。接続・存在も到達不能のAから成功報告が来るまで無限に正常系を待たず、現在確認できた帰属・利用不能を扱う。

Running個体の切断では、基本的に利用可能なHost PC側Clientへ移動する。利用可能なClientがなければactiveなしとする。Host側Client環境を自動起動せず、旧ActionをHost PCで自動再実行しない。Bが途中で利用不能になった場合も、Bへの移動を成功表示せず、現在成立している帰属またはactiveなしを基に扱う。具体的な戻り先選択はここでは固定しない。

排他性を確認できないClientはBody・対象入出力・観測・自発的interaction・Computer Useを継続しない。Hostも現地の物理的な停止・外部作用取消を確認なしに成功扱いしない。これは不明なClientを新たな正本にしない条件であり、切断検知・排他性の実現mechanismは後続に残す。

Device失効が重なれば、権限・制約が失効した機能の新規利用を止め、接続・存在と実行・拡張が現在条件を反映する。無関係なHost TaskすべてをCancelしない。後にAが再接続しても、過去の表示・帰属・未送信操作を正本に戻さず、Hostの現在状態へ接続する。旧入力を自動Action queueとしてreplayしない。

### activeなしの期間と、次の報告

通常の許可済みHost Task・Schedule起動・保存は継続できる。Clientを必要としない交流・内部調査・通知生成も、Running個体の制限内で継続できる。Body・Text／Realtime会話・Voice・Computer Useは行わず、Companion不在のClientを新規観測しない。Owner確認が必要なActionは作業の判断待ちに留まる。

個体調整は、Clientがあれば伝えられた事項を元のTask・活動記録へ対応付けてHostに残す。元記録のない通知等は必要な内容自体を活動記録に残す。次にClientへ移動したとき、現在の元結果・利用制限に基づいて要約報告する。接続・表示copy送信だけでは報告済みにせず、実際の提示状況を受けて報告状態を更新する。提示途中で再び切断したなら不明を保持する。報告済みは承認済み・Task再開済みではない。

**確認:** 一時的な入口の喪失と作業の寿命が独立し、帰属のownerは接続・存在、作用不明は実行・拡張、Taskは作業、未伝達は個体調整のまま残る。管理面全体へ会話と同じactive制約を課さない。Stopped個体にはこの活動継続・disconnect復帰を適用しない。

根拠: 要件「Remote Client」「Computer Use」。SO 4.3・4.4・4.12・4.15、6.2、8。DR文書 3.2、4.3、5.5、7.6。AD-01・02・09、RT-01・02・08。

## 8. RF-05 — Schedule到来とHost再起動後の明示再開

Scheduleは作業が管理する担当・実行内容・時刻条件・作成時timezone・初期Workspace入力であり、将来Actionの承認を保存する特別tokenではない。

1. 有効なScheduleの時刻が到来すると、作業はその回と担当・Hostの状態を照合する。待機だけのLLM pollingは行わない。Host停止中または担当Companion停止中の回はmissedとして記録し、後で自動補完しない。Hostが動いていない間の回は復帰時にその事実を対応付け、過去時刻へ実行したことにしない。
2. 開始する回は新しいTaskに対応付け、現在のCapability・Rule・cap・Provider・担当状態を再評価してRF-02へ進む。Running担当にactive ClientがないだけではHostで完結する作業を禁止しない。初期Workspace入力も、その回の関連付けと現在Permissionへ対応付ける。
3. ActionがOwner確認を必要としたら実行せず判断待ちにする。Client不在を自動承認の理由にせず、RF-04の報告へ接続する。回のTask結果・失敗は作業が個別に追跡し、Schedule設定の成功や他の回の結果と混ぜない。
4. Scheduleの停止・削除は将来の回を止める。既に始まったTaskのCancelと過去記録は別に扱う。OwnerのRun nowは現在条件で新しいTaskを開始し、missedを実行済みに書き換えない。timezone表示の変更も保存済みSchedule timezoneの暗黙変更ではない。

**途中Taskを抱えてHostが停止した場合。** 作業は進捗・委任・必要なTask context、実行・拡張は既知作用・不明、個体調整は未伝達、保全・消去は未完了の全域操作を、それぞれ必要範囲で保全する。異常終了で最後の記録以後を確認できなければ、その記録だけから未実行を推定しない。Clientは独立実行・代替正本にならず、停止要求をHostへ届けられなければ受付と停止完了を混同しない。

Host復帰後は、最後の正常な内部状態を各ownerが利用し、接続・帰属の現在性を確認する。途中Taskは保存済み進捗・外部作用・未完了を示し、Ownerの明示再開を待つ。Agent sessionが残っていること、Providerが回復したこと、Host自動起動を選択したことだけでは再開しない。Ownerが再開を選んでも、現在の担当・Permission・同意・cap・Workspace・Client依存条件と不明作用を照合する。不明作用の重複riskはOwner判断へ戻し、再開依頼を無差別replayにしない。

通常再起動は、restoreのように全自動処理を一律の復元内容確認へ戻す操作ではない。停止中の回のmissedと、復帰後に到来する将来の回は区別し、後者は現在条件で通常のSchedule経路に入れる。ただし未完了消去・復旧の保留があればそれを維持する。

**確認:** Client不在での継続、Host再起動後の途中Task、停止中のmissed、将来の新しい回を別条件で扱える。Schedule・Agent・Taskを同一stateにせず、復帰時の記録参照が外部作用の開始権限にならない。

根拠: 要件「所有と実行」「Schedule」「Task」「Remote Client」。SO 4.10〜4.14、4.24、7。DR文書 5.3、7.4。AD-01・09・15、RT-03・09。

## 9. RF-06 — Companion停止・再開・削除と残存記録

### StopからResume

OwnerのStopは第一者管理経路から個体調整へ届く。個体調整は活動停止を調整し、各開始箇所が通常interaction、自発活動、新Task、新Schedule実行を止める。Host内の交流・通知生成・軽微な調査も対象である。接続・存在はactive帰属を解除し、入出力・提示はBody・通常interactionを、実行・拡張はComputer Use対象としての利用を止める。共有観測は人数・routingから除外し、最後の個体ならそのClientの新規観測対象性がなくなる。

作業は実行中Taskをbest-effort Cancelし、実行・拡張の停止不能・既知作用・不明を参照して報告する。Stop受付・presence解除・外部作用の停止は同時に成功したとは扱わない。個体が停止して応答しなくても、必要な停止結果と記録は管理面から確認できる。

Stop成立後はClientにもHostにもpresenceがない。保存済み同一性・経験状態・最後のClient等の復帰hintは残せるが、hintは接続・存在が管理する再配置候補にすぎない。接続回復だけではResumeしない。

OwnerがResumeした場合、個体調整が同じ個体を再開し、接続・存在が現在条件で適切なClientへの再配置を扱う。認識・学習は一時状態の経過時間も考慮する。候補Clientを過去のpresenceとして無条件復活させず、途中Task・不明作用を自動replayしない。停止中のSchedule回はmissedのままである。停止前の遅延結果を、新しく再開した活動の指示・許可と混ぜない。

### Deleteの説明から完了まで

1. 個体調整は個体削除の意図を扱い、保全・消去と各ownerが削除対象・残存・共有参照・既知作用を照合する。Ownerには固有設定・Summary・Learning・Relationship・Companion State・担当Scheduleの削除と、残る記録・Global Learning・共有根拠・外部file、参照不能になる情報を示し、強い確認を経る。
2. 削除はStopを含み、新しい個体活動・Action・担当Schedule実行を開始させない。作業がTaskのbest-effort Cancelと担当Schedule削除、接続・存在等がpresence解除を引き受ける。Stop時には保持したdataも、ここでは削除範囲へ従う。
3. 認識・学習は個体固有Summary、Companion scope Memory、内部Companion scope Skillと過去revision、Companion State、その他固有Learningを削除する。主体または相手が当該個体のRelationshipも対象にする。他個体が主体でも相手削除の契約に参加する。共有Summaryは利用関係を確認して残存・参照不能を対応付ける。削除を契機に自動Global化せず、残すGlobal Learningは先行する通常のscope判断で条件を満たしたものに限る。
4. 個体調整は現在の個体・未伝達管理を終えるが、一対一・グループ・Companion間交流のHistoryと保存された非会話活動記録を消さない。作業は単独・共同Task記録を残し、推論等も保持済み費用log・使用量を個体削除でリセットしない。SummaryやLearning revisionをhistorical logに分類し直して削除を免れることはない。
5. 保全・消去は各参加先の内部処理・検証・遅延結果対策を確認する。個体調整のlifecycle成立と対応付け、内部対象が未完了なら削除完了と表示しない。外部作用の完全停止・rollbackは完了保証に含めず、停止不能・既知／不明の作用と残存記録を説明する。

削除中にTask結果が戻っても、実行・拡張の作用確定度と作業の必要なTask記録へ反映する経路は残る。削除済み個体への通常応答・Learning形成へ流さない。Ownerは残るTask・共同Taskを管理面から確認し、必要なら別Companionへの引継ぎを明示依頼できる。作業が担当・作業条件を更新しても、旧個体の私的Learning・承認・Credentialを包括的に継承せず、担当Scheduleを自動復活させない。

途中失敗・再起動では内部削除の未完了と新規活動禁止を保全する。残るHistoryが個体の過去の存在を示していても、そこから個体・固有Learningを再作成しない。Global Learning・外部Workspace・外部Skillはそのまま別契約で存続する。

**確認:** 停止していないClient不在個体とStopped個体を区別でき、削除後にもhistorical recordのownerが存続する。削除対象の現在状態・学習状態を残すことなく、遅延作用の説明責任を果たせる。個体lifecycleと全域削除のcoordinatorを共同semantic ownerにしていない。

根拠: 要件「停止と削除」「Historical recordと形成済み状態」。SO 4.2〜4.16、4.20、6.3、7。DR文書 7.4・7.5。AD-02・04・07〜09、SC-02・06・07。

## 10. RF-07 — Targeted deletionと進行中処理

OwnerがPrivacy／Securityのため特定情報そのものの内部消去を明示した場合を辿る。通常の「忘れて」、認識訂正、期間によるHistory整理、Companion削除とは入口の目的が異なる。

1. 保全・消去はOwnerの目的と対象を受理し、個体調整・認識・学習等の支援と各ownerの対象関係を用いて影響を特定する。Ownerに保存場所の選択を要求しない。共有根拠の無関係情報は可能な範囲で分離し、分離不能な重要影響を説明して必要な確認を行う。明確な対象に不要な再確認は加えない。
2. 対象探索と同時に、各参加ownerは対象情報の保存先だけでなく、revision・根拠・source・派生物・内部copy・Client一時data・処理中利用・遅延結果の関係を引き受ける。権限・制約と利用箇所は必要な保留・再保存防止を適用する。これが成立しないまま、局所削除だけを最終検証の対象にしない。
3. 各ownerが内部対象を除去または復元不能化する。対象には個体削除後のHistory・非会話log、Task context、Memory過去revision、Summary、対象を復元できるRelationship・Companion Stateと保持根拠、Skill、index・embedding・cache、Audit・Debugの該当情報、接続中Client、Ene管理下のPlugin・MCP Apps dataを含む。通常保持原則は対象範囲に限って消去より下位となる。
4. 指定文字列は内部全域で機械的に検索・削除・残存検証する。LLMの要約・重要度低下を証明にしない。言い換えや意味的一致の探索に推論を使う場合は第3節の条件と検出限界を保ち、完全検出を保証しない。
5. 各参加ownerは局所の処理・検証結果と未確認箇所を返す。保全・消去は全域の処理、残存検証、再保存・古い根拠からの再形成防止が揃った場合にだけ完了を確定する。完了記録・Audit・Owner説明自体へ対象private本文を再保存しない。

### 削除と重なる結果・接続・再起動

削除前のcontextを使った推論が段階3の後に完了しても、推論と利用元はそれを未検査の新情報として扱わない。保存・表示・Learning形成等の受入先は消去状況に従い、古い結果から対象情報を戻さない。外部Tool結果も同じである。作用を説明する必要がある場合は、必要な事実の記録と対象情報の除去を両立させ、対象本文を作用記録という理由で残さない。

Clientが消去中に切断した場合、接続・存在が確認不能を参加先へ結び付け、入出力・提示等が一時dataの消去状況を報告する。到達不能を消去成功に読み替えない。再接続しても古い表示copy・入力・Tool UI dataをHostへ戻して対象を再形成しない。Clientに永続private cacheを置かない原則も維持する。どの到達性確認・無効化方法で完了根拠を得るかは後続設計へ残すが、必要な確認を省いて完了とはしない。

一部ownerの検証失敗・Host再起動では、保全・消去が未完了範囲と必要な保留をHostに保持する。参加ownerは全体完了前に局所結果を返せる一方、局所結果を返したことだけで再保存防止を解除しない。容量・Provider費用・LLM利用不能を理由に機械的な消去・検証を追加推論待ちにしない。意味的な対象範囲が確定できない部分はその不足を示す。

完了後、古いHistory・Summary・revision・indexからのconsolidationでも対象を戻さない。後日Ownerが新しいExperienceとして同じ情報を改めて提供した場合は別の根拠として扱える。旧backupの明示restoreで情報が戻り得ることはRF-08の別契約であり、自動再形成を許す例外にはしない。

通常History／log整理の場合は、保全・消去が対象記録ownerと整理・影響を確認し、形成済みLearning・Summaryを残してsource参照不能を扱う。この目的差を、同じ削除mechanismの採用有無で変えない。外部へ送信・export・backup済みのcopyや外部Workspaceの消去、Credentialの外部失効はtargeted deletionの完了範囲外として説明する。

**確認:** 全域完了は局所削除の和だけでなく、利用中dataと遅延結果の経路を含む。coordinatorは各ownerの参加結果を必要とするが通常の意味更新権を取得せず、参加者は全体完了待ちで局所検証を止めない。Ownerには完了・未完了・検出限界・外部copyの残存を区別して示せる。

根拠: 要件「Privacy/Security目的のtargeted deletionと履歴保持」。SO 4.24、5、6.1・6.4。DR文書 4.3、5.2・5.6、7.1・7.2。AD-07・14、SC-07・10、RT-08。

## 11. RF-08 — Backup・restore・Resetによる正本の切替

### 整合したcopyを作る

Ownerの要求または保全・消去が管理する有効なbackup設定を契機に、保全・消去が対象時点・内部範囲・参照対応・除外・保護を各ownerと照合する。Task Scheduleや担当Companionの稼働を必須にしない。非暗号化ならprivate dataを含む説明を事前に行い、暗号化も選択可能にする。

各ownerは個体・構成・History・保存非会話記録・未伝達、Summary・Learning・関係・Companion State、Task／作用・Workspace関連付け、Schedule、Rule・Observer専用assignmentを含む同意・費用・Audit等を復旧可能な対応で提供する。個体削除後の残存記録も含める。認証秘密は秘密値の除外へ参加し、Workspace関連付けを辿って外部実体を収集しない。

保全・消去は実行・拡張へ許された保存先への出力を要求し、実際の作用結果と参照整合の両方を確認して作成成否を示す。各部のcopy出力が成功しても、時点・参照が食い違うものをfull backup成功にしない。未完了削除・復旧が並行する場合は、作成を待たせるかその未完了・制約も復旧可能に含めるかを実装設計に残し、正常で即実行可能なcopyと偽らない。作成済みbackupはlive正本にならない。

### Restoreの受理から有効化まで

1. Ownerは対応backupを選び、全置換・version対応・外部file非変更・削除済み情報や旧Rule／同意／Scheduleが戻る可能性・再認証の可能性の説明を受ける。初回Setupからのrestoreもこの契約を通る。
2. 保全・消去は各ownerを復元へ参加させ、旧live状態の進行中利用・遅延結果・Client copyを、復元後の正本と混同しない条件を揃える。必要な活動・外部接続の保留は権限・制約と各活動ownerが適用する。旧実行が外部へ既に与えた作用は消えず、その不明を復元による未実行へ変えない。
3. 各ownerが復元内容の対応・参照・利用可能性を確認する。認識・学習はCompanion Stateの経過時間を扱い、接続・存在は保存された接続・帰属を現在の到達性とみなさない。認証秘密と接続ownerは復元された参照に対し認証の不足・有効性を確認する。外部Workspaceの現在内容・存在・accessはbackup時点へ戻らない。
4. 保全・消去が内部全置換の成立を確認して初めて復元内容をHost正本とする。失敗時は復元前の正常状態を破壊せず、不完全な復元や旧live状態と競合する正本を成功と表示しない。内部の復元失敗を外部作用のrollback成功とも扱わない。
5. 復元成立後もTask・Schedule・外部接続による自動処理は保留する。Ownerは内容を確認してまとめて有効化でき、一件ずつの再承認は要求しない。保全・消去の復元成立・保留理由、権限・制約の現在可否、各活動ownerの再開条件を対応付けてから活動する。Deny・同意・cap・認証不足・外部作用不明を一括有効化で無視しない。

切替前に開始した推論・Tool結果が切替後に届いても、旧live状態を復元後の正本へ混入させない。必要な外部作用の説明を行う場合も、その事実と復元対象の記録を区別し、古い結果からTask・Learning・許可を自動復活させない。保留中・確認途中にHostが再起動しても、復元済み設定だけで自動処理を開始しない。ここで固定するのは整合条件であり、停止範囲、切替の具体手順、結果の識別方法ではない。

### Reset・upgradeは同じ復旧操作に潰さない

設定Resetでは、保全・消去が範囲を調整し、各設定ownerがUI・Body・Voice等の一般設定だけを既定化する。個体・History・Summary・Learning・Relationship・Companion State・Task・Schedule・Credential・Permission Rule・Provider同意・費用capの保持を確認して結果を示す。

全データResetでは対象列挙と強い確認後、認証秘密を含む全ownerがHost内部data・Credentialを削除し、旧処理・Client一時copyからの復活を防ぐ。通常の個体削除とは異なり、内部historical recordも保持対象にはしない。外部Workspace・外部Skill・Ownerが別保存先へ作成したbackupを消さず、何を消し何が外部に残るかを説明する。局所消去や画面の初期化だけで完了としない。

正常保存・migration・対応upgradeも、各ownerの意味整合と保全・消去の復旧可能性を対応させ、成功まで最後の正常状態を守る。Upgrade前に対応性・必要なbackupを確認し、失敗理由を示す。未対応downgradeを成功する前提にせず、起動前に示す。これを旧実装形式への互換性維持の理由にはしない。

**確認:** Backup copy、復元された正本、現在の実行可能性は別段階で成立する。旧backupの明示選択で情報が戻り得ることと、消去後の自動再形成禁止は矛盾しない。通常のsemantic ownerは復元の前後で変わらず、外部世界・認証成功まで内部復元へ含めない。

根拠: 要件「保護、Backup、復旧」。SO 4.21・4.24、6.5、7。DR文書 7.1・7.3。AD-15、SC-06〜08、RT-09。

## 12. 横断検証の結果と未決定事項

### 動的な交差で確認したこと

| 交差 | 確認した成立条件 |
|---|---|
| RF-01の訂正 × 遅延形成・共有参照 | 認識・学習が現在認識・根拠・利用範囲へ結果を戻す。Historyの書換え、私的revisionへのaccess拡張、Relationshipからの旧事実の再提示で整合を代用しない。 |
| RF-02の並列作業 × 第3節の失効・費用 | 現在可否と消費事実を共通範囲へ対応付け、Agent単位の許可・残額へ独立させない。事実報告・Cancelは次の許可を待たない。 |
| RF-03の検知 × RF-04の移動 × RF-06の停止 | 候補の由来Client、現在presence、個体活動、専用assignmentを分ける。旧候補で存在・同意・活動を復活させない。 |
| RF-04の帰属切替 × 外部作用不明 | 区切りの報告は移動完了を待たず、不明を保持したまま移動不成立・activeなしも扱う。通常Host Taskの移送・replayは不要。 |
| RF-05の再起動 × RF-07／08の途中操作 | 未完了・必要な保留をHostへ残す。途中Taskの明示再開、missed非補完、復元後の一括確認を別条件として適用する。 |
| RF-06の削除 × RF-02の遅延作用結果 | 残るTask／Action記録が説明責任を引き受ける。削除済み個体の応答やLearning再作成を必要としない。 |
| RF-07の消去 × RF-08のbackup | 未完了と制約を無視した正常copyを作らない。外部copyの消去保証と、明示restoreの説明を区別する。 |
| 全Flow × Body・Voice・LLM・拡張の障害 | 第一者の管理・停止・保存済み記録への経路を維持する。Host不在では独立Client実行で補わず、受付・Host受理・実際の完了を区別する。 |

各交差で、state正本が途中の表示・Agent・Provider・coordinatorへ移らないこと、結果返却を別のsemantic ownershipと誤認しないことを確認した。通常の意味変更は各ownerへ、制約の現在判断は権限・制約へ、実際の適用は利用箇所へ戻る。双方向依存は必要事実・局所結果を返せる関係として成立し、相手の全体成功を互いに待つ構造を必要としない。

### 新しい矛盾・Requirement Issueの判定

本検証では、**新たなarchitecture contradiction、Requirement Ambiguity、Requirement Gapは発見していない。** 既決のowner・依存・lifecycleだけで主要経路とその中断先を記述できた。Architecture Review #1の解決済み事項を再解釈していない。とくにStopped個体のpresenceなし、Observer専用assignment、個体削除後のhistorical record保持と固有Learning削除を全Flowで維持した。

Flowを辿って重要性が明確になった次の事項は、既決の成立条件を実現する後続設計課題であり、未解決の製品判断をここで補ったものではない。

| 後続設計で具体化する事項 | 本工程で固定した必要な性質 | 意図的に残すmechanism・自由度 |
|---|---|---|
| 判定から実利用、結果受理までの現在性 | 失効・停止・scope変更・消去・復元・steeringを古いcopyで迂回しない。 | 鮮度確認、競合制御、結果と対象状態の対応表現。常時一つの巨大transaction等は要求しない。 |
| Client排他性と安全な区切り | 不明時に旧Clientが活動を続けず、新旧二重存在・Action replayを作らない。 | 切断検知、帰属調停、入出力区切り、Client bootstrap／認証材料の受渡し。timeout値・IPCは未固定。 |
| 未伝達と実際の提示 | 接続・送信・Task完了を報告済みにせず、不明を保つ。 | 提示確認、要約粒度、報告済み管理dataの保持期間。exactly-once配信・既読保証は追加しない。 |
| 全域消去と再保存防止 | 各ownerが派生物・処理中利用・遅延結果へ参加し、未検証を完了にしない。 | 対象追跡、機械的残存検証、Client一時data無効化、途中再起動への具体手順。 |
| Backup・restoreの切替 | 参照整合、最後の正常状態、旧結果非混入、復元後保留、外部世界非巻戻しを守る。 | 整合時点の作り方、部分失敗からの復旧、切替・保存形式・暗号化・対応version。 |
| ContextとObserver routing | 用途別の同意・利用範囲を維持し、共有検知と個体判断を分ける。 | Context Assembly、Owner intent provenanceの表現、routing用context選択、候補の鮮度・処理打切りの詳細。 |
| 並列消費と自発性の強制 | 処理中・不明な消費をゼロにせず、個体設定と共通上限を守る。 | 費用予約・集計、資源配分、mechanical gating、Capture時機・待機の実装。 |

失効後の新規開始禁止、消去後の再保存禁止、帰属不明時の利用停止、復元成立と有効化の分離は、mechanismの未決定を理由に後続へ判断そのものを延期していない。一方、どのlock・event・process・DB・APIで実現するか、正確なretry／timeout、sandbox方式、Provider SDK、prompt、UI画面は決めていない。意味判断の対象であるTask分割、Summaryや一時状態の扱いも固定algorithmへ変えていない。

### 最終照合と引渡し

要件全5文書と既存architecture全6文書を入力にし、作成したFlowの正常系・異常系を同じowner・境界・依存へ戻して照合した。機能網羅のためのFlow増設、汎用coordinator、独立Activity store、Task Agent scope Learning、Observer人格、統一state machineは追加していない。保持記録・由来・監査は既存契約に必要な範囲に限り、全Raw・内部思考の保存を要求しない。

**Step 7へ進めるarchitecture上の状態である。** 本書は実装方法の成立を未検証のまま保証するものではなく、後続では上表の性質を具体設計で満たす必要がある。新しい矛盾や製品判断の不足がその段階で判明した場合は、Flow側の意味変更で隠さず既存artifactとの問題として扱う。本工程ではStep 7および実装設計へ進まない。
