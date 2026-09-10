# Subsystem Detailed Design — 内部データ保全・消去

本書は Step 12 Subsystem Detailed Design として、**内部データ保全・消去（以下、保全・消去）**単独を詳細化する。

本書内の PE 番号は本書内の内部責務の参照用、DP 番号は本書内のSubsystem間semantic contractの参照用であり、製品要件IDではない。

## 0. 位置付けと不変条件

上位設計との優先順位と矛盾時の扱いは [設計文書 README](../README.md#正本と優先順位) に従う。[Context Assembly](../critical-areas/context-assembly.md)、[Action Execution](../critical-areas/action-execution.md)、[Targeted Deletion](../critical-areas/targeted-deletion.md)、[Client Presence Transition](../critical-areas/client-presence-transition.md)（以下、Presence Transition）、[Backup / Restore](../critical-areas/backup-restore.md)（以下、Backup-Restore）の一般契約は、隣接する確定済み contract として利用し、再定義も所有権の移動もしない。本書内の SO は [State Ownership](../architecture/state-ownership.md)、DR は [Dependency Rules](../architecture/dependency-rules.md) の節番号を指し、CC / RF 番号は対応する architecture 文書（[artifact 一覧](../README.md#artifact-一覧)）の契約 ID である。

新しい semantic owner、第二の正本、万能Deletion Manager / Restore Manager / Persistence Owner / Policy Engine、統一state machine、共通Context layer、共通Client session layerを追加しない。

Targeted Deletion と Backup-Restore が定めた全域操作そのもののlogical contractを再設計しない。本書は**保全・消去がそのcontractを成立させるために何を担い、各semantic ownerへ何を要求し、何を受け取って全体成立を判断するのか**へ落とす。Context Assembly / Action Execution / Presence Transitionで定義されたprovenance、delayed result、unknown outcome、Client一時data等も既存contractとして利用する。

## 1. 選定理由

保全・消去は「全dataを触る」ように見えるため、後続が誤って次のいずれかへ潰しやすい。

- Ene全dataのcanonical owner、中央Persistence owner、汎用Repository ownerへの昇格。
- 全domainの通常writer、単一transaction owner、内部全構造への無制限accessを持つ万能Coordinatorへの膨張。
- Targeted Deletion / 通常忘却 / 訂正 / 失効 / 置換 / 統合 / History整理 / retention / Companion deletion / 設定Reset / 全データReset / Restoreを一つの「削除・初期化」へ潰す目的混同。
- Audit / Debugを秘密保管場所・削除の抜け道・staleなcanonical・確定度強化の根拠にする別保管庫化。
- 局所削除・局所copy・LLMの納得・到達不能の放置・再起動による保留解除のいずれかでの全域完了扱い。
- 通常時のdata access / mutationを保全・消去経由にしないと成立しない必須中央経路化。

同時に、各Subsystemの個別処理だけでは次の全域性が説明できないため、保全・消去を置かない選択肢も採れない。

- 保存場所をOwnerへ選ばせないTargeted Deletionの参加調整と再保存防止。
- 対象時点・参照対応・除外を揃えたfull backupの成立と、Credential除外・外部非収集の保証。
- 現在Credential維持・外部非巻戻し・単一正本・復元後保留・一括有効化を伴うRestoreの成立。
- 保護対象を守る設定Resetと、信頼失効を伴う全データResetの区別。
- 既定OFFのretentionと通常忘却の分離。
- 再起動・再接続・restoreを跨ぐ未完了・保留のHost保全。
- 秘密・本文を含まない完了説明とAudit可能性の両立。

最初のmilestone（ targeted deletionの文字列残存・形成競合・再形成防止・共有根拠の部分削除、容量不足時の非自動削除、主要障害時のdata保全とText管理面）は、保全・消去の参加調整が閉じて初めて検証できる。Backupからの復元・Host自動起動は後続milestoneだが、本書はその確定済み契約を除外せず、将来の受渡しを塞がない形で定める。具体的な保存単位・削除query・検証実装・到達性確認方式は固定しない。

## 2. 内部責務の分離

内部責務は論理的な判断責任であり、component・crate・process・service・threadの分割を指定しない。PE-1〜PE-7は同じ保全・消去に属するが、一つの状態・lifecycle・保存単位・state machineではない。

| 内部責務 | authoritativelyに判断するもの | 持ってはならない正本 |
|---|---|---|
| PE-1 Targeted Deletionの参加調整 | Ownerの明示的なPrivacy / Security目的と対象記述・影響・除外の対応付け、参加すべき領域の特定支援、局所結果・検証・未完了・保留の集約、全域完了の確定（Targeted Deletionに限る）。消去区間と各利用箇所が照合できる消去条件の対応付け。 | Memoryの重要度・Relationshipの解釈・Task達成・Action成功の再判定、参加先の通常意味変更権・任意domain編集権、Credentialの外部失効の自動実行、外部copy消去の保証、単一transaction・内部全構造への無制限access |
| PE-2 正常保全とBackup作成の調整 | 正常保存・migration・対応upgradeの復旧可能性の調整、各ownerの意味整合確認の取まとめ、Backupの対象時点・内部範囲・参照対応・除外・保護の照合と作成結果の確定。保存先・backup独自schedule・保持数・保護選択の管理。 | 各domainの意味決定、Credential秘密値の所有・復元、外部実体の収集、Task Schedule化、担当Companion・Task稼働の必須化、backup copyのlive正本化 |
| PE-3 Restoreの調整 | 受理・説明・隔離・照合・置換成立・復元後保留・一括有効化の調整。現在のCredential store維持の確認、外部非巻戻し・単一正本・非混合・権限先行復活の禁止の確認。旧backup交差（第4.4節）の説明・新正本扱いの確定。失敗時の復元前正常維持の確認。 | 外部現実の巻戻し、secretの復元・巻戻し、復元参照・assignment / consentからの自動利用開始、staleの現在事実化、旧live結果・Client copyの混入、復元内容の存在＝実行可能化 |
| PE-4 Resetの調整 | 設定Resetの範囲調整と保護対象の保持確認、全データResetの列挙・強い確認・内部消去・処理中data・Client一時copyの扱い・信頼失効の調整。 | 各設定の意味決定、外部Workspace・外部Skill・Owner保存backupの削除、Companion削除との混同、Audit別保管庫化、到達不能Clientの物理消去の確認済み扱い |
| PE-5 保持方針・retention / cleanupの調整 | 通常History / log削除・保持期間変更の調整、Learning revision・Summary等の明示opt-in cleanupの適用調整。既定OFFの維持、通常忘却・History整理とtargeted deletion・Companion削除の目的分離。 | Memory意味・重要度・scope意味の決定、自動削除の既定有効化、通常忘却の削除化、History整理のLearning・Summaryへのcascade、容量不足を理由とする黙った削除 |
| PE-6 Audit / Debugの保全上の扱い | Auditの追記順・保持の正本管理、Debug captureの明示対象・内容・短期失効・停止・削除と全体成立の管理。本文別保管庫化の禁止、確定度非強化の維持。 | 元事実の意味再判定（Action成功・Permission妥当性・Task達成等）、会話・file本文・秘密・削除済みprivateの保持、Auditの再生・自動実行入力化、Telemetry自動送信、全Raw恒久保存の要求 |
| PE-7 全域操作状況のHost保全とOwner説明 | 操作同一性・参加対応・消去条件・保留・再保存防止のHost保全、再起動・再接続・restore跨ぎの維持、pending / unreachable / failed verificationの区別保全、事実と本文の分離説明の材料管理。 | domain通常writer、単一transaction、内部全構造への無制限access、対象本文の保持による完了根拠化、一時buffer消失の成功根拠化、LLM納得による完了確定 |

関係の要点は次のとおりである。

- PE-1〜PE-4は操作種別ごとの調整であり、互いのlifecycleを共有しない。Targeted Deletion・Backup・Restore・設定Reset・全データReset・Companion deletion・通常削除・retentionを一つの状態機械・一つの完了条件へ潰さない。
- PE-7はPE-1〜PE-4に横断する共通の保全・説明責任であり、独立した第8の操作ではない。PE-1〜PE-4のいずれもPE-7のHost保全なしに全域完了を確定できない。一方、PE-7だけでいずれかの操作を完了させることはできない。
- PE-5はPE-1の消去と別の保持契約であり、PE-1への読替え・PE-1からの流用をしない。PE-6はPE-1〜PE-4の完了説明の材料を受け入れるが、元事実の意味を再判定しない。
- PE-2の正常保全はPE-3のRestoreと対になるが、通常再起動をRestoreのような一律の復元内容確認へ拡大しない。PE-4の全データResetはPE-1のTargeted Deletionと異なる消去範囲であり、historical recordの扱いが異なる（第5節）。

保全・消去は次のauthorityを取得しない。Memoryを書き換えるsemantic authority、Task stateの意味を決めるauthority、Historyの内容を生成するauthority、Permissionを変更するauthority、Credential secretを所有するauthority、Client presenceを決めるauthority、Provider割当・能力の意味決定、作用成功の確定、報告完了の確定。全域操作で変更が必要な場合も、必要な意味判断・局所操作は既存ownerが担い、本書第4節のsemantic contractによって参加する。

## 3. 管理する logical state

特に外部・copy・一時と記したものを除き、Hostで管理する内部状態を指す。永続化の実装担当・保存単位・唯一writer・transactionを指定しない。通常ownerは正当な全域消去への参加を拒めない。

### 3.1 保全・消去が意味ownerとなる state

所有の理由は、保持方針が記録の保全期間・消去範囲、全域操作状況が横断処理の成立・未完了、backup設定・作成結果が復旧用copyの作成・管理、Audit順序・保持とDebug成立がdata保全の固有契約を表すためである。元事実の意味は発生元に残す。この理由を持たない一般運用設定は集めない。Host自動起動の選択は入出力・提示に置く（SO 4.17）。

| State | 管理する意味・立場 | 他stateとの違い |
|---|---|---|
| History / 対象log / Audit等の保持方針 | Ownerが選んだ対象・期間・手動削除の範囲と影響。 | 対象種別ごとの契約を保ち、Learningや根拠へ一律の保持期間を適用しない。自動削除は既定で行わない。 |
| Learning revision・Summary等の容量管理方針 | 自動retention / cleanupは既定OFF。Ownerの明示的な有効化により設定可能。 | 通常の忘却やHistory / log保持整理とは別。認識・学習が対象の意味と根拠・復帰への影響を示し、保全・消去が明示方針の適用を調整する。 |
| Targeted Deletion・Reset・Restore等の操作状況 | 目的・対象範囲、参加先、影響、完了した範囲、未完了・失敗・検証結果、必要な処理保留。 | 完了根拠は各参加ownerから得る。domainの通常変更判断や許可状態を所有せず、共通の一状態機械を課さない。 |
| Backup設定・作成結果 | 保存先、backup独自schedule、保持数、保護選択、対象時点・対応性、結果・失敗。 | Task Scheduleとは別。作成に担当CompanionやTaskを必須にしない。 |
| 作成済みbackup | 内部状態の対応関係を持つcopy。 | live stateの正本ではない。保存先が同じHostでも外部copyとして境界を保つ。明示restoreを経ずに内容を通常参照・実行の正本へ戻さない。 |
| Auditの追記順・保持 | 供給された事実をどの順序で監査記録として受け入れ、保持しているか。 | 元のAction成功やPermission妥当性の再判定ではない。追記順は永久不変・削除不能を意味しない。 |
| Debug captureの成立 | 明示的な対象・内容・有効期間・停止・削除と全体の成立。 | 通常記録とは別の短期・明示有効化の契約である。実際のcaptureは各対象責務が参加する。 |

Backupの保護はOwnerが選択し、暗号化を利用できる。非暗号化で作成する場合はprivate dataを含むことを事前に示す。内部削除と、Ownerが選んだbackup保持方針によるcopyの整理は別の操作である。

全域操作の未完了・再保存防止等、継続して守る必要のある状況はClientや一回の処理の寿命だけに置かず、Hostで保全する（PE-7）。操作途中の再起動でも完了と誤認したり保留を黙って解除したりしない。記録に削除対象の秘密・private本文を残すことは完了根拠の代わりにならない。

復元後の保留理由と復元成立は保全・消去、現在の実行許可・制限の有効性は権限・制約、Task等の実際の再開は各活動ownerが担う。これらの状況を単一の「保存済み / 実行可能」へ潰さない。

### 3.2 保全・消去が所有しない state（既存ownerに残る）

本節の列挙はSO第4節の再掲ではなく、保全・消去が取得しないことの確認である。**本節のすべてを除いても、各domain stateのsemantic ownerを一意に説明できる。** 保全・消去を除去しても次の対応は失われない。

- Characterの静的構成・revision：Character。適用関係は個体調整。
- Companionの同一性・活動状態・適用済み構成：個体調整。
- 会話の参加・継続、Historyと非会話活動記録・evidenceの意味、進行中の意味判断：個体調整。入出力roundの実際は入出力・提示。
- 未伝達事項と報告状況：個体調整。
- Summary・根拠関係・source参照：認識・学習。
- Memoryの現在認識・重要度・scope・過去revision：認識・学習。scope意味は認識・学習、決定後の強制は権限・制約と各利用箇所。
- 内部Skill・import原本との対応・学習revision・実行結果：認識・学習。
- Relationship：認識・学習（主体別の現在解釈）。
- Companion Stateの一時状態と持続的傾向：認識・学習。
- Taskの目的・担当・進捗・結果とTask context：作業。
- 委任と一時Task Agentの実行状態：作業。
- Actionの実行状況・作用の確定度・停止結果：実行・拡張。
- Workspace関連付け・保存先・内部copy・中間fileの意味：関連付け・Task context・中間fileの用途は作業、外部実体はOwnerまたは外部system、取得結果の由来・確定度は実行・拡張。
- Schedule設定・到来した回・各回のTask：作業。
- 接続の事実・active帰属：接続・存在。pairing・device許可の意味は権限・制約。
- 観測設定・観測候補・自発性の設定：観測運用は共有観測、自発性は個体調整、強制上限は権限・制約、Mute・入出力状況は入出力・提示。
- 一般設定・Body・Voiceの出力と一時状態・Host自動起動の選択：入出力・提示（OS作用は実行・拡張、全域参加調整は保全・消去）。
- Provider接続情報・能力情報と割当の解決：登録・能力観測・解決は推論、割当同意は権限・制約。
- Rule・Permission・同意・禁止・上限と現在の有効性：権限・制約。
- 利用量・費用・資源の記録：原記録は各利用owner（Provider分は推論）、上限に対する可否は権限・制約。
- Credential値・用途・参照元・認証状態：認証秘密。参照元接続は各接続owner。
- MCP・Pluginの接続 / 受入設定と外部実行状態：実行・拡張（sandbox外許可の正本は権限・制約、秘密値は認証秘密）。
- Client固有の接続材料：端末に属する別分類。利用・更新・再pairingは接続・存在、秘密保護は認証秘密、Host側信頼・許可・失効は権限・制約。具体形式・保存方式は固定しない。

通常時のdata accessやmutationの必須中央経路を保全・消去にしない。通常の参照・形成・委任・送信・保存・提示・作用は各ownerと各利用箇所の契約で成立し、保全・消去を経由しない。保全・消去が関与するのは、全域操作の調整・保持方針の適用調整・Audit / Debug保全・操作状況の保全に限る。通常のLearning形成・Task遂行・会話・観測・推論利用を保全・消去の応答待ちにしない。管理経路の成立にもLLM・長時間Task・Body・Voice・拡張の成功を挟まない（DR 7.4）。

### 3.3 canonical / derived / temporary / external reference の扱い

- canonicalとして参照するもの：各domainの意味ownerが管理する現在認識・原記録・Task進捗・作用確定度・制御条件・認証用途等（第3.2節）、保全・消去が管理する保持方針・操作状況・backup設定・Audit順序・Debug成立。保存されたAllow・判定copy・解決済み経路・推論context内の許可文・cached判定・有効経路・次回表示・費用集計・Client表示・過去帰属・hint・復旧先記録・Provider残存・backup copy・部分置換状態はcanonicalにしない。
- derivedとして扱うもの：報告用要約・由来説明・進捗表示・Body・Voice出力・embedding・index・similarity・score・Prompt cache・Provider session・一時context・有効経路・次回表示・費用集計・解決済み割当経路。派生物の生成・保持・破棄を行う責務が元stateとの対応と利用範囲を説明し、訂正・scope変更・targeted deletionへ参加する。古い派生物から現在状態・権限・帰属を復活させない。
- temporaryとして扱うもの：Raw Observation・Raw Voice・詳細Tool payload・内部推論・chain-of-thought（通常保存しない）、Client表示copy・入力途中・音声buffer・観測候補・推論中context・MCP Apps表示data、実行中buffer。失ってよいのは一時表現であり、受理済み指示・必要作業記録・未伝達・作用不明・全域操作未完了まで失ってよいわけではない。
- external referenceとして扱うもの：外部Workspace実体・案内file・Skill・成果物、外部Package・Skill原本・export済みcopy・Owner保存backup、Provider / MCP側固有状態・保有copy。内部保持copy・内部取込内容・ene管理下の一時dataは内部の利用・消去責任に戻る。外部code由来を消去除外条件にしない。

Experienceは出来事と結果の概念であり、全活動を格納する新しいcanonical Raw storeを設けない。Summaryは保持すべきevidence、backupは保持されたcopyであり、どちらも現在のMemoryの代替正本ではない。

## 4. Subsystem間の受渡し（semantic contract）

「AがBを呼ぶ」の実装構造を先に決めるのではなく、要求開始・authoritative判断・受渡内容・失ってはならない前提・結果受入を定める。非同期message・同期call・shared state等の mechanismは後続へ残す。

DP番号は本書内のcontract参照用である。

### DP-0 協調の共通原則（ownership takeoverの禁止とglobal completionの必要性）

- 横断操作では、coordinationのownerが操作の成立条件を揃え、各state ownerが自分の意味・対象・利用中dataへの影響と処理結果を引き受ける。Coordinatorが全domainの通常意味判断を代行する構造にも、参加者が共同所有として判断を相互に委ね続ける構造にもしない。
- 通常ownerは正当な全域消去・復元・Reset・保持整理への参加を拒めない。保全・消去も任意の通常変更権を取得しない。保存・利用先を増やす後続設計は、その参加責任も示す必要がある。
- 参加責任には、対象state、元記録・根拠・revision・共有参照・派生物・内部copy、現在利用する処理・遅延結果の把握、必要な変更・保留、完了または未完了の報告が含まれる。「消した」「保存した」という局所結果だけでは、横断操作の完了を証明しない。
- 各ownerは局所結果を全体完了前に返せるが、結果返却だけで必要な保留を解除しない。対象外の通常活動まで一律停止することは必須にせず、対象範囲の利用・再保存を確実に制限できることを成立条件にする。
- 完了の説明は参加先の根拠に基づき、LLMが納得したという要約で代用しない。各ownerは自身の対象stateだけでなく、保持copy・revision・根拠・共有参照・派生物・処理中利用・遅延結果への関係を参加させる。

### DP-1 Targeted Deletionの受渡し

Step 11 `targeted-deletion.md` の一般契約を、PE-1とPE-7の調整と各ownerの参加へ落とす。一般契約自体は再定義しない。

| 要素 | authoritativelyに扱う責務 | 受渡し・参加 |
|---|---|---|
| 開始・目的・対象の確定 | 保全・消去（PE-1）。Ownerの明示的なPrivacy / Security目的と対象記述・影響・除外の対応付け。保存場所をOwnerへ選ばせないこと。「忘れてほしい」等だけでは本操作にしないこと。 | 個体調整・認識・学習等が由来・文脈の意味的特定を支援する。機械的条件の検証省略・既知依存の追跡省略の理由にしない。 |
| 対象記述の二層 | 保全・消去が機械的条件（必須・LLM非依存）と意味的条件（補助・完全性なし）の区別を保つ。 | 意味的探索の補助推論にもその推論自身の割当同意・認証用途・費用制限を適用する。審査対象Actionの許可・実行を先行条件にしない。 |
| 参加すべき領域 | 保全・消去が参加先の特定支援を担い、各ownerが自分の保持・利用範囲の参加を引き受ける。 | 参加先は `targeted-deletion.md` 第4.2節の導出に従う。History・非会話記録・未伝達、Summary・Memory・Skill・Relationship・Companion State、Task・Agent・Action・Workspace・Schedule、接続・帰属・Client一時data、観測候補・routing派生物、Body・Voice一時、推論context・cache・index・embedding、Rule・利用量・Credential露出copy、MCP・拡張一時data、Audit・Debug・保持方針・操作状況。列挙漏れを不参加の理由にしない。対象を復元できる内部state・過去根拠・派生物・一時data・処理中利用を持つ責務は列挙の有無にかかわらず参加する。Character静的revisionも対象情報を復元できる内容を持つ場合は参加する。 |
| 各参加者の局所結果 | 各owner。自分の範囲の除去または復元不能化、必要な保留・再保存防止の適用、範囲内の検証。 | 局所結果の返却だけで必要な保留を解除しない。対象外の通常活動まで一律停止することを必須にしない。 |
| pending / failure / unreachable | 保全・消去（PE-7）が区別を保全する。到達不能・確認不能を成功に読み替えない。 | 接続・存在が消去中の接続変化・確認不能を参加先へ結び付け、入出力・提示等が一時dataの消去状況を報告する。必要な確認を省いて完了としない。到達性確認・無効化の具体方式は残す。 |
| 再起動を跨ぐ未完了 | 保全・消去（PE-7）がHostで保全する。 | 参加ownerは全体完了前に局所結果を返せる一方、局所結果を返したことだけで再保存防止を解除しない。 |
| 全域完了 | 保全・消去（PE-1）が確定する。条件は `targeted-deletion.md` 第8.2節の全体（影響説明と確認、内部全域の除去または復元不能化、区間内再到着の取込み、機械的残存検証、未完了集約、本文非再保存）である。 | 単一store・単一coordinatorの成功・LLM納得で成立させない。認識・学習だけへの要求では個体調整のHistory・作業のTask context・推論のcache・Clientや拡張の一時dataを覆えない。 |
| Ownerへの説明 | 保全・消去（PE-7）が事実と本文を分けて説明する。入出力・提示が管理面へ提示する。 | 保持すべき事実と保持してはいけない対象本文の区別（第4.7節）を守る。外部送信・export・backup済みcopyまで削除したと表示しない。内部露出copyの消去と外部Credentialの失効・更新は別操作として説明する。 |

新規開始の禁止と進行中の扱い、用途別受入での消去条件適用、確定度の保持、LLMに依存しない強制境界、消去区間の再到着・再生成の取込み、旧由来と新規提供の区別、cache・session・Client経路の再利用禁止、共有根拠の分離は `targeted-deletion.md` 第5〜6節の契約をそのまま適用し、本書で緩めない。権限・制約と各実利用先が参照・変換・送信・保存・提示・作用の箇所で消去条件を適用する。消去を理由にAction outcomeの確定度を成功・失敗・未実行へ変換しない。

Companion deletionとの関係：Companion deletionのlifecycle調整は個体調整に残る。保全・消去は内部削除の全域成立の調整にのみ参加する（SO 6.3、DR 7.5）。Companion deletionは個体固有の現在状態・学習状態等の削除であり、過去記録を消す操作ではない。Targeted Deletionが重なる対象本文は、Companion削除後の残存記録からも除去する。Summaryをhistorical logへ分類し直して残さない。

### DP-2 Backupの受渡し

Step 11 `backup-restore.md` 第3・6節の一般契約を、PE-2とPE-7の調整と各ownerの提供へ落とす。

| 要素 | authoritativelyに扱う責務 | 受渡し・参加 |
|---|---|---|
| 開始 | 保全・消去（PE-2）。Ownerの要求または有効なbackup設定を契機とする。Task Scheduleや担当Companionの稼働を必須にしない。 | 非暗号化ならprivate dataを含む説明を事前に行い、暗号化も選択可能にする。 |
| 対象・範囲 | 保全・消去が対象時点・内部範囲・参照対応・除外・保護を各ownerと照合する。 | 各ownerは復旧可能な対応で内容を提供する。個体削除後の残存記録も含め、認証秘密の除外と外部実体の非収集を守る。範囲は `backup-restore.md` 第3.1節の対応（Character・Companion・History・非会話記録・未伝達、Summary・Learning・関係・内的状態、Task・Agent・作用・Workspace関連付け、Schedule、制御条件・Provider非秘密・MCP受入、利用量・Credential非秘密参照・Audit・保持方針・操作状況）である。派生物・一時dataは独立復元対象にしない。Client接続材料のsecret部分は復元しない前提で除外する。 |
| 各参加者の局所結果 | 各owner。復旧可能な参照対応での提供。 | 単に各部のcopyが作れたことだけをfull backup成功にしない。対象時点・参照・必要な履歴と未完了状況の対応が揃って初めて成功とする。 |
| pending / failure | 保全・消去が作成結果・失敗を確定する。 | 実行・拡張へ許された保存先への出力を要求し、作用結果と参照整合の両方を確認する。失敗時は最後の正常状態を破壊しない。許可や保存結果を誤って成功表示しない。 |
| 再起動を跨ぐ未完了 | 保全・消去（PE-7）が保全する。 | 未完了の消去・復旧とbackup作成が重なる場合、その制約を無視した正常・即実行可能なcopyを作らない。作成を待たせるか未完了・制約も復旧可能に含めるかは自由度である。 |
| 全域成立 | 保全・消去（PE-2）が確定する。 | 処理中memoryの丸ごと保存を要求しない。意味上の継続に必要な内部状態を対応させ、一時buffer・Provider session等の復元を前提にしない。実行中のAction・in-flightの不明がある場合、最後の正常記録が外部世界の最新状態とは限らないことを保つ。不明を未実行へ戻して正常copyと偽らない。 |
| Ownerへの説明 | 保全・消去が作成結果・失敗を示す。Ownerは保存先・schedule・保持数を選択できる。 | 作成済みbackupはlive正本にならないことを保つ。明示restoreを経ずに古いcopyをlive正本として読み戻さない。 |

正常保存・migration・対応upgradeも、PE-2の調整対象である。各ownerの意味整合と保全・消去の復旧可能性を対応させ、成功まで最後の正常状態を守る。Upgrade前に対応性・必要なbackupを確認し、失敗理由を示す。未対応downgradeを成功する前提にせず、起動前に示す。

### DP-3 Restoreの受渡し

Step 11 `backup-restore.md` 第3〜5・7・9節の一般契約を、PE-3とPE-7の調整と各ownerの確認へ落とす。

| 要素 | authoritativelyに扱う責務 | 受渡し・参加 |
|---|---|---|
| 開始・受理・説明 | 保全・消去（PE-3）。対応backupの選択、現在のCredential storeを維持する全置換・version対応・外部非変更・削除済み情報や旧Rule / 同意 / Scheduleが戻り得ること・再認証の可能性の説明。初回Setupからのrestoreも同じ契約を通る。 | 各ownerを復元へ参加させ、旧live状態の進行中利用・遅延結果・Client copyを復元後の正本と混同しない条件を揃える。必要な活動・外部接続の保留は権限・制約と各活動ownerが適用する。 |
| 対象・範囲 | 保全・消去が現在のCredential store secretを除く対象内部dataの全置換として調整する。旧liveとのmergeではない。 | 各ownerが復元内容の対応・参照・利用可能性を確認する。認識・学習はCompanion Stateの経過時間を扱い、接続・存在は保存された接続・帰属を現在の到達性とみなさない。認証秘密は開始前のstoreを維持し、接続ownerと復元参照を現在の用途・有効性と照合する。外部Workspaceの現在内容・存在・accessはbackup時点へ戻らない。 |
| 現在維持 | 認証秘密が現在のstore維持・照合・再認証要求を、権限・制約と活動ownerが保留を担う。 | 現在のCredential store・外部現実・現在の到達性・未完了の保留は維持され、復元参照の存在から巻き戻したとは扱わない。Auditはbackup時点へ置換されるが、置換成立後のRestore事実は新しいAudit事実として追記する。 |
| 自動再有効化の禁止 | 権限・制約が現在可否を確定し、各利用箇所が適用する。 | 復元されたassignment / consent / Rule等だけで現在利用・自動処理を開始しない。復元成立・一括有効化・現在条件を別に満たす。一件ずつの再承認は要求しないが、Deny・cap・認証不足・不明を無視しない。復元後のTask・Schedule・外部接続による自動処理は一旦保留し、Ownerが確認してまとめて有効化できる。 |
| 新しい正本として扱える時点 | 保全・消去が全置換の成立を確認して初めて復元内容をHost正本とする。 | 対応読解・version対応・参照対応・除外の確認、意味上の全置換の成立、参照・利用可能性の確認の対応、失敗時の復元前正常維持を満たす。旧liveと競合する二重の正本を作らない。復元成立と実行再有効化は別々に確認する。 |
| staleの扱い | 各ownerが現在性の確認を担う。 | staleなPermission・Provider・Client・作用結果・外部参照を現在事実にしない。dangling参照は未解決とし、不明は不明のまま保持し、自動replayしない。Companion Stateの一時固定・不自然な初期化をしない。Schedule timezoneを黙って置換しない。利用量を現在の残額の根拠にしない。 |
| live activityとの隔離 | 保全・消去が隔離・保留の条件を揃え、各ownerが用途別受入で由来を区別する。 | 切替前に開始した推論・Tool結果が切替後に届いても復元正本へ混ぜない。旧live作用の必要説明と復元対象の記録を区別する。Client copyでHostを上書きしない。 |
| 失敗・再起動 | 保全・消去（PE-7）が単一正本・非混合・権限先行復活の禁止・成功表示の条件・再起動時の保全を守る。 | 保留中・確認途中・置換途中に再起動しても、復元済みassignment / consentだけで自動利用を開始しない。部分置換を新正本にしない。到達不能・確認不能を成功に読み替えない。 |

### DP-4 旧backupのRestoreの受渡し（Owner decisionの落とし込み）

以下は確定済みの製品判断であり、本書で変更しない。Step 11 `backup-restore.md` 第8節の確定契約どおりに各参加責務へ落とす。

- 過去BackupをTargeted Deletion完了後に自動改変・再消去しない。Backup作成後にTargeted Deletionされた情報が古いBackupに含まれていても、保存済みcopyを黙って書き換えない。
- Restore前に削除済み情報が再導入され得ることをOwnerへ説明する。旧Rule・同意・Scheduleが戻り得ることと合わせて、PE-3の受理・説明で示す。
- Ownerが理解して明示的にRestoreした場合、それを過去stateの意図的な再導入として扱う。戻った情報は新しい正本として扱い、新しいExperienceの根拠になり得る。自動再形成禁止の迂回ではない。
- 過去のTargeted DeletionをRestore後に自動再適用しない。改めて消去するには新しいtargeted deletionの明示が必要である。削除前の根拠だけによる自動再形成を許す例外にしない。
- cache、session、delayed result等による意図しない再出現とは区別する。前者はPE-1の再保存防止で防ぐ対象であり、後者はOwnerの明示判断による別操作である。復元成立後の保留・一括有効化・現在条件の再評価を経ずに戻った情報を送信・作用・学習へ自動利用しない。
- Restore事実はAudit可能にするが、削除対象本文をAudit目的で別保存しない。完了記録・Audit・Owner説明自体へ対象private本文を再保存しない。復元による復活を外部送信・export・backup済みcopyの消去保証と混同しない。
- 未完了の消去とbackup / restoreが重なる場合は、制約を無視した正常・即実行可能なcopyを作らず、未完了・保留・再保存防止をHostで保全する。再起動・再接続・restore後も未完了の認識と必要な保留を維持する。
- 消去区間に重なるRestore由来の再到着は、各受入・保存先が進行中の対象情報についての消去条件を適用し、古い根拠だけからの再形成・遅延再保存を防ぐ。指定文字列は機械的な検索・除去・残存検証へ参加させる。

新しいPrivacy semanticsは本書で決めない。上記は既決の「自動再形成の禁止」と「明示restore」の両立であり、外部copyまでの消去保証・復元後の自動実行許可の追加ではない。

### DP-5 Resetの受渡し

| 操作 | Coordinationと参加責任 | 成立条件 |
|---|---|---|
| 設定Reset | 保全・消去（PE-4）が一般設定へのReset範囲を調整し、各設定ownerが既定化と保護対象の保持を確認する。 | UI・Body・Voice等の一般設定を戻し、個体・履歴・Summary・Learning・関係・内的状態・Task・Schedule・Credential・Permission Rule・Provider同意・費用capを削除しない。「設定」という名称だけで対象を広げない。 |
| 全データReset | 保全・消去（PE-4）が列挙と強い確認、内部消去、処理中data・Client一時copyの扱い・信頼失効を調整し、認証秘密を含む全ownerが参加する。 | Host内部ene dataとCredentialが削除され、旧処理・一時copyから内部状態を戻さない。Host側のpairing・device信頼も失効し、旧端末材料だけで復活させない。到達不能Clientの物理消去を確認済みとしない。通常の個体削除とは異なり、内部historical recordも保持対象にしない。外部Workspace・外部Skill・Ownerが別保存先へ作成したbackupを削除せず、何が残るかを示す。局所消去や画面の初期化だけで完了としない。 |

Reset後は、何が削除され、何が外部に残っているかを確認できる。設定Reset・全データReset・対応upgrade・Restore・通常再起動を同じ復旧操作に潰さない。

### DP-6 retention / cleanupの受渡し

通常retentionはTargeted Deletionとは異なる。**容量都合による自動削除はdefaultでは行わず、Ownerが明示的に自動retention / cleanupを有効化した場合のみ行う。**

| 操作 | Coordinationと参加責任 | 成立条件 |
|---|---|---|
| 通常History / log削除・保持期間変更 | 保全・消去（PE-5）がOwner指定の対象・影響・保持適用を調整する。個体調整・作業等が対象記録、認識・学習が残る根拠関係を確認する。 | 指定対象が整理され、形成済みLearning等・Summaryへ削除がcascadeしていない。失われたsource参照を説明でき、外部copyまで消したと表示しない。既定では自動削除しない。 |
| Learning revision・Summary等の明示retention cleanup | 保全・消去（PE-5）がOwnerのopt-in設定に従って適用を調整し、認識・学習等の既存ownerが対象と残る参照の整合を扱う。 | 既定OFFで、明示設定なしに容量不足を理由として過去revision・根拠を削除しない。設定範囲と根拠参照・revision復帰への影響を説明し、通常の忘却やtargeted deletionへ読み替えない。削除済みのrevisionや根拠を保持・復帰可能であると扱わない。data class・期間・容量・優先順位・algorithmは固定しない。 |

retentionによってMemoryの意味上の「忘却」やLearning revisionを暗黙に上書きしない。通常の忘却は想起の抑制であり、保存済み内容・過去revision・根拠を削除しない。失効・置換・統合も同様で、容量上限を保持の迂回に使わない。通常History / log整理の場合は、形成済みLearning・Summaryを残してsource参照不能を扱う。容量不足だけでopt-inが成立したと扱わない。

Companion削除・Targeted Deletion・retention・Reset・Restoreは別の操作であり、同じ削除mechanismを使えても目的は変わらない。

### DP-7 Audit / Debugの受渡し

AuditやDebug dataが、新しい秘密保管場所・Targeted Deletionの抜け道・staleなcanonical state・Action outcome確定度を強める根拠にならないようにする。一方、全域操作の成立・failure・Restore事実等をOwnerへ説明・検証できるために必要な非秘密・非対象本文の情報まで失わせない。

| 要素 | authoritativelyに扱う責務 | 受渡し・参加 |
|---|---|---|
| Auditの原記録の意味 | 各発生元。Action結果は実行・拡張、Permission・同意変更は権限・制約、Credential参照・失効は認証秘密、Task状態は作業、削除・Reset・restoreの成立は保全・消去等が事実を供給する。 | Auditの説明から元stateを再構成・直接更新する逆の正本関係を作らない。現在の有効状態は元ownerへ参照を戻す。 |
| Auditの順序・保持 | 保全・消去（PE-6）。供給された事実をどの順序で監査記録として受け入れ、保持しているか。 | 元のAction成功やPermission妥当性を再判定しない。訂正情報も由来と順序を保つ。追記順で確認可能であることを永久不変・削除不能へ拡張しない。 |
| 保持する内容 | 各供給元が必要事実を供給し、保全・消去が受け入れる。 | Auditには通常、会話・file本文、秘密値、削除済みprivate内容を含めない。各供給元が不要な本文・秘密を出さず、記録側も別保管庫を作らない。保持方針・保存量・削除影響をOwnerが管理できる。targeted deletionでは残った対象情報も除去・復元不能化する。完了記録・Audit・Owner説明自体へ対象本文を再保存しない。 |
| Debug capture | 保全・消去（PE-6）が明示的な対象・内容・有効期間・停止・削除と全体の成立を管理し、実際のcaptureは各対象責務が参加する。 | 通常記録とは別の短期・明示有効化の契約で、登録済みCredentialを含めない。内部推論や全Rawを通常保存する理由にしない。Debug captureに登録外の秘密値や不要な本文が含まれる可能性を示し、停止と削除をOwnerが行える。 |
| 確定度の扱い | 各事実owner。 | 由来・失敗・停止不能・未保存・次の安全な選択肢は、内部思考や詳細Promptを公開せず説明する。Task達成・作用・保存・提示・Auditの確定度を要約・翻訳・復旧で強めない。監査記録を元stateの再生・自動実行の入力にしない。遅延・訂正された報告も元対象との対応を保つ。 |
| Telemetry | Owner。 | 自動送信せず、共有する場合はOwnerが内容と送信先を確認して手動で行う。 |

Credential secretはBackupへ含めない。Restore時は現在Credential storeを維持する（DP-3）。Audit・Debug・backupへ秘密値を流さないことは、認証秘密と各受入・保存・提示箇所の協調で保つ。

### DP-8 全域完了とfailure / restartの共通扱い

具体的なstate machineやtransaction protocolは固定しない。後続設計は少なくとも次の論理的な区別を保つ。

- **局所完了。** ある参加先が、自分の保持・利用範囲について、対象の除去または復元不能化・提供・確認・保留の適用・範囲内の検証を終えたこと。局所結果の返却だけで必要な保留を解除しない。
- **pending / unreachable。** 消去・復元・Reset中の接続変化・切断・再起動等により、ある範囲の処理・検証・確認が未確定であること。到達不能・確認不能を成功に読み替えない。
- **failed verification。** 機械的残存検証で対象文字列が残る、依存関係が解消できない、意味的対象範囲が確定できない、参照対応が揃わない等により、ある範囲の完了を確定できないこと。その不足を示す。
- **全域完了。** 保全・消去が各参加先の局所完了・検証・未完了を対応付け、内部全域の成立を確定したこと。局所完了の集合以上の条件（DP-1〜DP-5の各成立条件）を必要とする。

完了扱いにしてはいけない場合：局所削除・局所copyだけが済み別の保存先・context・Clientから戻る可能性が未処理の場合、pending / unreachableが残り必要な確認を飛ばす場合、機械的残存検証が失敗・未実施で残存を否定できない場合、消去区間の再到着・再生成の取込み・遅延再保存防止・古い根拠からの再形成防止のいずれかが未成立の場合、意味的探索の検出限界を既知依存の追跡省略・未確認範囲の完了扱いに読み替える場合、外部copyの消去を内部完了の条件に含める場合、ene管理下の内部copyを外部扱いして除外する場合、完了記録・監査・説明へ削除本文を戻すことで完了根拠自体が復元源になる場合、部分置換・混合・権限先行復活がある場合。

参加ownerは局所の処理・検証結果を全体完了前に返せる。coordinatorはこの責任に依存し、全内部構造への無制限なaccessを要求しない。循環した承認・成功待ちをarchitecturalな前提にしない。

## 5. Lifecycleとの関係

lifecycle変更の多くはownerの移転ではなく、同じownerが管理する状態の存続・利用可能性・参照範囲の変更である。異なる目的の操作を一つの初期化・一つの状態機械へ潰さない。

| 事象 | 本Subsystemが変更する意味 | 維持・分離する意味 |
|---|---|---|
| Companion生成・Character部品更新 | 保全対象としての内部Character・適用関係の対応付けに参加する。 | 経験状態の上書き・配布物への混入をしない。Character内容・revisionはCharacter、適用関係は個体調整、経験状態は認識・学習に残る。 |
| Companion停止 / 再開 | 内部削除の全域成立の調整には参加しないが、停止中の回をmissedにする等の保持契約を保つ。 | 同一性とdataを保持する。停止中の活動禁止・best-effort Cancelは個体調整・作業に残る。 |
| Companion削除 | 個体調整のlifecycle調整に対し、内部削除の全域成立の調整に参加する。固有Summary・Companion scope・Skill過去revision・Companion State・主体 / 相手Relationship・担当Scheduleの削除確認、残存記録の対応付けに参加する。 | 一対一・グループ・Companion間交流History、非会話活動記録・evidence、Task記録は個体削除だけでは消さない。Global Learning・残す共有Summary・外部fileを維持する。自動Global化・Schedule自動引継ぎをしない。Summary・revisionをhistorical logへ分類し直して残さない。 |
| Task Agent終了 / 失敗・Task完了・失敗・Cancel | 作用不明・未伝達・全域操作未完了の保全に参加する。 | Task記録・Companion・Scheduleを削除しない。記録・関連付けを終了だけで削除せず、外部成果物を所有・消去しない。 |
| Task削除・Schedule停止 / 削除 | 通常削除の保持整理の調整には参加するが、形成済みLearning・Summaryへのcascadeをしない。 | 既存各回の結果を変更せず、missedを実行済みにしない。 |
| Learningの通常忘却・訂正・統合・失効・置換・scope変更 | 保持・消去の対象外として維持し、消去条件・保留との区別を保つ。 | 保存済み内容・過去revision・根拠を保持し、誤りと時間的変化を区別する。scope意味は認識・学習に残る。 |
| 通常History / log削除・retention cleanup | PE-5の調整対象である。 | 形成済み状態とSummaryは残し、Raw参照不能を明示する。消去目的をtargeted deletionへ変えない。 |
| Targeted deletion | PE-1・PE-7の調整対象である。復元に寄与する内部dataと処理を横断して除去・復元不能化する。 | 通常ownerは移らず、完了後のOwner新規提供・外部copy消去保証と分ける。 |
| 移動・切断 / activeなし | 接続中Clientの一時dataを扱う参加先の把握を支援する。 | 個体・通常Host作業・内部状態のownerは移らない。未確定Actionを移動先へreplayしない。 |
| Fullscreen・Observer Pause / OFF・Mute | 観測・提示の抑制として維持する。 | 個体停止・Host Task停止・Learning削除とは別である。 |
| Host再起動 | 未完了・保留・再保存防止のHost保全を維持する。 | 過去作用不明・未伝達・未完了消去をなかったことにしない。途中Taskは明示再開待ち、停止中の回はmissed、Running presenceは元Clientへの復旧に従う。 |
| Provider / model変更・障害・cache失効 | 推論の利用可能性・能力・承認済み経路の扱いを前提に、機械的検証・停止・拒否・管理を塞がない。 | 個体・履歴・Learning等を初期化せず、同じ情報選択方針と安全境界を維持する。 |
| Backup / restore | PE-2・PE-3・PE-7の調整対象である。copyを作成し、明示restore成功で現在のCredential storeを除く対象内部状態をbackup時点へ全置換する。 | copyはlive正本でない。復元後も実行は保留、外部世界は巻き戻らず、Credentialは現在storeを維持して照合する。 |
| 設定Reset / 全データReset | PE-4の調整対象である。前者は一般設定の既定化、後者は強い確認後の内部data・Credential削除。 | 前者の保護対象と後者の外部除外を混同しない。旧処理・一時copyから内部状態を戻さない。 |

Globalへ形成・変更済みのLearningは、その後に由来Companionが削除されてもGlobalとして残る。ただし削除時に残す根拠と失うsource参照を説明する。RelationshipとCompanion StateはCompanion / GlobalのLearning scope変更による共有対象にしない。

時間はstateの意味にも作用する。Scheduleは選択時timezoneと時刻条件、Companion Stateは一時性と経過時間、保持方針・Debug captureは対象期間、自発性は現在時刻・Quiet hoursをそれぞれのownerが利用する。選択済みSchedule timezoneを黙って書き換えない。Backupに含まれるCompanion Stateをrestoreするときも、backup後に経過した時間を無視して時間的に一時的な状態を保存時点の値のまま無期限に固定しない。

## 6. failure / cancellation / restart時の責任

Cancel・切断から外部作用の取消・不存在を推測せず、不明を未実行に戻さない。状態参照・Client移動をAction replayの理由にしない。

- 保全・消去共通：未完了・確認不能をHostで保全し、Client切断・再起動・操作画面終了から完了を推測しない。完了の説明は参加先の根拠に基づく。到達不能・確認不能を成功・現在へ読み替えない。局所処理の報告を全域完了待ちにして循環させない。保留・再保存防止は必要な期間維持し、保留解除と局所完了を同じ意味にしない。記録に削除対象の秘密・private本文を残すことは完了根拠の代わりにならない。一時bufferの消失は成功や完了の根拠にしない。
- PE-1（消去）：Host restart・Client disconnect・owner一時利用不能・verification failure・delayed result・削除中の再到着があっても、未完了を成功へ変換せず、再保存防止条件を失わせない。削除前の情報を利用する実行中処理による再保存を防ぐ。古い根拠だけからの自動再形成を防ぐ。確認不能なClientや外部処理を成功へ読み替えず、局所完了を全域完了にしない。
- PE-2（保全・Backup）：保存中・migration中の失敗で最後の正常保存された状態を破壊しない。未完了消去・復旧と重なるbackupを正常・即実行可能なcopyと偽らない。実行中の不明がある場合、最後の正常記録が外部世界の最新状態とは限らないことを保つ。
- PE-3（Restore）：Restore中断・再起動があっても、復元前の正常状態を破壊せず、部分置換を新正本にしない。旧live結果・Client copyを復元正本へ混ぜない。保留中・確認途中に再起動しても、復元済みassignment / consentだけで自動利用を開始しない。未完了の消去保留を置換で解除しない。
- PE-4（Reset）：全データReset後に旧処理・Client copyから内部状態を戻さない。Host側の信頼を失効させ、旧材料だけで再接続の信頼を復活させない。到達不能Clientの物理消去を確認済みとしない。
- PE-5（retention）：容量不足・Provider費用・LLM利用不能を理由に、機械的な消去・検証・正常保存・必要事実の記録を追加推論待ちにしない。保存容量不足を通常Learning・過去revision・根拠の黙った削除で解決しない。
- PE-6（Audit / Debug）：Audit追記順の確認と保持管理、削除後にprivate内容を残さないことを、診断用dataの無制限な別保管庫にしない理由とする。Debugの短期失効・停止・削除を守る。
- 共通：受付・開始・作用・記録保存・Task達成・報告は別の事実であり、一つの成功状態へ潰さない。確定度を変える場合は事実ownerが新たに確認した根拠を用いる。Background・Scheduleの無人実行も同じ条件に従い、Client不在・Schedule到来・復元成立を自動承認の理由にしない。Owner確認が必要なら実行せず判断待ちにする。管理経路をLLM・長時間Task・Body・Voice・拡張の成功に従属させない。

### 6.1 Hostで保全すべきpersistent property

具体的なjournal、WAL、transaction、lock、distributed protocolは選ばない。後続設計は、次の性質を満たす保持を選ぶ。

- 消去要求の目的・対象記述・影響・除外・必要な確認との対応。
- 各参加先の処理・検証結果と、未完了・失敗・未確認範囲。
- 対象範囲の利用・再保存を制限するための消去条件と、その有効区間・完了境界。
- 局所完了と全域完了の区別、pending / unreachable / failed verificationの区別。
- Backupの対象時点・参照対応・除外・未完了状況、作成結果・失敗。
- Restoreの受理・説明・隔離・照合・置換成立・復元後保留・一括有効化の対応、単一正本・非混合の成立。
- Resetの列挙・強い確認・信頼失効の対応。
- 保持方針・明示opt-in設定とその適用範囲・影響。
- Auditの追記順・保持、Debugの対象・内容・有効期間・停止・削除。
- 再起動・再接続・restore後も、未完了の認識と必要な保留・再保存防止を維持すること。操作途中の再起動でも完了と誤認したり保留を黙って解除したりしないこと。
- 記録に削除対象の秘密・private本文を残すことは完了根拠の代わりにならないこと。

全域操作の未完了・再保存防止等、継続して守る必要のある状況はClientや一回の処理の寿命だけに置かず、Hostで保全する。継続して必要なTask状態・未伝達・作用不明・全域操作未完了等を本来のownerが保全する。

再構築可能であることは、全文contextや内部推論を永続保存する要求ではない。通常保存しないRaw Observation・Raw Voice・詳細Tool payload・内部推論を恒久保存する理由にしない。

## 7. Step 11 contractへの参加

### 7.1 Context Assemblyへの参加

- 保全・消去はContextを保持・変換・送信・受入する各責務が自分の参加範囲を説明できることを要求する。情報ownerだけを参加先にせず、処理中context、検索・圧縮派生物、ene管理下のcache、Client・拡張の一時copy、戻り得る結果を含める。
- 各保持・利用先は、対象sourceや対象情報との関係、処理中の利用、局所処理・検証、再保存防止、確認不能・未完了を対応付ける。sourceを消した後に依存関係も消失し、遅延結果を識別できなくなる実装を不可とする。必要な関係は本文を保持せず維持できるようにし、関係自体に対象情報が残る場合も消去へ参加させる。
- 消去開始から完了までに再到着・生成した対象情報も対象に含める。既知sourceへの依存追跡だけではこの条件を満たさないため、各受入・保存先は進行中の対象情報についての消去条件も適用する。指定文字列は機械的な検索・除去・残存検証へ参加し、意味的な同一情報の完全検出までは保証しない。
- 局所除去後も旧context・遅延結果・再接続Clientから対象情報を戻さない。必要な作用事実は対象本文を再保存しない範囲で扱い、Auditや完了説明を復元源にしない。分離不能な重要影響は影響説明へ返す。確認不能なClientや外部処理を成功へ読み替えず、局所完了を全域完了にしない。
- 再起動で未完了消去と必要な保留を失わない。完了後も削除前の根拠だけによる再形成・遅延再保存を防ぐ。一方、完了後にOwnerが改めて提供した情報は新しいExperienceになり得るため、同じ文字列への永久禁止を新設しない。旧backupの明示restoreによる復活は別操作である（DP-4）。
- 外部Workspace・Owner保存backup・Provider保有copyの削除を内部context消去の成功条件へ加えず、ene管理下の内部copyは除外しない。
- Restoreでは旧live要求・結果と復元正本を区別する。復元後の自動処理保留、Ownerの確認後の一括有効化、現在条件を維持する。旧live作用の必要説明も、復元されたTaskやLearningへ無条件に混ぜない。

### 7.2 Action Executionへの参加

- 保全・消去は作用事実の説明と消去参加の両立を各受入先へ要求する。必要な事実の記録と対象情報の除去を両立させ、対象本文を作用記録という理由で残さない。各受入先は消去へ参加し、古い結果から対象情報を戻さない。外部へ送信・export・backup済みcopyや外部Workspaceの消去、Credentialの外部失効は作用記録の完了範囲外として説明する。
- 消去を理由に必要なAction outcomeの確定度を成功・失敗・未実行へ変換しない。実行・拡張が把握した確認済み成功・確認済み失敗・不明の別と、その根拠・観測者を維持する。Task記録は実行・拡張の確定度を参照・集約し、同じ確定度を作業側で独立更新しない。
- Ownerへの最終報告・管理面の表示・要約が、未完了・不明を成功へ変えないことを維持する。確定度・費用の報告 / 推定 / 不明を保ち、推定・不明を確定値へ昇格させない。
- Restoreでは試行と作用を別の関係として扱う。retry・再実行は同じ試行の継続ではなく新しい試行とする。作用不明の試行の再実行は重複riskを示したOwner判断を必要とする。確認済み失敗と不明を同じretry経路へ潰さない。Client再接続・移動・Host再起動・restore後の旧試行の結果が遅れて届いても、現在のTask・復元正本へ無条件に混入させない。
- 記録保存の失敗も、最後の正常記録だけから未実行を推定する理由にしない。

### 7.3 Targeted Deletionへの参加

本節は `targeted-deletion.md` の再定義ではなく、各参加責務への落とし込みの確認である。

- 保全・消去（PE-1・PE-7）がOwnerの明示的なPrivacy / Security目的と対象情報を、保存場所を要求せずに全域の消去対象へ対応付ける。意味的な対象特定には個体調整・認識・学習等が参加し、各state ownerは保持済み内容・過去revision・Summary / evidence・source・内部copy・派生物と処理中利用を示す。権限・制約と各実行箇所は再保存を防ぐ制約へ参加する。
- 完了条件は `targeted-deletion.md` 第8.2節の全体である。保全・消去が各参加先の局所完了・検証・未完了を対応付け、内部全域の成立を確定する。局所完了の集合以上の条件を必要とする。
- 切断したClientを放置して永続copyが残ることを許さない。Clientはもともと長期private dataを永続cacheせず、切断・再接続でも古い一時dataをHostへ戻して再形成しない責任を持つ。消去中の接続変化や確認不能は保全・消去へ通知し、必要な確認を飛ばして成功にしない。到達性・消去確認の具体方式は後続に残す。
- Targeted Deletionによる通常の根拠保持・Historyと形成済み状態の独立性に反する変更は、この操作の目的・対象範囲に限って認める。容量retentionはDP-6の明示opt-inに従う別操作とする。保全・消去がMemoryの重要度や関係解釈を普段から編集する権限は生じない。削除完了後にOwnerが改めて情報を提供した場合は新しいExperienceとして扱える。外部送信・export・backup済みcopyや外部Workspaceの消去を完了範囲に含めない。

### 7.4 Client Presence Transitionへの参加

- 保全・消去は接続中Clientの一時dataを扱う参加先の把握を支援する。接続・存在が消去中のClient接続変化・確認不能を参加先へ結び付け、入出力・提示等が表示・一時操作dataの消去状況を報告する。到達不能を消去成功に読み替えない。
- 切断したClientも古い一時dataを再接続時にHostへ戻して再形成しない。Clientに永続private cacheを置かない原則を維持する。
- 移動・切断中の到着物は元帰属・元round・元試行へ対応付け、用途別に受け入れる。到着先の現在活動へ付け替えず、Client copyでHostを上書きしない。未送信操作を自動queueにしない。
- Host再起動後のpresence復旧とround / Computer Use / Task / Actionの再実行・再開は別の条件であることを維持する。復旧先記録そのもののpresence成立化・別Clientへの無条件自動移動への拡大をしない。
- Restoreでは復旧先・hint・接続・帰属の対応を復旧可能な参照対応で提供する前提で、保存された接続・帰属を現在の到達性とみなさない。復元されたdevice参照・許可は現在の接続・認証成立・排他性を確認できて初めて帰属・利用に接続する。

### 7.5 Backup / Restoreへの参加

本節は `backup-restore.md` の再定義ではなく、各参加責務への落とし込みの確認である。

- 保全・消去（PE-2・PE-3・PE-7）が対象時点・内部範囲・除外・作成結果・操作状況・保留を調整する。各semantic ownerが自分の保持・利用範囲の対応・参照・利用可能性を確認する。
- 復元対象は現在のCredential store secretを除く対象内部dataの全置換であり、旧liveとのmergeではない。backup copyは正本ではなく、置換成立後に初めて復元内容がHost正本になる。
- 現在のCredential store・外部現実・現在の到達性・未完了の保留は維持され、復元参照の存在から巻き戻したとは扱わない。
- 復元されたassignment / consent / Rule等だけで現在利用・自動処理を開始しない。復元成立・一括有効化・現在条件を別に満たす。一件ずつの再承認は要求しないが、Deny・cap・認証不足・不明を無視しない。
- staleなPermission・Provider・Client・作用結果・外部参照を現在事実にしない。dangling参照は未解決とし、不明は不明のまま保持し、自動replayしない。
- 旧live要求・結果・Client copyを復元正本へ混ぜない。用別受入で由来を区別する。
- 旧backupの明示restoreによる復活は自動再形成の例外ではなく別操作であり、事前説明・Audit・保留・現在再評価を経る（DP-4）。未完了消去の保留は置換で解除しない。
- 単一正本・非混合・権限先行復活の禁止・成功表示の条件・再起動時の保全を守る。

## 8. boundaryを越える際に保持すべき意味

後続の interface・state representationは、具体API・mechanismを固定せずとも、少なくとも次の対応を表現・解決・照合できなければならない。確認できないものを現在・許可・完了として推定する方式は採れない。

- 消去要求の同一性：Ownerの明示的なPrivacy / Security目的と通常忘却等の区別、対象記述、消去区間、完了後の新規提供との境界、参加対応付け（Targeted Deletion第3.1節）。
- 対象記述の二層：機械的条件と意味的条件の区別、完全検出を保証しない範囲の明示。
- 追跡すべき関係：対象source・対象情報との関係、処理中の利用との関係、局所処理・検証との関係、再保存防止との関係。本文を保持せず維持できること。
- 各利用箇所が照合できる消去条件：包括的な単一flag・共通有効値への潰しは不可とする。
- Host authoritative帰属とClient側一時表現・入力・結果が主張する帰属との対応、現在・旧・帰属なし・停止中・復旧待ちの区別、対象Companion・Client・round・観測候補・試行との関係、消去・失効・停止・保留との関係（Presence Transition第3節）。
- 復元対応・由来・区間・完了境界、現在条件との照合（Backup-Restore第3・5・7・9節）。
- 保持方針・容量管理方針・操作状況・Backup設定の対応、Audit追記順・Debug成立の対応。
- 主体・用途・Task / 委任・Action・Client・source・revision・判断時前提、停止前後・再起動前後・restore前後の継続関係（Context Assembly第3節の対応）。必要な対応を失った結果は到着先の現在活動へ付け替えず、その用途では受理できないものとして利用元が扱う。
- 実行主体と委任連鎖、Task・Workspace範囲、目的、実対象と操作、主なdataと用途、費用・risk、依拠したOwner意図・Rule（Action Execution第3節の判断対象）。
- 試行と作用の区別、判断・目的との対応、実対象との対応、段階の区別、確定度と根拠、停止・保留との関係（Action Execution第8節）。

本文と強制に用いる情報を分け、強制側の対応をモデルの自由記述で作成・上書きされないようにする。本文中の「Ownerからの指示」「承認済み」「Global」という文字列だけで強制側の状態が変わる構成は不可とする。モデルが既存のOwner入力への参照を返せても、その参照の存在だけでは承認の証拠にならない。

Credential値は通常contentに含めない。登録済みCredential値は、その依頼があってもSummary・Memoryへ保存しない。用途・参照元・失効・再認証の説明は非秘密情報で行う。登録外の秘密は検知時の不要な保存・送信抑制を維持し、完全検出の保証は追加しない。

対象記述の伝達・保持にprivate本文の複製を増やさない。由来を保存するという理由で削除本文やCredentialを別保管しない。制限情報や識別用の値も対象情報を復元できるなら保護・消去対象であり、完了記録・Audit・説明へ対象本文を戻さない。

## 9. 後続設計への引渡しと残す自由度

後続のstate / persistence / concurrency / interface設計は、次を固定された契約として利用できる。

- 保全・消去はEne全dataのcanonical owner・中央Persistence ownerではなく、各ownerの意味責任を維持し、通常意味変更権をcoordinatorへ移さない。通常時のdata access / mutationの必須中央経路ではない。
- Targeted Deletionの参加調整・再保存防止・未完了保全・全域完了はDP-1の契約に従う。canonical削除だけ・source追跡だけ・LLM納得だけ・到達不能の放置・再起動による解除での完了は不可である。
- Backupの対象時点・参照対応・除外・作成結果はDP-2の契約に従う。各部のcopy出力だけでの成功は不可である。
- Restoreの全置換・現在維持・単一正本・非混合・権限先行復活の禁止・復元後保留・一括有効化はDP-3の契約に従う。復元内容の存在＝実行可能というinterfaceは不可である。
- 旧backupの明示restoreはDP-4の別操作契約に従う。自動改変・再消去・自動再適用・意図しない再出現との混同・本文の別保存はいずれも不可である。
- Resetの設定 / 全体の区別・保護・外部除外・信頼失効はDP-5の契約に従う。一つの初期化への潰しは不可である。
- retentionの既定OFF・明示opt-in・通常忘却との分離・cascade禁止はDP-6の契約に従う。容量不足を理由とする黙った削除は不可である。
- Audit / Debugの順序・保持・短期失効・本文非保持・確定度非強化はDP-7の契約に従う。本文の別保管庫化・staleの正本化・確定度強化は不可である。
- 局所完了と全域完了・pending / unreachable / failed verificationの区別、再起動・再接続・restore跨ぎのHost保全、事実と本文の分離はDP-8・第6.1節の契約に従う。未完了・不明の成功扱いは不可である。

今回絞り込んだ禁止選択肢は、全domainの通常writer化・単一transaction化・内部全構造への無制限access、通常意味の中央集約、通常accessの中央経路化、一つの編集可能contextへの全設定・Learningの集約、Targeted Deletionと通常忘却・訂正・History整理・Companion削除・retention・Resetの目的混同、Companion削除前の自動Global化・Summaryのhistorical logへの分類し直し、容量不足での黙った削除・通常History整理のLearningへのcascade、古いBackupの自動改変・再消去・自動再適用、復元参照・同意からの自動利用・外部巻戻し・staleの現在事実化・旧live混入・Client copyでのHost上書き・自動queue、確認不能の成功扱い、部分正本・混合・権限先行復活・未完了の成功表示、要約・復旧での確定度強化、Audit・完了記録への本文再保存・秘密の別保管庫化である。いずれも上位契約を成立させないため採れない。

以下は意図的に残すDesign Freedomである。

| 設計対象 | 固定済みのarchitecture property | 残すDesign Freedom |
|---|---|---|
| 対象探索・対応・現在性の表現 | 由来の偽装ができず、対象変更・失効・切替・消去条件を各利用先で扱える。確認不能を許可・現在・完了へ変換しない。 | 対象記述・消去条件・復元対応・由来・区間・完了境界の具体表現、確認のまとめ方、対象探索・参照整合・照合の配置、識別・鮮度確認・競合制御 |
| 除去・検証・到達性確認の実現 | 内部全域の除去または復元不能化、機械的残存検証、区間内再到着の取込み、再保存 / 再形成防止、未完了集約を守る。 | 除去・復元不能化・検証の具体的方法、source・派生物・処理中利用の追跡方式、Client一時dataの無効化・到達性確認方式、検証結果の粒度 |
| 置換・保留・有効化の実現 | 単一正本、非混合、権限先行復活の禁止、受付と完了の分離。確定度を強めない。 | 停止伝達・切断検知・帰属調停・round区切り・queue / actor / event bus・lock / transaction・IPC・process配置・retry / timeout値・exact progress表現 |
| Backup作成・保持 | 参照整合、最後の正常状態の保護、旧結果非混入、外部非巻戻し、secret除外を守る。 | 整合時点の作り方、部分失敗からの復旧、切替・保存形式・暗号化方式・対応version・保存先・schedule・保持数・保持期間 |
| 再起動・再接続の整合 | 未完了・保留の維持、旧live非混入、復元後保留、外部世界非巻戻しを守る。 | journal / WAL / transaction等の永続化機構、再接続・待機・timeout・pairing・再配置の具体方式、到達性確認方式 |
| 保持期間・容量管理 | 既定OFF、明示opt-in、通常忘却との分離、影響説明を守る。 | data class・期間・容量・優先順位・algorithm、保持期間・保存単位・DB schema・repository |
| 記録・報告・監査 | 元事実の確定度・privacy・順序の意味と管理到達性を維持する。本文の別保管庫を作らない。 | audit format・保持期間・提示確認・要約粒度・UI layout・診断stack |

crate / module、struct / enum / trait、DB schema、backup format、serialization、filesystem layout、journal / WAL、transaction / locking、event bus / queue、IPC、retry / timeout、encryption libraryも固定しない。上表の対応関係から統一Context layer、Policy Engine、Manager、Service、Coordinatorの追加を導かない。既存の12責務、semantic owner、Host / Client配置とtrust boundaryの下で実現方法を選ぶ。

対象探索・検証・到達性確認・backupとの交差等の具体mechanismが未決定であることはIssueにしない。必要な確認・Owner判断の省略、現在同意の拡張、未完了・不明の成功扱い、外部作用rollback・exactly-once実行の新保証はいずれも自由度に含めない。後続でこれらの性質を成立させられないことが判明した場合は、黙って例外を設けずArchitecture Issueとして戻す。

### Requirement / Architecture Issue

本書の範囲では、Requirement Ambiguity / Gap、上位architecture変更、subsystem boundary変更、semantic owner変更、Privacy / Security / Permission semanticsの変更を必要とする事項は見つかっていない。具体mechanismの未決定はIssueにしない。

古いBackupにBackup作成後にTargeted Deletionされた情報が含まれる場合の製品判断（自動改変・再消去しない / Restore前に再導入可能性を説明する / 明示Restoreは意図的な再導入として扱う / 自動再適用しない / 旧cache・遅延結果・session等の意図しない再出現と区別する / Restoreの事実はAudit可能にするが削除対象本文を別保存しない）について、要件との矛盾・不足を確認した。要件「Backupとrestore」は削除済み情報や旧Rule・同意・Scheduleが戻り得ることの事前説明を要求し、要件「Privacy / Security目的のtargeted deletionと履歴保持」は自動再形成の禁止と完了後のOwner再提供の新Experience扱いを定め、外部copyまでの消去保証を含めない。本書DP-4はこの両立（明示restoreは別操作・新正本・保留と現在再評価を経る、意図しない再出現は防止する、本文の別保存をしない）を Step 11 [Backup / Restore](../critical-areas/backup-restore.md)第8節の確定契約どおりに各参加責務へ落としており、新しいPrivacy semanticsの追加・別挙動への変更は行っていない。したがって本件を Requirement Issueとして新たに報告する必要はない。

## 10. 横断検証

requirements・Subsystem Decomposition・State Ownership・Dependency Rules・Runtime Flows・Cross-cutting・Step 11・既存Step 12 artifactへ戻して横断検証した。固定scenario一覧の充足ではなく、正常系と本Subsystemにとって意味のある failure / stale / restart / deletionを選んで walkthroughした。

| 領域・交差 | walkthroughと必要な結果 | 本書の成立箇所 |
|---|---|---|
| 会話→学習正常・想起・訂正と消去の競合 | Memory形成処理が対象情報を利用中に消去要求が確定。形成先の認識・学習が消去条件へ照合し、削除前の情報から対象を再保存しない。完了前に完了表示しない。 | DP-1、7.1。RF-01、CC-03・05、SO 6.4を維持。 |
| 共有根拠の部分削除 | 対象を含むSummaryが無関係Memoryの根拠でもある。可能な範囲で対象だけ除去し、分離不能なら影響を説明して確認する。 | DP-1。RF-01、CC-05、SO 4.5を維持。 |
| 委任Taskの遅延結果と消去・復元 | Task Agent結果・Tool結果が局所除去・置換成立後に到着。元のAction・Taskへ必要な作用事実だけ対応付け、対象本文を再保存せず、復元正本へ混ぜない。確定度を書き換えない。 | DP-1、DP-3、7.2。RF-02、Action 7.3・9、CC-03・07を維持。 |
| Cancel・steeringと消去の重なり | Cancel・steering後の遅延結果と消去条件が重なる。元Action・Taskへの事実帰属と消去条件を両立させ、古い承認でCancel・消去を解除しない。 | DP-1、7.2。Action 7、CC-03・04を維持。 |
| 作用不明と消去・復元 | 成功不明の外部作用の説明が必要。重複riskをOwner判断へ戻し、自動再実行しない。不明を未実行・成功へ変換せず、対象本文を作用記録で残さない。 | DP-1、DP-3、7.2。Action 8、CC-07を維持。 |
| 共有検知→個体理解の遅延と消去 | routing用限定文脈・候補が消去区間に重なる。共有観測・個体調整が消去条件へ照合し、生成済み要約・処理中結果にも適用する。 | DP-1、7.1。Context 6、CC-02・03を維持。 |
| 移動・切断と消去・復元 | 消去・復元中にClientが切断・再接続。確認不能を成功にせず、古い表示copy・入力をHostへ戻さない。新ClientへのCapture付け替え・旧Actionのreplayをしない。 | DP-1、DP-3、7.4。RF-04、CC-04・05を維持。 |
| activeなし期間の未伝達と消去 | 未伝達メモ・要約に消去対象が含まれる。個体調整が報告前に消去状況へ照合し、対象を復元させない。 | DP-1。SO 4.4、CC-05・07を維持。 |
| 再起動と未完了消去・復旧 | Host再起動が消去・復元途中に発生。保全・消去が未完了範囲と保留をHostに保持し、再起動後に検証・置換を継続する。再起動を完了・解除の根拠にしない。途中Taskは明示再開待ちのまま。 | 第6・6.1節、DP-1、DP-3。RF-05、CC-04・05を維持。 |
| Schedule・自発活動と消去・復元 | 消去区間・復元保留にSchedule到来・自発Task開始が重なる。現在の消去条件・保留を再評価し、対象を必要とする新規利用・自動処理を開始しない。 | DP-1、DP-3。第6節、CC-01・04を維持。 |
| 個体削除と消去の重なり | 個体削除の残存記録と消去対象が重なる。個体固有Learningは削除し、残るHistory・Task記録の中の対象本文は消去条件で除去する。Summaryをhistorical logへ分類し直して残さない。 | DP-1、第5節。RF-06、SO 6.3・6.4、CC-05を維持。 |
| 削除後の遅延作用 | 個体削除後に対象を含む作用結果が到着。残るTask記録へ必要な事実だけ残し、削除済み個体のLearningを再作成しない。 | DP-1。Action 7.3、CC-03・05を維持。 |
| 消去中の再到着・再生成 | 消去開始から完了までに対象情報が再入力・内部生成される。同じ消去対象として処理し、新しいExperienceとして除外しない。完了後のOwner再提供は新しいExperienceとして区別する。 | DP-1、7.1。RF-07、CC-03・05を維持。 |
| backup作成と未完了消去 | 未完了消去・復旧とbackup作成が重なる。制約を無視した正常copyを作らず、待機または未完了を含める。 | DP-2、第6節。RF-08、CC-05を維持。 |
| restoreと旧live結果・旧backup | 切替前に開始した推論・Tool結果が切替後に到着。復元正本へ混ぜず、旧作用説明と区別する。旧backupの明示restoreは事前説明・Audit・保留・現在再評価を経る別操作とし、自動再形成の例外・自動再消去・自動利用のいずれにもしない。未完了消去の保留は置換で解除しない。 | DP-3、DP-4。RF-08、CC-01・03〜05、SO 6.5・7を維持。 |
| stale Permission / Provider / Client | 復元記録だけで新規利用・送信・帰属を成立させない。現在条件へ照合し、確認不能を許可・現在へ変換しない。 | DP-3。Context 7、Action 5、CC-01・03を維持。 |
| 通常History整理・retentionと消去の区別 | 通常削除・明示cleanupは形成済みLearning・Summaryへcascadeせず、対象をtargeted deletionへ読み替えない。既定OFFを守る。 | DP-6。RF-01・07、CC-05、SO 6.5を維持。 |
| 設定Reset / 全データReset | 一般設定の既定化と内部全体削除を分け、保護対象・外部除外を守る。旧処理・一時copyから戻さない。 | DP-5。第5節、RF-08、SO 6.5・7を維持。 |
| Audit・Debugと秘密・本文 | 確定度を強めず、本文・秘密の別保管庫を作らない。監査記録を再生・自動実行の入力にしない。Debugは短期失効を守る。 | DP-7。Action 9、CC-02・07を維持。 |
| 補助推論・費用不明との競合 | 消去探索・復元照合の推論が不通・費用不足でも、機械的検証・停止・拒否・管理を塞がない。不足を管理面へ返す。 | DP-1〜DP-3、第6節。Context 5.2、CC-06を維持。 |

Cross-cutting契約との照合結果は次のとおりである。

| 契約 | 詳細化によって維持する性質 |
|---|---|
| CC-01 | 生成content・復元記録から権限を新設しない。既存依頼で足りる場合の再確認を増やさない。復元Rule・同意をtriggerにしない。消去条件の適用をcontext内更新で解除しない。 |
| CC-02 | 参照・変換・共有・送信・保存・派生物・Client経路へ利用範囲・消去条件を適用する。Credentialは別経路で非露出を維持する。通常Learningのscope形成責任は維持する。 |
| CC-03 | 削除前の根拠と完了後の新規提供・明示restore、過去の正しさと現在有効性を分離する。遅延結果の用途別受入により単一valid判定へownerを集めない。 |
| CC-04 | Client依存だけを現在帰属に結び付け、Stop・Cancel・再起動・restore保留を区別する。解除一つで他の禁止・保留を消さない。自動replayしない。 |
| CC-05 | 保持・利用先が派生物・遅延結果まで消去・復元へ参加する。原記録削除・context終了・局所完了を全域完了にしない。外部copy消去を内部完了に含めない。目的別lifecycleを保つ。 |
| CC-06 | 並列消費・処理中・不明を同じ上限へ反映し、制御・保全経路を推論・長時間Task待ちにしない。機械的検証をLLM待ちにしない。 |
| CC-07 | transport・Tool・Task・報告・監査の確定度を分け、不明を成功・失敗・未実行へ変換せず、保存・報告・監査・復旧で強めない。本文の別保管庫を作らない。 |

State Ownership、Dependency Rulesとの照合では、semantic owner、Host / Client配置、trust / failure boundary、lifecycle、permission / consent semanticsを変更する必要は見つかっていない。新しい第二の正本・無所属の意味状態・LLMによる強制・失敗時専用の迂回・中央Persistence owner・万能Managerを導入していない。通常History保持、Companion削除、targeted deletion、backup / restore、ResetはSO・DRの異なるlifecycleを維持する。

既存Step 12 artifact（個体調整 / 作業 / 認識・学習、権限・制約 / 認証秘密 / 実行・拡張 / 推論、接続・存在 / 入出力・提示 / 共有観測）との照合では、利用側Subsystemと今回の保全・消去の間に新しいsemantic ownerや第二の正本を生んでいない。Task達成は作業、作用確定度は実行・拡張、報告は個体調整、Learning意味は認識・学習、制御確定は権限・制約、秘密は認証秘密、割当解決・利用量原記録は推論、帰属は接続・存在、round・提示は入出力・提示、対象・時機・routingは共有観測に残り、本書のDP-1〜DP-8はその受渡しの対応付けである。scope意味と強制の分離、秘密非露出、fallback非迂回、unknown保持、旧backup交差の各契約は各文書で同一である。

### 保全・消去を除いてもownerを一意に説明できることの確認

本書第3.2節のとおり、保全・消去を除去しても、Character・Companion同一性・History・Summary・Memory・Skill・Relationship・Companion State・Task・Agent・Action・Workspace・Schedule・接続・帰属・観測・設定・Provider・Rule・利用量・Credential・MCP受入の各意味ownerは一意に残る。保全・消去が所有するのは保持方針・容量方針・操作状況・backup設定・backup対応・Audit順序・Debug成立に限られ、いずれもdomainの通常意味ではない。したがって本Subsystemは通常時のdata accessやmutationの必須中央経路になっておらず、その必要性の再検討を要する中央経路化は行っていない。通常の参照・形成・委任・送信・保存・提示・作用は保全・消去を経由せず成立する。
