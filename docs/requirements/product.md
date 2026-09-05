# 製品定義

状態: **再構成済みBaseline**
最終確認: 2026-09-05

## 製品の要約

Eneは、一人のOwnerが管理するHost上で継続する、Companion中心のパーソナルAIシステムである。

日常的な体験の主役は、身体を持ち、同じ個として時間を越えて続くCompanionである。会話、Memory、Relationship、関心、身体表現と、実際のPC作業を一つの存在から利用できる。

EneはCompanionを体験上の中心に置くが、汎用作業Agentとしての能力を劣後させない。Tool、MCP、Skill、Provider、Task AgentはCompanionが使う能力であり、別の主役や劣化した付属機能ではない。

## 利用者と実行場所

- 一つのEne環境は一人のOwnerに属する。複数人の独立した利用者を同じ環境へ収容しない。
- HostはEne Coreを実行し、Eneが管理する永続状態の正本を保持するOwner管理下のPCである。
- Clientは同じHostへ接続する表示・会話・操作の入口であり、永続状態の正本ではない。
- 推論はHost、OwnerのLAN、またはOwnerが選んだCloud Providerで実行できる。

## 主要概念

### Companion

Characterを基に作られ、Memory、Relationship、状態、関心、設定をExperienceから形成する継続的な個体。同じCharacterから作ったCompanionも別個体である。

### Character

配布可能な静的人格、設定、Body、Voice、表現、推奨Skillの組み合わせ。特定Ownerとの経験や関係は含まない。Character Packageはそのimport/export単位である。

### Experience

CompanionがOwnerとの対話、Task、Tool利用、Observation、他のCompanionとの交流その他の活動を通じて経験した出来事とその結果。ExperienceはMemory、Skill、Relationship、状態等が形成または更新される根拠になり得る。Experienceであることは、そのRaw dataを恒久保存することを意味しない。

### Task

OwnerからCompanionへ依頼され、開始、進行、判断待ち、完了、失敗、Cancel等の状態を持つ作業単位。Companionは必要に応じてTask Agentへ一部を委任できる。

### Task Agent

CompanionからTaskの一部を委任された一時的な主体。独立した長期人格やRelationshipを持たず、委任元Companionの権限とTask境界内で作業し、結果を返す。

### Workspace

Taskが作業対象として利用するfolder、file、外部source等との関連付け。WorkspaceはTaskより上位の独立containerやデータ所有主体ではなく、Taskに従属する作業場である。Taskとの関連が削除されても、外部のfileやsource自体は削除しない。

### MemoryとSkill

Experienceから形成され得るLearning。Memoryは出来事、事実、意味、好み等を後の理解に用いる知識であり、一般世界知識の保存領域ではない。Skillは将来の類似Taskで再利用できる手順、専門知識、実行上の注意、補助resource等をまとめたLearningで、Agent Skillsとの相互運用を前提とする。

### Capability、Action、Rule

CapabilityはFilesystem、Network、Device等を利用する実行能力、ActionはCapabilityを使う具体的作用、Ruleは将来のActionを評価するOwnerの方針である。Rule自体はActionを開始しない。

## 製品対象

- Desktop Bodyを持つWindowsおよびLinuxのHost／Client
- 日本語および英語の第一者UI
- Host、LAN、Cloudを組み合わせられるProvider構成
- 同じOwnerが利用する複数Clientと、複数の別個体Companion

正確なOS version、Linux distribution、display session、CPU architecture、hardware要件、利用可能なProviderとmodelはReleaseごとのSupport Matrixで定める。会話言語と音声品質は、選択したCharacter、LLM、STT、TTSにも依存する。

## 同梱Character

Eneは再配布可能なオリジナルCharacter `ene` を同梱する。Aliciaは開発・検証fixtureに限定し、製品配布へ含めない。

## 非目標

- 複数Owner向けの共有Coreや企業向けmulti-tenant SaaS
- Eneの永続状態をCloud Providerだけへ置くこと
- Companion体験と切り離された単用途Agent製品
- Tool call、Task Agent構成、内部推論を日常UIの主役にすること
- Ene独自の3D、Voice、Skill制作環境を一から提供すること
- Taskより上位の恒久的なProject container、Project lifecycle、成果物専用libraryを作ること
- 任意のCore改変や恒久的UI置換を許す汎用Plugin API
- Ene所有のMarketplace、relay、Cloud account、課金基盤を現在の製品範囲に含めること
- すべての構成で完全Offline推論を保証すること
- 内部思考、Raw画面、Raw音声、全Tool payloadを常時保存すること
- 既存config、IPC、Plugin protocol、保存形式、CLIとの互換性を維持すること

## 公開時に決める事項

公式な配布地域、対象年齢、年齢確認、地域法に基づくAI表示やContent policy、Marketplaceの提供は、公開計画を立てる時点で最新情報を再調査して決める。現時点では特定の結論を製品要件にしない。
