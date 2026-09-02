# 非機能要件

状態: **Baseline / 対話による初版確定**
最終確認: 2026-09-02

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

## 2. レイテンシとスケジューリング

### NFR-LAT-001 日常対話の応答性

音声またはテキスト入力からCompanionの反応が返るまで、常駐型会話として自然に感じられる応答性を目標とする。Local / Cloud Provider、STT / TTS、Network、モデル性能による差をユーザーへ適切に示す。具体的な目標値は未確定である。

### NFR-LAT-002 Main LLMの節約

Context Monitor、Memory Retrieval、Learning Review、Consolidation、Sub-agent待機、Schedule待機により、必要以上に高価・高性能なMain LLMを起動してはならない。

### NFR-LAT-003 イベント駆動

時間待ちや外部状態待ちを、LLMターンの反復や短い間隔のpollingで実現してはならない。RuntimeのEvent、Schedule、状態キューによって必要な時だけ処理する。

### NFR-LAT-004 将来のfull-duplex余地

現行版でfull-duplex / barge-inを提供しなくても、将来、入力音声と出力音声の同時処理、発話中断、自己音声抑制等を追加するために、会話Coreを全面的に再設計することを前提にしない。具体的な音声プロトコルは未確定である。

## 3. 信頼性、永続性、復旧

### NFR-REL-001 通常のソフトウェア水準の復旧性

Core、Host、Provider、Pluginの停止・クラッシュ・再起動に対して、一般的なデスクトップソフトウェアとして合理的な範囲で、確定済みの会話、主要な状態、Task、Learning待ちを破損・不必要な消失から保護しなければならない。

### NFR-REL-002 タスク・キューの継続

Host停止中に処理できなかったSchedule、Task、Learning Review、Consolidationは、可能な範囲で再起動後に待機・再開・再実行できる状態として保持しなければならない。固定時刻に実行できなかったことだけで、処理を無言で失わない。

### NFR-REL-003 部分障害の隔離

STT、TTS、Main LLM、VLM、Embedding、Network、MCP、Plugin、Sub-agentの単一障害で、Core、他のCompanion、他のProvider、永続データ全体を不必要に停止させてはならない。

### NFR-REL-004 正確な状態表示

完了、失敗、中断、保留、承認待ち、Provider利用不能を区別して表示・記録し、失敗した処理を成功として扱ってはならない。

### NFR-REL-005 復旧要件の範囲

Eneは、完全なイベントソーシング、全操作のクラッシュリプレイ、無停止の自動Failoverを必須としない。通常のトランザクション、必要な状態保存、主要イベントの監査ログによって、製品として合理的な復旧性を実現する。自動Provider fallbackは通常挙動の要件としない。

## 4. Local-first、Privacy、Security

### NFR-PRIV-001 Hostを正本とする

Eneの永続的なユーザーデータとCompanion StateはHost PCを正本としなければならない。Cloud Provider、Remote Client、第三者Pluginは、Eneの状態の唯一の保存先になってはならない。

### NFR-PRIV-002 送信最小化

Cloud Providerへ送信する情報は、許可された目的に必要な範囲に限定できなければならない。ローカルで処理可能であることと、Cloudへ送信してよいことを同一視しない。

### NFR-PRIV-003 観測範囲の可視性

ユーザーは、どのアプリ、Window、Directory、情報種別、Client、Provider、Pluginが観測・処理・保存・送信の対象になっているかを、必要な詳細度で確認・変更できなければならない。

### NFR-PRIV-004 保存量の抑制

監査に必要な主要イベントは残す一方、LLMの逐語的な内部推論、常時観測のRaw画面・音声、意味のない低レベルイベントを既定で無制限に保存してはならない。保持期間、容量、暗号化、Export方式は未確定である。

### NFR-SEC-001 機械的な権限強制

LLMのプロンプト、Memory、Skill、Character Package、Pluginの記述だけで、機械的に拒否されたFilesystem、Credential、Network、Device、Cloud Egress、Computer Useへアクセスできてはならない。

### NFR-SEC-002 第三者Pluginの隔離

第三者Pluginの障害、未許可アクセス、悪意ある入力が、Core、他Plugin、他Companion、ユーザーデータの完全性を直接損なわないよう、原則として隔離・Broker・Permission境界を適用しなければならない。

### NFR-SEC-003 高帯域処理の安全な性能

Plugin隔離とPermission強制は、Local LLM / VLM、画像、音声、Embedding等の高帯域処理へ不要なコピー・直列化・中継遅延を強制しない。安全性と性能を両立できる通信方式を選べること。

## 5. Providerと費用

### NFR-PROV-001 構成の独立性

Main LLM、Context Monitor、Sub-agent、VLM、Embedding、STT、TTS等は、Local、Cloud、Hybridをコンポーネントごとに選択・交換できなければならない。一つのProviderの採用が、他の全Providerの実行場所を強制してはならない。

### NFR-PROV-002 課金への明示的同意

有料または従量課金のProviderを、ユーザーの明示的選択なしに有効化・契約・課金開始してはならない。推奨設定は提示できるが、推奨を同意とみなさない。

### NFR-PROV-003 自動切替の扱い

Providerが利用不能な場合、ユーザーが選んでいない別Providerへ黙って切り替えてはならない。自動切替を将来提供する場合も、明示的な設定、Data Egress、費用、予算制御と整合させる。詳細は未確定である。

### NFR-COST-001 利用量の追跡可能性

Provider利用量と推定コストを、Provider、用途、Companion、Sub-agent、Task、自律・バックグラウンド処理の単位で追跡できなければならない。Providerが請求額を提供しない場合は推定値と明示する。

### NFR-COST-002 予算制御

ユーザーは期間ごとの予算上限を設定でき、上限接近の通知、上限到達時の停止・保留・明示的承認要求等を選べなければならない。自律起動とバックグラウンドTaskも同じ予算制御の対象とする。

## 6. Platform、Host / Client

### NFR-PLAT-001 現行Platform

現行の正式対象はHost OSとDesktop ClientのWindowsおよびLinuxである。Platform固有の観測、Body、音声、Computer Useは、そのPlatformで可能な範囲を明示する。

### NFR-PLAT-002 将来Platform

HostおよびDesktop ClientへのmacOS、Remote ClientへのMobileおよびWebは将来対象とする。将来のClientを追加するため、Companion State、Permission、能力の所在をDesktop Clientへ固定しすぎない。

### NFR-PLAT-003 Remote接続

Remote Clientから接続してもHost上の同じCompanionと会話・Task確認ができなければならない。Clientがない場合のCoreの継続性と、Client固有のBody・Observation・Computer Useの違いを混同しない。

### NFR-PLAT-004 完全Offlineの扱い

Local Providerを選択できることは要件だが、ネットワーク切断時にどの範囲の機能を保証するかは未確定である。Cloud依存機能が使えない場合も、状態を破損させず、利用不能理由を示す。

## 7. 拡張性、保守性、観測可能性

### NFR-EXT-001 拡張境界

Plugin、MCP、Skillの責務と権限境界を明確にし、Ene Coreを書き換えずに新しいProvider、Observation、Capability、Body、Memory処理、Trigger、Harness拡張等を追加できるようにする。

### NFR-EXT-002 Skillの移植性

Skillは可能な限りAgent Skills互換形式でImport / Exportでき、Ene固有の利用履歴、scope、provenance、評価、改訂をPortableな手順本文と分離して管理できなければならない。

### NFR-OBS-001 監査可能性

ユーザーが、主要な会話、主体間通信、Task、Tool / MCP / Computer Use、Permission、Schedule、自律行動、Memory / Skill変更、Provider利用、Plugin障害を、過剰な内部推論の保存なしに追跡できなければならない。

### NFR-OBS-002 障害診断

機能単位の利用不能、Provider障害、Plugin障害、権限拒否、承認待ち、Resource制限を、ユーザーが復旧方法を判断できる程度に説明できなければならない。

## 8. UXとアクセシビリティ

### NFR-UX-001 認知負荷の抑制

日常的なMemory生成、Skill改善、Relationship・Interest更新、内部Task管理のために、ユーザーへ逐次確認を要求してはならない。必要な管理能力は第3層の詳細UIと自然言語で提供する。

### NFR-UX-002 抽象度の一貫性

Companion UI、表層設定、詳細設定・管理の3層で、同じ状態を異なる抽象度で表示できなければならない。内部機構を隠すことと、監査・設定を不可能にすることを同一視しない。

### NFR-UX-003 劣化時の継続

STT/TTS、Body、Provider、Pluginの一部が使えなくても、可能な代替手段で同じCompanionの体験を継続できる。代替への切替は、ユーザーに理解できる形で示す。

### NFR-A11Y-001 アクセシビリティ

Desktop Client、設定、詳細管理、テキスト代替UIが満たすアクセシビリティ基準、キーボード操作、スクリーンリーダー対応等の具体的な範囲は未確定である。将来のClient追加を妨げない設計とする。

## 9. 定量化待ちの項目

- 基準Hardwareとシナリオ別のCPU、GPU、RAM、Storage予算
- 音声入力から発話開始までの目標レイテンシ、TTS中断時間、Remote接続遅延
- Context Monitorの観測頻度、Main LLM起動率、Token・API利用量の目標
- Learning / Consolidationのバッチ量、アイドル判定、優先度と停止条件
- Crash後の許容データ損失、再開可能性、ログ保持期間と容量
- Cloud送信時の暗号化、Credential保管、保存地域、Providerごとの削除保証
- 各OSでの正式サポート範囲、アクセシビリティ基準、Remote ClientのSLO
