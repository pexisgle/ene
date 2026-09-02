# 用語集

状態: **Baseline / 対話による初版確定**
最終確認: 2026-09-02

ここでは、実装型やprotocol名ではなく、要件定義で使うDomain用語を定義する。用語の具体的なschema、配置、通信方式は別途決める。

## 製品と実行主体

### Ene

1人のユーザーのために、Companion、Memory、State、対話、PC作業、Permission、学習を統合する製品全体。

### Ene Core / Core

Host上で継続的に動作し、Companionの個体、永続状態、Scheduler、Context Monitor、Learning、Agent Harness、Permission等を管理する中核。

### Host

Ene CoreとEneの永続状態の正本を保持するコンピューター。現行のHost OS対象はWindowsとLinux。HostはClient未接続時にも許可された処理を継続できる。

### Client

Host上の同じEneへ接続する対話、表示、操作のインターフェース。現行のDesktop Client対象はWindowsとLinux。macOS、Mobile、Web Clientは将来対象。将来的には表示端末に限らず、Permissionの範囲で音声I/O、Observation、Body、Computer Use等のCapabilityをHostへ提供し得る。

### Companion

人格、意図、Emotion、Memory、Relationship、Interest、Body表現を持ち、ユーザーを自分なりに支援する継続的な個。Main LLMの一回のSessionやSub-agentとは異なる。

### Character Definition

Personality、Goals / Values、Role、Story / Lore、Avatar、Voice、感情表現など、Companionの配布可能な初期定義。

### Character Instance

特定のユーザー環境でCharacter Definitionをもとに動作するCompanion個体。ユーザーとのMemory、Emotion、Mood、Relationship、Interest、学習履歴を保持する。

### Character Package

Character Definition、Story / Lore、Avatar資産、Voice設定、感情表現、Role等をまとめたImport / Export単位。将来の配布・販売Marketplaceを想定するが、形式と流通は未確定。

### Main LLM / Main Companion

Companion自身の会話、解釈、発話、行動、委任を担う推論主体。Main LLMは常時実行せず、Event駆動で起動する。Main CompanionはLLMそのものだけでなく、CompanionとしてのStateと判断主体を指す場合がある。

### Agent Harness

Companionの意図を、PC・アプリケーション・Web・ファイル・外部サービス等に対する汎用作業へ変換して実行する仕組み。Harnessは製品の主役ではなく、Companionの内部能力である。

### Sub-agent

Companionから委任されたTaskを実行する一時的な作業主体。原則として独立した人格や長期Relationshipを持たず、意味のある進捗と最終まとめをCompanionへ返し、完了後は破棄される。

### Child Sub-agent

Sub-agentが必要に応じて生成する一時的な作業主体。現行要件では `Companion → Sub-agent → Child Sub-agent` までを許可し、Child Sub-agentの再帰的な生成は許可しない。

### Task

Companionが自分で行う、またはAgent Harnessへ委任する一つの目的を持った作業。実行中、待機中、承認待ち、完了、失敗、中断等の状態を持ち得る。

### Event

ユーザー入力、重要な観測、Schedule、Sub-agent報告、Companion間発話、状態変化等、Runtimeが主体の起動や処理を開始する契機。

### Schedule

指定時刻、定期予定、Reminder、将来再評価する時刻等を表す時間ベースの起動条件。

## Memoryと学習

### Memory

経験、事実、会話、関係、知識等を将来の想起・判断に利用するために保持する情報。単なるDBレコードの名称ではなく、種別、scope、provenance、confidence等を伴う意味上の単位。

### Working Memory

現在の会話、Task、直近の観測・文脈など、短期間の作業に使うMemory。

### Core Memory

Companionが常時または高頻度に参照する、少量で重要なMemory。通常の出来事を無制限に格納しない。

### Episodic Memory

「いつ、誰と、何が起きたか」という経験のMemory。

### Semantic Memory

経験や原資料から抽出された、比較的安定した事実・知識のMemory。

### Shared Memory

複数Companionが参照可能なscopeのMemory。Sharedであることは、すべてのCompanionがその出来事を直接経験したことを意味しない。

### Companion-specific Memory

特定Companionの経験、解釈、ユーザーとの関係、相手Companionとの出来事等に属するMemory。別Companionへ自動的に同じ経験として共有しない。

### Provenance

Memory、Skill、Decision等がどの会話、観測、Task、外部情報、推測、Import元から得られたかという由来。

### Confidence

情報や解釈の確からしさ。ユーザーの明示発言、直接観測、外部情報、Sub-agentの報告、Companionの推測等を同一のConfidenceとして扱わない。

### Emotional significance

あるMemoryがCompanionにとってどの程度感情的に重要だったかを示すMemory側の属性。現在のEmotion Stateそのものではない。

### Recall decay

時間経過、利用状況、関連性等によりMemoryの想起優先度が下がること。通常の忘却では物理削除を意味しない。

### Learning System

Experienceから将来に残す価値を判断し、Memory、Skill、Emotion / Mood、Relationship、Interest等へ反映し、結果を再評価する上位の仕組み。

### Learning Review

経験ごとに、Memory候補、Skill候補、State更新、または永続学習なしを判断する処理。

### Consolidation

蓄積されたExperienceやMemoryを、重複統合、Semantic抽出、矛盾整理、重要度・Confidence・scope・Recall priority再評価、Skill候補抽出等によって整理する処理。非緊急処理はアイドル時・Event駆動で行う。

### Skill

既存のTool、MCP、Plugin、Provider、Computer Use、Capabilityを、いつ・どのように使うかを表すProcedural Knowledge。Skill自身は新しい能力やPermissionを付与しない。

### Skill Manager

SkillのIndex、Retrieval、Import / Export、scope、Provenance、Revision、利用履歴、成功・失敗評価、自動生成・改善、Enable / Disableを管理するEne固有の管理層。

## Stateと個体差

### Companion State

Companionが継続的に持つEmotion、Mood、Relationship、Interest、意図、現在Context等の状態。Memoryとは別系統だが、想起・Appraisal・意思決定へ影響する。

### Emotion / Affective State

出来事に反応し、意思決定、発話、行動、Memory、Relationship等へ影響する短・中期的な内部状態。基礎軸としてValence、Arousal、Control / Dominanceを扱う方針だが、schemaと数値範囲は未確定。

### Appraisal

出来事の意味をCompanion自身のGoals、Values、Personality、Memory、Relationship等に照らして評価する処理。Novelty、Goal relevance、Goal conduciveness、Agency、Coping / Control、Norm compatibility等を含み得る。

### Baseline

Personality等に由来する、比較的安定した感情の基準状態。

### Mood

短期Affectより長く持続し、現在の解釈や反応の傾向に影響する状態。具体的な期間や更新式は未確定。

### Affect

意味のあるEventへ反応する短期的な感情状態。時間経過によるDecay、強度、Reappraisal等の影響を受け得る。

### Reactivity / Inertia / Recovery

それぞれ、Eventに反応する強さ、Stateが持続・変化しにくい傾向、BaselineやMoodへ回復する傾向を表すCompanionまたはCharacter固有の感情ダイナミクス。

### Reappraisal

出来事の原因、意味、結果、ユーザーの意図等に新しい情報が得られたとき、過去のAppraisalと関連Stateを再評価する処理。

### Semantic State

Emotion、Mood、Relationship、Interest等の内部数値・構造化Stateを、現在値、baselineとの差、変化、trend、原因となったEvent / Appraisal、Confidence等を保ちながら、Main LLMが理解しやすい意味表現へ変換したもの。Memory、Skill、Conversation、Observation / Context、TaskそのものをSemantic Stateへ含めることを意味しない。

### Companion Context

Main LLMを起動するときに、そのCompanionの判断に必要な情報だけを統合した文脈。Personality、Goals / Values、Semantic State、関連Memory、Skill、Conversation、Observation / Context、Task、Schedule、関連Appraisal等を必要に応じて組み合わせる。Semantic Stateとは別の上位コンテキストである。

### Relationship

Companionとユーザー、またはCompanion同士の長期的な関係状態。少なくともFamiliarity、Closeness、Trustを区別する。

### Familiarity

共有経験と相互理解をどの程度蓄積しているか。

### Closeness

相手がCompanionにとってどの程度心理的に近く重要な存在か。

### Trust

相手への信頼。必要に応じてPredictability、Dependability、Benevolent expectation等の側面を分ける。単一の好感度ではない。

### Interest

Companionが特定の話題、Entity、活動等へ持つ長期的な関心・嗜好・話題傾向。EmotionやRelationshipとは別である。

## 拡張、権限、観測

### Plugin

Ene Runtime / Architecture自体の拡張機構。Provider、Observation、Context、Memory処理、Trigger、Harness、Body、Event、管理UI等へ公開されたExtension Pointを通じて参加できる。Pluginであること自体は特権を意味しない。

### MCP

外部のToolまたはResourceをEneへ接続する手段。外部Tool接続だけで足りる場合は、Ene独自PluginよりMCPを優先する。

### Provider

LLM、VLM、Embedding、Reranker、STT、TTS等の処理を提供するLocalまたはCloudの実行バックエンド。コンポーネントごとに交換・選択できる。

### Tool

CompanionまたはAgentが呼び出して、外部状態の取得・変更や計算を行う個別の実行能力。ToolはPlugin、MCP、Harness等から提供され得るが、Tool自体がPermissionを付与しない。

### Observation

Desktop、OS、Window、アプリ、Clipboard、Schedule、通知、Task等から現在の状況を取得すること。Observationの許可はMemory保存、Main LLMへの伝達、Cloud送信の許可を意味しない。

### Context Monitor

継続的なObservationを低コストに整理し、重要な変化をCompanionごとに評価して通知する共有の軽量機構。最終的な発話・行動の意思決定主体ではない。

### Permission

Observation、Context処理、Main LLMへの伝達、Memory保存、Filesystem、Computer Use、Network、Credential、Cloud Egress、自律実行、通知、承認等を許可・拒否・保留する統一的な制御ポリシー。

### Delegation

ユーザーがCompanionへ目的や範囲を委任すること。自然言語の委任は、機械的なHard Boundaryを越える権限付与ではない。

### Sandbox / ACL / Broker

LLMやPluginの判断に依存せず、Filesystem、Network、Credential、Device、Capability等へのアクセスを機械的に制限・仲介するHard Boundaryの実現手段。具体方式は未確定。

### Data Egress

Host上のデータをCloud Provider、Remote Service、第三者Plugin等の外部へ送信すること。Localで観測可能であることとは別のPermission対象。

### Hard Boundary

LLM、Memory、Skill、Character、Plugin、自然言語ポリシーによって緩められない、システム側で強制する安全・権限・物理制約。

### Control plane / Data plane

Plugin等との通信における、要求・状態・権限・Lifecycle等の小さな制御情報と、画像・音声・Tensor等の大量データを分けて扱うという設計上の区別。具体的な方式は未確定。

## UIと可用性

### Companion UI

Desktopマスコット、音声、吹き出し、自然言語会話、Companionとしての発話・結果を中心とする日常UI。内部HarnessやSub-agentを主役にしない。

### 表層設定

一般ユーザー向けに、Character、Voice、Provider、基本Permission、自発性、Companion間交流等を設定するUI。

### 詳細設定・管理

Harness、Sub-agent、MCP、Plugin、詳細Permission、Context Monitor、Memory、Skill、ログ、診断、Provider利用、コスト等を確認・管理する第3層のUI。

### Graceful Degradation

一部機能が利用不能でも、失敗範囲を明示し、利用可能な機能を不必要に停止しないこと。STT/TTSはテキスト入力・表示で代替する。未設定の別Providerへ自動的に切り替えることを意味しない。

### Audit Log

会話、主体間通信、主要Action、Decision、Permission、Schedule、State変更、Provider利用、Plugin障害等を、後から確認できる意味のある履歴。LLMの逐語的な内部推論や全Raw観測の保存を意味しない。

### 推定コスト

Providerの実請求額を直接取得できない場合に、利用量と価格情報から算出した参考値。請求額と区別して表示する。
