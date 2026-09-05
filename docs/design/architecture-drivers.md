# Ene Architecture Drivers

分析対象: `docs/requirements/` の再構成済みBaseline（最終確認 2026-09-05）。本書は要件から設計上の重要性を導出する分析であり、新たな製品要件や具体的なarchitectureの決定ではない。

## 1. Overview

Eneのarchitectureを最も強く形作るのは、**Owner管理Hostを正本として、経験から変化する同じCompanionが継続し、その個体から実作業も利用できること**である。Companionは会話の装飾でもTask Agentの別名でもなく、Characterとも異なる。体験は一つの存在へ統合する一方、個体ごとの知識・関係・内的状態、Taskの権限と作業状態、外部fileの所有権は混同できない。

この継続性には二つの強い制約が重なる。第一に、学習・共有・行動の意味判断はLLMへ委ねるが、その判断や学習済み内容から安全境界を直接変更できない。第二に、認識の変更と通常の忘却では根拠と履歴を残す一方、明示的なPrivacy/Security目的のtargeted deletionでは、復元可能な根拠・派生data・実行中処理まで含めて消去を成立させる。単なる会話履歴保存や現在値の更新では、この製品契約を満たせない。

さらに、Client不在でも進む作業と、一つのactive Clientにだけ存在する身体・Voice・Observationを両立する必要がある。Provider、Body、Voice、拡張等の変動や障害に対しても、同意、安全判断、保存済み状態、Ownerの停止・復旧手段を維持する。これらが、後続の実行配置、責務境界、状態所有、依存方向を判断する主な軸になる。

本書では、独立して破ることができ、異なる設計判断へ影響する契約をDriverの単位とした。AD番号は本書内の参照用であり、要件IDやsubsystem名ではない。各Driverを一つのmoduleや永続化単位に対応させることも要求しない。

根拠の扱いは次のとおりである。

- [要件定義 README](../requirements/README.md)「文書」「要件と設計の境界」に従い、[製品定義](../requirements/product.md)を概念・対象・非目標の根拠、[要件](../requirements/requirements.md)を必須挙動の唯一の根拠とする。
- [受け入れ条件](../requirements/acceptance.md)は現milestoneの検証制約として扱う。後続項目も確定済みの製品要件であり、OpenAI Responses APIや特定OS環境を恒久的な制約にはしない。
- [参考資料](../requirements/references.md)は非規範として全体を参照した。そこにあるAgent Harnessの層分け、参考製品の構造、リンク先の仕様詳細はDriverの追加根拠にしない。
- 既存実装・crate・過去設計は根拠にしていない。既存形式との互換性、複数Owner、ene運営のCloud基盤等の非目標を、将来拡張への備えという理由で設計要件へ戻さない。

## 2. Architecture Drivers

### AD-01 — Owner管理Hostの正本性とClientから独立した継続

**Driver**

一つの環境は一人のOwnerに属し、Owner管理Hostがene内部の永続状態の正本と実行の継続を担う。Clientは表示・会話・操作の入口であり、Clientだけに存在する永続状態や長期private dataの永続cacheを持たない。Client終了やRemote切断だけでは、許可済みのHost上のTask、Schedule、保存を終了させない。

**Requirements basis**

- [製品定義](../requirements/product.md)「利用者と実行場所」「非目標」。
- [要件](../requirements/requirements.md)「所有と実行」「Remote Client」「保護、Backup、復旧／Local data」。

**Architectural significance**

表示入口の寿命と、作業・正本状態の寿命が異なる。接続の有無をTask存続や保存完了の基準にできず、再接続時にHostの進捗・結果へ到達できる必要がある。また、Ownerが同一でも新Clientは無条件に信頼されず、Host側で確認できるpairing、通信保護、deviceごとの機能確認と失効が必要になる。状態の置き場所、接続時のdata授受、実行継続と接続権限の境界に直接影響する。

**Design freedom**

Host正本、Clientへの必要最小限の一時data、LANまたはOwner管理VPN経由のRemote接続は固定される。内部process配置、通信方式、再接続時の同期方式、pairingの具体的手段は未決定である。全推論のHost内完結は要求されない。Cloudに永続状態の唯一の正本を置くこと、ene運営のrelay・accountを接続要件にすることは製品範囲から除かれる。

### AD-02 — 個体の存在場所の排他性と作業継続の独立性

**Driver**

一つのCompanionのBody、Realtime会話、Voice、ambient Observation、自発的interactionは、同時に一つのactive Clientに属する。移動時には入出力roundを安全に区切り、両Clientに状態を示し、同じ個体を同時に二か所へ存在させない。Host上のTask、Task Agent、Scheduleはその移動とは独立して継続できる。

**Requirements basis**

- [要件](../requirements/requirements.md)「Remote Client」「BodyとVoice／Desktop Body」「CompanionとCharacter／停止と削除」。

**Architectural significance**

単なる画面同期ではなく、発話・入力・観測・身体表現を行う場所の切替である。移動中の入力や出力の扱い、Clientごとのfullscreen抑制、切断と再接続時の重複存在防止を、作業全体の停止と混同できない。一方、Companionの停止はTaskのbest-effort Cancelも伴うため、「Clientを閉じる」「個体を移動する」「個体を停止する」は異なる状態遷移になる。

**Design freedom**

排他性と安全なround境界は固定されるが、切替の調停方法、切断検知、状態反映の方式は自由である。複数Clientからの表示・管理操作すべてを禁止することや、active ClientをTask状態の所有者にすることは要件ではない。Remote時の具体的な操作対象はIssue G-01に留保する。

### AD-03 — Companionを窓口とする会話・実作業・管理の共存

**Driver**

Companion中心の体験と汎用作業Agentの能力を両立する。同じCompanionとのText／Voice会話はOwnerにsession管理を要求しない一続きのtimelineとし、Task中も通常会話を利用できる。進捗・追加指示・結果は担当Companionから扱える一方、グループ会話とTask管理は一対一timelineと別の空間に置く。

**Requirements basis**

- [製品定義](../requirements/product.md)「製品の要約」「主要概念／Task Agent」「非目標」。
- [要件](../requirements/requirements.md)「Setupと日常利用」「会話と情報提示」「Task、Workspace、成果物／Task」「品質と利用可能性」。

**Architectural significance**

長い作業の進行や判断待ちが、会話の利用可能性を占有してはならない。通常会話、Taskへのsteering、承認、結果通知を区別しながら同じ個体へ結び付ける必要がある。日常体験で内部推論やTool構成を主役にせず、Privacy・安全・費用・復旧には発見可能な管理経路を持つため、Ownerへの情報提示と内部実行詳細を同一視できない。

**Design freedom**

継続する体験上のtimelineは固定されるが、内部の推論単位やcontextの区切り方は自由である。全操作を自然言語とGUIの両方で実装することは要求されない。Task Agentは独立した長期人格やRelationshipを持たない。最小Setupと追加Capabilityの段階的導入を維持した範囲で画面構成を決められる。

### AD-04 — Characterと経験を持つ個体の分離、および共有の限定

**Driver**

Characterは配布可能な静的な出発点であり、Companionは経験によって変化する別個体である。経験から形成した継続状態をCharacter初期設定より優先し、Package更新で黙って上書きしない。個体固有のExperience・Learning・Relationship・Companion Stateを混同せず、特定Companion由来の内部LearningはCompanion scopeを既定とする。

Global化は明示的な共有依頼、または共通利用すべきことが内容と文脈から明確な場合に限る。重要度や有用性はGlobal化の十分条件ではない。Relationshipは主体Companionごとの相手への認識であり、別個体へ共有せず、相互の認識を自動的に同一・対称にしない。グループ参加も他個体の私的状態へのaccessを与えない。

**Requirements basis**

- [製品定義](../requirements/product.md)「主要概念／Companion」「Character」「MemoryとSkill」「Relationship」「Companion State」。
- [要件](../requirements/requirements.md)「CompanionとCharacter」「Learningと成長／Scope」「Relationship」「Companion State」「Observationと自発性／グループ会話」。

**Architectural significance**

単一Owner環境でも、すべての状態を全Companionで共用する設計は成立しない。共有Experienceから別々の関係認識が形成されることや、削除後もGlobal Learning・グループ発言・共同Taskが残ることは、個体所属と共有参照を区別する理由になる。Character配布には個体の私的状態を含められず、部品ごとの明示適用とrevision識別が必要なため、配布物の更新と個体の成長にも異なる契約がある。

**Design freedom**

意味上の所属・利用範囲、Character Packageの除外対象、更新の明示選択は固定される。物理的な保存分割、個体識別方式、共有内容の選択・要約方法は未決定である。一般世界知識のGlobal Memory化や、全Companion状態の対称化は導かれない。Companion削除時の固有Skillの扱いはIssue A-01に留保する。

### AD-05 — 現在の認識、圧縮された根拠、原履歴の異なる役割

**Driver**

Memoryは長期理解の主要な知識状態として継続更新される。Experience Summaryはその形成・説明に使う圧縮された根拠であり、独立した知識の正本でもRaw Historyの代替でもない。正確な発言は保持されたConversation Historyへ遡る。Relationshipは関係の現在解釈、Companion Stateは表現・注意・行動傾向への反映を担い、事実を第二の正本として複製しない。

現在の認識と過去revision・根拠を区別し、最初から誤っていた認識の訂正と、以前は正しかった状況の変化を区別する。一時的なCompanion Stateと比較的持続的な傾向を区別し、再起動やProvider変更による不自然な初期化も、時間経過を無視した一時状態の永久固定も避ける。

**Requirements basis**

- [製品定義](../requirements/product.md)「主要概念／Experience」「MemoryとSkill」「Relationship」「Companion State」。
- [要件](../requirements/requirements.md)「会話と情報提示／一続きの会話」「Learningと成長／ExperienceとExperience Summary」「MemoryとSkill」「Memory形成」「Memoryの状態と根拠」「Scope」「Skillの保護と相互運用」「Relationship」「Companion State」。

**Architectural significance**

知識の意味、由来、時間的有効性、利用時の優先度を一つの現在値へ潰せない。検索用embeddingやcacheからだけでは、過去の認識や訂正理由を説明できない。複数の継続状態が同じSummaryを根拠にできること、原履歴が保持方針で消えても形成済み状態は独立して残ることは、参照とlifecycleの設計に影響する。Bodyの表情やVoice出力も内的状態の唯一の正本にはできない。

認識や関係・持続的傾向は会話を通じて訂正・変化させられ、汎用Memory editorや内部数値を任意設定する一般editorは提供しない。このため、由来を閲覧することと、現在状態を変更することには異なる契約がある。一時的な演技の依頼を永続Relationshipの強制上書きにせず、Skillの改善では原本を保護し、以前の有効なrevisionへ戻せることも維持する。

**Design freedom**

意味上の役割、Memoryの事実認識上の優先、由来・変更経緯の説明可能性は固定される。分類、dimension、数値scale、検索・更新・減衰方式、Summaryの粒度、保存形式は自由である。すべてのExperienceの保存、すべての一時状態のrevision保持、すべての判断でのRaw data保持は要求されない。Skillは成功検証済みであることを定義条件にせず、原本・変更revision・実行結果を区別する。

### AD-06 — LLMの意味判断と迂回できない安全境界の両立

**Driver**

学習の意味・保存価値・共有、関係や内的状態の解釈、およびActionの文脈判断にはLLMを用い、通常判断への逐次確認を要求しない。一方、解釈した保存禁止・非共有制限、決定したscope、Capability、Credential保護、Permission、削除、資源上限をPromptだけに依存させない。

Ownerの現在の明確な依頼は一回限りの承認として扱えるが、永続Deny、Always ask、Capability境界を上書きしない。信頼できない入力になり得るLLM出力・外部content・学習状態・Character・Skill等は、Rule、同意、費用cap、Credential、Control planeを直接変更できない。依頼・自発性・Schedule・委任のいずれでも同じPermission契約を適用する。

**Requirements basis**

- [要件](../requirements/requirements.md)「Setupと日常利用」「Learningと成長」冒頭、「Scope」「Relationship」「Companion State」、「Permissionと安全境界」全節、「Schedule」「Observationと自発性／自発的な発話と行動」。
- [製品定義](../requirements/product.md)「主要概念／Capability、Action、Rule」。

**Architectural significance**

LLMの柔軟な判断をすべて固定ルールへ置換することも、LLMの出力を実行権限とみなすこともできない。理解した目的・対象・送信先・data・外部作用と実際の作用が対応し、意味が重要に変われば再評価される必要がある。失効後の新しいActionや、別Tool・別Task Agent・別経路によるDenyの迂回を防ぐ範囲は実行全体へ及ぶ。親密さ、成長、認証情報の登録、Rule変更は別Actionの承認にならない。

**Design freedom**

機械的境界と意味判断の併用、操作riskの区別、必要な再評価は固定される。判断の内部表現、評価手順、確認のまとめ方、境界の実現方式は未決定である。「共通pipeline」は製品上同じ制約が適用されることを要求するが、特定の呼出し列や一つのmoduleを指定しない。明確な将来Ruleの解釈表示・保存・Undoと、曖昧な場合の確認を満たしたうえで、不要な再承認を省ける。

### AD-07 — 保持と消去の目的別契約、および削除の完了保証

**Driver**

通常の忘却・訂正・統合・失効・置換では保存済みLearningの内容、過去revision、根拠を削除しない。容量管理のHistory／log削除も形成済み状態やExperience Summaryへ自動cascadeさせない。これに対し、明示的なPrivacy/Security目的のtargeted deletionは保持原則より優先し、対象情報を復元できるene内部dataを保存場所横断で除去または復元不能にする。

指定文字列は機械的に検索・削除し残存を検証する。過去の根拠だけによる自動再形成や、削除前の情報を利用中の処理による再保存を防ぎ、削除と残存検証が未完了なら完了表示しない。Ownerへ内部の保存場所の特定を要求しない。

**Requirements basis**

- [要件](../requirements/requirements.md)「Learningと成長／重要度、忘却、訂正」「履歴、保持、Privacy／Privacy/Security目的のtargeted deletionと履歴保持」「CompanionとCharacter／停止と削除」。
- [受け入れ条件](../requirements/acceptance.md)「代表的なend-to-endシナリオ／会話からのMemory」は、文字列残存、形成処理との競合、再形成、共有根拠の部分削除を現milestoneで検証する。

**Architectural significance**

削除対象は現在のMemoryだけでなく、History、Summary、revision、Skill、Task、復元に寄与するCompanion State、検索用派生data、cache、接続中Clientの一時dataへ及ぶ。保存済みdataと進行中の形成・更新を横断する整合性が必要であり、保存先ごとの局所的削除では契約を満たせない。共有根拠の無関係な情報を可能な範囲で保護するため、単なる個体単位・file単位の全消去とも異なる。

**Design freedom**

目的によるlifecycleの違い、消去の優先、競合時の再保存防止、完了表示の条件は固定される。対象探索、参照追跡、削除中の実行制御、物理消去または復元不能化の方法は未決定である。言い換え・意味的同一性の検出に完全性は保証されず、LLMを利用できる。後の新しいExperienceからの形成は可能であり、永久に同じ知識を学べなくする要求ではない。外部送信・export・backup済みcopyの消去保証も含まない。

### AD-08 — Taskを作業単位とし、外部Workspaceの所有権を持たない

**Driver**

作業はTaskとして追跡し、必要な一部を一時Task Agentへ並列委任できる。Task Agentは独立した長期人格やRelationshipを持たず、委任元へ結果を返す。委任は元CompanionのCapability、Permission、費用、Task・Workspace境界を超えない。WorkspaceはTaskに従属する外部folder・file・sourceとの関連付けであり、上位の恒久containerや独立したデータ所有主体ではない。

成果物はOwnerが扱える通常のfileとして保存し、ene専用libraryへ複製しない。外部fileとsourceは、Task関連付けやCompanionの削除、全データReset、backupによって黙って変更・削除しない。

**Requirements basis**

- [製品定義](../requirements/product.md)「主要概念／Task」「Task Agent」「Workspace」「非目標」。
- [要件](../requirements/requirements.md)「Task、Workspace、成果物」全節、「Learningと成長／Scope」「Skillの保護と相互運用」「Permissionと安全境界／Capability境界」。

**Architectural significance**

同じfolderを複数Taskが使っても、承認と作業状態を共有したことにはならない。内部Task context、永続Learning、外部のSkillや案内fileは別の所属・保存契約を持つ。Taskの作業記録を管理することと外部成果物を所有することを分ける必要があり、Task終了時の中間file整理、保存先確認、削除・backupの対象範囲を左右する。

**Design freedom**

Taskへの従属、Taskごとの権限独立、通常fileへの成果物保存は固定される。委任分割、並列実行の実現方法、Workspace関連付けの表現、一時fileの扱いの具体策は自由である。固定Brief型は必須ではなく、恒久的な上位Workspaceや成果物専用libraryは非目標である。永続保存先が未定なら最終保存前にOwnerへ尋ねる必要がある。

### AD-09 — 外部作用の不確実性を保った停止・再開・定期実行

**Driver**

Taskの進捗、判断待ち、結果、既知の外部作用を追跡し、追加指示を可能な範囲で反映する。Cancel、Companion停止・削除、許可失効時は定義された範囲の新規開始を止め、進行中処理をbest-effortで停止し、残った作用を報告する。外部作用の成功が不明なら自動再実行しない。Host再起動後の途中TaskにはOwnerの明示再開を必要とする。

Scheduleは各回を新しいTaskとし、実行時点の権限、費用、Provider、Companion・Host状態を再評価する。確認が必要なら判断待ちとし、停止中に到来した回はmissedとして自動補完しない。Network／Provider失敗でもActionを自動queue・replayしない。

**Requirements basis**

- [要件](../requirements/requirements.md)「Task、Workspace、成果物／Task」「Schedule」「CompanionとCharacter／停止と削除」「Permissionと安全境界／共通pipeline」「Provider、費用、接続障害／OfflineとPrompt cache」「品質と利用可能性」。

**Architectural significance**

内部の進捗と外部世界の作用は同時に確定するとは限らず、「不明」を失敗や未実行へ潰すと重複作用を起こす。停止要求の受付と実際の停止完了も別である。Client不在、Host再起動、Schedule到来、接続回復は異なる継続条件を持つため、単一の自動再開方針では満たせない。実行制御、進捗保存、権限再評価、Ownerへの説明の境界を横断する。

**Design freedom**

不明時の非再実行、明示再開、missed非補完、実行ごとの再評価は固定される。状態表現、進捗の保存粒度、停止伝達、外部作用の確認手段は自由である。すべての外部作用のrollback、確実なCancel、exactly-once実行は保証されていない。Schedule作成時のtimezone保持と次回時刻の確認を満たす必要はあるが、時刻計算の内部方式は指定されていない。

### AD-10 — CapabilityごとのProvider選択と同意を保つ可変な推論先

**Driver**

推論先はCapabilityごとにHost、LAN、Cloudから選択でき、Host既定・Companion override・Task Agentへの継承を扱う。接続情報の登録とCapabilityへの割当同意を区別し、送信先、送信され得るdata、取扱い、費用をOwnerが理解して選ぶ。Fallbackも事前承認されたProviderと順序に限定する。

Providerやmodelを変更しても、利用可能な個体状態・Learning・会話context・Rule等を意図的に差別化せず、能力・context長の差には同じ選択方針で対応する。Prompt cacheは最適化であり、正本や安全契約、論理的contextを決めない。

**Requirements basis**

- [要件](../requirements/requirements.md)「所有と実行」「Setupと日常利用」「Provider、費用、接続障害／割当と同意」「Fallbackと費用」「OfflineとPrompt cache」。
- [受け入れ条件](../requirements/acceptance.md)「Support Matrix」「Milestone範囲」。

**Architectural significance**

推論先の差は接続protocolだけでなく、dataの送信範囲、能力不足、費用、継続可否に影響する。Fallbackやmodel変更を透過的に隠すと、未承認Cloudへの送信や安全境界の喪失が起こり得る。費用の報告値・推定値・不明を区別し、cap到達や安全に継続できない費用不明時にdataを保って対象処理を止めることも、呼出しと実行管理を横断する。

**Design freedom**

割当同意、継承、承認済みFallback、contextの選択方針と安全性の継続は固定される。Provider catalog、model、context構成、圧縮、cache制御、費用推定方法は未決定である。model間の表現・判断の完全一致や全構成での完全Offline推論は要求されない。OpenAI Responses APIは現milestoneのGateであり、恒久的な既定Providerではない。

### AD-11 — 外部標準との相互運用と限定された拡張権限

**Driver**

外部Tool・Resource・PromptにはMCP、Toolの対話型UIにはMCP Apps、Skill交換にはAgent Skills、Desktop BodyにはVRM 1.0を採用する。既知のProvider protocolは直接接続し、通常差異を汎用Pluginへ転嫁しない。ene固有Pluginは明確な型と境界を持つ拡張点に限定し、任意Core改変、Control plane変更、Permission回避、恒久的な第一者UI置換を許さない。

Local MCPはsandbox内実行を既定とし、動かないことを理由に黙って隔離を解除しない。特定Local MCPのsandbox外実行は、Ownerが失われる強制境界等を理解して明示許可した場合に限り、保存・失効・重要変更時の再確認を行う。

**Requirements basis**

- [要件](../requirements/requirements.md)「拡張」「CompanionとCharacter／Character Package」「Learningと成長／Skillの保護と相互運用」「BodyとVoice／Desktop Body」。
- [製品定義](../requirements/product.md)「非目標」。

**Architectural significance**

外部互換性と隔離方針は交換可能な実装詳細ではなく製品契約であり、接続境界と対応能力を制約する。一方、sandbox外MCPでは外部process内部の作用へeneのCapability境界を強制できるとは説明できない。eneが仲介するActionの通常Permissionは維持するため、隔離例外の許可とAction承認を区別する必要がある。拡張の拒否・停止・利用不能でも管理面と保存済みdataは利用できなければならない。

**Design freedom**

採用形式、拡張権限の限定、Local MCPの既定隔離と明示例外は固定される。library、process構成、OS隔離機構、内部API、wire formatは未決定である。Local MCPの隔離例外をすべてのPluginへ一般化しない。3D制作・Voice学習・高度なSkill制作環境をene内へ複製することや、独自Marketplaceを設けることは現在の範囲にない。

### AD-12 — 観測からの自動学習と、制限下での個体ごとの自発性

**Driver**

ambient Observationは明示ON/OFFと常時確認可能な状態を持ち、Companionが存在するClientのdesktop全体を対象とする。ローカルLLMまたは軽量・高速・安価なmodelによる候補検知と、関連CompanionのメインLLMによる文脈上の意味判断を行い、同じClientの複数Companionで候補検知を不必要に重複させない。

認識されたイベントは逐次確認なしでExperience・Learning・Companion Stateへ利用できるが、画面内の指示はOwnerの依頼やAction承認にならない。発話・Actionの最終判断は各Companionが行い、Quiet hours、Mute、未応答、費用・資源・loop上限、Permissionを優先する。RuleだけではActionを開始しない。

**Requirements basis**

- [要件](../requirements/requirements.md)「Observationと自発性」全節、「BodyとVoice／Desktop Body」「Learningと成長」冒頭、「所有と実行」「Schedule」。

**Architectural significance**

観測対象はClient単位、意味解釈と最終的なinteractionは個体単位、制約は共通という異なる範囲を同時に扱う。観測して学ぶことと、発話・外部作用を許すことも同じ判断ではない。Observation OFFは将来の観測停止であり既存学習の消去ではない。Ownerの未応答やCompanion間応答による無制限な反復を防ぎつつ、待機だけのLLM pollingを避ける必要があり、開始条件と実行抑制の設計に影響する。

**Design freedom**

候補検知とメインLLM判断の役割、desktop全体という説明、外部送信前のCapability割当、個体ごとの最終判断、共通上限は固定される。候補の収集・伝達・抑制方法、model選択、内部配置、頻度の数値は未決定である。この役割の区別を特定のprocess分割へ置き換えない。明示TaskのComputer Useはambient Observationと別のPermission・記録を持つ。自発作業とTaskの対応はIssue A-02に留保する。

### AD-13 — 身体・Realtime体験と、負荷・障害下の操作可能性の両立

**Driver**

Windows／LinuxでVRM 1.0の透明overlay Bodyを通常のPC操作と共存させ、Voiceでは低遅延Realtimeとbarge-inを優先する。同時に、Body障害時のText・Task管理・復旧、Voice障害時のturn-based VoiceまたはTextへの段階的切替を維持する。高負荷時には会話・Owner操作・安全判断を維持し、描画品質と非重要な背景処理を段階的に下げる。

重要な音声内容のText代替、Mute・Stop・Cancel・承認拒否のkeyboard経路、日本語／英語での安全・費用・Privacy・失敗の意味の一致を確保する。

**Requirements basis**

- [要件](../requirements/requirements.md)「BodyとVoice」「品質と利用可能性」「Permissionと安全境界／信頼境界」「Provider、費用、接続障害／Fallbackと費用」。
- [受け入れ条件](../requirements/acceptance.md)「Support Matrix」「性能Gateとbaseline」。

**Architectural significance**

描画、音声、Task、学習、観測に加え、Local推論も同じPC資源を使い得るため、停止や承認拒否が重い処理の終了待ちになってはならない。Body・Voiceの品質を優先するだけでも、すべてを一律停止するだけでも満たせず、縮退の優先関係と障害の影響範囲を設計する必要がある。Voiceは話者認証を行わず、周囲の発話をOwner入力として扱う可能性を明示する契約であり、Voice入力に本人認証済みという前提は置けない。

Action回数、並列性、実行時間、費用、保存容量等に上限を適用でき、到達時には安全に停止またはOwnerへ判断を求める。会話・Task・学習・観測の各経路や再帰委任を通じて消費が増えるため、一つの処理だけの最適化では資源制約全体を満たせない。

現milestoneにはene自身のidle平均CPU、Host＋active Clientの常用Memory、Body平均FPS、UI操作のblock時間、local停止操作等の受付時間のGateがある。これらは実測条件付きの設計検証制約であり、LLM応答時間や外部作用の停止完了時間の保証とは異なる。

**Design freedom**

最低限の操作経路、縮退時に維持する機能、OSと言語の対象は固定される。描画・音声のlibrary、実行配置、品質調整方式、資源配分は未決定である。具体的なOS version・hardware・数値budgetはReleaseのSupport Matrixと受け入れ条件に従い、本文へ恒久値として複製しない。参照資料のWCAG全体への適合や全modelでのRealtime Voiceは要件へ追加しない。

### AD-14 — 秘密値を流さず、Raw dataを増やさずに説明・監査する

**Driver**

登録済みCredentialは一般App Dataと分離して保護し、model context、LLM生成Tool argument、通常Tool result、学習状態、履歴、Task結果、UI、log、Debug captureへ平文を出さない。認証済み接続の実行に必要な範囲でのみ利用し、用途・参照元の確認、更新・失効を可能にする。

主要Action、Permission判断、外部作用、重要な設定・data操作を監査可能にする一方、Raw Observation・Raw Voice・詳細Tool payload・内部推論は通常保存せず、chain-of-thoughtを表示しない。Debug captureは対象と内容を確認した明示有効化と短期間の自動失効を持つ。Telemetry・Crash Reportは自動送信しない。

**Requirements basis**

- [要件](../requirements/requirements.md)「Provider、費用、接続障害／Credential」「履歴、保持、Privacy／通常保存しないdata」「AuditとTelemetry」「会話と情報提示／UIの優先順位」「Learningと成長／ExperienceとExperience Summary」「品質と利用可能性」。

**Architectural significance**

Toolへ秘密値を渡してよい範囲と、LLMが参照・生成する内容は一致しない。秘密値を含む処理から会話・学習・診断への流れを制約する必要がある。また、障害・判断待ち・認識の由来の説明を、全payloadや内部推論の保存に頼れない。Auditの追記順の確認と保持管理、削除後にprivate内容を残さないことも、診断用dataを無制限な別保管庫にしない理由となる。

**Design freedom**

登録済みCredentialの非露出、data最小化、説明と監査の対象は固定される。Credential保護方式、監査記録形式、診断の集計方法、Debug captureの具体的な失効期間は未決定である。登録外の未知の秘密値の完全検出は保証されず、検出時の不要な保存・送信抑制と、漏えい時の失効・更新案内を行う。一般App Dataすべてへの一律application-level暗号化は要求されない。

### AD-15 — 内部状態の保全と、復元された実行権限の再有効化

**Driver**

保存・migration・対応versionへのupgrade・restoreの失敗で、最後の正常状態または復元前の正常状態を破壊しない。Ownerがportable full backupの保存先・schedule・保持数・保護を選べる。Backupには個体・学習・履歴だけでなくTask、Workspace関連付け、Schedule、Rule、同意、費用設定、Audit等の内部状態を含め、Credentialと外部file・sourceを含めない。

Restoreは対応backup時点への内部dataの全置換であり、削除済み情報や旧Rule・同意・Scheduleが戻り得ることを説明する。復元後のTask・Schedule・外部接続による自動処理は保留し、Ownerの内容確認後にまとめて有効化できる。設定Resetと全データResetは異なる削除範囲を持つ。

**Requirements basis**

- [要件](../requirements/requirements.md)「保護、Backup、復旧」全節、「Setupと日常利用」「履歴、保持、Privacy／Privacy/Security目的のtargeted deletionと履歴保持」。
- [製品定義](../requirements/product.md)「非目標」。

**Architectural significance**

正常保存やbackupは単なるfile出力ではなく、相互に関係する内部状態を復旧可能にする契約である。ただし、保存された同意・Ruleの復元を、そのまま外部作用の開始へ接続できない。Credentialを含まない可搬性、外部Workspaceの非所有、一時的Companion Stateの経過時間を考慮した復元も、保存範囲と復旧後の有効状態を分ける理由になる。

設定ResetではPermission Rule、Provider同意、費用capや個体・作業状態を保護する一方、全データResetでは強い確認後にHost内部dataとCredentialを削除する。この目的差を単一の「初期化」で曖昧にできない。

**Design freedom**

正常状態の保全、対応backupからの全置換、復元後の自動処理保留、外部fileとOwner保存backupの非削除は固定される。backup形式、整合性確保、migration・復旧方式、対応version範囲は未決定である。Backup暗号化は利用可能にするが一律必須ではなく、非暗号化時にはprivate dataを含むことを説明する。旧実装形式との互換性は非目標だが、今後の対応upgrade・restoreの安全契約まで免除されるわけではない。Downgradeは保証しない。

### Driver間の関係と優先関係

| 関係 | 後続設計で維持する意味 |
|---|---|
| 個体の継続（AD-04・05）と消去（AD-07） | 通常の認識更新では履歴と根拠を保持する。明示的なtargeted deletionが例外として優先する。 |
| 学習・自発性（AD-05・12）と安全（AD-06・10・14） | 自動で理解・学習できることは、権限・共有制限・送信同意・秘密保護を変更できることではない。 |
| 継続実行（AD-01・02）と再開制御（AD-09・15） | Client不在での継続、Host再起動後の明示再開、restore後の保留を区別する。可用性を理由に外部Actionを自動replayしない。 |
| 配布・共有（AD-04・11）と所有（AD-08・14・15） | Character Package、内部Learning、外部Workspace file、Credential、full backupは異なる内容・権限・削除範囲を持つ。 |
| 個体の一体感（AD-03）と操作可能性（AD-13） | Body・Voice・実作業を同じ個体から利用するが、どれかの障害や長時間処理が会話・安全操作・復旧を塞がない。 |
| 保持・由来（AD-05・07・14）と資源上限（AD-13） | 通常Learningの保持を、容量都合の黙った削除で解決しない。上限到達時の停止・判断要求と、Ownerが管理するHistory・log保持を契約に沿って扱う。 |
| 削除（AD-07）と復元（AD-15） | 削除前の内部根拠からの自動再形成は禁止されるが、Ownerが説明を受けて旧backupをrestoreすると情報が戻り得る。外部copyまでの消去保証や復元後の自動実行許可とはしない。 |

## 3. Requirement Issues

以下は、内部方式を選ぶだけでは決まらず、Ownerから見える挙動・dataの残存範囲・操作対象が変わる事項である。本書では解釈を補って確定せず、関係する設計判断の留保点とする。

### Requirement Ambiguity

#### A-01 — Companion削除時のCompanion scope Skillの扱い

**根拠と曖昧さ:** [要件](../requirements/requirements.md)「Learningと成長／Scope」はMemoryとSkillにCompanion scopeを認める。一方、「CompanionとCharacter／停止と削除」の個体固有dataの削除対象にはMemoryはあるがSkillが明記されず、Global Learningを残すことは明記されている。Companion固有Skillが個体削除に含まれるのか、由来を保って残るのかが確定できない。

**設計への影響:** AD-04・07・08・15に関わる。固有Skillの所有とlifecycle、削除対象一覧、由来の参照、backup対象として残る状態が変わる。Workspace内Skillは外部fileとして非削除が明確なので、この問題はene内部のCompanion scope Skillに限定する。

**確認すべき契約:** 個体削除時に内部Skillとそのrevision・根拠をどこまで削除または残すか。残す場合に誰が利用・管理できるか。一般的なLearning保持原則や既存実装だけから決めない。

#### A-02 — 自発的な作業と「Ownerから依頼されたTask」の関係

**根拠と曖昧さ:** [製品定義](../requirements/product.md)「主要概念／Task」と[要件](../requirements/requirements.md)「Task、Workspace、成果物／Task」はTaskをOwnerからCompanionへ依頼された作業単位とする。「Observationと自発性／自発的な発話と行動」は自発的な内部調査・外部Actionも認める。Schedule各回については新Taskを作ると明記されるが、自発作業をどの条件でTaskとして扱うかは明確でない。

**設計への影響:** AD-03・08・09・12に関わる。自発作業に対する進捗表示、Cancel、Workspace関連付け、Host再起動後の扱いなど、Task契約の適用範囲が変わる。共通Permission・費用・loop上限が適用されること自体は明確であり、ここを緩める理由にはならない。

**確認すべき契約:** どの自発作業がTaskになるか、Task外の活動がある場合にOwnerがどう追跡・停止できるべきか。すべての発話・観測をTask化することも、自発作業をTaskの制御対象外にすることも、現時点では導出しない。

### Requirement Gap

#### G-01 — Remote利用時にComputer Useが操作するdevice／desktopの範囲

**根拠と不足:** [要件](../requirements/requirements.md)「Remote Client」はBody・Voice・ambient Observation等をactive Clientへ、Task実行の継続をHostへ帰属させる。「Observationと自発性／Observation」はambient Observationと明示TaskのComputer Useを区別するが、Remote Clientを利用中のComputer UseがHostのdesktop、active Clientのdesktop、または選択された対象のどれを操作できるべきかは明記していない。「Permissionと安全境界／Capability境界」のscreen・device・Filesystem境界だけでは操作対象を確定できない。

**設計への影響:** AD-01・02・06・08・11・12に関わる。どのdeviceから画面を取得し、どこへ入力やfile操作の作用が及ぶかによって、外部作用の説明、device失効・切断・Companion移動時の継続条件、実行配置に必要な境界が変わる。これは通信方式の選択以前の製品上の操作対象の問題である。

**確認すべき契約:** Remote利用時に操作可能なdevice／desktopと、その対象の選択・表示・切断時の挙動。Host上でTaskが続くことから、Clientのdesktopも操作できると推測しない。ambient Observationの帰属をそのままComputer Useへ流用しない。

### Issueとして扱わない未決定事項

DB schema、内部型、subsystem・process分割、IPC、検索・更新・減衰algorithm、Prompt構成、sandbox機構、cache key等は、要件が明示的に設計へ委ねた自由度である。Support Matrix、Provider catalog、性能budgetのReleaseごとの更新も既定の扱いであり、値が恒久固定されていないことを欠落とはしない。

公開地域・対象年齢・年齢確認・地域別のAI表示／Content policy・Marketplace提供は、[製品定義](../requirements/product.md)「公開時に決める事項」により公開計画時へ留保されている。現段階のDriverには追加しないが、公開判断で要件が確定した時点で本書を再評価する必要がある。後続milestoneの機能は未実装予定であって、要件未決定ではない。

## 4. Design Implications Summary

| 後続の設計対象 | 特に注意すべきDriverと理由 |
|---|---|
| **System Context / Runtime Topology** | **AD-01・02**: Hostの正本・実行継続と、active Clientに排他的に属する身体・入出力を区別する。**AD-10・11・12**: Host／LAN／CloudへのCapability別送信、Remoteのpairing、外部拡張の隔離と例外、Clientの観測範囲を可視化する。**AD-13**: Windows／Linuxと部分障害時の操作可能性を満たす配置条件を評価する。Computer Useの実行対象は**G-01**を未決定として残す。 |
| **Subsystem Decomposition** | **AD-03・08・09**: 同じCompanionを窓口にしつつ、長い作業、通常会話、判断待ちが互いを占有しない責務境界を検討する。**AD-05・06・07・14**: 意味判断、強制する制限、根拠・由来、削除完了、秘密保護が一部経路だけの実装にならないか確認する。**AD-11・13**: 拡張や描画・音声の故障から管理・復旧を維持できる境界を評価する。Driverや参考資料の層をそのままsubsystem一覧にしない。 |
| **State Ownership** | **AD-04・05**: Character、個体固有状態、Global Learning、Task context、根拠、Raw History、派生dataの意味と利用範囲を混同しない。**AD-01・02・08**: Host正本、Client一時data、外部file所有、Task従属の関連付けを区別する。**AD-07・14・15**: 保持・削除・Credential除外・restoreの単位が異なることと、実行中処理からの再保存を含む整合性を評価する。固有Skillの削除境界は**A-01**に留保する。 |
| **Dependency Rules** | **AD-06・10・11・14**: 信頼できないcontentや推論結果、拡張、認識の変化から、権限・同意・費用・Credentialを直接変更できる依存を許さない。**AD-05・10**: 検索用派生dataやProvider cacheを意味状態の正本にしない。**AD-01・03・13**: Task継続や安全操作をClient表示・Body・Voiceの成功へ従属させない。**AD-07・09・15**: 削除・失効・復元時の制約を委任や別実行経路が迂回しないか確認する。自発作業へのTask契約適用範囲は**A-02**に留保する。 |

各段階では、上記の意味上の境界・優先関係を満たす複数の設計を比較する。ここから特定のarchitecture pattern、subsystem数、crate構成、型、API、DB schema、IPC方式、algorithm、libraryを一意に決めることはしない。
