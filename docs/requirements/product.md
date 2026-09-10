# 製品定義

状態: **再構成済みBaseline**

本書は、eneの目的、主要概念の意味と違い、製品対象、非目標を定義する。Ownerから観測できる必須挙動、条件、例外は[要件](requirements.md)だけに置き、本書は該当節を参照する。

## 製品の要約

eneは、一人のOwnerが管理するHost上で継続する、Companion中心のパーソナルAIシステムである。

日常的な体験の主役は、身体を持ち、同じ個として時間を越えて続くCompanionである。会話、Memory、Relationship、Companion State、身体表現と、実際のPC作業を一つの存在から利用できる。

eneはCompanionを体験上の中心に置くが、汎用作業Agentとしての能力を劣後させない。Tool、MCP、Skill、Provider、Task AgentはCompanionが使う能力であり、別の主役や劣化した付属機能ではない。

## 利用者と実行場所

- Owner: 一つのene環境を所有し管理する一人の利用者。複数人の独立した利用者を同じ環境へ収容しない。
- Host: ene Coreを実行し、eneが管理する永続状態の正本と継続実行を担うOwner管理下のPC。
- Client: 同じHostへ接続し、Companionが存在してOwnerと対話し、そのPCの画面・device・Computer Use等と関わる場所。表示・会話・操作の入口であり、永続状態の正本ではない。Hostと同じPCにも別PCにも置ける。
- 推論先: Host、OwnerのLAN、またはOwnerが選んだCloud Provider。

所有、継続、接続の契約は[所有と実行](requirements.md#所有と実行)と[Remote Client](requirements.md#remote-client)に定める。

## 主要概念

### Companion

Characterを基に作られ、Memory、Relationship、Companion State、設定をExperienceから形成する継続的な個体。同じCharacterから作ったCompanionも別個体である。Ownerとの会話、判断、Taskの開始・委任・調整、結果の統合の中心であり、まとまった実作業を本体で直接抱える主体ではない。

Running Companionは基本的に同時に一つのClientにだけ存在し、Body、会話、Voice、Observationとの関係、自発的interaction、Computer Useはその存在場所に結び付く。active Clientがない間もHost正本で同じ個体として存続する。Stopped Companionはデータを保持して再開できるが、どのClientにもHostにもpresenceを持たない。存在場所、移動、停止、削除の契約は[Remote Client](requirements.md#remote-client)と[停止と削除](requirements.md#停止と削除)に定める。

### Character

配布可能な静的人格、設定、Body、Voice、表現、推奨Skillの組み合わせ。特定Ownerとの経験や関係は含まない。Character Packageはそのimport/export単位である（[Character Package](requirements.md#character-package)）。

### Experience

CompanionがOwnerとの対話、Task、Tool利用、Observation、他のCompanionとの交流その他の活動を通じて経験した出来事とその結果。Memory、Skill、Relationship、Companion State等が形成または更新される根拠になり得るが、Raw dataの恒久保存を意味しない。

Experience Summaryは、複数messageや一連の行動を意味的なまとまりとして要約した、長期状態を形成・説明するための圧縮された根拠である。Raw Historyの代替でも独立した知識の正本でもない（[ExperienceとExperience Summary](requirements.md#experienceとexperience-summary)）。

### Task

開始、進行、判断待ち、完了、失敗、Cancel等の状態を追跡する作業単位。Ownerの依頼によるものとCompanionが自発的に始めるものを含む。Taskは実行主体そのものではない。

### Task Agent

CompanionからTaskまたはその一部を委任された一時的な実行主体。独立した長期人格やRelationshipを持たず、委任元Companionの権限とTask境界内で作業し、結果を返す。まとまった作業は原則Task Agentへ委任するが、Companion自身の軽微な処理まで一律に委任するものではない（[Task](requirements.md#task)）。

### ObservationとObserver

ObservationはClientの画面やComputerの操作状況を観測する活動である。ObserverはClientに紐づく特殊な共有主体であり、Capture・候補検知を共有し、文脈上関係がありそうなCompanionへeventを伝える役割を指す。CompanionやTask Agentではなく、Companionごとの自発性とは異なる制御単位である。専用processや内部subsystemを意味しない（[Observation](requirements.md#observation)）。

### Workspace

Taskが作業対象として利用するfolder、file、外部source等との関連付け。Taskより上位の独立containerやデータ所有主体ではなく、Taskに従属する作業場である（[Workspace](requirements.md#workspace)）。

### MemoryとSkill

Experienceから形成され得るLearning。Memoryは出来事、事実、意味、好み等を後の理解に用いる現在の認識であり、一般世界知識やRaw Historyの保存領域ではない。Skillは将来の類似Taskで再利用できる手順、専門知識、実行上の注意、補助resource等をまとめたLearningで、Agent Skillsとの相互運用を前提とする。

Memoryは後のExperienceによって継続的に更新される。通常の忘却は削除ではなく想起の抑制であり、Privacy/Security目的でOwnerが明示したtargeted deletionだけが例外的な消去である。特定CompanionとのExperienceから形成されたLearningはCompanion scopeを既定とし、Global scopeは複数Companionで共通に利用すべきことが明確な場合に限る。保持、訂正、scope、容量管理、targeted deletionの契約は[MemoryとSkill](requirements.md#memoryとskill)、[Scope](requirements.md#scope)、[重要度、忘却、訂正](requirements.md#重要度忘却訂正)、[Privacy/Security目的のtargeted deletionと履歴保持](requirements.md#privacysecurity目的のtargeted-deletionと履歴保持)、[Learningと根拠の容量管理](requirements.md#learningと根拠の容量管理)に定める。

### Relationship

あるCompanionとその相手（Ownerまたは別のCompanion）との共有Experienceから形成される、そのCompanion自身による現在の関係認識。Companion側の個体固有状態であり、相手側の認識と自動的に同一または対称にはならない。Memoryの補助となるcompactな状態であり、事実の詳細を第二のMemoryとして複製しない（[Relationship](requirements.md#relationship)）。

### Companion State

Experienceから形成される、あるCompanion自身の現在の内的状態と振る舞いの傾向。感情、関心、人格上の傾向等を含み得る。Memoryが出来事・事実についての認識、Relationshipが相手との関係についての解釈を扱うのに対し、Companion Stateはそれらを現在の表現、注意、会話や行動の傾向へどう反映するかを扱う。一時的な状態と比較的持続的な傾向があり得る（[Companion State](requirements.md#companion-state)）。

### Capability、Action、Rule

CapabilityはFilesystem、Network、Device等を利用する実行能力、ActionはCapabilityを使う具体的作用、Ruleは将来のActionを評価するOwnerの方針である。Rule自体はActionを開始しない（[Permissionと安全境界](requirements.md#permissionと安全境界)）。

## 製品対象

- Desktop Bodyを持つWindowsおよびLinuxのHost／Client
- 日本語および英語の第一者UI
- Host、LAN、Cloudを組み合わせられるProvider構成
- 同じOwnerが利用する複数Clientと、複数の別個体Companion

正確なOS version、Linux distribution、display session、CPU architecture、hardware要件、利用可能なProviderとmodelはReleaseごとのSupport Matrixで定める。会話言語と音声品質は、選択したCharacter、LLM、STT、TTSにも依存する。

## 同梱Character

eneは再配布可能なオリジナルCharacter `ene` を同梱する。Aliciaは開発・検証fixtureに限定し、製品配布へ含めない。

## 非目標

- 複数Owner向けの共有Coreや企業向けmulti-tenant SaaS
- eneの永続状態をCloud Providerだけへ置くこと
- Companion体験と切り離された単用途Agent製品
- Tool call、Task Agent構成、内部推論を日常UIの主役にすること
- ene独自の3D、Voice、Skill制作環境を一から提供すること
- Taskより上位の恒久的な作業containerや独自lifecycle、成果物専用libraryを作ること
- 任意のCore改変や恒久的UI置換を許す汎用Plugin API
- ene所有のMarketplace、relay、Cloud account、課金基盤を現在の製品範囲に含めること
- すべての構成で完全Offline推論を保証すること
- 内部思考、Raw画面、Raw音声、全Tool payloadを常時保存すること
- 既存config、IPC、Plugin protocol、保存形式、CLIとの互換性を維持すること

## 公開時に決める事項

公式な配布地域、対象年齢、年齢確認、地域法に基づくAI表示やContent policy、Marketplaceの提供は、公開計画を立てる時点で最新情報を再調査して決める。現時点では特定の結論を製品要件にしない。
