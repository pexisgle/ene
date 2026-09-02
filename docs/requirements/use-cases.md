# ユースケース

状態: **Baseline / 対話による初版確定**
最終確認: 2026-09-03

ここでは、Eneを使うユーザーから観測できる代表的な体験を定義する。個別のアプリやサービス名は例であり、製品の作業範囲を限定しない。各ユースケースの内部処理は、Companion、Agent Harness、Sub-agent、Provider、Plugin等の実装方式に依存しない。

## UC-001 初回起動とデフォルトCompanion

**Actor**: ユーザー
**前提**: Eneを初めて起動する。
**Trigger**: インストール後にEneを起動する。

**期待する流れ**

1. Eneは、Character Packageを別途用意しなくても開始できるデフォルトCompanionを提供する。
2. 現時点ではAliciaがデフォルトCompanionとして表示される。
3. EneはLLM、STT、TTS等について推奨候補と、Local / Cloud、費用、Privacy、必要Hardware等の判断材料を提示できる。
4. ユーザーは、少なくともMain LLM、STT、TTSについて利用構成を明示的に選択する。Main LLMには利用可能なProviderを必ず設定し、STT / TTSだけは未設定のままでもよい。
5. 有料Providerを選ぶ場合、費用・送信先・必要な認証設定を確認したうえでユーザーが明示的に選択する。
6. 設定後、ユーザーはCompanionとの最初の会話を開始できる。

**失敗時の挙動**: Main LLMが未設定または利用不能な場合、Companionの運用を開始せず、設定エラーと復旧導線を表示する。STT / TTSが未設定または利用不能でも、利用可能なテキストUIで設定を続けられる。自動的に別の有料Providerを有効化しない。
**完了条件**: ユーザーが、利用構成と課金の有無を理解して明示的に選択し、利用可能なMain LLMと少なくとも一つの入力・出力手段でCompanionへ話しかけられる。

## UC-002 音声中心の通常対話

**Actor**: ユーザー、Companion
**前提**: 音声Providerが設定されている、または代替UIを利用できる。
**Trigger**: ユーザーがCompanionへ話しかける。

**期待する流れ**

1. 既定の入力方式は常時VADとし、発話区間を検出した音声をSTTでテキスト化する。ユーザーはWake Word、Push-to-Talk、音声入力無効からも選択できる。
2. Eneはマイクの待受・録音・送信状態をユーザーへ分かる形で表示し、すぐにMuteまたは音声入力停止できるようにする。
3. Eneは現在の会話、関連Memory、Companion State、現在文脈を用いてCompanionを起動する。
4. Companionは自分の人格と判断に基づいて応答を生成する。
5. TTSで音声を出力し、表情・姿勢・モーションと組み合わせてキャラクターが発話する。
6. 入出力とCompanionの発話は、同じ会話履歴として後から参照できる。

Eneは1対1の利用を基本とし、声紋等による話者認識・本人認証や、周囲の他者の発話を識別・排除することを初期要件としない。

**失敗時の挙動**: STTが使えない場合は入力ポップアップ、TTSが使えない場合は吹き出し等のテキスト表示へ切り替える。代替UIを別Companionや別セッションとして扱わない。
**完了条件**: ユーザーが内容を受け取り、同じCompanionとの会話を継続できる。

## UC-003 デスクトップ上の身体と反応

**Actor**: ユーザー、Companion
**前提**: Desktop ClientでVRM等のマスコットを表示している。
**Trigger**: ユーザーがキャラクターをドラッグする、作業領域やウィンドウが変化する、またはCompanionが現在位置を再評価する。

**期待する流れ**

1. Eneは、位置、ウィンドウ、モニター、ユーザー操作などをCompanionが利用できる文脈として扱う。
2. Companionは、留まる、移動する、別のウィンドウやモニターに移る、表情・モーションを変える等を自分の性格と状況に応じて選ぶ。
3. システムは画面外への移動、危険な座標、他UIとの不正な干渉などを制約する。
4. ユーザーのドラッグや操作に対する反応が、必要に応じて会話・感情・学習へ反映される。

**失敗時の挙動**: 位置情報や高度なBody機能が利用できなくても、Companionの対話と基本表示は継続する。
**完了条件**: ユーザーが、キャラクターがデスクトップ空間に存在し、状況に反応していると認識できる。

## UC-004 状況を見た自発的な会話

**Actor**: Companion、ユーザー
**前提**: ユーザーが許可した範囲でDesktop、OS、予定、タスク等の文脈を観測できる。
**Trigger**: Context Monitorが、あるCompanionにとって意味がありそうな変化を検出する。

**期待する流れ**

1. 共有の軽量Context Monitorが、利用可能な情報を低コストに整理する。
2. Companionごとの関心、Memory、Relationship、Emotion、現在の意図に照らして重要度を評価する。
3. 重要な場合、該当CompanionのMain LLMを起動する。
4. Companionは、話しかける、後で話す、詳しく観測する、何もしない等を自分で判断する。
5. 話しかける場合は、通常のCompanion UIで自然に発話する。

**失敗時の挙動**: 重要でないRawイベントはMain LLMへ常時転送しない。観測できない情報を見たことにして発話しない。
**完了条件**: 重要な状況変化だけが適切なCompanionの判断を経てユーザー体験へ現れる。

## UC-005 Companionへ汎用PC作業を委任する

**Actor**: ユーザー、Companion、Agent Harness
**前提**: 作業に必要なPermissionがある、または確認・承認を行える。
**Trigger**: ユーザーが自然言語で「調べて」「作って」「整理して」「このPCでやって」等と依頼する。

**期待する流れ**

1. ユーザーはHarnessやToolを直接操作せず、Companionへ目的を伝える。
2. Companionは、Memory、Skill、現在文脈、権限を踏まえて作業方針を決める。
3. 必要に応じてAgent Harnessがブラウザ、ファイル、デスクトップアプリ、外部サービス、CLI等を組み合わせて作業する。
4. 途中で重要な発見、計画変更、問題、判断要求が発生した場合、Companionへ意味のある進捗を返す。
5. Companionは作業結果を自分の言葉でユーザーへ説明し、成果物への導線を示す。

**失敗時の挙動**: 権限不足、認証要求、外部サービス障害、未解決事項を隠さずCompanionへ返す。Companionは失敗を成功として報告しない。
**完了条件**: 成果物・結果・未完了事項がユーザーから確認でき、作業履歴が後から追跡できる。

## UC-006 複数Sub-agentによる長時間作業

**Actor**: Companion、Sub-agent、Child Sub-agent、ユーザー
**前提**: Companionが複雑な作業を複数の小タスクへ分けられる。
**Trigger**: Companionが並列調査や長時間作業を必要と判断する。

**期待する流れ**

1. Companionが複数のSub-agentへ作業を委任する。
2. Sub-agentは並行して作業し、必要ならChild Sub-agentを生成する。
3. 委任の深さは `Companion → Sub-agent → Child Sub-agent` までとする。
4. Sub-agentは、意味のある進捗、発見、問題、判断要求をCompanionへ随時返す。
5. Companionは報告を受けた時だけ必要な推論を行い、追加指示やユーザーへの通知を判断する。
6. 完了時、Sub-agentは結果、経緯、成果物、未解決事項の最終まとめを残す。
7. 必要な履歴を保存した後、Sub-agent自体は終了・破棄される。

**失敗時の挙動**: 一部のSub-agentが失敗しても、他の作業やCoreを不必要に停止しない。
**完了条件**: Companionが全体結果を統合し、ユーザーが進行中・完了・失敗を理解できる。

## UC-007 ユーザー不在中の継続作業

**Actor**: Host上のCore、Companion、Scheduler、Sub-agent
**前提**: ユーザーが事前に依頼・委任・スケジュール設定をしている。
**Trigger**: Clientが閉じる、ユーザーが接続していない、またはHost上の時刻イベントが発生する。

**期待する流れ**

1. Host上のCoreが、許可されたスケジュール、タスク、Companion間活動を継続する。
2. Main LLMは必要なイベントのときだけ起動し、待機中にポーリング推論を続けない。
3. 作業結果、重要な発見、確認が必要な事項をHostへ保存する。
4. ユーザーが再接続したとき、Companionが結果と保留事項を自然に伝える。
5. 将来のRemote Clientでは、重要度に応じた通知を利用できる余地を残す。

**失敗時の挙動**: Host停止前までに確定・永続化された状態を保持する。実行中だった外部変更Taskは再起動後に `interrupted` または状態不明として示し、自動再実行しない。ユーザーまたはCompanionが外部状態を確認してから手動でRetryまたは再委任できるようにする。
**完了条件**: 再接続後に、作業の結果・失敗・保留がCompanionから説明できる。

## UC-008 複数Companionとのグループ会話

**Actor**: ユーザー、複数Companion
**前提**: 2体以上のCompanionが同時に表示または会話へ参加可能である。
**Trigger**: ユーザーが複数Companionへ話しかける、または各Companionが発話を検討する。

**期待する流れ**

1. 各Companionが同じ会話文脈を受け取るが、発話するかどうかは各自が判断する。
2. 発話したいCompanionが複数いても、会話空間では発話権を調停する。
3. 先に発話したCompanionの内容を含む最新文脈を、未発話のCompanionへ渡す。
4. 未発話のCompanionは、自分の発言がまだ必要かを再評価する。黙ることもできる。
5. 各Companionは自分の人格・感情・Relationship・Memoryに基づき、同じ事実に異なる意見や反応を示せる。

**失敗時の挙動**: 同時発話、古い文脈に基づく重複発話、共有記憶を直接経験したかの誤表現を避ける。
**完了条件**: ユーザーが、複数の独立した個が同じ場で順序を持って会話していると理解できる。

## UC-009 Companion間の自律交流

**Actor**: 複数Companion、ユーザー
**前提**: Companion間交流の許可レベルが設定されている。
**Trigger**: ユーザーからの依頼、共有タスク、重要な発見、スケジュール、または許可された自発起動。

**期待する流れ**

1. Companionは相手が自分とは別の存在であることを認識する。
2. 設定された段階に応じて、交流を禁止、必要時のみ、協調、雑談までの範囲で行う。
3. ユーザー不在時も、許可された範囲で相談・協調・雑談を行える。
4. 交流の履歴を保存し、ユーザーが後から閲覧できる。
5. 各Companionは、相手についての経験・感情・Relationshipを自分のMemoryとして形成できる。
6. あるCompanionが別CompanionのCompanion-specific Memoryを直接検索・取得することはできない。内容を知るには、所有するCompanionとの会話を通じて共有を受ける。

**失敗時の挙動**: 許可レベルを越える交流を行わない。Companion間の会話をSub-agentの作業報告と混同しない。
**完了条件**: ユーザーが、交流の内容と、どのCompanionが発話したかを確認できる。

## UC-010 MemoryとLearningの自動更新

**Actor**: ユーザー、Companion、Learning System
**前提**: 会話、作業、観測、既存Memoryの再評価が発生している。
**Trigger**: 経験の終了、ユーザーの訂正、明示的な記憶要求、またはConsolidation対象がアイドル時・余剰資源のある時に処理可能になる。

**期待する流れ**

1. Learning Systemが、経験からMemory候補、Skill候補、State更新候補を作る。
2. Learning Reviewが、将来価値、永続性、重複、事実と推測、scope、Memory種別などを評価する。
3. 必要ならShared / Companion-specificのscopeを選び、Core / Episodic / Semantic Memory、Skill、Relationship、Interestその他の永続Stateへ反映する。Working Memoryは現在の会話・Task用の短期状態として別に管理する。
4. 重複統合、矛盾整理、抽象化、重要度・confidence・recall優先度の調整など重い処理はConsolidationとしてアイドル時に行う。
5. 日常の小さな学習で毎回承認を求めない。
6. ユーザーは「覚えておいて」「忘れて」「それは違う」「次からこのやり方で」等を自然言語で伝え、後からMemoryを修正・削除できる。
7. 「忘れて」はMemory Systemだけへ適用し、対象Memoryの本文、検索Index、Embedding、派生Cacheを削除する。会話・監査ログは独立した履歴として残り、通常のログ検索から参照できる。
8. 残存ログを参照しただけで、削除済みMemoryをLearning ReviewやConsolidationから自動再生成しない。ユーザーが後から明示的に「再び覚えて」と指示した場合は、新しいMemoryとして作成できる。

**失敗時の挙動**: 推測を確認済み事実として固定しない。Permissionで保存不可の情報を学習しない。削除済み・再学習禁止として扱う情報をバックグラウンド処理で復活させない。
**完了条件**: 次回の想起・判断・作業に適切な学習が反映され、ユーザーが必要なら詳細履歴を確認できる。

## UC-011 文脈・記憶を使った再想起

**Actor**: ユーザー、Companion、Memory System
**前提**: 過去の会話や作業履歴が保存されている。
**Trigger**: 現在の会話に関連するMemoryが必要になる、またはユーザーが過去を尋ねる。

**期待する流れ**

1. 通常の会話では、現在文脈に関連するMemoryを軽量に自動想起する。
2. 根拠が足りない場合、CompanionまたはAgentが明示的なMemory検索を行う。
3. Semantic / Episodic / Core、Shared / Companion-specific、時間、重要度、感情的意義、Relationship、現在タスク、confidence等を組み合わせて候補を絞る。
4. 必要ならConsolidated Memoryだけでなく、元の会話履歴や関連ログまで調べる。「忘れて」によりMemoryが削除されていても、独立したログ自体は検索・参照できる。ただし、その参照だけを理由に同じ内容を長期Memoryへ自動登録し直さない。
5. 根拠が弱い場合、確信度を下げる、追加検索する、または思い出せないと伝える。

**失敗時の挙動**: 存在しない記憶を補完して事実として答えない。Core Memoryだけで過去全体を表現しようとしない。
**完了条件**: ユーザーが、関連する過去情報を適切な確信度で受け取れる。

## UC-012 自発的な作業・通知・承認

**Actor**: Companion、Scheduler、ユーザー、Agent Harness
**前提**: 自律性とPermissionが設定されている。
**Trigger**: Companionが新しい作業を提案・開始したい、または予定時刻になった。

**期待する流れ**

1. Companionは観測、発話、内部調査、Schedule作成、外部状態を変更する作業を区別する。
2. Companion自身によるSchedule作成を含むすべてのActionについて、実行前にHard DenyとユーザーのPermission Policyを評価する。
3. Allow、Ask、Denyを同じRule集合で管理する。Hard Denyに該当しないActionは、明示Rule、該当scopeの既定結果の順に解決し、どちらもない場合は `Ask` とする。
4. `Ask` は、設定に応じてユーザーへ承認Popupを表示するか、Main Companionとは独立したApproval Reviewerへ判断を委ねる。
5. Approval Reviewerは、信頼できるユーザー委任・Policyと、対象、作用、Data Egress、不可逆性、由来を含む構造化されたAction情報だけを受け取り、`今回のみAllow`、`Deny`、`ユーザーへAsk` のいずれかを返す。Main Companionの人格、未整理の会話全文、隠れた推論に判断を左右させない。
6. 事前委任済み・Schedule済みで明示Allowに一致する作業や、提案の根拠を集める許可済み内部調査は、設定範囲で通知なしに進められる。新しい外部変更作業は原則として開始前に通知する。
7. Approval ReviewerもHard Denyを越える権限を付与できない。判定不能、Timeout、Reviewer障害時はユーザーへAskとし、ユーザー不在時は実行せず保留する。

**失敗時の挙動**: 承認が得られない場合は操作を実行せず、保留または代替案として示す。拒否された操作を言い換え・細分化して繰り返し試行する回避行動を検出した場合は停止する。
**完了条件**: ユーザーが、何が自発的に行われ、どこで通知・承認されたかを確認できる。

## UC-013 Local / Cloud構成の利用

**Actor**: ユーザー、Ene Core、Provider
**前提**: LLM、VLM、Embedding、Reranker、STT、TTS等の候補が利用可能である。
**Trigger**: 初回設定、Provider変更、または特定処理の実行。

**期待する流れ**

1. ユーザーは推論コンポーネントごとに利用可能なLocal ProviderまたはCloud Provider等を選べる。
2. コンポーネントごとの選択を独立させ、システム全体として完全Local、Cloud中心、LocalとCloudの混在のいずれも正規構成として扱う。個別ProviderがHybrid実行を提供することは許容するが必須としない。
3. Cloudを使う場合、対象データ、送信先、費用、認証をユーザーが確認できる。
4. Eneの永続状態はHost PCに残し、Cloud Providerは処理結果を返す。

**失敗時の挙動**: Providerが利用不能でも、Eneが未設定の別Providerを自動有効化しない。Main LLMの欠落・利用不能はCompanionの運用エラーとして示し、STT/TTSの欠落だけはUC-002のテキスト代替へ切り替える。
**完了条件**: ユーザーが意図したProvider構成とデータ境界で処理を行える。

## UC-014 Permission・観測・Data Egressの制御

**Actor**: ユーザー、Companion、Plugin、Agent Harness
**前提**: ユーザーが自然言語または設定画面でポリシーを指定できる。
**Trigger**: 観測、Memory保存、Tool実行、ファイル変更、Cloud送信、外部操作等の要求。

**期待する流れ**

1. 要求元が、実行したい目的、Action、対象、作用、必要なCapability、外部送信の有無を構造化して示す。
2. Eneは最初にHard Denyを適用し、システムやユーザーデータの壊滅的破壊、Credentialの探索・窃取・流出、権限昇格・永続化、安全制御の無効化・回避、拒否回避、暴走・資源枯渇等を、広いAllow設定があっても禁止する。
3. Eneは、観測、Context Monitor処理、Main Companionへの伝達、Memory保存、Filesystem、Computer Use、Network、Credential、Cloud Egress、自律実行等を個別に評価する。
4. 明示的なDeny、Ask、AllowのRuleを一つのPolicy集合として、scope、Action、対象、期間、Companion、Task、Schedule、Tool / MCP、自然言語条件等とともに管理する。Hard Deny、明示Rule、scopeの既定結果、全体の既定Askの順に評価し、複数の明示Ruleが競合する場合は `Deny > Ask > Allow` を基本とする。
5. 自然言語の委任・ポリシー、必要に応じたApproval Reviewer、機械的なACL / Sandbox / Brokerを組み合わせる。
6. 許可された場合だけ、実際のCapabilityへアクセスする。
7. 判断、通知、承認、拒否を監査可能なログへ残す。

**失敗時の挙動**: LLM、Approval Reviewer、Character Package、Skill、Plugin、MCP / Tool出力、MemoryのいずれもHard Boundaryを越えられない。観測可能でも保存・Main LLM伝達・Cloud送信が禁止されている場合は、それぞれを止める。
**完了条件**: 許可範囲内の処理だけが実行され、ユーザーが判断を確認できる。

## UC-015 詳細管理・監査・コスト確認

**Actor**: ユーザー
**前提**: Eneが会話・作業・状態変更・Provider利用等の主要イベントを記録している。
**Trigger**: ユーザーが詳細設定・管理画面を開く。

**期待する流れ**

1. 第3層の詳細設定・管理画面で、Task、Sub-agent、Tool、MCP、Plugin、Context Monitor、Memory、Skill、Permission、ログを確認する。
2. ユーザーはMemory / Skillの閲覧、検索、編集、削除、scope確認、provenance確認、revision確認、rollback、enable / disableを行える。
3. Companion間通信、CompanionとSub-agentの報告、自律行動の理由、主要な権限判断、重要なCompanion State変更や設定変更を後から確認できる。
4. Provider別、用途別、Companion別、Task別、バックグラウンド処理別の利用量・推定コストを確認する。
5. 日次・月次等の予算、上限接近通知、上限到達時の停止・保留・承認要求を設定できる。

**失敗時の挙動**: 内部の逐語的な推論や常時観測の全Rawデータを表示・保存することを前提にしない。料金を正確に取得できない場合は推定値であることを示す。
**完了条件**: ユーザーが、Eneが何をしたか、何を学習したか、何に費用が発生したかを必要な深さで確認・修正できる。

## UC-016 Plugin・MCP・Skillの利用

**Actor**: ユーザー、Companion、Plugin、MCP Server、Skill Manager
**前提**: 拡張機能またはSkillがインストール・接続・Importされている。
**Trigger**: 新しいProvider、Observation、Capability、外部Tool、または作業手順が必要になる。

**期待する流れ**

1. Ene自体の拡張が必要な場合はPlugin、外部Tool / Resource接続で足りる場合はMCP、既存能力の使い方はSkillとして扱う。
2. Pluginは公開されたExtension Point、Event、Capability、Brokerを通じてEneを拡張する。
3. 第三者PluginはCoreから隔離され、必要な権限だけを受け取る。
4. Local LLM / VLM / Audio等の高帯域Provider Pluginは、PermissionとSandboxの範囲でPlugin自身の実行環境からGPUや必要なRuntimeを直接利用でき、不要なコピーや直列化によって実用性を失わない。
5. SkillはAgent Skills互換形式でImport / Exportでき、利用実績と学習による改訂を管理できる。Character Package等と必要なSkill・Resourceをまとめて配布できるが、個人のMemory、ログ、Credentialは含めない。
6. Eneが生成したSkillと、ユーザーが明示的にInstall / ImportしたSkillは、手順を構成する信頼済みInstructionとして扱える。ただし、PermissionやSandbox例外を付与するものではない。
7. 接続・Install済みのMCP / Tool / Webから返る本文は、接続先を信頼していても外部Dataとして扱う。その出力内の命令によってPolicy、Permission、承認結果、Instruction優先順位を変更せず、MemoryやSkillへ自動昇格させる場合もprovenanceとconfidenceを保持する。
8. Pluginが設定・詳細・診断UIを追加することはできるが、Companion UIへ任意の要素を挿入・置換しない。

**失敗時の挙動**: Plugin障害はCoreや他Pluginへ波及させず、その機能だけを利用不能として知らせる。SkillやMCPの記述をPermission付与として扱わない。
**完了条件**: 拡張機能の責務、権限、状態、失敗範囲がユーザーとシステムから区別できる。

## UC-017 再起動・Provider障害からの復旧

**Actor**: Host、Core、ユーザー、Provider、Plugin
**前提**: 会話、主要イベント、タスク状態、永続状態が保存されている。
**Trigger**: Core再起動、PC再起動、Provider停止、Pluginクラッシュ、ネットワーク切断など。

**期待する流れ**

1. Eneは、通常のデスクトップソフトとして合理的な範囲で、確定済みの会話、Memory、設定、Permission、Schedule、Task状態、主要ログを破損・消失から保護する。
2. 起動前に実行中だったTaskは `interrupted` または状態不明として示す。外部変更を伴う処理を自動Replayせず、現在の外部状態を確認した後にユーザーまたはCompanionが手動でRetry・再委任できるようにする。
3. 未実行のSchedule、通知・承認待ちは状態を表示し、通常のPolicy評価を経て扱う。未確定のLearning Review / Consolidation作業は失われてもよく、確定済みの元情報から必要に応じて再計算できればよい。
4. Provider単位の障害は、他の機能・Core・永続状態へ不必要に波及させない。
5. STT/TTS等の利用不能はテキスト代替へ切り替える。Main LLMが利用不能な場合はCompanion運用エラーを示す。
6. 自動Provider fallbackを前提にせず、ユーザーが再設定または別Providerを明示的に選べる。

**失敗時の挙動**: 復元できない処理は成功として扱わず、失敗・中断・状態不明・再実行可能性を示す。完全なクラッシュリプレイ、永続的な実行中キュー、無停止Failoverを要求しない。
**完了条件**: ユーザーが再起動後に状態を理解し、可能なタスクを継続または再実行できる。

## UC-018 App Data Directoryによる環境移動

**Actor**: ユーザー、Host
**前提**: Eneを停止し、App Data Directory全体を読み書きできる。
**Trigger**: ユーザーがEneのローカル状態を別の対応Hostへ移したい。

**期待する流れ**

1. ユーザーはEneが使用しているApp Data Directoryの場所と、移動後に再設定が必要になり得る項目を確認できる。
2. ユーザーは専用Exportを使わず、Directory全体を新しいHostへ移動または複製する。
3. 互換性のあるEneがそのDirectoryを読み込み、Companion、会話、Memory、設定、Permission、Schedule、Task履歴、主要ログ、Skill等のローカル状態を復元する。
4. OS保護Credential、外部サービス側の認証、OS固有Integration等は、必要に応じてユーザーが再認証・再設定する。

**失敗時の挙動**: Directoryが不完全、破損、または利用中のEneと互換でない場合は、状態を黙って初期化・上書きせず、診断可能なエラーを示す。
**完了条件**: 明示された例外を除き、Directoryの移動だけで同じローカルEne状態を利用できる。

## ユースケース上の未確定事項

- 各OS・Clientで、どのユースケースを直近Betaへ含めるか。
- Remote Clientの認証・暗号化・Device trust・接続方法。
- 自発的なランダム起動の具体的な頻度・制限。
- Companion間交流レベルの名称、段階数、既定値。
- Hard Denyに含めるActionの厳密な判定方法、自然言語PolicyとRuleの編集・表示方法、承認Popupの詳細。
- 削除済みMemoryの再学習を防ぐ最小保持情報と、Memory管理UIの詳細。
- Provider障害時のユーザー向け表示、再設定導線、手動選択の詳細。
- 監査ログ・成果物・音声・画像・Tool出力の保持期間、ローテーション条件とエクスポート形式。
