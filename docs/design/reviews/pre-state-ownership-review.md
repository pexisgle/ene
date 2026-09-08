# Step 4 前独立レビュー — State Ownership 前監査

状態: 独立レビュー
最終確認: 2026-09-06
対象:

- docs/requirements/ （README, product, requirements, acceptance, references）
- docs/design/architecture-drivers.md
- docs/design/system-context.md
- docs/design/runtime-topology.md
- docs/design/subsystems.md

参照（非正本）: docs/design/reviews/architecture-drivers-review.md

本書は Step 4: State Ownership へ進む前の独立監査である。新しい architecture を設計しない。State Ownership そのものを決定しない。既存実装・crate・DB・過去 architecture を正当化根拠にしていない。参考製品・一般的 Agent 構成は比較材料にとどめ、要件変更の理由にしていない。

## 1. Verdict

**PASS WITH CHANGES**

architecture の基本構造は妥当である。Drivers・System Context・Runtime Topology・Subsystem 分解は要件と大筋で整合し、相互矛盾や過剰な分割は確認されなかった。Subsystem 境界そのものの作り直しは不要である。

一方、Step 4 で誤読されると ownership が不安定になる文言・列挙不足が Minor として残る。いずれも境界の変更ではなく、Step 3 文書への小規模な明確化で足りる。State Ownership Risk は Step 4 で扱うことが正しいものに限定し、Subsystem Defect へ格上げしていない。新規の Requirement Ambiguity / Gap は検出しなかった。

件数: Critical 0 / Major 0 / Minor 15（内訳: Architecture Defect 6、State Ownership Risk 7、Design Freedom 2）。

## 2. Executive Assessment

### 要件との整合

要件 Baseline（製品定義の概念・対象・非目標、要件の行動・安全・データ契約、受け入れ条件の milestone 検証制約、参考資料の非規範扱い）は Drivers AD-01〜15 へ過不足なく反映されている。特に Host 正本・Client 一時性、active Client 排他と通常作業の独立継続、Companion 中心と Task 原則委任、Character と経験個体の分離・限定共有、認識・圧縮根拠・原履歴の役割分離、LLM 意味判断と機械的境界の両立、目的別保持・消去、Task・Workspace・成果物所有、停止・再開・定期実行、Capability 別推論と同意、外部標準と限定拡張、Client 共有観測と個体自発性、身体・Realtime と縮退・操作可能性、秘密非露出・Raw 最小・監査、保全・復元と再有効化の分離は、いずれも Driver・SC・RT・Subsystem で追跡可能である。過去レビューの F-01〜F-05 / N-01・N-02 に相当する論点（単独 Task 記録、待機のみの LLM polling 禁止、Plugin 例示、観測・自発性の適用単位、非 active Client の会話）は現 Baseline と現 architecture で解消されており、再オープンの根拠は見つからなかった。

### System Context / Runtime Topology / Subsystem 間の整合

SC-01〜10、RT-01〜10、lifecycle・trust・failure boundary、Subsystem の Runtime relation / mapping は相互に矛盾しない。Host 正本・Client 非正本・外部所有物の非所有、active Client 排他と Host 通常作業の非移送、推論・作用・観測・UI の trust 分離、部分障害下の入口・状態保護は三文書で同じ意味である。配置差（H/C/P/M/X/U）を Subsystem 差へ不必要に変換していない。H/C を module・process へ一対一にしないこと、M の sandbox 内・外を二 component にしないこと、P を三層 runtime にしないこと、X を両側重複起動にしないこと、U を常駐 service にしないことは明記され、Subsystem 側も場所を横断する責任分担として定義されている。逆に runtime・trust・lifecycle 上分けるべき責務（個体 vs 作業 vs 存在 vs 提示、推論 vs 実行、意味判断 vs 権限制約 vs 秘密、保全協調 vs domain 意味）を一つへ潰していない。

### Subsystem 粒度

12 Subsystem はそれぞれ独立した変更理由・lifecycle・trust boundary・failure boundary・semantic responsibility を持つ。統合が疑われる箇所（Task・Task Agent・Schedule、Memory・Skill・Relationship・Companion State・Summary、Body・Voice・Text・管理、推論・実行、権限・秘密・保全）は、いずれも統合理由（共通の作業責任、共通経験に基づく形成判断、同一体験の入出力切替と安全操作、利用契約の違い、Owner 由来制御の共通契約、全域完了の協調）が文書化され、内部で別契約を残す条件が付されている。数だけによる過剰評価はしない。詳細は 4 節のとおり、いずれも維持が妥当であり、merge/split の再検討を要する境界はない。

### Overengineering

単なる adapter / helper / mechanism の Subsystem 昇格、将来拡張だけの boundary、責任の薄い分離は確認されなかった。Character、接続・存在、共有観測、推論、認証秘密はいずれも薄さではなく安全・lifecycle 分離のために必要である。保全・消去、作業、入出力・提示の広さは cross-cutting の協調責任として意図的であり、巨大化の防止条件（唯一 writer・DB 一元所有の否定、目的別 cascade・単一 transaction の否定、単一障害単位化の否定）が付されている。残る課題は境界の是非ではなく、Step 4 が単一所有・単一 lifecycle・単一 transaction と誤読しないための文言明確化である。

### Step 4 への readiness

Step 4 を設計できる責務境界は揃っている。7 節の Subsystem 文書内 State Ownership Questions は適切な問いであり、本レビュー 5 節のリスクと併せて Step 4 への入力にできる。Minor 6 件の文言明確化の後に進めることを推奨するが、いずれも Step 4 作業と並行可能な規模である。Requirement clarification は不要である。
## 3. Findings

本節の Severity は現在の architecture が Step 4 へ進めるかで付けた。State Ownership Risk は Step 4 で扱うことが正しい場合に Minor とし、無理に Major へ上げていない。別案の好みは Finding にしていない。Evidence は要件正本と architecture 文書の対応であり、既存実装は根拠にしていない。

### F-01 — 保全・消去の「扱う」が意味上の所有と協調の区別を弱める

**Severity:** Minor

**Type:** Architecture Defect

**Target**

docs/design/subsystems.md「内部データ保全・消去」Responsibilities / Key collaborations / 5 節 Boundary Decisions・6 節 Cross-cutting。

**Finding**

保全・消去は backup の保存先・schedule・保持数・暗号化・結果、対応 backup からの全置換 restore、対応 upgrade、設定 Reset と全データ Reset の範囲調整、History・log・Audit の通常保持・手動削除・保持期間・保存量、targeted deletion の全域協調、Debug capture 支援を「扱う」とする。一方で「すべての状態の意味決定、唯一の保存 writer、DB・repository の一元所有は決めない」「局所完了と全体完了を区別する」とし、各参加責務との協調を条件にしている。境界の意図は協調責任であり、全状態の所有ではない。しかし「扱う」の繰り返しは、Step 4 が backup・保持・消去・復旧の設定・状態まで保全・消去の単一所有・単一 writer・単一 transaction と誤読する余地を残す。

**Evidence**

要件は backup 対象差（内部状態を含め Credential と外部 Workspace 実体を除外）、restore 全置換と自動処理保留・一括有効化、設定 Reset と全データ Reset の目的差、通常保持と targeted deletion の目的差、Audit 追記順・保持管理、Debug capture の明示有効化・短期失効を、異なる目的の契約として定める。Subsystem はこれらを「目的別の契約は別に保ち、一律 cascade や一つの transaction を導かない」とするが、Responsibilities 本文では目的別の所有差（例: backup schedule と Task Schedule、History 保持と Audit 保持、通常削除と targeted deletion）が一覧化されていない。

**Why it matters**

このまま Step 4 へ進むと、保存・削除・復旧の整合範囲を一つの保存単位・一つの lifecycle・一つの transaction へ寄せ、domain 側の意味上の ownership を奪う構造になりやすい。保全・消去への無制限依存と、各参加先からの完了根拠収集の循環待ちも誘発しやすい。Subsystem 文書 8 節が循環回避を求めているが、前提の文言が弱い。

**Recommended disposition**

Step 3 の architecture を修正すべき（小規模）。保全・消去を coordinator と明記し、各意味責務の範囲・根拠・進行中利用を受けて保存・消去・復旧を協調し完了根拠を返す関係へ寄せる。意味上の所有が domain 側に残ること（会話記録の意味は個体調整、作業記録は作業、学習の意味は認識・学習、秘密は認証秘密、制御条件は権限・制約）を対応表で明示する。境界の分割・統合は不要である。

### F-02 — 作業内の Task・Task Agent・Schedule・Workspace・保存先の内部 lifecycle 列挙が薄い

**Severity:** Minor

**Type:** Architecture Defect

**Target**

docs/design/subsystems.md「作業遂行・実行管理」Responsibilities / 5 節「Task・Task Agent・Schedule を作業内に統合」。

**Finding**

Task・Task Agent・Schedule を作業へ統合する理由（担当・制限・結果を持つ作業の開始と遂行という共通責任、受渡し連鎖の回避）は妥当である。5 節も「Schedule の timezone・missed・CRUD を Task の進捗へ潰さず、一つの Agent 停止を Task 記録削除にしない。内部の実行・状態管理方法は共通化必須ではない」とする。しかし Responsibilities 本文は Task 進行・判断待ち・完了・失敗・Cancel・引継ぎ、委任・並列・進捗・結果、追加指示、Cancel 受付と停止結果の区別、再起動後明示再開・非 replay、Schedule 作成・変更・停止・削除・Run now・timezone・次回・missed・失敗、Task context・Workspace 関連付け・保存先確認・中間 file 整理を一括列挙し、内部で別 lifecycle として維持すべき対応（Task インスタンス vs Task Agent 試行 vs Schedule 設定 vs 各回、Workspace 関連付け vs 外部実体、永続成果物 vs 一時中間 file、保存先未定時の確認）が Step 4 への制約として一覧化されていない。

**Evidence**

要件は Task 追跡状態、原則委任と軽微処理の例外、Task と Task Agent の区別、委任元境界、追加指示・Cancel best-effort・成功不明時非再実行、再起動後明示再開、各回新 Task・実行時再評価・判断待ち・missed 非補完・担当削除時 Schedule 削除・自動引継ぎなし、Workspace の Task 従属・Task ごと権限独立・関連付け削除と外部非削除、成果物の通常 file 保存・既定作業場所・未定時確認・一時と永続の区別を別契約として定める。Subsystem はこれらを一つの Responsibilities に束ねている。

**Why it matters**

Step 4 が作業を単一 state・単一 lifecycle の所有者と誤読すると、担当削除で残る記録と消える Schedule、実行主体終了後も残る記録、各回の独立追跡、Workspace 関連付けと外部所有、中間 file 整理と永続物非削除の区別が一つの Task 状態へ潰れる。Task 化閾値・分類が固定されていないことと混ざり、会話・軽微調査まで Task 化する設計も排除できなくなる。

**Recommended disposition**

Step 3 の architecture を修正すべき（小規模）。作業内の別契約リスト（Task 設定・インスタンス・各回、Agent の一時委任・試行、Schedule 設定・各回、Workspace 関連付け、保存先・中間 file）を 5 節の判断に追記し、9 節の「同じ Subsystem 内を一つの型・保存単位・実行 loop・障害単位へまとめる根拠にしない」を作業にも適用することを明示する。Subsystem 分割は不要である。

### F-03 — 認識・学習 5 概念の区別は保持されているが単一 store 誤読の余地が残る

**Severity:** Minor

**Type:** Architecture Defect

**Target**

docs/design/subsystems.md「経験に基づく認識・学習」Responsibilities / Non-responsibilities / 5 節「Memory・Skill・Relationship・Companion State・Summary を認識・学習に統合」。

**Finding**

5 概念を認識・学習へまとめる理由（共通経験に基づく形成・訂正と根拠説明の相互関係、Summary 複製や scope 判断重複の回避）は妥当であり、「知識と手順、関係と内的状態、現在認識と圧縮根拠、個体 scope と Global、恒久 revision と一時状態は別契約として残す」「共通 schema・同時更新・一律の保持期間は導かない」まで明記されている。境界自体に欠落はない。残る課題は Responsibilities が保存価値・形成・更新・統合・想起を一括の文脈判断として書き、Memory 主要知識 vs Skill 再利用手順・原本保護・検証区別 vs Relationship の主体別 compact・非対称・Memory 優先 vs Companion State の一時と持続・時間経過・再起動初期化禁止と無期限固定禁止 vs Summary の共通圧縮根拠・source 追跡・正本否定・Credential 契約の所有差が、Step 4 で単一 canonical・単一 revision・単一保持へ潰れる余地である。

**Evidence**

製品定義と要件は Memory（主要知識・現在認識・継続更新・revision・根拠・誤訂正と時間変化の区別・派生非正本）、Skill（Agent Skills 互換・原本保護・別 revision・復帰・根拠と検証区別・Workspace 内外部扱い・scope）、Relationship（主体別・非共有・非対称・補助・compact・第二 Memory 化禁止・Memory 優先・数値 editor なし・親密さによる権限不変）、Companion State（一時と持続の区別・無期限固定禁止・不自然初期化禁止・根拠関連・一時全過去値保存不要・Character 書換えなし・権限不変・出力非正本）、Summary（複数状態の共通根拠・Raw 複製禁止・独立正本否定・Privacy・backup・削除・cache 境界・Credential 契約・source 追跡・共有時部分削除）を別契約として定める。

**Why it matters**

Step 4 が認識・学習を 5 概念の単一 owner・単一保存・単一 lifecycle とすると、通常更新の revision 保持と targeted deletion の優先、共有根拠の部分削除、Skill 原本と revision と外部原本、一時状態の経過時間扱い、Relationship の相手削除時の両側削除、重要度と scope の分離が壊れる。意味判断と強制境界の混同や全域消去の再形成防止にも波及する。

**Recommended disposition**

Step 3 の architecture を修正すべき（小規模）。5 節の「残す区別」を Responsibilities に対応付ける表または箇条で再掲し、Step 4 で別状態として扱うことを明示する。概念ごとの Subsystem 分割は不要であり、むしろ Summary 複製・判断重複を招くため維持すべきである。

### F-04 — device 許可の正本と適用点が三者に分散し正本の明示がない

**Severity:** Minor

**Type:** Architecture Defect

**Target**

docs/design/subsystems.md「接続・存在」「権限・制約」「実行・拡張」の device 許可記述、および 6 節 Permission 行。

**Finding**

接続・存在は「Remote の pairing、通信保護、device 別の許可・失効の適用を権限・制約と協調」、権限・制約は「device ごとの許可機能を扱う」、実行・拡張は「device 別の許可と通常の Permission を適用し」「Computer Use を active Client へ限定」とする。意図は 6 節のとおり中央の制御契約と各利用箇所での適用の協調である。しかし正本（device 許可の意味・失効範囲の所有）と適用点（接続の帰属調停、実行の作用可否）の区別が本文で明示されず、三者がそれぞれ「扱う・適用する」と読める。

**Evidence**

要件は新規 Client の Host 側確認可能な pairing、Host–Client 通信保護、pairing 済み device・最終接続・許可機能の確認と device ごと失効、Computer Use の active Client 限定と device 別許可・通常 Permission、失効後の新規不開始・進行中 best-effort 停止・別経路迂回禁止を定める。Topology は Host と Client device の trust 境界として pairing・保護・失効を Host 管理下に置く。Subsystem は三者へ分散している。

**Why it matters**

Step 4 が device 許可を接続・存在の帰属状態、権限・制約の制御状態、実行・拡張の作用可否の三重正本にすると、失効伝播と進行中停止、device 失効を無関係な Host Task 全 Cancel にしない条件、pairing 済み＝active＝Action 許可済みではない条件が不安定になる。依存も循環しやすい（接続→権限→実行→接続）。

**Recommended disposition**

Step 3 の architecture を修正すべき（小規模）。device 許可の意味・失効範囲の正本は権限・制約、帰属調停への反映は接続・存在、作用可否への適用は実行・拡張と明記し、6 節の協調行に対応付ける。境界変更は不要であり、Step 4 で所有と参照を分ける入力にする。

### F-05 — scope・保存禁止の解釈と決定後制限の文言が重複する

**Severity:** Minor

**Type:** Architecture Defect

**Target**

docs/design/subsystems.md「認識・学習」「権限・制約」「個体調整」の scope・Rule 解釈記述。

**Finding**

認識・学習は「Companion／Global scope の意味判断を扱い」「非共有意図、利用範囲、権限・制約が扱う保存制限を守る」、権限・制約は「明示的な保存禁止・非共有意図を解釈した後の制限、決定した scope、個体・Task の利用範囲を保存・参照・共有の各責務と協調して適用」、個体調整は「Owner に由来する依頼・Rule 解釈等を権限・制約へ提示」とする。「解釈」「意味判断」「決定」「適用」が三者にまたがり、分散解釈（目的を持つ判断責務）と中央実効制約（権限・制約）の対応が Step 4・Step 5 へ委ねられている。意図自体（LLM 解釈と Prompt のみ依存禁止の両立、学習内容からの自己変更禁止）は正しい。

**Evidence**

要件は共有内容・背景の選択・要約の LLM 委ねと明示的非共有の優先、scope の Companion 既定と Global 限定条件・重要性のみ昇格禁止・不明時 Companion 留保、将来 Rule の解釈表示・保存・Undo と曖昧時確認、LLM 出力・学習状態・Character・Skill 等からの Rule・同意・費用・Credential・Control plane 直接変更禁止を定める。Subsystem は意味判断を利用元、制限を権限・制約へ置くが、「解釈」の所在が重複して読める。

**Why it matters**

このまま Step 5 へ進むと、学習出力・外部 Prompt から制御変更へ直接到達する依存、または解釈者の不一致（共有判断と Rule 判断の食い違い）を許す。Step 4 でも決定 scope・保存禁止の正本（権限・制約の有効条件）と、意味判断の材料（認識・学習等の解釈・由来）の所有が曖昧になり、複数 owner になる。

**Recommended disposition**

Step 3 の architecture を修正すべき（小規模）。「目的を持つ判断責務が自然言語・文脈を解釈し、権限・制約が解釈後の有効条件（保存禁止・scope・Rule・同意・失効）を所有し、各利用箇所が現在の有効条件を適用する」と明記する。依存方向の厳密化は Step 5 に残す。

### F-06 — 入出力・提示の統合に対する内部障害・trust 分離の列挙が薄い

**Severity:** Minor

**Type:** Architecture Defect

**Target**

docs/design/subsystems.md「Owner入出力・体験提示」Responsibilities / Non-responsibilities / Why boundary。

**Finding**

Body・Text・Voice・管理・安全操作・MCP Apps 提示を入出力・提示へ統合する理由（同一体験の入出力切替・状態説明・安全操作・代替経路の一元化、domain 判断との分離）は妥当であり、「同じ Subsystem を一つの障害単位にしない」「Body・Voice・Tool UI の障害が Text や管理へ波及しない内部の分離は必要」と条件付けられている。しかし Responsibilities は overlay・移動・状態表現、VAD・Mute・barge-in・段階切替・話者非認証明示、Setup・段階導入・管理経路、keyboard・Text 代替・日英一致、fullscreen・負荷・障害の通知、MCP Apps 提示を列挙するにとどまり、内部の障害単位・trust 差（Body renderer、Voice 経路、Text・管理、MCP Apps の外部 UI trust、表示 copy と Host 正本）の分離が Step 4 への制約として一覧化されていない。

**Evidence**

要件は Body 失敗時の Text・Task 管理・設定・復旧、Voice の段階切替と失敗理由・keyboard 経路、Mute・Stop・Cancel・承認拒否の描画・音声・LLM 完了待ち禁止、重要音声の Text 代替、MCP Apps の第一者承認・管理権限・恒久 UI 置換への非昇格、Tool UI 終了と server・Task・Action 終了の区別、Client 一時 data と Host 正本の区別、Raw・推論非保存を定める。Subsystem は Non-responsibilities で独自決定の禁止・Control plane 直通の禁止・管理の従属禁止・CoT・Prompt 非表示・数値 meter 禁止を定めるが、内部状態の所有差までは明示しない。

**Why it matters**

Step 4 が入出力・提示を単一表示状態・単一 failure 所有者とすると、管理操作が Body・LLM・Tool UI の成功へ従属し、表示 copy が会話・Task の正本になり、round と timeline が混ざる。F-10 の round・timeline・copy 問題や一時 data 問題へ波及する。

**Recommended disposition**

Step 3 の architecture を修正すべき（小規模）。Why boundary の条件を Responsibilities に対応付ける内部区別リスト（Body 表現 vs Text・管理 vs Voice 経路 vs 外部 UI trust、表示 copy vs Host 状態、受付 vs 完了 vs 不明）を追記する。Body・Voice・Text・管理画面への機械的分割は不要であり、むしろ切替・安全操作の重複を招くため維持すべきである。

### F-07 — 未伝達メモと Task 結果・履歴・通知の正本が曖昧になる risk

**Severity:** Minor

**Type:** State Ownership Risk

**Target**

個体調整・作業・入出力・提示・保全・消去の境界。Subsystem 文書 7 節「未伝達事項と会話・作業記録をどう関連付けるか」に対応。

**Finding**

要件は active Client がない間に伝えられなかった事項をメモし、次に移動した Client でまとめて要約報告することを定める。一方 backup 対象一覧にメモの明示がなく、History・Task 記録・通知生成・Companion 間交流との所有差も定めない。Subsystem は個体調整が Host 内活動継続と次 Client 報告を担い、作業が通常 Host 作業を継続し、入出力が状態を示すとするが、メモが会話状態か Task 結果の copy か通知かを決めていない。これは正しい留保であり、現時点で Subsystem Defect ではない。

**Evidence**

要件 Remote Client： Schedule 起動・継続中許可済み Host Task 等は継続、Body・会話・Voice・Computer Use は行わず、新規観測なし、Companion 間交流・通知生成・内部調査は継続可、提示・伝達は次 Client へ延期、メモして次 Client で要約報告。backup は Companion・Character 設定・History・Summary・Learning・Relationship・State・Task・関連付け・Schedule・Rule・同意・費用・Audit を含むがメモを明示しない。Subsystem 7 節は未伝達事項と元結果・要約報告状況の対応を表示 copy や接続状態を正本にしない条件付きで問う。

**Why it matters**

Step 4 でメモを別正本にすると Task 結果・History との重複・不整合、削除・backup・保持の漏れ、再起動・restore 時の消失・二重報告が起きる。表示 copy や接続状態を伝達完了の正本にすると、未伝達と既達の区別が壊れる。

**Recommended disposition**

Step 4 で明確化すべき。メモを独立正本にせず、元結果（作業・個体等の状態）への参照と要約報告状況の対応として扱い、表示 copy・接続状態を正本にしない。Step 3 境界の変更は不要である。Requirement への戻りは不要である（ state 表現の自由度）。

### F-08 — 共有 Summary・個体固有・Global・非対称 Relationship・Skill 連鎖の所属 risk

**Severity:** Minor

**Type:** State Ownership Risk

**Target**

認識・学習・個体調整・作業・保全・消去の境界。7 節「共有根拠と個体固有状態の所属」「意味状態・根拠・原履歴・派生 data」に対応。

**Finding**

一つの Summary を複数継続状態の共通根拠にできること、Companion scope 既定・Global 限定、Relationship の主体別・非対称・相手削除時両側削除、Skill の import 原本・変更 revision・外部 Workspace・Companion 削除時内部 revision 削除と自動昇格禁止、共有根拠の部分削除、source 範囲追跡と正確引用の Raw 参照、派生非正本が要件で別契約として定められている。Subsystem はこれらを認識・学習内の別契約とし、7 節で所属・参照の問いを立てている。現時点で境界の誤りではなく、Step 4 で所有と参照を分けるべき risk である。

**Evidence**

製品定義・要件の Memory・Skill・Relationship・Companion State・Experience・Summary・Scope・削除・backup 契約、および Subsystem の認識・学習 Responsibilities・Non-responsibilities・5 節統合判断・7 節問い。Character 推奨 Skill と内部 Skill の由来対応付け、Workspace 内 Skill の外部扱い、Global・外部の残存と内部 revision 削除の区別を含む。

**Why it matters**

Step 4 で共有 Summary を個体所有にすると他個体の私的参照・自動共有、Global を個体にすると削除時巻き込み・自動昇格、Relationship を対称共有にすると別個体 feedback 混入、Skill 原本と revision と外部を同一にすると原本破壊・backup 除外漏れ・復旧時上書き、派生を正本にすると訂正理由・過去有効性の喪失が起きる。targeted deletion の部分削除・再形成防止もここに依存する。

**Recommended disposition**

Step 4 で明確化すべき。現在認識・revision・Summary・source 範囲・派生の所属と参照、Companion・Global・共有・非対称・原本・外部の区別を、Subsystem 変更なしに定義する。優先度は最高群である。

### F-09 — 保存条件と実行時有効条件・並列費用集約・失効伝播の risk

**Severity:** Minor

**Type:** State Ownership Risk

**Target**

権限・制約と各利用元（個体調整・作業・推論・実行・拡張・共有観測・接続・存在）・認証秘密の境界。7 節「解釈、許可設定、現在の有効性をどう対応付けるか」に対応。

**Finding**

Owner 依頼の一回承認と将来 Rule 保存・Undo・曖昧時確認、永続 Deny・Always ask・Capability 境界の上書き禁止、Rule・Credential・同意・Relationship 変更の他 Action 暗黙承認禁止、失効後の新規不開始・進行中 best-effort 停止・別経路迂回禁止、委任の権限・費用・再帰・loop・並列制限回避禁止、Provider 割当同意・承認済み fallback・費用報告値・推定・不明の区別・cap 到達時 data 保全停止が要件で定められ、Subsystem は権限・制約の中央契約と各利用点適用の協調とする。保存された条件と実行時に有効な条件、全体・Provider 別 cap の並列集約、device 失効・Companion 停止・cap 到達の伝播と進行中処理の扱いは Step 4・Step 5 へ委ねられ、現時点で欠落ではない。

**Evidence**

要件 Permission・Capability・信頼境界・割当・Fallback・費用・Credential・Schedule・自発性・拡張の各節、AD-06・09・10・14、SC-03〜05・08、RT-03・05〜10、Subsystem 権限・制約 Responsibilities・6 節協調・7〜8 節問い。

**Why it matters**

Step 4 が保存条件を有効条件と同一視すると、復元された旧 Rule・同意・Schedule の即時自動処理接続、失効中・変更中の利用、並列 Task・Agent の全体 cap 超過、費用不明時の継続、別 Tool・Task Agent・Client 経路での Deny 迂回が起きる。中央呼出し列の先取りや、検索派生・Prompt cache を権限根拠にする誤りも誘発する。

**Recommended disposition**

Step 4 で明確化すべき。保存条件と有効条件、報告値・推定・不明、全体と Provider 別の対応、失効・上限・判断待ちの反映を、単一中央呼出し列を先取りせず定義する。厳密な依存方向は Step 5 へ渡す。優先度は最高群である。

### F-10 — 会話 round・timeline・Task steering・表示 copy の risk

**Severity:** Minor

**Type:** State Ownership Risk

**Target**

個体調整・接続・存在・入出力・提示・作業の境界。7 節「接続、device 許可、active 帰属、入出力 round」「未伝達事項」「進捗と作用の確定度」に対応。

**Finding**

Text 入出力も active Client に属し、別 Client 会話・操作は呼出し・移動を経て元残置のまま Text のみ応答を基本モデルにしないこと、移動時 round 安全区切りと両 Client 状態表示・同時二重存在禁止、通常 Host 作業の非移送・Client 依存 Action の区切り・非再実行、Task 中も通常会話可・進捗が会話占有しないこと、一対一 timeline とグループ・Task 管理の空間分離が要件で定められ、Subsystem は個体調整（意味・委任・統合）、接続・存在（排他調停）、入出力・提示（提示・安全操作・代替）、作業（遂行・状態）へ分けている。round の正本、timeline の正本、steering と通常会話の対応、表示 copy の位置は Step 4 へ委ねられ、現時点で欠落ではない。

**Evidence**

要件 Remote Client・一続きの会話・Task・Computer Use・UI 優先順位、AD-02・03、SC-02、RT-01・02、Subsystem 個体・接続・入出力・作業の Responsibilities・Non-responsibilities・関係概要・7 節問い。A-04・G-01・G-02 解決済みを前提とし、非 active からの Text 可否は呼出し・移動を経る基本モデルとして解消済みである。

**Why it matters**

Step 4 が round を timeline と同一視すると複数 Client からの同時 round 直列化・移動時切断・切替調停が壊れ、表示 copy を正本にすると Host 正本・再接続到達・切断時 Host Client 復帰・active なし存続が壊れ、steering を通常会話と同一にすると長い作業・判断待ちが会話を占有し、管理 Cancel・記録確認が Companion 応答成功へ従属する。Client 依存不明と Host 通常継続の混同は重複作用を起こす。

**Recommended disposition**

Step 4 で明確化すべき。round・帰属・timeline・Task 状態・表示 copy の所属と参照、区切り・利用不能・作用不明の対応を、専用 presence service や接続 process の指定なしに定義する。優先度は最高群である。

### F-11 — Task context・内部保持 copy・外部所有・学習昇格の risk

**Severity:** Minor

**Type:** State Ownership Risk

**Target**

作業・実行・拡張・認識・学習・保全・消去の境界。7 節「意味状態・根拠・原履歴・派生」「作業の担当・実行主体・各回」に対応。

**Finding**

Task context は永続 Learning へ自動昇格させず、Workspace は Task 従属の関連付けで外部実体を所有せず、同じ folder の複数 Task は作業状態・承認を共有せず、関連付け削除・Task 削除・Companion 削除・Reset・backup で外部を黙って変更・削除せず、成果物は通常 file・既定場所・未定時確認・一時と永続の区別、作業 content の内部保持 copy には内部 Privacy 契約適用が要件で定められ、Subsystem は作業（context・関連付け・保存先・中間整理）、実行（対象作用・不確実性・sandbox・受入）、認識（昇格禁止・外部参照）、保全（除外・保留・非伝播）へ分け、原履歴の単一正本化を否定している。context・copy・関連付け・外部・学習の所有差は Step 4 へ委ねられ、現時点で欠落ではない。

**Evidence**

要件 Task・Workspace・File・Scope・Skill 保護・Character・backup・Reset・System Context SC-06・Topology RT-03・06・09・Subsystem 作業・実行・認識・保全の Responsibilities・5 節判断・7 節問い。

**Why it matters**

Step 4 が Task context を学習・History と同一にすると自動昇格・重複・scope 漏れ、内部 copy を外部と同一にすると削除・Reset・backup の伝播漏れ・外部消去誤表示・Credential 混入、関連付けを外部所有にすると Task 削除・失効の巻き込み、逆に外部を内部にすると所有権取得・専用 library 化・恒久 container 化の非目標復活が起きる。targeted deletion・backup 除外・復旧保留もここに依存する。

**Recommended disposition**

Step 4 で明確化すべき。Task context・Workspace 関連付け・外部実体・内部 copy・永続 Learning・外部 Skill の所属・参照・削除・復旧範囲を、Task 従属と外部非所有を保って定義する。優先度は高群である。

### F-12 — 一時状態と持続傾向・経過時間の復帰 risk

**Severity:** Minor

**Type:** State Ownership Risk

**Target**

認識・学習・個体調整・保全・消去の境界。7 節「一時的状態と継続的傾向をどう復帰させるか」に対応。

**Finding**

Companion State の一時と持続の区別、一時固定禁止・持続の補強・精密化・弱化・訂正・置換、再起動・Client 切替・Provider 変更の不自然初期化禁止と一時無期限固定禁止、持続の根拠関連・由来確認・一時全過去値保存不要、backup 内一時状態の経過時間無視の無期限固定禁止、Character 書換えなし・権限不変・出力非正本が要件で定められ、Subsystem は認識・学習が時間経過の継続・再解釈を担い、保全が restore 整合・実行保留を担うとする。何を保存し何を再解釈するのは Step 4 へ委ねられ、現時点で欠落ではない。

**Evidence**

製品定義 Companion State、要件 Companion State・Memory・backup・Local data、AD-04・05・09・15、SC-02、RT-08・09、Subsystem 認識・個体・保全の Responsibilities・7 節問い。

**Why it matters**

Step 4 が一時を永続と同一にすると restore・再起動で一時が永久固定され、逆に持続を一時と同一にすると再起動・Provider 変更で不自然初期化・成長喪失が起きる。数値 scale・減衰・更新方式の自由度と混ざり、汎用 editor 化・meter 化の禁止も壊れる。

**Recommended disposition**

Step 4 で明確化すべき。時間経過・保持根拠の関連付け、保存対象と再解釈対象、再起動・restore・Provider 変更の条件を、分類・dimension・scale・減衰の具体化なしに定義する。優先度は中〜高群である。

### F-13 — Credential 参照と秘密値・backup 除外・監査 redaction、全域消去の完了根拠 risk

**Severity:** Minor

**Type:** State Ownership Risk

**Target**

認証秘密・推論・実行・拡張・保全・消去・入出力・提示・権限・制約の境界。7 節「Credential の参照と秘密値」「全域消去の対象と完了根拠」「Backup・restore・Reset」に対応。

**Finding**

登録 Credential の一般分離・model context・生成 Tool argument・通常 result・UI・学習・履歴・Task・log・Debug 非露出、設定・認証 flow 登録・用途・参照元確認・更新・失効、接続実行の必要範囲利用・LLM・通常 result 非露出、backup 除外・全データ Reset 削除・restore 後再認証、未登録秘密の抑制と完全検出非保証・漏えい時失効案内、targeted deletion の内部全域（History・Summary・revision・evidence・Relationship・State 根拠・Skill・source・index・embedding・cache・接続中 Client 一時・ene 管理下拡張・処理中・遅延結果）・機械的検索・残存検証・再保存・再形成防止・未完了表示禁止・外部 copy 非保証・通常保持との区別が要件で定められ、Subsystem は認証秘密の独立、推論・実行・提示・記録の各経路の非露出分担、保全の全域協調、権限の実行制約協調とする。参照と値、除外と dangling、supplier redaction と store、部分削除と検証、遅延・再起動後の未完了は Step 4 へ委ねられ、現時点で欠落ではない。

**Evidence**

要件 Credential・通常保存しない data・Audit・targeted deletion・Local data・Backup・Reset、AD-06・07・10・11・14・15、SC-03・04・07・10、RT-07・08・09、Subsystem 認証・推論・実行・保全・入出力・権限の Responsibilities・Non-responsibilities・6 節協調・7〜8 節問い。

**Why it matters**

Step 4 が参照を値と同一にすると通常経路・Client 永続複製・Audit・Debug・表示への露出、除外を無視すると backup 混入・復旧後 dangling・再認証漏れ、全域消去を局所削除と同一にすると根拠・派生・一時・遅延からの復元・再保存、機械的検証前の完了表示、通常忘却・容量削除との混同、外部 copy 消去誤表示が起きる。各参加先の完了待ち循環も誘発する。

**Recommended disposition**

Step 4 で明確化すべき。用途・参照元・認証状態・更新・失効と backup 除外の対応、通常 result 経路との依存分離、秘密を含まない参照・診断の所属、根拠・派生・一時・処理中・遅延の追跡と再保存防止・残存検証・未完了の整合範囲を、具体 transaction・競合機構の固定なしに定義する。優先度は高群（消去・秘密）である。

### F-14 — Schedule・backup・観測の時刻待ちを汎用化しないことは Design Freedom として妥当

**Severity:** Minor

**Type:** Design Freedom

**Target**

docs/design/subsystems.md 5 節「Schedule をすべての時刻待ちへ一般化しない」。

**Finding**

製品 Schedule（担当 Companion・各回 Task・timezone・missed・CRUD）を観測 Capture 時機（共有観測）、backup schedule・保持数（保全・消去）へ一般化せず、共通時計・待機機構を後続選択に残す判断は妥当である。要件は Schedule 各回新 Task・再評価・missed 非補完・polling 禁止、観測頻度・分散・Pause 優先、backup 保存先・schedule・保持を別契約として定めており、Subsystem は担当・Task 化の有無・scope の違いを保っている。別案（汎用 scheduler Subsystem）も可能だが、要件を満たし後続自由度を残す現案に問題はない。

**Evidence**

要件 Schedule・Observation・Backup、AD-01・09・12・15、Subsystem 5・6 節。

**Why it matters**

汎用化すると backup に未要求の担当・Task 化、観測に Task 契約、Schedule に無関係な待機が混ざる。現状は Step 4 で時刻参照と停止・到来・再開の整合性を定義すればよく、Subsystem 変更は不要である。

**Recommended disposition**

Design Freedom として維持すべき。Step 4 で時刻参照関係のみ定義する。

### F-15 — 原履歴の単一正本化・H/C の module 対応付けをしないことは Design Freedom として妥当

**Severity:** Minor

**Type:** Design Freedom

**Target**

docs/design/subsystems.md 5 節「原履歴を独立した全活動の正本 Subsystem にしない」、9 節 Design Freedom、Runtime mapping。

**Finding**

会話発言・参加者を個体調整、作業進捗・作用を作業等の責任から記録し、Summary 形成・保全・保持管理と記録意味を分け、全 Raw 蓄積を要求しない判断、および H/C・P/M/X/U を Subsystem・module・process へ一対一にしない判断は妥当である。要件は正確引用の Raw 参照、Summary の圧縮根拠・source 追跡、Task 記録の残存・引継ぎ、Host 正本・Client 一時・外部非所有を定めるが、単一 History store・単一 module 対応を要求しない。別案も可能だが現案に問題はない。

**Evidence**

要件 Experience・Memory・Task・Backup・Remote、AD-01・04・05・08、SC-01・06、RT-01・08・09、Subsystem 5・9 節。

**Why it matters**

単一正本化すると原記録と解釈・保存価値と想起の混同、全 Raw 保管の肥大、H/C 対応付けは配置差の Subsystem 化・重複 domain を起こす。現状は Step 4 で現在・revision・Summary・source・派生の所属・参照を定義すればよい。

**Recommended disposition**

Design Freedom として維持すべき。

## 4. Subsystem Boundary Assessment

12 Subsystem すべてに無理に Finding を作っていない。評価は責務・lifecycle・trust・failure・semantic の独立性で行い、数では評価していない。

| Subsystem | 評価 | 理由 |
|---|---|---|
| 個体継続・対話調整 | boundary is justified | Client・Provider session・一時 Agent を越える同一判断・文脈・委任・統合の中心。作業・表示・学習へ分割すると同一文脈再構成になる。停止・削除の強制を LLM 成功へ従属させない条件付き。軽微処理の本体実施と Task 原則委任の両立は lifecycle 差として残す（F-02・F-07・F-10 へ）。 |
| Character構成・配布 | boundary is justified | 配布 revision・部品選択（Owner 管理構成変更）と経験形成・訂正の変更理由が異なる。学習・個体へ吸収すると private 混入・成長上書き、資材別分割すると配布確認・適用契約の重複。推奨 Skill と内部 Skill の由来対応は Step 4 参照（F-08）。 |
| 作業遂行・実行管理 | boundary is justified, internal listing should be strengthened | 追跡作業・一時遂行・各回開始の共通責任。個体・表示・Tool 接続へ吸収すると存続・明示再開が失われ、細分化すると受渡し連鎖になる。内部 lifecycle 列挙のみ弱い（F-02）。Schedule 汎用化・Brief 固定化・外部所有化は否定済みで妥当。 |
| 経験に基づく認識・学習 | boundary is justified, internal contracts must be kept distinct in Step 4 | 共通経験の形成・訂正・由来説明の相互関係。概念別分割は Summary 複製・判断重複、History・応答統合は原記録と解釈の混同。5 概念・scope・原本・一時持続の別契約保持が条件（F-03・F-08・F-12）。 |
| Client接続・存在調停 | boundary is justified | 排他性は入出力・Body・観測・Computer Use を横断する domain 制約。個別機能分散は解釈食い違い、接続一覧と帰属同一視は接続だけでの存在・操作許可、Host/Client 別複製は重複。接続と存在の内部区別が条件。device 正本の明示のみ不足（F-04）。 |
| Owner入出力・体験提示 | boundary is justified, internal separation should be listed | 同一体験の切替・状態説明・安全操作・代替の一元化と domain 正本の分離。domain 結合は障害・UI 変更の波及、機械的 Body・Voice・管理分割は切替・安全の重複。単一障害・単一 trust 化の否定が条件（F-06・F-10）。MCP Apps trust 維持を含む。 |
| 共有観測・関連付け | boundary is justified | Client・全体の制御単位と個体自発性の scope 差、観測同意と操作許可の分離。個体吸収は重複検知・scope 混同、Computer Use 統合は同意漏れ、Capture・検知・routing 分割は同一条件の重複。同意拡張禁止・Raw 非保存が条件。 |
| 推論利用 | boundary is justified | 割当・継承・能力差・fallback・費用・論理 context の利用契約の共通化。利用元分散は切替・送信不一致、全 Tool 統合は推論失敗と作用不明の混同、Provider・用途別分割は名称分割。意味判断の利用元残置・cache 非正本が条件。 |
| Action実行・拡張受入 | boundary is justified | 判断と作用確定の分離、外部 code 参加・不確実性の共通責任。Task 内埋没は別経路抜け、MCP・Plugin・内蔵別は契約重複。機能意味の利用元残置・全機能 MCP 化否定が条件。sandbox 既定・明示例外・Remote 外部管理の区別を含む。 |
| 権限・利用制約 | boundary is justified, wording should separate interpretation vs effective condition | 依頼・自発・Schedule・委任・拡張の共通 Owner 由来制御。経路別独立は迂回・再承認不一致、意味判断一体化は認識・親密さによる権限変化。秘密・消去・実行機構の非集約、許可・同意・上限の内部別義・失効範囲維持が条件（F-04・F-05・F-09）。巨大中央呼出し列の先取りなし。 |
| 認証秘密の保護 | boundary is justified | 秘密の保護・更新・失効・backup 除外と説明・context 利用可否の差。権限・設定統合は区別崩壊、Provider・MCP 別分割は同一契約分散。用途別保護と通常経路非露出の分担が条件（F-13）。認証成功の承認化・一般暗号化・完全検出の否定を含む。 |
| 内部データ保全・消去 | boundary is justified as coordinator, must not become owner | 全域対象・除外・進行中と完了根拠の対応付け。局所分散は漏れ・再保存見落とし、全意味・更新統合は巨大 domain・storage 化。目的別 cascade・単一 transaction の否定、汎用 History・backup・削除基盤化の否定が条件（F-01・F-13）。coordinator 明示のみ不足。 |

merge/split の再検討を要する境界はない。Runtime 配置差の Subsystem 化（H/C 複製、M 二重化、P 三層化、X 重複起動、U 常駐化）は否定済みで妥当である。逆の潰し（個体・作業・存在・提示、推論・実行、判断・制約・秘密、保全協調と意味）も起きていない。

## 5. State Ownership Risks for Step 4

現在の architecture 変更は不要だが、Step 4 で慎重に決定すべき ownership 問題である。State Ownership そのものを完成させない。いずれも「この問題を Step 4 で明確にしないと現在の Subsystem 境界が曖昧になる」形式である。Subsystem 文書 7 節の問いと併せて使うこと。

優先度は Step 4 の失敗影響（ multi-owner・lifecycle 混同・循環・god 化・再形成・露出）で付けた。

### P0 — 必ず Step 4 で閉じる（境界の安定に直結）

1. 共有 Summary と個体固有・Global・非対称 Relationship・Skill 連鎖（F-08）。現在・revision・Summary・source 範囲・派生の所属と参照、Companion・Global・共有・非対称・原本・外部の区別、自動昇格禁止・部分削除・相手削除時両側削除・派生非正本。Subsystem 文書 7 節の共有・意味・根拠問いと対応。
2. 保存条件と実行時有効条件・並列費用集約・失効伝播（F-09）。保存 Rule・同意・scope・保存禁止と有効条件、報告・推定・不明、全体・Provider 別、上限・失効・判断待ち、単一呼出し列の先取り禁止。Step 5 への依存方向入力を含む。
3. 会話 round・timeline・steering・表示 copy（F-10）。round・帰属・timeline・Task 状態・表示 copy、安全区切り・利用不能・作用不明、専用 presence 固定なし。未伝達メモ（F-07）と接続・device・active（F-04、前提）と合わせて閉じる。
4. Credential 参照と秘密値・除外・redaction、全域消去の完了根拠（F-13）。用途・参照・状態・更新・失効と除外、通常経路分離、秘密なし参照・診断所属、根拠・派生・一時・処理中・遅延の追跡・再保存防止・残存検証・未完了、transaction・競合の固定なし。秘密・消去の高影響群。

### P1 — Step 4 前半で閉じる（lifecycle・所有の混同防止）

5. 未伝達メモと元結果・報告状況（F-07）。独立正本化せず参照・報告対応とし、表示 copy・接続を正本にしない。再起動・restore・削除・backup の扱いを含む。
6. Task context・内部 copy・外部・学習昇格（F-11）。context・関連付け・外部実体・内部 copy・学習・外部 Skill の所属・参照・削除・復旧、Task 従属・外部非所有。targeted deletion・backup 除外・保留と連動。
7. 一時と持続・経過時間の復帰（F-12）。保存と再解釈、再起動・restore・Provider 変更、分類・scale・減衰の具体化なし。meter・editor 禁止と連動。

### P2 — Step 4 中に閉じる（診断・設定の second store 化防止）

8. Audit・Debug・Error 説明と正本（6 節協調・F-13 関連）。事実供給（担当）・順序・保持（保全）・説明（入出力）、会話・file 本文・秘密・削除済み private の別保管庫化禁止、手動共有の内容・送信先確認、Debug の対象・短期失効・停止・削除。秘密・消去と連動。
9. 一般設定と domain 設定・Setup 流・Reset 範囲（6 節・F-01 関連）。UI・Body・Voice 一般と同意・Rule・cap・観測・自発・device の散在、Settings Subsystem 非設置の意図、Reset 目的差と所有対応。表示言語による許可・費用・Privacy・失敗の意味不変を含む。
10. 費用・資源・負荷の集約と縮退（6 節・F-09 関連）。上限（権限）、利用量・報告・推定・不明（推論等）、縮退（各機能）、専用資源 Subsystem なし、全体・Provider 別 cap の並列整合、会話・操作・安全の保護、容量理由の学習黙削除禁止。
11. Context・由来・最小 data と派生非正本（6 節・F-08 関連）。用途（個体・作業）、意味・根拠（認識等）、利用範囲（権限）、能力差対応（推論）、独立 Context・検索・cache Subsystem なし、scope 適用・検索・scoring 未定。組立て・algorithm は Step 4 以降。
12. 時刻・待機・抑制と Fullscreen・停止・不在（6 節関連）。日時・timezone 参照、Schedule・backup・観測・自発の各 scope、polling 禁止、Fullscreen・停止・不在の参照・反映、Host 通常の fullscreen 停止禁止・Host 停止時 Client 代行禁止。

Subsystem 文書 7 節の 12 問いは上記へ包含される。7 節を捨てず、本節の優先度で順序付けて使うこと。いずれも「この state をこの Subsystem が所有すべき」の詳細設計ではなく、所有・参照・削除・復元の問いとして扱うこと。

## 6. Requirement Coverage

Architecture Drivers・System Context・Runtime Topology・Subsystem を通じ、重要要件の取りこぼしは確認されなかった。必要なら Finding へ昇格する不足はない。

- 所有と実行・Setup・日常： AD-01・03・10・12、SC-01・09・10、RT-01・03・08、個体・作業・接続・入出力・推論・権限・保全で到達可能。最小 Setup・新規・復元選択・言語・自動起動・同梱 ene・段階 Capability・管理反復不要・隔離理解不要・繰返し確認抑制を含む。
- Companion・Character・停止・削除： AD-02・04・08・09、SC-02・06、RT-02・03・09、個体・Character・作業・認識・保全で到達可能。別個体・継続・経験優先、Package 除外・revision・部品明示・黙上書き禁止、停止・削除・best-effort・Schedule・Relationship・内部 Skill revision・Global・外部残存・引継ぎ・事前説明を含む。
- 会話・UI・Body・Voice： AD-02・03・12・13、SC-02・09・10、RT-01・02・04・10、個体・接続・入出力・共有観測で到達可能。timeline・Voice-Text 継続・active 帰属・呼出し移動、グループ・Task 空間分離、Body overlay・状態・fullscreen・負荷・失敗時 Text・管理、Voice Realtime・段階切替・VAD・Mute・非認証明示・keyboard、CoT・Prompt 非表示・meter 禁止・平易説明を含む。
- Observation・自発・グループ： AD-02・06・10・12、SC-02〜04・10、RT-02・04・05・08、共有観測・個体・接続・入出力・認識・権限・推論・実行で到達可能。ON・OFF・Pause・常時確認・学習利用説明・停止と削除の区別、Client 共有・存在 Client 限定・desktop 全体・頻度・分散・軽量検知・関連 routing・重複回避・メイン判断・逐次確認なし・画面指示の非昇格・個体最終判断・送信割当・同意非拡張・Raw 非保存・Computer Use 区別、個体別自発・OFF・上限・scope 非統合・抑制優先・反復抑制・同 pipeline・Task 原則・軽微例外・Rule 非 trigger、参加者明示・相互応答・順序・無制限禁止・私的非共有を含む。
- Learning・Scope・Skill・忘却・Relationship・State： AD-04〜07・10・12・14、SC-03・07・10、RT-05・08・09、認識・個体・作業・権限・保全で到達可能。LLM 委ね・逐次確認なし・訂正可能性と保存禁止・scope・Credential・Permission・削除・上限の Prompt のみ禁止、Experience・Summary 共通根拠・Raw 否定・正本否定・Credential・追跡、Memory 主要・Skill 手順・検証区別・重複回避・由来・訂正、形成判断・保存・想起の区別・明示要求・禁止優先・無条件追加禁止、現在・revision・根拠・誤訂正と時間変化・派生非正本、Companion 既定・Global 限定・Task 非昇格・外部区別・不明時留保・非共有優先・対話訂正・editor なし、原本保護・revision・復帰・結果区別・Workspace 外部、重要度と scope 分離・通常 lifecycle 非削除・忘却抑制・通常依頼の targeted 非昇格・会話反映・History 削除の形成不変、Relationship 主体別・非共有・非対称・補助・compact・Memory 優先・継続形成・editor なし・根拠説明・演技非上書き・親密さ権限不変、State 個体別・非複製・compact・一時持続区別・固定禁止・初期化禁止・根拠・訂正・Character 非書換・権限不変・出力非正本を含む。
- Task・Computer Use・Workspace・成果物・Schedule： AD-01〜03・08・09、SC-01・02・06・08、RT-02・03・06・09、作業・個体・実行・接続・認識・保全で到達可能。受諾・確認・拒否、追跡・依頼・自発、まとまり Task 化・原則委任・本体非抱込・軽微例外・閾値非固定、Task・Agent 区別・並列・境界内・結果返却、進捗・steering・追加・Cancel best-effort・不明非再実行・再起動明示再開・終了報告、active 限定・移動先行・存在と分離なし・存在は許可なし・区切り遅延・非再実行・切断 best-effort・移動と再実行の分離、Workspace 従属・複数利用可・権限独立・関連削除と外部非削除・Brief 非必須、通常 file・既定場所・未定時確認・一時永続区別・外部非所有、Schedule CRUD・初期 Workspace・各回新 Task・token 化なし・再評価・判断待ち・timezone・missed・Run now・polling なしを含む。
- Permission・Capability・信頼・Provider・Credential・拡張： AD-06・10・11・14、SC-03〜05、RT-03・05〜07・10、権限・実行・推論・認証・個体・作業・観測・接続・入出力で到達可能。機械境界と LLM 判断・再評価・一回承認・Deny・Always ask・境界上書き禁止・Rule 解釈・Undo・曖昧時確認・他承認の暗黙化禁止・失効後不開始・停止・迂回禁止、Filesystem 限定・traversal 拒否・操作分離・Capability 識別・Background 非拡大・委任上限・迂回禁止、LLM 等の不信・直接変更禁止・外部指示の非昇格・複製・拡張・反復・資源の禁止・上限・停止・監査、Host 既定・override・継承・登録と同意の区別・送信・取扱・費用同意・不足表示・差別化禁止・選択方針・安全継続、承認済み fallback・未承認 Cloud 移送禁止・cap・報告・推定・不明区別・保全停止、通常失敗・Local・履歴・cache 正本否定・論理不変・token 表示限定、Credential 分離・登録・context・argument・学習・履歴・結果・log・Debug 非露出・必要範囲・用途確認・更新失効・未登録抑制・完全非保証・失効案内、MCP・Apps・Skills・直接 protocol・限定 Plugin・改変・回避・置換の禁止・sandbox 既定・黙解除禁止・明示許可・保存失効・再確認・包括承認化禁止・内部強制の非表示・利用不能時管理維持を含む。
- 履歴・Privacy・保全・復旧・Remote・品質： AD-01・02・04・05・07・09・13〜15、SC-01・06〜10、RT-01・02・08・09、保全・認識・作業・個体・接続・入出力・権限・認証で到達可能。targeted 例外・既定保持・場所特定不要・全域除去・機械検索検証・LLM 補助限界・共有部分削除・再形成防止・処理中再保存防止・未完了表示禁止・通常保持優先・通常依頼の非昇格・editor 化禁止・手動・保持期間・既定非自動・容量非 cascade・特定補助・事前影響・内部限定・外部 copy 非保証、Raw・推論・payload 非保存・Debug 明示・短期・停止削除・登録 Credential 除外、Audit 追記・本文・秘密・削除済み除外・保持・量・影響・Telemetry 手動、OS account・一律暗号化否定・分離・正常保護、portable・保存先・schedule・保持・結果・対象・Credential・外部除外・暗号化・平文警告・全置換・version・外部不変・削除・旧権限の復帰・再認証・保留・一括有効化・失敗時保護、upgrade 非破壊・互換・backup・失敗理由・Downgrade 非保証、設定・全データ Reset 範囲・外部・backup 除外・事後確認、LAN・VPN・relay 否定・pairing・保護・device 確認失効・一時・永続非 cache・active 排他・滞留・呼出し移動・基本モデル・自発移動・重複禁止・round 区切り・通常継続・非移送・依存区切り・切断 Host 復帰・非再実行・Remote 継続・不在継続・依存可否・Host 完結・延期・メモ要約・自発同一・自動起動なし、Body・言語対象・誤成功表示禁止・資源・Gate・Text 代替・keyboard・Error・Locale 不変を含む。

受け入れ条件の milestone 後続項目（Voice・Observation・グループ・自発交流、Global、Skill 生成改善、State、Relationship、Schedule・Remote・Character、他 Provider・fallback、自動起動・復元）は確定要件として分解対象に含まれ、恒久制約への固定化はされていない。参考資料の Harness 層・製品構造・link 先詳細は根拠化されていない。

新たな Requirement Ambiguity / Gap は検出しなかった。A-01〜A-04・G-01・G-02 は現 Baseline で解決済みであり、過去レビューの N-01・N-02 相当も現要件で解消されている。N-01 相当（非 active Text）は呼出し・移動を経る基本モデル・元残置 Text のみ応答の非基本モデル化で、N-02 相当（適用単位）は Observer の全体・Client 別と自発性の Companion 別・種類別の scope 非統合で、それぞれ Owner 可視の挙動として確定している。いずれも再オープンの根拠はなく、残る未決定は DB・API・表現・依存方向・機構等の設計自由度である。公開時留保（地域・年齢・表示・Marketplace）は要件どおり公開計画時とし、Driver・Subsystem に追加していない。

## 7. Overengineering Assessment

数だけによる評価はしていない。統合すると責務・安全境界が壊れる分離は維持すべきである。

- unnecessary subsystem： なし。Character（静的配布と成長の分離・private 混入・上書き防止）、接続・存在（横断排他と表示・OS 対応の分離）、共有観測（Client 共有と個体判断・同意・記録の分離）、推論（能力・context・fallback と作用・不明の分離）、認証秘密（値と説明・同意・保存の分離）はいずれも adapter・helper・mechanism ではなく独立した変更理由・lifecycle・trust・failure を持つ。将来拡張だけの boundary、薄い責任の分離もない。
- unnecessary abstraction： なし。共通 pipeline・Harness 層・形式名・配置名を Subsystem・module・呼出し列・保存単位へ固定化していない。Provider・MCP・Plugin 名別の分割、Body・Voice・Text・管理の機械的分割、Task・Agent・Schedule の細分化、5 概念の概念別分割、H/C・P/M/X/U の一対一対応はいずれも否定済みで妥当である。
- premature responsibility： なし。独自 Marketplace・relay・account・課金、恒久 Workspace container・成果物 library、汎用 Plugin API・恒久 UI 置換、独自 3D・Voice・Skill 制作環境、全構成 Offline、内部思考・Raw 常時保存、既存互換のいずれも Subsystem として追加していない。backup 暗号化一律必須・一般 App Data 一律暗号化・exactly-once・確実 Cancel・rollback・model 一致も要求化していない。
- duplicate responsibility： 境界の重複はなし。残るは F-01〜F-06 の文言・列挙の弱さであり、所有の重複ではない。device 許可・scope 解釈・表示 copy・round・context・copy・設定の協調は 6 節のとおり判断・制約・作用・保存の対応付けであり、Step 4 で所有と参照を分ければ重複にならない。collaboration の結果、双方が同じ意味上の owner になる関係は確認されなかった。共有観測の routing が個体意味判断・発話・Action を置き換えないこと、Skill・Prompt 読込みが制御獲得にならないこと、MCP Apps 操作が第一者承認にならないこと、保存 Rule・同意の復元が自動開始にならないことは明記済みである。

逆に維持が必要な分離：

- Character と個体・学習の分離（配布と成長、private 除外・revision・部品明示）。
- 個体調整と作業の分離（会話・判断・委任・統合と追跡遂行・判断待ちの非占有、待機 LLM polling 禁止）。
- 存在調停と提示の分離と Host/Client 別非分割（排他 domain と OS・言語・縮退）。
- 共有観測と自発・Computer Use の分離（Client 単位共有と個体単位判断・Task 別 Permission）。
- 推論と実行・拡張の分離と adapter 別非分割（利用契約と作用・不明）。
- 意味判断と権限・制約・認証秘密の分離（解釈と実効・値と説明）。
- 保全・消去の協調独立と全所有の否定（全域完了と domain 意味の分離）。
- Schedule・backup・観測の時刻待ちの非汎用化、原履歴の非単一正本化、H/C 非対応付け。

## 8. Readiness for Step 4

**Minor 修正後に進める。Requirement clarification は不要である。**

- そのまま進める： 不可ではないが、F-01〜F-06 の誤読余地を残すため推奨しない。
- Minor 修正後に進める： 推奨。いずれも Subsystem 文書への小規模追記であり、Step 4 と並行可能である。
- Major 修正後に進める： 不要。境界の作り直しは不要である。
- Requirement clarification： 不要。新たな Gap はなく、過去 Issue の再オープンも不要である。

### Step 4 を別 model（例: Astra）で実施するときに必ず確認させるべき事項

1. 保全・消去は coordinator であり、全状態の owner・唯一 writer・単一 transaction ではない（F-01）。backup・保持・消去・復旧の目的別契約を一律 cascade・単一 lifecycle にしないこと。
2. 作業内の Task・Agent・Schedule・関連付け・保存・中間は別 lifecycle・別契約である（F-02）。担当削除の残記録と消 Schedule、主体終了後の記録、各回独立、関連と外部、中間と永続を潰さないこと。
3. 認識・学習の 5 概念・scope・原本・一時持続は別状態・別保持である（F-03・F-08・F-12）。共通 schema・同時更新・一律保持・単一正本にしないこと。重要度と scope、訂正と時間変化、派生非正本を守ること。
4. device 許可の正本は権限・制約であり、接続・実行は適用・反映である（F-04）。三重正本・循環にしないこと。pairing＝active＝許可ではないこと。
5. 分散解釈と中央実効制約を混ぜないこと（F-05・F-09）。学習・外部 Prompt から制御へ直接到達させず、保存条件と有効条件、報告・推定・不明、全体・Provider 別、失効・上限を区別すること。
6. 入出力・提示を単一状態・単一障害にしないこと（F-06・F-10）。Body・Voice・Text・管理・外部 UI trust、表示 copy と正本、受付・完了・不明を分けること。管理を Body・LLM・Tool UI 成功へ従属させないこと。
7. 未伝達メモ・context・copy・設定を別正本にしないこと（F-07・F-11）。元結果への参照・報告対応、Task 従属・外部非所有、表示・接続の非正本化を守ること。
8. Credential 参照と値・除外・redaction、全域消去の追跡・検証・未完了を守ること（F-13）。通常経路・Client 複製・Audit・Debug への露出、backup 混入・dangling、局所完了の全体化、通常忘却・容量削除との混同、外部 copy 誤表示をしないこと。
9. 5 節 P0 から順に閉じ、Subsystem 文書 7 節の問いを捨てないこと。所有の詳細設計（この state はこの Subsystem）を先取りせず、問い形式で所有・参照・削除・復元を定義すること。
10. 設計自由度（DB・API・表現・依存方向・機構・algorithm・配置・catalog・budget）を Requirement Gap にしないこと。過去解決済み（A-01〜A-04・G-01・G-02、N-01・N-02 相当）を好みで再オープンしないこと。既存実装を根拠にしないこと。新しい architecture を勝手に設計しないこと。

### 最終確認（本レビュー自身の点検）

- 別案の可能性のみを Defect にしていない。F-01〜F-06 は Step 4 誤読の根拠がある文言・列挙不足に限定し、F-14・F-15 は Design Freedom として維持した。
- Step 4 で解決すべき所有・参照・削除・復元の問いを Subsystem Defect と誤認していない。F-07〜F-13 は Risk とし、境界変更を要求していない。
- Requirement Gap と Design Freedom を混同していない。状態表現・依存方向・機構・algorithm・配置・catalog・budget は Gap にせず、Owner 可視の挙動不確定のみを Gap とする基準で新規 Gap なしとした。
- 過去解決済み Issue を根拠なく再オープンしていない。A-01〜A-04・G-01・G-02 および N-01・N-02 相当の現 Baseline での解消を確認し、再オープンは不要とした。
- 既存実装を判断根拠にしていない。要件正本と architecture 文書の対応のみを用いた。
- 本レビュー自身が新しい architecture を設計していない。所有の決定・writer・transaction・schema・API・配置・algorithm の具体化は Step 4 以降に残した。
- 本工程では docs/design/reviews/pre-state-ownership-review.md 以外を変更していない。
