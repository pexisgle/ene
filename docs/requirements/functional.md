# 機能要件

状態: **Baseline / 対話による初版確定**
最終確認: 2026-09-02

ここでは、Eneが提供すべき機能、システム上の必須能力・制約、および外部から観測できる挙動を定義する。要件を満たすためのcrate名、DB schema、IPC protocol、モデル名、アルゴリズムは固定しない。`将来` と付いた要件は、現行版の必須範囲ではないが、将来追加できる構造を妨げてはならない。

## 1. Core、Host、ライフサイクル

### FR-CORE-001 Ene Core

Eneは、Companion、永続状態、Scheduler、Context Monitor、Learning、Agent Harness、Permissionを統合して管理するEne Coreを持たなければならない。

### FR-CORE-002 単一ユーザー境界

1つのEne環境は、原則として1人のユーザーのための環境として扱わなければならない。複数人を独立したテナントとして同じCoreへ収容することは現行の製品能力に含めない。

### FR-CORE-003 Hostの正本

Host上のEne Coreは、会話履歴、Memory、Companion State、Character Instance、設定、Permission、Task履歴、成果物その他のEne永続状態を管理しなければならない。

### FR-CORE-004 Clientからの接続

複数のClientは、同じHost上の同じCompanionと会話・操作できなければならない。ClientごとにCompanionの別コピーや別の正本を作ってはならない。

### FR-CORE-005 Client非接続時の継続

Clientが閉じている、またはどのClientも接続していない場合でも、Host上のCoreは、許可されたSchedule、Task、Learning、Companion間活動、自律処理を継続できなければならない。

### FR-CORE-006 イベント駆動のMain LLM

Companionは概念上常に存在するが、Main LLMを常時実行してはならない。ユーザー入力、重要なContext Monitorイベント、Schedule、Sub-agentまたは他Companionの意味のある報告、許可された自発起動などを契機に推論を開始できなければならない。

### FR-CORE-007 待機の扱い

外部イベント、Schedule、Sub-agentの進捗を待つためだけにMain LLMのターンを消費してはならない。待機はRuntimeが管理し、状態変化または明示されたイベントが発生したときに必要な主体を起動しなければならない。

### FR-CORE-008 時間・状況メタ情報

Companionが自分のLLMだけでは直接把握しにくい現在日時、経過時間、作業継続時間、次の予定までの時間、接続状態、実行中Task、直近イベント等を、関連する場合にContextとして受け取れるようにしなければならない。

### FR-CORE-009 自発起動源

Eneは、ユーザー入力、Context Monitor、Schedule、Sub-agent報告、Companion間通信、許可されたランダムまたは確率的な自発起動を、Main LLM起動の契機として扱えるようにしなければならない。

### FR-CORE-010 自発性の設定

ユーザーは、Companionが観測、発話、内部作業、外部状態を変更する作業をどの程度自発的に行えるかを設定できなければならない。

## 2. Companion、Character、複数個体

### FR-CMP-001 Companionの個体性

Companionは、単なるLLMセッション、表示テーマ、または人格の切替モードではなく、継続する個体として扱わなければならない。人格、意図、感情、Memory、Relationship、Interest、現在のTaskを次回の起動でも適切に引き継げなければならない。

### FR-CMP-002 Character Definition

Companionの配布可能なCharacter Definitionは、少なくともPersonality、Goals / Values、Role、嗜好、話し方、Story / Lore、Avatar、Voice、感情表現、自発性、得意分野等を定義できなければならない。

### FR-CMP-003 Character Instance

Eneは、Character Definitionから特定ユーザーの環境で動作するCharacter Instanceを作成しなければならない。Instanceは、そのユーザーとのMemory、Emotion、Mood、Relationship、Interest、学習履歴を持ち、Definitionの配布元と分離して扱わなければならない。

### FR-CMP-004 明確な個としての判断

Companionは、ユーザーの指示をそのまま反復するだけでなく、Personality、Goals、Values、Memory、Emotion、Relationship、Interestに基づいて、賛成、反対、躊躇、好み、優先順位、発話、行動を自分なりに判断できなければならない。どこまで反対・拒否するかはCharacter設定で調整できなければならない。

### FR-CMP-005 複数Companion

1つのEne Core上に複数のCompanionを作成・保持・同時表示できなければならない。各Companionは、性格、役割、Avatar、Voice、Emotion、Mood、Interest、専用Memory、ユーザーとのRelationship、他CompanionとのRelationship、利用する能力や自発性を個別に持てなければならない。

### FR-CMP-006 相互に独立した解釈

同じ観測、Shared Memory、ユーザー発話であっても、Companionごとに異なるAppraisal、感情、意見、行動、表現を生成できなければならない。

### FR-CMP-007 Companion間の認識

Companionは、他のCompanionを自分とは別の存在として認識し、相手の発話・行動・存在を文脈として扱えるようにしなければならない。

### FR-CMP-008 Companion間のRelationshipとMemory

Companion同士の会話や出来事について、各Companionが相手についてのMemory、Emotion、Mood、Interest、Relationshipを形成・更新できなければならない。片方の個人的な経験を、別のCompanionへ自動的に同一の経験としてコピーしてはならない。

### FR-CMP-009 Companion間交流レベル

ユーザーは、Companion間の交流を少なくとも「禁止」「必要時のみ」「協調」「雑談を含む交流」の複数段階で設定できなければならない。段階数、名称、既定値は未確定である。

### FR-CMP-010 Character Package

Character Definition、Story / Lore、Avatar資産、Voice設定、感情表現、Role、初期設定等を、1つのCharacter PackageとしてImport / Exportできなければならない。将来の配布・販売Marketplaceを妨げない構造とする。

### FR-CMP-011 デフォルトCompanion

新規ユーザーがCharacter Packageを別途用意しなくてもCompanion体験を開始できるよう、少なくとも1体の利用可能なデフォルトCompanionを提供しなければならない。具体的にどのCharacterをデフォルトとするかは製品構成・決定記録で管理し、この機能要件では固定しない。

### FR-CMP-012 Character Packageの権限境界

Character Package、Character Definition、Character Instanceの設定は、Filesystem、Network、Credential、Computer Use、Cloud Egressその他のPermissionを付与してはならない。Characterが希望・得意分野・必要Capabilityを宣言しても、実効権限はユーザーとEneのPermission Systemが決定する。

## 3. 会話、発話調停、表現

### FR-CONV-001 会話の主体

ユーザーが日常的に接する主体はCompanionでなければならない。Agent Harness、Sub-agent、Tool、MCP、Plugin、内部推論は、通常の会話の主役や必須操作対象にしてはならない。

### FR-CONV-002 発話意図の個別判断

複数Companionが同じ会話を受け取る場合でも、「自分がいつ発話するか」「何を言うか」「黙るか」は各Companion自身が判断できなければならない。

### FR-CONV-003 発話の排他

同一の会話空間で、複数Companionが論理的に同時発話してはならない。音声、テキスト、将来のfull-duplex処理のいずれでも、ユーザーが発話順序を認識できる調停を行わなければならない。

### FR-CONV-004 最新文脈での再評価

Companion Aが先に発話を開始した場合、発話待ちのCompanion BはAの発話を含む最新の会話文脈を受け取ったうえで、自分がなお発話する必要があるかを再評価しなければならない。発話要求をキューへ積んで全員に順番どおり発話させてはならない。

### FR-CONV-005 同一会話履歴

音声入力、テキスト入力、Companion発話、グループ会話、Companion間の発話は、対応する会話空間と履歴へ順序を保って保存・表示できなければならない。

### FR-CONV-006 将来のfull-duplex

現行版は半二重の音声対話を満たせばよい。将来、発話中の割り込み、barge-in、入力と出力の同時処理を追加できるよう、会話の開始、停止、キャンセル、割り込み、音声ストリームを単一の不可分な処理として固定してはならない。

## 4. Desktop Body、音声、補助UI

### FR-BODY-001 デスクトップマスコット

現行のDesktop Clientは、VRMを用いたDesktopMate風の常駐デスクトップマスコットとしてCompanionを表示できなければならない。将来、Live2D等のBody表現を追加できなければならない。

### FR-BODY-002 デスクトップ空間との相互作用

Companionは、位置、画面端、ウィンドウ、フォーカス中アプリ、複数モニター、ユーザーによるドラッグ・クリック・ホバー、作業領域等を、Bodyと現在文脈に関係する情報として扱えるようにしなければならない。

### FR-BODY-003 Body行動の判断と実行

Companionは、留まる、移動する、作業領域を避ける、別モニターへ移る、ウィンドウ上へ移る、表情・姿勢・モーションを変える等の意図を自分で判断できなければならない。座標計算、画面外への逸脱防止、衝突回避、OS制約の適用はシステム側で行う。

### FR-BODY-004 Body機能の劣化

VRM、位置情報、モーション、複数モニター等の一部が利用できなくても、可能な範囲でCompanionの会話、記憶、Task処理を継続できなければならない。

### FR-AUDIO-001 音声中心の対話

音声Providerが設定されている場合、ユーザーはSTTによる音声入力とTTSによる音声出力を中心にCompanionと対話できなければならない。

### FR-AUDIO-002 STT代替

STTが未設定または利用不能な場合、Eneはテキスト入力ポップアップを表示し、同じCompanionとの会話を継続できなければならない。

### FR-AUDIO-003 TTS代替

TTSが未設定または利用不能な場合、Eneは吹き出し等のテキスト表示でCompanionの発話内容を提示しなければならない。

### FR-AUDIO-004 音声代替の一貫性

STT/TTSの代替UIは別の会話モード、別のユーザー、別のCompanionとして扱ってはならない。入力、出力、Memory、Learning、Relationshipは同じ会話の一部として処理する。

## 5. Context Monitorと観測

### FR-CTX-001 観測可能な文脈

ユーザーのPermissionの範囲で、Desktop画面、Window、OS状態、Accessibility情報、Clipboard、アプリケーション、現在の作業、Schedule、通知、実行中Task、接続状態等、取得可能な文脈情報を扱えるようにしなければならない。具体的な情報源はPlatformごとに異なり得る。

### FR-CTX-002 共有の軽量Monitor

Eneは、常時または高頻度の文脈監視を担う共有の軽量・高速なMonitorを1つ持てなければならない。可能な構成ではLocalの軽量モデルや安価な前処理を利用できるようにする。

### FR-CTX-003 複数Companionの文脈

共有Monitorは、観測した事実と時系列メタ情報を共有しつつ、各CompanionのRole、Interest、現在の意図、Emotion、Relationship、関連Memory等を用いて、Companionごとの関連性・緊急性を評価できなければならない。

### FR-CTX-004 重要情報の供給

重要な文脈変化は、該当するCompanionが利用できる意味のあるContextとして供給できなければならない。重要でないRawイベントをMain LLMへ常時流してはならない。

### FR-CTX-005 必要時の詳細取得

Main Companionが追加情報を必要と判断した場合、許可された範囲で最新画面、Window情報、Accessibility tree、Terminal、アプリ状態その他のRawまたは詳細Contextを明示的に取得できなければならない。

### FR-CTX-006 Monitorと意思決定の分離

Context Monitorは「重要そうな変化がある」ことを通知できるが、最終的に話しかける、行動する、詳しく見る、無視することは各Companionが判断しなければならない。

### FR-CTX-007 観測と保存の分離

観測できること、Monitorが処理できること、Main Companionへ伝えられること、長期Memoryへ保存できること、Cloudへ送信できることを別々のPermissionとして評価できなければならない。

### FR-CTX-008 将来のClient Capability

現行のDesktop Observation / Computer UseはHost PCを主対象とする。将来、接続Clientが音声I/O、Observation、Body、Computer Use等のCapabilityを提供し、Host上の同じCompanionがそれらを利用できるよう拡張可能でなければならない。Capabilityの所在をHostだけに固定し、Client対応のためにCoreを大規模に再設計することを前提としてはならない。

## 6. Agent Harness、Sub-agent、Task

### FR-AGENT-001 汎用PC作業

Companionは、ユーザーのPC上で実行可能な作業を、許可された範囲で汎用的に行えるAgent Harnessを利用できなければならない。調査、開発、文書、表計算、画像、ファイル、ブラウザ、デスクトップアプリ、外部サービス、CLI等は能力の例であり、作業範囲の上限ではない。

### FR-AGENT-002 自然言語委任

ユーザーは、Tool、Harness、Sub-agentを個別に操作するのではなく、Companionへ目的や希望を自然言語で伝えて作業を委任できなければならない。

### FR-AGENT-003 Companionを最上位主体とする委任

Agent Harnessの最上位の意思決定主体はCompanionでなければならない。Companionは現在文脈、Memory、Skill、Permission、Character Stateに基づいて、直接実行するかSub-agentへ委任するかを判断できなければならない。

### FR-AGENT-004 Sub-agentの一時性

Sub-agentはCompanionの意図を実作業へ変換する一時的な作業主体であり、原則として独立した人格、長期Relationship、専用の継続個体ではない。必要な成果と履歴だけをCompanionおよびTaskへ返せなければならない。

### FR-AGENT-005 並列実行

Companionは、複雑なTaskを複数のSub-agentへ分割し、独立した作業を並列に実行できなければならない。

### FR-AGENT-006 Child Sub-agent

Sub-agentは必要に応じてChild Sub-agentを生成できなければならない。委任の最大深度は `Companion → Sub-agent → Child Sub-agent` とし、Child Sub-agentがさらにSub-agentを生成してはならない。

### FR-AGENT-007 進捗共有

Sub-agentは、作業中に、Companionが状況を把握し判断できる十分な粒度で、意味のある進捗、発見、計画変更、問題、権限不足、判断要求、フェーズ完了を返さなければならない。低レベルのマウス移動や全操作をそのままMain LLMへ流すことは要求しない。

### FR-AGENT-008 双方向の作業会話

CompanionはSub-agentからの報告を受けて作業中に起動し、質問、追加指示、計画変更、ユーザーへの通知を行えるようにしなければならない。Sub-agentが意味のある報告をしていない間、Companionをポーリングのために起動し続けてはならない。

### FR-AGENT-009 最終まとめ

Taskの完了・失敗・中断時、Sub-agentは、結果、実行した主要な作業、重要な発見、生成した成果物、未解決事項、再開または後続作業に必要な情報を最終まとめとして返さなければならない。

### FR-AGENT-010 Sub-agentの終了

最終まとめと必要な履歴が保存された後、Sub-agent自体は終了・破棄できなければならない。過去のSub-agent個体を長期人格として維持することは現行要件ではない。

### FR-AGENT-011 Task状態

Eneは、実行中、待機中、承認待ち、完了、失敗、中断等のTask状態をユーザーとCompanionが確認できるようにし、Clientの切断やCore再起動を跨いで必要な状態を保持できなければならない。

### FR-AGENT-012 CancelとRetry

ユーザーまたはCompanionは、実行中・待機中のTaskやSub-agent作業を必要に応じてCancelできなければならない。失敗・中断した作業は、影響範囲と現在状態を確認したうえで、適切な単位からRetryまたは再委任できなければならない。Retryによって既に完了した外部変更を無条件に重複実行することを前提としてはならない。

## 7. Schedule、自律行動、通知

### FR-AUTO-001 Schedule

ユーザーおよびCompanionは、指定時刻、定期予定、Reminder、後で再評価する時刻等をScheduleとして登録できなければならない。Schedule到来時、Host上のCoreは必要なCompanionまたはTaskを起動できなければならない。

### FR-AUTO-002 自発的な観測・発話・作業

Eneは、ユーザーから明示的な入力がない場合でも、許可された範囲で観測、Companion間交流、ユーザーへの発話、内部調査、Sub-agent起動、外部状態を変更する作業を行えるようにしなければならない。

### FR-AUTO-003 通知と承認の分離

新しい外部状態変更作業は原則として開始前にユーザーへ通知しなければならない。事前に委任された作業、Schedule済み作業、提案の根拠を集める内部調査等は、設定とPermissionの範囲で通知なしに実行できる。通知だけでよい場合と明示的承認が必要な場合を区別できなければならない。

### FR-AUTO-004 文脈に応じた判断

通知・承認・自動実行の要否は、Companionの文脈、Taskの影響、ユーザーの委任、現在のPermissionを考慮して判断できなければならない。ただし、LLMの判断でHard Boundaryを緩めてはならない。

### FR-AUTO-005 ユーザー不在時の結果

ユーザー不在時に行った許可済みの自律処理は、結果、重要な発見、失敗、承認待ち、次の判断をHostへ保存し、次回接続時にCompanionが自然に説明できなければならない。

## 8. Memory System

### FR-MEM-001 Memoryの種類

Eneは少なくとも次のMemory種別を区別して扱わなければならない。

- Working Memory: 現在の会話、Task、直近文脈などの短期状態
- Core Memory: 常時または高頻度に参照する、少量で重要な情報
- Episodic Memory: いつ、誰と、何が起きたかという経験
- Semantic Memory: 経験等から抽出された比較的安定した事実・知識

### FR-MEM-002 Memory scope

長期Memoryは少なくとも `Shared` と `Companion-specific` のscopeを持たなければならない。Shared Memoryは全Companionが参照できる情報、Companion-specific Memoryは特定Companionの経験・解釈・関係・固有情報を扱う。同一の出来事から両方のMemoryが生成されてもよい。

### FR-MEM-003 Memoryの概念属性

Memory Systemは、少なくとも内容、種別、scope、source / provenance、作成・更新時刻、importance、confidence、emotional significance、関連Companion・人物・Entity、更新・矛盾・置換関係を扱えなければならない。具体的なDB schemaは未確定である。

### FR-MEM-004 Memory候補の生成

Memory候補は、ユーザーとの会話、Companion間会話、Agent Harnessの作業、Sub-agent報告、許可されたContext Observation、ユーザーの明示的な記憶要求、既存Memoryの再評価等から生成できなければならない。イベント発生だけを理由に長期保存してはならない。

### FR-MEM-005 長期Memoryの選別

Learning Systemは、長期保存の候補について将来価値、永続性、一時性、新規性、重複、事実と推測の区別、confidence、Shared / Companion-specific、Core / Episodic / Semanticへの適合性を評価できなければならない。Working Memoryは現在の会話・Task用の短期状態として別に管理し、長期保存先の一種として扱うことを前提としない。

### FR-MEM-006 Memoryの更新・矛盾処理

既存Memoryと関連する新情報を得た場合、Memory Systemは、create、merge、reinforce、supersede、invalidate、mark uncertain、conflicting informationの保持等を選べなければならない。時間によって変化する事実は、過去と現在を区別して保持できなければならない。

### FR-MEM-007 Provenanceとconfidence

ユーザー本人の明示発言、直接観測、外部情報、Sub-agentの調査、Companionの推測等を、同じ確実性の情報として扱ってはならない。推測から生成されたMemoryは、確認済み事実として自動的に扱ってはならない。可能な場合、元の会話・観測・Taskまで追跡できなければならない。

### FR-MEM-008 Core Memoryの維持

Core Memoryは小規模かつ厳選された領域として維持しなければならない。Learning Systemは重要性、安定性、再利用頻度等を考慮して昇格・降格を判断できなければならず、通常の出来事を無制限にCoreへ入れてはならない。

### FR-MEM-009 Shared Memoryと経験の区別

Shared Memoryを知っていることを、すべてのCompanionがその出来事を直接経験したこととして表現してはならない。Companion固有の視点、記憶、感情、Relationshipは各個体のものとして保持しなければならない。

### FR-MEM-010 MemoryへのPermission

観測やContext処理が許可されていても、長期Memoryへの保存が禁止されている情報を保存してはならない。Memory生成、更新、削除、scope変更もPermissionと保持ポリシーの対象としなければならない。

## 9. Memory Retrieval、忘却、Consolidation

### FR-RET-001 自動想起

Companionは、現在の会話、Task、観測、Emotion、Relationship、Interest等に応じて、関連するMemoryを自動的に想起できなければならない。Core Memoryは常時または高頻度、その他は必要時のRetrievalを基本とする。

### FR-RET-002 明示的なMemory検索

Companion、Agent、またはユーザーは、必要に応じてMemoryを明示的に検索できなければならない。自動想起だけで根拠が足りない場合、追加検索、条件変更、過去の会話検索を行えるようにする。

### FR-RET-003 複合Retrieval

Memory Retrievalは単一のVector Similarityだけに依存してはならない。Semantic similarity、Keyword / full-text、Entity、時間、Memory type、scope、importance、recency、emotional significance、Relationship relevance、Companion relevance、Task relevance、confidence、provenance等の信号を組み合わせられなければならない。

### FR-RET-004 段階的Retrieval

通常の自動想起は軽量に実行できなければならない。曖昧な質問、過去を深く尋ねる質問、根拠が弱い場合に限り、追加候補取得、Reranking、Conversation History検索等の高精度処理を実行できなければならない。

### FR-RET-005 種別・原資料に応じた検索

Semantic Memory、Episodic Memory、Core Memory、Shared / Companion-specific Memoryを同一の意味で扱わず、必要に応じて時間、参加者、Entity、元会話、scope等を使い分けられなければならない。Consolidated Memoryだけでなく根拠となるConversation Historyや関連ログも検索可能でなければならない。ただし削除・再学習禁止等の保持ポリシーによって利用不可となった原資料を、想起や再学習のために再利用してはならない。

### FR-RET-006 不確かな想起

根拠が弱い、または見つからない場合、Companionは存在しない記憶を補完して事実として扱ってはならない。追加検索を行う、確信度を下げる、または思い出せないと伝えることができなければならない。

### FR-CON-001 Recall decay

忘却は原則として物理削除ではなく、時間経過、利用状況、関連性等によるrecall priorityの低下として扱わなければならない。強い関連cueがあれば古いMemoryを再想起できなければならない。

### FR-CON-002 物理削除

ユーザーの削除要求、Privacy / retention policy、明確な誤記憶、不要な完全重複等の場合、Memoryを物理的に削除できなければならない。

### FR-CON-003 Consolidation

Learning Systemは、経験、Conversation、Memory候補、Agent経験を対象にバックグラウンドでConsolidationを実行できなければならない。重複統合、Semantic Memory抽出、importance・confidence・scopeの再評価、矛盾検出・整理、recall priority調整、Skill候補抽出、Relationship・Interestへの影響評価を扱えること。

### FR-CON-004 アイドル時・イベント駆動のConsolidation

非緊急のConsolidationは固定時刻の実行を前提とせず、Hostが起動し、ユーザー作業と競合しないアイドル状態や十分な余剰資源を利用して実行できなければならない。処理待ちのキューはHostの停止・再起動を跨いで保持し、負荷が上がった場合は中断・延期・分割できなければならない。

### FR-CON-005 即時学習と遅延整理

明示的な訂正、削除、重要な記憶要求、次回から使う方法など即時性の高い変更は、可能な範囲でConsolidationを待たず反映できなければならない。重複統合、抽象化、再評価、忘却調整等の重い処理は後から実行できる。

### FR-CON-006 削除後の再学習防止

ユーザーによる削除またはPrivacy / retention policyによって長期Memoryとして利用しないと決定された情報は、元Conversation、Observation、Task履歴等が別の保持目的で残っている場合でも、その削除意図に反してLearning ReviewまたはConsolidationから同じ内容の長期Memoryとして自動再生成してはならない。Memoryのみの削除、原資料自体の削除、原資料を保持した再学習禁止をどのようにユーザーへ選択させるかは未確定である。

## 10. Skill SystemとLearning System

### FR-SKL-001 Agent Skills互換

EneのSkill形式は独自規格を必須とせず、可能な限りAgent Skillsのオープンな形式および運用と互換でなければならない。標準的な `SKILL.md` と関連resourceをImport・利用でき、Eneが生成・改善したSkillも可能な限り互換形式でExportできなければならない。

### FR-SKL-002 Skillの意味

Skillは、既存のTool、MCP、Plugin、Provider、Computer Use、Capabilityをどのように使うかを表すProcedural Knowledgeでなければならない。Skill自身が新しいCapability、Credential、Permission、Sandbox例外を付与してはならない。

### FR-SKL-003 Skillのscope

Skillは少なくとも `Shared` と `Companion-specific` のscopeを持てなければならない。前者はEne全体で再利用できる手順、後者は特定Companionの経験や接し方に基づく手順を扱う。

### FR-SKL-004 Skill Manager

Eneは、PortableなSkill形式とは別に、indexing、retrieval、Import / Export、enable / disable、scope、provenance、version・revision history、利用履歴、成功・失敗評価、自動生成、自動改善を管理するSkill Managerを持たなければならない。

### FR-SKL-005 Progressive Disclosure

通常はSkillの名前と概要等の軽量な情報から候補を検索し、必要なSkillだけ `SKILL.md` 本文、references、scripts、assets等を段階的にロードできなければならない。すべてのSkill全文を毎回Main LLMへ渡してはならない。

### FR-SKL-006 Skillの学習・評価

作業から再利用可能な手順を発見した場合、Learning Systemは新しいSkillを作成できなければならない。Skillを使った結果、成功、失敗、ユーザーの訂正、より良い方法、外部環境の変化等を評価し、改善、非推奨化、無効化を行えること。

### FR-SKL-007 Importと学習改訂の分離

外部からImportした元のSkillと、Eneが経験から加えた変更・Override・Revisionを区別できなければならない。provenance、revision historyの確認と、必要な場合のrollbackが可能でなければならない。

### FR-LRN-001 Learning Review

Eneは、`Experience → Learning Review → Memory / Skill / Companion Stateへの反映 → 次回利用 → 結果評価` の閉ループを持たなければならない。

### FR-LRN-002 Learningの出力

Learning Reviewは、Memory作成・更新、Core昇格・降格、Skill作成・更新、Relationship更新、Interest更新、Companion Stateへの必要な反映、または永続学習なしを選択できなければならない。Working Memoryの通常更新を長期学習の出力と同一視しない。

### FR-LRN-003 自動学習と低い認知負荷

通常のMemory生成、Skill改善、Relationship・Interest更新について、ユーザーへ逐次承認を要求してはならない。ユーザーは日常会話の自然言語で「覚える」「忘れる」「訂正する」「次回からこの方法にする」を指示できなければならない。

### FR-LRN-004 Learningの実行主体

Learning ReviewとConsolidationは、Main Companion LLMを常時または毎回利用することを要求してはならない。軽量モデル、Host上のバックグラウンド処理、アイドル時処理へ委譲できなければならない。

### FR-LRN-005 LearningとPermission

Learning SystemはPermission Systemより上位の権限を持ってはならない。自己改善によってSandbox、Data Egress、Tool Permission、Credential、観測範囲その他のHard Boundaryを変更してはならない。

### FR-LRN-006 詳細なLearning管理

第3層の詳細設定・管理UIで、Memory / Skillの検索、閲覧、編集、削除、scope変更、provenance確認、revision確認、rollback、enable / disable、Learning Activity確認を行えるようにしなければならない。

## 11. Emotion、Mood、Appraisal、Semantic State

### FR-AFF-001 感情を意思決定へ接続

Emotion Systemの状態は表情や音声を選ぶためだけの値ではなく、会話内容、注意、発話タイミング、自発行動、Taskの選択・優先順位、Memory形成、Relationship更新に影響する内部状態として扱えなければならない。

### FR-AFF-002 Current Affective State

Companionは、少なくとも概念上、Valence、Arousal、Control（Sense of Control）に相当する基礎的な連続状態を扱えなければならない。ControlはPADモデルにおけるDominance由来の概念をEne向けに「状況を制御・対処できる感覚」として扱う名称であり、他者を支配する意味を要求しない。具体的な数値範囲、保存形式は未確定である。

### FR-AFF-003 Appraisal

意味のある出来事について、Companionは少なくともNovelty / Expectedness、Goal relevance、Goal conduciveness、Agency、Coping potential / Control、Norm / Value compatibilityに相当する観点から評価できなければならない。

### FR-AFF-004 個体ごとのAppraisal

同じ出来事でも、Personality、Goals、Values、Memory、Relationship、Interest等によって異なるAppraisalと感情変化を生じさせられなければならない。

### FR-AFF-005 Baseline、Mood、Fast Affect

Emotion Systemは、Personality由来の比較的安定したbaseline、比較的長く持続するMood、イベントに反応する短期のFast Affectを、少なくとも意味上は区別できなければならない。

### FR-AFF-006 Decay

Fast Affectや必要な状態は、新しい意味のあるイベントがない場合、時間経過に応じてbaselineまたはMoodへ近づく自然なdecayを扱えなければならない。時間経過のためだけにMain LLMを定期起動して再計算することを必須としてはならない。

### FR-AFF-007 Reactivity、Inertia、Recovery

Character設定は、イベントへのreactivity、状態のinertia / persistence、recovery tendency等の感情ダイナミクス上の個体差を表現できなければならない。全Companionへ同じ固定値を強制してはならない。

### FR-AFF-008 Reappraisal

出来事の意味、原因、結果、ユーザーの意図等について新しい情報が得られた場合、Companionは過去のAppraisalをreappraisalし、Current Affective State、Mood、Memory、Relationshipを更新できなければならない。単なるdecayと意味変化によるreappraisalを区別できること。

### FR-AFF-009 感情表出との分離

内部のCurrent Affective Stateと発話、Voice、表情、姿勢、モーション、Body行動を同一視してはならない。同じ内部状態でも、CompanionのPersonalityや現在のContextに応じて異なる表現を選べなければならない。

### FR-AFF-010 時間情報の意味付け

現在時刻、ユーザーとの間隔、作業継続時間、予定までの時間等は、Companionが利用できる客観的なContextとして提供しなければならない。「一定時間経過したら必ず疲れる」「特定の感情は固定時間で消える」等の意味付けを製品共通の固定ルールとして要求してはならない。時間の意味と反応は、Character、State、Memory、現在Contextを踏まえて判断できなければならない。

### FR-AFF-011 Semantic Stateへの変換

Emotion、Mood、Relationship、Interest等の内部数値・構造化Stateを、そのままMain LLMへ大量に渡すことのみを前提としてはならない。Eneは、現在値、baselineとの差、直近の変化・trend、主な原因となったEvent / Appraisal、confidence等を、LLMが理解しやすい意味表現であるSemantic Stateへ変換できなければならない。

### FR-AFF-012 Semantic State変換の軽量性

Internal StateからSemantic Stateへの基本変換は、Main LLMを必須とせず、Runtime側の決定的・軽量な処理等で実行可能でなければならない。数値や構造化Stateを意味表現へ変換するだけのために、高価なMain LLMを定期的に起動することを要求してはならない。

### FR-AFF-013 LLMへの利用

CompanionのLLMは、Semantic StateをPersonality、Goals、Values、Memory、現在Contextと合わせて解釈し、発話、行動、表現、追加観測、Task委任を判断できなければならない。内部Stateの特定数値を機械的に発話へ変換することを要求しない。

### FR-AFF-014 Companion Context

Eneは、Semantic StateをMemory、Skill、Personality、Goals / Values、現在のConversation、Observation / Context、Task、Schedule、関連Appraisal等とは区別して保持し、Main LLM起動時に必要なものだけを統合したCompanion Contextを構成できなければならない。MemoryやTaskそのものをSemantic Stateへ変換しなければ利用できない設計を前提としてはならない。

## 12. RelationshipとInterest

### FR-REL-001 Relationshipの分離

Companionとユーザー、および必要に応じてCompanion同士のRelationshipは、少なくともFamiliarity、Closeness、Trustを区別して扱わなければならない。

### FR-REL-002 Familiarity

Familiarityは利用日数や発話回数だけではなく、共有経験と相互理解の蓄積を表す状態として更新できなければならない。長い利用期間と心理的な親密さを同一視してはならない。

### FR-REL-003 Closeness

Closenessは、相手がCompanionにとってどの程度心理的に近く重要な存在かを表す長期状態として扱えなければならない。

### FR-REL-004 Trust

Trustは必要に応じてPredictability、Dependability、Benevolent expectation等の側面を区別して扱えるようにしなければならない。単一の好感度へ統合してはならない。

### FR-REL-005 Relationship Event Appraisal

Responsiveness、Self-disclosure、Support、Cooperation、Conflict、Promise kept / broken、Boundary respected / violated等の経験を、必要に応じてRelationship更新の材料として扱えなければならない。

### FR-REL-006 Relationshipの個体性

同じユーザーについてのSharedな事実があっても、各CompanionのFamiliarity、Closeness、Trust、解釈、感情は独立して更新できなければならない。

### FR-REL-007 派生評価

Satisfaction、Sense of security、Affection、Resentment等は、必要なときにMemory、Relationship、Emotion、Contextから派生的に解釈できればよく、恒久的な単一数値軸として保存することを必須としない。

### FR-REL-008 Character固有の関係傾向

Attachment anxiety / avoidance、Need for closeness、Independence、Expressiveness等の関係傾向は、必要なCharacterについてPersonality / Character Definition側で表現できなければならない。これらを全Companion共通のRelationship Stateや好感度軸として必須化してはならない。

### FR-REL-009 Relationshipの時間的安定性

Familiarity、Closeness、Trust等の長期Relationship Stateを、小さな単発イベントだけでゲーム的に極端に上下させることを基本挙動としてはならない。通常は経験の蓄積を反映して緩やかに変化しつつ、重大な支援、裏切り、境界侵害その他の高い意味を持つ出来事については、Appraisalと既存関係に応じた相応の大きな変化を許容しなければならない。

### FR-INT-001 Interestの分離

InterestはEmotion、Relationship、Memoryとは別の長期的な関心・嗜好・話題傾向として扱わなければならない。

### FR-INT-002 Interestの学習

会話、観測、Task経験、選択、成功・失敗等の蓄積から、Companionが特定の話題・Entity・活動へ持つ関心の傾向をLearning Systemが更新できなければならない。

### FR-INT-003 Interestの利用

InterestはMemory Retrieval、Appraisal、話題選択、自発的発話、Taskの優先順位、Companion間交流等へ影響できるが、Interestの高さを特定の感情や行動へ機械的に直結させてはならない。

## 13. Permission、委任、Sandbox

### FR-PERM-001 統一Permission System

Eneは、Companion、Agent Harness、Sub-agent、Plugin、MCP、Skill、Provider、Context Monitorのすべてへ適用できる統一Permission Systemを持たなければならない。

### FR-PERM-002 Permissionの対象

Permissionは少なくとも、Observation、Context Monitor処理、Main Companionへの伝達、Memory保存、Filesystem、Computer Use、Network、外部サービス、Credential、Cloud Data Egress、自律実行、事前通知、明示的承認を区別して扱えなければならない。

### FR-PERM-003 自然言語による委任

ユーザーは、「このプロジェクトは調査・編集してよい」「学校関連は見てよいが送信前に聞く」等の自然言語で、目的、対象、期間、操作、通知、承認のポリシーを委任できなければならない。

### FR-PERM-004 機械的なHard Boundary

Eneは、Filesystem ACL、Sandbox、Broker、OS権限その他の機械的な境界によって、LLMが許可されていないファイル、Credential、Network、Device、Capabilityへアクセスできないよう強制しなければならない。

### FR-PERM-005 意味判断と強制の分離

LLMは、機械的に許可された範囲内で、目的、文脈、ユーザー委任に基づいて実行、通知、承認要求、拒否を判断できる。LLMの判断、Memory、Skill、Character設定、PluginはHard Boundaryを緩められない。

### FR-PERM-006 観測・伝達・保存・送信の段階制御

「取得してよい」「Monitorで処理してよい」「Main Companionへ渡してよい」「長期Memoryへ保存してよい」「Cloudへ送信してよい」を別々に設定・評価できなければならない。

### FR-PERM-007 Companionごとの可視範囲

必要に応じて、Companion、Task、Plugin、Providerごとに、観測、Memory、Tool、Cloud Egress、自律実行の可視・利用範囲を異なるものとして設定できなければならない。

### FR-PERM-008 Permissionの監査

Permissionの要求、許可、拒否、通知、承認、ポリシー変更、実行結果を、後から確認可能な監査情報として扱わなければならない。

## 14. Provider、Local / Cloud構成

### FR-PROV-001 推論コンポーネントの交換

Main LLM、Context Monitor、Sub-agent LLM、VLM、Embedding、Reranker、STT、TTSその他の推論コンポーネントは、同じ能力の別Providerへ置き換えられるようにしなければならない。

### FR-PROV-002 構成単位の選択

ユーザーは、推論コンポーネントごとに利用可能なLocal ProviderまたはCloud Provider等を独立して選択できなければならない。その結果として、システム全体を完全Local、Cloud中心、またはLocalとCloudの混在構成として正規に扱えること。個別ProviderがLocalとCloudを組み合わせるHybrid実行を提供することは許容するが、すべてのProviderにHybrid実行を要求しない。

### FR-PROV-003 Cloudと正本の分離

Cloud Providerを利用しても、Eneの永続状態、Memory、Character Instance、Permission、Task履歴の正本をCloud側へ移してはならない。Cloudは必要な処理の委譲先として扱う。

### FR-PROV-004 有料Providerの明示選択

有料または従量課金の可能性があるProviderを、ユーザーの明示的な選択なしに有効化、契約、課金開始してはならない。推奨構成を提示することはできるが、推奨を自動採用してはならない。

### FR-PROV-005 自動Provider fallback

Provider障害時に、Eneが未設定の別Providerへ自動的に切り替えることを必須としない。通常は利用不能を明示し、ユーザーが別Providerを選択・設定できるようにする。自動切替の詳細方針は未確定である。

### FR-PROV-006 ProviderごとのData Egress

Cloud Providerを使う処理では、送信されるデータの種類、送信先、認証、費用をユーザーが確認でき、Permissionで許可されたデータだけを送信しなければならない。

### FR-PROV-007 初期Provider構成の明示選択

初回設定では、少なくともMain LLM、STT、TTSについて推奨候補を提示できるが、利用するProviderまたは未設定をユーザー自身が明示的に選択できなければならない。STT / TTSは未設定を正規の構成として扱い、その場合はテキストUIで代替する。推奨候補をユーザーの選択とみなして自動適用してはならない。

## 15. Plugin、MCP、Skillの責務

### FR-EXT-001 汎用Plugin System

Eneは、LLM / VLM / Embedding / STT / TTS Providerや単純なToolの追加に限定されない汎用Plugin Systemを持たなければならない。Pluginは、Provider、Observation source、Context処理、Memory処理、Scheduler / Trigger、Capability、Agent Harness、Body / Avatar、内部Event、設定・管理UI等の拡張点へ参加できる。

### FR-EXT-002 公開Extension Point

Pluginは、Eneが明示的に公開したExtension Point、Event、Capability Interface、API、Brokerを通じて拡張しなければならない。Coreの任意の内部状態、DB、権限を直接書き換えることを前提としない。

### FR-EXT-003 MCPの優先利用

外部ToolまたはResourceを接続するだけで目的を満たせる場合、Ene独自Pluginを必須とせず、MCPを利用できなければならない。Plugin SystemをMCP Serverの再実装にしてはならない。

### FR-EXT-004 三者の区別

Eneは、Plugin、MCP、Skillを別概念として扱わなければならない。

- Plugin: Ene Runtime / Architecture自体を拡張する。
- MCP: 外部のTool / Resourceを接続する。
- Skill: 既存のCapabilityをどう使うかを教える。

### FR-EXT-005 第三者Pluginの隔離

第三者Pluginは原則としてEne Coreから隔離された実行環境で動作し、公開API、Event、Capability、Brokerを通じてのみEneへアクセスしなければならない。Pluginの障害や未許可アクセスでCore、他Plugin、ユーザーデータの完全性を損なってはならない。

### FR-EXT-006 高帯域Pluginの性能

Pluginの隔離は、Local LLM / VLM、STT、TTS、Embedding、画像、音声その他の高帯域・低遅延処理を実用上妨げてはならない。PermissionとSandboxの範囲内で、隔離されたPluginプロセス自身がGPU、Accelerator、必要なRuntimeへ直接アクセスできる構成を許容し、大量データを扱うPluginへ不要なコピー・直列化・Core経由の中継を強制しない拡張・通信方式を選べなければならない。

### FR-EXT-007 Pluginの権限

Pluginは、Pluginであることを理由にFilesystem、Network、Credential、Observation、Memory、Cloud Egress、UI等へ無制限にアクセスしてはならない。PluginのCapability要求は通常のPermissionとSandboxで評価する。

### FR-EXT-008 Plugin UIの境界

Pluginは、Eneが提供する設定、管理、詳細表示、診断、Plugin固有の画面へUIを追加できる。ただし、通常のCompanion UIのマスコット、発話、吹き出し、主要対話面へ任意のUI要素を挿入・置換してはならない。Eneが定義したBody API等を介した正規の表現拡張は妨げない。

## 16. UIの3層

### FR-UI-001 情報公開の3層

Eneは、内部機構を日常体験へ露出しすぎず、必要なユーザーが深い情報へ到達できるよう、次の3層のUIを持たなければならない。

1. **Companion UI**: Desktopマスコット、音声、吹き出し、自然言語の会話、日常の発話・作業結果。
2. **表層設定**: Character、Provider、Voice、基本自発性、基本Permission、Companion間交流など、一般ユーザー向けの設定。
3. **詳細設定・管理**: Harness、Sub-agent、MCP、Plugin、詳細Permission、Data Egress、Context Monitor、Memory、Skill、ログ、診断、コストなど。

### FR-UI-002 Companion UIの抽象度

Companion UIでは、ユーザーが「Companionが考え、行動している」と感じられることを優先し、Sub-agent tree、Tool call、内部推論、Provider graph等を常時の主表示にしてはならない。

### FR-UI-003 表層設定

表層設定では、内部アーキテクチャを理解しなくても、Character、表示、Voice、Provider、基本Permission、自発性、Companion間交流を設定できなければならない。

### FR-UI-004 詳細設定・管理

詳細設定・管理では、内部Taskの進捗、Sub-agent tree、Companion間通信、CompanionとSub-agentの報告、重要Contextイベント、Memory / Skillの由来、Permission判断、Provider利用、Plugin障害、診断情報を確認できなければならない。

### FR-UI-005 視覚的な深さ

3層の情報公開レベルは、画面構成、導線、視覚的階層で一貫して表現しなければならない。具体的な色、テーマ、コンポーネントは未確定である。

## 17. ログ、監査、コスト

### FR-OBS-001 主要イベントの記録

Eneは、後から何が起きたかを追跡できるよう、会話、主要な通信、重要なContextイベント、Task、Tool / MCP / Computer Use、Permission判断、自律行動、Schedule、Memory変更、Skill変更、重要なEmotion / Mood / Relationship / Interest等のCompanion State変更、設定変更、Provider利用、Pluginの起動・停止・障害等を記録しなければならない。高頻度の微小なState変化をすべて永続記録することは要求しない。

### FR-OBS-002 主体間通信

ユーザーとCompanion、Companion同士、CompanionとSub-agentの発話・報告・指示・質問・最終まとめを、必要な履歴として確認できなければならない。

### FR-OBS-003 Rawデータと内部推論の扱い

LLMの逐語的な内部推論、Context Monitorが破棄した大量の非重要イベント、意味のない低レベル操作、音声ストリームやスクリーンショットの常時保存を既定の永続ログにしてはならない。必要な意味のあるDecision、Action、Resultを記録する。

### FR-OBS-004 ログ管理

ユーザーは、必要に応じてログの保存期間、容量、デバッグ情報、画像・音声・大きなTool出力の保存可否、カテゴリ除外を管理できなければならない。正確な保持期間と容量は未確定である。

### FR-COST-001 利用量と推定コスト

Eneは、Providerの利用量と推定コストを記録しなければならない。請求額を正確に取得できない場合は、請求額ではなく推定値であることを示す。

### FR-COST-002 コストの内訳

利用量・推定コストは、Provider、LLM / VLM / Embedding / STT / TTS等の用途、Companion、Sub-agent、Task、自律処理、バックグラウンド処理等の単位で集計・確認できなければならない。

### FR-COST-003 予算制御

ユーザーは、日次・月次その他の期間で予算上限を設定し、上限接近時の通知、上限到達時の停止、保留、明示的承認要求等を設定できなければならない。予算制御は自律処理とバックグラウンド処理にも適用する。

### FR-COST-004 コスト情報の明示

Providerの料金形態、概算品質・速度、必要なHardware、外部送信、API Key等の追加設定を、ユーザーがProvider選択前に確認できなければならない。

## 18. 劣化、障害、状態継続

### FR-RES-001 機能単位のGraceful Degradation

STT未設定・利用不能時はテキスト入力、TTS未設定・利用不能時はテキスト表示へ切り替えなければならない。Main LLM、Provider、Plugin、Sub-agent、Networkの障害は、利用不能となった機能を明示し、可能な他機能とCoreを不必要に停止してはならない。

### FR-RES-002 自動Fallbackの非必須化

Eneは、自動Provider fallbackや全機能の無停止継続を必須としない。Providerが利用不能な場合、ユーザーが状態を理解し、再設定・手動選択・後で再実行できるようにする。

### FR-RES-003 再起動後のTaskとLearning

Core、Host、Provider、Pluginの再起動・停止後も、確定済みの会話、Memory、主要ログ、Task状態、通知・承認待ち、Learning / Consolidation待ちを合理的な範囲で復元または再開可能にしなければならない。

### FR-RES-004 失敗の正確な表示

復元できない、途中で中断された、権限不足、外部サービスが失敗した処理を、成功したものとしてCompanionやユーザーへ報告してはならない。

## 19. 機能要件として未確定の項目

- Emotion System、Current Affective State、Mood、Relationship、Interest、Semantic Stateの正確なschema、数値範囲、更新式。
- Memory・Skillの保存形式、検索アルゴリズム、Embedding・Rerankerの採用。
- 「忘れる」要求におけるMemoryのみの削除、原資料の削除、原資料を残した再学習禁止の具体的なUI・保持方式。
- Permissionのリスク分類、通知・承認の閾値、自然言語ポリシーの競合解決。
- Providerの一覧、モデル、価格表、Credential保管方式、自動切替の詳細。
- Plugin API、Extension Point、IPC、Sandbox、Broker、高帯域転送の具体方式。
- Character Packageのmanifest、署名、依存関係、Marketplaceの審査・課金方式。
- Companion間交流の段階数・既定値と、自発的ランダム起動の頻度・制限。
- ログ、Rawデータ、成果物、音声、画像、Tool outputの保持・削除・Export形式。
- Client側のObservation、Computer Use、音声I/O、Remote通知の開始時期と機能範囲。
- Remote Clientの認証、暗号化、Device trust、接続方式。
