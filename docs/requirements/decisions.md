# 要件決定記録

状態: **Active decision log / 初版確定**
最終確認: 2026-09-02

ここには、今回の対話で明示的に確認した製品上・設計上の判断と、その理由を残す。`functional.md` と `non-functional.md` は「何を満たすか」を定義し、この文書は「なぜその方向を選んだか」「どの実装詳細をまだ固定していないか」を記録する。既存実装の構造は、決定の根拠ではない。

## 記録形式

```text
R-Dxxx: 短いタイトル
Status: confirmed | superseded
Nature: product decision | design direction
Decision: ...
Reason: ...
Consequences: ...
Supersedes: ... （任意）
```

## 確定した判断

### R-D001: Companion-first製品

Status: confirmed
Nature: product decision
Decision: Eneの主役と最優先の製品価値を、AITuber / AI companionとしての人格的な体験に置く。Agent Harness、Computer Use、Sub-agent、Tool実行、内部推論はCompanionの内部能力として扱う。
Reason: Eneは、PC操作機能の前面にキャラクターを置く製品ではなく、ユーザーが一緒に過ごせる個が必要に応じて実作業を行う製品を目指すため。
Consequences: 日常UIはCompanion中心とし、内部エージェントの詳細は必要な場合だけ深い管理画面で見せる。

### R-D002: 単一ユーザー・複数デバイス

Status: confirmed
Nature: product decision
Decision: Eneの基本モデルは、1人のユーザーが1つのEne環境を所有し、複数のDevice / Clientから同じCompanionへ接続する構成とする。
Reason: 個人的な記憶、関係性、設定、権限を一人のユーザーの管理下に置くため。
Consequences: マルチテナント・チーム利用は現行範囲外であり、Remote ClientもHost上の状態の正本を複製しない。

### R-D003: Host / Client分離

Status: confirmed
Nature: design direction
Decision: Ene Core、永続状態、Scheduler、Context Monitor、Learning、HarnessはHostに置き、Desktop / Remote Clientは同じCoreへ接続する。
Reason: ユーザーが接続していない間もCompanionとTaskが継続する必要があるため。
Consequences: 現行はWindows / LinuxをHostとDesktop Clientの対象とし、将来macOS、Mobile、Web Clientを追加できる余地を残す。Remote Clientは現行の正式対象ではない。

### R-D004: 複数の独立したCompanion

Status: confirmed
Nature: product decision
Decision: 1つのEne Core上に複数のCompanionを持たせ、各Companionを独立した人格・State・Memory・Relationshipを持つ個として扱う。
Reason: キャラクター切替ではなく、同じ環境に複数の個が存在する体験を実現するため。
Consequences: Shared Memoryは存在するが、共有情報を各Companionの直接経験としてコピーしない。Companion間会話と会話調停が必要になる。

### R-D005: Character DefinitionとInstanceの分離

Status: confirmed
Nature: product decision
Decision: 配布可能なCharacter Definition / Packageと、特定ユーザーのもとで成長するCharacter Instanceを分ける。
Reason: 同じキャラクターを複数ユーザーが使っても、個人的なMemory、Emotion、Relationshipを混ぜないため。
Consequences: Import / Export、将来の配布・販売Marketplaceを想定できるが、PackageはPermissionを付与しない。

### R-D006: Aliciaを現行デフォルトとする

Status: confirmed
Nature: product decision
Decision: Character Packageなしでも開始できるデフォルトCompanionを提供し、現時点の製品構成ではAliciaを採用する。
Reason: 初回起動時にキャラクター選択を必須にせず、Companion体験へすぐ到達できるようにするため。
Consequences: 機能要件は特定Character名へ固定せず「少なくとも1体の利用可能なデフォルトCompanion」とする。将来Ene独自キャラクターへ置き換える余地を残し、移行方法と時期は未確定とする。

### R-D007: VRM常駐UIと音声中心

Status: confirmed
Nature: product decision
Decision: 現行の主UIはDesktopMate風のVRMデスクトップマスコットとし、通常対話はSTT / TTSによる音声を中心にする。
Reason: EneのAITuberらしさと「同じPC上に存在する」感覚を製品の入口にするため。
Consequences: STT未設定時はテキスト入力、TTS未設定時は吹き出し表示を使い、同じ会話として継続する。

### R-D008: Full-duplexは将来、構造は阻害しない

Status: confirmed
Nature: product decision
Decision: 現行版で人間に近いfull-duplex / barge-inを必須とせず、将来追加するために会話・音声I/Oを全面再設計する必要がない構造を目指す。
Reason: 初期の複雑性を抑えつつ、音声体験の将来性を失わないため。
Consequences: 現行の半二重対話、発話停止・キャンセル・割り込みを将来拡張できる境界として扱う。

### R-D009: Bodyの意味判断と物理制約の分離

Status: confirmed
Nature: design direction
Decision: Companionが移動、滞在、作業領域回避、表情、モーション等の意味・意図を判断し、座標、衝突、画面外防止、OS制約はシステムが強制する。
Reason: キャラクターの自律性と安全なデスクトップ操作を両立するため。
Consequences: ドラッグやWindow変化もCompanionのContextとして扱える。Live2D等の将来Bodyを追加できる。

### R-D010: Event-driven Main LLM

Status: confirmed
Nature: design direction
Decision: Companionは常在するが、Main LLMはユーザー入力、重要Event、Schedule、Sub-agent報告、他Companion発話等の必要な時だけ起動する。
Reason: 常時推論はコスト、GPU、電力、軽量性と衝突し、待機のためのLLM pollingも不要なため。
Consequences: 待機・時間経過・Sub-agentの進行はRuntimeが管理し、意味のあるEventでCompanionをwakeする。

### R-D011: 共有の軽量Context Monitor

Status: confirmed
Nature: design direction
Decision: 継続的な観測と重要度判定はEne全体で1つの軽量・高速なMonitorを共有し、複数CompanionのContextを持って各個体への関連性を判断する。可能ならLocalの軽量モデルを使う。
Reason: 大量のRaw desktop情報を各Main LLMへ流さず、コストと負荷を抑えるため。
Consequences: Monitorは重要そうな変化を通知するが、発話・行動・無視の最終判断は各Companionが行う。必要時にはMain Companionが詳細情報を取得する。

### R-D012: 汎用HarnessをCompanionの下に置く

Status: confirmed
Nature: product decision
Decision: Agent Harnessはコーディング専用ではなく、ユーザーのPC上で許可された汎用作業を行う。最上位の意思決定主体はCompanionとする。
Reason: 調査、文書、ファイル、ブラウザ、デスクトップ、外部サービス等を一つの自然な委任体験で扱うため。
Consequences: ユーザーはHarnessを直接操作せず、Companionへ自然言語で目的を伝える。

### R-D013: 一時的なSub-agentと最大深度

Status: confirmed
Nature: design direction
Decision: Sub-agentはCompanionの意図を実行する一時的な作業主体とし、複数並列実行とChild Sub-agentを許可する。ただし委任深度は `Companion → Sub-agent → Child Sub-agent` までとする。
Reason: 一般的なHarnessの分割・並列能力を利用しつつ、無制限な再帰と人格の増殖を防ぐため。
Consequences: Sub-agentは意味のある進捗をCompanionへ返し、最終まとめ後に破棄する。

### R-D014: Sub-agentからの意味のある進捗共有

Status: confirmed
Nature: product decision
Decision: Sub-agentは実行中に発見、計画変更、問題、判断要求、フェーズ完了等をCompanionへ共有し、Companionは報告に応じて追加指示やユーザー通知を判断する。
Reason: Companionが作業を自分の意図として把握し、ユーザーの質問や自発的な報告に答えられるようにするため。
Consequences: 低レベルの全操作を流すのではなく、Task履歴と最終まとめを残す。Task / Sub-agentはCancelと安全なRetryを扱える必要がある。

### R-D015: 自律性を観測・発話・作業に分ける

Status: confirmed
Nature: product decision
Decision: 観測、Companionの自発発話、内部調査、外部状態変更作業を別の自律性・Permissionとして扱う。新規の外部変更作業は原則通知し、事前委任・Schedule済み・提案の根拠調査は通知なしを許可する。
Reason: 能動的なCompanionらしさと、予期しない外部変更の抑制を両立するため。
Consequences: 通知と承認を区別し、詳細なリスク閾値は未確定のままHard Boundaryと文脈判断を組み合わせる。

### R-D016: Host中心のLocal-first

Status: confirmed
Nature: product decision
Decision: 会話、Memory、Emotion / Mood、Relationship、Character Instance、設定、Permission、Task履歴等の永続状態はHost PCを正本とする。
Reason: ユーザーの個人的な存在としてのEneを、特定Cloud ProviderやClientの可用性から分離するため。
Consequences: Cloudは必要な計算の委譲先であり、Remote Clientは同じ正本へ接続する。

### R-D017: Providerをコンポーネントごとに選ぶ

Status: confirmed
Nature: product decision
Decision: Main LLM、Context Monitor、Sub-agent LLM、VLM、Embedding、Reranker、STT、TTS等について、利用可能なLocal ProviderまたはCloud Provider等をコンポーネントごとに独立して選べるようにする。その結果として完全Local、Cloud中心、LocalとCloudの混在を正規構成とする。個別ProviderがHybrid実行を提供することは許容するが必須としない。
Reason: Hardware、コスト、品質、Privacyの要求がコンポーネントごとに異なるため。
Consequences: 特定Providerや現在のPlugin方式を要件へ固定しない。Cloud送信は個別Permissionの対象とする。「Hybrid」は主としてシステム全体の混在構成を指し、全ProviderへLocal+Cloudの複合実行を要求しない。

### R-D018: 自動Provider fallbackを前提にしない

Status: confirmed
Nature: product decision
Decision: Providerが利用不能になったとき、ユーザーが選んでいない別Providerへ黙って切り替えることを、Eneの基本挙動や必須の復旧方法にしない。
Reason: 課金、Data Egress、品質、Privacyの変更をユーザーが意図しないまま発生させないため。
Consequences: 利用不能を明示し、再設定や別Providerの明示選択を可能にする。将来の自動切替の詳細は未確定。

### R-D019: 有料Providerは明示的に選択する

Status: confirmed
Nature: product decision
Decision: 推奨Providerを表示できるが、有料・従量課金Providerを推奨だけで有効化、契約、課金開始しない。
Reason: 常駐・自律処理では、ユーザーが気付かない費用が発生し得るため。
Consequences: 初回設定で料金形態、送信先、品質、速度、必要Hardware、認証要件を確認できる。

### R-D020: Pluginを汎用拡張基盤とする

Status: confirmed
Nature: product decision
Decision: PluginはLLM / STT / TTS ProviderやTool追加だけでなく、Observation、Context、Memory、Trigger、Harness、Body、内部Event、設定・管理UI等、Eneが公開する拡張点へ参加できる汎用基盤とする。
Reason: Ene本体を書き換えずに、能力とRuntimeの拡張を可能にするため。
Consequences: Pluginは任意のCore内部を直接変更せず、通常のPermissionと隔離境界を通る。

### R-D021: 外部ToolはMCPを優先する

Status: confirmed
Nature: design direction
Decision: 外部Tool / Resourceの接続だけで足りる場合は、Ene独自Pluginを作らずMCPを利用する。
Reason: Plugin SystemをMCPの再実装にせず、標準的な外部接続とEne内部拡張を分離するため。
Consequences: MCPは外部能力、PluginはEne自体の拡張、Skillは既存能力の使い方として扱う。

### R-D022: 第三者Pluginは隔離する

Status: confirmed
Nature: design direction
Decision: 第三者Pluginは原則アウトプロセス等でCoreから隔離し、API、Event、Capability、Brokerを経由してアクセスする。隔離は高帯域・低遅延処理を不必要に阻害しない。
Reason: Pluginのクラッシュ、未許可アクセス、GPU Runtime障害からCoreと他データを守りつつ、Local Providerの性能を確保するため。
Consequences: Control planeとbulk dataの転送を分ける余地を持たせ、PermissionとSandboxの範囲で隔離されたProvider Plugin自身がGPU / Acceleratorや必要なRuntimeへ直接アクセスできる構成を許容する。具体的なIPCやSandbox方式は未確定。

### R-D023: Companion UIへのPlugin干渉を制限する

Status: confirmed
Nature: product decision
Decision: Pluginは設定、管理、詳細表示、診断等へUIを追加できるが、通常のCompanion UIのマスコット、発話、吹き出し、主要対話面へ任意のUIを挿入・置換できない。
Reason: AITuberとしての一貫した主役と、第三者拡張性を両立するため。
Consequences: Eneが公開するBody APIや正規の表現拡張は妨げない。

### R-D024: Memoryの直交する分類

Status: confirmed
Nature: product decision
Decision: MemoryをWorking、Core、Episodic、Semanticに分け、scopeをSharedとCompanion-specificに分ける。SkillはProcedural Memoryとして別管理する。
Reason: 現在の作業状態、常時参照する情報、経験、抽象化された事実、作業手順、共有範囲を混同しないため。
Consequences: Working Memoryは短期状態として扱い、Coreは小さく厳選し、Episodic / SemanticはRetrievalする。Learning Reviewの長期保存先としてWorking Memoryを扱わない。Memoryの具体schemaは未確定。

### R-D025: 自動想起と明示検索を併用する

Status: confirmed
Nature: product decision
Decision: 普段は軽量な自動想起を行い、必要時はCompanion、Agent、ユーザーが明示的な深掘り検索を実行できるようにする。RetrievalはEmbedding単独でなく、時間、Entity、scope、感情的意義、Relationship、Task、confidence等を組み合わせる。
Reason: 自然な会話と、曖昧・古い・根拠が弱い過去の調査を両立するため。
Consequences: Consolidated Memoryだけでなく、保持ポリシーで利用可能な元Conversation / Logまで辿れるようにする。

### R-D026: 忘却は主にRecall decayとする

Status: confirmed
Nature: design direction
Decision: 通常の忘却は物理削除ではなくRecall priorityの低下とし、ユーザー削除、Privacy、明確な誤記憶、完全重複等では物理削除や再学習禁止を行えるようにする。
Reason: 強いcueがあれば古い経験を思い出せる余地を残しつつ、ユーザーの削除意図とPrivacyを守るため。
Consequences: Memoryはimportance、recency、emotional significance、confidence等を持つ。削除意図に反して原資料から同じMemoryを自動再生成しない。

### R-D027: Learning Review / Consolidationを分離する

Status: confirmed
Nature: design direction
Decision: 個々のExperienceからの候補作成・即時反映をLearning Review、重複統合・抽象化・再評価・忘却調整をConsolidationとして扱い、非緊急のConsolidationは主にアイドル時・余剰資源のある時に行う。
Reason: ユーザー体験への即時性と、常駐Coreの軽量性を両立するため。
Consequences: Host停止・高負荷を跨ぐ処理キューが必要になるが、固定夜間実行やMain LLM常時実行は要求しない。アイドル時処理をLearning Reviewそのものと混同しない。

### R-D028: 学習は自動、管理は深いUI

Status: confirmed
Nature: product decision
Decision: 日常のMemory / Skill / Relationship / Interestの更新は通常自動とし、ユーザーへ逐次承認を求めない。確認、修正、削除、Rollback、詳細履歴は第3層の管理UIと自然言語で提供する。
Reason: ユーザーの認知負荷を最低限にしつつ、制御可能性を失わないため。
Consequences: Permissionと保持ポリシーは自動学習より上位に置き、推測を事実として保存しない。

### R-D029: Agent Skills互換のSkill

Status: confirmed
Nature: product decision
Decision: Skillは独自フォーマットを作らず、可能な限りAgent Skillsの形式・運用と互換にする。Ene固有のscope、評価、provenance、revision、利用履歴はSkill Manager側で管理する。
Reason: Import / Export、Progressive Disclosure、他のSkill対応Agentとの再利用性を確保するため。
Consequences: Skillは既存Capabilityの使い方であり、Plugin、MCP、Permissionを代替・付与しない。

### R-D030: Emotionを連続StateとAppraisalで表す

Status: confirmed
Nature: design direction
Decision: Emotion Systemは表情だけでなく意思決定に影響する内部状態を扱い、基礎状態はValence、Arousal、Control（Sense of Control）を中心とする。ControlはPADモデルのDominanceに由来するが、Eneでは状況への主体性・対処可能感を表す名称として扱う。出来事の意味はNovelty / Expectedness、Goal relevance、Conduciveness、Agency、Coping、Norm compatibility等のAppraisalで扱う。
Reason: 感情の連続性、個体差、同じ出来事への異なる反応を表現し、感情ラベルの大量な固定軸や「Dominance=他者支配」という誤解を避けるため。
Consequences: Baseline、Mood、Fast Affect、Decay、Reactivity、Inertia、Recovery、Reappraisalを区別し、正確な更新式は未確定とする。

### R-D031: RelationshipをFamiliarity・Closeness・Trustで分ける

Status: confirmed
Nature: design direction
Decision: Relationshipは少なくともFamiliarity、Closeness、Trustを別状態として扱い、Trustは必要に応じてPredictability、Dependability、Benevolent expectation等へ分ける。
Reason: 利用期間、心理的な近さ、信頼性を単一の好感度へ潰さないため。
Consequences: Relationshipは通常、経験の蓄積に応じて比較的緩やかに変化し、小イベントだけでゲーム的に極端に上下させない。ただし重大イベントは相応に大きく影響し得る。Satisfaction、Affection、Resentment等は必要時の派生解釈とし、依存を最大化する単一Commitment指標を製品Coreに置かない。Attachment anxiety / avoidance、Need for closeness、Independence、Expressiveness等は必要なCharacterのPersonality側の傾向として扱い、全Companion共通のRelationship軸にはしない。

### R-D032: UIを3層に分ける

Status: confirmed
Nature: product decision
Decision: Companion UI、表層設定、詳細設定・管理の3層で情報公開の深さを分ける。
Reason: 日常は自然なCompanion体験に集中し、高度なユーザーには監査・管理能力を提供するため。
Consequences: 3層の違いは画面構成、導線、視覚的階層で一貫して表現する。具体的な色、テーマ、コンポーネントは未確定。

### R-D033: 主要イベントを監査し、内部推論は保存しない

Status: confirmed
Nature: design direction
Decision: 会話、主体間通信、Task、Tool / MCP / Computer Use、Permission、Schedule、重要Context、Memory / Skill変更、重要なCompanion State変更、設定変更、Provider利用、Plugin障害等を追跡可能にする。一方、逐語的な内部推論、全微小State変化、Raw音声、Raw画面、意味のない低レベルイベントを既定で永続保存しない。
Reason: 監査性、トラブルシュート、Privacy、Storage消費を両立するため。
Consequences: 監査ログを完全なクラッシュリプレイやEvent Sourcingと同一視しない。

### R-D034: コストを第一級に管理する

Status: confirmed
Nature: product decision
Decision: Provider利用量・推定コストを、用途、Provider、Companion、Sub-agent、Task、バックグラウンド処理ごとに追跡し、予算上限、通知、停止・保留・承認要求を設定できるようにする。
Reason: 常駐・自律型システムでは、ユーザーが気付かない利用量の増加を防ぐ必要があるため。
Consequences: 料金を正確に取得できない場合は推定値と表示し、予算到達時に未選択Providerへ自動fallbackしない。

### R-D035: 軽量性を最優先する

Status: confirmed
Nature: product decision
Decision: Ene Coreは通常のブラウジング、開発、文書作成等を邪魔しない程度に軽量であることを最優先級とし、通常はHostの余剰資源を利用する。
Reason: DesktopMateのように常駐させる製品であり、常時負荷が体験を損なうため。
Consequences: VRM / Providerの負荷はCoreと分けて評価し、高負荷時は非緊急処理を抑制する。固定CPU率などの数値は未確定。

### R-D036: 一般的なソフトウェア水準の復旧性

Status: confirmed
Nature: product decision
Decision: Crash、再起動、Provider・Plugin障害に対し、確定状態、主要履歴、Task、Learning待ちを合理的な範囲で保護・復元する。ただし完全なクラッシュリプレイ、無停止Failover、自動Provider fallbackは必須にしない。
Reason: 必要な信頼性を確保しつつ、過剰な耐久基盤を製品要件に持ち込まないため。
Consequences: 部分障害を隔離し、成功・失敗・中断・保留を正確に表示する。

### R-D037: Semantic StateとCompanion Contextを分ける

Status: confirmed
Nature: design direction
Decision: Current Affective State、Mood、Relationship、Interest等の内部数値・構造化Stateは、LLMが理解しやすいSemantic Stateへ変換する。一方、Memory、Skill、Conversation、Observation / Current Context、Task等はSemantic Stateそのものとはせず、必要な情報をSemantic StateとともにCompanion Contextへ統合してMain LLMへ渡す。
Reason: 数値をそのままLLMへ渡す不明瞭さを避けつつ、MemoryやTaskまで一つの意味表現へ押し込んで責務を曖昧にしないため。
Consequences: Internal State → Semantic Stateと、複数情報源 → Companion Contextという2段階を区別する。基本的なSemantic State変換はMain LLMを必須とせず軽量なRuntime処理で実行可能にし、具体的なPrompt形式は未確定とする。

### R-D038: 初期Provider構成はユーザーが明示的に選ぶ

Status: confirmed
Nature: product decision
Decision: 初回設定でMain LLM、STT、TTS等の推奨候補を提示できるが、利用するProviderまたは未設定はユーザー自身が明示的に選択する。STT / TTSは未設定を正規の構成とする。
Reason: Providerは費用、Privacy、Hardware負荷、品質に影響するため、推奨をユーザーの選択として扱わないため。
Consequences: STT / TTS未設定時はテキストUIを利用する。有料ProviderについてはR-D019の明示同意も満たす。

### R-D039: 将来ClientもCapabilityを提供できる

Status: confirmed
Nature: design direction
Decision: 現行のDesktop Observation / Computer UseはHost PCを主対象とするが、将来のClientが音声I/O、Observation、Body、Computer Use等をCapabilityとしてHost上の同じEneへ提供できる構造を保つ。
Reason: 複数デバイスから同じCompanionへ接続する将来像で、Client側を単なる表示端末に固定しないため。
Consequences: Companion Stateと永続状態の正本はHostに維持する。Observation等のCapabilityの実行場所はHostだけに固定せず、追加時にCoreやPermission Systemを大規模に再設計することを前提としない。具体的なRemote protocol、認証、暗号化、開始時期は未確定。

### R-D040: Memory Retrievalは検索精度を重視する

Status: confirmed
Nature: product decision
Decision: Memory Retrievalは軽量性だけでなく高い検索精度を重要品質として扱い、長期Memoryが増加しても関連Memoryの取りこぼしと無関係Memoryの混入を抑える。自動想起と明示的Deep Retrievalの双方を評価対象とする。
Reason: 保存量が増えても必要な記憶を正しく思い出せなければ、長期Memoryの価値とCompanionの一貫性が失われるため。
Consequences: Embedding単独に固定せず、Hybrid RetrievalとRerankingを利用可能にし、代表的な評価セットと定量指標は実測して決める。

### R-D041: 削除されたMemoryを自動再学習で復活させない

Status: confirmed
Nature: product decision
Decision: ユーザー削除またはPrivacy / retention policyによって長期Memoryとして利用しないと決定された情報は、原資料が別目的で残っていても、Learning ReviewやConsolidationによって同じ長期Memoryとして自動再生成しない。
Reason: 「忘れて」というユーザー意図やPrivacy削除が、バックグラウンド学習によって無効化されることを防ぐため。
Consequences: Memoryのみの削除、原資料自体の削除、原資料を残した再学習禁止を区別できる余地を持つ。具体的なUI、保持情報、tombstone等の実装方式は未確定。

### R-D042: Companion StateとCurrent Contextを分ける

Status: confirmed
Nature: design direction
Decision: Emotion Systemの状態、Mood、Relationship、Interest、意図等の継続的な内部状態をCompanion Stateとし、現在のConversation、Desktop / OS観測、Schedule、Task、接続状態、時間情報等の外部・一時的情報をCurrent Contextとして区別する。
Reason: 「Companion自身の状態」と「現在周囲で起きていること」を同じStateへ押し込み、永続化・Semantic State・Context構築の責務を曖昧にしないため。
Consequences: Main LLM起動時は両者を必要に応じてCompanion Contextへ統合する。Current ContextをCompanion Stateとして恒久保存することを前提としない。

## 未確定事項

次の項目は、今回の判断から直接は決まらないため、別途検証・対話して決める。

| ID | 未確定項目 |
|---|---|
| O-001 | 各OS・Clientのリリース時期、対応機能、Remote接続の仕様・認証・暗号化・Device trust |
| O-002 | AliciaからEne独自キャラクターへの移行方法、Character Packageのmanifest・署名・Marketplace方式 |
| O-003 | LLM、VLM、Embedding、Reranker、STT、TTSのProvider・モデル・価格・Credential保管 |
| O-004 | 完全Offlineで保証する機能範囲 |
| O-005 | CPU、GPU、RAM、Storage、対話・音声・Remoteの定量的な性能予算 |
| O-006 | Current Affective State、Mood、Relationship、Interest、Semantic State、Memoryの正確なschemaと更新式 |
| O-007 | Permissionのリスク分類、通知・承認の閾値、自然言語ポリシーの競合解決 |
| O-008 | Plugin API、Extension Point、IPC、Sandbox、Broker、bulk data転送の具体方式 |
| O-009 | Companion間交流の段階数・既定値、ランダム起動の頻度・制限 |
| O-010 | ログ、Memory、成果物、音声、画像、Tool outputの保持・削除・Export形式 |
| O-011 | 「忘れる」要求のMemory削除、原資料削除、再学習禁止を区別するUI・保持方式 |
| O-012 | Accessibility基準、UIテーマ、3層の視覚表現 |
