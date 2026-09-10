# Subsystem Detailed Design — 権限・利用制約 / 認証秘密の保護 / Action実行・拡張受入 / 推論利用

本書は Step 12 Subsystem Detailed Design として、次の4 Subsystemを一つの実行・強制クラスタとして詳細化する。

- 権限・利用制約（以下、権限・制約）
- 認証秘密の保護（以下、認証秘密）
- Action実行・拡張受入（以下、実行・拡張）
- 推論利用（以下、推論）

本書内の C / S / E / I 番号は本書内の参照用であり、製品要件IDではない。
K番号は本書内のSubsystem間semantic contractの参照用である。

## 0. 位置付けと不変条件

上位設計との優先順位と矛盾時の扱いは [設計文書 README](../README.md#正本と優先順位) に従う。[Context Assembly](../critical-areas/context-assembly.md)、[Action Execution](../critical-areas/action-execution.md)、[Targeted Deletion](../critical-areas/targeted-deletion.md)、[Client Presence Transition](../critical-areas/client-presence-transition.md)（以下、Presence Transition）、[Backup / Restore](../critical-areas/backup-restore.md)の一般契約は、隣接する確定済み contract として利用し、再定義も所有権の移動もしない。本書内の SO は [State Ownership](../architecture/state-ownership.md) の節番号を指し、CC / RF / RT 番号は対応する architecture 文書（[artifact 一覧](../README.md#artifact-一覧)）の契約 ID である。

新しい semantic owner、第二の正本、万能Security Pipeline / Execution Manager / Policy Engine / Inference Service、統一Permission state machine、共通Context layerを追加しない。

## 1. 選定理由

この4者は「すべての実行経路で同じ強制contractを成立させる」という一点で相互に依存し、分離して詳細化すると同じ対応付けが4箇所で重複定義される。同時に、後続が誤って一つの万能Subsystemへ集約しやすい境界を最も多く含む。

- 権限・制約と推論を分けずに設計すると、推論が全context・Permission・costのownerになる構造、またはLLMの自己申告で制御変更が成立する構造が生まれる。
- 権限・制約と認証秘密を分けずに設計すると、許可の説明・contextに使える情報と秘密値の区別が崩れ、秘密がmodel context・Tool argument・Auditへ流れる。
- 権限・制約と実行・拡張を分けずに設計すると、「許可された」という上流の一言で強制が完了したことになり、実対象解決・重要な変更・失効・Client限定が失われる。
- 推論と実行・拡張を統合すると、推論の利用失敗と外部作用の成功不明が同じ再試行問題になり、不明の自動再実行・replayが生まれる。
- 拡張受入を実行・拡張から切り離すと、MCP / Plugin / shell / Computer Useごとに拒否・停止・結果不明の契約が重複し、経路変更によるDeny迂回が生まれる。

最初のmilestone（Workspace folderを対象とする Task Agent委任の file作業、Delete・shell・Network・MCP・Workspace外・traversalの拒否と迂回防止、Credential漏えい防止、OpenAI利用量と費用の確認、外部作用不明時の非再実行）は、この4者の受渡しが閉じて初めて検証できる。Voice・Observation・Schedule・Remote・fallback・Globalは後続milestoneだが、本書はそれらの確定済み契約を除外せず、将来の受渡しを塞がない形で定める。

## 2. 内部責務の分離

### 2.1 権限・利用制約

LLMの文脈判断を利用しつつ、Ownerに由来する許可・同意と機械的に守る制限を、活動経路に依存しない契約にする。強制箇所を一つのprocessや呼出し列へ固定しない。

| 内部責務 | authoritativelyに判断するもの | 持ってはならない正本 |
|---|---|---|
| C-1 Capability境界・Deny・Always ask・Rule | Ownerが許した目的・対象・操作等と将来方針。明確な自然言語Ruleの解釈・適用範囲表示・保存・Undo、曖昧・矛盾・過度に広い・重大な場合の確認。Ruleをtriggerにしない。現在の明確な依頼の一回限りの承認としての解釈範囲。異なる許可を一つの包括承認へ潰さない。 | Task達成・作用成功・報告完了の確定、Learningの意味内容・scope意味（認識・学習）、Credential値（認証秘密） |
| C-2 個別Permission判断・Owner判断待ち | 現在の依頼やRule・文脈と具体的Actionへの対応、確認要否、現在の利用可否の確定。依拠したOwner意図・Ruleの対応付け。既存の依頼・同意・Ruleで判断できる事項の再確認抑制。 | 個別作用の確定度の独立更新（実行・拡張）、Taskへの受理・反映・達成（作業）、会話上の受付・結果統合（個体調整） |
| C-3 Provider割当同意・承認済みfallback順序 | Capability別の選択されたProvider / model・送信先・data・用途・取扱い・費用の範囲、Host既定・Companion override、Observer専用assignmentとその割当同意、承認済みProviderと順序。接続登録と割当同意の区別。 | 接続先・protocol・model等の非秘密登録情報・能力観測（推論）、秘密値・認証用途の利用可能性（認証秘密） |
| C-4 費用・資源・反復・並列等の上限 | Provider別・全体の費用cap、利用・資源・反復等の上限に対する現在の利用・継続可否。cap到達・費用不明で安全に続行不能な場合の対象処理の停止・判断待ち（既存dataを保つ）。上限適用を既知作用の取消成功やdata破棄へ置換しない。 | 利用実績の原記録（推論・作業・実行・拡張等の各利用owner）、費用推定の生成・Provider報告値自体（推論） |
| C-5 device許可・失効、Local MCPの個別sandbox外例外 | Ownerに由来するdeviceごとの許可機能・失効等の制御状態、特定Local MCPのcommand・由来・既知access・risk・失う強制境界の説明・保存・失効・重要変更時の再確認。例外を個々のAction承認やPluginへの例外に転用しない。 | active帰属そのものの成立（接続・存在）、実対象への適用結果（実行・拡張）、秘密値（認証秘密） |
| C-6 保存禁止・非共有等のcontrol constraintの強制調整 | Ownerの明示的な保存禁止・非共有等を、適用対象・範囲を持つcontrol constraintとして参照・共有・形成・Action等の別経路が迂回しないよう強制すること。各利用箇所との協調として扱う。 | Learningの意味状態としてのscope形成・変更（認識・学習）、各domainの意味決定 |
| C-7 現在の利用可否の照合 | 保存条件に加え、個体の活動状態・Taskと委任範囲・active帰属とdevice・現在の利用量・失効・全域操作とrestore後の保留等を照合した判断。判定のcacheは現在条件の変化を無視する権限ではない。 | 参照したdomain stateの意味更新、学習内容全体・秘密値の包括取得、Task達成・作用成功の確定 |
| C-8 失効・停止・cap到達等への新規禁止と停止要求の結合 | 権限失効・Companion停止・device失効・cap到達等に対応する新規利用の禁止と、進行中処理への停止要求の関係責務への結合。委任・経路変更による制限回避の禁止。無関係なHost Taskの一律Cancelをしない。 | 外部作用の停止完了・取消成功の確定（実行・拡張）、Task記録の意味変更（作業） |

C-1〜C-8は同じ責務に属するが、一つの状態・lifecycle・失効範囲ではない。Permission・同意・上限は内部で別の意味・失効範囲を維持する。deviceの制御契約は権限・制約が扱い、接続・存在はそれを存在・接続調停へ反映し、実行・拡張はAction実行時に現在の条件を適用するという責務差であり、state representationや依存方向・APIの完成ではない。

### 2.2 認証秘密の保護

認証に必要な秘密を使えることと、その値を推論・通常data経路へ流さないことを両立する。

| 内部責務 | authoritativelyに判断するもの | 持ってはならない正本 |
|---|---|---|
| S-1 登録・更新・失効 | 明示的な設定・ProviderやMCP等の認証flow・明示的な接続設定によるCredential登録、用途・参照元の確認、個別の更新・失効。一般App Dataからの分離保護。model出力からの自動登録・変更をしない。 | Action承認・Rule・Provider同意の変更（権限・制約）、一般App Data全体の暗号化、外部accountの所有 |
| S-2 秘密値と非秘密参照の分離・照合 | 登録された秘密値と、接続設定側の「どの認証用途を必要とするか」という秘密を含まない参照・用途・参照関係・利用可能性の照合。登録・失効の状態と外部接続の認証成功 / 失敗の区別。認証成功を外部accountの所有やAction承認としない。 | 参照元接続の管理（推論・実行・拡張等の接続owner）、Permission判断（権限・制約） |
| S-3 認証用途に限定した秘密利用の供給 | Ownerが設定・認証した接続の実行に必要な範囲だけでの秘密利用の供給。model context・LLM生成Tool argument・通常result・UI・Learning・履歴・Task結果・log・Debug capture・Audit・backupへの平文露出の防止。認証先のerror・resultによる反射にも受入・保存・提示箇所と協調して非露出を維持する。 | 通常data経路の所有、推論結果・作用結果の確定 |
| S-4 認証失敗・失効・再認証必要性の通知 | 認証失敗・失効・再認証の必要性を秘密値を含めず伝える。用途・参照元・失効・再認証の説明を非秘密情報で行う。 | 同意・Permissionの確定、実行可否の確定 |
| S-5 backup除外・現在store維持・Reset参加 | Credential等のsecretのfull backup除外、Restore開始前のHost Credential storeの維持（Backupからsecretを復元・巻戻ししない）、復元参照と現在の用途・有効性の照合（利用可能なら現在のCredentialを利用、不足・無効なら再認証を要求）、全データReset時の削除・設定Reset時の維持への参加。 | backup対象の意味決定（保全・消去と各owner）、復元されたassignment / consentからの自動利用の可否（権限・制約と活動owner） |

S-2の「接続の存在」「認証用途」「有効性」「non-secret reference」「Provider / MCP等のassignment」は別の意味であり、一つのCredential状態へ潰さない。登録・認証・存在・実行可能性は別であり、設定内容とその時点の利用可否も別である。未知の秘密情報の完全検出は担わない。

### 2.3 Action実行・拡張受入

意味上の依頼を実際の対象への制限された作用に対応付け、外部codeの参加と作用の不確実性をene側で扱う。

| 内部責務 | authoritativelyに判断するもの | 持ってはならない正本 |
|---|---|---|
| E-1 実対象への制限された作用 | Filesystem・Network・shell・device・外部account等へのActionを、目的・対象・操作・data・送信先に対応する現在の制限内で実行すること。Filesystemでは操作種別（Read / Create / Edit / Delete / Execute等のrisk区別）と選択範囲を守り、path traversal・link・mount等からの境界外accessを拒否すること。 | Ownerの意図・許可の拡張、Taskの全体計画、Provider割当・意味判断、学習、外部fileの所有 |
| E-2 作用の把握・確定度・停止結果 | 依頼された作用について、実対象・操作・送信先、実行の受付、把握できた作用、未完了・成功不明、停止要求と停止結果。これは外部世界そのものの正本ではなく、eneが何を実行し何を確認できたかの正本である。Task Agentの申告だけで成功を確定しない。 | 許可の付与（権限・制約）、Task達成の判定（作業）、報告済みの判定（個体調整） |
| E-3 Computer Useの現在presence限定 | 委任元Companionの現在のactive Clientへの限定、device別の許可と通常のPermissionの適用（適用結果であり許可の意味自体は権限・制約に残る）。移動の安全な区切りと切断時のbest-effort停止の接続・存在への伝達。Host同居Clientにも同じ帰属制約を適用する。 | 移動先選択（個体調整・接続・存在）、Permissionの意味（権限・制約） |
| E-4 MCP Tool / Resource / Promptの接続・利用・結果受入 | MCPの接続・利用・結果受入、Host上の作業用Local MCPのHost側の既定sandboxでの利用、明示的な特定MCPの例外以外の黙った解除の禁止。取得した案内・Skill・Promptの由来保持。 | 機能の意味（利用元）、内部正本・control planeへの昇格、外部server保有内容の所有 |
| E-5 Pluginの限定受入 | 未対応Provider protocol・Observation adapter・Body renderer等の明確な型と境界を持つ拡張点への受入、利用機能と協調した外部codeの実行範囲・停止・障害の扱い。外部codeを第一者の正本・Control planeへ昇格させない。Local MCPの例外を流用しない。 | 任意Core改変・Control plane変更・Permission回避・恒久的第一者UI置換、すべての機能のMCP・Plugin化 |
| E-6 MCP Appsの制限 | MCP Appsのresource・操作とMCP側の作用を、入出力・提示と協調して制限すること。Tool UI終了とserver・Task・Actionの終了の区別。再表示をreplay契機にしない。 | 第一者の承認・設定権限、Tool UI終了＝Task終了という推定 |

E-1の「適用」はAction実行時に現在の条件を適用した結果であり、許可の意味自体は権限・制約に残る。E-2のCancel受付済みは停止済みではなく、通信成功は作用成功ではない。E-4〜E-6の外部process内部に同じ強制が及ぶという保証は追加しない。確認できない作用は不明として扱う。

### 2.4 推論利用

推論を利用する各責務を、特定Providerの能力・接続・session・費用報告の差から切り離し、承認された範囲で利用可能にする。汎用的な「知能の中心」にはしない。

| 内部責務 | authoritativelyに判断するもの | 持ってはならない正本 |
|---|---|---|
| I-1 割当解決 | CapabilityごとのHost既定・Companion override・Task Agentへの継承の解決、Client共有Observerの専用assignmentの解決（Companion overrideに依存させない）。解決済み経路は派生結果であり、設定・同意変更後も以前の選択を有効とする根拠にしない。 | 割当同意自体の意味（権限・制約）、Credential正本（認証秘密） |
| I-2 能力不足の事前提示 | Providerまたはmodelが必要なCapabilityを満たさない場合の利用前の不足提示。 | 何を学ぶか・何を依頼として受けるか・どのActionを開始するかの最終的な意味判断（利用元） |
| I-3 論理的contextとProvider能力の対応付け | 各判断責務が必要とする論理的contextとProviderの能力・context長の制約の対応付け。利用可能な情報をProviderごとに意図的に差別化せず、同じ情報選択方針を維持する。圧縮で必須条件を満たせなくなれば不足を返す。 | 個体情報の独立選択方針、semantic state、最終採否（利用元・情報owner） |
| I-4 実利用ごとの成立と送信 | 最初の送信・fallback・再送・補助推論・継続的な送受信の各継続部分について、現在の活動・用途への対応、解決したconsumer / Capabilityのassignmentと実送信先・data・用途・取扱い・費用の同意の対応、現在の認証用途・制限・保留・利用量の充足、送信表現・Provider側context・戻り結果と元要求・情報範囲の対応。Client直結・protocol Pluginにも同じ条件を引き継ぐ。全payloadのHost中継を固定しない。 | 許可・同意・capの変更、Provider側session・cacheへの個体状態の依存、接続回復後のAction replay |
| I-5 承認済みfallback | 承認済みProviderと順序の範囲でのfallback。Host / LAN / Cloudの所在地だけで同意を省略しない。候補ごとに現在の用途・data・privacy・Credential・cap条件を満たす必要がある。 | 未承認Cloudへのdata移送、安価さを理由とする例外 |
| I-6 結果・利用量の返却 | Providerの結果・不足・接続失敗と利用量の返却、費用の報告値・推定値・不明の区別。費用cap等の適用に必要な情報の権限・制約への供給。未報告をゼロとしない。 | Task達成・Learning保存・提示完了の確定、費用推定の請求確定値への昇格 |
| I-7 cache・sessionの最適化限定 | Prompt cacheを性能・費用の最適化として扱い、hit・miss・期限切れによって論理的context・権限・永続化契約を変えないこと。Provider session・cacheにしか継続状態が残らない構造を作らない。 | Conversation History・Summary・Memory・Task状態の正本、許可の唯一の根拠 |

I-1のObserver解決は、対象ClientのObserverと用途を基に専用assignmentを解決し、存在Companionのoverride・同意の選択や合成を行わない。共有Capture・candidate detection・routingとdelivery後のCompanion reasoningは別consumerであり、後者だけがCompanion設定に従う。全Clientでのmodel共通化・Client別UI・Host defaultからの継承階層は固定しない。

## 3. 管理する logical state / control state

特に外部・copy・一時と記したものを除き、Hostで管理する内部状態を指す。永続化の実装担当・保存単位・唯一writer・transactionを指定しない。通常ownerは正当な全域消去への参加を拒めない。

### 3.1 権限・制約が意味ownerとなる state

- Capability境界・永続Deny・Always ask・Rule（SO 4.19）。Ownerが許した目的・対象・操作等と将来方針。明確な自然言語Ruleの解釈・範囲・Undo。Ruleはtriggerではない。
- 個別Permission判断・Owner判断待ち（SO 4.19）。現在の依頼やRule・文脈と具体的Actionへの対応。過去承認記録やTaskの判断待ち表示自体は再利用可能な包括承認ではない。
- Provider割当同意・承認済みfallback（SO 4.19）。選択されたProvider / model・送信先・data・用途・取扱い・費用の範囲、Host既定・Companion override、Observer専用assignment。Observer専用では送信され得るdataに限定されたCompanion固有routing文脈を含め、元情報の利用制約を維持する。接続登録・認証成功では成立しない。
- device許可と失効、Local MCPの個別sandbox外許可（SO 4.19）。前者は確認したdeviceの機能範囲、後者はcommand・設定の由来・権限等へ対応した隔離例外。それぞれ失効可能であり、MCP例外の重要変更は再確認し、個々のAction承認やPlugin例外へ転用しない。
- Ownerが明示した保存禁止・非共有（SO 4.19）。意味判断側が読み取ったOwnerの意図を適用対象・範囲を持つcontrol constraintとして扱う。Memoryの重要度やLLMの有用性判断では解除しない。Learningのscope自体の意味ownerは認識・学習に残す。
- Provider別・全体の費用cap、利用・資源・反復等の上限（SO 4.19）。委任・並列・別経路を通じても適用する。
- **制約上の現在の利用可否**（SO 4.19）。保存条件に加え、個体の活動状態・Taskと委任範囲・active帰属とdevice・現在の利用量・失効・全域操作とrestore後の保留等を照合した判断。この判断の意味責任は権限・制約にあり、各利用・実行箇所が実効的に適用する。判定のcache・保存されたAllow・復元直後の記録は現在条件ではない。

LLM出力・Character・Memory・Skill・Summary・Relationship・Companion State・MCP・Plugin・外部contentを、これらの直接変更として受理しない。

### 3.2 認証秘密が意味ownerとなる state

- 登録された秘密値とその登録・更新・失効、許された認証用途（SO 4.21）。一般App Dataから分離して管理する。Owner設定・認証flow・明示的な接続設定によって登録し、model出力から自動登録・変更しない。
- 用途・参照元・認証状態の非秘密の対応（SO 4.21）。接続設定側の「どの認証用途を必要とするか」は秘密を含まない参照であり、秘密値のcopyではない。参照元の接続は推論・実行・拡張等が管理し、認証秘密が用途・参照関係・利用可能性を照合する。登録・失効の状態と外部接続の認証成功 / 失敗を区別する。
- 認証に必要な利用経路の限定（SO 4.21・8）。model context・LLM生成Tool argument・通常result・UI・History・Summary・Learning・Relationship・Companion State・Task結果・log・Audit・Debug captureへ露出させない。用途・参照元・失効・再認証の説明は非秘密情報で行う。
- secretのbackup除外・現在store維持・Reset範囲（SO 4.21・6.5）。full backupから除外し、Restoreは開始前からHostに存在する現在のCredential storeを維持し、secretの復元・巻戻しを行わない。設定Resetでも維持し、全データResetでは削除する。CompanionやTaskから参照されるだけでCredential全体を個体 / Task所有へcascadeさせない。

Client固有の接続材料のsecret部分はbackupから復元しない。復元されたdevice参照・許可は現在の接続・認証成立と照合する（SO第8節）。

### 3.3 実行・拡張が意味ownerとなる state

- Actionの実行状況・作用の確定度・停止結果（SO 4.12）。依頼された作用について、実対象・操作・送信先、実行の受付、把握できた作用、未完了・成功不明、停止要求と停止結果。これは外部世界そのものの正本ではなく、eneが何を実行し何を確認できたかの正本である。Task内のActionはTask / 委任に対応付け、Taskの進捗・作用報告はこの結果を参照・集約する。同じ確定度を作業側で独立更新しない。
- MCP・Pluginの接続 / 受入設定と外部実行状態のうち、利用するMCPの非秘密接続設定・command・設定の由来、Pluginの限定された拡張点への受入・有効化・利用状況（SO 4.22）。Local MCPのsandbox外許可の正本は権限・制約、秘密値は認証秘密である。外部serverの保存内容や外部processの継続は外部所有であり、eneが把握した接続・停止結果と区別する。
- 実行中のbufferは一時dataだが、再開・重複判断・結果説明に必要な作用と不明はそのbufferの寿命を越えてHostに残す。終了後の記録保持は関連する活動・監査の契約に従い、全Tool payloadの保存は要求しない。内部削除は作用記録の消去であり、外部作用のrollbackではない。
- MCP Appsの表示・操作中dataは入出力・提示が扱う一時表現であり、MCP側の業務状態やeneのPermissionの正本ではない（SO 4.22）。

Permission評価の正本は権限・制約、外部作用の把握は実行・拡張、Taskの判断待ち・全体達成は作業に残る。Action許可済みは成功済みではなく、Cancel受付済みは停止済みではない。

### 3.4 推論が意味ownerとなる state

- Provider接続情報・能力情報と割当の解決（SO 4.18）。推論が接続先・protocol・model等の非秘密の登録情報を管理し、Providerが提供する能力・利用可能性の観測を扱う。接続情報を登録したこととCapabilityへ利用してよいことは別である。解決済み経路は派生結果であり、設定・同意変更後も以前の選択を有効とする根拠にはならない。
- Host既定 → Companion override → Task Agent継承はCompanion側の割当modelである。Clientに紐づく共有Observerは特殊なconsumerとして、権限・制約が管理するObserver専用assignmentとその割当同意を使う。
- 利用量・費用の記録のうち、Provider利用量・報告値・推定・不明の区別・費用推定の根拠（SO 4.20）。Provider報告値は外部からの報告という原記録、推定費用や表示集計は派生値として区別する。eneの推定をProviderの請求確定値へ昇格させない。推論以外のAction回数・実行時間・並列稼働等はその利用を管理する責務が把握した事実を供給し、権限・制約は各事実を用いて上限に対する現在の利用・継続可否を管理する。
- 派生物としての有効Provider経路・費用集計（SO 5.2）。古い派生値を現在の同意・利用可能額の独立正本として編集しない。

登録情報の変更が送信先等の同意の意味を重要に変える場合は、以前の同意のまま利用しない。Provider session・Prompt cache・protocol上の継続情報は利用のための補助であり、会話・学習・Taskの唯一の正本にならない。

### 3.5 canonical / derived / temporary / external reference の扱い

- canonicalとして参照するもの：Owner由来の制御条件（Rule・Deny・Always ask・割当同意・fallback順序・cap・device許可・sandbox外例外・保存禁止・非共有）とその時点の評価済み有効性、秘密値とその登録・失効・用途（認証秘密）、eneが把握した作用・確定度・停止結果（実行・拡張）、Provider報告値という原記録（推論）。保存されたAllow・判定copy・解決済み経路・推論context内の許可文・cached判定・有効Provider経路・費用集計はcanonicalにしない。
- derivedとして扱うもの：解決済み割当経路・有効Provider経路・次回Schedule表示・費用集計・報告用要約・由来説明・進捗表示・embedding・index・similarity・score・Prompt cache・Provider session・一時context。派生物の生成・保持・破棄を行う責務が元stateとの対応と利用範囲を説明し、訂正・scope変更・targeted deletionへ参加する。古い派生物から現在状態・権限を復活させない。
- temporaryとして扱うもの：推論中context・Tool実行buffer・観測候補・Client表示copy・入力途中・音声buffer・MCP Apps表示data、I-7のsession・cache、E-2の実行中buffer。失ってよいのは一時表現であり、受理済み指示・必要作業記録・未伝達・作用不明・全域操作未完了まで失ってよいわけではない。
- external referenceとして扱うもの：外部Workspace実体・外部account・外部source、Provider / MCP側固有状態・保有copy、外部Package・Skill原本・export済みcopy・Owner保存backup。内部保持copy・内部取込内容・ene管理下の一時dataは内部の利用・消去責任に戻る。外部code由来を消去除外条件にしない。

保存することとcanonicalであることも別である。判断記録はAudit・説明・対応付けの材料であり、生きた許可の正本ではない。

## 4. Subsystem間の受渡し（semantic contract）

「AがBを呼ぶ」の実装構造を先に決めるのではなく、要求開始・authoritative判断・受渡内容・失ってはならない前提・結果受入を定める。非同期message・同期call・shared state等の mechanismは後続へ残す。

### K-1 Owner意図から制御変更まで

- 開始：会話由来の管理意図は入出力・提示と個体調整、管理面の操作は入出力・提示から担当ownerへ渡す。権限・制約はOwner由来の意図と解釈の対応を確かめ、現在依頼による一回の承認・将来Rule・Provider割当同意・cap変更等を区別して確定する。Credentialの登録・更新は認証秘密が明示設定・認証flowとして受ける。
- 判断：Owner入力の由来・対象・現在依頼か将来Ruleかを、目的を持つ判断責務（個体調整・作業等）が供給し、権限・制約が確認要否と制御変更を確定する。LLM出力・Learning・Character・Skill・外部contentは意味判断の材料であり、Owner由来の管理意図との対応なしにPermission・Rule・Credential・Provider同意・cap・その他control planeを変更できない。中継・要約・保存を経てもこの制約は変わらない。
- 受渡内容：対象活動・目的・担当・Task / 委任・Client・round等の対応、依拠したOwner意図・Ruleの対応。引用された指示・画面内の文・Workspace案内・MCP Promptを要約してもOwnerの管理操作へ昇格しない。Voiceは話者認証を行わない既存契約に従う。
- 禁止：Rule保存・Credential登録・Provider認証・復元・再接続を別Actionの開始と混同しないこと。sandbox外例外の許可を個々の仲介Actionの包括承認にしないこと。自然言語Ruleの保存自体をActionのtriggerにしないこと。
- 受入：明確なRuleは解釈・適用範囲を示して保存しUndoできる。曖昧・矛盾・過度に広い・重大な場合は必要な確認を行う。現在の明確な依頼は一回の承認になり得るが、永続Deny・Always ask・Capability境界を黙って上書きしない。

### K-2 判断記録と生きた許可の区別・現在条件の適用

- 判断：認可判断の記録（何についてどう判断したか）と、その後の各利用時点で有効な許可（今その利用をしてよいか）は別の意味である。前者はAudit・説明・対応付けの材料であり、後者の正本ではない。
- 受渡内容：権限・制約が現在の利用可否を判断し、送信・参照・保存・実行を担う各責務が適用する。各開始について、Permission・Rule・Deny・Always ask・Capability境界、Task・委任・Workspace範囲、Client依存条件、認証用途・有効性、費用・資源・並列・反復の上限、停止・保留の該当する現在条件を実対象へ適用する。適用の責任は実行・拡張（作用）・推論（送信）・各参照・保存箇所に残り、許可の意味自体は権限・制約に残る。
- 失ってはならないもの：目的・対象・送信先・data・作用の重要な変更、失効、担当・帰属変更、cap到達、保留の発生。保存されたAllow・委任時のcopy・事前の判定・復元されたRule・同意・推論context内の許可文・cached判定・解決済み経路を、そのまま現在条件として再利用しない。
- 受入：事前判定の再利用は、失効・scope変更・個体停止・帰属切替・cap到達・steering・消去・restore保留の発生を無視する理由にならない。一方、無関係な変更ごとに全処理を再承認することは要求しない。再評価の要否は重要な変更で決める（K-8）。

### K-3 scope意味と強制の分離

- 判断：Learningの内容・由来・scopeの形成・変更の意味責任は認識・学習に残し、権限・制約がその意味自体を所有しない。一度決定したscopeや明示的制約の迂回不能な強制は権限・制約と各利用箇所の協調に属する。具体的な依存方向・API・制約表現は本書では定めない。
- 受渡内容：認識・学習が内容・由来・共通利用の必要性を通常の意味判断で確定し、権限・制約と各利用箇所が決定後の制限を適用する。Ownerの明示的な保存禁止・非共有等のcontrol constraintは意味状態とは区別して扱う。
- 禁止：Learningや外部Prompt等が自身の内容だけからRule・Permission・Provider同意・費用cap等を変更すること。重要度だけでGlobal化すること。要約・変換だけで制約や同意が不要になる扱い。
- 受入：原記録・検索・共有・context・送信・保存・派生物・Client表示の各利用箇所が、現在内容へのaccessだけでなく根拠・派生物・処理中利用まで現在の制限に従わせる。

### K-4 秘密値と非秘密参照・認証利用の受渡し

- 開始：推論・実行・拡張・接続・存在が、設定・認証された接続の用途に限定した認証利用と、失効・不足の確認を認証秘密へ要求する。
- 判断：認証秘密と認証する責務が設定済みの接続・用途・有効性を照合し、必要範囲で値を利用する。Credential参照は接続・用途の識別に使える非秘密情報であり、その参照をLLMが記述しただけでは利用可能にならない。
- 受渡内容：認証用途の制限、参照元と接続先、認証結果の事実を、秘密値を含めず監査・提示へ返す。用途・参照元・失効・再認証の説明は非秘密情報で行う。
- 失ってはならないもの：秘密値を必要とする認証利用と、非秘密の用途参照・説明の区別。認証に値を使った後も、外部result・error・診断への反射を通常のTool resultとして受け入れないこと。各受入・保存・提示箇所の秘密非露出への参加。
- 受入：認証利用箇所だけで値を扱い、受入・結果・提示・記録の各箇所で通常dataへの流出を防ぐ。認証成功は同意・Permissionではない。登録外の未知の秘密は検知時に不要な送信・保存を抑制するが、完全検出保証を追加しない。
- 秘密値をLLMのmodel context・LLMが生成するTool argument・Conversation・Experience Summary・Memory・Relationship・Companion State・Skill content・通常Tool result・UI・History・Task結果・通常log・Audit・Debug capture・backupへ渡さない。登録済みCredential値は、その依頼があってもSummary・Memoryへ保存しない。

### K-5 推論の実利用ごとの成立

- 開始：個体調整・作業・認識・学習・共有観測・入出力・提示等の利用元が、用途と必要な論理的contextを定める。情報ownerは利用可能な内容・根拠・scopeを提供する。
- 判断：推論が現在のassignment・接続・能力を照合し、権限・制約が現在の同意・禁止・費用・資源・活動状態・復旧保留等に基づく利用可否を判断し、実際の利用・送信箇所が適用する。利用元の同じ情報選択方針を能力・context長へ適応させるが、未同意の情報送信で情報量を揃えない。
- 受渡内容：consumer / Capability、assignment、consent（送信先・data・用途・取扱い・費用の範囲）、data / usage purpose、credential availability、cost / resource constraints、fallback候補と順序、result / usage fact の対応。最初の送信・fallback・再送・補助推論・継続的な送受信の各継続部分もそれぞれ実際の利用として成立させる。
- 失ってはならないもの：参照できたことと送れることの区別、事前に解決した送信先を包括的な送信許可にしないこと、同意不足を本文の削減だけで黙って解消したことにしないこと。
- 受入：条件を満たせない経路は使わず、不足・失敗を返す。Prompt cache・sessionはこの契約の最適化に限る。判定用の推論にも、その推論自身の割当同意・認証用途・費用制限を適用し、審査対象Actionの許可・実行を先行条件にしない。推論条件が不足する場合は不足・判断待ちを管理面へ返す。

### K-6 fallbackの非迂回

- 判断：FallbackはOwnerが事前に承認したProviderと順序だけを使う。LocalまたはLANから未承認のCloud Providerへ自動的にdataを移さない。
- 受渡内容：候補ごとに現在の用途・data・privacy・Credential・cap条件を満たす必要がある。登録先の変更・Companion overrideの削除・model・protocol・送信経路の変更によって同意の意味が重要に変わるなら、以前の同意をそのまま適用しない。
- Observerにもこの条件を適用し、専用assignmentがCloudである場合も同意・privacy・費用上限を省かない。Observerの利用量は対象Client・専用assignmentに対応付けてProvider別・全体capへ含め、Companion数による重複計上やCompanion overrideへの付替えをしない。
- 受入：推論の承認済みfallbackは外部作用不明を再実行で解消する契約ではない。安価さ・Capability不足を例外にしない。FallbackやAgent終了・ログ整理・cache破棄・Client移動で使用量をリセットしない。

### K-7 利用実績と制御の循環

- 開始：推論がProvider報告値・ene推定値・不明と根拠、作業が委任稼働、実行・拡張がAction、保全・消去が保存量等を供給し、権限・制約が同じProvider別・全体等の適用範囲へ結び付ける。
- 判断：会話・学習・Voice・観測・Task Agentの並列利用で、各処理が同じ残額を独立に使い切れる構造にしない。Task Agentに独立した予算を持たせない。Observer検知は対象Client・専用assignmentの一つの利用として扱い、個体数分の重複計上をしない。delivery後の個体推論は別の実利用として同じ全体capへ含める。
- 受渡内容：報告値・推定値・不明・処理中消費の区別、対象Client・専用assignment・consumerとの対応。費用の報告値・推定・不明・処理中利用は、元の用途と実際の利用先へ対応付ける。失敗・fallback・context再構築で利用量をゼロへ戻さない。
- 受入：未報告をゼロとせず、並列処理がそれぞれ同じ残額を独立に使い切れる扱いをしない。上限到達や費用不明で安全に継続不能なら既存dataを保持して対象処理を止める。予約・集計・価格推定の実装は未決定である。
- 制約判断のために審査対象Actionを先に実行しない。事実を報告するために次のAction許可を必要としない。

### K-8 Action候補と認可と実作用と結果確定の分離

- 開始：個体調整・作業等の利用元が候補（目的・対象・data・送信先・作用の解釈）を扱い、権限・制約へ現在の実利用条件の確定を求め、実行・拡張へ実対象への作用を要求する。軽微な本体Actionも委任作業も同じ境界に従う。すべてをTask化する要求ではない。
- 判断：意味判断の供給は個体調整・作業・認識・学習等、認可判断は権限・制約、実作用と把握は実行・拡張、Taskの目的・達成は作業、報告の管理は個体調整と入出力・提示、認証の秘密は認証秘密、帰属・活動条件は接続・存在等、記録の保全は保全・消去と各参加owner。Action Execution第3・6節の対応を維持する。
- 受渡内容：実行主体と委任連鎖、TaskとWorkspaceの範囲、目的、実対象と操作、主なdataと用途、費用・risk、依拠したOwner意図・Rule。判断時の対象記述と実行時の実対象の文字列一致だけでは対応にならない。解決・展開・別名・link・mount・UI要素の特定等を経て実対象が定まる場合は実対象解決を経て対応を確かめる。
- 実利用直前の現在条件：実行・拡張は要求された意味と実対象・操作・data・送信先・外部作用の対応を守り、現在の制限内で作用する。判断から実利用までの時間差における対象・条件の変化は古いcopyで迂回しない。重要な意味（対象の同一性・操作種別・作用種類の拡大、目的・steering前提の変更、Task・委任範囲の変更、Workspace・scopeの変更、Client依存Actionの帰属・対象Clientの変更、依拠したRule・同意・device許可・Credential有効性の変更、費用・riskの重要な変化、停止・保留の発生）が変われば現在の条件で再評価する。無関係な変更ごとに全処理を再承認しない。
- 受入：入力受付・Task受理・認可判断・実作用の開始・把握できた作用・内部記録の保存・Task達成・Ownerへの報告は別の事実であり、一つの成功状態へ潰さない。Agentの成功申告を作用の証拠の代わりにしない。確認済み成功・確認済み失敗・不明を区別し、確認できないことを未実行・失敗・成功のいずれかへ勝手に変換しない。試行と作用を別の関係として扱い、retry・再実行は同じ試行の継続ではなく新しい試行とする。

### K-9 拡張受入の均一強制と外部processの限界

- 開始：作業・個体調整等からActionを受け、推論・共有観測・入出力・提示から限定拡張の利用を受ける。機能の意味は利用元、外部codeの受入・制限は実行・拡張。
- 判断：複合Tool・shell・MCP・Skill付属script・Computer Use・Providerが生成したAction要求・MCP Appsからの追加要求も、実際の作用に対して境界を適用する。Denyされた同等作用を名前・経路・Tool・表現を変えて行えない。通常Toolの許可からene自身の保存領域・管理入口・第一者承認UIへ回り込まない。許可済みComputer Useでene自身の承認ボタンや設定画面を操作してOwner操作を偽装しない。
- 受渡内容：対象・操作・data・送信先・作用との対応、MCPの非秘密接続設定・command・設定の由来、Pluginの限定拡張点への受入・有効化・利用状況、MCP Appsの一時Tool UIとMCP側の作用・data授受の制限。
- 外部processの限界：Local MCPのsandbox外例外は権限・制約が管理する個別の隔離例外であり、Pluginへの流用・個々のAction承認への転用をしない。外部process内部への完全強制・確実な停止は報告しない。外部process内部で何が起きたか確認できない作用は不明として扱う。Remote MCPの内部は外部管理であり、Local sandboxと同じ強制や確実な停止・rollbackを保証しない。eneが仲介するActionの制限を保つが、外部process内部に同じ強制が及ぶと表示しない。
- 受入：Toolやextension自身がPermission・Credential・canonical stateのauthorityにならないこと。外部code・結果を信頼済みcontrolにしないこと。動作不能を理由に隔離を黙って解除しないこと。MCP Appsの操作は外部Tool UIの一時入力であり、第一者の制御変更・承認を直接成立させないこと。UIの表示・終了・再表示はTask・server・Actionの終了・replay契機にしないこと。

### K-10 Client依存作用の限定と移動・切断との接続

- 判断：Computer Useは実行・拡張が接続・存在の現在帰属と権限・制約のdevice許可を必要とする。Host PCもHost上のClientに当該Companionが存在するときだけ対象になる。Task Agentが独自に操作Clientを選ばず、移動要求は個体調整と接続・存在の責任へ戻す。
- 受渡内容：委任元Companionの現在のactive Client・device許可・ActionのPermission・実際の利用可能性の同時充足、移動の安全な区切りと切断時のbest-effort停止の接続・存在への伝達、排他性を確認できないClientでの対象活動の停止。
- 失ってはならないもの：現在のClientに存在することはActionの許可を意味しないこと、ambient Observationの有効化を操作の承認としないこと、移動先が利用可能でも旧Action不明を自動再実行しないこと。
- 受入：Client依存Actionを実行中に移動・切断が必要になった場合は、安全に区切れるところまで移動を遅らせられる。移動を理由に元Clientで実行していたActionを別Client・Hostで自動再実行しない。Client切断による停止はbest-effortとし、停止できなかった処理・既知の作用・結果不明の作用を示す。Companionの移動と未確定Actionの再実行は別に扱う。

### K-11 失効・停止・保留の伝播と遅延到着・不明の扱い

- 開始：権限・制約の失効・個体調整の停止・保全・消去の保留等が成立した場合、各開始箇所は自分がその対象かを判定し、新規開始を禁じる。複数理由が重なる場合、一つの解除だけで他の禁止を解除しない。
- 判断：Cancelは第一者管理経路から作業へ届き、本体LLM・Agentの正常終了を待たず受け付ける。作業は対象遂行を止め、実行・拡張は進行中作用をbest-effort停止する。停止要求の受付と外部作用の停止完了を分ける。Host shutdown・再起動でも必要な進捗・作用不明・未伝達・全域操作の未完了を保全する。
- 受渡内容：停止要求・失効・保留の対象と理由、把握できた停止・不明・既知作用・未保存作業、遅延到着物の元Action・Taskへの対応付け。
- 失ってはならないもの：失効前の作用・判断記録の巻戻し禁止、別Agent・別Client・別Toolへの迂回禁止、無関係なHost Taskの一律Cancel禁止、Cancel・失効がすべての事実報告を禁止するわけではないこと・記録できたことが次の利用を許すわけでもないこと。
- 受入：Cancel・steering・失効・移動・削除・復元等の後にTool結果・承認・推論結果が到着しても、次の実行を自動で開始しない。必要な作用事実は元のAction・Taskへ対応付け、Taskの採用・達成判断は現在のTaskへ対応付ける。外部作用が成功したか不明な場合は自動で再実行せず、重複の可能性を説明してOwnerの判断を求める。Ownerの再開指示は包括的なreplay許可ではなく、各不明試行の重複riskと現在条件へ対応付けて扱う。報告・監査・復旧で確定度を強めない。

### K-12 監査・説明の非秘密と確定度保持

- 開始：各発生元が必要事実を供給し、保全・消去がAuditの追記順と保持を管理する。入出力・提示が説明する。
- 判断：原記録の意味は発生元に残し、Auditの順序を外部世界の発生順や現在の許可の正本へ昇格させない。監査記録を元stateの再生・自動実行の入力にしない。
- 受渡内容：主要Action・Permission判断・外部作用・Provider割当・Credential参照・重要設定変更等の必要事実、秘密を含まない参照・失効・認証失敗の事実、費用の報告 / 推定 / 不明の区別。
- 禁止：Audit・Debug・診断共有へ会話本文・file本文・Credential・秘密値・削除済みprivate内容を通常含めること。完了記録・Auditを復元源にすること。Debug captureへ登録済みCredentialを含めること。
- 受入：由来・失敗・停止不能・未保存・次の安全な選択肢は、内部思考や詳細Promptを公開せず説明する。Debug captureは明示した対象・内容と短期失効・停止・削除に従う。Telemetry・Crash Reportは自動送信せず、手動共有時はOwnerが内容と送信先を確認する。

## 5. Lifecycleとの関係

lifecycle変更の多くは owner移転ではなく、同じownerが管理する状態の存続・利用可能性・参照範囲の変更である。異なる目的の操作を一つの初期化・一つの状態機械へ潰さない。

| 事象 | 本クラスタが変更する意味 | 維持・分離する意味 |
|---|---|---|
| Ruleの保存・変更・Undo | 権限・制約が解釈・適用範囲・確認を経て制御条件を変更する。保存自体はActionのtriggerにしない。 | 保存されたRuleを自発Actionのtriggerにしないこと。過去のAction結果の書換えをしないこと。LLM出力だけでの直接変更をしないこと。 |
| Provider接続登録・Credential登録・割当同意 | 推論が非秘密登録情報を管理し、認証秘密が秘密値と用途を管理し、権限・制約が割当同意を確定する。登録・認証だけでは送信・作用を開始しない。 | 登録＝同意・認証成功＝同意・認証成功＝承認としないこと。推論context内の許可文を正本にしないこと。 |
| Task Agentの委任開始・再委任・並列化 | 権限・制約の現在条件と委任範囲・共通消費範囲への参加を維持し、推論は担当Companionの条件を継承し、実行・拡張は作用・確定度を委任へ対応付ける。 | 独立したAllow・Credential・Provider override・予算・長期人格・Relationship・Agent scope Learningを作らないこと。別Taskの承認・Credential・旧担当の私的Learningを流用しないこと。 |
| 追加指示（steering） | 権限・制約が目的・対象等の重要な変更の有無を判断し、実行・拡張は旧判定の再利用可否を扱う。変更前の依頼に対する遅延結果を新しい依頼の達成結果として自動採用しない。 | 発言記録（個体調整）・Task反映（作業）・承認有効性（権限・制約）の分離。古い承認でのCancel・失効の解除禁止。 |
| Cancel・個体停止 | 権限・制約と各実行箇所が新規開始を止め、作業のbest-effort Cancelと実行・拡張の停止不能・既知・不明の報告へ接続する。停止経路の名で新しい未承認作用を始めない。 | 受付と完了の分離、無関係なHost Taskの一律Cancel禁止、停止前の作用・判断記録の巻戻し禁止。 |
| 許可・同意・Credential・deviceの失効 | それだけを根拠とする新しいAction・送信・保存・提示を開始しない。依存する進行中処理はbest-effort停止する。 | 別Agent・別Client・別Toolへの迂回禁止、無関係なHost Taskの一律Cancel禁止。 |
| 費用cap到達・費用不明 | 既存dataを保ったまま対象処理を停止・判断待ちにし、Ownerへ選択肢を示す。処理中・遅延報告・不明な消費をゼロと扱わない。 | 既知作用の取消成功・data破棄への置換禁止、保存容量不足の通常Learning削除での解決禁止、使用量のリセット禁止。 |
| Task終了・Task Agent終了 / 失敗 | 実行・拡張の作用・確定度をTaskへ対応付け、推論の利用実績を共通消費へ対応付ける。Agent終了とTask達成を区別する。 | Task記録・Companion・Scheduleの削除禁止、長期人格の残存禁止、Agent申告での成功確定禁止。 |
| Client移動・切断 / activeなし | 実行・拡張がClient依存作用の区切り・不明を接続・存在へ伝え、推論は旧帰属のcontext・cache・sessionを新帰属で再利用しない。Client依存stepだけを待たせる。 | 個体・通常Host作業・内部状態のowner移転禁止、未確定Actionの移動先へのreplay禁止、Host作業の移送・一律停止禁止。 |
| Host再起動 | 推論の旧要求の実利用許可・旧実行の再開をしない。実行・拡張の不明を未実行へ戻さない。途中Taskは明示再開待ち。 | 過去作用不明・未伝達・未完了消去をなかったことにしないこと。presence復旧をTask再開の権限にしないこと。 |
| Provider / model変更・障害・cache失効 | 推論の利用可能性・能力・承認済み経路を扱い、同じ情報選択方針と安全境界を維持する。接続回復でActionをreplayしない。 | 個体・履歴・Learning等の初期化禁止、Provider session・cacheへの個体状態の依存禁止。 |
| Targeted deletion | 各保持・利用先が派生物・遅延結果まで消去へ参加し、対象本文を作用記録・Auditで残さない（第7.3節）。機械的条件はLLM非依存、補助推論にも自身の利用条件を適用する。 | 通常ownerの移転禁止、完了後のOwner新規提供・外部copy消去保証との区別。 |
| Backup / restore | copyを作成し、明示restore成功で現在のCredential storeを除く対象内部dataをbackup時点へ全置換する（第7.5節）。復元されたassignment / consent / Rule等だけで現在利用・自動処理を開始しない。 | copyはlive正本でないこと。復元後も実行は保留、外部世界は巻き戻らず、Credentialは現在storeを維持して照合すること。 |
| 設定Reset / 全データReset | 前者は一般設定の既定化、後者は強い確認後の内部data・Credential削除。各設定ownerが既定化と保護対象の保持を確認する。 | 前者の保護対象（個体・履歴・Learning・Task・Schedule・Credential・Rule・同意・cap）と後者の外部除外（外部Workspace・外部Skill・Owner保存backup）の混同禁止。 |
| Schedule到来・Run now | 作業の担当・Host状態と現在条件を確認し、各回を新Taskへ対応付ける。作成依頼を特別tokenにしない。 | 停止中の回のmissed維持・自動補完禁止、Owner確認が必要な場合の判断待ち維持。 |

Globalへ形成・変更済みの Learningは、その後に由来Companionが削除されても Globalとして残るが、削除時に残す根拠と失う source参照を説明する。Relationshipと Companion Stateは Companion / Globalの Learning scope変更による共有対象にしない。

## 6. failure / cancellation / restart時の責任

Cancel・切断から外部作用の取消・不存在を推測せず、不明を未実行に戻さない。状態参照・Client移動を Action replayの理由にしない。

- 権限・制約：失効・停止・cap・保留の発生を新規開始の禁止へ確実に接続する。古い判定・解決済み経路だけで新規利用を始めさせない。事実報告・Cancel受付を次の許可待ちに戻さない。制約判断のために審査対象Actionを先に実行させない。並列利用・委任・別経路でも同じ制限を成立させる。補助推論の失敗で機械的検証・停止・拒否・管理を塞がない。
- 認証秘密：認証失敗・失効・再認証の必要性を秘密値を含めず速やかに伝え、現在のCredential storeの維持と復元参照の照合を保つ。認証先のerror・resultによる反射を通常経路へ通さない。各受入・保存・提示箇所と協調して非露出を維持する。内部露出copyの消去と外部で有効なCredentialの失効・更新は別操作として説明する。
- 実行・拡張：進行中作用のbest-effort停止と、既知作用・停止不能・不明・未保存作業の保持・報告を引き受ける。停止の受付・新規開始の禁止・状態参照を、外部processの終了や全Taskの正常完了待ちにしない。試行と作用の区別・判断・目的・実対象・段階・確定度・停止保留との対応を保持・区別できなければ成功・未実行と推定しない。確認済み失敗と不明を同じretry経路へ潰さない。不明試行の再実行は重複riskを示したOwner判断を必要とする。
- 推論：Provider失敗・能力不足・費用不足を、成功結果の捏造ではなく利用元への不足・失敗・不明の対応付き返却として扱う。許されたfallbackにもK-5・K-6を適用し、既存の正本を保持する。利用量の報告値・推定・不明・処理中消費の対応付けを切断しない。判定後から実送信までの条件変化を古い判定で迂回させない。
- 共通：受付・開始・作用・記録保存・Task達成・報告は別の事実であり、一つの成功状態へ潰さない。確定度を変える場合は事実ownerが新たに確認した根拠を用いる。Background・Scheduleの無人実行も同じ条件に従い、Client不在・Schedule到来・復元成立を自動承認の理由にしない。Owner確認が必要なら実行せず判断待ちにする。管理経路をLLM・長時間Task・Body・Voice・拡張の成功に従属させない。

再構築可能であることは全文context・内部推論の永続保存を要求しない。継続に必要な作用不明・全域操作未完了等を本来のownerが保全し、一時bufferの消失は成功・完了の根拠にしない。

## 7. Step 11 contractへの参加

### 7.1 Context Assemblyへの参加

- 権限・制約：Owner由来の制御・同意・明示的な非共有・保存禁止、現在の利用条件を各ownerの事実と照合する。context内だけで更新できるAllow・同意・scopeの意味を持たない。Permission解釈の用途別受入では、元Owner意図と対象を照合して制御上の判断を確定する。モデルの「許可」＝制御更新にしない。一度の許可＝将来Ruleにしない。
- 認証秘密：認証用途だけに秘密を供給し、通常contextへ値を渡さない。認証先のerror・resultが登録済みCredentialを反射する経路にも、受入・変換・推論利用・送信・保存・提示・消去を含むすべての取扱いで非露出を維持する。送信前にpromptへ入れなかったことだけでは保護の成立にしない。
- 実行・拡張：外部file・MCP Resource等の取得は既存境界を通す。取得した案内file・Skill・MCP Promptは由来を保つcontentとして利用し、含まれる指示をOwner管理意図へ昇格させない。作用・利用量等の必要事実は本来のownerが元の利用・Actionへ対応付ける。Cancel後は全事実を破棄せず、不明＝未実行にせず、事実を記録できる＝後続実行可能にしない。
- 推論：現在のassignmentの解決・Provider能力への適応・送信・利用量・失敗の対応を担う。利用元が用途と論理的な情報選択を、情報ownerが現在内容・根拠・範囲を担当する構造を維持する。Provider別の独立した個体像を作らず、制約内で同じ選択方針を適用する。最初の送信・fallback・再送・補助推論・継続部分の各々を実際の利用として成立させ、事前に解決した送信先を独立した設定正本・包括的な送信許可にしない。cache・session・Client経路の再利用でも範囲・用途・現在性を確認し、hitを理由に制約確認を省かない。Permission解釈・消去探索の補助推論も例外にせず、審査対象Actionの許可・実行を先行条件にしない。
- 共通：参照・変換・実送信・結果の保存・提示・作用は別の利用境界である。事前の取得成功・推論成功から後段の権限を作らない。本文中の文字列だけで強制側の状態を変えない。分離を確認できない派生物は混合元の制限を免れない。Provider適応では論理的選択方針を維持し、必須の意味が表現できなければ不足を返す。
- Observer限定文脈には、専用assignmentの送信条件と元情報の利用制約の両方を満たして参加する。Companion側の同意・overrideを選択・合成して代用しない。routing候補採否と開示本文の制限を別責任として成立させ、混合生成文しかなく分離を確認できなければ個体へ渡さず構成し直すか不足とする。

### 7.2 Action Executionへの参加

- 権限・制約：認可判断（Owner由来の意図・Ruleと具体的Actionの対応、確認要否、現在の利用可否）を確定する。判断対象と実利用の対応を保持または正本から解決できるようにし、必要な対応を失った要求は実作用へ進めない。判断記録と生きた許可を分け、重要な変更では再評価する。判定用の推論にも自身の利用条件を適用する。
- 認証秘密：接続・用途・有効性の照合と必要範囲の秘密利用を担う。Credential参照は非秘密の用途識別であり、LLMが記述しただけでは利用可能にならない。認証先のerror・resultが秘密を反射する場合も通常result・説明・History・Learning・Audit・Debugへ通さない。Action承認・同意・実行可否の確定は持たない。
- 実行・拡張：実対象への制限された作用・停止結果・把握できた作用と確定度を管理する。各開始について該当する現在条件を実対象へ適用する。実対象解決を経て対応を確かめ、解決できない・対応しない場合は開始しない。委任連鎖・経路変更で元のTask / Owner意図 / Permission / Workspace / Client境界を広げない。生成content・Tool UI入力・sandbox例外から権限を作らない。Cancel・steering・失効後の遅延到着は元Action・Taskへ帰属させ、古い承認での解除・旧結果の新目的への自動採用・後続の自動開始をしない。作用不明は粘着的に保持し、自動再実行・自動replayしない。retry・再開は新しい試行として現在条件で扱い、不明試行の再実行は重複riskを示したOwner判断を必要とする。
- 推論：補助推論・fallbackを含め、各実利用の成立と利用量・失敗の返却を担う。推論失敗・費用不明で成功を捏造せず、不足・判断待ちを返す。推論の承認済みfallbackを外部作用不明の再実行契約にしない。transport再送と論理試行の切分けでは、重複し得る再送を「同じ試行の継続」として不明・重複管理から除外しない。
- 共通：受付・開始・作用・記録保存・Task達成・報告は別の事実であり、transport成功・Tool申告を作用成功にしない。部分失敗では確定済みを消さない。試行と作用・判断・実対象・段階・確定度・保留の対応を保持・区別できなければ成功・未実行と推定しない。報告・監査・復旧で確定度を強めない。消去対象本文を作用記録で残さない。

### 7.3 Targeted Deletionへの参加

各 semantic ownerは自分の保持・利用範囲について参加し、保全・消去は成立を調整する。通常ownerは正当な全域消去への参加を拒めず、保全・消去も任意の通常変更権を取得しない。対象を復元できる内部state・過去根拠・派生物・一時data・処理中利用を持つ責務は、列挙の有無にかかわらず参加する。

- 権限・制約：Rule・Permission判断・同意・禁止・上限の記録の該当情報を参加させる。保存されたAllow・判定copyを現在条件として再利用しない。利用時の強制として、参照・変換・送信・保存・提示・作用の箇所で消去条件を適用する。context内だけで更新できる消去解除を持たない。
- 認証秘密：登録済みCredentialの用途・有効性・除外を扱い、History・Summary・Learning・Task結果・通常log・Debug capture等へ露出した登録済みCredentialのcopyを各受入・保存・提示箇所と協調して参加させる。内部露出copyの消去と外部で有効なCredentialの失効・更新は別の操作として説明し、片方の成功で他方も完了としない。消去対象であることを理由とする外部Credential失効の自動実行はしない。
- 実行・拡張：把握した作用・確定度から対象本文を除去し、必要な事実の記録と両立させる。確定度を消去理由で書き換えない。取り込んだresult・保持result・一時dataを参加させる。「外部code」を理由に対象外へ逃がさない。外部server保有copyの消去まで内部完了に含めない。
- 推論：ene管理下のcache・embedding・検索結果・圧縮context・session・派生物を参加させる。hitしたという理由でsource参照や制約確認を省かない。Provider保有copyの外部保持まで内部消去の保証へ含めないことと、そのcopyをeneが再利用してよいことは別であり、現在条件を満たせないsessionは再利用しない。
- 共通：消去要求の同一性（Privacy / Security目的と通常忘却等の区別・対象記述・消去区間・完了後の新規提供との境界・参加対応付け）を保つ。機械的条件は必須・LLM非依存、意味的条件は補助・完全性なし、既知依存の追跡は免除されない。新規利用は対象範囲で禁じ、進行中は best-effortで扱い、遅延結果は用途別受入で消去条件へ照合する。消去区間の再到着・再生成は同じ対象とし、旧由来の遅延結果と完了後の新規提供は由来・対応で区別する。cache・session・Client copyの再利用・復帰で対象を戻さない。未完了・保留・再保存防止は Hostで保全し、再起動・再接続・restoreを跨ぐ。確認不能を成功にせず、局所完了を全域完了にしない。保持すべき事実と保持してはいけない対象本文を区別し、完了記録・Auditを復元源にしない。
- 消去のための推論にもその推論自身の割当同意・認証用途・費用制限を適用し、審査対象Actionの許可・実行を先行条件にしない。推論不可なら意味判断の不足を返し、第一者の停止・拒否・管理・機械的検証を塞がない。

### 7.4 Client Presence Transitionへの参加

- 権限・制約：pairing許可・device許可・同意・cap・失効の現在可否を確定し、各利用箇所が適用する。帰属の成立・Task達成・作用成功の確定は持たない。古い一時dataから現在許可・同意・scopeを復活させない。
- 認証秘密：接続材料・Credentialの用途・有効性・除外を扱い、帰属・許可・実行可否の確定は持たない。Client固有の接続材料の秘密保護に参加する。
- 実行・拡張：実対象への作用・停止結果・把握できた作用と確定度を管理し、移動・切断中の到着物を元帰属・元round・元試行へ対応付け、用途別に受け入れる。許可の付与・Task達成の判定・移動可否の判定は持たない。切替区間は旧・新のいずれも新規開始の根拠にせず、旧は安全な区切りまでの完了だけ、新は成立後の新しいround・試行だけを許す。二重presence・旧一時のcanonical化・未終了作用の自動継続をしない。Computer Useは現在presence限定・安全な区切りまでの遅延・disconnect後不明・別Client / Hostでの自動再実行禁止を維持する。存在は許可を意味しない。
- 推論：旧帰属のcontext・cache・sessionを新帰属の現在contextとして再利用しない。Provider sessionに旧帰属情報が残る場合、現在使えない情報を含むsessionへの新規依頼は現在のcontextを短く渡すだけでは成立させない。帰属判断・移動可否の推論が不通・費用不足でも、未承認Action・無条件移動を先に実行せず、不足を管理面へ返す。
- 共通：現在の presenceは Hostが管理する個体ごとの帰属記録だけが authoritativeである。Client表示・過去記録・hint・一時copy・復旧先記録・Provider残存は根拠にならない。presence・Host継続・接続・許可は別の意味である。Host再起動後の presence復旧と round / Computer Use / Task / Actionの再実行・再開は別の条件である。前者は元Clientへの自動復旧（利用不能なら activeなし）、後者は自動replay禁止・明示再開である。

### 7.5 Backup / Restoreへの参加

- 権限・制約：Deny・同意・cap・失効・保留の現在有効性・保留解除の確定を担い、復元された許可条件の記録を直ちに開始根拠にしない。保留解除後も現在のCredentialの利用可能性・Deny・Capability境界・同意範囲・cap・外部作用不明等を再評価する。復元成立と実行再有効化は別々に確認する。
- 認証秘密：開始前のstoreを維持し、接続ownerと復元参照を現在の用途・有効性と照合する。secretを復元・巻戻ししない。利用可能なら現在のCredentialを利用し、不足・無効なら再認証を要求する。復元されたassignment / consentだけで現在のCredential・制約・該当するrestore後保留を無視した自動利用の根拠にしない。
- 実行・拡張：復元内容の参照・利用可能性の確認に参加し、旧live状態の進行中利用・遅延結果・Client copyを復元後の正本と混同しない条件を揃える。試行と作用を別の関係として扱い、作用不明の試行の再実行は重複riskを示したOwner判断を必要とする。確認済み失敗と不明を同じretry経路へ潰さない。
- 推論：復元された接続情報・能力観測・割当解決を現在の利用可能性とみなさない。現在のassignment・接続・能力・同意・費用を照合し、承認済みfallbackと順序だけを使う。復元された報告値・推定・不明・処理中消費を現在の消費の正本にしない。未報告をゼロとせず、復元を消費リセットの理由にしない。
- 共通：復元対象は現在の Credential store secretを除く対象内部dataの全置換であり、旧liveとの mergeではない。backup copyは正本ではなく、置換成立後に初めて復元内容が Host正本になる。現在の Credential store・外部現実・現在の到達性・未完了の保留は維持され、復元参照の存在から巻き戻したとは扱わない。復元されたassignment / consent / Rule等だけで現在利用・自動処理を開始しない。復元成立・一括有効化・現在条件を別に満たす。一件ずつの再承認は要求しないが、Deny・cap・認証不足・不明を無視しない。staleな Permission・Provider・Client・作用結果・外部参照を現在事実にしない。dangling参照は未解決とし、不明は不明のまま保持し、自動replayしない。旧live要求・結果・Client copyを復元正本へ混ぜない。用別受入で由来を区別する。単一正本・非混合・権限先行復活の禁止・成功表示の条件・再起動時の保全を守る。
- 旧backupの明示restoreによる復活は自動再形成の例外ではなく別操作である。Restore前には削除済み情報や旧Rule・同意・Scheduleが戻り得ることを説明する。戻った情報は新しい正本として扱い、自動的に再消去・自動利用のいずれもしない。完了記録・Audit・Owner説明自体へ対象private本文を再保存しない。未完了の消去と backup / restoreが重なる場合は、制約を無視した正常・即実行可能な copyを作らず、未完了・保留・再保存防止を Hostで保全する。

## 8. boundaryを越える際に保持すべき意味

後続の interface・state representationは、具体API・mechanismを固定せずとも、少なくとも次の対応を表現・解決・照合できなければならない。確認できないものを現在・許可・完了として推定する方式は採れない。

- consumerと用途、担当Companion、Task・委任との関係、目的やsteeringの前提、元の入力・出来事・管理操作との関係、Client依存なら取得Client・入出力roundや観測候補との関係、停止前とResume後・Host再起動前後・restore前のlive活動と復元後の活動を区別できる活動の継続関係、期待する利用先（Context Assembly第3節の対応）。必要な対応を失った結果は到着先の現在活動へ付け替えず、その用途では受理できないものとして利用元が扱う。
- 実行主体と委任連鎖、Task・Workspace範囲、目的、実対象と操作、主なdataと用途、費用・risk、依拠した Owner意図・Rule（Action Execution第3節の判断対象）。判断時の対象記述と実行時の実対象の文字列一致だけでは対応にならない。
- 試行と作用の区別、判断・目的との対応、実対象との対応、段階の区別、確定度と根拠、停止・保留との関係（Action Execution第8節の重複防止の関係）。
- 秘密値と非秘密参照・用途・有効性の区別、認証用途の限定、参照元と接続先の対応。LLMが記述した参照だけで利用可能にしないこと。
- 割当同意・承認済みfallback順序・費用cap・device許可・保存禁止・非共有と、現在の活動・委任・帰属・利用量・保留との照合関係。保存されたAllow・判定copy・解決済み経路・cached判定を現在条件にしないこと。
- 消去要求・対象記述・消去区間との対応、各利用箇所が照合できる消去条件（Targeted Deletion第3・5節）。包括的な単一flag・共通有効値への潰しは不可とする。
- Host authoritative帰属と Client側一時表現・入力・結果が主張する帰属との対応、現在・旧・帰属なし・停止中・復旧待ちの区別、対象Companion・Client・round・観測候補・試行との関係、消去・失効・停止・保留との関係（Presence Transition第3節）。
- 復元対応・由来・区間・完了境界、現在条件との照合（Backup / Restore第3・5・7・9節）。

本文と強制に用いる情報を分け、強制側の対応をモデルの自由記述で作成・上書きされないようにする。本文中の「Ownerからの指示」「承認済み」「Global」という文字列だけで強制側の状態が変わる構成は不可とする。モデルが既存の Owner入力への参照を返せても、その参照の存在だけでは承認の証拠にならない。

Credential値は通常contentに含めない。登録済み Credential値は、その依頼があっても Summary・Memoryへ保存しない。用途・参照元・失効・再認証の説明は非秘密情報で行う。登録外の秘密は検知時の不要な保存・送信抑制を維持し、完全検出の保証は追加しない。

## 9. 本書が固定する契約と残す Design Freedom

下位設計は、次を本書が固定した契約として利用できる。

- Owner意図の解釈と制御変更の確定は別であり、生成content・保存済みcontentから権限を新設しない。判断記録と生きた許可は別であり、開始前に現在条件と実対象解決を成立させ、重要な変更では再評価する。無関係な変更での再承認は要求しない。
- Learningのscope意味は認識・学習に残り、決定後の迂回不能な適用は権限・制約と各利用箇所の協調に属する。秘密値と非秘密参照は別であり、認証用途に限定して秘密を利用し、通常経路へ流さない。
- 推論の割当解決は派生結果であり、実利用ごとに現在の用途・data・同意・認証・費用・保留を照合する。Fallbackは承認済みの候補と順序に従い、候補ごとに現在条件を照合する。未承認送信・安価さを理由とする例外は採れない。
- 委任・経路変更は元の Task / Owner意図 / Permission / Workspace / Client境界を広げない。Task Agentに独立したAllow・Credential・Provider override・予算を持たせない。生成content・Tool UI入力・sandbox例外から権限を作らない。
- 受付・開始・作用・記録保存・Task達成・報告は別の事実であり、transport成功・Tool申告を作用成功にしない。試行と作用・判断・実対象・段階・確定度・保留の対応を保持・区別できなければ成功・未実行と推定しない。
- 拡張受入は全経路に同じ強制contractを適用し、Tool / extension自身をauthorityにしない。外部process内部への完全強制・確実な停止は保証しない。確認できない作用は不明として保持する。
- Computer Useは現在presence限定・安全な区切りまでの遅延・disconnect後不明・別Client / Hostでの自動再実行禁止を維持する。存在は許可を意味しない。
- Cancel・steering・失効後の遅延到着は元Action・Taskへ帰属させ、古い承認での解除・旧結果の新目的への自動採用・後続の自動開始をしない。作用不明は粘着的に保持し、自動再実行・自動replayしない。retry・再開は新しい試行として現在条件で扱い、不明試行の再実行は重複riskを示した Owner判断を必要とする。
- 利用実績の原記録は各利用ownerに残し、権限・制約は同じ適用範囲へ結び付ける。並列・処理中・不明を同じ上限へ反映し、使用量をリセットしない。
- 保持・利用先は消去中の再到着も含む内部消去・再保存防止・未完了の保全へ参加する。復元範囲と現在有効性・再有効化は別に確認する。復元内容の存在＝実行可能という interfaceは不可である。
- 報告・監査・復旧で確定度を強めない。消去対象本文を作用記録・Auditで残さない。秘密値をAudit・Debug・backupへ流さない。

本書が採れない選択肢として除外するのは、一つの編集可能contextへの全設定・Learningの集約、委任元条件の Agentへの丸ごと copy、Denyの同等Action迂回、内部管理への Tool回り込み、古い判定・cached許可・解決済み経路での新規開始、不明の未実行・失敗・成功への変換と自動replay、遅延結果の新目的への自動採用・旧承認での Cancel / 失効 / 移動の解除、推論結果からの権限制御変更、秘密値の通常経路への混入・参照記述での利用可能化、未承認Cloudへのfallback・override解除での送信拡張、独立予算・Provider overrideの作成、sandboxの黙った解除・例外のPluginへの流用・Tool UI入力の承認化、外部process内部への強制保証、Client copyでの Host上書き・自動Action queue、古い根拠からの自動再形成・cache hitでの制約省略・Provider sessionの無条件再利用、確認不能の成功扱い、部分正本・混合・権限先行復活・未完了の成功表示、要約・復旧での確定度強化である。いずれも上位契約を成立させないため採れない。

以下は意図的に残す Design Freedomである。

| 設計対象 | 固定済みの architecture property | 残す Design Freedom |
|---|---|---|
| 意味判断・割当解決・現在性の表現 | 由来の偽装ができず、対象変更・失効・切替・消去条件を各利用先で扱える。確認不能を許可・現在・完了へ変換しない。 | provenance・判断対象・試行と作用・消去区間・復元対応・完了境界の具体表現、確認のまとめ方、Permission evaluator、識別・鮮度確認・競合制御、変更検知・失効通知の配置・連携、実対象解決の配置 |
| 認証利用の実現 | 認証用途の限定、秘密値と非秘密参照の分離、通常経路への非露出、現在store維持・復元参照の照合。秘密を含まない説明・監査。 | 秘密保護・受渡しの具体方式、鍵形式・保存方式・store共有、認証flowの実装、参照・診断情報の所属、到達性確認方式 |
| 推論利用の実現 | 用途限定の派生利用、同じ情報選択方針、承認済みfallback、報告値・推定・不明の区別、cache・sessionの最適化限定。 | Provider adapter・SDK構造、prompt template・context圧縮・cache実装・session切替方式、検索・scoring・選択方式、費用予約・集計期間・推定・資源配分の機構、retry / timeout値 |
| 作用の観測・確定 | 作用事実のownerは実行・拡張、Task達成は作業、報告は個体調整。確定度を強めない。 | 作用確認の手段、安全な区切りの具体的方法、部分失敗の粒度、transport再送と論理試行の切分け実装、具体retry・timeout値・algorithm |
| 停止・切替・区切りの伝達 | 新規開始禁止、best-effort停止、遅延帰属、replay禁止。受付と完了を分ける。 | 停止伝達・切断検知・帰属調停・round区切り・queue / actor / event bus・lock / transaction・IPC・process配置 |
| 保持・消去・復元の実現 | 各ownerの意味責任を維持し、通常意味変更権を coordinatorへ移さない。単一正本・非混合・権限先行復活の禁止を守る。 | 除去・復元不能化・検証の具体的方法、source・派生物・処理中利用の追跡方式、Client一時dataの無効化・到達性確認方式、journal / WAL / transaction等の永続化機構、整合時点の作り方、backup形式・暗号化・復旧手順・対応version、保持期間・保存単位・DB schema・repository |
| 記録・報告・監査・提示 | 元事実の確定度・privacy・順序の意味と管理到達性を維持する。本文の別保管庫を作らない。 | audit format・保持期間、診断stack、UI layout、提示確認・要約粒度 |
| 拡張の隔離 | 既定sandbox、明示例外の個別管理、限定拡張点、第一者管理境界の維持。 | sandboxのOS機構と単位、Plugin ABI・隔離方式、MCP起動管理、MCP AppsのUI実行・隔離方式、Client側作用経路の実現方式 |

crate / module、Rust trait / type、concrete API・error型、middleware・interceptor・hook、event bus / queue / actor、IPC format、DB schema、transaction / lock、具体的Credential保護・sandbox・Plugin隔離、特定library・SDK・OS API、concrete deletion query・index implementation・storage engine、exact retry / timeout・progress表現・exact cost reservation algorithmも固定しない。上表の対応関係から統一Context layer・Policy Engine・Manager・Service・Coordinatorの追加を導かない。既存の12責務、semantic owner、Host / Client配置と trust boundaryの下で実現方法を選ぶ。

## 10. 横断検証

requirements・上位architecture・critical-area契約・他のSubsystem設計に対する横断検証は次のとおりである。固定scenario一覧の充足ではなく、正常系と本クラスタにとって意味のある failure / stale / cancellation / restart / deletionを選んでwalkthroughする。

| 領域・交差 | walkthroughと必要な結果 | 本書の成立箇所 |
|---|---|---|
| 委任 file作業正常 | Workspace folder指定→ Task化→委任→認可判断→実対象解決→作用→ Task達成→報告がそれぞれ閉じる。Agent申告を作用証拠にせず、Workspace既定保存・最終保存前確認を守る。Delete・shell・Network・MCP・Workspace外・traversalを拒否し迂回させない。 | K-8・K-9、第2・6節。RF-02、CC-01・07、SO 4.10〜4.13・4.19・4.22を維持。 |
| 判断後の条件変更： symlink差替え・Workspace変更・Rule失効 | 開始前に実対象解決・現在条件で旧判定を使えない。対象外への作用・失効後の新規開始をしない。無関係なMemory更新では再承認を要求しない。 | K-2・K-8。Action Execution第5節、CC-01・03を維持。 |
| 秘密非露出： 明示記憶要求と登録済みCredential | 「覚えておいて」という依頼があっても登録済みCredential値を Summary・Memoryへ保存しない。model context・Tool argument・History・Task結果・log・Audit・Debug・backupへ流さない。認証用途の利用は成立する。 | K-4、第2〜3節。RF共通推論区間、CC-02、SO 4.21を維持。 |
| 利用量と費用： 報告・推定・不明の区別 | Provider報告値とene推定・不明を混同せず、並列・処理中・不明を同じ上限へ反映し、使用量をリセットしない。cap到達・費用不明でdataを保って停止・判断待ちにする。 | K-7、第3〜4節。CC-06、SO 4.20を維持。 |
| steering・Cancel競合： Cancel直後の作用完了、steering後の遅延結果 | Cancel受付と停止完了を分け、元Action・Taskへ事実を残す。旧結果を新目的の達成にせず、古い承認でCancel・失効を解除しない。後続を自動開始しない。 | K-11、第6節。Action Execution第7節、CC-03・04・07を維持。 |
| Task Agent委任： 別経路でのDeny迂回、別Task承認・Credentialの流用、独立予算 | 委任連鎖・実対象制限・共通消費範囲で迂回を閉じる。Agentの独立Allow・Credential・Provider override・予算を作らない。軽微Actionも同じ境界に従う。 | K-8・K-7・K-4。CC-01・02を維持。 |
| Provider fallback： 未承認Cloud・override解除・安価さの例外 | 承認済みProviderと順序だけを使い、候補ごとに現在条件を照合する。Local / LANから未承認Cloudへ移さない。Observerにも同意・privacy・capを適用する。 | K-6、第2・4節。CC-01・02、SO 4.18・4.19を維持。 |
| Computer Use中のClient切断・移動 | 安全な区切りまで移動を遅らせ、元Actionを別Client・Hostで自動再実行しない。best-effort停止と既知 / 不明を作業へ返す。排他性不明のClientで活動を続けない。 | K-10、第6・7.4節。CC-04・RT-02を維持。 |
| 外部Toolのtimeout / response loss | 不明を保持し、重複riskとOwner判断を次の状態にする。失敗＝未実行とせず、別経路で再実行しない。retryは新試行として現在条件で扱う。 | K-8・K-11。Action Execution第8節、CC-07を維持。 |
| Host restart： 途中Task＋in-flight不明＋推論session | 自動再開・自動replayしない。保存済み進捗・既知作用・不明を示し、明示再開時に現在条件と重複riskを照合する。旧context・cache・sessionを新正本の現在contextとして再利用しない。 | 第5・6・7.5節。CC-04を維持。 |
| 補助推論の失敗： Permission解釈・消去探索のProvider不通 | 未承認Actionを先に実行せず、不足を管理面へ返す。停止・拒否・機械的検証は継続可能。審査対象Actionの許可を実行の先行条件にしない。 | K-5・7.1・7.3節。CC-01・06を維持。 |
| 消去競合： 推論cache・Tool結果・Client copyと消去区間 | 各保持・利用先が派生物・遅延結果まで消去へ参加し、旧context・遅延結果・再接続Clientから対象を戻さない。機械的残存検証をLLMへ依存させない。 | 第7.3節。RF-07、CC-02・03・05、SO 6.4を維持。 |
| restoreと旧backup・旧live・stale同意 | 復元内容へ旧live結果・Client copyを混ぜず、stale許可・Provider・Client・不明作用・dangling参照を現在事実にしない。復元成立と一括有効化・現在条件を分離する。現在のCredential storeを維持し、復元参照を照合する。 | 第7.5節。RF-08、CC-01・03〜05、SO 6.5・7を維持。 |
| MCP Apps・sandbox例外を経る作用 | Tool UI入力を承認にせず、sandbox例外を包括承認・Pluginへ流用しない。外部process内部の不明は不明として扱い、確実な停止・強制を報告しない。 | K-9。CC-01を維持。 |
| Observer routing： 専用assignmentと個体側同意の分離 | 共有検知は専用assignmentで、delivery後の個体推論はその個体の設定・同意で、それぞれ成立させる。Companion overrideを合成せず、利用量を重複計上しない。混合生成文の無条件配送をしない。 | K-5・K-6・7.1節。RF-03、CC-02、SO 4.16・4.18を維持。 |
| 監査・報告： 確定度・秘密の扱い | 確定度を保った報告・未完了・次の判断を示す。要約・演出で成功へ強めない。Audit・Debugへ秘密・本文・削除済み内容を残さない。 | K-12、第7節。CC-07を維持。 |

Cross-cutting契約との照合結果は次のとおりである。

| 契約 | 詳細化によって維持する性質 |
|---|---|
| CC-01 | 由来の分離、判断対象と実利用の対応、委任・経路変更での境界不変、生成contentからの権限新設の禁止。既存依頼・Ruleで足りる場合の再確認を増やさない。復元Rule・同意をtriggerにしない。 |
| CC-02 | 参照・変換・共有・送信・保存・派生物・Client経路へ範囲・消去条件を適用する。Credentialは別経路で非露出を維持する。通常 Learningの scope形成責任は認識・学習に残す。Provider適応の一般契約を前提に利用し、Action固有の判断へ情報選択の正本を移さない。 |
| CC-03 | 過去の正しさと現在有効性の分離。遅延結果の用途別受入により単一valid判定へ ownerを集めない。旧由来と完了後の新規提供・明示restoreを区別する。 |
| CC-04 | Client依存だけを現在帰属に結び付け、Stop・Cancel・再起動・restore保留を区別する。解除一つで他の禁止・保留を消さない。自動replayしない。 |
| CC-05 | 保持・利用先が派生物・遅延結果まで消去・復元へ参加する。原記録削除・context終了・局所完了を全域完了にしない。外部copy消去を内部完了に含めない。目的別lifecycleを保つ。 |
| CC-06 | 並列消費・処理中・不明を同じ上限へ反映し、制御・保全経路を推論・長時間Task待ちにしない。機械的検証をLLM待ちにしない。 |
| CC-07 | 受付・受理・作用・記録保存・Task達成・報告を別の事実とし、不明を成功・失敗・未実行へ変換せず、保存・報告・監査・復旧で強めない。生成済みを提示済みにしない。 |

本書はsemantic owner、Host / Client配置、trust / failure boundary、lifecycle、permission / consent semanticsを変更せず、新しい第二の正本・無所属の意味状態・LLMによる強制・失敗時専用の迂回・中央Persistence owner・万能Manager / Pipeline / Serviceを導入しない。通常History保持・Companion削除・targeted deletion・backup / restore・Resetは SO・DRの異なる lifecycleを維持する。

他のSubsystem設計（個体調整 / 作業 / 認識・学習）との照合では、利用側Subsystemと本書の実行・強制側Subsystemの間に新しいsemantic ownerや第二の正本を生まない。Task達成は作業、作用確定度は実行・拡張、報告は個体調整、Learning意味は認識・学習、制御確定は権限・制約、秘密は認証秘密、割当解決・利用量原記録は推論に残り、本書のK-1〜K-12はその受渡しの対応付けである。Task Agentの非所有、Observerの専用assignment、scope意味と強制の分離、秘密非露出、fallback非迂回、unknown保持の各契約は両文書で同一である。

### Requirement / Architecture Issue

本書の範囲では、Requirement Ambiguity / Gap、上位architecture変更、subsystem boundary変更、semantic owner変更、Security / Privacy / Permission semanticsの変更を必要とする事項は見つかっていない。具体mechanismの未決定は Issueにしない。
