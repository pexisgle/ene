# Backup / Restore — 復元範囲・正本切替・再有効化の詳細設計

本書は、BackupからEneの内部状態を復旧するときに、**何が復元され、何が現在環境から維持され、何が自動的には再有効化されず、どの時点で復元された内部stateを新しい正本として扱えるか**のlogical contractを詳細化する。後続のstate / persistence / concurrency / interface設計が安全に依存できる水準まで定め、具体的なmechanismは固定しない。

製品挙動のsource of truthは[要件Baseline](../requirements/README.md)、[製品定義](../requirements/product.md)、[要件](../requirements/requirements.md)とする。[受け入れ条件](../requirements/acceptance.md)も検証範囲へ含め、[参考資料](../requirements/references.md)は非規範として扱う。既存実装から製品挙動を補わない。

本書の「要求」「対応」「置換成立」「保留」「一括有効化」「旧live」「復元正本」は論理的な関係であり、共通object、永続record、protocol、state machine、enumを指定しない。番号付きの段階は必要な前後関係を示し、すべてを直列実行する指定ではない。

## 1. 詳細化する問題と範囲選定

Restoreを「backupの内容をDBへ上書きする」へ落とすと、次の成立が失われる。後続設計が誤りやすい点と、本書で定める契約は次のとおりである。

| 誤りやすい点 | 本書で定める契約 |
|---|---|
| 復元された設定・参照・Task・Scheduleの存在を、現在実行可能であることと同一視する。 | 復元範囲と現在有効性の分離、再有効化の別確認（第3〜4節）。 |
| 現在のCredential storeを過去へ巻き戻し、復元されたassignment / consentだけで送信・作用を開始する。 | secret除外と現在store維持、復元参照と現在Credentialの照合、自動利用の禁止（第3節）。 |
| 外部Workspace・Provider保有state・外部作用をbackup時点へ巻き戻す。 | 外部現実の非巻戻しとdangling参照の未解決扱い（第3〜4節）。 |
| 完了済み・結果不明のexternal Actionを未実行・失敗・成功へ書き換え、replayで解消する。 | 確定度の保持と自動再実行の禁止（第4〜5節）。 |
| Running Companionのpresence復旧とTask / Actionの再実行を同じ復帰にする。 | presence復旧と再実行・明示再開の分離（第4、7節）。 |
| 切替前の旧live推論・Tool結果・Client copyを復元正本へ混ぜる。 | 旧liveと復元正本の非混入、由来別の用途別受入（第5節）。 |
| 古いBackupの明示restoreによる復活を、targeted deletion後の自動再形成と同じにする。 | 完了済み消去と明示restoreの区別、未完了保留の維持（第8節）。 |
| 一部だけ新しい正本にし、旧stateと復元stateを意味的に混ぜる。 | 単一正本と全置換成立の条件（第3、9節）。 |
| 実行authorityだけ先に復活させ、完了していないRestoreを成功表示する。 | 復元成立と再有効化の分離、成功表示の条件（第7、9節）。 |
| Host再起動で部分置換を新正本にし、保留を黙って解除する。 | 再起動を跨ぐ保全と未完了の維持（第9節）。 |

今回の範囲は、復元対象と現在維持の対応、staleの扱い、live活動との隔離、backup作成の整合、受理から一括有効化までの成立条件、削除との交差、失敗・再起動のlogical propertyまでとする。Contextの情報選択・Provider適応の一般契約は[Context Assembly](context-assembly.md)を、認可判断と実作用・確定度・不明の一般契約は[Action Execution](action-execution.md)を、全域消去の参加・完了の一般契約は[Targeted Deletion](targeted-deletion.md)を、帰属切替・区切りの一般契約は[Client Presence Transition](client-presence-transition.md)を利用し、再定義しない。Backup scheduleの時機・保持数管理はTask Scheduleへ統合しない。

Targeted deletionと通常retention / Companion deletion / Resetは異なるlifecycleとして維持する。外部Workspace、Provider保有state等はBackupされた内部stateと同一の時間へ巻き戻るとは限らない。

## 2. 上位architectureとの位置関係

[Subsystem Decomposition](subsystems.md)の12責務、[State Ownership](state-ownership.md)（SO）第4〜8節、[Dependency Rules](dependency-rules.md)（DR）第3〜7節、[System Context](system-context.md)の内外境界、[Runtime Topology](runtime-topology.md)の配置・寿命・trust / failure boundaryを変更しない。新しいsemantic owner、中央Persistence owner、万能Restore Manager、統一restore state machineを追加しない。

| 本書内の役割 | 既存の責任とauthoritativeな判断 | 本書が持ってはならない正本 |
|---|---|---|
| 復旧用copyの作成・復元の調整 | 保全・消去が対象時点・内部範囲・除外・作成結果・操作状況・保留を調整する。 | 全domainの通常意味変更権、単一transaction、内部全構造への無制限access |
| 復元内容の意味・参照・時間的整合 | 各semantic ownerが自分の保持・利用範囲の対応・参照・利用可能性を確認する。 | 旧live状態の書換え、外部現実の巻戻し、現在有効性の自己確定 |
| 現在有効性・保留の確定 | 権限・制約がDeny・同意・cap・失効・保留を確定し、各利用箇所が適用する。 | domainのTask達成・作用成功・報告完了の確定 |
| 認証の照合 | 認証秘密が現在のstore維持・復元参照との照合・再認証要求を扱い、接続ownerが用途・有効性を確認する。 | Action承認、同意、実行可否の確定 |
| 帰属・活動条件 | 接続・存在、個体調整、作業等が移動・停止・未伝達・Task再開の現在条件を供給する。 | 帰属成立による実行権限の付与 |
| 時間的解釈 | 認識・学習がCompanion Stateの経過時間、作業がScheduleの次回導出を扱う。 | 一時状態の永久固定、全過去値の恒久保存 |

[Cross-cutting Design](cross-cutting.md)のCC-01（意図と実利用の対応）、CC-03（現在性と用途別受入）、CC-04（停止範囲と継続・再開）、CC-05（目的別lifecycleと全域操作の成立）、CC-07（確定度）を、復元固有の意味へ詳細化する。CC-02の利用範囲、CC-06の消費連続性は前提として利用し、再定義しない。Context Assemblyで確定した由来・用途・現在性・用途別結果受入、Action Executionで確定した判断対象と実対象の対応・委任不変・確定度・不明保持、Targeted Deletionで確定した消去区間・再保存防止・未完了保全、Client Presence Transitionで確定したauthoritative帰属・区切り・不明保持へ、復元のsemantic ownershipを移さない。

HostがEne内部persistent stateのcanonical holderであること、Backup copyそのものはcanonical stateではないこと、Restoreは外部世界を過去へ巻き戻さないことを維持する。

## 3. 復元されるもの・維持されるもの・自動再有効化されないもの

### 3.1 復元対象の対応

復元対象は、Restore開始前からHostにある現在のCredential storeのsecret部分を除く対象内部dataである。対応backup時点への全置換であり、旧live状態との意味的なmergeではない。少なくとも次の対応を、復旧可能な参照対応で含める必要がある。具体的な保存単位・表現は未定でも、各部のcopy出力が成功しただけをfull backup成功にしない。

- Characterの静的構成・revisionと個体への適用関係。外部Package原本は対象外であり、内部へ取り込んだ設定だけを扱う。
- Companionの同一性・活動状態（Running / Stoppedの別を含む）・適用済み構成。削除済み個体の私的stateはbackup時点に存在しないものとして扱い、個体削除後に残るhistorical recordは含める。
- Conversation History（一対一・グループ・Owner不参加のCompanion間交流を含む）、保存された非会話活動記録・historical evidence、未伝達事項と報告状況。原記録と報告状況を分け、表示copy送信を報告済みにしない対応を保つ。
- Experience Summaryと根拠関係、Memoryの現在認識・過去revision、内部Skill・import原本との対応・学習revision・実行結果、Relationship、Companion Stateと保持済み根拠。共有根拠の利用関係を保ち、Global本文から私的根拠全文へのaccess拡大を作らない。
- Taskの目的・担当・進捗・結果とTask context、一時Task Agentの委任対応、Actionの実行状況・作用の確定度・停止結果。Agentの一時context消失を完了根拠にせず、Task記録として残すべき事実を対応付ける。
- Taskとfolder・file・sourceのWorkspace関連付け。関連付け先の外部実体は辿って収集しない。
- Schedule設定・作成時timezone・初期Workspace入力・到来した回の発生対応。各回のTask記録との対応を保ち、missedを後から実行済みに書き換えない。
- Rule・個別Permission判断の記録・Provider割当同意（Observer専用assignmentを含む）・承認済みfallback・device許可・保存禁止・非共有・費用cap等の制御条件の記録。保存されたAllow・判定copyを現在条件として再利用しない対応を保つ。
- Provider接続情報（非秘密）・能力情報、MCP・Pluginの非秘密接続設定・受入設定。秘密値は含めない。
- 利用量・費用の記録（報告値・推定・不明の区別を含む）。現在消費の正本として扱わない条件は第4節に従う。
- Credentialの用途・参照元（非秘密の参照に限る）。秘密値そのものは含めない。
- Auditの追記順・保持の中の該当記録。完了記録・Auditへprivate本文・秘密を戻さない。
- 保持方針・全域操作の状況・backup設定と作成結果のうち、復旧可能な対応に必要な範囲。Task Scheduleとは別のbackup独自scheduleとして扱う。復元される操作状況はbackup時点の対応であり、置換成立前の未完了の保留・再保存防止を黙って破棄する意味ではない。第3.2節・第8節の維持条件に従う。

派生物（embedding・検索index・query派生物・Prompt cache・Provider session・有効経路・次回表示・集計表示等）は独立した復元対象にしない。元stateとの対応と利用範囲を保ち、古い派生物から現在状態・権限を復活させない。Provider session・cacheにしか継続状態が残らない構造を作らない。

一時data（Raw Observation・Raw Voice・詳細Tool payload・内部推論・Client表示copy・入力途中data・音声buffer・観測候補・推論中context・MCP Apps表示data等）は、復元して現在として継続する対象にしない。受理済み指示・必要な作業記録・未伝達・作用不明まで失ってよいわけではなく、対応するHost正本の範囲で復元する。

Client固有の接続材料のsecret部分はbackupから復元しない。復元されたdevice参照・許可は現在の接続・認証成立と照合する（SO第8節）。

### 3.2 現在環境から維持されるもの

次のものは復元対象ではなく、Restore時点の現在環境・外部現実として維持する。復元された設定・参照の存在から、これらをbackup時点へ巻き戻したとは扱わない。

- Restore開始前からHostにある現在のCredential store。secret値とその登録・更新・失効の状態を含み、Backupからsecretを復元したり過去時点へ巻き戻したりしない。全データResetによるCredential削除とは別の維持契約である。
- 外部Workspaceのfile・sourceの現在内容・存在・access、外部account・外部sourceの状態、Provider・MCP側の固有状態・保有copy、外部作用の既成事実、OS・device・Networkの現在状態、現在日時・timezone情報。
- 現在の接続・到達性・device利用可能性・排他性の事実。保存された接続・帰属を現在の到達性とみなさない。
- Restore前に未完了の全域操作の保留・再保存防止に必要な条件。特に未完了のtargeted deletionの消去条件・検証未完了・再保存防止は、置換によって黙って解除しない。第8節に従う。復元されるbackup時点の操作状況と競合する場合は、対象範囲の利用・再保存を確実に制限できることを優先し、現在の保留を破棄した状態を正常とは扱わない。
- Restore操作自体の受理・説明・実行保留。復元前の正常状態は、置換成立まで破壊しない。

Auditは3.1のとおりbackup時点へ置換されるが、置換成立後のRestoreの事実（目的・対象範囲・影響・除外・必要な確認・成立結果）は新しいAudit事実として追記する。追記順を外部世界の発生順や現在の許可の正本へ昇格させず、監査記録を元stateの再生・自動実行の入力にしない。置換によりbackup時点より後のpre-restore Auditが正本として残らなくなることを、外部copyが消えたことや外部作用がなかったことの根拠にしない。Ownerにはbackup時点より後の活動記録が正本として残らないことを事前に示す。

Backup設定自体が復元対象に含まれる場合、保存先・schedule・保持数・保護選択はbackup時点の設定へ置換される。ただしOwnerが別保存先へ作成した既存backup copyそのものは削除せず、内部削除でcopyも消えたと表示しない。

### 3.3 自動的には再有効化されないもの

復元されたassignment / consent / Rule等から、現在利用可能であることを自動的に導かない。復元された記録は直ちに開始根拠にならず、保留解除後も現在の条件を再評価する。一件ずつの再承認は要求しないが、一括有効化でDeny・同意・cap・認証不足・外部作用不明を無視しない。

- 復元されたProvider / MCP等の参照は、現在のCredential storeと照合する。利用可能なら現在のCredentialを利用し、不足・無効なら再認証を要求する。参照の復元は秘密値や認証成功の復元ではない。
- 復元されたassignment / consent / 承認済みfallbackは、現在のCredential・現在の制約・既存のRestore後保留条件を満たして初めて利用できる。Owner起点の利用にも同じ条件を適用する。
- 復元されたRule・Deny・Always ask・Capability境界・device許可・sandbox外例外は、現在の活動・委任・帰属・利用量・失効・保留と照合する。保存されたAllow・判定copy・解決済み経路だけで新規利用を始めない。
- Restore後のTask・Schedule・外部接続による自動処理は一旦保留し、Ownerが復元内容を確認してまとめて有効化できる。保全・消去の復元成立・保留理由、権限・制約の現在可否、各活動ownerの再開条件を対応付けてから活動する。
- 復元されたScheduleの各回は新しいTaskとして現在条件で開始し、停止中の回を自動補完しない。復元されたTaskの再開はOwnerの明示再開と現在条件を必要とし、不明作用の重複riskはOwner判断へ戻す。
- 復元されたClient device参照・許可は、現在の接続・認証成立・排他性を確認できて初めて帰属・利用に接続する。古い接続材料だけでHost側のpairing・許可を復活させない。
- Host自動起動の選択は復元対象の一般設定として扱えても、選択済みからTaskの明示再開・restore後の有効化・active Client不在時のHost側Client環境の自動起動を導かない。

### 3.4 新しい正本として扱える時点

次の論理的な成立条件を満たして初めて、復元内容をHost正本とする。それまではbackup copyも部分置換状態も正本にしない。旧live状態と競合する二重の正本を作らない。

1. 対応backupの読解・version対応・参照対応・除外（secret・外部実体）が確認されている。対応しないbackupを部分適用しない。
2. 現在のCredential storeを除く対象内部dataの全置換が、意味上の対応として成立している。局所copyの成功の集合だけを成立にしない。
3. 復元内容の参照・利用可能性の確認に必要な対応（認識・学習の経過時間解釈、接続・存在の現在性確認、認証秘密と接続ownerの照合、権限・制約と活動ownerの保留対応）が揃っている。
4. 失敗時は復元前の正常状態を破壊していない。不完全な復元や旧live状態と競合する正本を成功と表示しない。内部の復元失敗を外部作用のrollback成功とも扱わない。

復元成立と実行再有効化は別々に確認する。復元成立だけでは自動処理を開始せず、第7節の一括有効化と各活動の現在条件を満たす必要がある。通常再起動のように途中Taskを明示再開待ちにするだけでは足りず、復元内容全体の確認を経る。

## 4. 現在stateと復元stateの分離・staleの扱い

復元された設定・参照・Task・Schedule等が存在することと、それらが現在実行可能であることを分離する。古いBackupからのRestoreによって、staleなPermission・Provider条件・Client状態・完了済みまたは結果不明なexternal Action・現在存在しないexternal resourceを無条件に現在事実にしない。

- **Permission・Rule・同意・上限。** 復元された記録と、現在の利用可否は別の意味である。現在の依頼・担当・委任・Workspace・帰属・利用量・失効・保留を照合する。重要な意味（目的・実対象・操作・送信先・data・外部作用等）が変われば再評価する。無関係な変更ごとに全処理を再承認しない。
- **Provider条件。** 復元された接続情報・能力観測・割当解決は現在の利用可能性ではない。現在のassignment・接続・能力・同意・費用を照合し、承認済みfallbackと順序だけを使う。Local / LANから未承認Cloudへdataを移さない。Provider変更で継続状態を分断したり意図的に差別化したりしない。
- **Client状態。** 復元された接続・帰属・device参照は現在のpresence・接続・許可ではない。現在の接続・許可・排他性を確認できて初めてpresenceが成立する。元のClientが利用可能になるまではactiveなしとして扱え、別Clientへの無条件自動移動・Stoppedへの適用・Host側Client環境の自動起動に広げない。過去の帰属を現在の利用可能性として再成立させない。
- **外部Actionの作用結果。** 復元された確認済み成功・確認済み失敗・不明の別と、その根拠・観測者を維持する。unknown outcomeをRestoreによって未実行・失敗・成功へ書き換えたりreplayしたりしない。Task Agentの申告だけで成功を確定しない。記録保存の失敗も、最後の正常記録だけから未実行を推定する理由にしない。
- **外部resource。** 復元されたWorkspace関連付け・保存先・外部参照は、外部の現内容・存在・accessをbackup時点へ戻さない。解決できない・対応しない参照は開始しない。近い対象への作用・広い対象への作用へ読み替えない。外部fileを内部成果物libraryへ複製しない。
- **Companion Stateの時間。** Backupに含まれるCompanion Stateをrestoreするときも、backup後に経過した時間を無視して時間的に一時的な状態を保存時点の値のまま無期限に固定しない。一時的状態と持続的傾向の区別を保ち、再起動・Client切替・Provider変更だけで意味のある状態を初期化しない。具体dimension・scale・減衰式は固定しない。
- **Scheduleの時刻。** 復元された作成時timezone・時刻条件を保持し、表示localeや現在のOS timezoneから黙って置換しない。次回時刻は保存条件と現在日時・timezoneの規則から導出し、表示値を独立条件の正本にしない。Host停止中・担当停止中に到来した回はmissedとし、自動補完しない。Run nowは現在条件で新しいTaskを開始する。
- **利用量・費用。** 復元された報告値・推定・不明・処理中消費はbackup時点の知識であり、現在の消費の正本ではない。未報告をゼロとせず、復元された古い使用量を現在の残額の根拠にしない。cap到達・費用不明で安全に継続不能ならdataを保って対象処理を止める。復元を消費リセットの理由にしない。

古い情報は過去の根拠として有効な場合があるが、現在値や現在の承認としては代用できない。共通の一時点snapshotや全domainの共通時刻順を要求しないが、用途成立に必要な対応が矛盾する場合はその用途で採用せず再評価する。

## 5. Restoreとlive activity

Restore開始前後に存在するlive活動と復元stateの関係を、活動種別ごとに区別する。具体的なprocess停止方法やtransaction mechanismは固定しない。必要なのは、旧live activityから遅れて届いた結果を復元後のstateへ無条件に混入させないことである。

| live活動 | 復元stateとの関係 |
|---|---|
| 会話・inference（応答・形成・Permission解釈・消去探索を含む） | 切替前に開始した推論・Tool結果が切替後に届いても、旧live状態を復元後の正本へ混入させない。用途別受入で現在の対象・制限・意味へ照合し、古い結果で訂正を上書きせず、旧結果を新目的の達成にしない。旧context・cache・sessionを新正本の現在contextとして再利用しない。 |
| Task / Task Agent | backup時点より後の旧live Task記録は、置換成功後は正本として残らない。成功前の置換未成立の間は復元前正常状態を正本とする。旧liveの遅延結果を復元されたTaskの達成・Learning・許可へ自動採用せず、後続を自動開始しない。復元されたTask（backup時点で進行中だったものを含む）は保留し、明示再開と現在条件を必要とする。Agent session残存・Provider回復・Host自動起動選択だけでは再開しない。 |
| Action（試行と作用） | 試行と作用を別の関係として扱う。retry・再実行は同じ試行の継続ではなく新しい試行とする。作用不明の試行の再実行は重複riskを示したOwner判断を必要とし、Owner判断なしに開始しない。確認済み失敗と不明を同じretry経路へ潰さない。 |
| delayed result | 「記録する」「semantic更新する」「次のActionを開始する」「Ownerへ提示する」は別の受入判断である。消去条件・保留・帰属も照合する。到着が遅いことだけで出来事を新しくせず、最新到着だけで現在値を決めない。 |
| Client接続・一時data | 旧liveのClient copy・未送信操作・表示copyでHost正本を上書きしない。未送信操作を自動Action queueとしてreplayしない。再接続の古い一時stateだけでpresence・Permission・再開を成立させない。復元されたdevice参照も現在の成立と照合する。 |
| Schedule | 切替前の到来と復元された設定を区別する。復元成立だけでは将来回を開始せず、一括有効化後も各回を現在条件で評価する。missedを実行済みに書き換えない。backup scheduleの運用時機をTask Scheduleへ依存させない。 |
| targeted deletion等の全域操作 | 第8節に従う。未完了の消去条件・保留を置換で解除せず、旧live結果からの再保存・古い根拠からの再形成を防ぐ。確認不能を成功にせず、局所完了を全域完了にしない。 |

切替前に旧live状態で開始した要求・結果と、復元後の活動・復元正本を、Context Assembly第3節の対応（主体・用途・Task / 委任・Action・Client・source・revision・判断時前提、停止前後・再起動前後・restore前後の継続関係）で区別する。必要な対応を失った結果は到着先の現在活動へ付け替えず、その用途では受理できないものとして扱う。旧live作用の必要説明を行う場合も、その事実と復元対象の記録を区別し、古い結果からTask・Learning・許可を自動復活させない。

Background・Scheduleの無人実行も同じ条件に従い、Client不在・Schedule到来・復元成立を自動承認の理由にしない。Owner確認が必要なら実行せず判断待ちにする。

## 6. Backup作成の整合

Ownerの要求または有効なbackup設定を契機に、保全・消去が対象時点・内部範囲・参照対応・除外・保護を各ownerと照合する。Task Scheduleや担当Companionの稼働を必須にしない。非暗号化ならprivate dataを含む説明を事前に行い、暗号化も選択可能にする。

- 各ownerは復旧可能な対応で内容を提供する。個体削除後の残存記録も含め、認証秘密の除外と外部実体の非収集を守る。単に各部のcopyが作れたことだけをfull backup成功にしない。対象時点・参照・必要な履歴と未完了状況の対応が揃って初めて成功とする。
- 未完了の消去・復旧とbackup作成が重なる場合、その制約を無視した正常・即実行可能なcopyを作らない。作成を待たせるか未完了・制約も復旧可能に含めるかは自由度である。backupの運用設定・時機は保全・消去に残す。
- 実行中のAction・in-flightの不明がある場合、最後の正常記録が外部世界の最新状態とは限らないことを保つ。不明を未実行へ戻して正常copyと偽らない。
- 作成済みbackupはlive正本にならない。明示restoreを経ずに古いcopyをlive正本として読み戻さない。内部削除でOwner保存backup・export・外部送信copyも消えたと説明しない。
- Ownerは保存先・schedule・保持数を選択でき、作成結果と失敗を確認できる。失敗時は復元前の正常状態と同様に、最後の正常状態を破壊しない。許可や保存結果を誤って成功表示しない。
- backup形式・保存実装・整合性確保・暗号化方式・対応version・復旧手順は固定しない。暗号化されていないbackupを作成する場合は、History・Summary・Memory・Relationship・Companion State等のprivate dataが含まれることを事前に明示する。

Backup作成に担当CompanionやTask Agentの稼働を必要とせず、管理面と保存済みdataの利用可能性をBody・Voice・Provider・拡張の成功へ従属させない。

## 7. 受理から新しい正本・一括有効化まで

固定state machineを作らないが、後続設計は少なくとも次の論理的な前後・成立条件を保つ。すべてを一つのbooleanや単一flagへ潰す実装は不可とする。

1. **受理・説明。** Ownerは対応backupを選び、現在のHost Credential storeを維持する対象内部dataの全置換・version対応・外部file非変更・削除済み情報や旧Rule / 同意 / Scheduleが戻り得ること・ProviderやMCP等の再認証の可能性の説明を受ける。初回Setupからのrestoreもこの契約を通る。
2. **隔離・保留。** 保全・消去は各ownerを復元へ参加させ、旧live状態の進行中利用・遅延結果・Client copyを復元後の正本と混同しない条件を揃える。必要な活動・外部接続の保留は権限・制約と各活動ownerが適用する。旧実行が外部へ既に与えた作用は消えず、その不明を復元による未実行へ変えない。
3. **照合。** 各ownerが復元内容の対応・参照・利用可能性を確認する。認識・学習はCompanion Stateの経過時間を扱い、接続・存在は保存された接続・帰属を現在の到達性とみなさない。認証秘密は開始前のstoreを維持し、接続ownerと復元参照を現在の用途・有効性と照合する。外部Workspaceの現在内容・存在・accessはbackup時点へ戻らない。
4. **置換成立。** 保全・消去が全置換の成立を確認して初めて復元内容をHost正本とする。失敗時は復元前の正常状態を破壊せず、不完全な復元や旧live状態と競合する正本を成功と表示しない。
5. **復元後保留。** 復元成立後もTask・Schedule・外部接続による自動処理は保留する。保留中・確認途中にHostが再起動しても、復元済みassignment / consentだけで自動利用を開始しない。Owner起点の利用にも現在のCredential・制約・該当する保留条件を適用する。
6. **一括有効化。** Ownerは内容を確認してまとめて有効化でき、一件ずつの再承認は要求しない。保全・消去の復元成立・保留理由、権限・制約の現在可否、各活動ownerの再開条件を対応付けてから活動する。Deny・同意・cap・認証不足・外部作用不明を一括有効化で無視しない。

ここで固定するのは整合条件であり、停止範囲、切替の具体手順、結果の識別方法ではない。通常再起動のように途中Taskを明示再開待ちにするだけでは足りず、復元内容全体の確認を経る点が異なる。設定Reset・全データReset・対応upgradeとは同じ復旧操作に潰さない。

## 8. Backupと削除の交差

古いBackupには、そのBackup作成後にTargeted Deletionされた情報が含まれている可能性がある。現在のTargeted Deletion契約と矛盾せず、同時に「Ownerが明示的に古いBackupをRestoreする」という操作の意味も成立させる。

- Restore前には、削除済み情報や旧Rule・同意・Scheduleが戻り得ることを説明する。Ownerが説明を受けて旧backupを明示restoreすると情報が戻り得ることは、自動再形成禁止の迂回ではなく要件に定めた別操作である。
- 戻った情報は新しい正本として扱い、自動的に再消去しない。完了後にOwnerが改めて同じ情報を提供した場合と同様に、新しいExperienceの根拠になり得る。改めて消去するには新しいtargeted deletionの明示が必要である。削除前の根拠だけによる自動再形成を許す例外にはしない。
- 復元成立後の保留・一括有効化・現在条件の再評価を経ずに、戻った情報を送信・作用・学習へ自動利用しない。戻ったRule・同意・Scheduleを即座の自動処理へ接続しない。
- 完了記録・Audit・Owner説明自体へ対象private本文を再保存しない。復元による復活を、外部送信・export・backup済みcopyの消去保証と混同しない。
- 未完了の消去とbackup / restoreが重なる場合は、制約を無視した正常・即実行可能なcopyを作らず、未完了・保留・再保存防止をHostで保全する。再起動・再接続・restore後も未完了の認識と必要な保留を維持し、操作途中の再起動でも完了と誤認したり保留を黙って解除したりしない。
- 消去区間（開始から全域完了まで）に再到着・再生成した対象情報は同じ消去対象とする。Restoreによって戻る対象情報が消去区間に重なる場合は、各受入・保存先が進行中の対象情報についての消去条件を適用し、古い根拠だけからの再形成・遅延再保存を防ぐ。指定文字列は機械的な検索・除去・残存検証へ参加させる。
- 通常History / log整理・容量retention・Companion削除・設定Resetとtargeted deletionの目的差を維持する。通常削除からのcascadeで形成済みLearning・Summaryを消さず、Companion削除の残存記録をtargeted deletionの除外にしない。

新しいPrivacy semanticsは本書で決めない。上記は既決の「自動再形成の禁止」と「明示restoreによる復活」の両立であり、外部copyまでの消去保証・復元後の自動実行許可の追加ではない。

## 9. 失敗・再起動のlogical property

具体的なatomic swap、journal、snapshot、WAL、transaction、copy-on-write等は選ばない。後続設計は、次の性質を満たす保持・切替を選ぶ。

- **単一正本。** いかなる時点でもHost正本は一つである。置換成立前は復元前正常状態が正本であり、成立後は復元内容が正本である。backup copy・部分置換状態を正本にしない。
- **非混合。** 旧stateと復元stateを意味的に混ぜない。旧liveのTask進捗と復元されたLearning、復元されたRuleと旧liveの消費実績等を組み合わせた第三の状態を作らない。
- **権限の先行復活の禁止。** 実行authorityだけ先に復活させない。復元成立・一括有効化・現在条件の確認前に、新規Action・送信・外部作用を開始しない。
- **成功表示の条件。** 完了していないRestoreを成功表示しない。局所copy・局所確認の成功を全域成功にしない。未確認・検証失敗・pending / unreachableを成功に読み替えない。失敗時は復元前の正常状態を守り、失敗と安全な次の操作を説明する。
- **再起動時の保全。** 保留中・確認途中・置換途中にHostが再起動しても、復元済みassignment / consentだけで自動利用を開始しない。復元前の正常状態・未完了の認識・必要な保留・再保存防止をHostで保全し、部分置換を新正本として扱わない。再起動を完了・解除の根拠にしない。途中Taskは明示再開待ち、停止中のSchedule回はmissed、Running個体のpresenceは元Clientへの復旧というそれぞれの既存条件に従うが、restoreの復元後保留がある場合はそれを維持する。
- **Client到達不能の扱い。** 到達不能・確認不能を消去・復元の成功に読み替えない。再接続しても古い表示copy・入力・Tool UI dataをHostへ戻して対象を再形成しない。どの到達性確認・無効化方法で完了根拠を得るかは後続設計へ残すが、必要な確認を省いて完了としない。
- **記録の保全。** 記録に削除対象の秘密・private本文を残すことは完了根拠の代わりにならない。一時bufferの消失は成功や完了の根拠にしない。継続して必要なTask状態・未伝達・作用不明・全域操作未完了等を本来のownerが保全する。

通常保存・migration・対応upgradeも、成功まで最後の正常状態を守る既存契約に従う。未対応downgradeを成功する前提にせず、起動前に示す。

## 10. 横断検証

requirements、上位architecture、および既存Step 11詳細設計へ戻して横断検証した。チェックリストの機械的な充足ではなく、正常系と重要なfailure / stale / delayed / restartを選んでwalkthroughした。

| 領域・交差 | Walkthroughと必要な結果 | 本書の成立箇所 |
|---|---|---|
| 要件「Backupとrestore」正常 | portable full backupの作成・保存先・schedule・保持数・暗号化選択・結果確認が閉じる。除外（secret・外部実体）と非暗号化時のprivate説明を守る。 | 3、6。AD-15、SC-06、RT-09を維持。 |
| 要件 同意・Credential・費用 | 登録だけで利用せず、割当同意・承認済みfallback・capを現在の条件で照合する。復元参照と現在storeの照合で再認証を要求できる。 | 3〜4。SO 4.18〜4.21、DR 5.4・5.5を維持。 |
| 要件 Schedule・Task・Computer Use | 復元後保留・一括有効化・現在再評価・missed非補完・不明非replay・active限定を守る。 | 3〜5、7。SO 4.10〜4.14、AD-09を維持。 |
| 要件 Companion State・時刻 | 経過時間を解釈し、一時固定・不自然な初期化をしない。Schedule timezoneを黙って置換しない。 | 4。SO 4.9・4.14を維持。 |
| RF-08 backup作成と未完了消去 | 未完了消去とbackup作成が重なる。制約を無視した正常copyを作らず、待機または未完了を含める。 | 6、8。CC-05、Targeted Deletion 7.4を維持。 |
| RF-08 restoreと旧live結果 | 切替前に開始した推論・Tool結果が切替後に到着。復元正本へ混ぜず、旧作用説明と区別する。 | 5。Context 8.1、Action 7.3、CC-03を維持。 |
| RF-08 旧backupの明示restore | 削除済み・旧Rule等が戻り得ることを事前説明し、復活は別操作としてAuditへ残す。自動再形成の例外にしない。 | 8。AD-15、CC-05を維持。 |
| stale Permission / Provider / Client | 復元記録だけで新規利用・送信・帰属を成立させない。現在条件へ照合し、確認不能を許可・現在へ変換しない。 | 3〜4。Context 7、Action 5、CC-01・03を維持。 |
| 完了済み・不明作用の書換え | 不明を未実行・失敗・成功へ変換せず、自動replayしない。retryは新試行として現在条件で扱う。 | 4〜5。Action 8、CC-07を維持。 |
| dangling外部参照 | 解決不能・対応不一致の参照で開始せず、外部を黙って変更・削除しない。 | 3〜4。SC-06、DR 6.4を維持。 |
| Client旧copy・再接続 | Client copyでHostを上書きせず、未送信操作を自動queueにしない。復元device参照も現在成立と照合する。 | 5。Client Presence 6・8、CC-03を維持。 |
| 消去中の切断・再起動・restore | 確認不能を成功にせず、古い一時dataを戻さない。未完了・保留・再保存防止をHostで維持する。 | 8〜9。Targeted Deletion 7、CC-05を維持。 |
| Restore失敗・再起動 | 部分正本・混合・権限先行復活・成功誤表示をしない。復元前正常を保全し、再起動後も保留を維持する。 | 9。RT-09、CC-04・05・07を維持。 |
| 復元後保留中の再起動 | 復元済みassignment / consentだけで自動利用を開始しない。Owner起点利用にも現在条件を適用する。 | 7、9。CC-04を維持。 |
| Observer・自発性との交差 | 復元されたObserver専用assignment・自発性設定を現在条件なしに有効化しない。Companion overrideを合成せず、旧Captureを付け替えない。 | 3〜5。Context 6、CC-02・04を維持。 |
| 補助推論・費用不明との競合 | 復元内容の照合・消去探索の推論が不通・費用不足でも、機械的検証・停止・拒否・管理を塞がない。不足を管理面へ返す。 | 3、7。Context 5.2、CC-06を維持。 |
| Audit・報告 | 確定度を強めず、本文の別保管庫を作らない。監査記録を再生・自動実行の入力にしない。 | 3、9。Action 9、CC-07を維持。 |

Cross-cutting契約との照合結果は次のとおりである。

| 契約 | 詳細化によって維持する性質 |
|---|---|
| CC-01 | 生成content・復元記録から権限を新設しない。既存依頼で足りる場合の再確認を増やさない。復元Rule・同意をtriggerにしない。 |
| CC-02 | 参照・変換・共有・送信・保存・派生物・Client経路へ利用範囲・消去条件を適用する。Credentialは別経路で非露出を維持する。通常Learningのscope形成責任は維持する。 |
| CC-03 | 削除前の根拠と完了後の新規提供・明示restore、過去の正しさと現在有効性を分離する。遅延結果の用途別受入により単一valid判定へownerを集めない。 |
| CC-04 | Client依存だけを現在帰属に結び付け、Stop・Cancel・再起動・restore保留を区別する。解除一つで他の禁止・保留を消さない。自動replayしない。 |
| CC-05 | 保持・利用先が派生物・遅延結果まで消去・復元へ参加する。原記録削除・context終了・局所完了を全域完了にしない。外部copy消去を内部完了に含めない。 |
| CC-06 | 並列消費・処理中・不明を同じ上限へ反映し、制御・保全経路を推論・長時間Task待ちにしない。機械的検証をLLM待ちにしない。 |
| CC-07 | transport・Tool・Task・報告・監査の確定度を分け、不明を成功・失敗・未実行へ変換せず、保存・報告・監査・復旧で強めない。 |

State Ownership、Dependency Rulesとの照合では、semantic owner、Host / Client配置、trust / failure boundary、lifecycle、permission / consent semanticsを変更する必要は見つかっていない。新しい第二の正本・無所属の意味状態・LLMによる強制・失敗時専用の迂回・中央Persistence owner・万能Restore Managerを導入していない。通常History保持、Companion削除、targeted deletion、backup / restore、ResetはSO・DRの異なるlifecycleを維持する。

Context Assemblyとの照合では、由来・用途・現在性・用途別結果受入・処理中無効化の契約を復元へ接続し、旧live結果の混入・古いsessionによる制約迂回を許していない。Action Executionとの照合では、判断対象と実対象の対応・委任不変・試行と作用の区別・確定度・不明保持・遅延帰属・報告での確定度保持を復元へ接続し、移動・再接続・再起動・restoreによる自動再実行を許していない。Targeted Deletionとの照合では、消去条件の適用・区間内再到着の取込み・旧由来と新規提供・明示restoreの区別・cache・session・Client copyの再利用禁止・未完了保全の契約を復元へ接続し、古いBackupからの復活を新しいExperience・別操作として扱い、自動再形成の例外にしていない。Client Presence Transitionとの照合では、authoritative帰属・切替区間の新規開始禁止・活動別区切り・到着物の帰属・Host継続・再起動復旧と再実行の分離の契約を復元へ接続し、復元された帰属のcanonical化・二重presence・旧作用の自動継続を許していない。

## 11. 後続設計への引渡しと残す自由度

後続のstate / persistence / concurrency / interface設計は、次を固定された契約として利用できる。

- 復元対象は現在のCredential store secretを除く対象内部dataの全置換であり、旧liveとのmergeではない。backup copyは正本ではなく、置換成立後に初めて復元内容がHost正本になる。
- 現在のCredential store・外部現実・現在の到達性・未完了の保留は維持され、復元参照の存在から巻き戻したとは扱わない。
- 復元されたassignment / consent / Rule等だけで現在利用・自動処理を開始しない。復元成立・一括有効化・現在条件を別に満たす。一件ずつの再承認は要求しないが、Deny・cap・認証不足・不明を無視しない。
- staleなPermission・Provider・Client・作用結果・外部参照を現在事実にしない。dangling参照は未解決とし、不明は不明のまま保持し、自動replayしない。
- 旧live要求・結果・Client copyを復元正本へ混ぜない。用別受入で由来を区別する。
- 明示restoreによる復活は自動再形成の例外ではなく別操作であり、事前説明・Audit・保留・現在再評価を経る。未完了消去の保留は置換で解除しない。
- 単一正本・非混合・権限先行復活の禁止・成功表示の条件・再起動時の保全を守る。

今回絞り込んだ禁止選択肢は、復元内容の無条件上書き・merge、Credentialの巻戻し・復元、復元参照・同意からの自動利用、外部現実の巻戻し・danglingの黙った解消、不明の書換えと自動replay、presence復旧によるTask・Actionの再開権限化、旧live結果・Client copyの混入・Host上書き・自動queue、古い根拠からの自動再形成・cache hitでの制約省略・Provider sessionの無条件再利用、確認不能の成功扱い、部分正本・混合・権限先行復活・未完了の成功表示、再起動による保留解除・部分置換の正本化、要約・復旧での確定度強化である。いずれも上位契約を成立させないため採れない。

以下は意図的に残すDesign Freedomである。

| 設計対象 | 固定済みのarchitecture property | 残すDesign Freedom |
|---|---|---|
| 復元対応・現在性の表現 | 由来の偽装ができず、対象変更・失効・切替・消去条件を各利用先で扱える。確認不能を許可・現在・完了へ変換しない。 | 復元対応・由来・区間・完了境界の具体表現、確認のまとめ方、対象探索・参照整合・照合の配置、識別・鮮度確認・競合制御。 |
| 置換・保留・有効化の実現 | 単一正本、非混合、権限先行復活の禁止、受付と完了の分離。確定度を強めない。 | 停止伝達・切断検知・帰属調停・round区切り・queue / actor / event bus・lock / transaction・IPC・process配置・retry / timeout値・exact progress表現。 |
| Backup作成・保持 | 参照整合、最後の正常状態の保護、旧結果非混入、外部非巻戻し、secret除外を守る。 | 整合時点の作り方、部分失敗からの復旧、切替・保存形式・暗号化方式・対応version・保存先・schedule・保持数・保持期間。 |
| 再起動・再接続の整合 | 未完了・保留の維持、旧live非混入、復元後保留、外部世界非巻戻しを守る。 | journal / WAL / transaction等の永続化機構、再接続・待機・timeout・pairing・再配置の具体方式、到達性確認方式。 |
| 記録・報告・監査 | 元事実の確定度・privacy・順序の意味と管理到達性を維持する。本文の別保管庫を作らない。 | audit format・保持期間・提示確認・要約粒度・UI layout・診断stack。 |
| 時間・費用 | 経過時間の解釈、missed非補完、並列・処理中・不明の上限反映を守る。 | 減衰・時刻計算・Capture時機・費用予約・集計期間・推定・資源配分の機構。 |

archive / file format、encryption implementation、DB schema、serialization、transaction mechanism、filesystem layout、Rust type / trait、crate / module、IPC、locking、exact progress representation、retry / timeout、specific libraryも固定しない。上表の対応関係から統一Context layer、Policy Engine、Manager、Service、Coordinatorの追加を導かない。既存の12責務、semantic owner、Host / Client配置とtrust boundaryの下で実現方法を選ぶ。

対象探索・検証・到達性確認・backupとの交差等の具体mechanismが未決定であることはIssueにしない。必要な確認・Owner判断の省略、現在同意の拡張、未完了・不明の成功扱い、外部作用rollback・exactly-once実行の新保証はいずれも自由度に含めない。後続でこれらの性質を成立させられないことが判明した場合は、黙って例外を設けずArchitecture Issueとして戻す。

### Requirement / Architecture Issue

本書の範囲では、Requirement Ambiguity / Gap、上位architecture変更、semantic owner / subsystem boundary変更、Privacy / Security / Permission semanticsの変更を必要とする事項は見つかっていない。古いBackupに含まれる削除済み情報の復活は、既決の「自動再形成の禁止」と「明示restore」の区別で成立し、新しいPrivacy semanticsを追加していない。具体mechanismの未決定はIssueにしない。
