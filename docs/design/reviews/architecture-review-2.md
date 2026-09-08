# Architecture Review #2

状態: 独立レビュー
最終確認: 2026-09-08
Reviewer: architecture作成者とは独立したArchitecture Reviewer

対象:

- 要件正本: `docs/requirements/`（README、product、requirements、acceptance、references。再構成済みBaseline 2026-09-07）
- レビュー対象artifact: `docs/design/architecture-drivers.md`、`system-context.md`、`runtime-topology.md`、`subsystems.md`、`state-ownership.md`、`dependency-rules.md`、`runtime-flows.md`、`cross-cutting.md`

参照（非正本・経緯確認のみ）: `reviews/architecture-drivers-review.md`、`reviews/pre-state-ownership-review.md`、`reviews/architecture-review-1.md`。これらの判定は前提にしていない。

レビュー方法: 要件5文書を先に読み、要件から独立に設計圧力を導出した。次に八つのartifactを、静的な責務・ownership・dependency、時間軸上のruntime behavior、複数Flowをまたぐcross-cutting contractの三面から一つのarchitectureとして読み、局所的には成立していても組み合わせると成立しない箇所を探した。正常系に加え、Remote切断中のComputer Use、targeted deletionと進行中Task Agentの競合、個体削除とGlobal Learningの根拠、削除前backupからのrestore、Host再起動後のRunning個体、activeなし個体のScheduleがClient依存stepへ到達する場合、非active Clientからの承認、Realtime VoiceのClient直結経路、判断用推論中のcap到達、削除後のTask引継ぎ等を各artifactへ戻して辿った。八文書間の重複記述が実際に不整合を生んでいないかも照合した。既存実装、crate構成、Git履歴、参考製品の構造は判断根拠にしていない。

本書はレビュー結果のみを記録する。要件およびarchitecture artifactは変更していない。

## 1. Overall Verdict

- **Verdict: PASS**
- Critical: 0
- Major: 0
- Minor: 3

総括。八つのartifactは、要件の必須挙動を欠落・改変・過剰拘束なく設計上の境界へ変換しており、artifact間に成立を妨げる矛盾はない。Host正本とClient一時data、一個体一active Clientと通常Host作業の独立継続、Companion／Task／Task Agent／Scheduleのlifecycle差、Character／個体／Learningの所属差、LLM意味判断と迂回不能な強制境界の分離、通常忘却とtargeted deletionの目的差、外部作用の不明の保持、Provider・MCP・Workspace・backup copyの非正本化は、Driverからcross-cutting contractまで同じ意味を保っている。Runtime Flowとcross-cutting contractは既決のowner・依存・lifecycleだけで正常系・異常系を記述できており、後から追加した契約はない。

残るMinorは三点であり、いずれも既存の境界を分割・統合せずに閉じる。F-01（Observer routingに用いるCompanion側文脈の利用範囲）はObservationの詳細設計前、F-02（Client側の接続認証材料）はRemote Clientの詳細設計前、F-03（restore時のCredential storeの扱い）はbackup／restoreの詳細設計前に処理すればよく、最初のmilestoneの詳細設計を妨げない。

各artifactが「新たなRequirement Ambiguity／Gapはない」と宣言している点に対し、本レビューはOwnerから観測できる挙動に関わる小さなAmbiguity／Gapを六件見出した（第8節）。いずれもarchitectureの主要部分の再検討を要さない。

## 2. 独立に導出した設計圧力と評価の軸

要件から独立に導いた圧力は次の十点である。architecture側がこれらをどう扱っているかを評価の軸にした。

1. Hostが正本と継続実行を担い、Clientは寿命の異なる入口である。接続の有無を作業や保存の基準にできない。
2. Running個体はactive Clientを一つまで持ち、Text会話・Body・Voice・観測・Computer Useがそこに帰属する。activeなしでも個体は存続し、Client非依存の活動は続く。Stopped個体はpresenceを持たない。
3. Companionは会話・判断・委任・統合の中心で、まとまった作業はTaskとして追跡されTask Agentへ委任される。Task記録は個体・Agentより長く残り得る。
4. Characterは静的な出発点、Companionは経験で変化する個体、Learningは個体scopeを既定とし共有は限定される。
5. Memory・Summary・History・Relationship・Companion Stateは異なる役割の状態であり、互いの第二の正本にならない。過去revisionと根拠は通常lifecycleで消えない。
6. 意味判断はLLMに委ねるが、保存禁止・scope・Permission・Credential・費用・削除の制限をPromptだけに依存させず、生成contentから制御権限へ昇格させない。
7. targeted deletionは保持原則より優先し、保存場所横断・派生物・実行中処理・接続中Clientまで消去を成立させる。通常のHistory削除・忘却・個体削除とは目的が異なる。
8. 外部作用の不明を未実行や失敗へ潰さず、再接続・移動・再起動・restoreでreplayしない。
9. 推論先はCapabilityごとに選ばれ、接続登録と割当同意は別であり、fallbackも承認済み範囲に限る。Observerは個体とは別のconsumerである。
10. Body・Voice・Provider・拡張の障害下でも、Text・管理・停止・復旧・保存済みdataへの経路を維持する。

## 3. Findings

Severityは「このarchitectureを前提として詳細設計へ進んだ場合の手戻りの大きさ」で付けた。順序はレビューが判断した重要度順である。各Findingについて、architecture artifactの欠陥／要件のAmbiguity・Gap／Design Freedom／後続詳細設計事項／問題なし、の区別を明示した。

### F-01 — Observer routingが参照する「利用可能な文脈」の出所と利用範囲が、architecture自身のscope modelの外にある

**Severity:** Minor

**分類:** architecture artifactの欠陥（要件側の連動Ambiguityとして第8節 RA-01）

**Affected artifacts:**
`subsystems.md`「共有観測・関連付け」Responsibilities／Key collaborations、「認識・学習」Key collaborations。`state-ownership.md` §3（Companion／Globalの適用範囲）、4.16、4.18、4.19。`dependency-rules.md` 3.2（共有観測 → 個体調整／認識・学習）、5.3（ambient Observation行と直後の段落）、§9。`runtime-flows.md` RF-03 段階3、§12。`cross-cutting.md` CC-02。

**Finding:**
要件はObserverについて四つの契約を同時に置く。(a) Observer専用のmodel／Provider assignmentで候補検知を行う、(b) 「文脈上関係がありそうなCompanionへ」だけeventを伝え「全Companionへ無条件に配信」しない、(c) 「Companionごとに候補検知を重複実行しない」、(d) 「共有候補検知やeventのroutingを理由に、Companion固有情報の利用範囲やProviderへの送信同意を広げない」。architectureはこの四つをすべて転記しているが、(b)を成立させるためにObserver側が参照するCompanion側の文脈が**何であり、どの利用範囲契約の下でObserver専用Providerへ送られるか**を決めていない。

各artifactの記述は「必要かつ利用可能な文脈」「許された関連付け用文脈」「限定context」で止まる。一方、architecture自身のscope modelは、`state-ownership.md` §3で「Companion／Globalは内部Memory・Skillの利用scopeである。…Summaryの利用範囲、Clientの存在場所をこの二択へ押し込まない」と明記し、4.19のOwner由来制御条件の一覧にもObserver向け提供可否の項目はない。この定義を厳密に適用すると、Companion scopeのMemory・現在会話・Task contextはいずれも「そのCompanion以外の主体」である共有観測にとって利用可能ではなく、Global Learningは複数Companion共通のOwner知識であって「このCompanionに関係があるか」を判定する材料ではない。つまり、既決のscope modelの下では、共有観測が(b)を判定するために参照できるCompanion側文脈は、実質的に何も定義されていない。

**Evidence:**

- [要件](../../requirements/requirements.md)「Observation」: 「文脈との関係を判断して、そのClientに存在するCompanionのうち関係がありそうなものだけへeventを伝える。…全Companionへ無条件に配信せず、Companionごとに候補検知を重複実行しない」「共有候補検知やeventのroutingを理由に、Companion固有情報の利用範囲やProviderへの送信同意を広げない」。「割当と同意」: Observerは「Companion scopeではなく、同じClientのCompanionのProvider overrideを適用・合成して選択しない」。
- `subsystems.md`「共有観測」: 「利用可能な文脈との関係から、そのClient上の関連するRunning Companionだけへroutingする」「関連付けのために全Companionの私的状態へ無制限にaccessしない」。「認識・学習」: 「共有観測には関連付けに必要かつ利用可能な文脈だけを提供する」。
- `dependency-rules.md` 3.2: 「routingに必要かつ利用可能な文脈を参照し、関連個体へ候補の意味判断を求める」。5.3: 「routing用contextは必要な範囲へ限り、同じClientにいる全Companionの私的状態を合併して一つのProviderへ送らない」。§9: 「Observerが全個体のcontextをまとめ、最も安いProviderへ送る」を禁止。
- `state-ownership.md` §3: scopeの二択はMemory・Skillの利用scopeに限る旨。4.16: 「共有候補から全個体の私的情報の利用やProvider同意の拡大を導かない」。
- `runtime-flows.md` RF-03 段階3: 「必要かつ利用可能なrouting用文脈だけを個体調整・認識・学習から参照し、第3節の同意・秘密・費用条件を満たす」。§12は「routing用context選択」を後続設計の自由度として列挙し、`dependency-rules.md` 11.4も「Observer routing context」をLater-design Noteに置く。

**Architectural consequence:**
Observationの詳細設計は、この点を最初に決めなければ着手できない。選択肢は少なくとも二系統に分かれる。(1) routingは画面由来の候補と、Ownerが設定する非Learningの信号（存在個体、Owner設定の関心・役割等）だけで行い、Companionの認識状態は参照しない。(2) 各Companionが自身のProvider・scopeの下で「Observerへ提供してよい関連性情報」を生成し、それをObserver assignmentの同意対象dataとして扱う。(1)は既存契約だけで成立するがrouting精度が下がり、(2)はCompanion固有情報に新しい利用範囲区分を導入するため、`state-ownership.md` 4.16／4.18／4.19と`dependency-rules.md` 3.2／5.3の変更と、要件側の(d)との整合確認を要する。詳細設計がこの判断を暗黙に(2)へ寄せると、要件(d)に対する送信同意の拡張が設計の内側で起こり、Privacy契約の破綻になる。逆に(1)しか許されないなら、Observerの「文脈との関係判断」の実効性は限定され、要件(b)の意図との距離をOwner判断で確認する必要がある。Subsystem境界・owner・Topologyの変更はどちらでも不要である。

**Recommended disposition:**
`state-ownership.md` 4.16／4.18と`dependency-rules.md` 3.2／5.3で、共有観測がrouting判定に参照できる情報の区分を決定する。既存のCompanion／Global scopeに含まれないことを明示し、(1)を採るなら「Companionの認識・会話・Task contextは参照しない」と書く。(2)を採るなら、Observer assignmentの同意説明に「Companion側が提供する関連性情報」がdataとして含まれること、その生成はCompanion側owner（個体調整または認識・学習）の責任で当該Companionの条件下で行われること、Companion scopeの内容そのものは送られないことを固定し、要件側で(d)との関係を確認する（RA-01）。Observationの詳細設計前に閉じる。mechanism（生成頻度、表現、鮮度）は決めない。

### F-02 — Client側が保持せざるを得ない接続認証材料が、Client非正本・Credential非cache契約と未整合のまま定義されていない

**Severity:** Minor

**分類:** architecture artifactの欠陥

**Affected artifacts:**
`runtime-topology.md` Runtime Elements「C: Client上のEne実行」Authoritative state列、Trust Boundaries「HostとClient device」。`state-ownership.md` 4.15（pairing行）、4.21、§8（Host → 第一者Client行、末尾段落）。`dependency-rules.md` 6.3、11.4 Later-design Notes。`runtime-flows.md` §12「Client排他性と安全な区切り」。

**Finding:**
Remote Clientは、再起動をまたいでHostへ再接続するために、少なくともHost到達情報と自身のdevice identityを証明する材料を永続的に保持する必要がある。要件は「新しいClientはOwnerがHost側で確認できるdevice pairingを必要とする」「HostとClientの通信を保護し…deviceごとに失効できる」と定めており、失効可能なdevice別の認証材料が存在することを前提にしている。

一方、architectureはClientについて「Ene内部永続状態の正本を持たない」「長期private状態やCredentialを永続cacheしない」（RT）、「History・Summary・Learning・Relationship・Companion State・Credential等を永続cacheしない」（SO §8）と繰り返し、`state-ownership.md` 4.15でHost側については「秘密値を要する認証材料は認証秘密の境界に置く」と決めている。しかしClient側に残る認証材料については、それが「Eneの永続状態」なのか、「Credential」に該当するのか、どの責務がその保護・失効・全データReset時の扱いを担うのかを、いずれのartifactも決めていない。`dependency-rules.md` 11.4と`runtime-flows.md` §12は「Client bootstrap／認証材料の受渡し」をLater-design Noteおよび自由度に置くが、受渡し方式ではなく、Clientがそれを**保持してよいか**という位置付け自体が未定義である。

**Evidence:**

- [要件](../../requirements/requirements.md)「Remote Client」: 「新しいClientはOwnerがHost側で確認できるdevice pairingを必要とする」「Ownerはpairing済みdevice、最終接続、許可された機能を確認し、deviceごとに失効できる」「Clientは表示と一時的な操作に必要なdataだけを受け取り、…Credential等を永続cacheしない」。「Credential」節のCredentialは「eneへ登録されたCredential」であり、ene自身が発行するdevice認証材料を明示的に扱っていない。
- `runtime-topology.md` C: 「表示・一時操作に必要なdataだけを保持し、長期private状態やCredentialを永続cacheしない」。
- `state-ownership.md` 4.15: 「Host側の確認に基づくpairingの許可…権限・制約。…秘密値を要する認証材料は認証秘密の境界に置く」。§8: 「Client切断時に失われてよいのは未確定の編集・入力、描画・再生buffer、再取得できる表示copy等である」。pairing材料はこの列挙のどちら側にも現れない。
- `dependency-rules.md` 6.3: 「必要最小限の表示・一時操作dataを利用し…永続private cache…を許さない」。11.4: 「Client bootstrap material」をLater-design Note。

**Architectural consequence:**
Remote Clientの詳細設計は、Clientに何かを永続保持させた時点で「Client非正本」「Credential非cache」の文言と衝突する。位置付けを決めずに進めると、(a) 認証材料を「Credential」に分類して認証秘密の契約（一般App Data分離、full backup除外、全データReset削除、失効）を機械的に当てて過剰になる、(b) 逆に「一時data」に分類してHost側で失効しても端末側に残る材料の保護・破棄を誰も引き受けない、のどちらかに滑る。SO 4.15の「秘密値を要する認証材料は認証秘密の境界に置く」がHost側の記述であることも、Client側まで認証秘密が責任を持つと誤読されやすい。境界の変更は不要であり、RTのC列とSO §8への一段落の追記で閉じる。

**Recommended disposition:**
`runtime-topology.md` CのAuthoritative state列と`state-ownership.md` §8に、Clientが保持する接続認証材料（device identity・Host到達情報・失効可能な認証材料）を「Ene内部永続状態でも登録Credentialでもない、Client deviceに属する接続材料」として明示し、Host側の失効（権限・制約が正本）に対する端末側の破棄・無効化責任と、全データReset・device失効・再pairing時の扱いをどの責務が担うか（接続・存在が有力だが本レビューでは決めない）を記す。受渡し方式・鍵の形式・保護方式は引き続き自由度に残す。Remote Clientの詳細設計前に閉じる。

### F-03 — Restoreで現在のCredential storeがどうなるかが未決定であり、SO 4.21の場合分けから欠けている

**Severity:** Minor

**分類:** architecture artifactの欠陥（要件側の連動Ambiguityとして第8節 RA-02）

**Affected artifacts:**
`state-ownership.md` 4.21、6.5（Restore行）。`runtime-flows.md` RF-08 段階3。`cross-cutting.md` CC-05「正常状態の保全と正本切替」。

**Finding:**
要件はrestoreを「ene内部dataをそのbackup時点の状態へ全置換する操作」とし、Credentialはbackupに含めず、「ProviderやMCP等の再認証が必要になり得る」と定める。Credentialは「ene内部データ」であり「一般App Dataから分離して保護する」対象でもある。この三つを合わせると、restore時に現在の（backup後に登録・更新された、あるいはbackup時点にも存在した）Credential storeが、(a) 全置換の対象外として維持され、復元された参照と照合される、(b) 内部dataの全置換に含まれて消去され、Ownerが再登録する、のどちらかであるかが要件から確定しない。「再認証が必要になり得る」は両方の読みと両立する。

`state-ownership.md` 4.21は「Credentialはfull backupから除外し、設定Resetでは維持、全データResetでは削除する。参照がbackupから戻っても値が復元されたとは扱わず、利用前に存在・用途・有効性を照合し、必要な再認証を示す」と、ResetとRestoreの場合分けを列挙しているが、restoreにおける現在storeの扱いだけが欠けている。RF-08 段階3・CC-05も「復元された参照に対し認証の不足・有効性を確認する」と参照側だけを扱い、store側の維持／消去を決めていない。

**Evidence:**

- [要件](../../requirements/requirements.md)「Backupとrestore」: 「Restoreは対応するbackupからene内部dataをそのbackup時点の状態へ全置換する操作とし、…ProviderやMCP等の再認証が必要になり得ることを事前に示す」「BackupにCredential…を含めない」。「Local data」: 「Credentialは一般App Dataから分離して保護する」。「Reset」: 全データResetは「Host内部のene dataとCredentialを削除する」と、Credentialを内部dataと並べて明示している。
- `state-ownership.md` 4.21: 上記引用。6.5 Restore行: 「対応backupで内部を全置換し…認証秘密が認証不足…を扱う」。
- `runtime-flows.md` RF-08 段階3: 「認証秘密と接続ownerは復元された参照に対し認証の不足・有効性を確認する」。

**Architectural consequence:**
Ownerから観測できる挙動（restore後に既存のProvider keyがそのまま使えるか、全部入れ直しか）が設計者の解釈で決まる。(a)を採る場合、restoreは「内部data全置換」の例外領域を一つ持つことになり、その例外が復元された割当同意（旧同意が戻り得る）と現在のCredentialの組合せで即時にCloud送信が成立する経路を作るため、RF-08の保留契約（Task・Schedule・外部接続による自動処理）がOwner起点の会話による推論利用を覆わないことと合わせて説明が必要になる。(b)を採る場合、全データResetとの差が「一般App Dataを復元するか消すか」だけになり、Ownerが復元直後に何もできない状態を生む。どちらも成立するが、保全・消去と認証秘密の協調条件（SO 6.5）と、復元後の有効化の意味（CC-05）が変わる。境界の変更は不要である。

**Recommended disposition:**
要件側で「restoreは現在のCredential storeを維持するか消去するか」を確定し（RA-02）、`state-ownership.md` 4.21の場合分けにrestore行を追加する。維持する場合は、復元された割当同意と現在Credentialの組合せで復元直後に送信が成立し得ることを、RF-08の保留範囲またはOwnerへの事前説明のどちらで扱うかをRF-08・CC-05に一文で明記する。backup／restoreの詳細設計前に閉じる。

## 4. 検討したがFindingにしなかった事項

Finding数を増やさないために、疑問として検討した事項の分類を記す。ここに挙げたものはいずれも上位architectureの変更理由にならない。

| 検討した事項 | 分類 | 判断 |
|---|---|---|
| Realtime VoiceをClientからProviderへ直結する場合、応答生成がProvider sessionで行われ、個体調整（Host）の「会話の進行中の意味判断」が名目化しないか。 | Design Freedom | RT-05は条件付きでClient経路を許し、SO 4.3はProvider sessionを継続会話の正本にしないと定める。Hostがcontext・Tool呼出し・transcriptの受理を握り、Clientが中継に留まる構成で成立する。Realtimeが成立しない場合の段階的切替も要件が定めており、boundaryを圧迫しない。 |
| 依存graphがほぼ完全（全責務 ↔ 権限・制約、推論、保全・消去）で、層構造が導出できない。 | 問題ではない | 要件「共通pipeline」「全経路への同じ制約」から導かれる形であり、DRは非循環化のための汎用mediatorを明示的に拒む。module層はSubsystem境界と独立に詳細設計で決める必要があるが、それはarchitecture上の欠陥ではない。 |
| Context Assemblyが四責務（用途＝個体調整・作業、内容・根拠＝認識・学習、利用範囲＝権限・制約、能力差適応＝推論）に分散し、専用Subsystemを置かない。 | 後続詳細設計事項 | 共通の機械的処理を複数Subsystemが共有することは許されている。最頻経路であるため詳細設計の初期対象にすべきだが、boundaryは足りている。 |
| 個体調整の責務が広い（個体lifecycle、会話・交流・非会話活動記録、未伝達、自発性設定、観測eventの意味判断、委任・統合、移動意図）。 | 問題ではない | 遂行・意味状態・強制・存在・提示を外に置くことで、同じ個体の判断・文脈を一箇所に保つ責務に限定されている。停止・削除の管理経路がLLM応答から独立する条件も付いている。 |
| 個体調整が所有する自発性のOFF・上限が、権限・制約の全経路強制modelの外にある。 | 後続詳細設計事項 | CC-06は「個々のLLMが停止を選ぶことだけに依存せず」と機械的抑制を要求している。個体調整内で意味判断と機械的gateを分ける必要があるが、要件はこれを権限・制約に置くことを求めておらず、Owner設定の所有先は妥当。 |
| 「Companionを通じて」行う追加指示・承認・Cancelのうち、何がactive Client制約を受ける会話で、何が任意Clientからの管理操作か。 | 問題ではない | DR 3.4がCancel・Schedule管理・記録確認・承認拒否・停止・削除を管理経路として分離し、steeringは個体調整経由（会話）とする。要件の「発見可能な管理経路」「keyboard経路」と整合する。UI設計で線引きを表示へ反映すればよい。 |
| Companion削除に個体調整（lifecycle）と保全・消去（内部全域成立）の二つのcoordinatorがある。 | 後続詳細設計事項 | RF-06 段階5が「内部対象が未完了なら削除完了と表示しない」と完了表示の条件を保全・消去側に置いており、成立条件は決まっている。Ownerへの報告主体はUI設計。 |
| 削除後のTask引継ぎで、元担当向けのPermission判断がそのまま新担当へ使われないか。 | 問題ではない | CC-01は判断対象に「実行主体・委任元」を含め、「重要な意味が変われば現在の条件で再評価する」と定める。SO 4.10も旧担当の承認を引き継がないとする。 |
| グループ会話・Companion間交流から形成される「共有Experience Summary」を、どのCompanionのProviderで形成するか。 | 後続詳細設計事項 | 要件は共有Summaryの存在を許すが単一形成を要求しない。認識・学習が個体ごとにSummaryを形成する構成も、共有Summaryを形成する構成も、SO 4.5とCC-02の「共有Summaryの保持は全内容の共有ではない」の下で成立する。 |
| Agent Skills付属scriptの実行にsandboxを求めていない。 | Design Freedom | 要件はLocal MCPにだけsandbox既定を置き、Skill scriptには通常のCapability・Permission適用を求める。DR 5.3はこれを踏襲する。安全な実行方法の選択はeneが担うため設計で選べる。 |
| Observerがdesktop全体を観測するため、別Companionとの一対一会話画面が画面経由で他Companionへ届く。 | 問題ではない（要件の決定） | 要件は観測対象をdesktop全体と定め、Learning状態のaccess制限とは別の契約。ene自身のwindowを除外するかは設計選択。 |
| Provider側のprompt cacheやsessionに削除済み情報が残る。 | 問題ではない | 要件・SC-07・SO 5.3が外部保有copyを消去保証から除外している。 |
| 保全・消去がcoordinatorからownerへ滑る懸念（Review #1 F-03相当）。 | 問題ではない | Host自動起動は入出力・提示へ移され、SO 4.24が保全・消去の所有理由を「data保全・消去・復旧そのものの意味」に限定している。八文書間で所有先の記述に不一致はない。 |
| targeted deletion中にOwnerが同一文字列を含む新しい発言をした場合の扱い。 | 要件のAmbiguity（軽微） | 第8節 RA-05。architecture側は「後日」の新Experienceを扱うが進行中の到着は未定義。設計で決められるが、Owner可視の挙動であるため要件側の確認を推奨。 |
| 八文書が同じ契約を繰り返し記述している。 | Findingにしない | 実際の不整合は確認できなかった。第7節に維持riskとして記す。 |

## 5. Requirements Conformance

### 要件からDriverへ

要件の全章はAD-01〜15のいずれかへ根拠付きで到達する。Requirements basisに挙げられた見出しは実在し、Driver本文と一致する。2026-09-07のOwner decisions（Companion間交流のHistory保持、Stopped個体のpresenceなし、Observer専用assignment、Textのactive Client帰属、Computer Useのactive Client限定、activeなし時の継続・報告）は要件本文に反映され、Driver・SC・RT・Subsystem・SO・DR・RF・CCで同じ意味で扱われている。受け入れ条件のSupport Matrix・数値Gate・OpenAI Responses APIは恒久制約へ昇格していない。参考資料のHarness層分けは根拠にされていない。

### Driverから下位artifactへ

- 意味の変化: 確認できなかった。特に、個体削除で残るHistory・非会話活動記録・Task記録と消える個体固有状態・Relationship・Scheduleの区別、切断時のHost PC上Clientへの移動とStopの区別、Running個体のactiveなしとStoppedの区別は、全artifactで一致している。
- 要件にない強い制約の追加: 下位が追加した制約は、通常ToolからEne内部正本・管理入口への回り込み禁止（DR 4.1）、判断用推論への割当同意・費用制限の適用（DR 5.2）、未伝達事項のfull backup包含（SO 4.4）、Host上の作業用Local MCPのHost配置（RT-06）の四点に限られ、いずれも要件の信頼境界・同意契約・Client非依存継続から導出可能な範囲にある。
- Design Freedomの喪失: 確認できなかった。process配置、IPC、pairing手段、駆動・待機方式、調停方式、Summary粒度、減衰式、Permission評価algorithm、sandbox機構、backup形式、Task化閾値、Capture時機は各artifactのDesign Freedom節で維持されている。
- 要件の取りこぼし: 確認できなかった。

### 要件側との差分

architecture各artifactは「新たなRequirement Ambiguity／Gapなし」と宣言しているが、第8節のとおり六件の小さなAmbiguity／Gapがある。うちRA-01・RA-02はF-01・F-03に連動し、残りはarchitectureの構造に影響しない。

## 6. Architecture Coherence

### 静的な責務・ownership・dependency

- 12 Subsystemはそれぞれ異なる変更理由・lifecycle・trust・failure境界を持ち、統合すると要件上の区別が壊れる分離に限られている。Subsystem内に統合された別契約（作業内のTask／Task Agent／Schedule／Workspace／中間file、認識・学習内の5概念、入出力・提示内のBody／Voice／Text／管理／外部UI）はState Ownershipで実際に別stateとして扱われている。
- 同じ意味に対する正本は一つに保たれている。Task進捗と個別Actionの確定度、Provider登録情報と割当同意、接続事実・device許可・active帰属、Rule解釈と確定、Memory／Relationship／Companion State、Audit事実と記録順序は、いずれも意味を分けたうえで境界が明示されている。
- content → controlの昇格禁止は、LLM出力・Learning・Character・Skill・MCP・Plugin・MCP Apps・Workspace file・Observer routingのすべてに適用され、Task Agent・Schedule・自発性・fallback・Client直結経路による迂回も閉じられている。
- Dependency Rulesは層構造を作らず、役割付き依存の許可範囲・禁止関係・強制箇所の義務・禁止近道で構成される。要件の性質から導かれる形であり、後続のDR適合確認は「禁止関係を通れる経路がないか」「各強制対象の利用箇所が条件依存を持つか」で行う必要がある。
- F-01・F-02が示すとおり、既決のscope model（Companion／Global）と非正本原則（Client非cache）は、それぞれObserver routing用の文脈とClient側接続材料という**既存区分に属さないdata**を持ち、そこだけ位置付けが未定義である。境界の作り直しは不要で、区分の追加で閉じる。

### 時間軸上のruntime behavior

- RF-01〜08は既決のowner・依存・lifecycleだけで正常系・異常系を記述しており、Flowのために新しいcoordinator・store・stateを追加していない。各Flowの中断先（判断待ち・不明・未完了・保留）が担当ownerへ閉じている。
- 「Clientを閉じる」「個体を移動する」「個体を停止する」「個体を削除する」「Hostを再起動する」「restoreする」は異なる状態遷移として維持され、CC-04の表で抑止範囲と復帰条件が一覧化されている。
- 第9節のwalkthroughのとおり、partial failure、stale・delayed result、cancellation、disconnect、restart、deletionとのrace、authority失効、restoreの切替のいずれでも、ownershipとdependency境界は崩れなかった。
- 一点、RF-05・SO 4.15・SO 7の組合せは、Host再起動後のRunning個体を「過去にactiveだった記録を正本にしない」→ activeなしへ寄せて読める。要件のG-02決定（接続済みClientへの自発移動を自動化しない）と整合するが、要件の「現在のClientに留まる」「Hostの再起動…を越えて同じ個体として継続する」と合わせた読みが分かれ得る（RA-06）。architectureの矛盾ではない。

### 複数Flowをまたぐcross-cutting contract

- CC-01〜07は、SO・DR・RFに散在する契約を、由来（CC-01）、利用範囲（CC-02）、現在性（CC-03）、停止範囲（CC-04）、目的別lifecycle（CC-05）、消費と制御経路（CC-06）、確定度（CC-07）へ再編したものであり、新しいowner・component・統一state machineを導入していない。契約間の優先関係（targeted deletionの優先、scope・同意の優先、停止・保留が事実報告・停止受付・機械的検証を塞がない）も既存決定の再掲であり矛盾はない。
- CC-03の遅延結果受入表は、Review #1以降に加わった検証で最も価値のある部分であり、RF文書で散発的に現れていた現在性判断を用途別に固定している。
- 八文書間の識別子（AD／SC／RT／Subsystem略称／SO節／DR／RF／CC）は実際に辿れる。相互参照の食い違いは確認できなかった。

## 7. Complexity / Over-engineering Assessment

### 過剰設計

- 不要なSubsystem: なし。Character、接続・存在、共有観測、認証秘密は薄く見えるが、配布物へのprivate混入防止、存在排他の横断性、Client単位共有と個体判断のscope差、秘密値と説明の分離という安全・lifecycle上の理由を持つ。最初のmilestoneでは共有観測以外の11責務が実際に必要になる。
- 不要な抽象化・中間層・coordination: なし。汎用mediator、Context／Settings／Persistence Subsystem、presence service、資源配分Subsystem、Provider別・MCP別・Plugin別Subsystem、統一state machine、単一transactionの追加はいずれも明示的に否定されている。DRの六つの「依存の役割」、CCの七契約は説明用分類であり、interface階層やcomponentを要求していない。
- 過剰な汎用policy／coordination: 権限・制約は多種の制御条件を束ねるが「強制箇所を一つのprocessや呼出し列へ固定しない」と条件付けられ、保全・消去は五種の横断操作に限定されている。要件（backup全体、全域消去、全置換restore）が横断性を要求しており、過剰とは判断しない。
- 将来要求由来のarchitecture: なし。非目標（Cloud正本、恒久Workspace、成果物library、汎用Plugin API、relay／account／Marketplace）は再導入されていない。
- 実装選択のarchitecture固定: なし。RT-06のLocal MCP Host配置、RT「Companionごとの専用runtime serviceを追加しない」は配置上の負の制約であり、process構造は自由に残されている。

### 不足

- F-01〜F-03。いずれも既存境界内の区分追加・場合分け追記で閉じる。
- Context Assemblyは最頻経路でありながら専用の設計対象になっていない。Subsystemを追加しない判断は妥当だが、scope適用・Credential非露出・割当同意・費用制限・Provider能力差への適応が同じ経路で交差するため、詳細設計の初期に明示的な設計対象とすべきである（第10節）。

### Premature Commitment

- 上位architectureの作り直しを強いる先行決定は確認できなかった。
- State Ownershipの24区分は概念domain modelに近い粒度だが、表現・保存単位・writerを固定しておらず、個別の所属判断が後続で微調整されても上位の作り直しにはならない。

### Artifact集合の重さ

八文書は同じ契約を繰り返し記述しており、Computer Useのactive Client限定、切断時のHost PC Client移動、missed非補完、Stopped個体のpresenceなし等は各文書に複数回現れる。本レビューでは実際の不整合を確認できなかったが、識別子体系が八種類に達し、ある契約が「どの文書のどの決定」に由来するかを読み取る負担は大きい。要件変更時に八文書を同期させるrisk、および詳細設計者が「決定」と「再掲」を区別できないriskがある。architecture上の過剰設計ではないためFindingにしないが、今後の改訂では各artifactが自身の決定だけを記述し上位契約は識別子参照に留める運用、または決定ごとの正本文書を示すindexを推奨する。

## 8. 新たなRequirement Ambiguity / Gap

以下はarchitecture artifactの欠陥ではなく、要件側に残るOwnerから観測できる挙動のAmbiguity／Gapである。Design Freedomや実装未決定は含めていない。architecture各artifactの「新たなIssueなし」宣言に対する差分として記す。

### RA-01 — Observer routingに用いるCompanion側文脈の位置付け（F-01と対）

要件はObserverが「文脈との関係を判断」して関係がありそうなCompanionへだけ伝えることと、routingを理由に「Companion固有情報の利用範囲やProviderへの送信同意を広げない」ことを同時に定めるが、関係判断に用いてよいCompanion側の情報が何であり、それをObserver専用Providerへ送ることが「利用範囲を広げる」に当たるかを定めていない。Owner設定の信号だけで判定するのか、Companion側が提供する関連性情報をObserver同意の対象dataとして扱うのかは、Ownerが同意する送信内容とrouting精度の両方を変える。

### RA-02 — Restore時の現在Credential storeの扱い（F-03と対）

restoreを「ene内部dataの全置換」としつつCredentialをbackupから除外し「再認証が必要になり得る」とする記述は、現在のCredential storeを維持するか消去するかを確定しない。Owner可視の挙動（restore後に既存keyがそのまま使えるか）が変わる。

### RA-03 — Learning revision・Experience Summaryの容量管理経路の不在

要件は通常lifecycleで保存済みLearning・過去revision・根拠を削除せず、Summaryを根拠として保持し、容量目的の削除経路として「指定日以前のConversation HistoryやTask等のlog」の手動削除と保持期間だけを定める。Learning revisionとSummaryは「log」ではなく、targeted deletion（Privacy／Security目的限定）とCompanion削除（全部）以外に整理経路がない。保存容量の上限到達時にeneが「安全に停止またはOwnerへ判断を求める」としても、Ownerが選べる選択肢が上限引上げ・History削除・個体削除に限られ、revision・Summaryの増分に対する管理手段がない。architectureはこの要件を正しく反映しており（保全・消去が「容量都合の黙った削除」を拒む）、欠陥は要件側にある。長期運用で顕在化する。

### RA-04 — Character Packageから取り込んだ推奨Skillの内部scopeと削除範囲

要件はExperience由来のLearningにCompanion scopeを既定とし、Companion削除時にCompanion scope Skillを過去revisionを含めて削除すると定めるが、Character Packageの推奨Skillとして取り込まれた内部Skill（Experience由来ではない）のscopeを定めていない。同じCharacterから作った二個体が同じSkillを別々に持つのか、一つを共有するのか、個体削除で取込原本が消えるのかがOwner可視の挙動として分かれる。architecture（SO 4.7）は「Companion／Global scopeの条件はMemoryと共通」とするだけで、取込時の既定を決めていない。

### RA-05 — Targeted deletion進行中に到着した同一情報の新規Experienceの扱い

要件は「削除後」にOwnerが改めて提供した情報を新しい根拠として扱えるとするが、削除開始から残存検証完了までの間にOwnerが同一文字列を含む発言をした場合、機械的な残存検証がそれを対象として削除するのか、新規Experienceとして残すのかを定めていない。architecture（RF-07）は「後日」の再提供だけを扱う。挙動はOwner可視であり、削除対象を要求時点のsnapshotとするか検証完了まで継続とするかの判断を要する。

### RA-06 — Host再起動後のRunning Companionの再帰属

要件はG-02で「接続済みClientへの自発的な移動は…自動化・義務化しない」と決める一方、「Companionは通常…現在のClientに留まる」「Hostの再起動…を越えて同じ個体として継続する」とも定める。Host再起動で全Clientが切断・再接続する場合、最後のactive Clientが再接続したときにCompanionがそこへ自動的に戻るのか、呼出し・事前指示・自発判断を要するのかは、両方の読みが可能である。architecture（SO 4.15「過去にactiveだった記録を正本にしない」）は後者へ寄せて読める。最初のmilestoneの単一Client構成では起動ごとに現れる挙動であるため、要件側での確認を推奨する。architectureの変更は、どちらの決定でも不要である。

## 9. Failure and Interruption Walkthrough

レビューで実際に辿った異常系と、ownership・dependency境界が維持されるかの結果。「維持」はarchitecture上の成立判断であり、実装・性能の検証ではない。

| Scenario | 辿った境界 | 結果 |
|---|---|---|
| Remote Client AでComputer Use実行中にnetworkが分断。HostはHost PC上のClientへ個体を移動。その後Aのnetworkが回復。 | RT「Client終了・Remote切断」、SO 4.15、DR 7.6、CC-04 | Aは排他性を確認できない間は対象活動を継続せず、Hostは作用不明を保持。Host Clientで旧Actionをreplayしない。再接続時はAの旧帰属・未送信操作を正本へ戻さない。維持。切断検知・timeoutは自由度。 |
| targeted deletion（文字列X）の開始時、Task AgentのcontextにWorkspace fileから読んだXがある。削除完了後、Agentが同じfileを再読込。 | SO 6.4、DR 7.2、RF-07、CC-03 | 削除前contextからの結果はXを再保存せず、外部fileの新規読取は新しいExperience。外部fileは消去対象外。維持。進行中に到着したOwnerの同一文字列発言はRA-05。 |
| Companion Aを削除。Global Memory MがAの個体固有Summary Sを根拠に持つ。 | SO 4.5・4.6、6.3、7、CC-02・05 | Mは残り、Sは共有根拠として残し得るが他Companionのaccessは広がらず、Ownerには残存・参照不能を削除前に示す。維持。 |
| targeted deletion完了後、それ以前のbackupからrestore。 | SO 6.5、RF-08、CC-05、要件「Backupとrestore」 | 情報は戻り、Auditも全置換されるため削除の記録も戻らない。Ownerへの事前説明で覆われる。維持（要件の決定）。 |
| Running個体がHost PC ClientにいるままHost再起動。Clientが再起動して再接続。 | SO 4.15、SO 7、RF-05、G-02 | architectureはactiveなしとして再確認し、呼出し・事前指示・自発判断で帰属を再成立させる読みに寄る。維持。RA-06として要件確認を推奨。 |
| activeなしのRunning個体のScheduleが到来し、TaskがComputer Use stepへ到達。 | RF-05、CC-04表、SO 4.12 | Host部分は継続、Client依存Actionは利用不能として作業が判断待ち等へ置き、自動承認や別Clientの選択をしない。維持。 |
| Companion AがClient 1に存在する間に、Ownerが非active Client 2のTask管理面から判断待ちActionを承認。 | DR 3.4、SO 4.19、RF-02、CC-01 | 承認は管理経路で権限・制約へ届き、Companionの移動を要しない。追加指示（steering）は会話として個体調整経由。維持。 |
| 同一Clientの二個体へObserver eventをrouting。一方の個体のProviderだけが失敗。 | RF-03、DR 5.3、CC-02 | 失敗個体の理解・活動は成立せず、他個体の判断や共有検知結果をコピーしない。維持。routing用文脈の出所はF-01。 |
| Realtime Voice session中に個体が別Clientへ移動。 | RT-05、SO 4.3、RF-04、CC-04 | 入出力・提示がroundを区切り、新Clientでsessionを再成立。transcriptはHostのHistoryへ。Provider sessionを正本にしない。維持。 |
| 費用cap到達時にPermission解釈用推論が実行不能。 | DR 5.2・5.6、SO 4.19・4.20、CC-06 | 審査対象Actionは判断待ち。Owner停止・拒否・機械的消去・正常保存はLLMに遮断されない。維持。 |
| 削除済みCompanionのTaskを別Companionへ引継ぎ。旧担当向けのAllowが残っている。 | SO 4.10・4.19、DR 4.2、CC-01 | 実行主体の変更は重要な意味変更として再評価対象。旧担当の私的Learning・承認・Credentialを継承しない。維持。 |
| 最初のmilestone構成（Host同居Clientのみ）でClient processがcrash。Hostは継続。 | RT「Client終了・Remote切断」、G-02、RF-04 | 移動先となるHost PC上のClientが利用不能のためactiveなし。Task継続、未伝達をHostに保持。Client再起動後は呼出し・事前指示・自発判断で復帰。維持。 |
| Companion Stop中にSchedule到来、その後Resume。 | SO 4.14、7、RF-06 | missedのまま。Resumeで旧Task・不明Actionをreplayしない。維持。 |
| Local MCPがsandbox内で動作不能。 | RT「Local MCPの実行境界」、DR 6.2 | 黙ってsandbox外へ切り替えず不足を返す。維持。 |

## 10. Later-design Watch Items

Findingではないが、詳細設計で忘れると境界が形骸化する事項に限る。

1. **Context Assemblyを最初の設計対象にする。** 用途・内容・利用範囲・能力差適応の四責務が交差する最頻経路であり、後付けではCC-02の「同じ情報選択方針」とscope・秘密非露出・同意・費用の同時適用を検証できない。
2. **Realtime VoiceのClient中継pattern。** Hostがcontext・Tool呼出し・transcript受理を握り、Clientは音声とsessionの中継に留める。短期認証材料の発行と失効、transcriptのHost反映状況の扱いを最初に定める。F-02の位置付け決定と合わせる。
3. **Owner意図の由来の機械的保持。** 個体調整が権限・制約へ渡す解釈には起点となったClient入力への参照を必ず付け、由来がTool result・Workspace file・MCP Prompt・画面内容でないことを機械的に確認できる形にする。DR 5.2・CC-01は原則の記述に留まる。
4. **個体調整が所有する自発性OFF・上限の機械的gate。** CC-06の要求を、個体調整内部の意味判断（LLM）と活動開始箇所の機械的抑制へ分けて実装する。権限・制約の強制経路とは別の第二の強制箇所になることを認識して設計する。
5. **Companion削除の完了報告主体。** 個体調整のlifecycle成立と保全・消去の内部全域成立を、Ownerに一つの完了として示す責務と、部分失敗時の説明の出所を決める。
6. **targeted deletionの対象snapshot境界。** RA-05の要件確認結果に従い、要求時点以降に到着した同一情報の扱いを機械的残存検証の仕様へ落とす。
7. **Restoreの静止条件。** 全置換の前に進行中Task・Task Agent・consolidation・観測をどう静止させ、静止不能時に何を保留・報告するかを定める。F-03の決定と合わせる。
8. **各artifactの決定正本の明示。** 第7節のとおり、今後の改訂では決定と再掲の区別を読める形にする。

## 11. Final Recommendation

- 現在のarchitectureは、後続設計（module／crate、process、IPC、DB、Harness、Context Assembly、Permission評価、sandbox、backup形式等）の安定した前提として採用できる。
- 先に修正すべきもの: F-01〜F-03はいずれもState Ownership・Runtime Topology・Dependency Rulesへの小規模な区分追加・場合分け追記で閉じる。F-01はObservationの詳細設計前、F-02はRemote Clientの詳細設計前、F-03はbackup／restoreの詳細設計前に処理すればよく、最初のmilestoneの詳細設計を止める必要はない。
- 要件側で決めるべきもの: RA-01・RA-02（F-01・F-03と対）、RA-03（容量管理経路）、RA-04（取込Skillのscope）、RA-05（削除進行中の新規Experience）、RA-06（Host再起動後の再帰属）。RA-06は最初のmilestoneで起動ごとに現れる挙動であるため早期に確認することを推奨する。他はそれぞれの機能の詳細設計前に確定させればよい。
- 詳細設計へ進めるか: 進める。gateを再度閉じる必要はなく、上記の修正は後続設計と並行できる。

### 「このarchitectureをほぼ固定した前提として詳細設計を始めた場合、後から上位architectureを大きく変更する可能性のある問題がまだ残っているか」への独立回答

残っていないと判断する。上位の作り直しを誘発し得る候補として検討したのは、(1) Observer routing用文脈の位置付け（F-01）、(2) Realtime VoiceのClient直結経路とHost所有の意味判断、(3) Context Assemblyの分散、(4) 権限・制約の全経路強制がhot pathに乗ること、(5) 依存graphが層構造を持たないこと、の五点である。(1)はどちらの決定でもSubsystem・owner・Topologyを変えず、Observer専用assignmentの同意対象dataと参照区分の追記で閉じる。(2)はHostがcontext・Tool・transcriptを握りClientが中継に留まる構成で既存境界内に収まり、成立しない場合の段階的切替を要件が定めている。(3)(4)(5)は詳細設計の負担を増やすが、要件「共通pipeline」「全経路への同じ制約」から導かれる形であり、境界の変更ではなく実装構造の選択で吸収される。RA-03（容量管理）はarchitectureではなく要件の長期的なGapであり、要件側の決定がどちらでも保全・消去と認識・学習の境界は変わらない。

## 12. Reviewer Self-check

- 本レビューのFindingはプロンプトの例示に依存していない。例示された観点（Host正本、Client非正本、Companion／Task lifecycle、presence、Computer Use制約、意味判断と強制の分離、external stateの非正本化、cross-domain operation、第二の正本の禁止）はいずれも問題なしと判断し、Findingにしていない。F-01〜F-03、RA-01〜06は要件と八文書を独立に横断して見出した。
- 特定artifactに偏っていない。Findingは共有観測・接続・存在・認証秘密・保全・消去に跨がり、影響先はSO・RT・DR・RF・CCに分散している。
- 既存の用語や境界を無批判に前提化していない。個体調整の広さ、権限・制約のhub化、Companion／Global scope modelの適用範囲、Client非cache原則の適用範囲を疑い、後二者からF-01・F-02を導いた。
- 一般論やpatternの好みをFindingにしていない。「依存graphが密である」「文書が重複している」「Context Assemblyに専用Subsystemがない」は観察としてCoherence／Complexity／Watch Itemsに記し、Findingにしていない。
- implementation detail（IPC、DB、process、timeout、heartbeat、pairing手段、Context Assemblyの具体方式）の不足をarchitecture defectにしていない。
- 本当に重要な問題より細かいFindingを優先していない。検討した十数件のうち、Owner可視の安全・Privacy境界または所有の未定義に関わる三件だけをFindingとし、残りは第4節で分類を明示した。
- Finding間に矛盾はない。F-01とRA-01、F-03とRA-02は同じ対象のarchitecture側とrequirements側の分担であり、重複ではない。
- SeverityはいずれもMinorとした。三件とも既存境界の分割・統合や依存方向の変更を伴わず、それぞれの機能の詳細設計前に閉じればよい。
- 本工程では `docs/design/reviews/architecture-review-2.md` 以外を変更していない。
