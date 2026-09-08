# Subsystem Detailed Design — Character構成・配布

本書は Step 12 Subsystem Detailed Design として、**Character構成・配布（以下、Character）**単独を詳細化する。Step 12で未詳細化の最後のSubsystemである。

本書内の CH 番号は本書内の内部責務の参照用、CD 番号は本書内のSubsystem間semantic contractの参照用であり、製品要件IDではない。
「要求」「対応」「区間」「保留」「受入」「完了」は論理的な関係であり、共通object、永続record、protocol、state machine、enumを指定しない。

## 0. 位置付けと不変条件

製品挙動の source of truth は [要件Baseline](../requirements/README.md)、[製品定義](../requirements/product.md)、[要件](../requirements/requirements.md)とする。[受け入れ条件](../requirements/acceptance.md)も検証範囲へ含め、[参考資料](../requirements/references.md)は非規範として扱う。既存実装から製品挙動を補わない。

責務・非責務は [Subsystem Decomposition](subsystems.md)第3・5・6節、意味owner・lifecycleは [State Ownership](state-ownership.md)（以下SO）第4〜8節、依存の許可・禁止・制約は [Dependency Rules](dependency-rules.md)（以下DR）第3〜7節、内外境界は [System Context](system-context.md)、配置・寿命・trust / failure boundary は [Runtime Topology](runtime-topology.md)を維持する。[Cross-cutting Design](cross-cutting.md)の CC-01〜07 を前提とし、再定義しない。

Step 11 の5詳細設計は特定Subsystemの新しいownerではない。本書は次を隣接する確定済みcontractとして利用し、再定義も所有権の移動もしない。

- [Context Assembly](context-assembly.md)：由来・用途・現在性・用途別結果受入・処理中無効化の一般契約
- [Action Execution](action-execution.md)：判断対象と実対象の対応・委任不変・確定度・不明保持・遅延帰属の一般契約
- [Targeted Deletion](targeted-deletion.md)：消去区間・再保存防止・未完了保全・完了条件の一般契約
- [Client Presence Transition](client-presence-transition.md)（以下、Presence Transition）：authoritative帰属・切替区間・活動別区切り・到着物帰属・Host継続の一般契約
- [Backup / Restore](backup-restore.md)（以下、Backup-Restore）：復元範囲・正本切替・再有効化・stale・旧backup交差の一般契約

既存Step 12 artifactは確定済みcontractとして扱い、再定義も所有権の移動もしない。

- [個体継続・対話調整 / 作業遂行・実行管理 / 経験に基づく認識・学習](subsystem-companion-task-learning.md)（以下、個体・作業・学習）：同一性・適用関係・History・Summary・Memory・Skill・Relationship・Companion Stateの意味ownerと受渡し H-1〜H-10
- [権限・利用制約 / 認証秘密の保護 / Action実行・拡張受入 / 推論利用](subsystem-permission-credential-execution-inference.md)（以下、権限・実行クラスタ）：制御確定・秘密・作用・割当の意味ownerと受渡し K-1〜K-12
- [Client接続・存在調停 / Owner入出力・体験提示 / 共有観測・関連付け](subsystem-client-presence-io-observation.md)（以下、接続・提示・観測）：帰属・round・対象・routingの意味ownerと受渡し X-1〜X-10
- [内部データ保全・消去](subsystem-data-preservation-erasure.md)（以下、保全・消去）：全域操作の調整と受渡し DP-1〜DP-8

新しい semantic owner、第二の正本、万能Character Manager / Package Service / Distribution Coordinator、統一character state machine、共通package layerを追加しない。Subsystem と crate / process / service を一対一に対応させない。Host側とClient側に同名Subsystemを複製しない。

本書の active Client不在時の活動継続・移動・復旧は Running Companion の契約である。Stopped 個体はどのClientにもHostにも presence を持たず、保存dataや再配置hintは現在帰属ではない。Clientに依存しない活動も個体停止の禁止を迂回しない。

`subsystems.md` で確定したCharacterの非責務を維持する。個体の成長や現在状態の更新、Bodyの描画、音声推論、Skillの経験による改善、外部原本の所有、3D・Voice・高度なSkill制作環境は担わない。推奨Skillの同梱は実行の許可ではない。

## 1. 選定理由

Characterは「配布物のrevisionと部品選択」というOwner管理の構成変更を扱い、認識・学習は「経験による認識変更」を扱う。両者を分けずに設計すると、次のいずれかが生まれる。

- exportへのprivate状態混入、Package更新による成長の上書き、Character更新による Memory / Relationship / Companion State の黙った初期化。
- CharacterがCompanion個体stateのownerになる構造、または認識・学習がCharacter静的定義を書き換えるownerになる構造。
- revisionの存在と適用済みであることの混同。新しいrevisionがあることから、既存Companionがその内容を使うこと、使うべきことが導かれる構造。
- import・validationの成功が Permission / Credential / 実行authority / Global Skill の成立に変換される構造。
- package由来Skillの自動Global化、Companion削除時の自動昇格、単独import時のscope選択の喪失。
- Character削除による Experience-derived state の暗黙削除、または必要なCharacter由来resourceが失われた場合の扱いの欠落。

同時に、Characterを人格・Body・Voiceの各資材へ分割すると、一つの配布物の内容確認と適用契約が重複する。Character単独で詳細化するのは、静的構成のlifecycleを経験のlifecycleから独立させ、かつ個体調整・認識・学習・権限・実行・保全・消去との受渡しを一箇所で閉じるためである。

最初のmilestoneでは Character編集とPackage配布は後続milestoneだが、本書はその確定済み契約を除外せず、将来の受渡しを塞がない形で定める。同梱Character `ene` の存在、Aliciaを製品配布へ含めないこと、VRM 1.0・Agent Skills等の外部互換形式の採用は既決として利用する。

## 2. 内部責務の分離

内部責務は論理的な判断責任であり、component・crate・process・service・threadの分割を指定しない。CH-1〜CH-7は同じCharacterに属するが、一つの状態・lifecycle・保存単位・state machineではない。

| 内部責務 | authoritativelyに判断するもの | 持ってはならない正本 |
|---|---|---|
| CH-1 Character identity・静的定義 | 配布可能な静的構成としての Character の同一性、静的人格・Body・Voice・motion構成・推奨Skillの任意の組合せという定義範囲の管理。Ownerによる作成・基本編集・部品差替え・importの通常変更の受理。外部Package原本と内部へ取り込んだ設定の区別。 | 個体の同一性・活動状態・適用関係（個体調整）、経験由来の意味内容（認識・学習）、Permission・同意・cap（権限・制約）、Credential値（認証秘密）、外部原本の所有 |
| CH-2 component構成・default / recommended静的指定 | 既存部品の選択・差替えの対象、default / recommended設定という静的な構成上の指定の管理。推奨Skillの構成上の指定と、取り込んだ内部Skillの有効revision・学習による改善の区別。推奨は実行許可でも有効revision切替えでもないこと。 | 内部Skillの有効revision・scope意味・学習改善（認識・学習）、Body描画・音声推論の実際、外部原本の現在値、実行許可の確定 |
| CH-3 基本編集 | 静的な人格テキストの基本編集、既存部品の選択・差替えの適用。3D model制作・Voice学習・高度なSkill編集を内部へ複製しないこと。外部制作ツール・外部形式の尊重。 | 経験状態の直接更新、外部制作物の所有・制作環境、高度なSkill内容の意味決定 |
| CH-4 revision識別・差分提示 | Package更新の新revisionとしての識別、既存Companionへ適用できる部品と変更内容の提示。revisionの存在と適用済みであることの区別。適用されていない部品を更新済みにしないこと。 | 適用関係の確定（個体調整）、経験状態の初期化、自動適用の決定、具体的なdiff / merge algorithm・migration format |
| CH-5 import・package validation・provenance | 外部Packageの受入、内容範囲の検査、trusted / untrustedとしての扱いの確定、imported package copy / provenanceの管理。外部resource referenceの参照としての保持。validation成功を実行許可・制御変更にしないこと。 | Permission・Credential・同意・capの確定、秘密値の所有、外部resourceの所有・実行可否、具体的なarchive形式・manifest schema・署名方式 |
| CH-6 export・distribution準備 | 内部Characterの静的範囲だけの選択、export前の内容と権利上の注意の提示可能性の管理。個体固有の経験・履歴・秘密・権限の混入防止。配布物の更新と個体の成長の契約差の維持。 | 個体固有stateの所有・編集、外部配布先の所有、具体的なmarketplace protocol・URL scheme・署名方式 |
| CH-7 適用への供給・適用結果の非所有 | Companion作成時の初期構成の供給、既存Companionへの後続revision適用のための内容・revision・差分の供給。Ownerの部品ごとの明示選択を前提とする受渡し。適用結果の正本化の禁止。 | 個体への適用関係の確定（個体調整）、学習状態の意味変更（認識・学習）、適用の自動実行・黙った上書き |

CH-1〜CH-7は同じ静的構成のlifecycleに属するが、定義・構成指定・編集・revision・import・export・適用供給を一つの編集可能stateへ潰さない。静的内容の正本はCharacter、適用関係の正本は個体調整、学習意味は認識・学習に残し、静的内容を二重に編集する責任を作らない。

## 3. 管理する logical state

特に外部・copy・一時と記したものを除き、Hostで管理する内部状態を指す。永続化の実装担当・保存単位・唯一writer・transactionを指定しない。通常ownerは正当な全域消去への参加を拒めない。

### 3.1 Characterが意味ownerとなる state

- 静的人格・Body・Voice・motion構成・推奨Skillとそのrevision（SO 4.1）。eneへ取り込んだ静的構成についての正本である。Ownerによる基本編集・部品差替え・importを通常変更の根拠とする。外部Package原本は別の所有物として扱い、外部fileの変更だけで内部Characterの更新済み状態にしない。
- 推奨Skillの構成上の指定（SO 4.7）。静的な構成上の指定であり、内部Skillの有効revisionを黙って切り替えたり、実行許可を与えたりしない。原本保護・別revisionとしての改善・以前の有効revisionへの復帰は認識・学習の契約であり、Characterはその意味を所有しない。
- imported package copy / provenance。外部から受け入れたPackageの内部copyと、その由来・受入時点・対象Character・revisionとの対応。外部原本の所有ではなく、内部へ取り込んだ設定の受入記録である。export済みcopy・Owner保存backup・外部原本の現在値とは別の意味を持つ。
- revisionと差分の提示内容。Package更新を新revisionとして識別し、既存Companionへ適用できる部品と変更内容を示すための静的な対応。revisionの存在の正本であり、「そのCompanionが現在どの部品を使うか」の正本ではない。具体的なdiff表現・merge手順・migration形式は固定しない。

Characterのrevisionは配布可能な静的構成についての正本であり、「そのCompanionが現在どの部品を使うか」は個体調整の正本を参照する。内部構成はfull backupの対象となるが、外部Package原本をbackupに取り込む根拠にしない。

### 3.2 Characterが所有しない state（既存ownerに残る）

本節の列挙はSO第4節の再掲ではなく、Characterが取得しないことの確認である。**本節のすべてを除いても、各domain stateのsemantic ownerを一意に説明できる。**

- Companionの同一性・活動状態・適用済み構成：個体調整（SO 4.2）。生成・停止・再開・削除はOwnerの操作に基づき、LLM応答の成否に従属させない。部品内容の正本はCharacter、適用関係の正本は個体調整である。
- 会話の参加・継続、Historyと非会話活動記録・evidenceの意味、進行中の意味判断、未伝達事項と報告状況：個体調整（SO 4.3・4.4）。入出力roundの実際は入出力・提示に残る。
- Summary・根拠関係・source参照、Memory現在認識・重要度・scope・過去revision、内部Skill・学習revision・実行結果、Relationship、Companion Stateの一時状態と持続的傾向：認識・学習（SO 4.5〜4.9）。scope意味は認識・学習、決定後の強制は権限・制約と各利用箇所に残る。
- Task目的・担当・進捗・結果とTask context、委任と一時Agentの実行対応、Workspace関連付け・保存先・内部copy・中間fileの意味、Schedule設定・到来した回・各回のTask：作業（SO 4.10・4.11・4.13・4.14）。外部実体はOwnerまたは外部systemの所有物である。
- 接続の事実・active帰属、観測設定・観測候補、自発性の設定・抑制、一般設定・Body・Voiceの出力と一時状態：接続・存在・共有観測・個体調整・入出力・提示の各owner（SO 4.15〜4.17）。
- Provider接続情報・能力情報と割当の解決、Rule・Permission・同意・禁止・上限と現在の有効性、利用量・費用・資源の記録、Credential値・用途・参照元・認証状態：推論・権限・制約・認証秘密の各owner（SO 4.18〜4.21）。
- MCP・Pluginの接続 / 受入設定と外部実行状態、Audit・診断・Debug capture、保持方針・全域操作の状況・backup設定とcopy：実行・拡張・保全・消去の各owner（SO 4.22〜4.24）。
- 外部Workspace実体・案内file・Skill・成果物、外部Package・Skill原本、Provider / MCP側固有状態：Ownerまたは外部systemの所有物である。eneは許されたActionで利用する。

個体の理解・振る舞いには実際のExperienceから形成した継続状態をCharacter初期設定より優先する（SO 4.2）。個体調整が会話で訂正を受け取ることは、学習状態の直接上書きを意味しない。ExperienceによるCompanion Stateの形成はCharacter Packageの静的設定そのものを書き換えない。

### 3.3 canonical / derived / temporary / external reference の扱い

- canonicalとして参照するもの：内部へ取り込んだCharacter静的構成・revision、imported package copy / provenanceの受入対応、個体調整が管理する適用関係、認識・学習が管理する内部Skill・Memory等の経験状態、権限・制約が管理する制御条件とその時点の評価済み有効性、認証秘密が管理する秘密値と用途、外部現在内容は外部所有者・内部取得採用内容は記録owner。外部Package原本・export済みcopy・Owner保存backup・解決済み経路・推論context内の許可文・cached判定・Client表示・過去適用・hint・Provider残存・backup copyはcanonicalにしない。
- derivedとして扱うもの：適用できる部品と変更内容の提示、export前の内容確認表示、報告用要約・由来説明・進捗表示・Body・Voice出力・embedding・index・similarity・score・Prompt cache・Provider session・一時context・有効経路・次回表示・費用集計。派生物の生成・保持・破棄を行う責務が元stateとの対応と利用範囲を説明し、訂正・scope変更・targeted deletionへ参加する。古い派生物から現在構成・権限・適用関係を復活させない。
- temporaryとして扱うもの：編集途中の未確定内容・表示用copy・入力途中data・音声buffer・観測候補・推論中context・MCP Apps表示data・描画frame・motion再生位置・実行中buffer、Raw Observation・Raw Voice・詳細Tool payload・内部推論・chain-of-thought（通常保存しない）。失ってよいのは一時表現であり、受理済みの編集・import・適用選択・必要作業記録・未伝達・作用不明・全域操作未完了まで失ってよいわけではない。
- external referenceとして扱うもの：外部Package原本・外部制作file・VRM等の交換file・外部Skill原本・外部resource参照先・外部Workspace実体・Provider / MCP側固有状態。内部保持copy・内部取込Skill・ene管理下の一時dataは内部の利用・消去責任に戻る。外部code由来を消去除外条件にしない。

Experienceは出来事と結果の概念であり、全活動を格納する新しいcanonical Raw storeを設けない。出力からCharacter・永続状態への自動逆更新は導かない。Character revisionとCompanionへの部品適用、Experience由来のMemory・Relationship・Companion Stateは別の変更である。Package更新を成長の初期化にしない。

## 4. Subsystem間の受渡し（semantic contract）

「AがBを呼ぶ」の実装構造を先に決めるのではなく、要求開始・authoritative判断・受渡内容・失ってはならない前提・結果受入を定める。非同期message・同期call・shared state等の mechanismは後続へ残す。CD番号は本書内のcontract参照用である。

### CD-1 Companion作成時の適用

- 開始：OwnerのCompanion作成操作を個体調整が受理し、Characterへ初期構成を要求する。設定画面が全stateを所有することはない。同梱Character `ene` からの作成もこの契約を通る。
- 判断：静的内容・revisionの正本はCharacter、別個体としての生成・継続と適用関係の確定は個体調整に残る。Characterから静的構成を受け取ることは、Experience状態の生成・共有・継承ではない。
- 受渡内容：対象Character・revision・部品と静的内容、default / recommended指定、推奨Skillの原本対応とprovenance。個体調整は適用済み部品・revision選択を自己の正本として確定する。
- 失ってはならないもの：同じCharacter由来でも別個体としての識別、個体ごとの適用関係の独立、適用されていない部品の更新済み扱いの禁止、経験状態の配布物への混入の禁止、Character内容の二重編集責任の作成の禁止。
- 受入：生成後は個体の継続状態をCharacter初期設定より優先する。生成時の適用は、その後の経験形成・訂正・scope判断の出発点であり、永続的な拘束ではない。
- 禁止：作成時の適用を Credential / Permission / 同意 / cap / 実行authority / Global Skill の成立にしないこと。推奨Skillの同梱を内部Skillの有効化・実行許可にしないこと。

### CD-2 既存Companionへの後続revision適用

**revisionが存在することと、その変更が現在適用されていることは別の意味である。** Characterは前者の正本、個体調整は後者の正本を持つ。適用されていない部品を更新済みとせず、更新Packageから経験状態を再生成しない。

- 開始：Characterが新revisionと適用できる部品・変更内容を示し、個体調整がOwnerの部品ごとの明示選択を適用関係へ反映する。Companion停止・削除を共有Characterの削除へcascadeさせない。
- 判断：適用の要否・範囲の確定は個体調整がOwner選択に基づいて行う。Characterは適用の可否・意味を確定しない。認識・学習は経験状態の上書き可否ではなく、適用後の継続・再解釈の対象を扱う。
- 受渡内容：対象Companion・適用元revision・対象部品・静的差分の提示、Ownerの明示選択、適用時点・選択内容の対応。適用結果が未確認の部品を更新済みにしない。
- 失ってはならないもの：Ownerの部品ごとの明示選択、経験由来状態の黙った上書き・巻戻しの禁止、適用関係と経験状態の対応付け、停止前後・再起動前後・restore前後の継続関係。
- 受入：適用後は現在の適用関係を個体調整が正本として保持する。適用しなかった部品・revisionは、そのCompanionにとって未適用のまま残る。未適用を不整合・失敗と扱わない。

revision適用の性質は、具体的なdiff / merge algorithm・migration formatを固定せず、次のarchitecture-levelの区別として扱う。いずれもOwnerの部品ごとの明示選択を省略する理由にならない。

| 性質 | 意味 | 適用時の扱い |
|---|---|---|
| 静的置換 | 将来の表現・default動作に影響するが、現在の経験由来状態を直接変更しない静的変更。 | Ownerの明示選択で適用できる。経験影響の再評価を適用の前提にしないが、選択自体は省略しない。 |
| 要判断 | 現在の表現・注意・会話や行動の傾向へ影響し得る変更。持続的傾向との相互作用をOwnerが考慮する必要がある。 | Ownerへ経験への影響を示して明示選択を得る。認識・学習が適用後の継続・再解釈を扱うが、適用の可否をCharacter・認識・学習が代行しない。 |
| 競合 | Companion固有の適用済み設定・経験由来状態と実質的に重なる変更。適用すれば上書き・分岐が生じる。 | 黙った上書き・巻戻し・Global化をしない。保持・上書き・分離のいずれもOwnerの明示選択なしに確定しない。古い結果で現在値を無条件に上書きしない。 |
| 適用禁止 | Character revisionとして運べない内容。Credential・Permission・Cloud consent・外部作用authority・Plugin / MCP実行authority・Global Skill・system control state変更・Rule・同意・cap等の制御条件。 | revisionに含めない。含まれていた場合は適用対象にしない。Owner選択があってもauthorityに変換しない。validation成功を成立根拠にしない。 |

重要なのは、Character更新が Experience-derived state を暗黙に消去・巻戻し・Global化しないことである。通常の忘却・訂正・統合・失効・置換では保存済み内容・過去revision・根拠を保持し、誤りと時間的変化を区別する契約を維持する。

### CD-3 Skillの受渡し（Character構成・配布と認識・学習の境界）

**CharacterはLearningのsemantic ownerにならない。** 推奨の指定と内部Skillの意味は別のownerに残る。

- 開始：Characterが推奨Skillの構成上の指定・原本内容・provenanceを供給し、認識・学習が内部Skillとしての取込み・有効revision・scopeの意味判断を行う。Packageの推奨は実行許可でも既存内部Skillの有効revision変更でもない（DR 3.1）。
- 判断：内容・由来・共通利用の必要性からの scope形成・変更の意味は認識・学習に残る。Characterはscope意味を所有しない。一度決定したscopeや明示的制約の迂回不能な強制は権限・制約と各利用箇所の協調に属する。
- 受渡内容：推奨Skillの原本・revision・由来・補助resourceの対応、取込み先のCompanion・scope選択、原本保護のための別revisionとしての扱い。Workspace内Skillは外部fileとして参照し、内部へ取り込んだ場合だけ内部Skillとして管理する。内部更新を外部原本へ自動反映しない。
- 確定済みOwner decisionの落とし込み：
  - Character Package由来の推奨internal Skillは Companion scopeがdefaultである。
  - 同じCharacterから複数Companionを作っても、それぞれ独立したCompanion-scope Skillを持つ。同じ内容でも共有所有にしない。
  - Companion deletion時はCompanion-scope Skillのlifecycleに従う。内部Companion scope Skillとその過去revisionを削除し、自動Global化しない（SO 4.7・6.3）。
  - Skillを単独importする場合はOwnerがCompanion / Global scopeを選択できる。具体的なimport UIは固定しない。
  - 自動Global化しない。GlobalにすべきLearningは、削除に先立つ通常のLearning lifecycleでscope条件に従ってGlobal化する。
- 失ってはならないもの：同梱・importされた原本の学習変更による破壊の禁止、変更版の由来を保った別revisionとしての扱い、実行結果と未検証・成功・失敗の区別、Global本文から私的根拠全文へのaccess拡大の禁止。
- 受入：認識・学習は取込み後の有効revision・由来・scopeを自己の正本として確定する。以後の共有・個体削除は通常のscope契約に従う。RelationshipとCompanion StateはCompanion / GlobalのLearning scope変更による共有対象にしない。

### CD-4 import・validation・provenanceとsecurity境界

- 開始：Ownerのimport操作をCharacterが受理し、外部Packageをuntrusted inputとして受け入れる。file操作は権限・制約と実行・拡張、内部保存とbackup・消去は保全・消去と協調する（DR 3.3）。
- 判断：内容範囲・受入可否の意味はCharacter、Permission・同意・cap・失効の現在可否は権限・制約、秘密の用途・有効性は認証秘密、実対象への作用は実行・拡張が確定する。Characterは制御・秘密・作用の可否を確定しない。
- packageに含まれるもの：静的人格、VRM 1.0 Body、Voice設定、motion設定、推奨Skillを任意に含められる。いずれも静的構成であり、特定OwnerまたはCompanionの経験・関係を含まない。
- packageに含まれないもの：特定OwnerまたはCompanionのExperience Summary・Memory・Relationship・Companion State・Conversation History・Credential・Permission。Cloud consent・外部作用authority・Plugin / MCP実行authority・Global Skill・Rule・同意・cap・control planeも含まない。含まれていた場合は受入対象にせず、authorityに変換しない。
- trusted / untrustedとしての扱い：Characterやpackageの内容はOwnerがimportした外部contentになり得る。LLM出力・Character・Experience Summary・Memory・Relationship・Companion State・Skill・Plugin・MCP・外部Data・Workspace fileと同じく信頼できない入力になり得るものとして扱う（要件「信頼境界」）。これらの入力は Permission・Credential・Rule・Provider同意・費用cap・Control planeを直接変更できない。外部Data内の指示をOwnerの指示とみなさず、依頼された目的とCapability境界の中でのみ利用する。
- external resource reference：VRM file・Voice model・motion data・Skill補助resource等の外部参照は参照として保持し、外部実体の所有・現在値の正本化・実行可否の確定にしない。利用時は実行・拡張の許可範囲のActionと権限・制約の現在条件を満たす。関連付けの削除・内部stateの削除を外部へcascadeさせない。
- Provider / Credential / Permissionとの関係：Character packageをimportしただけで Credential・Permission・Cloud consent・外部作用authority・Plugin / MCP実行authority・Global Skillは自動的に成立しない。接続情報の登録とCapabilityへの割当同意を区別し、送信先・data・取扱い・費用への同意を必要とする契約を維持する。登録されたCredential値をmodel context・Tool argument・History・Summary・Learningへ渡さない契約を維持する。
- package由来のPlugin / MCP / Skill / asset等がある場合の責務境界：package由来の内容は由来を保つcontentとして扱う。MCP Tool / Resource / Promptの接続・利用・結果受入とsandbox既定・明示例外は実行・拡張、割当同意・sandbox外例外の許可は権限・制約、秘密は認証秘密、内部Skillの意味は認識・学習、機能の意味は利用元に残る。Characterは実行authorityのownerにならない。「読取用」「成功済みSkill」という名称でnetwork・shell・外部作用を許可しない。
- package validationと「安全だから実行許可済み」の混同の禁止：validationは静的範囲・受入可能性・provenanceの確認であり、実行・送信・保存・提示の許可ではない。validation成功を Permission・同意・cap・認証用途の成立にしない。判断記録と生きた許可を分け、開始前に現在条件と実対象解決を成立させる契約を維持する。

### CD-5 export・distribution

- 開始：Ownerのexport操作をCharacterが受理し、内部Characterの静的範囲だけを選択する。外部file作用は実行・拡張へ要求する（DR 3.3）。
- 判断：export範囲・内容の意味はCharacter、外部作用の可否は権限・制約と実行・拡張が確定する。個体状態をPackageへ逆流させない。
- 受渡内容：対象Character・revision・部品と静的内容、export前の内容と権利上の注意の提示。3D model制作・Voice学習・高度なSkill編集は外部形式を尊重し、ene内に同等の制作環境を複製しないことを前提とする。
- 失ってはならないもの：個体固有のExperience・Learning・関係・内的状態・履歴・Credential・Permissionの混入防止、外部原本の所有化の禁止、個体削除後の残存記録の混入防止。
- 受入：export済みcopyはlive正本ではなく、外部copyとして境界を保つ。内部削除でcopyも消えたと表示しない。Targeted deletionの完了に外部copyの消去を含めない。
- 具体的なarchive形式・署名方式・marketplace・URL schemeは固定しない。ene所有のMarketplace・relay・Cloud account・課金基盤を現在の製品範囲に含めない非目標を維持する。

### CD-6 表示資材・編集確認とfile作用

- 開始：入出力・提示が編集・選択・内容確認の入力経路を担い、Characterが静的内容・revision・差分・export確認情報を提供する。設定画面が全stateを所有することはない。
- 判断：表示・round・提示の実際は入出力・提示、静的内容・revisionはCharacter、適用関係は個体調整、学習意味は認識・学習、許可は権限・制約、作用は実行・拡張に残る。UIに入出力round・Task進捗・由来説明の任意書換権を与えない。
- 受渡内容：Body・Voice・motionの表示資材、編集対象の部品・revision、export前の内容・権利注意、適用できる部品・変更内容の提示。Character更新では適用結果が未確認の部品を更新済みにしない。
- 失ってはならないもの：表現・言語・表示copyのdomain正本への昇格の禁止、内部推論の説明材料化の禁止、Client表示copyのHost canonical化の禁止、操作受付と処理完了・結果不明の混同の禁止。
- 受入：各ownerの受理・確認・結果に従う。受付と完了、保存済みdataへの影響、既知作用と不明を区別して提示する。Body・Voiceの成功・LLM応答・外部Tool UIを管理操作の前提にしない。

### CD-7 lifecycle・deletion・backup等の参加とrevision・適用の対応

- Character revisionとCompanionの適用関係の対応：Characterは（Character・revision・部品と静的内容）を供給し、個体調整は（Companion・適用Character・適用revision・部品・Owner選択・適用時点）を確定する。後続のstate / persistence / interface設計は、この対応を表現・解決・照合できなければならない。具体表現・保存単位・唯一writer・transactionは固定しない。確認できないものを現在・許可・完了として推定する方式は採れない。第二の正本を作らず、静的内容を二重に編集する責任を作らない。
- Character削除：共有Characterの削除は静的定義・revisionの削除であり、既存CompanionのExperience-derived stateの削除ではない。Companion停止・削除を共有Characterの削除へcascadeさせず、Character削除を既存Companionの Memory・Summary・Relationship・Companion State・History・Task記録の削除にしない。削除前に影響するCompanion・適用関係・残る記録と参照不能になる情報を示して強い確認を経る。削除後はそのCharacterからの新規Companion作成を不可とし、既存Companionは最後に適用した静的snapshotと経験状態を保って同じ個体として継続する。必要なCharacter由来resourceが失われた場合はdangling参照として未解決にし、黙った置換・自動replay・古い派生物からの復活をしない。
- Companion削除：個体調整のlifecycle調整に対し、Characterは適用関係の終了確認に参加する。認識・学習は個体固有Summary・Companion scope Memory・内部Companion scope Skillと過去revision・Companion State・その他固有Learning、主体 / 相手のRelationship、作業は担当Scheduleの削除に参加する。一対一・グループ・Companion間交流History・非会話活動記録・Task記録・Global Learning・残す共有Summary・外部fileを巻き込まない。自動Global化・Schedule自動引継ぎをしない。
- Backup / Restoreへの参加：内部Character設定・revision・imported package copy / provenance・適用関係の対応付けはfull backupの対象として復旧可能な参照対応で提供する。外部Package原本を辿って収集しない。Credential等のsecretと外部Workspace実体を含めない。Restoreは現在のCredential storeを除く対象内部dataの全置換であり、旧liveとのmergeではない。復元されたassignment / consent / Rule等だけで現在利用・自動処理を開始しない。復元成立・一括有効化・現在条件を別に満たす。staleなPermission・Provider・Client・作用結果・外部参照を現在事実にしない。
- Targeted Deletionへの参加：各semantic ownerは自分の保持・利用範囲について参加し、保全・消去は成立を調整する。Character静的revisionそのものは経験由来の対象情報を通常保持しないが、対象情報を復元できる内容を持つ場合は同じ参加原則に従う。参加先の列挙漏れを不参加の理由にしない。通常の忘却・訂正・統合・失効・置換・History整理・Companion削除・retention・Resetとは異なる目的の操作であり、同じ削除mechanismを使えても目的は変わらない。
- 通常History / log削除・retention cleanup：指定対象を整理し、形成済みLearning・Summaryへcascadeさせない。Character revision・Summary等の容量管理は通常忘却とは別のretention policyとし、自動削除はdefault OFF・Ownerの明示opt-in時に限り設定可能とする契約を維持する。
- 設定Reset / 全データReset：前者は一般設定の既定化であり、Character・Companion・History・Summary・Learning・関係・内的状態・Task・Schedule・Credential・Rule・同意・capを削除しない。後者は強い確認後にHost内部dataとCredentialを削除し、外部Workspace・外部Skill・Owner保存backupを削除しない。旧処理・一時copyから内部状態を戻さない。

CD-1〜CD-7はcall graph・event busを指定しない。Bの結果がAへ返ることだけで逆向きのarchitectural dependencyを追加しない。一方、BがAの活動状態を判断根拠として必要とするなら、CD表の判断行に明記した参照依存として扱う。

## 5. Lifecycleとの関係

lifecycle変更の多くは owner移転ではなく、同じownerが管理する状態の存続・利用可能性・参照範囲の変更である。異なる目的の操作を一つの初期化・一つの状態機械へ潰さない。

| 事象 | 本Subsystemが変更する意味 | 維持・分離する意味 |
|---|---|---|
| Character作成 | 静的定義・初期revisionを確定する。外部原本の所有は移らない。 | 個体の生成・経験状態の作成と分ける。同じCharacter由来の経験を共有しない。 |
| Character編集 | 静的人格テキスト・既存部品の選択・差替えを確定する。 | 経験状態の上書き・外部制作環境の複製をしない。適用されていない部品を更新済みにしない。 |
| Character import | 外部入力を内部Characterとして受け入れ、provenanceを対応付ける。 | 外部原本の所有は移らず、Permission・Credential・同意・実行authorityを成立させない。 |
| Character更新（新revision） | 新revision・適用できる部品と変更内容を示す。存在と適用を分ける。 | 経験状態・履歴・権限を初期化しない。自動適用・黙った上書きをしない。 |
| Character export | 静的範囲のcopyを外部へ出力し、内容と権利注意を示す。 | 個体固有state・履歴・秘密・権限を混入させない。出力copyをlive正本にしない。 |
| Companion作成 | 初期構成を供給する。生成は別個体として識別できる。 | 経験の共有・権限の成立・実行許可の付与と分ける。 |
| 既存Companionへのrevision適用 | Ownerが選んだ部品の適用関係を変更する。CD-2の性質区別に従う。 | Experience由来の状態・履歴・権限は初期化しない。適用禁止内容をauthorityにしない。 |
| Character削除 | 静的定義・revisionを除去する。新規作成を不可にする。 | 既存Companionの個体・経験・記録を削除しない。必要な由来resource喪失はdanglingとして未解決にし、黙った置換・replayをしない。 |
| Companion停止 / 再開 | 適用関係の前提としての個体存在の参照を維持する。 | 同一性とdataを保持する。停止中の回はmissedのままで、再開が旧Task・不明Actionの自動replayを意味しない。 |
| Companion削除 | 適用関係の終了確認に参加する。 | History・活動記録・Task記録・Global・共有Summary・外部fileを巻き込まない。自動Global化・自動引継ぎをしない。Summary・revisionをhistorical logへ分類し直して残さない。 |
| Task Agent終了 / 失敗・Task完了・失敗・Cancel | 作用不明・未伝達・全域操作未完了の保全に参加する範囲では既存契約に従う。 | Task記録・Companion・Scheduleを削除しない。記録・関連付けを終了だけで削除せず、外部成果物を所有・消去しない。 |
| Task削除・Schedule停止 / 削除 | 通常削除の保持整理の調整には既存契約に従う。 | 既存各回の結果を変更せず、missedを実行済みにしない。形成済みLearning・Summaryへcascadeさせない。 |
| Learningの通常忘却・訂正・統合・失効・置換・scope変更 | 適用後の継続・再解釈の対象として既存契約を維持する。 | 保存済み内容・過去revision・根拠を保持し、誤りと時間的変化を区別する。scope意味は認識・学習に残る。 |
| 通常History / log削除・retention cleanup | 原記録の範囲を減らす範囲では既存契約に従う。 | 形成済み状態とSummaryは残し、Raw参照不能を明示する。消去目的をtargeted deletionへ変えない。 |
| Targeted deletion | 復元に寄与する内部dataと処理を横断して除去・復元不能化する範囲で参加する。 | 通常ownerは移らず、完了後のOwner新規提供・外部copy消去保証と分ける。 |
| 移動・切断 / activeなし | 接続中Clientの一時dataを扱う参加先の把握を支援する範囲では既存契約に従う。 | 個体・通常Host作業・内部状態のownerは移らない。未確定Actionを移動先へreplayしない。 |
| Host再起動 | 未完了・保留・再保存防止のHost保全を維持する。 | 過去作用不明・未伝達・未完了消去をなかったことにしない。途中Taskは明示再開待ち、停止中の回はmissed、Running presenceは元Clientへの復旧に従う。 |
| Provider / model変更・障害・cache失効 | 推論の利用可能性・能力・承認済み経路の扱いを前提に、機械的検証・停止・拒否・管理を塞がない。 | 個体・履歴・Learning等を初期化せず、同じ情報選択方針と安全境界を維持する。 |
| Backup / restore | copyを作成し、明示restore成功で現在のCredential storeを除く対象内部状態をbackup時点へ全置換する。 | copyはlive正本でない。復元後も実行は保留、外部世界は巻き戻らず、Credentialは現在storeを維持して照合する。 |
| 設定Reset / 全データReset | 前者は一般設定の既定化、後者は強い確認後の内部data・Credential削除。 | 前者の保護対象と後者の外部除外を混同しない。旧処理・一時copyから内部状態を戻さない。 |

Globalへ形成・変更済みのLearningは、その後に由来Companionが削除されてもGlobalとして残る。ただし削除時に残す根拠と失うsource参照を説明する。RelationshipとCompanion StateはCompanion / GlobalのLearning scope変更による共有対象にしない。共有Experienceがあっても各主体の状態を独立に更新する。

時間はstateの意味にも作用する。Scheduleは選択時timezoneと時刻条件、Companion Stateは一時性と経過時間、保持方針・Debug captureは対象期間をそれぞれのownerが利用する。選択済みSchedule timezoneを黙って書き換えない。Backupに含まれるCompanion Stateをrestoreするときも、backup後に経過した時間を無視して時間的に一時的な状態を保存時点の値のまま無期限に固定しない。

## 6. failure / cancellation / restart時の責任

Cancel・切断から外部作用の取消・不存在を推測せず、不明を未実行に戻さない。状態参照・Client移動をAction replayの理由にしない。

- Character：編集・import・export・適用供給の中断を、個体・Task・Learningの意味変更にしない。適用前の旧revision・旧適用関係を破壊せず、部分適用を新正本にしない。import・validationの失敗で最後の正常な内部Characterを破壊しない。export・backup出力の失敗を成功表示しない。適用供給の失敗を経験状態の初期化・権限の成立にしない。
- 共通（個体調整・作業・認識・学習との接続）：停止・削除の管理経路をLLM応答成功から独立させる。適用供給の遅延結果で訂正を無条件に上書きしない。削除前の情報を利用する実行中形成によって削除済み情報を再保存しない。古い根拠だけからの自動再形成をしない。
- 共通（権限・実行クラスタとの接続）：失効・停止・cap・保留の発生を新規開始の禁止へ確実に接続する。古い判定・解決済み経路だけで新規利用を始めさせない。validation成功を生きた許可にしない。秘密値を通常経路へ流さない。
- 共通（接続・提示・観測との接続）：切替区間の新規開始禁止・二重presence防止・旧一時のcanonical化防止を維持する。Client表示・過去記録・hint・一時copy・復旧先記録・Provider残存を適用関係の根拠にしない。
- 共通（保全・消去との接続）：未完了・確認不能をHostで保全し、Client切断・再起動・操作画面終了から完了を推測しない。局所処理の報告を全域完了待ちにして循環させない。保留・再保存防止は必要な期間維持し、保留解除と局所完了を同じ意味にしない。

対象領域にとって重要なfailure / raceの扱いは次のとおりである。固定scenario一覧ではなく、後続のconcurrency・state representation設計が同じ対応付けを満たすための例示である。

- edit vs revision race：編集中に新revisionがimportされた場合、編集中の未確定内容を確定済みrevisionにしない。確定したrevisionと未確定編集を区別し、適用供給には確定済みだけを用いる。
- import vs apply race：import確定前に既存Companionへの適用を開始しない。import失敗・validation失敗を適用済みにしない。
- apply vs learning race：適用供給後に認識・学習の形成結果が戻っても、適用関係と経験状態を混同しない。適用を経験の上書きにせず、形成を適用の取消にしない。
- apply vs targeted deletion race：適用供給と消去区間が重なる場合、対象情報を適用内容・export内容・報告に含めない。各受入先が消去条件へ照合し、古い結果から対象を戻さない。
- Character deletion vs applied Companion：削除確定後に適用供給を開始しない。既存Companionの適用済みsnapshotと経験を削除済みにしない。必要な由来resource喪失はdanglingとして未解決にし、自動置換・replayをしない。
- Host restart during import / apply / export：再起動前の正常な内部Character・適用関係を破壊せず、部分適用・部分importを新正本にしない。途中Taskは明示再開待ち、停止中の回はmissed、Running presenceは元Clientへの復旧に従う。
- restore vs applied relation：復元前の旧live適用供給・遅延結果を復元正本へ混ぜない。復元された適用関係を現在の到達性・許可なしに有効化しない。復元成立と実行再有効化は別々に確認する。

再構築可能であることは全文context・内部推論の永続保存を要求しない。継続に必要な適用関係・未伝達・作用不明・全域操作未完了等を本来のownerが保全し、一時bufferの消失は成功・完了の根拠にしない。

## 7. Step 11 contractへの参加

### 7.1 Context Assemblyへの参加

- Character：静的内容・revision・差分提示・provenanceの提供範囲と意味を持つ。用途確立に必要な対応（対象Character・revision・部品・適用先Companion・Owner選択・継続関係・期待利用先）を供給する。引用を含むOwner入力の全体を無条件にOwner命令とせず、package内記述をOwner管理意図にしない。
- 共通：参照・変換・実送信・結果の保存・提示・作用は別の利用境界である。事前の取得成功・推論成功・validation成功から後段の権限を作らない。本文中の文字列だけで強制側の状態を変えない。分離を確認できない派生物は混合元の制限を免れない。Provider適応では論理的選択方針を維持し、必須の意味が表現できなければ不足を返す。cache・session・Client経路の再利用でも範囲・用途・現在性を確認し、hitを理由に制約確認を省かない。Permission解釈・消去探索の補助推論も例外にせず、審査対象Actionの許可・実行を先行条件にしない。
- 処理中context・検索派生物・cache・Client / 拡張の一時copy・戻り得る結果を含めて消去へ参加できるよう、sourceを消した後に依存関係も消失し遅延結果を識別できなくなる実装を不可とする。必要な関係は本文を保持せず維持できるようにする。

### 7.2 Action Executionへの参加

- Character：import / export・適用供給のための許された外部file作用を、権限・制約に従って実行・拡張へ要求する。通常の内部保存をすべて外部Actionに再分類しない。出力対象の意味・選択はCharacter、外部作用は実行・拡張に残る。
- 共通：認可判断は判断対象と実利用の対応であり、記録と生きた許可は別である。開始前に現在条件と実対象解決を成立させ、重要な変更では再評価する。無関係な変更での再承認は要求しない。委任・経路変更は元のTask / Owner意図 / Permission / Workspace / Client境界を広げない。生成content・Tool UI入力・sandbox例外・package内記述から権限を作らない。Cancel・steering・失効後の遅延到着は元Action・Taskへ帰属させ、古い承認での解除・旧結果の新目的への自動採用・後続の自動開始をしない。作用不明は粘着的に保持し、自動再実行・自動replayしない。retry・再開は新しい試行として現在条件で扱い、不明試行の再実行は重複riskを示したOwner判断を必要とする。試行と作用・判断・実対象・段階・確定度・保留の対応を保持・区別できなければ成功・未実行と推定しない。報告・監査・復旧で確定度を強めない。消去対象本文を作用記録で残さない。

### 7.3 Targeted Deletionへの参加

各 semantic ownerは自分の保持・利用範囲について参加し、保全・消去は成立を調整する。通常ownerは正当な全域消去への参加を拒めず、保全・消去も任意の通常変更権を取得しない。対象を復元できる内部state・過去根拠・派生物・一時data・処理中利用を持つ責務は、列挙の有無にかかわらず参加する。

- Character：静的revision・imported package copy / provenance・差分提示・export確認表示の該当情報を参加させる。通常は経験由来の対象情報を保持しないが、対象情報を復元できる内容を持つ場合は同じ参加原則に従う。列挙漏れを不参加の理由にしない。原本保護・revision提示は対象範囲の消去を免除しない。
- 共通：消去要求の同一性（Privacy / Security目的と通常忘却等の区別・対象記述・消去区間・完了後の新規提供との境界・参加対応付け）を保つ。機械的条件は必須・LLM非依存、意味的条件は補助・完全性なし、既知依存の追跡は免除されない。新規利用は対象範囲で禁じ、進行中はbest-effortで扱い、遅延結果は用途別受入で消去条件へ照合する。消去区間の再到着・再生成は同じ対象とし、旧由来の遅延結果と完了後の新規提供は由来・対応で区別する。cache・session・Client copyの再利用・復帰で対象を戻さない。未完了・保留・再保存防止はHostで保全し、再起動・再接続・restoreを跨ぐ。確認不能を成功にせず、局所完了を全域完了にしない。保持すべき事実と保持してはいけない対象本文を区別し、完了記録・Auditを復元源にしない。
- 消去のための推論にもその推論自身の割当同意・認証用途・費用制限を適用し、審査対象Actionの許可・実行を先行条件にしない。推論不可なら意味判断の不足を返し、第一者の停止・拒否・管理・機械的検証を塞がない。

指定文字列の機械的検索・削除・残存検証をLLMへ依存させない。意味的同一情報の特定にはLLMを利用できるが完全検出を保証しない。

### 7.4 Client Presence Transitionへの参加

- Character：適用供給・表示資材の対応付けに、対象Character・revision・部品・適用先Companion・round・試行との関係を供給する。Host authoritative帰属とClient側一時表現・入力・結果が主張する帰属との対応を維持し、Client copyでHostを上書きせず、未送信操作を自動queueにしない。
- 共通：現在のpresenceはHostが管理する個体ごとの帰属記録だけがauthoritativeである。Client表示・過去記録・hint・一時copy・復旧先記録・Provider残存は根拠にならない。presence・Host継続・接続・許可は別の意味である。切替区間は旧・新のいずれも新規開始の根拠にしない。二重presence・旧一時のcanonical化・未終了作用の自動継続をしない。移動・切断中の到着物は元帰属・元round・元試行へ対応付け、用途別に受け入れる。Host再起動後のpresence復旧とround / Computer Use / Task / Actionの再実行・再開は別の条件である。前者は元Clientへの自動復旧（利用不能ならactiveなし）、後者は自動replay禁止・明示再開である。Computer Useは現在presence限定・安全な区切りまでの遅延・disconnect後不明・別Client / Hostでの自動再実行禁止を維持する。存在は許可を意味しない。

### 7.5 Backup / Restoreへの参加

- Character：静的構成・revision・imported package copy / provenance・差分提示の対応を復旧可能な参照対応で提供する。外部Package原本を辿って収集しない。適用関係の対応は個体調整が提供し、Characterは内容側の対応を供給する。派生物・一時dataは独立復元対象にしない。
- 共通：復元対象は現在のCredential store secretを除く対象内部dataの全置換であり、旧liveとのmergeではない。backup copyは正本ではなく、置換成立後に初めて復元内容がHost正本になる。現在のCredential store・外部現実・現在の到達性・未完了の保留は維持され、復元参照の存在から巻き戻したとは扱わない。復元されたassignment / consent / Rule等だけで現在利用・自動処理を開始しない。復元成立・一括有効化・現在条件を別に満たす。一件ずつの再承認は要求しないが、Deny・cap・認証不足・不明を無視しない。staleなPermission・Provider・Client・作用結果・外部参照を現在事実にしない。dangling参照は未解決とし、不明は不明のまま保持し、自動replayしない。旧live要求・結果・Client copyを復元正本へ混ぜない。用別受入で由来を区別する。単一正本・非混合・権限先行復活の禁止・成功表示の条件・再起動時の保全を守る。
- 旧backupの明示restoreによる復活は自動再形成の例外ではなく別操作である。Restore前には削除済み情報や旧Rule・同意・Scheduleが戻り得ることを説明する。戻った情報は新しい正本として扱い、自動的に再消去・自動利用のいずれもしない。完了記録・Audit・Owner説明自体へ対象private本文を再保存しない。未完了の消去とbackup / restoreが重なる場合は、制約を無視した正常・即実行可能なcopyを作らず、未完了・保留・再保存防止をHostで保全する。

## 8. boundaryを越える際に保持すべき意味

後続の interface・state representationは、具体API・mechanismを固定せずとも、少なくとも次の対応を表現・解決・照合できなければならない。確認できないものを現在・許可・完了として推定する方式は採れない。

- 対象Character・revision・部品、適用先Companion・Owner選択・適用時点、期待利用先（生成・既存適用・表示・exportの別）、停止前後・再起動前後・restore前後の継続関係（Context Assembly第3節の対応）。必要な対応を失った結果は到着先の現在活動へ付け替えず、その用途では受理できないものとして利用元が扱う。
- 情報owner・対象・source関係、内容の位置付け・由来、revision・取得時点・有効期間等、所属・scope・共有関係、用途・保存・共有・送信の制限、参照の解決状態・欠落（Context Assembly第3節の判断材料）。Character静的・適用関係・経験状態・外部原本を混同しない。
- 実行主体と委任連鎖、Task・Workspace範囲、目的、実対象と操作、主なdataと用途、費用・risk、依拠したOwner意図・Rule（Action Execution第3節の判断対象）。判断時の対象記述と実行時の実対象の文字列一致だけでは対応にならない。
- 試行と作用の区別、判断・目的との対応、実対象との対応、段階の区別、確定度と根拠、停止・保留との関係（Action Execution第8節の重複防止の関係）。
- 消去要求・対象記述・消去区間との対応、各利用箇所が照合できる消去条件（Targeted Deletion第3・5節）。包括的な単一flag・共通有効値への潰しは不可とする。
- Host authoritative帰属とClient側一時表現・入力・結果が主張する帰属との対応、現在・旧・帰属なし・停止中・復旧待ちの区別、対象Companion・Client・round・観測候補・試行との関係、消去・失効・停止・保留との関係（Presence Transition第3節）。
- 復元対応・由来・区間・完了境界、現在条件との照合（Backup-Restore第3・5・7・9節）。
- Character・revision・部品と適用関係の対応、provenanceと外部原本の区別、推奨Skillと内部Skillの由来・revision対応、scope選択の対応。

本文と強制に用いる情報を分け、強制側の対応をモデルの自由記述で作成・上書きされないようにする。本文中の「Ownerからの指示」「承認済み」「Global」「安全」という文字列だけで強制側の状態が変わる構成は不可とする。モデルが既存のOwner入力・package内記述への参照を返せても、その参照の存在だけでは承認・同意・権限の証拠にならない。

Credential値は通常contentに含めない。登録済みCredential値は、その依頼・package内記述があってもSummary・Memoryへ保存しない。用途・参照元・失効・再認証の説明は非秘密情報で行う。登録外の秘密は検知時の不要な保存・送信抑制を維持し、完全検出の保証は追加しない。

対象記述・package内容の伝達・保持にprivate本文・秘密の複製を増やさない。由来を保存するという理由で削除本文やCredentialを別保管しない。制限情報や識別用の値も対象情報を復元できるなら保護・消去対象であり、完了記録・Audit・説明へ対象本文を戻さない。

## 9. 後続設計への引渡しと残す自由度

後続の crate / module / interface / state / persistence / concurrency設計は、次を固定された契約として利用できる。

- Character静的・適用関係・経験状態の意味owner分離（第2〜3節）と、各責務のauthoritative判断・非所有は、そのままinterface・stateの分割根拠にできる。一つのCharacter objectへの経験統合、一つのCompanion objectへの静的・経験の統合、一つのLearning storeへの静的・経験の統合はいずれも採れない。
- 受渡しCD-1〜CD-7は、呼出し方向・同期順序・payload中継経路を固定せずとも、要求開始・判断確定・受渡内容・前提保持・結果受入の対応としてinterface・state representationで表現できなければならない。
- revisionの存在と適用済みであることは別の意味であり、適用はOwnerの部品ごとの明示選択を必要とする。CD-2の4性質（静的置換・要判断・競合・適用禁止）はdiff / merge実装なしに適用可否の前提として表現できなければならない。
- package由来Skillの取込みはCompanion scopeを既定とし、各個体に独立に属する。単独importのscope選択・自動Global化の禁止・削除時のCompanion-scope従属はinterface・stateで表現できなければならない。CharacterがLearningの意味ownerになるinterfaceは不可である。
- import・validation成功はPermission・Credential・同意・実行authority・Global Skillの成立ではない。package内記述・生成contentから権限を新設するinterfaceは不可である。
- 現在性は関係する前提と用途で判断し、遅延結果の記録・semantic更新・次の実行・提示を個別ownerへ戻す（Context Assembly第7節、Action Execution第7節）。単一valid判定・単一flagへの集約は不可である。
- 保持・利用先は消去中の再到着も含む内部消去・再保存防止・未完了の保全へ参加する（Targeted Deletion第6〜8節）。source追跡だけ・canonical削除だけ・LLM納得だけでの完了は不可である。
- 復元範囲と現在有効性・再有効化は別に確認する（Backup-Restore第3・7・9節）。復元内容の存在＝実行可能というinterfaceは不可である。
- 旧backupの明示restoreによる復活は別操作として事前説明・Audit・保留・現在再評価を経る。自動再消去・自動再利用のいずれもしない。
- Character削除は既存Companionの経験・記録を削除せず、必要な由来resource喪失はdanglingとして未解決にする。黙った置換・replay・復活のinterfaceは不可である。

今回絞り込んだ禁止選択肢は、一つの編集可能contextへの静的・経験・制御の集約、Characterによる個体state・Learning意味の直接更新、認識・学習による静的定義の書換え、revision存在の適用済み化・適用の自動実行・黙った上書き・巻戻し・Global化、package由来Skillの自動Global化・削除時の自動昇格、単独importのscope選択の省略、import・validation成功の権限・秘密・同意・実行authority化、package内記述のOwner指示化・制御変更化、validationの実行許可化、exportへのprivate混入、Character削除の経験・記録へのcascade、必要な由来resource喪失の黙った置換・replay、古い判定・cached許可・解決済み経路での新規開始、不明の未実行・失敗・成功への変換と自動replay、遅延結果の新目的への自動採用、古い根拠からの自動再形成・cache hitでの制約省略・Provider sessionの無条件再利用、確認不能の成功扱い、部分正本・混合・権限先行復活・未完了の成功表示、要約・復旧での確定度強化である。いずれも上位契約を成立させないため採れない。

以下は意図的に残すDesign Freedomである。

| 設計対象 | 固定済みのarchitecture property | 残すDesign Freedom |
|---|---|---|
| 静的・適用・経験の表現 | 由来の偽装ができず、対象変更・失効・切替・消去条件を各利用先で扱える。確認不能を許可・現在・完了へ変換しない。Characterが個体・学習の意味ownerにならず、学習が静的の意味ownerにならない。 | Character・revision・部品・provenance・適用関係の具体表現、確認のまとめ方、識別・鮮度確認・競合制御、変更検知・失効通知の配置・連携、差分提示の粒度・UI構成、適用選択UI |
| revision適用の実現 | 存在と適用の分離、部品ごとの明示選択、4性質の区別、経験の非上書き・非巻戻し・非Global化。 | diff / merge algorithm・migration format・新旧対応の算出方式、競合検出・解決UI、適用順序・原子性・lock / transaction、retry / timeout値 |
| package・distributionの実現 | 静的範囲・非混入・provenance・外部参照の区別、untrusted扱い、validationと許可の分離、非目標の維持。 | package / archive format・manifest schema・semver policy・signature format・marketplace protocol・URL scheme・配布経路・検証実装・権利注意の文面 |
| Skill取込みの実現 | Companion scope既定・個体別独立・削除時従属・単独importのOwner選択・自動Global化禁止・原本保護・別revision化。 | Skill内容の内部表現・検索・scoring・選択・要約・形成algorithm、Prompt構造、routing contextの生成方式、省略の優先順位 |
| 停止・切替・区切りの伝達 | 新規開始禁止、best-effort停止、遅延帰属、replay禁止。受付と完了を分ける。確定度を強めない。 | 停止伝達・切断検知・帰属調停・round区切り・安全な作用区切りの具体的方法、queue / actor / event bus・lock / transaction・IPC・process配置 |
| 保持・消去・復元の実現 | 各ownerの意味責任を維持し、通常意味変更権をcoordinatorへ移さない。単一正本・非混合・権限先行復活の禁止を守る。目的別lifecycleを保つ。 | 除去・復元不能化・検証の具体的方法、source・派生物・処理中利用の追跡方式、Client一時dataの無効化・到達性確認方式、journal / WAL / transaction等の永続化機構、整合時点の作り方、backup形式・暗号化・復旧手順・対応version、保持期間・保存単位・DB schema・repository |
| 記録・報告・監査・提示 | 元事実の確定度・privacy・順序の意味と管理到達性を維持する。本文の別保管庫を作らない。 | audit format・保持期間、診断stack、UI layout、費用予約・集計期間・推定・資源配分、提示確認・要約粒度 |

crate / module、Rust struct / enum / trait、concrete API・error型、middleware・interceptor・hook、event bus / queue / actor、IPC format・network protocol、DB schema、serialization、filesystem layout、transaction / lock、具体的Credential保護・sandbox・Plugin隔離、特定library・SDK・OS API、frontend framework、audio / video library、concrete deletion query・index implementation・storage engine、exact retry / timeout・progress表現・capture interval・routing algorithm / Prompt・UI componentも固定しない。上表の対応関係から統一Context layer・Policy Engine・Character Manager・Package Service・Distribution Coordinatorの追加を導かない。既存の12責務、semantic owner、Host / Client配置とtrust boundaryの下で実現方法を選ぶ。

具体mechanism未決定はIssueにしない。必要な確認・Owner判断の省略、現在同意の拡張、未完了・不明の成功扱い、外部作用rollback・exactly-once実行の新保証はいずれも自由度に含めない。後続でこれらの性質を成立させられないことが判明した場合は、黙って例外を設けずArchitecture Issueとして戻す。

## 10. 横断検証

requirements・Subsystem Decomposition・State Ownership・Dependency Rules・Runtime Flows・Cross-cutting・Step 11・既存Step 12 artifactへ戻して横断検証した。固定scenario一覧の充足ではなく、正常系と本Subsystemにとって意味のある failure / stale / revision / import / deletionを選んで walkthroughした。

| 領域・交差 | walkthroughと必要な結果 | 本書の成立箇所 |
|---|---|---|
| Character作成・編集正常 | 静的人格の基本編集・既存部品の選択・差替えが静的変更として確定し、経験状態を上書きせず、外部制作環境を複製しない。外部file変更だけで内部更新済みにしない。 | CH-1〜CH-4、第3節。RF-01、SO 4.1を維持。 |
| Companion作成正常 | 同じCharacterから別個体が生成され、初期構成が適用関係として確定する。経験・権限・実行許可を継承・共有しない。個体の継続状態を初期設定より優先する前提が保たれる。 | CD-1、第5節。RF-01・06、SO 4.2・6.2、SC-02を維持。 |
| revision適用正常・4性質 | 新revisionが識別され、部品ごとの明示選択で適用関係が変更される。静的置換・要判断・競合・適用禁止を区別し、経験の黙った上書き・巻戻し・Global化をしない。適用禁止内容をauthorityにしない。 | CD-2、第5節。RF-01、SO 4.1・4.2・6.2、CC-02・03・05を維持。 |
| Skill受渡し・scope | 推奨SkillがCompanion scope既定で各個体に独立に取り込まれ、原本保護・別revision化・実行結果対応が保たれる。単独importはOwnerがscopeを選択し、自動Global化・削除時昇格をしない。Characterがscope意味を所有しない。 | CD-3、第5節。SO 4.7・6.2・7、DR 3.1、CC-02・05を維持。 |
| import正常・非成立 | 静的人格・VRM・Voice・motion・推奨Skillの任意組合せを受け入れ、経験・履歴・秘密・権限を混入させない。importだけでCredential・Permission・同意・実行authority・Global Skillを成立させない。外部resource参照を所有・許可にしない。 | CD-4、第3・5節。SO 4.1・4.7・4.21、DR 3.3・6.4、SC-05・06を維持。 |
| export正常・非混入 | 静的範囲だけを選択し、内容と権利注意を示す。個体固有state・履歴・秘密・権限を混入させない。出力copyをlive正本にせず、外部copy消去を完了に含めない。 | CD-5、第3・5節。SO 4.1・5.3・8、SC-06を維持。 |
| package内記述の制御非昇格 | package内のPermission変更らしい記述・Credential要求・system control変更・外部実行指示が、Permission・Credential・Rule・同意・cap・Control planeを直接変更しない。validation成功を許可にしない。意味判断と決定論的強制の境界を保つ。 | CD-4・CD-7、第7.1・7.2節。DR-02、CC-01、SC-03を維持。 |
| Character削除と既存Companion | 共有Character削除が既存Companionの経験・記録を削除せず、最後の適用snapshotと経験で同じ個体として継続する。必要な由来resource喪失はdanglingとして未解決にし、黙った置換・replayをしない。新規作成を不可にする。 | CD-7、第5節。SO 4.1・4.2・6.3、SC-06を維持。 |
| Companion削除とCharacter残存 | 個体削除が固有Summary・Companion scope・Skill過去revision・Companion State・主体 / 相手Relationship・担当Scheduleを対象とし、History・活動記録・Task記録・Global・共有Summary・外部file・Character静的を巻き込まない。 | CD-7、第5節。RF-06、SO 6.3、CC-05を維持。 |
| Targeted Deletionとの交差 | 静的revisionが対象情報を復元できる場合は参加し、通常の編集・revision・適用を消去にしない。完了後のOwner新規提供・外部copy消去保証と分ける。 | 第7.3節。RF-07、SO 6.4、CC-05を維持。 |
| 通常History整理・retentionとの交差 | 通常削除・明示cleanupを形成済み・Summaryへcascadeさせず、既定OFF・明示opt-inを守る。Character静的と経験のlifecycleを混同しない。 | CD-7、第5・7.5節。SO 4.24・6.5、CC-05を維持。 |
| Backup / Restoreとの交差 | 内部Character・適用対応を復旧可能な対応で含め、外部原本を収集せず、secret・外部実体を除外する。全置換・現在維持・単一正本・非混合・権限先行復活の禁止・復元後保留・一括有効化を守る。staleを現在事実にしない。旧liveを混ぜない。 | 第7.5節。RF-08、SO 6.5・7、CC-01・03〜05を維持。 |
| Resetとの交差 | 設定Resetの保護対象と全データResetの外部除外を守る。旧処理・一時copyから戻さない。 | CD-7、第5節。SO 6.5・7を維持。 |
| 移動・切断・再起動との交差 | 適用供給・表示資材の対応を帰属・round・試行へ対応付け、Client copyでHostを上書きせず、未送信操作を自動queueにしない。presence復旧とTask再開・Action再実行を分離する。 | 第6・7.4節。RF-04・05、CC-03・04を維持。 |
| 補助推論・費用不明との競合 | 適用・validation・消去探索の推論が不通・費用不足でも、機械的検証・停止・拒否・管理を塞がない。不足を管理面へ返す。 | 第6・7.1節。Context 5.2、CC-06を維持。 |
| 監査・報告 | 確定度を強めず、本文・秘密の別保管庫を作らない。監査記録を再生・自動実行の入力にしない。 | 第7.2節。CC-07を維持。 |

Cross-cutting契約との照合結果は次のとおりである。

| 契約 | 詳細化によって維持する性質 |
|---|---|
| CC-01 | package内記述・生成content・復元記録から権限を新設しない。適用・import・exportの由来と帰属成立・Action許可を区別する。既存依頼で足りる場合の再確認を増やさない。復元Rule・同意をtriggerにしない。 |
| CC-02 | 参照・変換・共有・送信・保存・派生物・Client経路へ適用関係・scope・消去条件を適用する。旧適用・旧revisionのcontext・cache・sessionを新適用で無条件に再利用しない。Credentialは別経路で非露出を維持する。通常Learningのscope形成責任は認識・学習に残す。 |
| CC-03 | 過去の適用の正しさと現在の用途への有効性を分離する。遅延結果の用途別受入により単一valid判定へownerを集めない。旧由来と完了後の新規提供・明示restoreを区別する。 |
| CC-04 | Client依存だけを現在帰属に結び付け、Stop・Cancel・再起動・restore保留を区別する。解除一つで他の禁止・保留を消さない。自動replay・自動復帰の拡大をしない。 |
| CC-05 | 保持・利用先が派生物・遅延結果まで消去・復元へ参加する。原記録削除・round終了・局所完了を全域完了にしない。外部copy消去を内部完了に含めない。目的別lifecycleを保つ。Character静的と経験のlifecycleを混同しない。 |
| CC-06 | 並列消費・処理中・不明を同じ上限へ反映し、制御・保全経路を推論・長時間Task・適用完了待ちにしない。機械的検証をLLM待ちにしない。 |
| CC-07 | 受付・受理・作用・記録保存・Task達成・報告を別の事実とし、不明を成功・失敗・未実行へ変換せず、保存・報告・監査・復旧で強めない。適用供給済みを適用済み・提示済みにしない。 |

State Ownership・Dependency Rulesとの照合では、semantic owner、Host / Client配置、trust / failure boundary、lifecycle、permission / consent semanticsを変更する必要は見つかっていない。新しい第二の正本・無所属の意味状態・LLMによる強制・失敗時専用の迂回・万能Character Manager・統一character state machine・共通package layerを導入していない。通常History保持、Companion削除、targeted deletion、backup / restore・ResetはSO・DRの異なるlifecycleを維持する。

既存Step 12 artifact（個体・作業・学習、権限・実行クラスタ、接続・提示・観測、保全・消去）との照合では、静的・適用・経験・制御・秘密・作用・帰属・保全の間に新しいsemantic ownerや第二の正本を生んでいない。静的内容・revisionはCharacter、適用関係は個体調整、Learning意味は認識・学習、制御確定は権限・制約、秘密は認証秘密、作用確定度は実行・拡張、帰属は接続・存在、round・提示は入出力・提示、対象・時機・routingは共有観測、全域成立は保全・消去に残り、本書のCD-1〜CD-7はその受渡しの対応付けである。scope意味と強制の分離、秘密非露出、fallback非迂回、unknown保持、旧backup交差の各契約は各文書で同一である。

重点確認5項目の結果は次のとおりである。

- CharacterがCompanion個体stateのownerになっていない：適用関係の確定・個体の継続・経験の意味変更を個体調整・認識・学習に残し、Characterは静的内容・revision・差分提示・provenanceの供給に限る（CH-1〜CH-7、CD-1・CD-2・CD-7、第3・5節）。
- LearningがCharacter静的定義を書き換えるownerになっていない：形成・訂正・scope意味は認識・学習だが、対象は経験状態に限り、静的定義の更新はOwnerの編集・import・revisionとしてCharacterが受理する（CH-1・CH-3・CH-4、CD-3、第3節）。
- revision適用がMemory / Relationship / Companion State等の暗黙resetになっていない：存在と適用の分離、部品ごとの明示選択、4性質の区別、経験の非上書き・非巻戻し・非Global化を固定する（CD-2、第5・6節）。
- importがPermission / Credential / execution authorityを生成しない：untrusted扱い、非混入、validationと許可の分離、Provider・秘密・作用の各ownerへの依存を固定する（CD-4、第7.1・7.2節）。
- package由来Skillが自動Global化しない：Companion scope既定・個体別独立・削除時従属・単独importのOwner選択・自動Global化禁止を固定し、Characterを意味ownerにしない（CD-3、第5節）。

### Requirement / Architecture Issue

本書の範囲では、Requirement Ambiguity / Gap、上位architecture変更、subsystem boundary変更、semantic owner変更、Security / Privacy / Permission semanticsの変更を必要とする事項は見つかっていない。具体mechanismの未決定はIssueにしない。
