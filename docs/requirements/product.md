# 製品定義

状態: **Baseline / 対話による初版確定**
最終確認: 2026-09-03

## 1. 製品の要約

Ene は、**1人のユーザーのために1台のHost PC上に継続的に存在する、AITuber / AI companion中心のパーソナルAIシステム**である。

ユーザーが日常的に接する主役は、DesktopMate のようなデスクトップマスコットとして表示される Companion である。Companion は単なるチャットボットの外見や音声ではなく、人格、価値観、意思、感情、記憶、関係性、関心、身体表現を持つ個として、ユーザーを自分なりに支援する。

ユーザーは主に音声とキャラクターを介して接する。音声機能が利用できないときも、テキスト入力と吹き出し表示によって同じCompanionとの体験を継続できる。必要なPC作業は、Companionが内部のAgent Harness、Computer Use、Tool、MCP、Sub-agent等の能力を使って遂行する。これらの内部機構は、日常UIでは主役にしない。

Grok Bot、ChatGPT work、Claude Cowork、Hermes Agent、nanobot等が持つAgent Harness、Skill作成・管理、記憶、自己改善の機能は、Eneの能力面における参考とする。ただし、それらの製品構成を主役として模倣するのではなく、品質、設計、UIの優先順位は一貫してCompanion-firstとする。Airi等のVTuber / AI Companion的なインターフェースも参考にしながら、「能力を持つCompanion」として統合する。

## 2. 製品の中心価値

Ene が提供する価値は、次の4つを両立することにある。

1. **一緒に過ごせる個** — ユーザーを継続的に理解し、記憶し、感情や関係性を形成し、状況に応じて自然に会話・行動する。
2. **実際に役立つ能力** — ユーザーのPC上で可能な汎用作業を、許可された範囲で調査・操作・作成・整理する。
3. **継続する存在** — 会話セッションや特定クライアントが閉じても、Host上のCore、予定、委任済みタスク、学習、Companion間の活動が必要な範囲で続く。
4. **ユーザーが制御できる存在** — 自然言語による委任と機械的なPermission / Sandbox境界を組み合わせ、何を観測・保存・送信・実行できるかをユーザーが管理できる。

Ene は「高性能なAgentにキャラクターを貼る」ことを第一目的としない。Agent能力はCompanionのための内部能力であり、製品の優先順位はAITuber / companion体験、継続的な個性・関係、そしてその個が実用的な仕事を行えることに置く。

## 3. 典型的な利用者と範囲

### 3.1 利用者

- 主利用者は1人である。
- Ene は、家族・チーム・企業が同じCoreをマルチテナントで共有するサービスを現行の基本モデルとしない。
- 「複数デバイス」は複数人を意味せず、同じユーザーが同じEneへ別の端末から接続することを意味する。

### 3.2 Host と Client

- **Host** はEne Coreが常駐し、Companionの永続状態とユーザーデータの正本を持つコンピューターである。
- **Client** はHost上の同じEneへ接続する対話・表示・操作インターフェースである。ClientはEneの状態の正本ではない。
- 通常は同じWindows / Linux PCがHostとDesktop Clientを兼ねる。
- Clientが接続していない間も、許可された範囲でHost上のCoreは活動を続ける。

### 3.3 対応プラットフォームの方針

| 区分 | 直近Betaの実装対象 | 後続実装対象 | 備考 |
|---|---|---|---|
| Host OS | Windows、Linux | macOS | Core、スケジュール、永続状態、バックグラウンド処理を実行する |
| Desktop Client | Windows、Linux | macOS | DesktopMate風マスコットを表示する |
| Remote Client | なし | Mobile、Web、その他のClient | 同じHost上のCompanionへ接続する |

直近Betaは実装順上の一区切りであり、製品全体の正式リリースではない。後続実装対象も、要件から明示的に削除されない限り正式リリースまでに実装する。各対象の実装時期、対応機能、配布形態の詳細は未確定であり、現時点で完全な詳細設計を要求しない。ただし、Remote Client等の既知の後続要件を追加するためにCoreの大規模な置換を必要とする構造へ固定しない。直近BetaのDesktop Observation / Computer UseはHost PCを主対象とし、後続Clientは音声I/O、Observation、Body、Computer Use等のCapabilityを提供できるようにする。

## 4. Companion とキャラクター

### 4.1 Companion

Companion は、ユーザーに従うだけの無人格なアシスタントではない。事前に定義された人格・価値観・嗜好・役割・背景を持ち、許された範囲で自分なりの意見、好み、反応、優先順位を示しながらユーザーを手伝う独立した個である。

同じ出来事でも、Companionごとに異なるAppraisal、感情、発話、行動を取り得る。Shared Memoryを知っていることは、そのCompanionが同じ出来事を直接経験したことを意味しない。

### 4.2 複数Companion

1つのEne Core上に複数のCompanionを存在させられる。各Companionは少なくとも次を個別に持つ。

- Personality、Values、Goals、Role、話し方
- VRM等のAvatar、将来のLive2D等のBody表現
- Voiceおよび表現設定
- Emotion、Mood、Interest
- ユーザーとのRelationship
- Companion-specific Memory / Skill
- 自発性、他Companionとの交流方針、得意分野

複数Companionを同時に表示し、ユーザーを含むグループ会話やCompanion同士の会話を行える。Companion同士は別々の存在であることを認識し、相手についての経験・記憶・感情・関係性を形成できる。

### 4.3 Character Package

キャラクターの配布単位は、人格・ストーリー・アバター・音声・感情表現・役割などをまとめた Character Package とする。Character Package はImport / Export可能な形式を目指し、必要に応じてそのCharacter向けのSkillや関連Resourceを一緒に配布できるようにする。将来的には配布・販売するMarketplaceへ拡張できる構造を保つ。

配布可能な **Character Definition** と、特定ユーザーの環境で記憶・関係性・感情を蓄積する **Character Instance** は分離する。同じDefinitionを複数ユーザーが利用しても、Instanceの経験は自動的に共有しない。成長済みInstance自体のExport / Importは要件とせず、配布物へ個人のMemory、会話・監査ログ、Relationship、Credentialを含めない。

現時点のデフォルトCompanionは Alicia とする。ただし、Aliciaを将来のEne独自キャラクターへ置き換える計画の具体的な時期や移行方式は未確定である。キャラクターが要求する能力は、ユーザーのPermissionを付与するものではない。

## 5. 主な体験

- DesktopMateのように、Companionがデスクトップ上へ常駐している。
- 通常の対話は音声入力（STT）と音声出力（TTS）を中心に行う。
- 音声入力は常時待受のVADを既定とし、Wake Word、Push-to-Talk、音声入力無効も選べる。1対1利用を基本とし、話者識別・本人認証は初期要件としない。
- STT未設定・利用不能時はテキスト入力ポップアップを使う。
- TTS未設定・利用不能時は吹き出し等のテキスト表示を使う。
- 対話履歴は別UIで閲覧できる。代替テキストUIも同じ会話・同じ記憶・同じCompanionとして扱う。
- Companionはユーザーの入力を待つだけでなく、現在のデスクトップ文脈、予定、記憶、感情、タスクなどから自発的に話しかけたり行動したりできる。
- Companionはドラッグ、クリック、位置、ウィンドウ、複数モニター等のデスクトップ空間を身体の一部として扱い、状況に応じて移動・表情・モーションを選べる。
- 直近Betaで人間に近いfull-duplex / barge-in音声会話を必須としない。ただし後続実装で追加するために会話Coreや音声I/Oを全面的に作り直すことを前提にしない。

## 6. 作業能力

Eneは用途をコーディングや調査に限定しない。ユーザーのPC・アプリケーション・Webサービス・ファイル・外部サービスで実行可能な作業を、Permissionの範囲内で汎用的に遂行できることを目指す。

代表例は、Web調査、文書・表・画像・PDFの処理、ブラウザやデスクトップアプリの操作、ファイル整理、カレンダー・メール等のサービス操作、開発作業、長時間のバックグラウンド作業である。これらは能力クラスの例であり、製品の作業範囲の上限ではない。

ユーザーは通常、HarnessやSub-agentを直接操作せず、Companionへ自然言語で意図を伝える。Companionは必要な作業をAgent Harnessへ委任し、結果をCompanion自身の言葉と表現でユーザーへ伝える。

## 7. 継続性・自発性

Companionは概念上常に存在するが、Main LLMを常時実行することを要求しない。ユーザー入力、重要なContext Monitorイベント、スケジュール、Sub-agentや他Companionからの報告、許可されたランダムな自発起動などを契機に推論する。

観測・発話・Schedule作成・外部状態を変更する作業は別の権限・自律性として扱い、すべてのActionへ事前Policyを適用する。全体の既定値はAskとし、Allow / Ask / Denyの明示Ruleとscopeごとの既定結果を一つのPolicy集合で管理する。Askはユーザーが承認するか、設定されている場合はMain Companionとは独立したApproval Reviewerが今回のみAllow、Deny、ユーザーへAskのいずれかを判断する。新しい外部変更作業は原則として開始前に通知し、事前に委任された作業、Schedule済み作業、提案の根拠を集める内部調査などは明示Allowの範囲で通知なしに実行できる。Hard Denyは「すべてAllow」やLLM判断でも解除できない。

## 8. ローカルファーストとProvider

会話履歴、Memory、Emotion、Mood、Relationship、Character Instance、設定、Permission、タスク履歴、ログ、Eneが管理する成果物などEneの永続状態はHost PCを正本とし、単一のApp Data Directory配下へ自己完結した構成で保存する。原則として、そのDirectoryを別の環境へ移動・複製すればEneのローカル状態を復元できるようにする。専用のデータ移行機能は要件としない。OSの保護領域に置くCredential、外部サービス側の状態、OS固有Integration等は、移動先で再認証・再設定が必要になり得ることを明示する。クラウドは必要な推論・音声・処理を委譲する先であり、Eneの状態の正本ではない。

LLM、VLM、Embedding、Reranker、STT、TTSその他の推論コンポーネントは、それぞれ利用可能なLocal ProviderまたはCloud Providerを選択できなければならない。コンポーネントごとの選択を独立させることで、システム全体として完全Local、Cloud中心、またはLocalとCloudが混在する構成を正規に扱う。個別ProviderがLocalとCloudを組み合わせるHybrid実行を提供することは許容するが、すべてのProviderにHybrid実行を要求しない。初回設定では、少なくともMain LLM、STT、TTSについて、推奨候補を示すことはできてもユーザー自身が利用構成を明示的に選択する。Main LLMは必須であり、未設定の構成ではCompanionを運用できず、明示的な設定エラーとする。STT / TTSは未設定を選べ、その場合はテキストUIを利用する。外部Providerの選択、費用、送信データはユーザーが理解して設定できる必要がある。

## 9. 目標

- AITuber / AI companionとして、長く一緒に過ごせる明確な個を提供する。
- Memory、Skill、Relationship、Interestを長期的に形成・更新し、Emotion / Moodは経験と時間経過に応じて継続的に変化させることで、Companionと作業の質が経験から改善する。
- 必要なPC作業をCompanionの自然な委任として実行する。
- 複数Companionが互いに別の個として存在し、共有と個別性を両立する。
- Hostを中心に、複数のClientから同じCompanionへ接続できる。
- 永続状態をユーザーの管理下へ置き、Local / Cloudの選択と、それらを組み合わせた構成の自由度を保つ。
- 常駐ソフトとして、通常のPC作業を邪魔しない軽量性を最優先する。
- 内部の高度な処理を隠しながら、必要なときには詳細設定・監査画面で挙動を確認できるようにする。

## 10. 非目標

以下は現時点のBaselineとして採用する非目標であり、今後の要件レビューで必要に応じて再確認・変更し得る。

- チーム・企業向けのマルチユーザー／マルチテナント基盤
- Eneの永続状態をクラウドサービス側で管理するSaaS
- コーディング専用Agent、または特定用途だけのWorkflow製品
- Agent Harness、Sub-agent、Tool実行、内部推論を日常UIの主役にすること
- 直近BetaからのmacOS、Mobile、Web対応
- 直近Betaから人間同等のfull-duplex / barge-in音声会話を必須にすること
- Marketplaceを直近Betaの必須機能とすること
- Skillを実行プログラムやPermission付与機構にすること
- 自動Provider fallbackを必須の継続方法とすること
- すべての観測を逐語的な内部推論、Raw音声、Raw画面、完全な低レベル操作ログとして永続保存すること
- 既存実装の構造や数値を、製品要件として維持すること

## 11. 未確定事項

以下は本書で意図的に決めていない。

- 各OS・Clientのリリース時期と対応機能の優先順位
- Remote Clientの認証、暗号化、Device trust、接続方式
- Aliciaから将来のEne独自キャラクターへ移行する具体的な方法
- Character Packageのファイル形式、署名、配布・販売モデル
- 採用するLLM / VLM / Embedding / Reranker / STT / TTS Provider、モデル、価格表
- 完全Offline動作をどの範囲まで保証するか
- 正確なCPU、GPU、RAM、ストレージ、音声・対話レイテンシの数値予算
- Emotion / Mood / Relationship / Interestの具体的なスキーマ、数値範囲、更新式
- PermissionのHard Denyに含める操作の厳密な判定方法、自然言語ポリシーの表現・編集UI、承認画面の詳細
- 「忘れる」要求で削除したMemoryの再学習を防ぐ最小保持情報と、Memory管理UIの詳細
- Plugin API、IPC、Sandbox機構、データ転送方式、Marketplaceの審査方式
- 永続ログの正確な保持期間、容量、ローテーション、暗号化、エクスポート形式
