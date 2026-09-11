# Context Selection / Continuity — 会話とTaskの文脈選択

本書は [Context Assembly](../critical-areas/context-assembly.md) の用途限定・参照・変換・不足・現在性の契約を、継続的に変化する会話と長時間Taskの文脈選択へ具体化する。[設計文書 README](../README.md) の優先順位に従い、[個体調整・作業・認識学習](../subsystems/companion-task-learning.md)、[Interface Boundaries](interface-boundaries.md)、[Persistence / Recovery](persistence-recovery.md)、[Concurrency Control](concurrency-control.md) の既存owner・保存分類・対応関係を使う。新しいSubsystem、知識の正本、共通ContextManagerは設けない。

## 1. 採用判断

採用するのは、**用途ごとの予算付き文脈選択と、必要箇所を保持済みの原記録から取り直せること**である。CodexのExperimental context managementを依存部品として組み込まず、会話全体をmodel管理のnotesへ移す方式も採らない。設計上の利点と、実測が必要な品質・費用の改善を区別する。

| 選択肢 | 利点 | eneでの制約と判断 |
|---|---|---|
| 長い履歴を毎回渡す | 直前の流れ・表現を残しやすい | 長さ・費用・遅延が増え、いずれ上限に達する。予算内の比較baselineには使えるが、無制限にはしない。 |
| 一つの要約を繰り返し更新する | 少量の文脈で継続できる | 要約漏れや古い解釈が次の要約へ残る可能性がある。目的限定の圧縮として使えても、原記録・現在認識の代替にしない。 |
| modelがnotesを管理し、窓を丸ごとresetする | 作業途中の情報を選んで持ち越せる | 保存・読戻しの判断、tool能力、復旧経路へ依存する。通常会話の既定にせず、Taskの縮小方式もこれ一つに固定しない。 |
| Working StateとEpisodeを新しい永続domainにする | 話題と未完了事項を明示できる | Memory・Task contextとの重複、話題境界の誤判定、更新・消去責任が増える。必須化しない。 |
| 最近の履歴・現在認識を選び、必要時に原記録を補う | 既存ownerを保ち、話題変更と再訪を同じ経路で扱える | 検索漏れ・古い候補・追加取得の遅延を制御する必要がある。これを基本方針にする。 |

この採用は「新方式の方が必ず高精度・低費用」という結論ではない。追加の継続hintやindexは第7節の比較で価値が確認できる場合に限り採用し、効果がなければ既存の選択経路を維持する。

## 2. 所有と時間軸

Ownerに見せる一続きのtimeline、Experience Summaryの意味的まとまり、推論requestに載せる範囲を区別する。話題の変化、token上限、round終了のいずれも、それだけでは会話終了・Task完了・Learningの忘却を意味しない。内部の話題分類を使っても単一のactive topicや排他的なepisode分割を要求せず、複数の話題や非連続のsource範囲を選べる。

通常会話の選択は個体調整、Taskの選択は作業が担い、各情報ownerから提供を受ける。Memoryは現在の知識・時間的意味、Summaryは形成の根拠、Relationship・Companion Stateはそれぞれの関係解釈・内的状態のままである。情報が短命であることだけを理由にMemoryから別の知識storeへ移さず、形成・更新・想起抑制は認識・学習の判断に残す。

会話の継続hintを使う場合は、個体調整の用途限定の派生表現とし、必要な話題・未解決の参照先・選択理由の短い手掛かりへ絞る。元の発言、受理したTask指示、未伝達事項等を失ってもhintだけあればよい構成にはしない。Taskのcheckpointも既存Task context・記録の再開用表現であり、独立した作業状態やAgent scope Learningではない。hintや検索結果を読み直したこと自体を、新しいExperienceや同じ事実を補強する独立した根拠として扱わない。

hintの省略・再生成・cache失効は、元History、Memoryと過去revision・根拠、Task記録の削除や意味変更ではない。反対に、正本の削除・訂正・scope変更を古いhintで取り消さない。保存価値がある情報を派生hintと呼んで既存の保持契約から除外せず、通常保存しないRaw Voice・Raw Observation・詳細Tool payload・内部推論を復旧目的で一律保存しない。

## 3. 変化する会話の選択と再参照

### 3.1 現在の入力を起点にする

通常会話では、各turnの現在入力、対応する最近の原履歴、利用可能な現在のMemory等から必要な文脈を選ぶ。短い返答・代名詞・比較の対象を解釈できるよう最近のやり取りを重視し、検索結果だけで置き換えない。固定message数やMemory件数は読取量の上限に使えても、必要文脈とrequest総量の保証にはしない。

`Aの話 → Bの話 → Aへの再訪`では、Bへの移行だけでAの記録や未完了Taskを失わず、Aの全内容をBへ常設注入もしない。曖昧な「さっきの件」は最近の履歴と関連候補を使って解釈し、一意に判断できなければ必要な対象だけを確認する。短い雑談まで目的・progress・next stepsへ無理に変換しない。

訂正や状況変化は、取得した古いMemory・要約と最近の入力を区別して扱う。Learningの非同期更新が終わる前でも、関連する新しい発言・訂正が応答に利用できるようにする。新しい発言は常に事実、引用は常にOwner命令、時刻が新しければ常に上位とはしない。Memoryの意味変更は既存の形成・revision経路へ戻す。

「明日」「来週」等は発言時点の日時・timezoneと必要な時間的意味を保ち、後の読戻し時点から解釈し直さない。話題から離れた時間や再起動を、予定の継続・完了の証拠にしない。継続hintを更新するときも対象入力・source範囲との対応を保ち、遅れて届いたhintを現在の会話へ無条件に採用しない。

### 3.2 原記録を必要な範囲だけ読む

必要なsourceの参照が分かっていれば直接取得を優先する。分からなければ、現在用途で利用できる範囲を検索してから原記録を読む。既存のmessage・source範囲の対応を利用し、本文一致で同一発言を判定しない。同じsourceの重複取得・注入を避け、現在入力は一度だけ載せる。ただし、Memoryとその根拠のように意味が異なる材料を本文の類似だけで同一視しない。context専用の永続Window IDや新しい公開ID体系は必須にしない。modelへ渡す参照の表現は既存のopaque projectionと所属検証の契約に従う。

検索結果は所在の手掛かりであり、根拠の全文でも現在値の確定でもない。原記録の話者・時点・会話空間・用途に必要な前後関係を保持し、現在認識を答える場合は対応する訂正・後続の変更も確認する。過去時点の説明には当時の有効性を残し、現在値の質問に古い記録だけを使わない。

日本語の言い換え、表記揺れ、話題再訪をliteral一致だけで扱えるとは仮定しない。検索方式は固定せず、候補の拡張やsemantic検索を導入する場合も必要性を比較する。indexの遅延・検索失敗・保持終了・権限上の利用不可・上限による省略は事実不在と区別する。参照が既知ならindexの更新を待たず、許可された正本の読取で補える。

読取の対象選択と本文取得の両方で、現在のconsumer・会話空間・Companion・Task・scope・送信同意を適用する。Global Memoryや同じWorkspaceから私的なHistoryを包括取得せず、結果の存在・件数・参照値にも必要な保護を適用する。通常の想起抑制を古いhintの常設注入で迂回しない。明示的な再参照とtargeted deletion後の再形成は別契約として扱う。

最初はHostによる必要範囲の取得でよく、model-facingのread-only toolはtool loopが必要になった経路だけへ公開できる。内部検索を任意SQL・file I/O・正本編集の入口にせず、外部sourceの取得は既存Action境界を通す。内部Historyから読み直した本文も、新しいsystem/developer命令やPermissionには昇格させない。

## 4. 予算、応答性、Provider適応

予算はmessage件数だけでなく、**最終的に送るrequest全体**に対して適用する。現在modelの利用可能な入力枠と、出力・推論等に必要な余裕、instructions、tool定義、schema、画像・音声等の寄与をProviderの数え方に従って考慮する。総contextと入力上限が別なら両方を満たす。token数・文字数・bytesは区別し、既存の入力文字数やtransport上限も独立に守る。

入力予算では、現在入力とその解釈・安全・目的に不可欠な情報を先に確保し、残りへ任意の背景を選ぶ。安全な整形・Credential scrub後の表現も含めて送信前に検査し、任意情報は意味の分かる単位で縮小する。tool callとresultの対応、原文引用、否定・訂正の関係を壊す切断はしない。不可欠な入力が収まらない場合は不足として扱い、先頭や末尾を黙って切って送らない。

tokenizerやmodel上限が不明な場合は、推定・不明を区別し、設定された保守的な上限や対応可能な経路を使う。厳密に計数できたと偽らず、成立を判断できない用途は保留する。model / Provider切替時は予算・表現能力・送信条件を再評価し、Codex固有backendやmodelによる自発的な`new_context`を成立条件にしない。

通常応答のたびに話題分類・hint更新・検索計画の追加LLM呼出しを必須にしない。任意の補助処理は応答を不必要に待たせず、過去sourceが回答に不可欠な場合だけ必要な取得を待つか、不足を説明する。Voiceでは確定した発話と提示・中断の対応を使い、未確定認識や未提示の返答を会話の既成事実にしない。

追加取得には、1回の出力量だけでなく1応答・Task step内の回数、累積文脈量、時間、費用の上限を適用する。進展のない検索・要約・resetの反復を止め、任意背景の失敗なら縮退し、必須根拠や安全境界の失敗ならその用途を保留する。未解決hintの存在は、自発発話・通知・Task・Scheduleの新しいtriggerや、待機中のLLM pollingの根拠にしない。

Prompt cacheは同じ論理的文脈を効率よく送る最適化に限る。安定した部分の再利用を検討しつつ、hit率のために訂正・現在時刻・Permission変更を遅らせない。毎turnの再選択は通信sessionの毎回resetを意味しない。Providerに残るopaqueな圧縮状態やcacheが変更・消去対象を含み、安全な再利用を確認できない場合は、その状態を持ち越さず再構成できる経路を使う。費用評価にはcache readだけでなくwrite・missと補助推論の消費も含める。

## 5. 長時間Taskの文脈縮小

Taskは通常会話と異なり、複数step・tool resultの蓄積でrequestが増える。必要になった経路では、選択・目的限定の要約・checkpointからの再構成を比較し、特定のreset方式を全consumerへ適用しない。Learning・Observer・Permission解釈等は、それぞれの用途に必要なsourceと前提を揃える。特に安全判断の根拠を任意の想起に任せない。

窓を切り替える場合、作業ownerは切替後に必要な採用済み目的・steering・進捗・未完了事項と、その原記録への対応を既存Task context・記録として保全する。すでに起きた作用、未確定のtool結果、作用不明は元Action等のownerへ結び付ける。保持されていない詳細payloadや、変更された外部fileを、保存時点のまま正確に復元できると扱わない。

切替前には次requestが依存する継続記録をdurableにし、切替後のrequestを組むHost側で必要参照の解決と現在のTask・steering・停止・Permission・消去等の前提を照合する。保存失敗・参照不能なら必要状態を落としたまま次Actionへ進まない。条件を満たす旧文脈や要約で継続できなければ保留する。通常の補助hintだけなら必須の保存工程にはせず、正本から再構成してよい。

これは既存の局所commit・前提照合・結果受入を使う順序であり、新しい分散two-phase commitや全域state machineを要求しない。検証するのは記録・参照・組み立てた入力の成立であって、LLMが「復元できた」と述べることや、以前と同じ内部推論を再現することではない。旧prompt全体を二重に永続保存する必要もない。

steeringやCancelが切替と競合した場合も旧checkpointで巻き戻さず、同じtool callを不明のまま自動再実行しない。未完了のcall/resultを保持するか、必要事実を記録して安全な境界で再構成するかはProvider適応で決めるが、対応のないresultや重複Actionを作らない。切替・hint更新のための推論も通常の同意・費用・中断契約に従う。

稼働中のTaskの窓切替とHost再起動後のTask再開は別である。再起動後の途中Taskは既存契約どおりOwnerの明示再開を待ち、Client移動・再接続・Provider回復・checkpoint残存だけで再開しない。通常会話への話題変更を、Taskへのsteering・Cancel・再開として自動採用することもしない。

## 6. 保持と消去への参加

新しいhint・検索結果・部分読取・checkpointを実際に保持する箇所は、[Context Assembly](../critical-areas/context-assembly.md) 第8節と [Targeted Deletion](../critical-areas/targeted-deletion.md) の既存参加範囲へ加える。source参照、本文、生成途中の結果、cacheを含み、Credential登録・更新に伴う非露出も既存の境界へ接続する。

Privacy/Security目的のtargeted deletionを、modelによるnotes編集・要約・想起抑制で代用しない。通常のHistory保持終了は逐語参照不能として扱い、形成済みLearningをcascade削除しない。派生hintを原履歴の隠れたbackupにせず、再接続・遅延更新から対象内容を戻さない。外部Providerの保持を消せたと表示することと、eneがその旧状態を再利用しないことも区別する。

## 7. 導入順と検証

既存の [実装順とmilestone](../../implementation/README.md) は変更しない。導入するsliceに応じて、まず総量を検査できる文脈選択とsource対応を揃え、必要な過去参照へboundedな直接読取・検索を接続する。Taskの継続はTask contextとtool loopが成立した経路で追加する。新crate、汎用notes file system、Episode schema、常時稼働の分類Agentを先行実装しない。

会話hint、話題index、検索方式の高度化は、最近の履歴と既存Learningだけでは不足するケースを確かめてから追加する。hint生成失敗時も基本経路を使え、採用しない場合でも原記録の再参照を妨げない。固定のtopic状態enum、減衰式、窓の大きさ、reserve比率、tool名、DB schemaは本書で決めない。

| 検証ケース | 確認すること |
|---|---|
| 複数話題の交錯・再訪・短い相槌 | 最新topicへの固定や不要な旧話題の常設注入を避け、必要な前後関係を回収する。 |
| 訂正・予定変更・日付を跨ぐ再訪 | Learning更新前と後、古いhintの遅着、相対日時を含め、現在と過去を混同しない。 |
| 同文の再送・日本語の言い換え | source identityを保ち、literal一致の不成立を情報不在にしない。 |
| 逐語引用・省略・保持終了・index遅延 | 引用は取得した原文だけから行い、未取得・利用不可・欠落を捏造で補わない。 |
| 私的会話・グループ・Global・共同Task | source本文だけでなく所在情報にも範囲を適用し、参照を共有権限にしない。 |
| 引用された命令・偽の承認・抑制済み情報 | 要約やhintで由来を昇格させず、旧派生物を制約の迂回路にしない。 |
| 長い入力・大きなtool結果・小さいmodelへの切替 | 最終requestと累積取得の上限、必要情報の維持、不足時の安全な結果を確認する。画像等を扱う経路ではその費用も含む。 |
| hint未生成・検索障害・低速な補助推論・Voice中断 | 任意背景の縮退と必須根拠の不足を区別し、補助処理を毎回の応答条件にしない。 |
| checkpoint失敗・steering/Cancel競合・作用不明 | 古い目的での継続、二重実行、保存失敗後のblind resetを防ぐ。restartは明示再開を待つ。 |
| Credential更新・targeted deletion・再接続 | 派生物・遅延結果・Provider継続状態を使う再露出を防ぐ。 |

品質比較は同じ入力列・model・予算条件で、予算内の最近の履歴＋Learningをbaselineにし、追加履歴取得、hintあり／なし、Taskでは要約方式と再構成方式を個別に比較する。話題追従・制約維持・誤った現在値・原文根拠・不要な再確認・Taskの進展と重複作用を確認し、出力の揺らぎがある場合は複数回評価する。

遅延は応答開始の中央値・上位側と補助処理待ちを、消費は入力・出力・補助推論・検索回数・cache read/writeを分けて測る。保持量と再構成失敗も確認する。比較条件と測定値を実装PRへ記録し、測定前に高速化率・費用削減・無損失復元を主張しない。品質や安全を落とす最適化を既定にせず、効果が確認できない追加hintやindexは採用しない。

## 8. 参考にした公開実装（非規範）

2026-09-11確認。以下は機構の観察対象であり、eneの製品要件・安全契約・性能保証の代用ではない。

- Codex [`compact_token_budget.rs`](https://github.com/openai/codex/blob/02a8f038b87ad34d4a1dc5058eda26972ed7aa6c/codex-rs/core/src/compact_token_budget.rs): token-budget modeの窓切替はmodel/serverによる要約を呼ばない。eneではこのreset方式を一律採用しない。
- Codex [`history-notes/tools.rs`](https://github.com/openai/codex/blob/02a8f038b87ad34d4a1dc5058eda26972ed7aa6c/codex-rs/ext/history-notes/src/tools.rs): 原履歴の参照と作業notesを分け、参照IDとbounded readを利用する。Codex backend依存のAPI、他Agentのnotesへのaccess、履歴の保存範囲は移植しない。
- Codex [Guardianをsummary-based compactionに維持する変更](https://github.com/openai/codex/commit/dd112a9fd56dd18a73e593edaa9c2faddcec1106): consumerによって方式を分けている。eneにGuardianというSubsystemを追加する提案ではない。
- OpenAI [Compaction](https://developers.openai.com/api/docs/guides/compaction): Responses APIのopaqueな圧縮itemは、上記Codexのnotes・historyによる方式と区別する。Providerの継続状態をeneの正本にはしない。
- OpenAI [Prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching): 同じprefixの再利用とcache read/writeの計測を参考にする。model固有の料金・閾値・保持条件は固定しない。
