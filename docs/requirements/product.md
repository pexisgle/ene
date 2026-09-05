# 製品定義

状態: **再構成済みBaseline**
最終確認: 2026-09-06

## 製品の要約

eneは、一人のOwnerが管理するHost上で継続する、Companion中心のパーソナルAIシステムである。

日常的な体験の主役は、身体を持ち、同じ個として時間を越えて続くCompanionである。会話、Memory、Relationship、Companion State、身体表現と、実際のPC作業を一つの存在から利用できる。

eneはCompanionを体験上の中心に置くが、汎用作業Agentとしての能力を劣後させない。Tool、MCP、Skill、Provider、Task AgentはCompanionが使う能力であり、別の主役や劣化した付属機能ではない。

## 利用者と実行場所

- 一つのene環境は一人のOwnerに属する。複数人の独立した利用者を同じ環境へ収容しない。
- Hostはene Coreを実行し、eneが管理する永続状態の正本と継続実行を担うOwner管理下のPCである。通常のTask・Task Agentの作業は、Computer Use等のClient依存部分を除き、CompanionのClient間移動から独立してHost上で継続する。
- Clientは同じHostへ接続し、Companionが存在してOwnerと対話し、そのPCの画面・device・Computer Use等と関わる場所である。表示・会話・操作の入口であり、永続状態の正本ではない。Hostと同じPCにも別PCにも置ける。
- 推論はHost、OwnerのLAN、またはOwnerが選んだCloud Providerで実行できる。

## 主要概念

### Companion

Characterを基に作られ、Memory、Relationship、Companion State、設定をExperienceから形成する継続的な個体。同じCharacterから作ったCompanionも別個体である。

基本的に同時に一つのClientへ存在し、Body、Realtime／Text会話、Voice、ambient Observationとの関係、自発的interaction、Computer Useがその存在場所に結び付く。必要に応じてClient間を移動できる。active Clientがない間もHost正本で同じ個体として存続し、Clientに依存する対話・身体・操作は行わず、伝えられなかった事項は次に移動したClientでまとめて報告する。Ownerとの継続的な会話、判断、Taskの開始・委任・調整、steering、結果の受領・統合の中心であり、多数のまとまった実作業を本体で直接抱えることを基本にしない。

### Character

配布可能な静的人格、設定、Body、Voice、表現、推奨Skillの組み合わせ。特定Ownerとの経験や関係は含まない。Character Packageはそのimport/export単位である。

### Experience

CompanionがOwnerとの対話、Task、Tool利用、Observation、他のCompanionとの交流その他の活動を通じて経験した出来事とその結果。ExperienceはMemory、Skill、Relationship、Companion State等が形成または更新される根拠になり得る。Experienceであることは、そのRaw dataを恒久保存することを意味しない。

Memory、Skill、Relationship、比較的持続するCompanion State等へ利用するときは、必要に応じて複数messageや一連の行動を、topic、出来事、Task、意思決定、共有体験等の意味的なまとまりとして要約したExperience Summaryを根拠にできる。Experience SummaryはRaw Historyの代替でも独立した知識の正本でもなく、長期状態を形成・説明するための圧縮された根拠である。

### Task

開始、進行、判断待ち、完了、失敗、Cancel等の状態を追跡する作業単位。Ownerの依頼によるものとCompanionが自発的に始めるものを含む。ある程度まとまった作業は基本的にTaskとして扱い、実行は原則Task Agentへ委任する。Taskは実行主体そのものではない。

### Task Agent

CompanionからTaskまたはその一部を委任された一時的な実行主体。独立した長期人格やRelationshipを持たず、委任元Companionの権限とTask境界内で作業し、結果を返す。Companion自身の軽微な処理や判断のための情報取得まで、一律にTask Agentへ委任するものではない。

### ObservationとObserver

ObservationはClientの画面やComputerの操作状況を観測する活動である。Observerは、そのCapture・候補検知をClient単位で共有し、文脈上関係がありそうなCompanionへeventを伝える役割を指す。Companionごとの自発性とは異なる制御単位であり、専用processや内部subsystemを意味しない。

### Workspace

Taskが作業対象として利用するfolder、file、外部source等との関連付け。WorkspaceはTaskより上位の独立containerやデータ所有主体ではなく、Taskに従属する作業場である。Taskとの関連が削除されても、外部のfileやsource自体は削除しない。

### MemoryとSkill

Experienceから形成され得るLearning。Memoryは出来事、事実、意味、好み等を後の理解に用いる現在の認識であり、一般世界知識やRaw Historyの保存領域ではない。Memoryは後のExperienceによって内容、重要度、scope、時間的意味等を継続的に更新できる。通常の忘却、訂正、失効、置換、統合では保存済みMemoryやその過去revision・根拠を削除せず、現在の認識や重要度、想起優先度等を更新する。OwnerがPrivacyまたはSecurityのため特定情報そのものをene内部から強制消去するよう明示したtargeted deletionだけは、この保持原則より優先する。MemoryはOwner、Companion、出来事、状況等についての長期的な理解ではRelationshipより優先される主要な知識状態である。Skillは将来の類似Taskで再利用できる手順、専門知識、実行上の注意、補助resource等をまとめたLearningで、Agent Skillsとの相互運用を前提とする。

ene内部のMemoryとSkillはCompanionまたはGlobalのscopeを持ち得る。特定CompanionとのExperienceから形成されたLearningはCompanion scopeを既定とし、Ownerが明示的に共有を求めた場合、または内容と文脈から複数Companionで共通に利用すべきことが明確な場合だけGlobal scopeにする。単に重要、有用、一般的な好みであることだけを理由にGlobal scopeへ昇格させない。

### Relationship

Relationshipは、あるCompanionとその相手であるOwnerまたは別のCompanionとの共有Experienceから形成される、そのCompanion自身による現在の関係認識である。各RelationshipはCompanion側の個体固有状態であり、相手側の認識と自動的に同一または対称にはしない。

RelationshipはMemoryの補助となるcompactな状態であり、人物情報、出来事、Preference等の詳細を第二のMemoryとして複製しない。必要な事実はMemoryやExperience Summaryを参照し、Relationship自身は距離感、交流傾向、関係の変化等、関係そのものの現在の解釈へ絞る。Memoryと矛盾する事実認識をRelationshipで上書きせず、Permission、Rule、安全境界にも影響しない。

### Companion State

Companion Stateは、Experienceから形成される、あるCompanion自身の現在の内的状態と振る舞いの傾向である。感情、関心、Experienceによって形成された人格上の傾向等を含み得る。

Memoryが出来事、事実、意味等についての現在の認識を扱い、Relationshipが相手との関係についての現在解釈を扱うのに対し、Companion Stateは、それらを現在の表現、注意、会話や行動の傾向へどう反映するかを扱う。MemoryやRelationshipの内容を第二の正本として複製しない。

Companion Stateには、最近のExperienceや時間経過によって変化する一時的な状態と、Experienceの蓄積によって形成される比較的持続的な傾向があり得る。具体的な分類、dimension、内部表現、数値scale、更新・減衰方式は設計で定める。

### Capability、Action、Rule

CapabilityはFilesystem、Network、Device等を利用する実行能力、ActionはCapabilityを使う具体的作用、Ruleは将来のActionを評価するOwnerの方針である。Rule自体はActionを開始しない。

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
