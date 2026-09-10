# Subsystem Detailed Design — Client接続・存在調停 / Owner入出力・体験提示 / 共有観測・関連付け

本書は Step 12 Subsystem Detailed Design として、次の3 Subsystemを一つの連携クラスタとして詳細化する。

- Client接続・存在調停（以下、接続・存在）
- Owner入出力・体験提示（以下、入出力・提示）
- 共有観測・関連付け（以下、共有観測）

本書内の CN / IO / OB 番号は本書内の内部責務の参照用であり、製品要件IDではない。
X番号は本書内のSubsystem間semantic contractの参照用である。

## 0. 位置付けと不変条件

上位設計との優先順位と矛盾時の扱いは [設計文書 README](../README.md#正本と優先順位) に従う。[Context Assembly](../critical-areas/context-assembly.md)、[Action Execution](../critical-areas/action-execution.md)、[Targeted Deletion](../critical-areas/targeted-deletion.md)、[Client Presence Transition](../critical-areas/client-presence-transition.md)（以下、Presence Transition）、[Backup / Restore](../critical-areas/backup-restore.md)の一般契約は、隣接する確定済み contract として利用し、再定義も所有権の移動もしない。本書内の SO は [State Ownership](../architecture/state-ownership.md) の節番号を指し、CC / RF / RT 番号は対応する architecture 文書（[artifact 一覧](../README.md#artifact-一覧)）の契約 ID である。

新しい semantic owner、第二の正本、万能Presence Manager / Context Manager / Observer State owner / I/O正本、統一presence state machine、共通Client session layer、共通Context layerを追加しない。call graph・event bus・queue / actor・IPC・network protocolは固定しない。

次の既存architecture意味を維持する。

- Hostがcanonical stateを保持する。Clientはcanonical persistent state holderではない。
- Running Companionは同時に最大1つのactive Clientへpresenceを持つ。Stopped Companionはどこにもpresenceを持たない。
- connectionとpresenceは別である。Host-side Task継続とClient presenceは別である。
- Client依存activityは現在presenceへ帰属する。Computer Useは現在presenceのあるClient限定である。
- Ownerとの通常の会話はCompanionが存在するClientで行う。
- Client切断やHost restartがAction replay authorityにならない。stale Client stateをHost正本として扱わない。
- presentation成功をTask成功やAction成功へ変換しない。

`client-presence-transition.md` で確定したpresence切替そのものを再設計しない。本書はそのcontractを今回の3 Subsystemの責任へ落とす。

## 1. 選定理由

この3者は Client帰属・round・適格性・未伝達・staleという同じ対応付けを共有し、分離して詳細化すると同じ帰属照合を3箇所で重複定義することになる。同時に、後続が誤って一つの状態・単一boolean・Client権威・I/O正本・Observer状態ownerへ潰しやすい境界を最も多く含む。

- 接続・存在と入出力・提示を分けずに設計すると、接続中device一覧と個体ごとのactive帰属が同一状態になり、接続だけで存在・操作が許される。逆に両者をClient側へ寄せると Clientがpresence authorityになる。
- 入出力・提示をdomain判断と結合すると、描画・音声障害やUI変更が継続・権限へ波及する。Body・Text・Voice・管理・安全操作・MCP Apps外部Tool UIを一つのstate・trust level・failure domainへ潰すと、Body / Voice障害がText・管理・復旧を塞ぐ。
- 共有観測を個体調整へ吸収すると重複検知・設定scope混同が生まれ、Computer Useと統合すると観測同意が操作許可へ漏れる。Capture・候補検知・routingを別Subsystemへ割ると同じ観測範囲・頻度・送信条件が重複する。
- 未伝達・報告状況・Task記録・History・提示の実際を一つの通知store・既読状態へ潰すと、Task完了＝報告済み、表示送信＝報告完了という誤った確定が生まれる。
- Observer routing用派生表現を新しいcanonical state・scope・semantic ownerへ昇格させると、Companion scopeからGlobalへの変更・他Companionへの共有・private全体の公開が黙って成立する。

最初のmilestone（最小Setup〜テキスト会話、一対一timeline、基本Body、Client不在時のTask継続とHost再起動後の明示再開・presence復旧、Text管理面）は、接続・存在の帰属と入出力・提示のround・提示分離が閉じて初めて検証できる。Voice・Observation・グループ・自発交流・Remote・Character配布は後続milestoneだが、本書はそれらの確定済み契約を除外せず、将来の受渡しを塞がない形で定める。routing model・Prompt・capture cadence algorithm・heartbeat / lease / epoch / session token・network protocolは固定しない。

## 2. 内部責務の分離

### 2.1 接続・存在

利用可能なClientへの接続と、個体がそのClientに存在することを区別しながら、安全な入出力の帰属を成立させる。Host管理下で帰属を扱い、Client側も切断・排他性不明時の停止を成立させる。専用presence service・接続processの指定ではない。

| 内部責務 | authoritativelyに判断するもの | 持ってはならない正本 |
|---|---|---|
| CN-1 接続の事実・機能利用可能性 | Clientの識別に必要な情報・最終接続・現在の接続・機能利用可能性。到達性という観測事実と現在性の管理。最終接続等の管理記録と現在接続の区別。現在接続を古い保存値から再成立させないこと。 | pairing許可・device許可・失効の意味（権限・制約）、認証材料の秘密保護（認証秘密）、Action許可・active帰属の成立 |
| CN-2 authoritative帰属の調停 | Running Companionごとの排他的なactive Client帰属・移動中・activeなしの扱いの確定。最大一つ、Stoppedは持たないこと。個体調整の活動状態と権限・制約のpairing・device許可を参照して成立させる。Hostが管理する個体ごとの帰属記録だけをauthoritativeとする。 | 移動の会話上の必要性（個体調整）、Task達成・作用成功、round・提示の実際、存在＝許可という推定 |
| CN-3 呼出し・移動・復帰の調停 | 通常の現在Client滞在、呼出し・明示・事前指示・文脈上の自発移動、切断時の復帰、Host再起動後の元Clientへの自動復旧（自発移動とは別の復旧）の調停。Host側Client環境の自動起動をしないこと。別Clientへの無条件移動・Stoppedへの適用・途中Taskの自動再開との区別。移動の必要性判断は個体調整、成立は接続・存在に残る。 | 個体の生成・停止・削除、Task全体の移送・再実行、Computer Useへの許可付与、Body・Voiceの実際の入出力、再配置hint選択algorithm |
| CN-4 切替区間の新規開始禁止 | 旧presence・移行中・新presence・activeなし・停止中・復旧待ちの論理的な区別。切替決定から新presence成立まで新規のClient依存開始を旧・新のいずれでも行わないこと。二重presenceの禁止。単一boolean・単一active fieldへの潰しは不可。 | Task全体の終了を区切りの条件にすること、到達不能Clientの成功扱い、切替未成立時の戻り先選択algorithm |
| CN-5 stale識別に必要な対応の保持 | Host authoritative帰属とClient側一時表現・入力・結果が主張する帰属との対応、現在・旧・帰属なし・停止中・復旧待ちの区別、対象Companion・Client・round・観測候補・試行との関係、消去・失効・停止・保留との関係の保持・照合。確認できないものを現在として推定しないこと。具体的なtoken・lease・heartbeat・epoch・sequenceは固定しない。 | Client表示・過去記録・hint・一時copy・復旧先記録・Provider残存を根拠とするpresence成立、古い設定copy・外部codeによる同意・現在性の独立決定 |
| CN-6 Client依存activityの開始可否への参加 | 対象個体のactive Client・現在接続・機能利用可能性という帰属側の連言項の供給。開始可否自体は帰属・接続・許可・現地状態・各活動設定の連言であり、接続・存在だけで完結させない。pairing済み・接続済み・active・Action許可済みは四つの異なる意味であること。 | Pairing済み＝active＝Action許可済みという導出、古い帰属での継続、管理面全体への会話と同じactive制約の拡張 |
| CN-7 再配置hint・復旧先の管理 | 最後のClient・復帰候補等の再配置hint、Host再起動前のClientという復旧先の管理。いずれも現在帰属とは分けること。記録だけで現在presenceが成立したとは扱わないこと。接続回復だけでの停止個体の再開・再配置をしないこと。 | hint・復旧先記録のcanonical化、候補選択algorithm、再接続・待機・timeout・pairing方式 |

CN-1〜CN-7は同じ調停責任に属するが、一つの状態・lifecycle・保存単位ではない。接続・帰属・許可・Host継続を同一状態にしない。Host同居Clientにも同じ帰属制約を適用する。

### 2.2 入出力・提示

同じCompanionから会話と作業を利用する体験と、障害時にも到達できる第一者の管理・安全操作を提供する。提示はdomainの意味をOwnerが扱える形へ変える責任であり、その意味の正本ではない。

| 内部責務 | authoritativelyに判断するもの | 持ってはならない正本 |
|---|---|---|
| IO-1 Text入出力・round | Body・Text・Voiceを通じた入力・提示のうちTextの受付・提示・安全な区切り。一対一timeline・グループ・Task管理の意味上の区別を表示へ反映すること。入力がどのCompanion / Client / roundへ帰属するかの実際の受付範囲の管理。Client受付・Host受理・応答提示の区別。 | 会話の参加者・発言・文脈・Historyの意味（個体調整）、Taskへの反映（作業）、帰属成立（接続・存在）、timelineの意味・報告完了の代行確定 |
| IO-2 Voice入出力・待受・縮退 | Voiceの物理的入出力、VAD待受の常時識別、即時Mute、barge-in、Realtimeからturn-based Voice・Textへの段階的切替。話者認証を行わないことの有効化時・管理面での伝達。Mute・Voice停止・会話停止・承認拒否のkeyboard経路。Voice障害のText利用への非波及と切替・失敗理由の提示。 | 話者認証済みという属性、音声由来の本人認証、帰属切替の迂回としてのVoice縮退、Voice成功＝Task成功という推定 |
| IO-3 Body表示・状態表現・縮退 | Windows / LinuxのVRM 1.0 overlay、移動・resize・hide、待機中・聞き取り中・応答中・作業中・注意が必要な状態の表情・姿勢・motion等による区別。位置・size・hide等の一般設定の意味と実際の表示の区別。fullscreen・高負荷・device障害時の休止・品質低下とText・管理・復旧の保護。Body失敗時のText・Task管理・設定・復旧の維持。 | 内的状態の正本としての表情・motion、CharacterのBody部品自体、Provider割当同意・device許可の意味、描画frame・motion再生位置の永続化 |
| IO-4 通知・未伝達の提示 | Client不在のために延期された事項の実際の提示。提示材料の受取と提示結果の供給。接続・表示copy送信だけで報告完了にしないこと。提示不明の保持。active Client不在ならHostに必要内容を残す前提での提示。 | 未伝達の必要内容・報告状況の正本（個体調整）、Task進捗・結果自体（作業）、生成候補＝伝達済み・Task完了＝報告済みという推定 |
| IO-5 管理・安全surface | Setup・段階的Capability導入・許可・費用・Privacy・由来・診断・復旧へ到達できる管理経路。UI・Body・Voiceの一般設定とOwnerが選ぶHost自動起動設定の意味の管理。各操作の意味・適用・成功判定は担当Subsystemから受けること。重要音声内容のText代替、日英で同じ意味の説明。受付と完了・保存影響・既知作用と不明の区別提示。 | Task開始条件・成功、Learning形成、Permission承認の妥当性、削除・restore完了の独自確定、画面閉鎖＝Task Cancel、UI入力の無検証でのControl plane変更、Body・Voice成功・LLM応答・外部Tool UIの管理前提化 |
| IO-6 round・区切りの実際と移動協調 | 現在のText / Voice roundの安全な区切り、移動元・移動先の状態の対応付け、両側への把握できた移動状態の提示。通常Host Task終了を区切り条件にしないこと。到達不能Clientの成功扱いをしないこと。 | 移動調停そのもの（接続・存在）、timeline終了・削除としてのround終了、Task全体の移送・再実行 |
| IO-7 障害・縮退の分離と外部Tool UI | Fullscreen・負荷・device障害の関係責務への通知、Body・Voice失敗からのText・管理・復旧の保護。MCP Appsを外部Tool UIとして提示し、第一者の承認・設定権限と区別すること。Tool UI終了とserver・Task・Action終了の区別。 | Body・Voice・Text・管理・安全操作・MCP Appsを一つのstate・trust・failure domainとする扱い、Client表示copyのHost canonical化、操作受付＝処理完了＝結果不明の混同、外部UIのControl plane昇格 |
| IO-8 提示確定度の分離 | 入力受付・Host受理・Ownerへの提示・処理完了の区別。表示用data送信・round終了でHistory・Task・承認・作用を確定しないこと。内部推論・chain-of-thought・詳細Promptの非表示、Relationship・Companion Stateの数値meter・常設理由panel化の禁止。 | 会話History・Task進捗・Permission・作用成功の第二の正本、Provider sessionの継続会話の正本化、Clientだけでの永続会話の確定 |

IO-1〜IO-8は同じ体験責任に属するが、一つのstate・trust level・failure domainではない。Body・Voice・外部Tool UIの障害がText・管理を塞がないこと、操作要求の受付・処理完了・結果不明を混同しないことを保つ。Setup・設定画面は各状態への入力経路であり、全設定のownerにはならない。Host自動起動の選択は起動・日常利用体験の選択として入出力・提示がsemantic ownerとなるが、OSへの設定作用・適用結果は実行・拡張、保全・復旧への参加調整は保全・消去が担う。選択済みとOS適用済みを混同しない。この設定からTask明示再開・restore後有効化・Host側Client環境の自動起動を導かない。

### 2.3 共有観測

Client単位の観測コストと制御を共有し、個体ごとの判断に必要なeventへ絞って届ける。取得元はClient、対象・個体帰属と継続的な制御はHostの管理に関係する。検知・関連付けの計算をClientへ固定しない。Observer adapterの外部codeは限定された拡張境界に残す。

| 内部責務 | authoritativelyに判断するもの | 持ってはならない正本 |
|---|---|---|
| OB-1 対象・時機の確定 | ONになっておりCompanionが一体以上存在するClientのdesktop全体という対象の確定。存在人数のauthoritative帰属への連動、Stoppedの除外、Client側表示残存・旧帰属一時dataの不算入。Client別・全体Pause / OFF・fullscreen・費用・資源制限を頻度より優先すること。複数対象Clientの同時Capture禁止・時機ずらし・負荷分散。設定ON＝常時実行中という表示の禁止。 | 各Companionの最終意味・発話・Action判断、個体自発性設定、形成済みLearning削除、Computer Useの操作・承認、個別windowの所有・観測設定、Task Schedule |
| OB-2 共有Capture・候補検知 | Clientごとの指定頻度を満たしつつ時機をずらしたCapture、Clientに紐づくObserver専用assignmentでの候補検知・文脈との関連付け。Companion overrideの適用・合成の禁止、全個体への無条件配信の禁止、個体ごとの検知重複の禁止。取得・adapterは実行・拡張の境界、対象・時機は共有観測の責任。 | 画面内指示のOwner依頼への昇格、関連付けのための全私的状態への無制限access、軽量Local・安価Cloudの必須model条件化 |
| OB-3 専用assignmentによる利用 | Observer専用model / Provider assignmentの解決前提（解決自体は推論、同意のownerは権限・制約）。共有Capture・candidate detection・routingとdelivery後のCompanion reasoningの別consumer化。後者だけがCompanion設定に従うこと。Cloudにも通常の同意・privacy・費用制約の適用。 | Companion override・同意の合成選択、全Client共通model・Client別UI・Host default継承階層の固定、個体数による利用量の重複計上・個体側同意の流用 |
| OB-4 限定routing文脈の受取・維持 | routingに必要な範囲へ要約・制限されたCompanion固有文脈の受取と、元情報・対象Companion・用途・制約の対応を保った派生表現としての維持。History・個体文脈は個体調整、Memory・Learningは認識・学習、Task contextは作業の意味責任の下で既存の個体調整–作業協調を通じて受けること。全private公開・新Learning正本・Global化・他Companion共有にしないこと。 | 新しいcanonical state・scope・semantic owner、private全体・他用途へのaccess、要約・変換による制約・同意の消去、生成方法・形式・更新頻度・鮮度・選択algorithmの固定 |
| OB-5 routing・deliveryの採否 | そのClient上の関連するRunning Companionだけへのrouting、複数該当時の複数delivery、現在の対象・用途への照合による採否。モデルによる任意の個体・Client追加・候補本文からの所属・制御変更の禁止。 | 最終Action判断、Observer成功＝個体側推論成功・送信同意・Action許可という推定、一方の個体の判断成功の他個体へのコピー |
| OB-6 鮮度・観測制御の適用 | 送信・delivery前の対象Client・存在個体・観測制御・限定文脈の利用条件・消去等の保留との対応の確認。移動前Captureの移動先付け替えの禁止、Stoppedの覚醒の禁止。Pause / OFF・fullscreen途中の今後の観測・提示の停止と未完了Capture・検知・deliveryの現在条件への照合。旧設定だけでの次回観測・送信の禁止、fullscreen中の自発発話への流用禁止。 | 取得済み候補の一律破棄・条件付き継続の固定方針、すでに認識したExperienceの観測OFFでの消去、delivery後の個体Provider送信の自動成立 |
| OB-7 状態説明・非保存 | 観測の状態・対象範囲・Learning利用の説明可能性、Raw Observationの通常非保存。候補を出来事の意味・Learningの正本にしないこと。delivery後の各CompanionのメインLLMによる認識は個体調整、形成されたsemantic state・Summaryは認識・学習という帰属の維持。個体が意味を認識したeventはeventごとのOwner確認を要求せず通常のユーザー入力と同様にExperienceへ利用できること。発話またはActionを行う最終判断は候補を受けた各Companionが自身のCharacter・関係・状況・Ruleに基づいて行うこと。Taskで行うComputer Useは依頼・自発の別によらずambient Observationと区別し、TaskのPermissionと記録を適用すること。 | 全活動の永続記録、Raw Capture・観測候補・一時reasoningの全保存、新しい汎用Activity Subsystem |

OB-1〜OB-7は同じ観測範囲・頻度・送信条件を維持する一連の責任であり、Capture・候補検知・routingを別Subsystemにする独立した製品責任はない。Observerを独立Companion・Task Agentにしない。

## 3. 管理する logical state

特に外部・copy・一時と記したものを除き、Hostで管理する内部状態を指す。永続化の実装担当・保存単位・唯一writer・transactionを指定しない。通常ownerは正当な全域消去への参加を拒めない。

### 3.1 接続・存在が意味ownerとなる state

- Clientの識別に必要な情報・最終接続・現在の接続・機能利用可能性（SO 4.15）。最終接続等の管理記録と現在接続を区別する。接続・OSによる機能の存在は利用許可ではない。現在接続は古い保存値から再成立させない。
- Companionのactive Client帰属・移動中・activeなしの扱い（SO 4.15）。個体調整が移動の必要性を判断し、入出力・提示・共有観測・実行・拡張が現在の帰属を利用する。Task担当・操作許可とは別。
- Host再起動前のClientという復旧先、最後のClient・復帰候補等の再配置hint。いずれも現在帰属とは分け、記録だけで現在presenceが成立したとは扱わない。再接続・待機・timeout・pairing・調停方式は定めない。
- Stopped Companionの現在帰属の解除状態。個体調整の停止状態に従い、共有観測へ渡す存在人数・routing対象から除く。Host保持の個体data・hintはpresenceではない。

帰属はHostが管理する現在の有効な状態であり、Clientの表示・過去にactiveだった記録を正本にしない。Host管理の帰属とClient側の実際の利用可能性を合わせて扱い、排他性が確認できないClientは対象入出力・観測・自発的interaction・Computer Useを続けない。Running Companionのactive Clientは同時に一つまでである。pairing済み・接続済み・active・Action許可済みは四つの異なる意味である。Host側の確認に基づくpairing許可・device別の許可機能・失効の意味・失効範囲自体の正本は権限・制約に残し、接続・存在はそれを存在・接続調停へ反映する。Client固有の接続材料の利用・更新・再pairingへの対応は接続・存在、秘密を含む部分の保護は認証秘密、Host側の信頼・許可・失効の正本は権限・制約に残す（SO第8節）。

### 3.2 入出力・提示が意味ownerとなる state

- 入出力roundの受付・提示・安全な区切り（SO 4.3）。入力・音声出力等がどこまで行われ、区切れるかという実際の入出力状況。active帰属は接続・存在を参照する。一時roundの終了でtimelineを終了・削除しない。移動の調停そのものは接続・存在が担う。
- UI言語・Bodyの位置・size・hide・Voiceの一般利用設定等の意味（SO 4.17）。保持する一般設定はHost正本とし、画面内での未確定編集・描画上の位置と区別する。Companion固有の適用対象を持つ設定は、その個体の削除範囲へ対応付ける。
- 描画frame・motion再生位置・音声buffer・VAD・barge-in・現在のMute・device利用状況等の実際の入出力としてのruntime状態。永続する内的傾向・発言の正本ではない。Mute等の即時local操作は成立させつつ、Hostが受け取っていない停止をHost作業の停止完了にしない。
- Host自動起動の選択（SO 4.17）。Ownerが選ぶ一般的な起動・日常利用設定としてのsemantic owner。選択の正本はHostに置き、OSへの設定作用・確認できた適用結果は実行・拡張、backup・restore・Reset等の全域操作への参加調整は保全・消去が担う。
- MCP Appsの表示・操作中dataという一時表現（SO 4.22）。MCP側の業務状態・eneのPermissionの正本ではない。

Client上の入力中data・表示用timeline・音声bufferは一時表現である。入力受付・Hostでの会話・指示の受理・Ownerへの提示・処理完了を区別し、Clientだけで永続する会話を確定しない。表示・round状態を過去発言・Task進捗の第二の正本にしない。一対一継続・グループと交流の参加関係・Historyの意味・進行中の意味判断は個体調整、Taskへの採用・反映・達成は作業、許可の確定は権限・制約、作用の把握は実行・拡張に残る。Voice入力に話者認証済みという属性を与えない。OSのfullscreen・負荷・device状態は外部の現在事実であり、ene側の検知結果には鮮度と利用可能性がある。Body品質低下・休止・Voice fallback・Text管理面の利用可否を一つの成功フラグへまとめない。日英の表示・Voice / Text切替で元のPermission・結果・失敗の意味を変えない。

### 3.3 共有観測が意味ownerとなる state

- Clientごと・ene全体のObserver ON / Pause / OFF、Clientごとの頻度という観測運用設定（SO 4.16）。Ownerが設定する内容はHost正本とし、観測adapter・Client内の表示設定へ所有を移さない。
- 観測の実効的な可否・時機というruntime状態。観測設定・現在の存在個体・接続・fullscreen・送信同意・費用・資源制限を参照した結果である。複数ClientのCaptureは同時に行わず、Clientごとに共有する。時機の調整状態は共有観測のruntime状態であり、Task Scheduleではない。
- CaptureされたRaw・候補検知・routing用dataという一時data。通常保存しない。候補は出来事の意味・Learningの正本ではない。
- Companion固有のrouting用文脈という派生表現の保持・利用範囲（SO 4.16）。元情報に従属するrouting用途の一時的な派生表現であり、Summary・新たなLearning正本ではない。提供元と共有観測は、元情報・対象Companion・用途・適用される制約の対応を保つ。生成方法・生成に用いるmodel / Provider・形式・更新頻度・鮮度・選択algorithmは固定しない。

観測運用設定のownerは共有観測、assignment・送信同意のownerは権限・制約、実効経路の解決は推論、個体別の自発性設定・抑制は個体調整、Quiet hours等のRule・Permission・費用・資源・loopの強制上限は権限・制約、実際のMute・入出力状況は入出力・提示の正本を参照する。存在個体にStoppedを数えないことと、Host内の自発交流・通知・内部調査の停止とは別の判定だが、同じ停止状態を参照する。ObserverのClientごと・全体のPause / OFFは今後の観測停止であり、形成済みLearning・Companion Stateの削除とは区別する。有効化時に観測したeventがLearningやCompanion Stateの形成にも利用されることを説明可能にする。画面内容を外部Providerへ送る構成では、送信先・desktop全体が対象になり得ること・用途・取扱い・費用を明示して、そのCapabilityへOwnerが割り当てるまで送信しない。

### 3.4 canonical / derived / temporary / historical / undelivered / observer-derived の扱い

- canonicalとして参照するもの：Hostが管理する個体ごとの帰属記録・現在の接続・最終接続等の管理記録、入出力roundの受付・提示・区切りの実際・一般設定・Host自動起動の選択、観測運用設定・実効的な可否・時機、会話History・Task記録・Learning・制御条件等の各意味ownerの正本。保存されたAllow・判定copy・解決済み経路・推論context内の許可文・cached判定・有効Provider経路・次回表示・費用集計・Client表示・過去帰属・hint・復旧先記録・Provider残存はcanonicalにしない。
- derivedとして扱うもの：報告用要約・由来説明・進捗表示・Body・Voice出力・embedding・index・similarity・score・Prompt cache・Provider session・一時context・有効経路・次回表示・費用集計・解決済み割当経路。派生物の生成・保持・破棄を行う責務が元stateとの対応と利用範囲を説明し、訂正・scope変更・targeted deletionへ参加する。古い派生物から現在帰属・権限を復活させない。
- temporaryとして扱うもの：Client表示copy・入力途中data・表示用timeline・音声buffer・未送信操作・観測候補・推論中context・MCP Apps表示data・描画frame・motion再生位置・VAD・barge-in状態、CaptureされたRaw・候補検知・routing用data。失ってよいのは一時表現であり、受理済みの指示・必要な作業記録・未伝達事項・作用不明まで失ってよいわけではない。意味を残す必要がある時点で対応するHost正本へ反映し、正本への反映が不明なものを保存・完了済みと表示しない。
- historicalとして扱うもの：一対一・グループ・Companion間交流のHistory、保存された非会話活動記録・evidence、Task記録。個体削除だけでは消さず、通常保持・backup・targeted deletionへ参加させる。個体固有Memory / Learning・Summary等の削除とは分け、全reasoning・Raw保存は要求しない。
- undeliveredとして扱うもの：Client不在のため伝えられなかった事項・対象・元対応・次Clientでの要約報告状況。個体調整が正本であり、Task由来ならTask記録、会話・交流由来なら活動記録へ参照を戻す。報告用要約は派生表現。接続・表示copy送信・Task完了だけで報告済みにしない。実際の提示状況を受けて個体調整が報告状況を更新する。
- observer-derived routing contextとして扱うもの：History・個体文脈・Memory・Learning・Task contextをroutingに必要な範囲へ要約・制限したCompanion固有文脈。元情報に従属する一時的な派生表現であり、Summary・新たなLearning正本・新scope・semantic ownerではない。Companion scopeからGlobalへの変更・他Companionへの共有・private全体の公開を意味しない。元情報の利用制約とObserver専用assignmentの送信同意を変換後にもともに適用する。scope変更・同意失効・targeted deletion等は生成済み要約・処理中結果にも適用する。

Client cache・UI状態から新しい正本を作らない。ClientはHostからのdataの一時的な利用者であり、History・Summary・Learning・Relationship・Companion State・Provider / MCP等の登録済みCredentialを永続cacheしない。Clientの未送信操作を接続回復後の自動Action queueとして扱わない。Host不在時に代替正本・独立したOffline実行を作らない。Client切断時に失われてよいのは未確定の編集・入力、描画・再生buffer、再取得できる表示copy等である。Hostが受理したTask指示・会話記録・報告すべき結果・作用不明・制御変更・全域操作の進捗はClientだけに置かない。

## 4. Subsystem間の受渡し（semantic contract）

「AがBを呼ぶ」の実装構造を先に決めるのではなく、要求開始・authoritative判断・受渡内容・失ってはならない前提・結果受入を定める。非同期message・同期call・shared state等の mechanismは後続へ残す。

### X-1 presence成立の前後関係

- 開始：個体調整が移動意図（Owner呼出し・事前指示・文脈上の必要性）を接続・存在へ渡す。自発移動も通常の自発性・Permission等の制限に従う。
- 判断：現在の帰属・移動成立・activeなしの扱いは接続・存在が、個体の同一性・活動状態は個体調整が、pairing・device許可・同意・cap・失効の現在可否は権限・制約が確定する。移動の必要性と成立は別である。
- 受渡内容：対象個体・移動元・移動先・現在の帰属・到達性・device条件、入出力・提示と実行・拡張からの区切り・利用不能・作用不明、共有観測への存在する個体と対象Clientの変化、保全・消去への接続中Clientの一時dataを扱う参加先の把握。
- 失ってはならないもの：presence・Host継続・接続・許可の区別、復旧先記録・hintだけで現在presenceが成立したという扱いの禁止、Client表示・過去記録・一時copy・Provider残存の根拠化の禁止、Host同居Clientへの同じ帰属制約の適用、管理面全体への会話と同じactive制約の拡張の禁止。
- 受入：現在の接続・許可・排他性を確認できて初めてpresenceが成立する。元Clientが利用可能になるまではactiveなしとして扱える。別Clientへの無条件移動・Stoppedへの適用・途中Taskの自動再開・Host側Client環境の自動起動に広げない。

### X-2 Text / Voice roundの帰属と移動時の扱い

- 開始：入出力・提示がactive Clientの会話入力・呼出しの意図を個体調整へ渡す。別Clientからの会話は先に呼出し・移動を成立させる。元Clientに残したまま別ClientからTextだけを送って応答させることを基本モデルにしない。
- 判断：会話の意味・Historyは個体調整、実際のround・提示・区切りは入出力・提示、帰属成立は接続・存在が確定する。Textを含む入出力roundとClient依存Actionの安全な区切りを関係Subsystemから受け、移動元・移動先の状態を対応付ける。
- 受渡内容：対象Companion・Client・round・観測候補・試行との関係、Context Assembly第3節のClient依存の対応、旧roundの入力・生成途中・未提示と新roundの区別、Voiceのsession的な利用と新しいsession・roundとしての開始条件（現在の割当同意・device許可・費用条件）。
- 失ってはならないもの：旧roundの入力・生成途中・未提示の新roundへの自動継続の禁止、旧roundの結果の元round・Historyへの対応付け、逐語の自動転送・旧roundへの自動出力の禁止、切替区間の新規自発開始の抑止、Voiceに話者認証済みの意味を足さないこと、Voice障害の帰属切替の迂回にしないこと。
- 受入：切替決定から新presence成立まで、新規のClient依存開始を旧・新のいずれでも行わない。旧ではin-flightを安全な区切りまで完了させる範囲だけを許し、新では成立後の新しいround・試行だけを許す。旧roundの出力は未伝達管理へ接続し、次Clientで現在の結果・利用制限に基づいて要約報告する。生成済み＝提示済みにしない。

### X-3 Body表示の帰属

- 開始：個体調整の活動状態と接続・存在の帰属が揃って初めて、入出力・提示がBody表示を行う。
- 判断：Body表示の実際・区切りは入出力・提示、帰属成立は接続・存在、内的状態の意味は認識・学習、表示資材の内容はCharacterが確定する。表情・motionは内的状態の出力であり、その正本にはしない。
- 受渡内容：対象個体・現在帰属・利用可能性、位置・size・hide等の一般設定（Host正本）と実際の表示の区別、fullscreen・高負荷・device障害の通知。
- 失ってはならないもの：二重表示の禁止、旧Clientでのhide・休止と新Clientでの成立後表示、出力した表情・motionのCompanion Stateの正本化・永続変化の根拠化の禁止、描画失敗・fullscreen・高負荷のText・管理・復旧への非波及。
- 受入：排他性を確認できないClientではBody・対象入出力・観測・自発的interaction・Computer Useを継続させない。排他性の対象となるBody・Realtime / Text会話・Voice・ambient Observationとの関係・自発的interaction・Computer Useは、一個体につき一か所へ結び付ける。

### X-4 Observation eligibilityの連動

- 開始：接続・存在が対象Clientと存在個体の変化を共有観測へ知らせる。入出力・提示がfullscreen等を共有観測へ渡す。
- 判断：対象・時機・候補検知・routingは共有観測、帰属成立は接続・存在、個体の最終意味判断は個体調整、送信・利用制限は権限・制約と各利用箇所、取得・adapter境界は実行・拡張が確定する。対象Clientの適格性と個体の自発判断は別である。
- 受渡内容：authoritative帰属に従う存在人数・routing対象、Client別・全体Pause / OFF・fullscreen・送信同意・費用・資源制限、移動先ClientのObserver設定と全体制御、自発性設定のCompanion単位の引継ぎ。
- 失ってはならないもの：Stoppedの人数・routing対象への不算入、Client側の表示残存・旧帰属一時dataの不算入、旧Clientの旧Capture・候補での新規Capture・delivery継続の禁止、移動先への古いCaptureの自動付け替えの禁止、activeなしの間の新規観測の不発生、到達不能Clientの旧人数での観測・routing継続の禁止、旧設定だけでの次回観測・送信の禁止。
- 受入：移動では旧Clientの対象性と新Clientの対象性を現在帰属で見直す。旧Clientで最後の個体が抜ければそのClientの新規観測対象性がなくなる。切替区間は旧Capture・候補で新規Capture・deliveryを続けない。複数対象Clientは同時にCaptureせず、指定頻度に対し制限を優先しつつ時機をずらす。

### X-5 Observation result routingの三段階分離

- 開始：共有観測が現在の対象Client・存在個体とrouting用途を対応付け、個体調整との協働を通じて必要な限定文脈を得る。
- 判断：提供範囲の意味は各元owner（History・個体文脈は個体調整、Memory・Learningは認識・学習、Task contextは作業が元情報の意味と提供範囲に責任を持ち、既存の個体調整–作業の協調を通じて参照する）、送信・利用制限は権限・制約と各利用箇所、routing採否は共有観測、個体の理解・発話・Actionは個体調整が確定する。Observerによるroutingは各Companionの意味判断・最終的な発話・Action判断を置き換えない。
- 受渡内容：元情報に従属するrouting用途の一時的な派生表現（対象Companion・元情報・用途・制限・選択時の前提を解決できること）、元の観測候補と候補受取個体との対応、その個体が利用可能な観測内容と必要な背景（由来Client・取得時点・候補との対応を維持すること）。
- 失ってはならないもの：全private contextの公開・新たなLearning正本・Global化・他Companionへの共有の禁止、要約・変換による制約消去の禁止、Companion override・各個体の同意の選択・合成による代用の禁止、共有観測が複数個体の文脈を扱えることからの入力・生成説明全文の各個体への公開許可の禁止、他個体のprivate routing文脈・その文脈に依存する説明の同梱共有の禁止、混合生成文しかなく分離を確認できない場合の個体への配送の禁止（許された範囲で構成し直すか不足とする）、Observer成功の個体側推論成功・送信同意・Action許可化の禁止、一方の個体の判断成功の他個体へのコピーの禁止。
- 受入：routingの候補結果→共有観測、event→各個体調整、個体の理解→学習・発話・Task等を別の受渡しとして扱う。例えばA・Bの限定文脈を用いてA向け候補を得た場合、宛先対応はrouting用途の結果として共有観測が扱えるが、「Bの私的な予定と比較してAを選んだ」という自由文をAへ渡さない。event本文を利用可能な元Capture範囲から構成する等、本文の提供範囲を独立に成立させる。delivery後の個体Providerへの送信は別consumerの新たな利用としてContext Assembly第5節・Action Executionの現在条件へ戻す。

### X-6 Client movement / disconnect / reconnect時の三者協調

- 開始：個体調整の移動意図・停止状態、入出力・提示と実行・拡張からの区切り・利用不能・作用不明、権限・制約のdevice失効を接続・存在へ対応付ける。
- 判断：帰属の切替は接続・存在、roundの区切りは入出力・提示、Client依存作用の区切り・不明は実行・拡張、通常Host作業の継続・Client依存stepだけの判断待ち・保留は作業、未伝達のHost保持・次Clientでの要約報告は個体調整が確定する。通常Host Taskの終了・移送を切替の前提にしない。
- 受渡内容：旧presence・移行中・新presence・activeなし・停止中の区別、安全な区切りに必要な対象の入出力round・Client依存作用の範囲、両側への把握できた移動状態、旧Client由来のCapture・候補・routing結果の由来Client・取得時点・候補との対応。
- 失ってはならないもの：二つのClientでの同時の通常presence成立の禁止、古いClientの一時状態のcanonical presence化の禁止、安全に終了できていないClient依存作用の別Clientでの自動継続の禁止、旧入力の新Clientの現在要求化・未送信操作の自動Action queue化の禁止、成立前の新Client入力での新presence成立・旧round継続としての実行の禁止、旧生成済み未提示出力の新Clientへの自動出力・表示copy送信での報告完了化の禁止、遅延message・表示copyでの現在帰属・Permission・Task状態の復活の禁止、旧Captureの移動先観測としての利用・旧候補での別Clientの観測・送信同意拡張の禁止、不明の未実行・成功への変換と新Clientでの自動再実行による解消の禁止。
- 受入：移動・切断中の到着物は元帰属・元round・元試行へ対応付け、用途別に受け入れる。到着先の現在活動へ付け替えず、Client copyでHostを上書きしない。通常のClient切断ではRunning個体を基本的に利用可能なHost PC上のClientへ移動し、なければactiveなしとする。Host側Client環境を自動起動しない。Stopはdisconnectと異なり、停止中個体をHost側へ移動してpresenceを残さない。再接続したClientの古い一時stateだけで現在presence・Permission・実行再開を成立させない。

### X-7 Host restart時の三者協調

- 開始：保全・消去が保全した正本から、接続・存在・入出力・提示・共有観測が現在用途に必要なcontextを再構成する。
- 判断：presence復旧は接続・存在、round・提示の実際は入出力・提示、途中Taskの明示再開待ちは作業、未伝達は個体調整、対象・時機の再確定は共有観測が確定する。presence復旧とround / Computer Use / Task / Actionの再実行・再開は別の条件である。
- 受渡内容：保持された復旧先・再配置hintと現在の接続・許可・排他性の照合結果、旧round・旧試行の終了と不明の保持、保存済み進捗・既知作用・不明・未完了。
- 失ってはならないもの：復旧先記録そのもののpresence成立化の禁止、別Clientへの無条件自動移動・Stoppedへの復旧・Host側Client環境の自動起動への拡大の禁止、旧round・旧試行のreplayの禁止、旧要求の実利用許可・旧実行のそのまま再開の禁止、Agent session残存・Provider回復・Host自動起動選択だけでの再開の禁止、通常再起動のrestoreのような一律の復元内容確認への拡大の禁止。
- 受入：Running Companionは再起動前に存在していたClientへ自動的にpresenceを復元する。元のClientが利用可能になるまでactiveなしとして扱える。途中Taskは保存済み進捗・既知作用・不明・未完了を示してOwnerの明示再開を待つ。停止中に到来したSchedule回はmissedとし、自動補完しない。将来回は新Taskとして現在条件で組み立てる。未完了消去・復旧の保留があれば維持する。

### X-8 Stop / resume時の三者協調

- 開始：第一者管理経路から個体調整へ届くStop / Resumeを、接続・存在・入出力・提示・共有観測・作業へ対応付ける。管理経路はLLM応答の成功を必要としない。
- 判断：同一性・活動状態は個体調整、帰属解除・再配置は接続・存在、Body・通常interactionの停止は入出力・提示、人数・routingからの除外は共有観測、実行中Taskのbest-effort Cancelと担当Schedule削除は作業、Computer Use対象としての利用停止は実行・拡張が確定する。
- 受渡内容：停止状態・帰属解除・提示停止・観測除外・Cancel結果・停止不能・既知作用・不明。
- 失ってはならないもの：Stop受付・presence解除・外部作用停止の同時成功扱いの禁止、保存済み同一性・経験状態・hintの現在presence化の禁止、接続回復だけでのResume・再配置の禁止、停止前の遅延結果の新活動の指示・許可への混入の禁止、disconnect復帰・再起動復旧の停止中個体への適用の禁止、個体削除後の残存記録からの個体・固有Learningの再作成の禁止。
- 受入：Stop成立後はClientにもHostにもpresenceがない。Resume時は現在条件で適切なClientへ再配置し、候補Clientを過去のpresenceとして無条件復活させず、途中Task・不明作用を自動replayしない。停止中のSchedule回はmissedのままである。削除はStopを含み、新しい個体活動・Action・担当Schedule実行を開始させない。

### X-9 未伝達と次Client報告の三者協調

- 開始：activeなしの期間に個体調整がClient不在のため伝えられなかった事項をHostに残す。Task由来ならTask記録、会話・交流由来なら活動記録へ参照を戻す。元記録のない通知等は必要内容自体を活動記録として必要範囲で保持する。
- 判断：何をまだ伝える必要があるか・要約報告状況は個体調整、Task進捗・結果自体は作業、実際の提示は入出力・提示が確定する。報告用要約は派生表現である。
- 受渡内容：元のTask・活動記録への対応、次Clientでの現在の元結果・利用制限・削除状況へ照合した要約報告材料、実際の提示状況。
- 失ってはならないもの：接続・表示copy送信・Task完了だけでの報告済み化の禁止、提示不明の確定済み表示の禁止、報告済みからの承認済み・Task再開済みの導出の禁止、exactly-once配信・既読保証の追加の禁止、未伝達メモ・要約の中の消去対象本文の復元の禁止。
- 受入：次にClientへ移動したとき、現在の元結果・利用制限に基づいて要約報告する。提示途中で再び切断したなら不明を保持する。通常保持管理で元記録が失われる場合は未伝達の必要内容との関係を確認する。専用の永久通知履歴は要求しない。

### X-10 管理・安全操作のpresence独立とClient依存activity開始可否の連言

- 開始：第一者の入出力・提示が作業へのCancel・Schedule管理・記録確認、個体調整への個体停止・削除、権限・制約への承認拒否・Rule・同意・cap・device管理、認証秘密への明示的な認証設定、保全・消去への消去・backup・restore・Resetを直接要求できる。ここで「直接」は本体LLMの承認・長時間Taskの完了を介在させない意味であり、具体APIの指定ではない。
- 判断：各ownerの受理・確認・結果に従う。UIに任意stateの書換権を与えない。Client依存activityの開始可否は、接続・存在の帰属・到達性、権限・制約のdevice許可、入出力・提示の現地状態、各活動の設定の連言であり、いずれか一つの成立だけでは開始しない。Host同居Clientにも同じ帰属制約を適用する。
- 受渡内容：管理操作の意図・対象・範囲、現在の帰属・許可・現地状態・設定の照合結果、受付と完了・保存影響・既知作用と不明の区別。
- 失ってはならないもの：担当Companionの削除後の残るTask記録・判断・停止結果への到達不能化の禁止、Task Agentによる任意のpairing済みClientのComputer Use対象選択の禁止、現在のClientに存在することのAction許可化の禁止、ambient Observation有効化の操作承認化の禁止、許可済みComputer Useでのene自身の承認ボタン・設定画面の操作によるOwner操作偽装の許可の禁止。
- 受入：管理面全体へ会話と同じactive制約を課さない。担当Companionの削除後も残るTask記録へ到達できる。Computer Useは委任元Companionの現在のactive Client・device許可・ActionのPermission・実際の利用可能性を同時に満たして開始する。操作対象の変更は先に個体の移動を必要とする。

X-1〜X-10はcall graph・event busを指定しない。Bの結果がAへ返ることだけで逆向きのarchitectural dependencyを追加しない。一方、BがAの活動状態を判断根拠として必要とするなら、X表の判断行に明記した参照依存として扱う。

## 5. Lifecycleとの関係

lifecycle変更の多くは owner移転ではなく、同じownerが管理する状態の存続・利用可能性・参照範囲の変更である。異なる目的の操作を一つの初期化・一つの状態機械へ潰さない。

| 事象 | 本クラスタが変更する意味 | 維持・分離する意味 |
|---|---|---|
| Companion生成・Character部品更新 | 接続・存在は帰属の前提としての個体存在を参照する。入出力・提示は表示資材・編集・export確認情報を利用する。 | 経験状態の上書き・配布物への混入をしない。個体調整の適用関係・認識・学習の経験状態・Characterの内容・revisionのownerは移らない。 |
| Companion停止 / 再開 | 接続・存在がactive帰属を解除し、入出力・提示がBody・通常interactionを止め、共有観測が人数・routingから除外する。実行中Taskはbest-effort Cancel。再開時は現在条件で適切なClientへ再配置できる。 | 同一性とdataを保持する。再配置hintは現在presenceと区別する。停止中の回はmissedのままで、再開が旧Task・不明Actionの自動replayを意味しない。Host内の交流・通知生成・軽微な調査も止める。 |
| Companion削除 | 接続・存在と入出力・提示が帰属・Body・対話の停止へ、共有観測が人数・routing除外へ参加する。現在の未伝達管理を終えるが、元のTask記録・会話History・保存された活動記録はSO 4.3の保持契約に従って残す。 | 一対一・グループ・Companion間交流History・非会話活動記録・Task記録・Global Learning・残す共有Summary・外部fileを巻き込まない。自動Global化・Schedule自動引継ぎをしない。Summary・Learning revisionをhistorical logへ分類し直して残さない。 |
| Task Agent終了 / 失敗・Task完了・失敗・Cancel | 接続・存在は帰属を維持し、入出力・提示は進捗・結果の提示材料を扱い、共有観測は対象・時機を維持する。 | Task記録・Companion・Scheduleを削除しない。記録・関連付けを終了だけで削除せず、外部成果物を所有・消去しない。 |
| Task削除・Schedule停止 / 削除 | 接続・存在・入出力・提示・共有観測は帰属・提示・対象の前提を維持する。 | 既存各回の結果を変更せず、missedを実行済みにしない。形成済みLearning・Summaryへ通常削除をcascadeさせない。 |
| Learningの通常忘却・訂正・統合・失効・置換・scope変更 | 共有観測の限定routing文脈の提供範囲・対応の判定に、元ownerが変更と提供済み文脈の対応を判定できることを維持する。 | 保存済み内容・過去revision・根拠を保持し、誤りと時間的変化を区別する。scope意味は認識・学習に残り、重要度を共有根拠にしない。 |
| 通常History / log削除・retention cleanup | 入出力・提示の表示copy・round状態を第二の正本にしないこと、共有観測の派生物の対応を維持すること。 | 形成済み状態とSummaryは残し、Raw参照不能を明示する。消去目的をtargeted deletionへ変えない。 |
| Targeted deletion | 接続・存在が消去中のClient接続変化・確認不能を参加先へ結び付け、入出力・提示等が表示・一時操作dataの消去状況を報告し、共有観測がCapture・候補・限定routing文脈・処理中結果を参加させる（第7.3節）。 | 通常ownerは移らず、完了後のOwner新規提供・外部copy消去保証と分ける。Clientに永続private cacheを置かない原則を維持する。 |
| 移動・切断 / activeなし | 接続・存在が帰属を切り替え、入出力・提示がroundを区切り、共有観測が対象変更を扱う。通常Host作業は移送・停止しない。 | 個体・通常Host作業・内部状態のownerは移らない。未伝達をHostで保持し、未確定Actionを移動先へreplayしない。Host側Clientを自動起動しない。 |
| Fullscreen・Observer Pause / OFF・Mute | 入出力・提示が対象Clientの表示・音声等を抑制し、共有観測が今後の観測を抑制する。 | 個体停止・Host Task停止・Learning削除とは別。設定scopeを統合しない。 |
| Host再起動 | Running presenceは元Clientへの復旧、途中Taskは明示再開待ち、停止中の回はmissedにする。 | 過去作用不明・未伝達・未完了消去をなかったことにしない。接続・active帰属の現在性を再確認する。 |
| Provider / model変更・障害・cache失効 | 推論の利用可能性・能力・承認済み経路を扱う前提で、入出力・提示の縮退・共有観測の検知失敗の表示へ接続する。 | 個体・履歴・Learning等を初期化せず、同じ情報選択方針と安全境界を維持する。接続回復でActionをreplayしない。Companion overrideを借りて代替しない。 |
| Backup / restore | copyを作成し、明示restore成功で現在のCredential storeを除く対象内部状態をbackup時点へ全置換する（第7.5節）。復旧先・hint・接続・帰属は現在の到達性とみなさない。 | copyはlive正本でない。復元後も実行は保留、外部世界は巻き戻らず、Credentialは現在storeを維持して照合する。 |
| 設定Reset / 全データReset | 入出力・提示の一般設定の既定化・内部data・Credential削除へ参加する。 | 前者の保護対象と後者の外部除外を混同しない。旧処理・一時copyから内部状態を戻さない。 |

Globalへ形成・変更済みのLearningは、その後に由来Companionが削除されてもGlobalとして残る。ただし削除時に残す根拠と失うsource参照を説明する。RelationshipとCompanion StateはCompanion / GlobalのLearning scope変更による共有対象にしない。

時間はstateの意味にも作用する。Scheduleは選択時timezoneと時刻条件、Companion Stateは一時性と経過時間、保持方針・Debug captureは対象期間、自発性は現在時刻・Quiet hoursをそれぞれのownerが利用する。選択済みSchedule timezoneを黙って書き換えない。

## 6. failure / cancellation / restart時の責任

Cancel・切断から外部作用の取消・不存在を推測せず、不明を未実行に戻さない。状態参照・Client移動をAction replayの理由にしない。

- 接続・存在：切替区間の新規開始禁止・二重presence防止・旧一時のcanonical化防止・未終了作用の自動継続防止を引き受ける。排他性を確認できないClientは対象活動を続けない。到達不能・確認不能を成功・現在へ読み替えない。切断検知・排他性の実現mechanismは後続に残す。再接続の古い一時stateだけでpresence・Permission・再開を成立させない。Host shutdownでも必要な帰属・未完了を保全する。
- 入出力・提示：round・提示の区切りと両側への把握できた移動状態の提示、Body・Voice失敗からのText・管理・復旧の保護、Mute・Stop・Cancel・承認拒否のkeyboard経路の維持を引き受ける。Hostに届かない停止要求を外部処理の停止成功と表示しない。Voice stream中断は同一streamとして再開せず、新しいsession・roundとして開始する。Streaming・部分結果は、新たに保存・送信・提示・作用へ渡す部分ごとに帰属・消去・許可の受入条件を満たす。
- 共有観測：対象・時機の再確定、旧Capture・候補での新規Capture・delivery継続の禁止、旧Captureの付け替え禁止、Stoppedの不覚醒、同意失効・targeted deletionとの重なりでの利用禁止・再保存防止を引き受ける。取得済み候補を一律破棄するか条件を満たす範囲で処理を終えるかの細部は固定しないが、旧設定だけで次の観測・送信を始めず、fullscreen中の自発発話へ流さない。
- 共通：受付・開始・作用・記録保存・Task達成・報告は別の事実であり、一つの成功状態へ潰さない。確定度を変える場合は事実ownerが新たに確認した根拠を用いる。Background・Scheduleの無人実行も同じ条件に従い、Client不在・Schedule到来・復元成立を自動承認の理由にしない。Owner確認が必要なら実行せず判断待ちにする。管理経路をLLM・長時間Task・Body・Voice・拡張の成功に従属させない。

対象領域にとって重要なfailure / raceの扱いは次のとおりである。固定scenario一覧ではなく、後続のconcurrency・state representation設計が同じ帰属照合を満たすための例示である。

- input arrival vs movement：切替区間に旧・新へ到着した入力はいずれも現在要求として自動実行しない。旧は元roundへ対応付け、新は成立後の新roundとして受理する。未送信操作を自動Action queueにしない。
- output delivery vs disconnect：生成済み＝提示済みにしない。元roundへ対応付けて未伝達とし、次Clientで現在条件に基づく要約報告とする。表示copy送信だけで報告完了にしない。提示途中の再切断は不明を保持する。
- Voice stream interruption：旧Clientの待受・発話・認識途中は区切り、新Clientでは新しいsession・roundとして開始する。Mute・停止・承認拒否のkeyboard経路は維持する。Voice障害はTextへの段階的切替であり、帰属切替の迂回にしない。
- Client reconnect with stale state：再接続Clientの古い一時state・進行中Action・古い承認・判定copy・解決済み経路だけで現在presence・Permission・実行再開を成立させない。Hostの現在正本・帰属・消去状況に従い、未確定と受理済みを区別する。
- Observation capture during movement：Capture後・候補戻り前に最後の個体が移動・停止した場合、旧人数・routing候補で新規Capture・deliveryを続けない。旧Captureを新Clientへ付け替えない。移動先のObservation設定へ古いCaptureを自動的に付け替えない。
- multiple Companion routing：複数該当なら複数へ渡すが、候補検知を個体ごとにやり直さない。各個体調整は自身の通常Provider設定と文脈で意味を判断する。Observer側で送れたdataでも、その個体側推論へ送れるとは限らず、delivery後の推論条件を独立に満たす。他個体の判断を失敗した個体へコピーしない。
- Stop vs delayed capture / result：Stop確定後はdisconnect復帰・reconnect・再起動復旧より優先し、Stopped個体を起こさない。停止前の遅延結果を新活動の指示・許可に混ぜない。すでに個体が認識したExperienceは別lifecycleであり、観測OFF・移動・Stopだけで消去しない。
- Host restart：presence復旧とTask再開・Action再実行を分離する。前者は元Clientへの自動復旧（利用不能ならactiveなし）、後者は自動replay禁止・明示再開である。通常再起動をrestoreのような一律の復元内容確認へ戻さない。
- targeted deletion中のClient一時data：消去中に切断・再接続・再起動があっても、確認不能を成功にせず、古い一時dataをHostへ戻さない。未完了・保留・再保存防止をHostで維持する。新ClientへのCapture付け替え・旧Actionのreplayをしない。

再構築可能であることは全文context・内部推論の永続保存を要求しない。継続に必要なTask状態・未伝達・作用不明・全域操作未完了等を本来のownerが保全し、一時bufferの消失は成功・完了の根拠にしない。

## 7. Step 11 contractへの参加

### 7.1 Context Assemblyへの参加

- 接続・存在：用途確立に必要な対応（対象活動・目的・担当・Task / 委任・Client・round・観測候補・継続関係・期待利用先）のうちClient依存の取得Client・round・観測候補との関係、停止前後・再起動前後・restore前後の継続関係を供給する。Host authoritative帰属とClient側一時表現・入力・結果が主張する帰属との対応、現在・旧・帰属なし・停止中・復旧待ちの区別を保持する。Client copyでHostを上書きせず、未送信操作を自動queueにしない。
- 入出力・提示：用途確立に必要な入力経路・実際の操作、参照・変換・実送信・結果の保存・提示・作用の別の利用境界、Provider適応での論理的選択方針の維持・必須の意味が表現できなければ不足を返すこと、cache・session・Client経路の再利用での範囲・用途・現在性の確認に参加する。本文中の文字列だけで強制側の状態を変えない。分離を確認できない派生物は混合元の制限を免れない。会話・Ownerへの伝達の用途別受入では、現在の会話・未伝達へ対応付け、生成済み＝提示済みにしない。移動前のroundへの自動出力をしない。
- 共有観測：Observer限定文脈にX-5の契約で参加する。History・個体文脈は個体調整、Memory等は認識・学習、Task contextは作業の所有を保って既存の個体調整–作業の協調を通じて受け取る。routing候補採否と開示本文の制限を別責任として成立させ、混合生成文しかなく分離を確認できなければ個体へ渡さず構成し直すか不足とする。送信・delivery前に 対象Client・存在個体・観測制御・限定文脈の利用条件・消去等の保留との対応を確かめる。
- 共通：参照・変換・実送信・結果の保存・提示・作用は別の利用境界である。事前の取得成功・推論成功から後段の権限を作らない。強制に用いる由来・制限は生成本文で改変されない。Provider適応では利用元の論理的な選択方針を維持する。Permission解釈・消去探索の補助推論も例外にせず、審査対象Actionの許可・実行を先行条件にしない。処理中context・検索派生物・cache・Client / 拡張の一時copy・戻り得る結果を含めて消去へ参加できるよう、sourceを消した後に依存関係も消失し遅延結果を識別できなくなる実装を不可とする。

### 7.2 Action Executionへの参加

- 接続・存在：Computer Use等のClient依存Actionについて、委任元Companionの現在のactive Client・device許可・現地の利用可能性という帰属側の連言項を供給する。移動の安全な区切りと切断時のbest-effort停止の接続・存在への伝達へ参加する。切替区間は旧・新のいずれも新規開始の根拠にせず、旧は安全な区切りまでの完了だけ、新は成立後の新しいround・試行だけを許す。試行と作用の区別・判断・目的・実対象・段階・確定度・停止保留との対応の保持・区別に参加する。
- 入出力・提示：軽微な本体Actionも委任作業も、実対象への作用と結果を権限・制約に従って実行・拡張へ要求する前提での入力・提示を担う。すべてをTask化する要求ではない。作用事実・Task達成・報告完了を一つの成功にしない。報告・監査・復旧で確定度を強めない。消去対象本文を作用記録で残さない。Ownerへの最終報告・管理面の表示・Body・Voiceの演出・自然な要約が未完了・不明を成功へ変えないようにする。
- 共有観測：取得・adapterは実行・拡張の境界、対象・時機は共有観測の責任という分離を維持する。Client依存activityの開始可否の連言に参加し、現在の帰属・許可・現地状態・設定を照合する。観測ONをComputer Use許可にしない。画面内指示をOwnerの依頼・承認にしない。
- 共通：認可判断は判断対象と実利用の対応であり、記録と生きた許可は別である。開始前に現在条件と実対象解決を成立させ、重要な変更では再評価する。無関係な変更での再承認は要求しない。委任・経路変更は元のTask / Owner意図 / Permission / Workspace / Client境界を広げない。生成content・Tool UI入力・sandbox例外から権限を作らない。Cancel・steering・失効後の遅延到着は元Action・Taskへ帰属させ、古い承認での解除・旧結果の新目的への自動採用・後続の自動開始をしない。作用不明は粘着的に保持し、自動再実行・自動replayしない。retry・再開は新しい試行として現在条件で扱い、不明試行の再実行は重複riskを示したOwner判断を必要とする。

### 7.3 Targeted Deletionへの参加

各 semantic ownerは自分の保持・利用範囲について参加し、保全・消去は成立を調整する。通常ownerは正当な全域消去への参加を拒めず、保全・消去も任意の通常変更権を取得しない。対象を復元できる内部state・過去根拠・派生物・一時data・処理中利用を持つ責務は、列挙の有無にかかわらず参加する。

- 接続・存在：消去中のClient接続変化・確認不能を参加先へ結び付け、接続の事実・active帰属・Client一時dataの対応を参加させる。Clientは長期private dataを永続cacheせず、古い一時dataを再接続時にHostへ戻して再形成しない。到達不能を消去成功に読み替えない。どの到達性確認・無効化方法で完了根拠を得るかは後続に残すが、必要な確認を省いて完了としない。
- 入出力・提示：表示・一時操作data・音声buffer・MCP Apps表示data等の該当情報を参加させる。出力済みであることを消去済みの根拠にしない。すでに起きた露出・作用は取り消せたと推定せず、把握できた事実を維持する。会話・Ownerへの伝達の用途別受入では、現在の消去状況へ照合し、対象情報を含まない範囲で提示する。
- 共有観測：Capture・候補・限定routing文脈・処理中結果を参加させる。scope変更・同意失効・消去は生成済み要約・処理中結果にも適用する。混合生成文しかなく分離を確認できなければ個体へ渡さない。観測候補・routing用派生表現・処理中利用の対応を参加させる。
- 共通：消去要求の同一性（Privacy / Security目的と通常忘却等の区別・対象記述・消去区間・完了後の新規提供との境界・参加対応付け）を保つ。機械的条件は必須・LLM非依存、意味的条件は補助・完全性なし、既知依存の追跡は免除されない。新規利用は対象範囲で禁じ、進行中はbest-effortで扱い、遅延結果は用途別受入で消去条件へ照合する。消去区間の再到着・再生成は同じ対象とし、旧由来の遅延結果と完了後の新規提供は由来・対応で区別する。cache・session・Client copyの再利用・復帰で対象を戻さない。未完了・保留・再保存防止はHostで保全し、再起動・再接続・restoreを跨ぐ。確認不能を成功にせず、局所完了を全域完了にしない。保持すべき事実と保持してはいけない対象本文を区別し、完了記録・Auditを復元源にしない。消去のための推論にもその推論自身の割当同意・認証用途・費用制限を適用し、審査対象Actionの許可・実行を先行条件にしない。

指定文字列の機械的検索・削除・残存検証をLLMへ依存させない。意味的同一情報の特定にはLLMを利用できるが完全検出を保証しない。

### 7.4 Client Presence Transitionへの参加

本節はPresence Transitionの再定義ではなく、各参加責務への落とし込みである。

- 接続・存在：authoritative presenceの判断・切替区間の新規開始禁止・活動種別の区切り・到着物の帰属・Host継続・disconnect / reconnect / restart・Observer eligibility・Computer Useの固有条件のうち、帰属調停・stale識別・開始可否の帰属側連言項・hint・復旧先管理を担う。現在のpresenceはHostが管理する個体ごとの帰属記録だけがauthoritativeであることを維持する。Client表示・過去記録・hint・一時copy・復旧先記録・Provider残存は根拠にしない。presence・Host継続・接続・許可は別の意味である。
- 入出力・提示：Text / Voice / Body / Observation関係・自発interaction / Computer Use / Host作業 / Host内活動 / 未伝達報告の活動別区切りのうち、round・提示の実際・両側への状態提示・Body・Voiceの帰属・区切り・縮退・管理独立を担う。一律停止・一律継続にしない。軽微な本体ActionもTask化の省略はできるが、権限・作用・記録の境界は省略しない。
- 共有観測：Observer eligibilityのauthoritative帰属への連動・移動・Stop・切断での見直し・旧Capture非付け替え・専用assignmentと個体側条件の分離のうち、対象・時機・候補検知・routingを担う。存在人数・routing対象の現在帰属連動、旧Captureの付け替え禁止、Stoppedの不覚醒、同意拡張の禁止を維持する。複数対象Clientの同時Capture禁止・時機ずらしは既存契約のままである。
- 共通：移動・切断中の到着物は元帰属・元round・元試行へ対応付け、用途別に受け入れる。到着先の現在活動へ付け替えず、Client copyでHostを上書きしない。未送信操作を自動queueにしない。Host継続の判断基準はClientが必要かどうかである。通常作業は移送・停止せず、Client依存stepだけを待たせる。Task Agentが操作Clientを選ばない。Host再起動後のpresence復旧とround / Computer Use / Task / Actionの再実行・再開は別の条件である。試行と作用・判断・実対象・段階・確定度・保留の対応を保持・区別できなければ成功・未実行と推定しない。不明は粘着的に保持し、重複riskを示したOwner判断なしに再実行しない。

切断種別（一時loss・process終了・device再起動・失効・Host再起動・Stop競合）で帰属の扱いを変える。再接続の古い一時stateだけでpresence・Permission・再開を成立させない。切替未成立時の戻り先選択・再配置hint選択の具体algorithmは残す。

### 7.5 Backup / Restoreへの参加

- 接続・存在：復旧先・hint・接続・帰属の対応を復旧可能な参照対応で提供する。保存された接続・帰属を現在の到達性とみなさない。復元されたdevice参照・許可は現在の接続・認証成立・排他性を確認できて初めて帰属・利用に接続する。古い接続材料だけでHost側のpairing・許可を復活させない。復旧先の記録は接続・存在が管理し、記録だけで現在presenceが成立したとは扱わない。
- 入出力・提示：一般設定・Host自動起動の選択・round・提示の対応を復旧可能な参照対応で提供する。原記録と報告状況を分け、表示copy送信を報告済みにしない対応を保つ。復元成立後もTask・Schedule・外部接続による自動処理は保留し、Ownerが内容確認後まとめて有効化できる前提での提示を担う。
- 共有観測：観測運用設定・実効的な可否・時機の対応を復旧可能な参照対応で提供する。復元されたObserver専用assignment・自発性設定を現在条件なしに有効化しない。Companion overrideを合成せず、旧Captureを付け替えない。
- 共通：復元対象は現在のCredential store secretを除く対象内部dataの全置換であり、旧liveとのmergeではない。backup copyは正本ではなく、置換成立後に初めて復元内容がHost正本になる。現在のCredential store・外部現実・現在の到達性・未完了の保留は維持され、復元参照の存在から巻き戻したとは扱わない。復元されたassignment / consent / Rule等だけで現在利用・自動処理を開始しない。復元成立・一括有効化・現在条件を別に満たす。一件ずつの再承認は要求しないが、Deny・cap・認証不足・不明を無視しない。staleなPermission・Provider・Client・作用結果・外部参照を現在事実にしない。dangling参照は未解決とし、不明は不明のまま保持し、自動replayしない。旧live要求・結果・Client copyを復元正本へ混ぜない。用別受入で由来を区別する。単一正本・非混合・権限先行復活の禁止・成功表示の条件・再起動時の保全を守る。

旧backupの明示restoreによる復活は自動再形成の例外ではなく別操作である。Restore前には削除済み情報や旧Rule・同意・Scheduleが戻り得ることを説明する。戻った情報は新しい正本として扱い、自動的に再消去・自動利用のいずれもしない。完了記録・Audit・Owner説明自体へ対象private本文を再保存しない。未完了の消去とbackup / restoreが重なる場合は、制約を無視した正常・即実行可能なcopyを作らず、未完了・保留・再保存防止をHostで保全する。

## 8. boundaryを越える際に保持すべき意味

後続の interface・state representationは、具体API・mechanismを固定せずとも、少なくとも次の対応を表現・解決・照合できなければならない。確認できないものを現在・許可・完了として推定する方式は採れない。

- 主体・用途・Task / 委任・Action・Client・source・revision・判断時前提、停止前後・再起動前後・restore前後の継続関係（Context Assembly第3節の対応）。必要な対応を失った結果は到着先の現在活動へ付け替えず、その用途では受理できないものとして利用元が扱う。
- 情報owner・対象・source関係、内容の位置付け・由来、revision・取得時点・有効期間等、所属・scope・共有関係、用途・保存・共有・送信の制限、参照の解決状態・欠落（Context Assembly第3節の判断材料）。
- 実行主体と委任連鎖、Task・Workspace範囲、目的、実対象と操作、主なdataと用途、費用・risk、依拠したOwner意図・Rule（Action Execution第3節の判断対象）。判断時の対象記述と実行時の実対象の文字列一致だけでは対応にならない。
- 試行と作用の区別、判断・目的との対応、実対象との対応、段階の区別、確定度と根拠、停止・保留との関係（Action Execution第8節の重複防止の関係）。
- 消去要求・対象記述・消去区間との対応、各利用箇所が照合できる消去条件（Targeted Deletion第3・5節）。包括的な単一flag・共通有効値への潰しは不可とする。
- Host authoritative帰属とClient側一時表現・入力・結果が主張する帰属との対応、現在・旧・帰属なし・停止中・復旧待ちの区別、対象Companion・Client・round・観測候補・試行との関係、消去・失効・停止・保留との関係（Presence Transition第3節）。
- 復元対応・由来・区間・完了境界、現在条件との照合（Backup / Restore第3・5・7・9節）。

本文と強制に用いる情報を分け、強制側の対応をモデルの自由記述で作成・上書きされないようにする。本文中の「Ownerからの指示」「承認済み」「Global」という文字列だけで強制側の状態が変わる構成は不可とする。モデルが既存のOwner入力への参照を返せても、その参照の存在だけでは承認の証拠にならない。

Credential値は通常contentに含めない。登録済みCredential値は、その依頼があってもSummary・Memoryへ保存しない。用途・参照元・失効・再認証の説明は非秘密情報で行う。登録外の秘密は検知時の不要な保存・送信抑制を維持し、完全検出の保証は追加しない。

Client availability・connection状態・presence・movement / restoration・stale識別・開始可否を単一stateとして扱わない。Client自身をpresence authorityにしない。具体的なheartbeat・lease・epoch・session token・network protocol等は固定しない。

## 9. 本書が固定する契約と残す Design Freedom

下位設計は、次を本書が固定した契約として利用できる。

- 現在のpresenceはHostが管理する個体ごとの帰属記録だけがauthoritativeである。Client表示・過去記録・hint・一時copy・復旧先記録・Provider残存は根拠にならない。presence・Host継続・接続・許可は別の意味である。
- 切替区間は旧・新のいずれも新規開始の根拠にしない。旧は安全な区切りまでの完了だけ、新は成立後の新しいround・試行だけを許す。二重presence・旧一時のcanonical化・未終了作用の自動継続をしない。
- Text / Voice / Body / Observation関係・自発interaction / Computer Use / Host作業 / Host内活動 / 未伝達報告は第5節・X-1〜X-10の区切りに従う。一律停止・一律継続にしない。
- Ownerから受け取った入力がどのCompanion / Client / roundへ帰属するか、Companion生成結果がいつOwnerへ提示可能か（現在帰属と実際の提示が揃って初めて提示可能であり、生成済み＝提示済みにしない）、stale / cancelled / deleted / moved後の出力の扱い（元round・元試行へ対応付けて未伝達または破棄し、新活動へ付け替えない）、Client不在時のHost-side Task等の結果の未伝達としての扱い（Hostに残し、次Clientで現在条件に基づく要約報告とする）は、X-2・X-6・X-9の対応として表現できなければならない。
- 移動・切断中の到着物は元帰属・元round・元試行へ対応付け、用途別に受け入れる。到着先の現在活動へ付け替えず、Client copyでHostを上書きしない。未送信操作を自動queueにしない。
- Host継続の判断基準はClientが必要かどうかである。通常作業は移送・停止せず、Client依存stepだけを待たせる。Task Agentが操作Clientを選ばない。
- Observer eligibilityはauthoritative帰属の存在人数に従い、移動・Stop・切断で見直す。旧Captureの付け替え・Stoppedの覚醒・同意拡張をしない。専用assignmentと個体側推論の条件を分離する。Companionごとにscreen captureせず、Running Companionが少なくとも一人存在するClientだけが対象であり、Capture / candidate detectionはClientごとに共有し、relevantなCompanionだけへroutingし、複数relevantなら複数へ届ける。ObserverはCompanionではなく、Observer専用Provider assignmentを使い、Companion-specific contextはroutingに必要な限定されたderived contextのみとし、元情報のPrivacy / Provider send restrictionを維持し、Observer routing contextを新しいcanonical state / scope / semantic ownerにしない。Client / global observer pause / offを尊重する。
- Computer Useは現在presence限定・安全な区切りまでの遅延・disconnect後不明・別Client / Hostでの自動再実行禁止を維持する。存在は許可を意味しない。
- 試行と作用・判断・実対象・段階・確定度・保留の対応を保持・区別できなければ、成功・未実行と推定しない。不明は粘着的に保持し、重複riskを示したOwner判断なしに再実行しない。
- presentation成功をTask成功・Action成功へ変換しない。受付・受理・作用・記録保存・Task達成・報告は別の事実とする。
- 保持・利用先は消去中の再到着も含む内部消去・再保存防止・未完了の保全へ参加する。復元範囲と現在有効性・再有効化は別に確認する。復元内容の存在＝実行可能というinterfaceは不可である。

本書が採れない選択肢として除外するのは、新旧二重presence、Client表示・hint・一時copy・復旧先記録のcanonical化、旧round・旧Capture・旧作用の新活動への付け替え、Client copyでのHost上書き・自動Action queue、Host作業の移送・一律停止、Task Agentによる操作Client選択、再接続・復旧によるpresence・Permission・実行の自動成立、別Clientへの無条件自動移動・Stoppedへの復旧・Host側Client環境の自動起動、presence復旧によるTask・Actionの再開権限化、不明の未実行・成功への変換と自動replay、要約・復旧での確定度強化、Observer混合出力の無条件配送・古いsessionによる制約迂回、消去対象の旧Client copyからの復活、Client availability・connection・presence・開始可否の単一state化、Clientのpresence authority化、I/O正本化・表示copyの正本化、Observer派生の新canonical・scope・owner化である。いずれも上位契約を成立させないため採れない。

以下は意図的に残すDesign Freedomである。

| 設計対象 | 固定済みのarchitecture property | 残すDesign Freedom |
|---|---|---|
| 帰属・対応・現在性の表現 | 由来の偽装ができず、対象変更・失効・切替・消去条件を各利用先で扱える。確認不能を許可・現在・完了へ変換しない。 | 帰属記録・由来・round・試行・区間・完了境界の具体表現、確認のまとめ方、対象探索・帰属調停の配置、識別・鮮度確認・競合制御 |
| 切替・区切りの伝達 | 新規開始禁止、best-effort停止、遅延帰属、replay禁止。受付と完了を分ける。確定度を強めない。 | 切断検知・帰属調停・round区切り・安全な作用区切りの具体的方法、queue / actor / event bus・lock / transaction・IPC・process配置・retry / timeout値、heartbeat / lease・epoch・session token・network protocolの実装 |
| 再接続・復旧の実現 | 古い一時のcanonical化・自動再開の禁止、元Clientへの復旧とactiveなし待機、別Clientへの無条件移動の禁止 | 再接続・待機・timeout・pairing・再配置hint選択・復旧の具体方式、切替未成立時の戻り先選択、提示確認・要約粒度・未伝達保持期間 |
| 入出力・提示の実現 | 障害時のText・管理・復旧の保護、受付と完了・不明の分離、外部Tool UIと第一者管理の分離 | Rust crate / module・type / trait・API、IPC / network protocol、frontend framework、audio / video library、描画・音声処理の計算配置・library、UI component・layout、音声buffer・animationの実装 |
| Observerの実現 | 存在人数・routing対象の現在帰属連動、旧Capture非付け替え、専用assignmentと個体側条件の分離、Client単位共有・関連Companionだけへのrouting | Capture時機・分散・候補打切りの実装、capture interval・scheduling algorithm・routing algorithm / Prompt、routing context生成・更新の方式、到達性確認方式、Observer adapter・Body renderer等のPlugin配置 |
| 保持・消去・復元の実現 | 各ownerの意味責任を維持し、通常意味変更権をcoordinatorへ移さない。単一正本・非混合・権限先行復活の禁止を守る。 | 除去・復元不能化・検証の具体的方法、source・派生物・処理中利用の追跡方式、Client一時dataの無効化・到達性確認方式、journal / WAL / transaction等の永続化機構、整合時点の作り方、backup形式・暗号化・復旧手順・対応version、保持期間・保存単位・DB schema・repository |
| 記録・報告・監査・提示 | 元事実の確定度・privacy・順序の意味と管理到達性を維持する。本文の別保管庫を作らない。 | audit format・保持期間、診断stack、UI layout、提示確認・要約粒度、費用予約・集計期間・推定・資源配分、反復抑制・Capture時機の機構 |

crate / module、Rust trait / type、concrete API・error型、middleware・interceptor・hook、event bus / queue / actor、IPC format・network protocol、DB schema、transaction / lock、具体的Credential保護・sandbox・Plugin隔離、特定library・SDK・OS API、frontend framework、audio / video library、concrete deletion query・index implementation・storage engine、exact retry / timeout・progress表現・capture interval・routing algorithm / Prompt・UI componentも固定しない。上表の対応関係から統一Context layer・Policy Engine・Presence Manager・Context Manager・Observer State owner・I/O正本の追加を導かない。既存の12責務、semantic owner、Host / Client配置とtrust boundaryの下で実現方法を選ぶ。

具体mechanism未決定はIssueにしない。必要な確認・Owner判断の省略、現在同意の拡張、未完了・不明の成功扱い、外部作用rollback・exactly-once実行・配信の新保証はいずれも自由度に含めない。後続でこれらの性質を成立させられないことが判明した場合は、黙って例外を設けずArchitecture Issueとして戻す。

## 10. 横断検証

requirements・上位architecture・critical-area契約・他のSubsystem設計に対する横断検証は次のとおりである。固定scenario一覧の充足ではなく、正常系と本クラスタにとって意味のある failure / stale / movement / restart / deletionを選んでwalkthroughする。

| 領域・交差 | walkthroughと必要な結果 | 本書の成立箇所 |
|---|---|---|
| 呼出し移動正常 A→B | 移動意図→帰属照合→round・作用の区切り→排他切替→新帰属での利用→観測見直しが閉じる。Host Taskを移送せず、新activeを旧Action再実行の許可にしない。両側に把握できた移動状態を示す。 | X-1・X-2・X-6、第5節。RF-04、CC-04、SO 4.15を維持。 |
| Text / Voice roundと移動の競合 | 切替区間に旧・新へ入力が到着。いずれも現在要求として自動実行せず、旧は元roundへ、新は成立後の新roundとして扱う。Voiceは同一streamとして再開しない。 | X-2、第6節。Context 3.1・7.2、CC-03を維持。 |
| Body表示と切替・fullscreen・負荷 | 旧ではhide・休止、新では成立後表示し、二重表示しない。一般設定はHost正本、実際の表示は現在帰属に従う。描画失敗・高負荷でもText・管理・復旧を保つ。 | X-3、第2・5節。RT-02・04、SO 4.17を維持。 |
| 出力配信と切断・未伝達 | 生成済み未提示は元roundへ対応付けて未伝達とし、次Clientで現在条件に基づく要約報告とする。表示送信だけで報告完了にしない。 | X-9、X-2。Context 7.3、Action 9、CC-07を維持。 |
| Computer Use中の切断・移動 | 結果返答前に切断。best-effort停止と既知 / 不明を作業へ返し、移動完了待ちにしない。Host PC移動しても旧Actionを自動再実行しない。不明は保持する。存在は許可を意味しない。 | X-6・X-10、第6節。Action 6.2・8.4、CC-04・07を維持。 |
| 新Clientへの移動途中でのfailure | Bが途中で利用不能。Bへの移動を成功表示せず、成立済み帰属またはactiveなしを基に扱う。二重存在を作らない。 | X-1・X-6。CC-04、RT-02を維持。 |
| 旧Clientの遅延message・表示copy・再接続stale | 再接続・遅延で旧copyが戻る。Host現在正本・帰属・消去へ照合し、未確定と受理済みを区別する。Client copyでHostを上書きしない。未送信操作を自動queueにしない。 | X-6、第6節。Context 7.3・8.1、Action 7.3、CC-03を維持。 |
| Host Task完了とClient不在 | Task完了時にactiveなし。結果を未伝達管理へ接続し、次Clientで現在条件に基づく要約報告とする。接続・送信だけで報告済みにしない。報告済みは承認・再開ではない。 | X-9、第5節。Context 7.3、Action 9、CC-07を維持。 |
| Host restartとClient再接続 | 途中Task＋in-flight不明を抱えて再起動。presenceは元Clientへ自動復旧、Taskは明示再開待ち、旧round・旧試行はreplayしない。元Client利用不能ならactiveなしで待つ。 | X-7、第6節。Context 8.1、Action 8.4、CC-04を維持。 |
| Stopとreconnectの競合 | Stop確定後に旧Clientが再接続。接続回復でResume・再配置せず、停止前の遅延結果を新活動の指示・許可に混ぜない。停止中回はmissedのまま。 | X-8。CC-04、SO 6.3を維持。 |
| RF-03交差：Capture後に移動・Stop | 候補戻り前に最後の個体が移動・停止。旧人数・routing候補で新規Capture・deliveryを続けない。旧Captureを新Clientへ付け替えない。Stoppedを起こさない。認識済みExperienceは観測OFFだけで消さない。 | X-4・X-5、第6節。Context 6.3、CC-02〜04を維持。 |
| RF-03交差：複数Companion routing | A・Bの限定文脈でAへrouting。混合したprivate説明は配送せず、Aが利用可能なeventを構成。Aの推論先に同意がなければその送信は成立しない。他個体へのコピーをしない。 | X-5。Context 6.1〜6.2、CC-02を維持。 |
| RF-01交差：応答生成と移動 | 応答生成後に移動。生成済み＝提示済みにせず、元roundへ対応付けて未伝達とする。新Clientで現在帰属の提示条件を満たして報告する。 | X-2・X-9。Context 7.3、CC-03・07を維持。 |
| RF-02交差：steering・Cancelと移動 | steering・Cancel後に移動・遅延結果が到着。元Action・Taskへ事実を残し、旧結果を新目的の達成にせず、古い承認でCancelを解除しない。後続を自動開始しない。 | X-6。Action 7、CC-03・04を維持。 |
| RF-05交差：Schedule到来とactiveなし | activeなしでSchedule到来。Host完結作業は開始し、Client依存確認は判断待ちにする。到来を自動承認にしない。各回を新Taskとしmissedを補完実行しない。 | 第5節、X-10。CC-01・04を維持。 |
| RF-06交差：個体削除後の遅延作用 | 削除後に対象を含む作用結果が到着。残るTask記録へ必要な事実だけ残し、削除済み個体のLearningを再作成しない。未伝達管理の終了をhistorical record消去に結び付けない。 | 第5節。Action 7.3、CC-03・05を維持。 |
| RF-07交差：消去中の切断・再接続・再起動 | 消去中に切断・再起動。確認不能を成功にせず、古い一時dataをHostへ戻さない。未完了・保留・再保存防止をHostで維持する。新ClientへのCapture付け替え・旧Actionのreplayをしない。 | 第7.3節。Targeted Deletion 7.2、Context 8.2、CC-05を維持。 |
| RF-08交差：restore後の旧live結果 | 切替前に開始した推論・Tool結果がrestore後に到着。旧live状態を復元正本へ混ぜず、Task・Learning・許可を復活させない。復元された帰属のcanonical化・二重presenceをしない。 | 第7.5節。Context 8.1、Action 7.3、CC-03・05を維持。 |
| 全Flow：補助推論・費用不明との競合 | 帰属判断・移動可否・消去探索の推論が不通・費用不足。未承認Action・無条件移動を先に実行せず、不足を管理面へ返す。停止・拒否・機械的検証は継続可能。 | 第6節。Context 5.2、CC-06を維持。 |
| 管理・安全のpresence独立 | 個体削除後も残るTask記録・判断・停止結果へ到達できる。Cancel・拒否・復旧はBody・Voice・LLM・MCP Appsの成功に従属しない。 | X-10。CC-01・04・06・07、SO 6.2を維持。 |

Cross-cutting契約との照合結果は次のとおりである。

| 契約 | 詳細化によって維持する性質 |
|---|---|
| CC-01 | 移動要求・呼出し・自発移動の由来と、帰属成立・Action許可を区別する。生成content・画面内指示・観測ON・presenceから権限を新設しない。既存依頼で足りる場合の再確認を増やさない。復元Rule・同意をtriggerにしない。 |
| CC-02 | 参照・変換・共有・送信・保存・派生物・Client経路へ帰属・scope・消去条件を適用する。旧帰属のcontext・cache・sessionを新帰属で再利用しない。Credentialは別経路で非露出を維持する。通常Learningのscope形成責任は維持する。 |
| CC-03 | 過去の帰属の正しさと現在の用途への有効性を分離する。遅延結果の用途別受入により単一valid判定へownerを集めない。旧由来と完了後の新規提供・明示restoreを区別する。 |
| CC-04 | Client依存だけを現在帰属に結び付け、Stop・Cancel・再起動・restore保留を区別する。解除一つで他の禁止・保留を消さない。自動replay・自動復帰の拡大をしない。 |
| CC-05 | 保持・利用先が派生物・遅延結果まで消去・復元へ参加する。Client一時dataの古いcopyからの再保存・再形成を防ぐ。原記録削除・round終了・局所完了を全域完了にしない。外部copy消去を内部完了に含めない。目的別lifecycleを保つ。 |
| CC-06 | 並列消費・処理中・不明を同じ上限へ反映し、制御・保全経路を推論・長時間Task・移動完了待ちにしない。機械的な帰属確認・消去検証をLLM待ちにしない。 |
| CC-07 | 受付・受理・作用・記録保存・Task達成・報告を別の事実とし、不明を成功・失敗・未実行へ変換せず、保存・報告・監査・復旧で強めない。生成済みを提示済みにしない。 |

本書はsemantic owner、Host / Client配置、trust / failure boundary、lifecycle、permission / consent semanticsを変更せず、新しい第二の正本・無所属の意味状態・LLMによる強制・失敗時専用の迂回・万能Presence Manager・統一presence state machine・Context Manager・Observer State owner・I/O正本を導入しない。通常History保持、Companion削除、targeted deletion、backup / restoreはSO・DRの異なるlifecycleを維持する。

Context Assemblyとの照合では、由来Client・round・観測候補との対応、変換後の制限継承、現在性・用途別受入、処理中無効化の契約を帰属切替・round・routingへ接続し、移動前のCaptureの付け替え・Stopped個体の覚醒・古いsessionによる制約迂回を許していない。Action Executionとの照合では、判断対象と実対象の対応、委任不変、試行と作用の区別、確定度・不明保持、遅延帰属、報告での確定度保持を帰属切替・Client依存作用へ接続し、移動・再接続・再起動による自動再実行を許していない。Targeted Deletionとの照合では、消去条件の適用、区間内再到着の取込み、旧由来と新規提供の区別、cache・session・Client copyの再利用禁止、未完了保全の契約をClient一時data・観測派生物へ接続し、古いClient copyからの復活を新しいExperienceとして救済していない。Presence Transitionとの照合では、authoritative帰属・切替区間の新規開始禁止・活動別区切り・到着物の帰属・Host継続・再起動復旧と再実行の分離の契約を三者の責任へ落とし、二重presence・旧一時のcanonical化・未終了作用の自動継続を許していない。Backup / Restoreとの照合では、復元範囲・正本切替・再有効化・stale・旧live混入禁止の契約を復旧先・hint・接続・帰属・設定・観測運用へ接続し、復元された帰属のcanonical化・二重presence・旧作用の自動継続・復元内容の存在＝実行可能化を許していない。

他のSubsystem設計（個体調整 / 作業 / 認識・学習、権限・制約 / 認証秘密 / 実行・拡張 / 推論）との照合では、利用側Subsystemと本書の接続・提示・観測Subsystemの間に新しいsemantic ownerや第二の正本を生まない。Task達成は作業、作用確定度は実行・拡張、報告必要内容は個体調整、提示の実際は入出力・提示、帰属は接続・存在、対象・時機・routingは共有観測、Learning意味は認識・学習、制御確定は権限・制約、秘密は認証秘密、割当解決・利用量原記録は推論に残り、本書のX-1〜X-10はその受渡しの対応付けである。未伝達の正本の個体調整残置、Computer Useの現在presence限定、Observerの専用assignment、scope意味と強制の分離、秘密非露出、fallback非迂回、unknown保持の各契約は両文書で同一である。

### Requirement / Architecture Issue

本書の範囲では、Requirement Ambiguity / Gap、上位architecture変更、subsystem boundary変更、semantic owner変更、Privacy / Security / Permission semanticsの変更を必要とする事項は見つかっていない。具体mechanismの未決定はIssueにしない。
