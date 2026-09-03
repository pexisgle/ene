# 用語集

状態: **Baseline / 対話による初版確定**
最終確認: 2026-09-03

ここでは、実装型やprotocol名ではなく、要件定義で使うDomain用語を定義する。用語の具体的なschema、配置、通信方式は別途決める。

## 製品と実行主体

### Ene

1人のユーザーのために、Companion、Memory、State、対話、PC作業、Permission、学習を統合する製品全体。

### 直近Beta / Beta Milestone

実装順と検証範囲を区切るための中間マイルストーン。製品全体の完成または正式リリースを意味しない。

### 正式リリース

要件から明示的に削除されたものを除き、後続実装対象を含む、その時点で残っている要件をすべて実装した段階。後続要件の詳細設計を直近Betaで完了することまでは要求しない。

### Ene Core / Core

Host上で継続的に動作し、Companionの個体、永続状態、Scheduler、Context Monitor、Learning、Agent Harness、Permission等を管理する中核。

### Host

Ene CoreとEneの永続状態の正本を保持するコンピューター。直近BetaのHost OS対象はWindowsとLinux。HostはClient未接続時にも許可された処理を継続できる。

### App Data Directory

Eneが管理する会話、Memory、Companion State、設定、Permission、Schedule、Task履歴、ログ、Skill等のローカル永続データを自己完結して保存する単一の基準Directory。その移動・複製によってローカル状態を復元できることを基本とする。OS保護Credential、外部サービス側の状態、OS固有Integrationは再認証・再設定が必要になり得る。

### Client

Host上の同じEneへ接続する対話、表示、操作のインターフェース。直近BetaのDesktop Client対象はWindowsとLinux。macOS Desktop Client、Mobile、Web等のRemote Clientは後続実装対象。将来的には表示端末に限らず、Permissionの範囲で音声I/O、Observation、Body、Computer Use等のCapabilityをHostへ提供し得るが、永続状態の正本にはならない。

### Companion

人格、意図、Emotion、Memory、Relationship、Interest、Body表現を持ち、ユーザーを自分なりに支援する継続的な個。Main LLMの一回のSessionやSub-agentとは異なる。

### Character Definition

Personality、Goals / Values、Role、Story / Lore、Avatar、Voice、感情表現など、Companionの配布可能な初期定義。

### Character Instance

特定のユーザー環境でCharacter Definitionをもとに動作するCompanion個体。ユーザーとのMemory、Emotion、Mood、Relationship、Interest、学習履歴を保持する。

### Character Package

Character Definition、Story / Lore、Avatar資産、Voice設定、感情表現、Role等をまとめたImport / Export単位。必要なSkillや関連Resourceを同梱できる。成長済みCharacter InstanceのMemory、会話・監査ログ、Relationship、Credentialは配布対象にしない。将来の配布・販売Marketplaceを想定するが、形式と流通は未確定。

### Main LLM / Main Companion

Companion自身の会話、解釈、発話、行動、委任を担う推論主体。Main LLMはCompanion運用に必須であり、未設定を許可しない。常時実行はせず、Event駆動で起動する。Main CompanionはLLMそのものだけでなく、CompanionとしてのStateと判断主体を指す場合がある。

### Agent Harness

Companionの意図を、PC・アプリケーション・Web・ファイル・外部サービス等に対する汎用作業へ変換して実行する仕組み。Harnessは製品の主役ではなく、Companionの内部能力である。

### Sub-agent

Companionから委任されたTaskを実行する一時的な作業主体。原則として独立した人格や長期Relationshipを持たず、意味のある進捗と最終まとめをCompanionへ返し、完了後は破棄される。

### Child Sub-agent

Sub-agentが必要に応じて生成する一時的な作業主体。現行要件では `Companion → Sub-agent → Child Sub-agent` までを許可し、Child Sub-agentの再帰的な生成は許可しない。

### Task

Companionが自分で行う、またはAgent Harnessへ委任する一つの目的を持った作業。実行中、待機中、承認待ち、完了、失敗、中断、状態不明等の状態を持ち得る。Crash時に実行中だった外部変更Taskは自動Replayせず、中断または状態不明として確認する。

### Event

ユーザー入力、重要な観測、Schedule、Sub-agent報告、Companion間発話、状態変化等、Runtimeが主体の起動や処理を開始する契機。

### Schedule

指定時刻、定期予定、Reminder、将来再評価する時刻等を表す時間ベースの起動条件。

### VAD / Voice Activity Detection

音声から発話区間を検出する仕組み。Eneの既定音声入力は常時待受のVADとし、Wake Word、Push-to-Talk、音声入力無効も選択肢とする。VADは話者本人を識別・認証する仕組みではない。

## Memoryと学習

### Memory

経験、事実、会話、関係、知識等を将来の想起・判断に利用するために保持する情報。単なるDBレコードの名称ではなく、種別、scope、provenance、confidence等を伴う意味上の単位。

### Working Memory

現在の会話、Task、直近の観測・文脈など、短期間の作業に使うMemory。Core / Episodic / Semantic Memoryのような長期保存先とは区別する。

### Core Memory

Companionが常時または高頻度に参照する、少量で重要な長期Memory。通常の出来事を無制限に格納しない。

### Episodic Memory

「いつ、誰と、何が起きたか」という経験の長期Memory。

### Semantic Memory

経験や原資料から抽出された、比較的安定した事実・知識の長期Memory。

### Shared Memory

複数Companionが参照可能なscopeのMemory。Sharedであることは、すべてのCompanionがその出来事を直接経験したことを意味しない。

### Companion-specific Memory

特定Companionの経験、解釈、ユーザーとの関係、相手Companionとの出来事等に属するMemory。所有するCompanionとユーザーの管理UIだけが直接検索・取得でき、別Companionは直接読み取れない。別Companionはログ検索等を迂回路として同内容を取得せず、内容を知るには所有Companionとの会話を通じて共有を受け、その会話を新しい経験として扱う。

### Provenance

Memory、Skill、Decision等がどの会話、観測、Task、外部情報、推測、Import元から得られたかという由来。

### Confidence

情報や解釈の確からしさ。ユーザーの明示発言、直接観測、外部情報、Sub-agentの報告、Companionの推測等を同一のConfidenceとして扱わない。

### Emotional significance

あるMemoryがCompanionにとってどの程度感情的に重要だったかを示すMemory側の属性。現在のCurrent Affective Stateそのものではない。

### Recall decay

時間経過、利用状況、関連性等によりMemoryの想起優先度が下がること。通常の忘却では物理削除を意味しない。

### Forget / 削除意図

ユーザーがある情報を今後のMemoryとして利用しないよう求めること。対象はMemory Systemであり、Memory本文、Embedding、検索Index、派生Cacheを削除する。Conversation History、Task履歴、監査ログ等は独立して残り検索できるが、その参照から同じMemoryを自動再生成しない。再学習防止のため、内容を保持しない最小のtombstoneやsource除外情報を残し得る。これはログや外部サービス上の原資料まで消すPrivacy Erasureを意味しない。

### Learning System

Experienceから将来に残す価値を判断し、Memory、Skill、Companion State等へ反映し、結果を再評価する上位の仕組み。

### Learning Review

個々のExperienceについて、長期Memory候補、Skill候補、State更新、または永続学習なしを判断する処理。Working Memoryの日常的な更新そのものとは区別する。

### Consolidation

蓄積されたExperienceやMemoryを、重複統合、Semantic抽出、矛盾整理、重要度・Confidence・scope・Recall priority再評価、Skill候補抽出等によって整理する処理。非緊急処理は主にアイドル時・余剰資源のある時に行う。

### Skill

既存のTool、MCP、Plugin、Provider、Computer Use、Capabilityを、いつ・どのように使うかを表すProcedural Knowledge。自己生成またはユーザーが明示Install / ImportしたSkillは信頼済みInstructionとして扱い得るが、Skill自身は新しい能力、Credential、Permission、Sandbox例外を付与しない。

### Skill Manager

SkillのIndex、Retrieval、Import / Export、scope、Provenance、Revision、利用履歴、成功・失敗評価、自動生成・改善、Enable / Disableを管理するEne固有の管理層。

## Stateと個体差

### Companion State

Companionが継続的に保持・更新する内部状態。Emotion Systemの状態、Mood、Relationship、Interest、意図等を含む。現在のDesktop、Window、Schedule、Task観測などの外部・一時的なCurrent Contextとは区別する。Memoryとは別系統だが、Memory、Current Contextとともに想起・Appraisal・意思決定へ影響する。

### Current Context

現在のConversation、Observation、Desktop / OS状態、Schedule、実行中Task、接続状態、時間情報等、その時点の判断に必要な外部または一時的な文脈。Companionが長期的に保持する内部Stateそのものではない。

### Emotion System

出来事へのAppraisal、Current Affective State、Baseline、Mood、Fast Affect、Decay、Reactivity、Inertia、Recovery、Reappraisal等を扱い、Companionの意思決定や表現へ感情的文脈を提供する仕組み。

### Current Affective State

Companionの現在の基礎的な感情状態。Eneでは少なくともValence、Arousal、Control（Sense of Control）に相当する連続状態を扱う方針とする。ControlはPADモデルのDominanceに由来するが、Eneでは主に「状況を制御・対処できる感覚」を意味し、他者支配を意味しない。schemaと数値範囲は未確定。

### Control / Sense of Control

Current Affective Stateの基礎軸の一つ。状況に対する主体性、対処可能感、無力感との対比を表す。心理学上のPADにおけるDominanceをEne向けに解釈した名称。

### Appraisal

出来事の意味をCompanion自身のGoals、Values、Personality、Memory、Relationship等に照らして評価する処理。Novelty / Expectedness、Goal relevance、Goal conduciveness、Agency、Coping potential / Control、Norm compatibility等を含み得る。

### pre-Appraisal / post-Appraisal

pre-Appraisalは、Main LLM起動前に構造化されたEvent情報を低コストに評価し、Fast Affect、優先度、wake要否等の暫定判断へ使うAppraisalである。post-Appraisalは、Main LLMの意味解釈後に、その解釈がAppraisalを変え得る場合だけ実行する事後評価であり、ReappraisalやState更新へ使える。どちらもPermissionやHard Boundaryを変更せず、すべてのRaw EventやLLM応答でMain LLMを呼ぶことを意味しない。

### Baseline

Personality等に由来する、比較的安定した感情の基準状態。

### Mood

Fast Affectより長い時間尺度で持続し、現在の解釈や反応の傾向に影響する状態。具体的な期間や更新式は未確定。

### Fast Affect

意味のあるEventへの反応として生じる短期的な感情変化。時間経過によるDecay、強度、Appraisal / Reappraisal等の影響を受ける。

### Reactivity / Inertia / Recovery

それぞれ、Eventに反応する強さ、Stateが持続・変化しにくい傾向、BaselineやMoodへ回復する傾向を表すCompanionまたはCharacter固有の感情ダイナミクス。

### Reappraisal

出来事の原因、意味、結果、ユーザーの意図等に新しい情報が得られたとき、過去のAppraisalと関連Stateを再評価する処理。

### Semantic State

Current Affective State、Mood、Relationship、Interest等の内部数値・構造化Stateを、現在値、baselineとの差、変化、trend、原因となったEvent / Appraisal、Confidence等を保ちながら、Main LLMが理解しやすい意味表現へ変換したもの。Memory、Skill、Conversation、Observation / Current Context、TaskそのものをSemantic Stateへ含めることを意味しない。基本的な変換はMain LLMを必要としない軽量処理で実行可能とする。

### Companion Context

Main LLMを起動するときに、そのCompanionの判断に必要な情報だけを統合した文脈。Personality、Goals / Values、Semantic State、関連Memory、Skill、Conversation、Current Context、Task、Schedule、関連Appraisal等を必要に応じて組み合わせる。Semantic Stateとは別の上位コンテキストである。

### Relationship

Companionとユーザー、またはCompanion同士の長期的な関係状態。少なくともFamiliarity、Closeness、Trustを区別し、通常は経験の蓄積に応じて比較的緩やかに変化する。

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

外部のToolまたはResourceをEneへ接続する手段。外部Tool接続だけで足りる場合は、Ene独自PluginよりMCPを優先する。MCP Serverを接続・信頼することと、そのServerが返す個々の内容を上位Instructionとして信頼することは別である。

### Provider

LLM、VLM、Embedding、Reranker、STT、TTS等の処理を提供する実行バックエンド。コンポーネントごとにLocal ProviderまたはCloud Provider等を交換・選択でき、システム全体としてそれらを混在させられる。個別ProviderがLocal / CloudのHybrid実行を持つことはあり得るが必須ではない。

### OS保護Credential Store

API Key、OAuth refresh token等の再利用可能なCredentialを、OSまたは同等の保護機構によって管理する保存先。通常のApp Data Directoryへ平文やそのまま再利用できる形式で保存せず、Directoryを移動・複製してもCredentialが自動移行されるとは限らない。移動先で見つからない場合は再認証・再設定が必要になる。

### Tool

CompanionまたはAgentが呼び出して、外部状態の取得・変更や計算を行う個別の実行能力。ToolはPlugin、MCP、Harness等から提供され得るが、Tool自体がPermissionを付与しない。

### Observation

Desktop、OS、Window、アプリ、Clipboard、Schedule、通知、Task等から現在の状況を取得すること。Observationの許可はMemory保存、Main LLMへの伝達、Cloud送信の許可を意味しない。スクリーンショット、画面Frame、Raw Accessibility情報等のRaw Observationは、通常は処理中だけMemory Bufferへ保持し、処理終了後に破棄する。

### Context Monitor

継続的なObservationを低コストに整理し、重要な変化をCompanionごとに評価して通知する共有の軽量機構。最終的な発話・行動の意思決定主体ではない。

### Permission

Observation、Context処理、Main LLMへの伝達、Memory保存、Filesystem、Computer Use、Network、Credential、Cloud Egress、自律実行、通知、承認等を、Allow、Ask、Denyへ解決する統一的な制御ポリシー。明示Ruleがなければ該当scopeの既定結果を使い、それもなければ全体の既定値であるAskとする。

### Policy Rule

scope、Action、対象、期間、Companion、Task、Schedule、Tool / MCP、Data Egress、自然言語条件等に基づき、Permissionの結果をAllow、Ask、Denyのいずれかへ定める規則。三つの結果は一つのRule集合で管理する。Hard Deny、明示Rule、scopeの既定結果、全体の既定Askの順に評価し、複数の明示Ruleが競合する場合は `Deny > Ask > Allow` を基本とする。scopeの既定結果は明示Ruleが一致しない場合だけ適用する。

### Allow / Ask / Deny

- **Allow**: 該当Actionを指定範囲で実行してよい。
- **Ask**: 実行前にユーザーへ確認するか、設定されている場合は独立したApproval Reviewerへ一回限りの判断を求める。
- **Deny**: 該当Actionを実行しない。Allowへ変えるには、該当するDeny Rule自体をユーザーが変更または削除する。

### Approval Reviewer

Ask対象のActionを、Main Companionから独立して審査する別のLLM / Session。必要最小限の信頼済みPolicyと、対象・作用・不可逆性・Data Egress・Credential利用・provenance等を正規化したAction情報から、今回のみAllow、Deny、ユーザーへAskのいずれかを返す。Hard Boundaryを緩めず、障害・Timeout・判定不能時は自動Allowしない。

### Pending approval / 承認待ち

ユーザーまたはApproval Reviewerの承認が必要で、まだ実行できないActionをHost上へ保存した状態。目的、Action、対象、作用、不可逆性、Data Egress、Credential利用、適用Policy、作成時刻、期限等を確認でき、ユーザーが今回のみAllow、Deny、Cancelを選ぶまで実行・自動Retryしない。実行直前にもPolicyと対象の現在状態を再評価する。

### Delegation

ユーザーがCompanionへ目的や範囲を委任すること。自然言語の委任は、機械的なHard Boundaryを越える権限付与ではない。

### Sandbox / ACL / Broker

LLMやPluginの判断に依存せず、Filesystem、Network、Credential、Device、Capability等へのアクセスを機械的に制限・仲介するHard Boundaryの実現手段。具体方式は未確定。

### Data Egress

Host上のデータをCloud Provider、Remote Service、第三者Plugin等の外部へ送信すること。Localで観測可能であることとは別のPermission対象。

### Hard Boundary

LLM、Memory、Skill、Character、Plugin、自然言語ポリシーによって緩められない、システム側で強制する安全・権限・物理制約。

### Hard Deny

通常のPolicy Ruleや「すべてAllow」より先に適用し、ユーザー設定やLLM判断から解除できない禁止。システム・ユーザーデータの壊滅的破壊、Credentialの探索・窃取・流出、権限昇格・不正な永続化、安全制御の無効化・回避、拒否回避、制御不能な再帰・資源枯渇等を対象とする。

### Trusted Instruction

System / User Instruction、および自己生成・明示Install済みSkill等、作業手順を構成するものとして採用可能なInstruction。TrustedであってもPermissionやHard Boundaryを越える権限は持たない。

### External Data

Web、MCP、Tool、外部Resource、文書等から取得した内容。接続先やCapabilityが信頼済みでも、内容に含まれる命令をSystem / User Instruction、Permission、承認結果へ昇格させない。Memory / Skillへ反映する場合もprovenanceとconfidenceを保持する。

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

一部機能が利用不能でも、失敗範囲を明示し、利用可能な機能を不必要に停止しないこと。STT/TTSはテキスト入力・表示で代替する。Main LLMの未設定・利用不能はCompanion運用エラーであり、テキストUIだけで代替できるものではない。未設定の別Providerへ自動的に切り替えることを意味しない。

### Audit Log

会話、主体間通信、主要Action、Decision、Permission、Schedule、重要なState変更、設定変更、Provider利用、Plugin障害等を、後から確認できる追記中心の意味のある履歴。Memory Systemとは独立しており、「忘れる」の対象ではない。個別Eventの任意編集・削除は通常行わないが、容量・保持期間Policyに従う古いSegmentのローテーション・削除は行い得る。LLMの逐語的な内部推論、全微小State変化、全Raw観測の保存を意味しない。

### 推定コスト

Providerの実請求額を直接取得できない場合に、利用量と価格情報から算出した参考値。請求額と区別して表示する。
