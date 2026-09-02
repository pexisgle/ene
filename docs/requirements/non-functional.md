# 非機能要件

状態: **Baseline / 対話による初版確定**
最終確認: 2026-09-03

ここでは、Eneが常駐型AITuber / AI companionとして継続利用できるための品質条件を定義する。数値化できていない項目は未確定として残し、実測結果や対象Hardwareを確認してから予算・閾値を決める。Rust、特定DB、特定IPC、特定モデルは非機能要件そのものではない。

## 1. 優先順位と軽量性

### NFR-PRIO-001 軽量性の優先

常駐して使うEneでは、**軽量であることを最優先級の非機能要件**とする。高度な機能を追加する場合も、通常のPC作業を妨げる常時負荷を前提としてはならない。

### NFR-RES-001 余剰資源の利用

Ene Core、Scheduler、Memory、Permission、Task管理、Plugin管理、Harness orchestration等の常駐処理は、通常のブラウジング、文書作成、開発作業等でユーザーが性能低下を意識しない程度の資源使用を目標とする。通常時はHostの余剰CPU、GPU、Memory等を利用し、ユーザーの作業と競合する場合は重要度の低い処理を抑制する。

### NFR-RES-002 重い処理の分離

VRM / 将来のLive2D描画、ユーザーが選択したLocal LLM / VLM / STT / TTS等のProvider、第三者Pluginの推論負荷は、Ene Coreの軽量性と分けて評価できなければならない。重いProviderを選択したことだけでCoreの設計を重いものにしてはならない。

### NFR-RES-003 アイドル時の負荷

ユーザー入力、重要Event、実行中Taskがないアイドル時、EneはMain LLMを実行せず、Context Monitor、描画、Scheduler、Persistence等も可能な範囲で低負荷にしなければならない。待機のためのLLM pollingを行わない。

### NFR-RES-004 高負荷時の抑制

ゲーム、レンダリング、ビルド、データ処理等でHostが高負荷になった場合、非緊急のContext処理、Learning、Consolidation、低優先度の自律Taskを中断・延期・分割できなければならない。緊急の通知、承認待ち、ユーザー対話等の扱いは優先度を持つ。

### NFR-RES-005 CoreとProviderの観測

性能計測では、Ene Core、Desktop Body、Context Monitor、Provider、第三者Pluginを区別して負荷を観測できなければならない。具体的なCPU、GPU、RAM、Storageの予算値は未確定である。

## 2. レイテンシ、検索品質、スケジューリング

### NFR-LAT-001 日常対話の応答性

音声またはテキスト入力からCompanionの反応が返るまで、常駐型会話として自然に感じられる応答性を目標とする。Local / Cloud Provider、STT / TTS、Network、モデル性能による差をユーザーへ適切に示す。具体的な目標値は未確定である。

### NFR-LAT-002 Main LLMの節約

Context Monitor、Memory Retrieval、Learning Review、Consolidation、Semantic Stateの基本変換、Sub-agent待機、Schedule待機により、必要以上に高価・高性能なMain LLMを起動してはならない。

### NFR-LAT-003 イベント駆動

時間待ちや外部状態待ちを、LLMターンの反復や短い間隔のpollingで実現してはならない。RuntimeのEvent、Schedule、状態キューによって必要な時だけ処理する。

### NFR-LAT-004 将来のfull-duplex余地

直近Betaでfull-duplex / barge-inを提供しなくても、後続実装で入力音声と出力音声の同時処理、発話中断、自己音声抑制等を追加するために、会話Coreを全面的に再設計することを前提にしない。具体的な音声プロトコルは未確定である。

### NFR-RET-001 Memory Retrievalの品質

Memoryが長期的に増加しても、関連するMemoryを高い精度で想起できることを重要な品質目標とする。検索速度だけを優先して関連Memoryの取りこぼしや無関係なMemoryの混入を恒常的に許容してはならない。自動想起とDeep Retrievalについて、代表的な会話・時間指定・Entity・Companion scope・曖昧検索等の評価セットを用意し、Precision / Recallその他の具体的な品質目標は実測に基づいて定量化する。

## 3. 信頼性、永続性、復旧

### NFR-REL-001 通常のソフトウェア水準の復旧性

Core、Host、Provider、Pluginの停止・クラッシュ・再起動に対して、一般的なデスクトップソフトウェアとして合理的な範囲で、確定済みの会話、Memory、設定、Permission、Schedule、主要ログ、Taskの最終状態を破損・不必要な消失から保護しなければならない。

### NFR-REL-002 再起動後の未完了状態

Host停止中に到来したSchedule、永続化済みの承認待ち、再起動前に実行中だったTaskは、再起動後に見落とさず状態を表示しなければならない。実行中Taskは `interrupted` または状態不明とし、外部変更を自動Replayせず、現在状態を確認した後に手動Retryできるようにする。未確定のLearning Review / Consolidation作業は耐久キューへ保存しなくてもよく、確定済みの元情報から必要に応じて再計算できればよい。

### NFR-REL-003 部分障害の隔離

STT、TTS、Main LLM、VLM、Embedding、Network、MCP、Plugin、Sub-agentの単一障害で、Core、他のCompanion、他のProvider、永続データ全体を不必要に停止させてはならない。

### NFR-REL-004 正確な状態表示

完了、失敗、中断、保留、承認待ち、Provider利用不能を区別して表示・記録し、失敗した処理を成功として扱ってはならない。

### NFR-REL-005 復旧要件の範囲

Eneは、完全なイベントソーシング、全操作のクラッシュリプレイ、実行中キューの完全耐久化、無停止の自動Failoverを必須としない。通常のトランザクション、確定状態の保存、主要イベントの監査ログによる軽量な復旧を基本とする。自動Provider fallbackは通常挙動の要件としない。

## 4. Local-first、Privacy、Security

### NFR-PRIV-001 Hostを正本とする

Eneの永続的なユーザーデータとCompanion StateはHost PCを正本としなければならない。Cloud Provider、Remote Client、第三者Pluginは、Eneの状態の唯一の保存先になってはならない。

### NFR-PRIV-002 送信最小化

Cloud Providerへ送信する情報は、許可された目的に必要な範囲に限定できなければならない。ローカルで処理可能であることと、Cloudへ送信してよいことを同一視しない。

### NFR-PRIV-003 観測範囲の可視性

ユーザーは、どのアプリ、Window、Directory、情報種別、Client、Provider、Pluginが観測・処理・保存・送信の対象になっているかを、必要な詳細度で確認・変更できなければならない。

### NFR-PRIV-004 保存量の抑制

監査に必要な主要イベントは追記中心で残す一方、LLMの逐語的な内部推論、常時観測のRaw画面・音声、意味のない低レベルイベントを既定で無制限に保存してはならない。ログが過大になった場合は保持期間・容量Policyに従い、古いSegment単位でローテーション・削除できるようにする。個別Eventの任意編集・削除を通常機能としない。保持期間、容量、暗号化、Export方式は未確定である。

### NFR-PRIV-005 削除意図の維持

「忘れる」はMemory Systemだけへ適用し、Conversation History、Task履歴、監査ログ等を削除しない。ログは引き続き履歴として参照できるが、その参照や通常のLearning Review / Consolidationによって削除済みMemoryを自動的に復活させてはならない。Memory本文、Embedding、検索Index、派生Cacheを削除し、再学習を防ぐ最小情報だけを保持できるようにする。ユーザーの明示的な再記憶要求は許容する。

### NFR-PRIV-006 App Data Directoryの可搬性

Eneが管理するローカル永続データは、単一のApp Data Directory配下で自己完結させ、そのDirectoryの移動・複製だけでローカル状態を復元できることを目標とする。専用のMigration Toolは必須としない。OS保護Credential、外部サービス側の状態、OS固有Integration等、再認証・再設定を要する例外は明示しなければならない。

### NFR-SEC-001 機械的な権限強制

LLMのプロンプト、Memory、Skill、Character Package、Pluginの記述だけで、機械的に拒否されたFilesystem、Credential、Network、Device、Cloud Egress、Computer Useへアクセスできてはならない。

### NFR-SEC-002 第三者Pluginの隔離

第三者Pluginの障害、未許可アクセス、悪意ある入力が、Core、他Plugin、他Companion、ユーザーデータの完全性を直接損なわないよう、原則として隔離・Broker・Permission境界を適用しなければならない。

### NFR-SEC-003 高帯域処理の安全な性能

Plugin隔離とPermission強制は、Local LLM / VLM、画像、音声、Embedding等の高帯域処理へ不要なコピー・直列化・中継遅延を強制しない。隔離されたProvider Pluginが、許可されたGPU / Acceleratorや必要なRuntimeを自身の実行環境から直接利用できることを妨げず、安全性と性能を両立できる通信方式を選べること。

### NFR-SEC-004 将来のRemote Client保護

将来Remote Clientを提供する場合、ClientとHostの接続は、ユーザーが意図したDeviceであることを確認でき、通信内容とCredential等を第三者から保護できなければならない。認証方式、暗号化方式、Device trust、鍵管理、失効方法は未確定である。

### NFR-SEC-005 Hard Denyの優先

システム・ユーザーデータの壊滅的破壊、Credentialの探索・窃取・流出、権限昇格・不正な永続化、安全制御の無効化・回避、拒否回避、制御不能な再帰・資源枯渇等のHard Denyは、明示Allow、「すべてAllow」、自然言語Policy、Main LLM、Approval Reviewerの判断より常に優先しなければならない。

### NFR-SEC-006 外部DataのInstruction隔離

Web、MCP、Tool、外部Resource、文書から得た内容を、接続・Install済みであることだけを理由にSystem / User InstructionやPermissionへ昇格させてはならない。外部Dataのprovenanceを保ち、そこに含まれる命令が権限・安全制御を変更できない構造にする。Prompt Injection検出の精度だけへ安全性を依存させてはならない。

### NFR-SEC-007 Approval Reviewerの独立性

Approval ReviewerはMain Companionと別のLLM / Sessionとして動作し、判断に必要な最小限の信頼済みPolicyと構造化Actionだけを受け取らなければならない。Main CompanionのPersonality、未整理の会話全文、隠れた推論、外部Data中の命令によって判断境界が変化してはならない。Reviewer障害・Timeout・判定不能時に自動Allowしてはならない。

## 5. Providerと費用

### NFR-PROV-001 構成の独立性

Main LLM、Context Monitor、Sub-agent、VLM、Embedding、Reranker、STT、TTS等は、利用可能なLocal ProviderまたはCloud Provider等をコンポーネントごとに独立して選択・交換できなければならない。一つのProviderの採用が、他の全Providerの実行場所を強制してはならない。システム全体としてLocalとCloudの混在を許容し、個別ProviderのHybrid実行は必須としない。

### NFR-PROV-002 課金への明示的同意

有料または従量課金のProviderを、ユーザーの明示的選択なしに有効化・契約・課金開始してはならない。推奨設定は提示できるが、推奨を同意とみなさない。

### NFR-PROV-003 自動切替の扱い

Providerが利用不能な場合、ユーザーが選んでいない別Providerへ黙って切り替えてはならない。自動切替を将来提供する場合も、明示的な設定、Data Egress、費用、予算制御と整合させる。詳細は未確定である。

### NFR-PROV-004 初期構成の選択可能性

初回設定では、Main LLM、STT、TTS等について推奨候補を提示できるが、利用構成はユーザーが明示的に選択できなければならない。Main LLMには利用可能なProviderを必須とし、未設定ではCompanionを運用できない明示的なエラーとする。STT / TTSの未設定は正規の選択肢として扱い、推奨設定をユーザーの同意なしに自動適用してはならない。

### NFR-COST-001 利用量の追跡可能性

Provider利用量と推定コストを、Provider、用途、Companion、Sub-agent、Task、自律・バックグラウンド処理の単位で追跡できなければならない。Providerが請求額を提供しない場合は推定値と明示する。

### NFR-COST-002 予算制御

ユーザーは期間ごとの予算上限を設定でき、上限接近の通知、上限到達時の停止・保留・明示的承認要求等を選べなければならない。自律起動とバックグラウンドTaskも同じ予算制御の対象とする。

## 6. Platform、Host / Client

### NFR-PLAT-001 直近BetaのPlatform

直近Betaの実装対象はHost OSとDesktop ClientのWindowsおよびLinuxである。Platform固有の観測、Body、音声、Computer Useは、そのPlatformで可能な範囲を明示する。

### NFR-PLAT-002 将来Platform

HostおよびDesktop ClientへのmacOS、Remote ClientへのMobileおよびWebは後続実装対象とする。要件から明示的に削除されない限り正式リリースまでの対象だが、詳細設計を直近Betaで完了することは要求しない。Companion Stateと永続状態の正本はHostに維持したまま、後続ClientがObservation、Body、音声I/O、Computer Use等のCapabilityを提供できるよう、それらの実行場所をHostまたは直近BetaのDesktop Clientだけへ固定しすぎない。

### NFR-PLAT-003 将来のRemote接続

後続実装でRemote Clientを提供する場合、Host上の同じCompanionとの会話、Task確認、許可された操作を行えるようにする。Remote Client自体は直近Betaの対象ではない。Clientがない場合のCoreの継続性と、Client固有のBody・Observation・Computer Useの違いを混同しない。

### NFR-PLAT-004 完全Offlineの扱い

Local Providerを選択できることは要件だが、ネットワーク切断時にどの範囲の機能を保証するかは未確定である。Cloud依存機能が使えない場合も、状態を破損させず、利用不能理由を示す。

### NFR-PLAT-005 将来のClient Capability

現行のObservation / Computer Useの主対象はHost PCとする。一方、将来のClientが自身の画面、OS、入力、音声、Body、Computer Use等をCapabilityとしてHost上のEneへ提供できるようにし、その追加にCoreやPermissionモデルの大規模な再設計を要求しないことを目標とする。

## 7. 拡張性、保守性、観測可能性

### NFR-EXT-001 拡張境界

Plugin、MCP、Skillの責務と権限境界を明確にし、Ene Coreを書き換えずに新しいProvider、Observation、Capability、Body、Memory処理、Trigger、Harness拡張等を追加できるようにする。

### NFR-EXT-002 Skillの移植性

Skillは可能な限りAgent Skills互換形式でImport / Exportでき、Ene固有の利用履歴、scope、provenance、評価、改訂をPortableな手順本文と分離して管理できなければならない。Character Packageや他のSkillとまとめて配布できるが、個人のMemory、ログ、CredentialをPortableな配布物へ自動的に含めてはならない。

### NFR-OBS-001 監査可能性

ユーザーが、主要な会話、主体間通信、Task、Tool / MCP / Computer Use、Permission、Schedule、自律行動、Memory / Skill変更、重要なCompanion State変更、設定変更、Provider利用、Plugin障害を、過剰な内部推論や全微小State変化の保存なしに追跡できなければならない。

### NFR-OBS-002 障害診断

機能単位の利用不能、Provider障害、Plugin障害、権限拒否、承認待ち、Resource制限を、ユーザーが復旧方法を判断できる程度に説明できなければならない。

## 8. UXとアクセシビリティ

### NFR-UX-001 認知負荷の抑制

日常的なMemory生成、Skill改善、Relationship・Interest更新、内部Task管理のために、ユーザーへ逐次確認を要求してはならない。必要な管理能力は第3層の詳細UIと自然言語で提供する。

### NFR-UX-002 抽象度の一貫性

Companion UI、表層設定、詳細設定・管理の3層で、同じ状態を異なる抽象度で表示できなければならない。内部機構を隠すことと、監査・設定を不可能にすることを同一視しない。

### NFR-UX-003 劣化時の継続

STTが利用できない場合はテキスト入力、TTSが利用できない場合はテキスト表示によって同じCompanion体験を継続できなければならない。Main LLMが未設定または利用不能な場合はCompanion運用エラーとして明示する。Body、Provider、Pluginその他の機能が利用不能な場合は失敗範囲を明示し、設定・ログ閲覧等のCore機能や無関係な機能へ不必要に波及させない。未設定の別Providerへの自動fallbackを代替手段として要求しない。

### NFR-A11Y-001 アクセシビリティ

Desktop Client、設定、詳細管理、テキスト代替UIが満たすアクセシビリティ基準、キーボード操作、スクリーンリーダー対応等の具体的な範囲は未確定である。将来のClient追加を妨げない設計とする。

## 9. 定量化待ち・詳細未確定の項目

- 基準Hardwareとシナリオ別のCPU、GPU、RAM、Storage予算
- 音声入力から発話開始までの目標レイテンシ、TTS中断時間、Remote接続遅延
- Memory RetrievalのPrecision / Recall、誤想起率、長期Memory増加時の品質劣化許容値
- Context Monitorの観測頻度、Main LLM起動率、Token・API利用量の目標
- Learning / Consolidationのバッチ量、アイドル判定、優先度と停止条件
- Crash前の確定状態に対する許容データ損失、`interrupted` / 状態不明の表示と手動Retry導線、ログ保持期間と容量
- Cloud送信時の暗号化、Credential保管、保存地域、Providerごとの削除保証
- Memory削除と再学習禁止の最小保持情報・表示方式
- Remote Clientの認証、暗号化、Device trust、鍵管理、接続方式、SLO
- 各OSでの正式サポート範囲、アクセシビリティ基準
