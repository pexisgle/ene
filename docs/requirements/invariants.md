# 不変条件

状態: **Baseline / 対話による初版確定**
最終確認: 2026-09-02

以下は、Provider、モデル、OS、UI、Storage、Plugin、通信方式などの実装を変更しても破ってはならない製品上の不変条件である。性能目標、具体的なschema、実装上の便利な制約はここへ置かない。

## 個体性と製品境界

### INV-001 Companionは継続する個体である

Companionは、一時的なLLMセッション、単なる表示テーマ、またはSub-agentの別名ではない。ユーザーとの経験、Memory、State、Relationshipを持つ継続する個体として扱う。

### INV-002 複数Companionは別個体である

同じEne Core上の複数Companionは、それぞれ別の人格、解釈、記憶、感情、Relationship、意図を持つ。Sharedな情報を知っていることは、同じ出来事を直接経験したことを意味しない。

### INV-003 Sub-agentはCompanionではない

Sub-agentはCompanionの意図を実行する一時的な作業主体であり、原則として独立した長期人格ではない。Sub-agentの作業履歴を残しても、Sub-agent個体をCompanionとして扱わない。

### INV-004 製品の主役はCompanionである

Agent Harness、Computer Use、Tool、MCP、Plugin、内部推論、Sub-agentはCompanionの能力であり、日常の製品体験の主役ではない。ユーザーは原則としてCompanionへ目的を伝える。

### INV-005 一つのEne環境は一人のユーザーに属する

複数Clientから接続できても、それは同じ一人のユーザーが同じEneへ接続することであり、同一Coreを複数ユーザーの独立テナントとして扱わない。

## 状態と正本

### INV-006 Hostが永続状態の正本である

Eneの永続的なユーザーデータ、Companion State、Memory、Character Instance、Permission、Task履歴の正本は、Ene CoreをホストするHost PCにある。Cloud Provider、Remote Client、Pluginを唯一の正本にしない。

### INV-007 観測・伝達・保存・送信は同一ではない

ある情報を観測できることは、その情報をContext Monitorで処理できること、Main Companionへ伝えられること、長期Memoryへ保存できること、Cloudへ送信できることを意味しない。これらの境界を個別に評価する。

### INV-008 推測は確認済み事実ではない

CompanionやSub-agentの推測、要約、解釈は、ユーザーの明示発言、直接観測、検証済み情報と同じ確実性で扱わない。Provenanceとconfidenceを失ったまま、推測を事実として固定しない。

### INV-009 Internal StateとCompanion Contextを混同しない

Current Affective State、Mood、Relationship、Interest等の内部数値・構造化Stateを、そのまま機械的に発話やMain LLMの判断へ直結しない。必要なStateは、由来・変化・確信度を保ったSemantic Stateへ変換して利用する。一方、Memory、Skill、Conversation、Observation / Context、Task等はSemantic Stateそのものではなく、必要に応じてSemantic StateとともにCompanion Contextへ統合する。

### INV-010 Memoryの種類とscopeを混同しない

Working、Core、Episodic、SemanticというMemoryの種類と、Shared、Companion-specificというscopeは別の軸である。Shared / Companion-specificを理由にMemory種別を失わない。Working Memoryが短期状態であることと、長期Memoryのscope分類も混同しない。

### INV-011 削除意図を学習が覆さない

ユーザー削除またはPrivacy / retention policyによって長期Memoryとして利用しないと決定された情報を、Learning Review、Consolidation、Retrievalその他の自動処理が、残存する原資料から同じ長期Memoryとして勝手に復活させてはならない。

## 安全性と権限

### INV-012 Hard BoundaryはLLMより強い

LLM、Memory、Skill、Character Package、Plugin、MCP、Provider、自然言語委任のいずれも、機械的に禁止されたFilesystem、Credential、Network、Device、Cloud Egress、Computer Use、Capabilityへアクセスできない。

### INV-013 学習は権限を増やさない

Learning Review、Consolidation、Memory更新、Skill生成・改善、Relationship更新は、Permission、Sandbox、Data Egress、観測範囲、Credentialその他のHard Boundaryを緩めたり新しく付与したりできない。

### INV-014 Characterは権限を付与しない

Character Definition、Character Package、Character Instanceが望むRole、Capability、行動特性は、ユーザー環境の実効Permissionを変更しない。

### INV-015 Skillは能力や権限を追加しない

Skillは既存Capabilityの使い方を表すProcedural Knowledgeであり、新しいTool、Provider、Credential、Filesystem、Network、Permission、Sandbox例外を追加しない。

### INV-016 Pluginは無制限の特権を持たない

PluginはEneを拡張できるが、Pluginであることを理由にCore、他Plugin、CompanionのPrivate Memory、ユーザーデータ、OS資源へ無制限にアクセスできない。公開Extension PointとPermission / Broker境界を通る。

## 実行と会話

### INV-017 Main LLMは待機のために動き続けない

Sub-agent、Schedule、外部サービス、ユーザー入力その他のイベントを待つためだけに、Main LLMを定期的に呼び出したりPollingしたりしない。待機はRuntimeが担い、意味のある状態変化で起動する。

### INV-018 会話空間は同時発話しない

複数Companionが同時に発話意図を持っても、同一会話空間で論理的に同時発話させない。先に発話した内容を含む最新文脈で、後続Companionが発話要否を再評価する。

### INV-019 Companionの判断と物理制約を分ける

Companionは、発話、作業、移動、表情、モーション等の意味や意図を判断できる。一方、Permission、Sandbox、画面外への逸脱、座標、衝突、OSの物理的制約はシステムが強制する。

### INV-020 音声代替は同じ存在である

STT/TTSが使えない場合のテキスト入力・吹き出しは、別の会話、別のユーザー、別のCompanion、別のMemoryとして扱わない。

## 拡張と可用性

### INV-021 Plugin、MCP、Skillは別の責務である

PluginはEne自体の拡張、MCPは外部Tool / Resourceの接続、Skillは既存能力の利用方法である。単純な外部Tool接続を理由にPluginとSkillを混同しない。

### INV-022 第三者Pluginの障害をCoreへ波及させない

第三者Pluginのクラッシュ、OOM、GPU Runtime障害、Deadlock、未許可アクセスによって、Ene Core、他Plugin、他Companion、永続状態全体の完全性を不必要に損なわない。

### INV-023 自動Provider切替を暗黙に行わない

Providerが利用不能になった場合、ユーザーが選んでいない別Providerへ黙って切り替えたり、課金やData Egressを新たに開始したりしない。別Providerの利用は、ユーザーが理解できる明示的な選択または将来定義する明示設定に基づく。

### INV-024 有料Providerは明示的に選択される

有料または従量課金のProviderを、推奨表示やインストールだけを理由に有効化、契約、課金開始しない。

### INV-025 記録可能性と内部推論を混同しない

主要な会話、判断、Action、Result、Permission、重要な状態変更は追跡可能にするが、LLMの逐語的な内部推論、すべての微小State変化、常時観測の全Rawデータを保存しなければ監査できない、とはしない。

### INV-026 過去資料は現行要件ではない

過去の設計・計画資料やGit履歴に書かれた設計、数値、実装構造を、明示的な再確認なしに現行要件や不変条件へ昇格させない。
