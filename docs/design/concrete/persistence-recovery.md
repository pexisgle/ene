# Persistence / 保存単位 / Recovery の具体設計 — Step 13 Concrete Design

本書は Step 13 の Persistence / 保存単位 / Recovery artifact である。[対応関係・識別](correspondence-identity.md)（CI）の identity / revision / generation / correlation / boundary token の意味を前提とし、変更しない。上位設計との優先順位と矛盾時の扱いは [設計文書 README](../README.md#正本と優先順位) に従う。本書内の SO は [State Ownership](../architecture/state-ownership.md) を指す。

## 1. 対象と非対象

### 1.1 本書が具体化するもの

- 何を永続化しなければならないか / 一時でよいか / 再構築可能派生か / 外部・credential-store かの分類。
- 保存単位（logical table / collection）の体系化と owner / durability / deletion / reconstruction の明示。
- どの property 同士が整合して保存・復旧されなければならないか（atomicity / ordering / durable-before-visible）。
- Host restart / crash 後にどの durable を読めばどの runtime を再構成できるか、自動再開してよいものとしてはいけないもの。
- Targeted Deletion / Backup / Restore と persistence schema の関係。
- concurrency 制御が成立するために persistence が持つべき revision / generation / expected value / operation / attempt / durable boundary。
- 上記から導かれる technology mapping（SQLite / sqlite-vec / filesystem / credential store）。

### 1.2 本書が決めないもの

- DB 製品固有の完全な `CREATE TABLE`（index, migration, vacuum, 暗号実装を含む）、製品固有 SQL 方言。
- crate / module 分割、process / thread 配置、IPC / wire format。
- concurrency mechanism（lock / MVCC / actor / queue / transaction protocol）、retry / timeout 値、scheduling algorithm、embedding / retrieval scoring。
- 各 Subsystem の完全な API 一覧。ただし「後続 interface で落としてはならない永続 property」は第11節で固定する。
- 暗号方式、archive 形式、署名方式。

### 1.3 用語

- **durable primary**: restart を跨いで canonical 現在値として参照される Host 正本。`correspondence-identity.md` §7.1 の persisted 対応に対応。
- **durable history**: 過去 revision / 原記録 / 試行 / Audit 等の履歴。append-only が自然なものと version chain が自然なものを区別する。
- **durable operation/recovery**: 全域操作・未完了・保留・再保存防止・不明等、crash を跨いで守る進行・条件。対応は失わず、本文は保持しない。
- **rebuildable derived**: durable primary から再構築できる派生。独立復元対象にしない。第二の正本にしない。
- **transient**: process / round / Client 一時表現。失ってよい。受理済み指示・作業記録・未伝達・不明・全域未完了は含めない。
- **external / credential-store**: ene live state ではない外部所有物と、秘密値・device-auth の分離保管。DB の durable には参照だけを持つ。device-auth は検証材料が非秘密でも backup から trust を復活させないためこの境界に置く。
- **semantic owner**: SO で定めた意味・通常変更・lifecycle 判断の引受先。persistence の table / record は owner にならない。
- **storage 共有**: 同じ SQLite / filesystem 技術を使うこと。ownership の統合を意味しない。

## 2. 原則

1. **persistence は semantic owner にならない。** table / record / file の存在・保存成功を、Task 達成・作用成功・許可・報告完了・全域完了の確定にしない。意味変更は各 owner が行い、persistence は対応の保持・照合可能性だけを支える。
2. **storage 上で近いことと ownership が同じことを混同しない。** 同じ SQLite file を共有しても table group ごとに owner を明示する。共有は atomicity / backup 整合のための mechanism 選択であり、意味の統合ではない。
3. **万能 table を作らない。** universal state / entity / revision / event log / generic JSON blob / generic KV を semantic schema の代替にしない。revision counter, generation counter, snapshot id, transaction id を全 domain で共有しない。
4. **contract から保存構造を導出する。** schema を先に決めて semantic contract を押し込まない。identity / revision / generation / correlation / boundary token の意味（correspondence-identity §4）を維持し、domain ID を統合せず、revision と generation を混同しない。
5. **必要な atomicity は最小にするが、壊れると意味が成立しない更新は保護する。** すべてを一つの global transaction にまとめない。一方で durable-before-visible, ordering, atomic compare が必要な境界は第7節で明示する。
6. **derived を第二の正本にしない。** embedding / index / cache / session / routing 派生物から現在状態・権限・帰属を復活させない。derived 消失は機能縮退であり、正本喪失ではない。
7. **本文非複製。** 対応の伝達・保持のために private 本文・Credential・削除対象本文を別保管しない。識別用の値であっても復元できるなら保護・消去対象である。

## 3. Durability 分類

分類名は本書の作業用であり、新しい統一 state 型ではない。各行の owner は SO / Step 12 の再掲であり変更しない。

### 3.1 分類定義

| 分類 | 意味 | restart 後の扱い |
|---|---|---|
| durable primary (D1) | canonical 現在値。Host 正本として参照される | そのまま正本として読む。stale 化したら現在性確認なしに利用しない |
| durable history (D2) | 過去 revision / 原記録 / 試行 / Audit。正確な引用・由来・訂正・検証の材料 | 追記順・revision 順で読む。現在値の代替にしない |
| durable operation/recovery (D3) | 未完了・保留・消去条件・復元操作・不明・未伝達報告状況等。継続して守る条件 | 未完了として再構成し、保留・再保存防止を維持する。完了・解除と誤認しない |
| rebuildable derived (R) | embedding / index / cache / session / 有効経路 / 表示集計 / routing 派生物等 | 失っても正本は失わない。再構築または縮退する。古い派生物で現在を復活させない |
| transient (T) | Client 入力途中・表示 timeline・audio buffer・VAD・Raw・候補・推論中 context・Agent 一時 context・実行中 buffer・MCP Apps 表示等 | 失ってよい。受理済み・作業記録・未伝達・不明・全域未完了は含めない |
| external / credential-store (E) | 外部 Workspace 実体・Provider/MCP 側状態・外部 Package 原本・export / backup copy・秘密値・Host device-auth store | DB の durable には参照だけを持つ。内容・秘密・device 検証材料を DB / Backup / Audit へ流さない |

### 3.2 状態 inventory と分類

| 意味上のまとまり（owner） | D1 durable primary | D2 durable history | D3 durable operation/recovery | R rebuildable derived | T transient | E external / credential-store |
|---|---|---|---|---|---|---|
| Character 静的構成・revision（Character） | 内部 Character 定義・現在 revision 一覧 | revision 別静的内容・差分提示内容・import provenance 受入対応 | import / 適用供給の中断時の未完了（旧 revision・旧適用を破壊しないための進行記録） | 適用可能部品の表示用派生・export 確認表示 | 編集中の未確定内容・表示 copy | 外部 Package 原本・外部制作 file・export 済み copy |
| Companion 同一性・適用関係（個体調整） | Companion lifecycle（Running/Stopped/Deleted  tombstone 最小）・現在適用関係（Companion→Character revision・部品・Owner 選択対応） | 適用関係の履歴・Companion 生成・適用の対応記録 | 停止・削除の未完了・新規禁止 hold（保全・消去と協調） | 適用結果の表示用派生 | 適用供給の一時 buffer | — |
| 会話 History・活動記録（個体調整） | —（現在値ではなく記録であるため D2 が正本） | History 原 record（参加者・文脈・時刻+tz）・非会話活動記録・evidence | 未完了の保持整理の進行（保全・消去と協調） | 報告用要約・由来説明・進捗表示 | Client 入力途中・表示 timeline・audio buffer | export 済み History copy |
| 未伝達・報告状況（個体調整） | 未伝達の必要内容・対象・元対応（Task 由来→Task record 参照、活動由来→活動 record 参照、元なし通知は必要範囲の活動 record） | 報告状況の遷移履歴（要否に応じて。少なくとも現在状況は D3 として保全） | 報告状況（Pending/Summarized/Presented/Unknown）・提示不明の保持 | 報告用要約（派生表現） | 表示 copy 送信・接続状態 | — |
| 進行中の意味判断（個体調整） | — | 残すべき発言・結果が History 等へ反映された後の記録 | —（進行中判断自体は永続化しない） | — | I-7 の進行中判断・推論・入出力の一時処理 | — |
| Task・委任・context・Workspace・Schedule（作業） | Task 現在（現在 TaskPurposeRef と目的本文・担当・進捗・待機・結果・未完了・次の判断）・委任対応（Task revision 前提・範囲・進捗・待機・停止・受領）・Workspace 関連付け・Schedule 設定（担当・内容・時刻条件・作成時 tz・初期入力） | Task revision 履歴・委任の対応履歴・Task context entry（採用 identity・由来・取得時点）・Schedule 発生対応（missed/Started/Cancelled + 各回 Task 対応）・Task 記録（終了≠削除） | 途中 Task の明示再開待ち・判断待ち・委任停止・Schedule missed の未完了・Task 削除 vs log 整理の進行 | Task 進捗表示・次回 Schedule 表示・由来説明 | W-2 の Agent 一時 context・推論作業領域・詳細 payload 全量 | 外部 Workspace 実体・案内 file・Skill・成果物（通常 file）・外部 Skill 原本 |
| Task 内部 copy・中間 file（作業） | 内部保持 copy の意味（由来・取得時点・用途）・中間 file の用途・期間・整理対象 | copy・中間 file の受入・整理の対応記録 | 整理待ち・保持方針との対応・targeted deletion 参加の未完了 | — | 実行中 buffer | 外部原本の現在値・成果物の外部実体 |
| Learning Summary・根拠（認識・学習） | Summary 本体（圧縮 evidence）・形成判断と根拠の対応（Summary→Memory/Skill/Relationship/State revision） | Summary の履歴・根拠関係の履歴・source 範囲参照 | 形成中処理の保留・消去参加の未完了 | embedding / index / query 派生・検索 score | 一時 reasoning・Raw | — |
| Memory・Skill・Relationship・State（認識・学習） | 各現在認識・有効 revision・scope・重要度・由来（Memory 主要知識、Skill 有効手順、Relationship 主体別解釈、State 一時/持続の区別） | 各過去 revision・変更経緯・利用根拠（誤訂正 vs 時間変化の区別を保つ） | 訂正・scope 変更の未完了・消去参加の未完了・再形成防止 hold | embedding / index / similarity / score・表示集計 | L の一時 buffer 的側面・検索中 context | — |
| Action 試行・作用・確定度（実行・拡張） | —（試行は履歴であり現在値ではない。確定度は D3 として保全） | Action attempt record（attempt・Task/委任対応・実対象・操作種別・依拠 Permission・段階・確定度・根拠対応） | 確定度 Unknown の粘着保持・hold・停止要求と停止結果・retry 前提（prior unknown 対応） | 作用報告の表示用派生 | 実行中 buffer・Tool 実行 buffer | 外部作用そのもの（外部所有）・外部 process 内部 |
| Rule・Permission・同意・禁止・上限（権限・制約） | Rule 本文・解釈・scope・現在 revision・Undo 対応・assignment 同意（Provider/model・送信先・data・用途・取扱い・費用・fallback 順序・Observer 専用含む）・device 許可・sandbox 外例外・保存禁止・非共有・cap 定義 | Rule revision 履歴・Permission 判断記録（生きた許可ではない）・同意変更履歴 | Owner 判断待ち・失効・停止・保留・消去・復元保留との照合結果（現在の利用可否は評価時に導出し、Allow copy を正本にしない）・新規禁止と停止要求の結合の未完了 | 解決済み割当経路・有効 Provider 経路・費用集計 | 推論中 context | — |
| 利用量・費用（推論 + 各利用 owner） | cap 定義（権限・制約）。利用事実は各 owner の原記録として D2 | 利用事実（報告・不明・処理中の別、consumer・用途・送信先対応） | 処理中・未報告・不明消費の保全（ゼロ化・リセットしない） | 費用集計・表示集計・cached token 表示 | 集計 cache（一時） | Provider 請求確定値（外部報告） |
| Credential（認証秘密） | 非秘密の用途・参照元・有効性・登録・更新・失効の対応（DB 側参照） | 登録・更新・失効の対応履歴（非秘密のみ）・認証失敗・再認証必要性の事実 | 再認証待ち・失効の未完了 | — | — | 秘密値本体（OS credential store 等の分離保管。DB / Backup / Audit / log / Debug へ流さない） |
| Provider 登録・能力・割当解決（推論） | 非秘密の登録・能力観測（最終観測は D1 だが現在性の確認なしに利用しない） | 能力観測の履歴・利用量の原記録 | 能力不足・接続失敗の未完了・fallback の未完了 | 解決済み経路・Prompt cache・Provider session・一時 context | 推論 session・cache・圧縮 context | Provider 側 session・cache・保有 copy（外部） |
| 接続・帰属・hint・復旧先（接続・存在） | 個体別帰属 record（state + active_client + presence generation）・hint・復旧先（非現在の参照）・最終接続管理 record | 帰属遷移 log・接続の観測事実の履歴 | 切替区間（旧/移行中/新/active なし/停止中/復旧待ち）の未完了・排他性未確認の保留 | 存在人数・routing 対象の導出値 | 現在接続の live 性（到達性）・切断検知の一時状態 | Client 固有の接続材料の秘密部分（Client / credential-store 側。DB へ流さない） |
| 入出力 round・提示（入出力・提示） | 一般設定（UI 言語・Body 位置/size/hide・Voice 一般設定）・Host 自動起動の選択（日常利用の意味。OS 適用結果と区別） | round→History 対応・提示状況の対応記録 | 提示不明の保持・移動区切りの未完了 | Body・Voice 出力・描画・表示集計 | round 進行・描画 frame・motion 位置・audio buffer・VAD・barge-in・Mute・device 利用状況・MCP Apps 表示 | OS の fullscreen・負荷・device 状態（外部現在事実） |
| 観測運用・候補・routing 派生（共有観測） | 観測運用設定（Client/全体 ON/Pause/OFF・頻度） | 対象・時機の判断対応記録 | 未完了 Capture・検知・delivery の保留・消去参加の未完了 | routing 用限定文脈（派生表現）・候補・検索派生物 | Raw・候補・routing 用 data・推論中 context | — |
| 保持方針・操作状況・backup 設定・Audit・Debug（保全・消去） | 保持方針・容量管理方針（既定 OFF）・backup 設定（保存先・独自 schedule・保持数・保護） | backup 作成結果・操作状況の履歴・Audit 追記順・保持 | Targeted Deletion / Restore / Reset / retention の操作状況（目的・対象・参加・影響・完了範囲・未完了・失敗・検証・保留）・消去条件・有効区間・完了境界・pending/unreachable/failed の区別 | 進捗表示・由来説明 | 一時 buffer | 作成済み backup copy（外部 copy として境界を保つ。live 正本ではない）・Owner 保存 backup・export copy |

補足：

- Client temporary state（入力途中・表示 timeline・audio buffer・未送信操作・Tool UI data）や Raw capture（Raw Observation / Raw Voice / 詳細 Tool payload / 内部推論 / chain-of-thought）は canonical persistent 化しない（固定 premise）。
- Backup copy は canonical ではない。明示 restore を経ずに live 正本として読み戻さない。
- Credential secret は通常 DB / Backup / Audit へ流さず、現在 credential-store 側で扱う（固定 premise）。
- Host 側 device-auth（pairing identity に対応する検証材料・現在 trust 範囲・失効）も E とし、非秘密の検証材料であっても backup へ含めず Restore で巻き戻さない（Group K）。DB の device 参照・許可記録だけでは認証・利用を成立させない。
- 派生物（embedding / index / query 派生 / Prompt cache / Provider session / 有効経路 / 次回表示 / 集計表示）は独立復元対象にしない。古い派生物から権限・状態・帰属を復活させない。

## 4. Logical persistence groups と schema  concretization

DB 製品固有 SQL は確定しない。以下は logical table / collection の提案であり、後続 interface 設計で同義の改名は許すが、owner 分離・revision / generation の区別・deletion / reconstruction の意味は維持すること。

### 4.1 技術共有と ownership 分離の方針

- Host durable（D1/D2/D3）は単一 SQLite file（例: `app.db`）に owner 別 table group として共存させる。共有理由は backup 整合（対象時点・参照対応）と、必要最小の cross-owner atomic read（§7）のためであり、ownership 統合のためではない。
- Derived（R）は別 SQLite file（例: `derived.db`、sqlite-vec を含む）または同一技術の別 group として、 primary を破壊せず削除・再構築できる配置にする。primary file の backup に derived を含めない。
- 内部保持 copy の blob 本体が大きい場合は filesystem（例: `internal_copies/`）に置き、DB 側は参照・由来・用途・削除 marker だけを持つ。blob を DB inline にする選択は許すが、消去の意味は変えない。内部 copy・中間 file の blob 本体は Task 内部 data として backup に含める（外部 Workspace 実体は収集しない）。
- Credential 秘密値は OS credential store 等の分離保管（DPAPI / libsecret / Keychain 等の抽象）に置き、DB 側は非秘密参照だけを持つ。Client 固有の接続材料の秘密部分も同様である。
- Portable full backup は filesystem 上の外部 copy（例: `*.ene-backup`）とし、暗号化選択可能・非暗号化時は private 説明を事前に行う。backup copy を live 正本にしない。

いずれも storage technology の共有・分離は logical persistence requirements から導いたものであり、semantic owner の統合・分離ではない。

### 4.2 Table group 一覧（owner 明示）

凡例：PK = primary key、CORR = correspondence relation（FK ではなく対応。参照先の意味 owner を移さない）、REV = revision field、GEN = generation field。

#### Group A — Character（owner: Character）

| logical table | PK | 主な field（REV/GEN/CORR 含む） | durability | deletion | reconstruction source |
|---|---|---|---|---|---|
| `character_identity` | `character_id` | 静的定義の存在、作成対応 | D1 | Character 削除で定義を除去。tombstone として `character_id` のみ残し内容は消去。既存 Companion の経験・記録へ cascade しない | backup / restore の復元対象。dangling は未解決として扱う |
| `character_revision` | `(character_id, character_revision)` | 静的部品・推奨 Skill 指定・import provenance 対応（CORR→`character_import`）、差分提示内容。REV=`character_revision` | D1+D2（現在一覧 + 履歴） | Character 削除で除去。targeted deletion で対象情報を復元できる内容を持つ場合のみ参加 | 現在 revision 一覧から供給。存在≠適用と区別する |
| `character_import` | `import_id` | CORR→対象 Character・revision、外部原本参照（E、所有ではない）、受入時点・validation 結果（実行許可ではない） | D2 | Character 削除で除去。targeted deletion は参加原則に従う | 受入対応の履歴として読む |

適用関係は Group B（個体調整）に置く。

#### Group B — Companion / History / 未伝達 / round 対応（owner: 個体調整。round の実際は入出力・提示）

| logical table | PK | 主な field | durability | deletion | reconstruction source |
|---|---|---|---|---|---|
| `companion` | `companion_id` | lifecycle `Running/Stopped/Deleted`、CORR→適用関係（`companion_applied_current`）、生成対応。Deleted は tombstone 最小（識別のみ残し私的 field は消去） | D1 | Companion 削除で個体固有 field を消去し tombstone 化。History・Task 記録へ cascade しない | restart 後に同一性・活動状態を再構成する起点 |
| `companion_applied_current` | `companion_id` | CORR→`(character_id, character_revision)`、適用部品群・`OwnerSelectionRef`（管理操作 identity + 時刻）、適用時点。REV=`character_revision` | D1 | Companion 削除で除去。Character 削除で dangling として未解決にし黙って置換しない | 現在適用を再構成する。未適用を更新済みにしない |
| `companion_applied_history` | `(companion_id, applied_seq)` | 過去適用の対応記録 | D2 | 同上 | 変更経緯として読む |
| `history_message` | `message_id` | CORR→`companion_id`・空間（一対一/グループ/Companion 間）・参加者・文脈、wallclock+tz、本文（D2 正本）。CORR→`round_id`・`presence_generation`・`restore_generation` | D2（append-only が自然） | 通常削除・retention で整理（形成済みへ cascade しない）。targeted deletion で該当本文を除去または復元不能化。本文保持のまま完了にしない | 正確な引用・未伝達の元記録・Summary source として読む |
| `activity_record` | `activity_id` | CORR→`companion_id`、種別・何を行い何を認識・報告したか、必要範囲の内容、CORR→Task/Action/Audit（各 owner を参照し独立更新しない） | D2 | 同上。結果説明に必要な範囲であることを消去拒否にしない | 由来説明・未伝達の元記録として読む |
| `undelivered` | `undelivered_id` | CORR→`companion_id`、CORR→source（`TaskRecord(task_id)` または `ActivityRecord(activity_id)`。第二の Task 正本にしない）、報告状況 `Pending/Summarized/Presented/Unknown`、CORR→`round_id`・`presence_generation`・`restore_generation` | D1+D3（必要内容 + 報告状況の保全） | Companion 削除で当該個体の管理を終えるが元 record は残す（SO 4.4）。targeted deletion で報告用要約・メモ中の対象本文を復元させない | restart 後に未伝達一覧を再構成し次 Client で要約報告する |
| `presence_attribution` は Group G に置く。round 進行自体は T であり、round 対応は `history_message` / `undelivered` / `action_attempt` の CORR として保持する。旧 round の入力・未提示を新 round へ付け替えない。 |

`companion` の Deleted tombstone は「個体が存在したこと」の historical attribution のためであり、私的 state の残存ではない。Summary を historical log へ分類し直して残さない。

#### Group C — Learning（owner: 認識・学習）

Memory / Skill / Relationship / State / Summary を一つの canonical / schema / revision model / retention へ潰さない。各概念で別 table とする。

| logical table | PK | 主な field | durability | deletion | reconstruction source |
|---|---|---|---|---|---|
| `summary` | `summary_id` | 圧縮 evidence、CORR→source 範囲（Conversation・Task 等の大まか範囲）、scope（個体固有/共有の識別）、形成時点。現在知識の正本にしない | D1+D2 | Companion 削除で個体固有 Summary を削除。共有は残存・参照不能を説明。targeted deletion で過去根拠も対象にし分離可能なら分離する | 形成・改訂の根拠として読む。逐語引用は History へ戻す |
| `summary_grounds_link` | `(summary_id, learning_id, learning_revision)` | CORR→形成先 Learning の `(id, revision)`。graph 全体を永続 object にしない | D2（CORR のみ。本文複製しない） | 同上。関係自体に対象情報が残る場合も消去へ参加させる | 根拠対応を辿るために読む |
| `memory_current` | `memory_id` | CORR→`companion_id` または Global、現在内容・時間的意味・重要度・scope、REV=`learning_revision`、CORR→根拠（Summary / source 範囲） | D1 | Companion 削除で Companion scope を削除（Global は残す）。targeted deletion は通常保持より優先する | restart 後に現在認識を再構成する |
| `memory_revision` | `(memory_id, learning_revision)` | 過去内容・scope・根拠・誤訂正 vs 時間変化の区別。REV=`learning_revision` | D2 | 同上。自動 cleanup は既定 OFF・明示 opt-in のみ | 訂正・由来説明として読む。現在値の代替にしない |
| `skill_current` / `skill_revision` | `skill_id` / `(skill_id, learning_revision)` | 有効 revision・由来・scope・原本対応・実行結果対応（未検証/成功/失敗の区別）。原本を破壊せず別 revision とする | D1+D2 | Companion 削除で Companion scope Skill と過去 revision を削除し自動 Global 化しない | 現在手順として読む。推奨＝有効切替え・実行許可にしない |
| `relationship_current` / `relationship_history` | `(subject_companion, peer)` / 同 + seq | 主体別現在解釈・保持過去・根拠。事実矛盾時は Memory 優先で再解釈する | D1+D2 | 主体・相手のいずれかの削除で削除する。共有 Summary 等まで一律削除しない | 現在解釈として読む。第二の Memory にしない |
| `companion_state` | `companion_id` | 一時状態と持続的傾向の区別・継続に要る状態・時間的意味・保持根拠。REV は全過去値の恒久 revision を要求しない | D1（+ 必要根拠の D2） | Companion 削除の対象。無関係な傾向まで一律初期化しない | restart 後に経過時間を解釈して再構成する。一時固定・不自然な初期化をしない |

Skill の scope default（Package 由来→Companion scope、単体 import→Owner 選択）は本 schema の初期値として扱い、意味判断自体は認識・学習に残る。

#### Group D — Task / 委任 / context / Workspace / Schedule（owner: 作業）

| logical table | PK | 主な field | durability | deletion | reconstruction source |
|---|---|---|---|---|---|
| `task` | `task_id` | 現在 REV=`task_revision`、現在 `TaskPurposeRef`・目的本文の現行値（steering 前提は現在 revision と `TaskPurposeRef` で表す）、担当、進捗・待機・完了・失敗・Cancel・結果・未完了・次の判断、CORR→`restore_generation`（保全・消去 slice で導入） | D1 | Task 削除で固有 Workspace 関連付けを削除。形成済み・外部成果物へ cascade しない | restart 後に保存済み進捗・既知作用・不明・未完了を示し明示再開待ちにする |
| `task_revision` | `(task_id, task_revision)` | 目的本文・`TaskPurposeRef`・採用指示・担当・委任前提の snapshot、steering 前後の区別。REV=`task_revision` | D2 | 同上 | 遅延結果の帰属（attempt→task revision→現在 Task）に使う |
| `delegation` | `delegation_id` | CORR→`(task_id, task_revision)` 前提・委任元 Companion・一時 Agent ephemeral id・scope 写し・進捗・待機・停止・受領 | D1+D2 | Agent 終了・担当削除で Task record を消さない | 委任の継続・停止・受領を再構成する。Agent 一時 context 消失を完了根拠にしない |
| `task_context_entry` | `entry_id` | CORR→`(task_id, task_revision)`、採用した目的・指示・材料・途中理解の identity 群・由来・取得時点。AU2 は採用目的の entry を持つ。用途は利用先が分岐する slice で導入する。有効性は item の採用 identity が現在有効な採用 identity と一致するかで解決する（revision 番号の一致では判定しない。`new_purpose: None` で引き継いだ目的は、採用 revision が現在 revision より前でも現在有効）。本文複製を要求しない | D1+D2 | Task 削除で固有分を削除。targeted deletion に参加する。Task 限り情報を Learning へ自動昇格しない | Task 判断・Observer 仲介（個体調整–作業協調経由）の材料として読む |
| `workspace_assoc` | `assoc_id` | CORR→`task_id`、外部 folder/file/source 参照（E、所有ではない）、利用条件・保存先・待機。作成時は関連付けを確定した場合のみ row を持ち、folder と保存先を保存する。利用条件・待機はそれらを解決する slice で追加する | D1 | Task 削除で関連付けを削除。外部実体へ cascade しない。backup は関連付けのみ含め実体を収集しない | 関連付けを再構成する |
| `internal_copy` | `copy_id` | CORR→`task_id`・元外部参照・由来・取得時点・用途、保管参照（DB inline または filesystem path）、削除 marker | D1 | Task 削除で整理。targeted deletion に参加する（外部所有を理由に除外しない） | 作業継続に必要な copy として読む。外部現在値と混同しない |
| `intermediate_file` | `file_id` | 用途・必要期間・整理対象・保管参照（外部に置いた場合も外部作用として Permission に従う） | D1（一時作業物だが Task 終了まで durable） | Task 終了または保持方針で整理する。永久成果物と混同しない | 整理対象として読む |
| `schedule` | `schedule_id` | 担当 Companion・実行内容・時刻条件・作成時 tz・停止等・初期 Workspace 入力、状態 | D1 | 担当削除で Schedule を削除し自動引継ぎしない。各回 Task 記録は残す | 将来回の通常判定として読む。作成依頼を token にしない |
| `schedule_occurrence` | `occurrence_id` | CORR→`schedule_id`、予定時刻・状態 `Missed/Started/Cancelled`・CORR→`started_task_id`（各回は新 Task） | D2 | missed を実行済みに書き換えない | missed 非補完・各回 Task 対応として読む |

Task record の lifetime を担当 Companion への参照が決めない。担当削除後も残 Task record は管理面から到達できる。

#### Group E — Action 試行・作用（owner: 実行・拡張）

| logical table | PK | 主な field | durability | deletion | reconstruction source |
|---|---|---|---|---|---|
| `action_attempt` | `attempt_id` | CORR→`task`（軽微な非 Task では NULL + 活動 record 対応）・委任・Workspace、解決後の実対象・操作種別（Read/Create/Edit/Delete/Execute を潰さない）・依拠 Permission 対応（`permission_evaluation_id`。生きた許可ではない）・段階（受付/開始/送信/把握）・確定度 `ConfirmedSuccess/ConfirmedFailure/Unknown`・根拠対応・hold・CORR→`presence_generation`・`restore_generation`・CORR→`prior_attempt_id`（retry の元不明対応） | D2+D3（append-only 試行 + Unknown の粘着保持） | 内部削除は作用記録の消去であり外部 rollback ではない。targeted deletion で対象本文を除去し事実と両立させる。確定度を消去理由で書き換えない | restart 後に Unknown を Unknown のまま再構成し重複 risk 付き Owner 判断へ戻す。retry は新 `attempt_id` とする |

retry・再送・fallback・再委任は新しい `attempt_id` とする。重複し得る再送を「同じ試行の継続」として除外しない。

#### Group F — 権限・制約・利用量（owner: 権限・制約。利用事実の原記録は各利用 owner）

| logical table | PK | 主な field | durability | deletion | reconstruction source |
|---|---|---|---|---|---|
| `rule` | `rule_id` | 現在 REV=`rule_revision`、本文・解釈・scope・Undo 対応 | D1 | — | 現在方針として読む。trigger にしない |
| `rule_revision` | `(rule_id, rule_revision)` | 過去本文・解釈・scope | D2 | — | 変更経緯として読む |
| `permission_evaluation` | `evaluation_id` | 実行主体＋委任 chain・Task＋Workspace 範囲・目的・実対象＋操作＋送信先・利用 data＋目的・費用・risk・依拠 Owner 意図・Rule の対応、判断（Allow/Deny/Ask/Wait）、評価時点の boundary snapshot（rule revision・consent・device・cap・失効・停止・保留・消去・復元条件の写し） | D2（判断記録。生きた許可ではない） | targeted deletion に参加する。保存された Allow を現在許可として復活させない | Audit・説明・対応付けとして読む。開始前に現在条件と再照合する |
| `assignment_consent` | `assignment_id` | Capability・Provider/model・送信先・data・用途・取扱い・費用範囲・Host 既定/Companion override/Observer 専用・fallback 順序、REV（同意 revision）・CORR→`restore_generation` | D1 | — | 現在同意として読む。登録・認証成功で成立させない |
| `device_permission` | `(device_id, function)` | 許可機能の記録・CORR→pairing identity。失効表示は現在 E 側 trust との照合で導出する | D1 | — | 復元対象の許可記録。現在 E 側の同一 pairing・trust 範囲・非失効と照合してのみ利用する。認証・失効の正本にはしない |
| `sandbox_exception` | `exception_id` | 特定 Local MCP・command・由来・既知 access・risk・失う強制境界・保存・失効・重要変更時の再確認状態。Plugin へ流用しない | D1 | — | 隔離例外として読む。包括承認にしない |
| `control_constraint` | `constraint_id` | Owner の明示的な保存禁止・非共有の適用対象・範囲。Learning scope 意味と区別する | D1 | — | 各利用箇所で迂回不能に適用する |
| `cap_limit` | `cap_id` | Provider 別・全体の費用・資源・反復・並列等の上限定義 | D1 | — | 現在可否の上限として読む |
| `usage_fact_provider`（owner: 推論） | `usage_id` | consumer（Companion/Observer 専用/Task 等）・報告/不明/処理中の別・量・CORR→attempt/task/assignment・報告時点 | D2+D3 | Companion 削除で費用 log を削除・使用量をリセットしない。targeted deletion で該当情報を参加させる（消去を消費リセットにしない） | cap 評価の入力として読む。未報告・処理中・不明をゼロにしない |
| `usage_fact_task`（owner: 作業） / `usage_fact_action`（owner: 実行・拡張） / `usage_fact_storage`（owner: 保全・消去） | `usage_id` | 同上（委任稼働・Action 実行・保存量等の各事実） | D2+D3 | 同上 | 同上 |

「現在の利用可否」は保存された行ではなく、保存条件＋活動状態＋委任＋帰属＋利用量＋失効＋保留等を照合した評価時に導出する。

#### Group G — 接続・帰属（owner: 接続・存在。pairing・device 許可の意味は権限・制約、秘密は認証秘密）

| logical table | PK | 主な field | durability | deletion | reconstruction source |
|---|---|---|---|---|---|
| `presence_attribution` | `companion_id` | state `Present/NoActive/InTransition/Stopped/RecoveryWait`・`active_client`・GEN=`presence_generation`（個体ごとの帰属 lifecycle 順序）。`active_client` は再起動前の帰属記録であり、起動直後は `RecoveryWait` として解釈し現接続・許可・排他性の確認を経て `Present/NoActive` へ確定する | D1+D3 | Stopped は帰属解除として保持する（hint と区別） | restart 後に現接続・許可・排他性の確認と合わせて presence を再構成する |
| `presence_transition_log` | `transition_seq` | CORR→`companion_id`、旧 state/新 state・旧 GEN/新 GEN・理由（Owner 呼出し・事前指示・自発判断・`DisconnectFallback`・Host restart 限定の `ReconnectRecovery`）・時点 | D2 | — | stale 診断・Audit として読む。authority ではない |
| `device_ref` | `device_id` | descriptor・CORR→pairing identity・`DeviceWireId` 対応（非秘密表示参照のみ） | D1 | 全データ Reset で削除 | 表示・対応解決用。検証材料は Group K（E）であり、行の復元だけで pairing を成立させない |
| `client_last_connection` | `client_id` | 最終接続管理 record・CORR→device / incarnation / connection・機能利用可能性・`transport_class`（SameMachine / Remote）の最終観測。分類は IPC §10.1 の Host transport adapter が確定 | D1 | — | 管理記録として読む。現在接続として再成立させない。fallback 候補判定では live connection の分類・認証・device 許可・排他性を再照合する |
| `relocation_hint` | `companion_id` | `last_client`・`recovery_destination`（非現在の参照） | D1 | — | 復旧先・再配置候補として読む。記録だけで presence にしない |

現在接続（到達性）は T であり、古い保存値から再成立させない。通常切断確定時は `ene-presence` が利用可能な Host PC Client（SameMachine）へ SD-Presence / AU7 で fallback し、なければ `NoActive` とする。Host 側 Client を自動起動せず、切断 Client の再接続だけで帰属を戻さない。Host restart に限り、Running presence は現接続・許可・排他性の確認ができれば復元前 Client へ自動復元し、できなければ active なしにする。Stopped に移動・復旧しない。

#### Group H — 入出力・提示 / 観測設定（owner: 入出力・提示 / 共有観測）

| logical table | PK | 主な field | durability | deletion | reconstruction source |
|---|---|---|---|---|---|
| `io_general_settings` | `settings_scope`（typed。generic KV にしない） | UI 言語・Body 位置/size/hide・Voice 一般設定等の typed field | D1 | Companion 固有分は個体削除範囲へ対応付ける | 一般設定として読む。未確定編集・描画位置と区別する |
| `host_autostart_choice` | `singleton` | Owner の起動・日常利用選択・選択時点・説明対応。OS 適用結果（実行・拡張の作用事実）と区別する | D1 | — | 選択として読む。Task 再開・restore 有効化・Host 側 Client 自動起動を導かない |
| `observer_settings` | `(scope_client_or_global)` | ON/Pause/OFF・Client ごと頻度 | D1 | — | 対象・時機の入力として読む。設定 ON＝実行中と表示しない |
| `spontaneity_settings` | `companion_id` | 雑談・通知・内部調査・交流の OFF 含む頻度・上限・未応答抑制 | D1 | Companion 削除範囲に従う | 自発性判断の入力として読む |

実効的な可否・時機・存在人数・routing 対象は導出（R/T）であり durable primary にしない。round の実際・提示状況の対応記録は Group B の CORR として保持する。

#### Group I — Provider 登録・観測（owner: 推論。割当同意は権限・制約）

| logical table | PK | 主な field | durability | deletion | reconstruction source |
|---|---|---|---|---|---|
| `provider_registration` | `provider_id` | 非秘密の接続先・protocol・model 等・能力の最終観測 | D1 | — | 解決の入力として読む。登録＝利用可にしない |
| `provider_capability_log` | `(provider_id, observed_at_seq)` | 能力・利用可能性の観測履歴 | D2 | — | 鮮度確認の材料として読む。古い観測を現在能力にしない |

解決済み経路・Prompt cache・Provider session は R/T であり独立復元対象にしない。

#### Group J — 保全・消去（owner: 保全・消去。元事実の意味は発生元）

| logical table | PK | 主な field | durability | deletion | reconstruction source |
|---|---|---|---|---|---|
| `retention_policy` | `policy_id` | History/log/Audit 等の対象・期間・手動範囲・影響 | D1 | — | 保持整理の入力として読む。既定で自動削除しない |
| `capacity_policy` | `policy_id` | Learning revision・Summary 等の opt-in retention（既定 OFF）・範囲・影響 | D1 | — | 明示 opt-in 時のみ適用する |
| `backup_setting` | `singleton` | 保存先・backup 独自 schedule・保持数・保護選択 | D1 | Task Schedule とは別 | backup 作成の入力として読む |
| `backup_point` | `backup_point_id` | 対象時点・参照対応・除外（secret・外部実体）・未完了状況・結果・失敗・保管参照（filesystem path）・暗号化 flag | D2 | 内部削除で Owner 保存 copy も消えたとしない | restore 選択・説明として読む。各部 copy 成功だけを成功にしない |
| `deletion_operation` | `operation_id` | GEN=`deletion_sweep_generation`、目的・対象記述（機械的条件は検索用 token として操作期間だけ保持する。全域完了へ遷移する前に token を除去または復元不能化し、完了記録・Audit へ本文を残さない。未完了の間は backup に復旧可能な対応として含めてよいが、token が復元可能な状態を全域完了として保存しない）・意味的条件・影響・除外・要確認対応・有効区間（開始〜検証完了）・状態（進行中/局所完了/finalizing/pending/unreachable/failed/全域完了） | D3 | 完了後も事実（目的・範囲・影響・除外・確認・結果）は Audit として残し本文は残さない | restart 後に未完了・保留・再保存防止を再構成する。`finalizing` または token が復元可能なら全域完了として扱わない |
| `deletion_participant` | `(operation_id, participant_owner)` | 局所処理・検証・未完了・失敗・未確認範囲・報告時点 | D3 | 同上 | 全域完了の集約として読む。局所返却で hold を解除しない |
| `erasure_condition` | `(operation_id, deletion_sweep_generation)` | 対象範囲の利用・再保存を制限するための条件・有効区間・完了境界。 ingestion 時の照合用 index | D3 | 全域完了の確定前に区間を閉じ、復元可能な検索 token を除去または復元不能化する。完了後の Owner 新規提供は新 Experience として区別する | 各受入・保存先が進行中・finalizing の消去条件として読む。token 消去未完了なら hold を維持する |
| `restore_operation` | `restore_id` | CORR→`backup_point_id`、GEN=`restore_generation`、受理・説明・隔離・照合・置換・保留・一括有効化対応・状態 | D3 | 完了後も事実は Audit へ追記し本文・秘密は残さない | restart 後に旧 live 分離・保留を再構成する |
| `restore_generation_state` | `singleton` | 現在 `restore_generation`（成功した Restore ごとの順序） | D1 | — | 復元 vs 旧 live の区別・stale 判定として読む |
| `reset_operation` | `reset_id` | 種別（設定/全データ）・列挙・強い確認・範囲・状態。クラッシュ中の継続のための進行 marker（filesystem marker と併用し DB 削除後も継続できる） | D3 | 全データ Reset 完了後は Host 内部 data と共に除去される（外部 Workspace・保存 backup は残す） | restart 後に Reset の継続・完了を再構成する |
| `audit_log` | `audit_seq`（audit scope の順序。global state version ではない） | 発生元・事実種別・非秘密・非対象本文の記述・CORR→対象 identity（task/attempt/operation/restore 等）・追記時点 | D2（append-only が自然） | 通常保持管理と targeted deletion へ参加する（残った対象情報を除去）。追記順を発生順・許可正本・再生入力にしない | 説明・検証として読む。本文別保管庫にしない |
| `debug_capture` | `capture_id` | 明示対象・内容・有効期間・停止・削除・状態 | D3（短期・明示有効化） | 期限・停止・削除で除去する。Credential を含めない | 診断として読む。通常保存の理由にしない |

`audit_seq` は Audit 追記順の確認のための scope 内順序であり、全 state 共通の snapshot id・transaction id・global version counter ではない。

#### Group K — Credential 参照・device-auth（保管 owner: 認証秘密。device trust の意味は権限・制約）

| logical table | PK | 主な field | durability | deletion | reconstruction source |
|---|---|---|---|---|---|
| `credential_ref` | `credential_id` | 用途・参照元（非秘密）・有効性・登録・更新・失効の対応。秘密値を含めない。非秘密参照は backup の復元対象に含めるが、秘密値（E）は除外・維持し、復元後に現在の store と照合する | D1 | 全データ Reset で削除する。設定 Reset で維持する。Companion/Task 参照で全体へ cascade させない | 現在 store 照合・再認証要求として読む |
| 秘密値本体 | `credential_id`（credential-store 側 key） | 値・用途限定利用 | E（OS credential store 等） | 失効・更新は別操作。内部露出 copy の消去と外部失効を混同しない | 認証用途に限定して利用する。model context・Tool argument・通常 result・UI・Learning・History・Task 結果・log・Audit・Debug・backup へ流さない |
| Host device-auth record | pairing identity（store 側 key） | device 対応・Host 側所有証明検証材料（公開鍵等の非秘密材料も含む）・現在 trust 範囲（許可機能の上限）・有効/失効状態 | E（credential-store の device-auth 用途。Provider/MCP Credential と用途分離） | device 失効で検証材料を無効化・削除。機能失効は現在 trust 範囲にも適用。全データ Reset で trust・材料を削除、設定 Reset では維持 | backup 除外・Restore 非置換。E 側の有効材料なしには auth を拒否。DB の許可と現在 trust 範囲の両方を満たす機能だけ利用できる |

Client 固有の接続材料の秘密部分も同様に E とし、DB 側は非秘密の用途参照だけを持つ。

device-auth の保護・検証材料保持は `ene-credential`、trust / 許可変更・失効の意味判断は `ene-permission`、現在接続・帰属への適用は `ene-presence` に残す。E 側 trust は権限・制約が決めた現在範囲の保持であり、認証秘密の独立許可判断ではない。既存の premise 供給・Host 媒介で接続し、owner 間の具体 crate 依存を追加しない。変更の完了は E 側 durable 更新と DB 側記録の対応が揃ってから返し、部分失敗では対象の利用を保留して再評価する。失効完了を DB flag だけで返さない。再 pairing は新 identity と Host PC 上の trusted first-party management surface の最終確認（IPC §18）を必要とし、旧 DB 行・旧材料を再有効化しない。

### 4.3 Rust persistence-facing type（例示）

コンパイル対象の code ではなく、後続 interface で同義の改名は許すが型分離と field 意味は維持すること。`correspondence-identity.md` §5 の newtype 方針に従う。

```rust
struct TaskRow {
    task: TaskId,
    revision: TaskRevision,              // D1 現在 + D2 履歴の対応
    restore_generation: RestoreGeneration, // 復元跨ぎ参照の世代タグ（保全・消去 slice で導入）
    purpose: TaskPurposeRef,             // 現在採用されている目的（steering 前後の区別）
    purpose_text: TaskPurpose,           // 現在 revision の snapshot
    assignee: AssigneeRef,
    progress: TaskProgress,              // 進捗・待機・完了・失敗・Cancel・結果・未完了・次の判断
    delegation_scope: DelegationScopeRef,
}

struct ActionAttemptRow {
    attempt: ActionAttemptId,
    task: Option<TaskRef>,               // Task revision 前提を含む。軽微 Action では None
    prior_unknown: Option<ActionAttemptId>, // retry の元不明対応。新試行の証明
    real_target: RealTargetRef,
    operation: OperationKind,
    relied_evaluation: PermissionEvaluationId, // 生きた許可ではない
    certainty: ActionCertainty,          // Unknown 粘着。owner の新 evidence でのみ更新
    presence_generation: PresenceGeneration,
    restore_generation: RestoreGeneration,
}

struct PresenceRow {
    companion: CompanionId,
    state: PresenceState,
    active_client: Option<ClientId>,
    generation: PresenceGeneration,      // 単調。値の新旧ではなく区間の識別
}

struct DeletionOperationRow {
    operation: DeletionOperationId,
    sweep: DeletionSweepGeneration,
    // 機械的条件の検索 token は操作期間だけ保持する。
    // 全域完了を durable に確定する前に除去または復元不能化し、
    // 完了記録・Audit へ対象本文を残さない。
    mechanical_search: Option<SealedSearchToken>, // 要暗号化・操作期間限定
    semantic_hint: Option<SemanticHintRef>,       // 完全性を保証しない
    valid_interval: ErasureInterval,
    status: OperationStatus,             // 進行中/局所完了/finalizing/pending/unreachable/failed/全域完了を潰さない
}

struct RestoreOperationRow {
    restore: RestoreId,
    backup_point: BackupPointId,
    generation: RestoreGeneration,       // 成功ごとに前進。旧 live 分離用
    status: OperationStatus,
    hold: HoldConditionRef,
}
```

`SealedSearchToken` の実装（暗号化・hash・平文の扱い）は固定しない。固定するのは「完了記録・Audit・説明へ対象本文を戻さない」「全域完了を durable に確定する前に検索 token を除去または復元不能化する」「token が復元可能な間は operation / hold を未完了として復旧できる」「token 保持中も通常 model context・他個体・外部送信へ渡す権限が生じない」ことである。

### 4.4 所有・durability matrix（要約）

| storage technology | 入る group | owner は統合されるか | backup に含めるか |
|---|---|---|---|
| Host SQLite `app.db`（durable） | A〜K の D1/D2/D3（秘密値・blob 本体・derived を除く） | いいえ。table group ごとに owner を明示する。transaction 共有は mechanism であり ownership ではない | はい（D1/D2/D3 の対応を復旧可能な形で。secret・外部実体・derived・transient を除く） |
| Host SQLite `derived.db` / 同技術の derived group（sqlite-vec 含む） | R（embedding / index / score / cache / 有効経路 / 集計） | いいえ。元 state の owner が対応・利用範囲を説明する | いいえ。再構築可能なので含めない |
| filesystem `internal_copies/` | D1 の blob 本体（内部 copy・中間 file） | いいえ。意味は作業等に残る | 内容を含める（Task 内部 data。外部 Workspace 実体は収集しない） |
| credential store（OS 分離） | E の秘密値・Client 接続材料の秘密部分・Host device-auth record（非秘密検証材料も含む） | 保管・保護は認証秘密。device trust の意味判断は権限・制約のまま | いいえ。除外し Restore で巻き戻さない |
| filesystem `*.ene-backup` | portable full backup copy（外部 copy） | いいえ。live 正本ではない | 自身が backup である。内部削除で copy も消えたとしない |
| Client / Provider / MCP / 外部 file | T / R の一時・派生・外部所有 | いいえ。Client・Provider・MCP を正本・owner にしない | いいえ（外部 copy 消去を内部完了に含めない） |

## 5. Derived data の扱い

性能目的の derived を第二の正本にしない。

| derived | primary（再構築元） | durable primary として保存するか | invalidation / revision relation | 備考 |
|---|---|---|---|---|
| embedding / search index / similarity / retrieval score | 認識・学習等の検索対象（Memory・Summary・History 等） | いいえ（R） | 対象の `(id, revision, scope, 利用制限)` + `restore_generation` + 消去条件 `(operation, sweep, valid_interval)`。hit を理由に source 参照・制約確認を省かない | 派生物がなくても正本は失われない。訂正・scope 変更・targeted deletion へ参加する。古い index から権限・状態を復活させない |
| Prompt cache / Provider session / Provider 向け一時 context | 論理的 context（用途・選択方針）+ 現在 assignment・同意 | いいえ（R/T） | 用途・data・送信先・同意・費用・保留・帰属・消去条件。hit/miss・期限切れで論理 context・権限・永続化契約を変えない | Provider 側にしか継続状態が残らない構造を作らない。現在使えない情報を含む session を再利用しない |
| 有効 Provider 経路・次回 Schedule 表示・費用集計・負荷表示 | 割当同意・時刻条件・利用事実 | いいえ（R） | 同意 revision・時刻条件・利用事実の対応。古い派生値を独立正本として編集しない | 導出値であり保存条件・利用可能額の正本ではない |
| Task 進捗表示・由来説明・報告用要約・Body・Voice 出力 | Task 記録・作用確定度・Memory 等の domain 正本 | いいえ（R） | 元 record の `(id, revision)` + 現在条件。要約・演出で確定度を強めない | 表現の成功・失敗と元の成功を混同しない |
| Observer 限定 routing 文脈 | History・個体文脈（個体調整）・Memory・Learning（認識・学習）・Task context（作業。個体調整–作業協調経由） | いいえ（R/T。一時的な派生表現） | 元情報・対象 Companion・用途・制約の対応 + `presence_generation` + 選択時前提 + 消去・失効条件。scope 変更・同意失効・消去を生成済み要約・処理中結果にも適用する | 新正本・新 scope・包括共有にしない。混合生成文しかなく分離を確認できなければ個体へ渡さない |
| 解決済み割当経路 | 登録・能力観測（推論）+ 割当同意（権限・制約） | いいえ（R） | 同意・登録の現在性。設定・同意変更後も以前の選択を有効とする根拠にしない | 独立した利用可能 assignment の正本にしない |

derived の生成・保持・破棄を行う責務が、元 state との対応と利用範囲を説明し、訂正・scope 変更・targeted deletion へ参加する。具体的な無効化・再計算方式は固定しない。

## 6. Recovery — restart / crash 後に何を読んで何を再構成するか

### 6.1 Recovery 原則

1. **保存した派生物を材料にできても、旧要求の実利用許可・旧実行をそのまま再開しない。**
2. **途中 Task は明示再開待ち、Schedule は将来回の通常判定、Running presence は元 Client への復旧**というそれぞれの既存条件に従う。通常再起動を restore のような一律の復元内容確認へ拡大しない。
3. **不明は不明のまま、未完了は未完了のまま、保留は保留のまま**再構成する。再起動・再接続・restore を完了・解除・成功の根拠にしない。
4. **復旧先・hint・Client 表示・過去帰属・Provider 残存だけで presence・許可・実行を成立させない。** 現在の接続・許可・排他性を確認できて初めて成立する。
5. **復元成立と実行再有効化は別々に確認する。** 復元成立だけでは自動処理を開始しない。

### 6.2 Recovery matrix

| 対象 | 読む durable（Host） | 再構成する runtime | 自動実行してよいか |
|---|---|---|---|
| Running Companion の presence | `presence_attribution` + `relocation_hint`/`client_last_connection` + 現接続・許可・排他性の live 確認 | 現在帰属（Present / NoActive / RecoveryWait）。元 Client が利用可能なら自動復元し、できなければ active なしで待つ | presence 復旧のみ自動でよい。別 Client への無条件移動・Stopped への適用・Host 側 Client 自動起動はしない。Task・Action の再開権限にしない |
| Stopped Companion | `companion`（Stopped）+ `presence_attribution`（Stopped） | presence なし・Observer 人数外・routing 対象外。hint は再配置候補のまま | 自動復旧しない。接続回復だけで Resume・再配置しない |
| 現在 presence attribution | `presence_attribution`（state + generation）+ `presence_transition_log` | 切替区間の区別（旧/移行中/新/active なし/停止中/復旧待ち）。移行中に crash したら active なし起点で扱い新旧いずれも新規開始しない | 切替の自動継続をしない。旧 in-flight は安全な区切りまで、旧作用の別 Client 自動継続をしない |
| Task / Schedule | `task` + `task_revision` + `task_context_entry` + `workspace_assoc` + `schedule` + `schedule_occurrence` + `restore_generation_state` | 保存済み進捗・既知作用・不明・未完了・判断待ち・missed/Started の対応。次回表示は保存条件＋現在日時・tz から導出する | 自動再開しない。明示再開と現在条件（担当・Permission・同意・cap・Workspace・Client 依存・不明 risk）を必要とする。停止中の回は missed のまま自動補完しない |
| 委任された作業 | `delegation` + `task` | 委任範囲・進捗・待機・停止・受領の対応。Agent 一時 context は失われたものとして扱う | Agent を自動再起動しない。Task 記録が残るだけで Agent を再起動しない。新委任は新 Task revision 前提で開始する |
| Unknown Action | `action_attempt`（`Unknown` + 根拠対応・hold + `prior_attempt` 対応 + generation タグ） | Unknown の保持・既知作用の説明・重複 risk の提示 | 自動再実行・replay しない。不明試行の再実行は重複 risk を示した Owner 判断を必要とし、新 `attempt_id` とする。確認済み失敗と不明を同じ retry 経路へ潰さない |
| 未伝達（undelivered） | `undelivered` + `history_message` / `task`（元 record）+ 報告状況 | 未伝達一覧・要約報告材料。次 Client で現在の結果・利用制限・削除状況へ照合して報告する | 自動報告済みにしない。接続・表示 copy 送信・Task 完了だけで報告済みにしない。提示不明を保持する |
| 削除 operation | `deletion_operation` + `deletion_participant` + `erasure_condition` | 未完了範囲・必要な保留・再保存防止・pending/unreachable/failed/finalizing の区別。検索 token が復元可能なら finalization 未完了として扱う | 自動完了・自動解除しない。検証と最終消去を継続する。局所結果返却で hold を解除しない。検索 token の除去・復元不能化前に全域完了へ遷移しない。完了後に削除前根拠だけによる再形成・遅延再保存を防ぐ |
| 復元 operation | `restore_operation` + `restore_generation_state` + `backup_point` | 旧 live 分離（`restore_generation`）・受理・説明・保留・一括有効化の対応。置換成立前は復元前正常が正本、成立後は復元内容が正本 | 置換成立前は旧正常を破壊しない。成立後も自動処理を開始しない。部分置換を新正本にしない。再起動で保留を解除しない |
| retention / reset 等の全域操作 | `retention_policy` + `capacity_policy` + `reset_operation`（+ filesystem marker）+ `deletion_operation` / `restore_operation` | 未完了・保留・再保存防止の維持。設定 Reset の保護対象と全データ Reset の外部除外の区別 | 未完了を完了としない。全データ Reset の途中で再起動しても旧処理・一時 copy から復活させない。到達不能 Client の物理消去を確認済みにしない |
| 現在 Character 適用 revision | `character_revision` + `companion_applied_current` + `OwnerSelectionRef` | 現在適用関係。未適用 revision は未適用のまま | 自動適用しない。Package 更新を成長の初期化にしない。適用禁止種別を authority にしない |
| Learning 現在・revision | `memory_current` + `memory_revision` + `skill_current`/`skill_revision` + `relationship_current`/`history` + `companion_state` + `summary` + `summary_grounds_link` | 現在認識・過去 revision・根拠・誤訂正 vs 時間変化の区別・scope | 遅延形成が現在を無条件上書きしない。到着順を根拠の新旧にしない。古い根拠だけによる自動再形成をしない |

Host shutdown でも必要な進捗・作用不明・未伝達・全域操作の未完了を保全し、外部作用が Host と同時に消えると推定しない。一時 buffer の消失は成功・完了の根拠にしない。

再起動時、および Agent 停止で所有 in-flight を失った `Reserved` は、当該利用 owner の再評価経路が元の reservation・利用対応を読み、IB K-G `CommitUsageCommand(actual = 不明)` で `Committed / Unknown` に確定する（CCT §9.2）。release・ゼロ化せず、cap 集計に引き続き含める。不明消費と孤立理由を費用管理面で報告値と区別して示し、安全継続不能なら停止・Owner 判断待ちとする。具体的な Owner の扱い・表示方式はこの非ゼロ化契約内の Freedom とする。

### 6.3 再開してよいもの / 復旧するだけで自動実行してはいけないもの

- **自動でよい**: Running presence の元 Client への復旧（確認できた場合）、Host 完結の許可済み Task・Schedule・保存の継続（active なしでも継続できる範囲）、将来 Schedule 回の通常判定、未完了消去・復旧の保留の維持（守るための継続）。
- **復旧するだけで自動実行してはいけない**: 途中 Task の実行再開（明示再開待ち）、委任 Agent の再起動、Unknown Action の再実行・replay、Schedule missed の補完、復元後の Task・Schedule・外部接続の自動処理（復元後保留＋一括有効化待ち）、停止中個体の再開・再配置、Client 依存の対話・操作の新規開始（現在帰属・許可の確認なしには開始しない）、削除前根拠だけによる Learning 再形成、古い判定・解決済み経路だけによる新規利用。

## 7. Update and consistency boundaries

すべてを一つの global transaction へまとめない。一方で途中で壊れると意味が成立しない更新については atomicity / ordering / durable-before-visible を明確にする。具体的な transaction library・lock 方式は後続でもよい。共有 SQLite transaction は mechanism であり ownership の統合ではない。

### 7.1 Atomicity / ordering 要件表

| 更新 | 整合していなければならない property | 要求する性質 | 備考（owner を統合しないための分離） |
|---|---|---|---|
| Conversation / History 記録と Learning 形成 | History 原 record・活動 record・Summary・Memory revision・根拠対応・scope・保存禁止・非共有・消去条件 | History append と未伝達登録（該当時）は同一 durable transaction で原子にする（§7.2 AU1）。Learning 形成は別 transaction とし、History durable 後に認識・学習が現在認識・根拠へ照合して採否を決める。到着順を根拠の新旧にしない | 会話受付・History・Learning 更新完了を同一条件にしない。全状態の同時更新を要求しない |
| Task / delegation / steering | Task 現在・Task revision・Task context entry・Workspace 関連付け・委任 scope・steering 前提・Permission 現在条件 | Task 作成時は task + task_revision + 初期 task_context_entry を原子にし、Workspace 関連付けを確定した場合は workspace_assoc を含める。commit 前は委任・実行から不可視にする（durable-before-visible）。steering は新 revision + 新 context の原子 forward とし、旧目的の結果を新目的に自動採用しない。委任は `expected_task_revision` の atomic compare を満たして作成する | Task 達成判断（作業）と作用確定度（実行・拡張）と許可確定（権限・制約）を同一 transaction にしない。attempt 確定後に Task が別途読み取って達成を更新する |
| Action attempt / effect / outcome | attempt・Task revision 前提・委任 scope・実対象・操作種別・依拠 Permission・段階・確定度・根拠・hold・presence/restore generation | attempt insert は Task revision 前提・委任有効性・実対象解決・現在許可の照合を満たして原子にする。確定度 `Unknown→Confirmed` は新 evidence との原子更新とし、owner 以外は独立更新しない。retry は `prior_attempt` 対応付きの新 row とし旧 row を上書きしない | Task 側の確定度独立更新をしない。Agent 申告を証拠にしない。Task 達成・報告は別受入とする |
| Permission / usage / cap | Rule revision・同意 revision・device・cap 定義・利用事実（報告/不明/処理中の別）・失効・停止・保留 | 評価時は保存 Allow・委任時 copy・事前判定・復元 Rule・context 内許可文・cache 判定を現在許可として再利用しない。並列消費は同一 SQLite transaction 内で利用事実の atomic insert + cap 照合を行い、同一残額の独立使い切りを許さない（mechanism 共有であり owner 統合ではない）。処理中・不明をゼロにしない | 利用事実の原記録は各 owner に残し、権限・制約は可否だけを管理する。Task・推論側に独立許可・使用実績の正本を作らない |
| presence attribution / generation | 帰属 state・active_client・presence generation・現接続・許可・排他性・hint・復旧先 | 帰属切替は `expected_generation + expected_state` の atomic compare による `旧→移行中→新` の durable 遷移とし、新旧いずれも新規開始しない区間を保つ。hint・復旧先の更新と帰属成立を同一視しない。現在接続を古い保存値から再成立させない | 帰属成立（接続・存在）と移動必要性（個体調整）と許可確定（権限・制約）を同一更新にしない。live 到達性は DB 外の確認であり DB atomic に含めない |
| undelivered 登録 / delivery 確定 | History / Task 元 record・未伝達必要内容・報告状況・提示状況・帰属・消去条件 | 会話由来の登録は History append と原子にする。Task 由来の登録は Task 結果 durable 後の別 transaction で `undelivered` を原子に登録し、Task durable→未伝達可視の順序を保つ（Task と未伝達を単一 transaction にしない。共有 SQLite transaction は mechanism 共有として許すが、Task 達成と報告管理の owner を統合しない）。delivery 確定（Presented）は実際の提示確認（入出力・提示→個体調整）を受けてから durable 更新し、送信だけで確定しない（durable-after-confirmed） | 接続・表示 copy 送信・Task 完了を報告完了にしない。報告済みを承認・再開にしない |
| Targeted Deletion 進行 | 削除 operation・sweep・消去条件・有効区間・完了境界・参加者局所結果・hold・再保存防止・検索 token の復元不能化 | operation + erasure_condition の durable を参加開始より先行させる（durable-before-enforce）。各参加者の局所完了・検証は durable 化してから coordinator へ返し、返却で hold を解除しない。全参加の集約＋機械的残存検証＋区間内再到着の取込みを満たした後、検索 token を除去または復元不能化し、その成立を確認してから全域完了を原子に確定する。token の最終消去と完了 marker を同じ durable commit に含められない場合、その間は `finalizing` の未完了状態を維持する | 全 domain の通常意味変更権・単一 transaction・無制限 access を coordinator に与えない。対象外の通常活動の一律停止を必須にしない |
| Backup 作成 | 対象時点・参照対応・除外・保護・各 owner 提供・未完了状況・保管 file | file durable を `backup_point` 成功 marking より先行させる（durable-before-mark-success）。対象時点・参照・履歴・未完了の対応が揃って初めて成功とし、各部 copy 成功だけを成功にしない。未完了消去・復旧と重なる場合は制約を無視した正常 copy を作らない | 処理中 memory の丸ごと保存を要求しない。一時 buffer・Provider session の復元を前提にしない |
| Restore 正本切替 | 復元内容・Credential 維持・外部非巻戻し・単一正本・非混合・権限先行復活の禁止・旧 live 分離・保留・一括有効化 | staging（別 file / 別 group）での照合・検証を先行させ、`restore_generation` の原子 switch で正本を切り替える。switch 前は復元前正常が正本、switch 後は復元内容が正本とし、第三の混合を作らない。switch 前の crash は旧正本＋pending、switch 後の crash は新正本＋保留とする。復元済み assignment/consent だけで自動利用を開始しない | Credential store・外部現実・現在の到達性・未完了の保留は置換対象から除外して維持する。Audit は backup 時点置換＋成立後の Restore 事実追記とする |
| Character revision / applied relation | Character revision・差分提示・Owner 明示選択・適用関係・経験状態 | 新 revision insert と適用は分離する。適用は `expected_character_revision + OwnerSelectionRef` の atomic compare による current pointer 更新＋履歴 append とし、未確認部品を更新済みにしない。適用禁止種別を revision に含めない・適用対象にしない | 内容の正本（Character）と適用関係の正本（個体調整）と経験状態（認識・学習）を同一更新にしない。Package 更新を成長の初期化にしない |
| Companion deletion | 個体固有設定・Summary・Companion scope Learning・State・Relationship・担当 Schedule・残存 History・Task 記録・Global・共有 Summary・既知作用 | 個体 lifecycle（個体調整）と全域成立（保全・消去）を対応付けるが、全 owner の削除を単一 transaction にしない。各 owner の局所削除・検証を durable 化し、未完了なら削除完了と表示しない。新規活動禁止 hold を削除開始より先行させる。遅延結果から削除済み個体状態を再作成しない | History・Task 記録を個体削除だけで消さない。Summary を historical log へ分類し直して残さない。自動 Global 化・Schedule 自動引継ぎをしない |

### 7.2 代表的な atomic unit（SQLite transaction の例示。mechanism 共有であり ownership ではない）

- AU1a（同 owner 原子）: `history_message` insert + `undelivered` insert（会話・交流由来の要時）。いずれかだけが残る中間を可視にしない。
- AU1b（順序＋原子登録）: Task 由来の `undelivered` は Task 結果 durable 後に別 transaction で原子に登録する（Task durable→未伝達可視の順序。単一 transaction にまとめず、共有 SQLite transaction は mechanism として許す）。Task durable なしに未伝達だけが残る dangling、Task durable ありに未伝達なしの報告漏れのいずれも残さないよう、crash 後は Task 結果と未伝達の対応を照合して未登録を補完できること。
- AU2（同 owner 原子）: `task` insert + `task_revision` insert + 初期 `task_context_entry` insert(s) +（Workspace 関連付けを確定した場合のみ `workspace_assoc` insert）。commit 前は委任・実行から不可視。いずれかの insert が失敗したら先行 insert を含めて rollback し、一部だけの行・未 commit の部分を可視にしない。
- AU3（cross-owner atomic read）: `delegation` insert 時の `expected_task_revision` 照合（Task 現在 revision との compare）。不一致なら不受理・再評価へ戻す。
- AU4（同 owner 原子）: steering 時の `task_revision` forward + `task` current pointer 更新（現在 `TaskPurposeRef`・目的本文も同一 commit で更新し、`new_purpose: None` は直前値を維持する）+ 新 `task_context_entry`（目的を変更しない場合も直前の採用目的 entry を新 revision に引き継いで記録する）。旧 revision を残す。
- AU5（同 owner 原子 + 別 owner 順序）: `action_attempt` insert（前提照合付き）。確定度更新は同 row の `expected_certainty + 新 evidence` の原子更新。Task 達成は別 transaction で attempt を読み取って更新する（durable-before-adopt）。
- AU6（cross-owner atomic read）: `usage_fact_*` insert + cap 照合（`cap_limit` + 関連 `usage_fact_*` の合計読み取り）。同一 transaction 内で判定し、cap・不明で継続不可なら data 保持のまま停止・判断待ちにする。
- AU7（同 owner 原子）: `presence_attribution` の `expected_generation + expected_state` 照合付き更新 + `presence_transition_log` append + `relocation_hint` 更新（要時）。
- AU8（順序）: 未伝達 `Presented` は提示確認後の durable 更新とする。送信時には更新しない。
- AU9（順序＋完了境界）: `deletion_operation` + `erasure_condition` durable → 各 `deletion_participant` durable → 全参加の集約・機械的残存検証・区間内再到着の取込み → 検索 token の除去または復元不能化 → 全域完了の durable 原子確定。token の最終消去と完了 marker を同一 durable commit にできない場合、その間は `finalizing` として D3 を維持し、crash 後に最終消去を再開する。検索 token が復元可能な状態で `全域完了` を保存しない。
- AU10（順序）: backup file durable → `backup_point` 成功 marking。失敗時は最後の正常を破壊しない。
- AU11（原子 switch）: restore staging 検証 → `restore_generation_state` bump + 正本 pointer switch の原子確定。失敗時は復元前正常を維持する。
- AU12（同 owner 原子 + cross-owner 照合）: `companion_applied_current` 更新 + `companion_applied_history` append（`expected_character_revision` 照合付き）。
- AU13（協調・非単一 transaction）: Companion 削除は各 owner の局所 durable の集約とし、単一 global transaction にしない。新規禁止 hold を先行させる。

cross-owner の atomic read（AU3/AU6 等）は同一 SQLite file の transaction を使うことを許すが、意味変更権の統合・共同所有・無制限 access を意味しない。Coordinator は参加 owner の局所結果に依存し、全構造への無制限 access を要求しない。

## 8. Targeted Deletion と persistence schema の関係

`targeted-deletion.md` と `data-preservation-erasure.md` を前提とし、semantic owner・boundary を変更しない。

### 8.1 追跡不能にしないための persistence 要件

- 各 durable content row は、本文を複製せず、次の対応を保持または正本から解決できること：`(owner, source 関係, 取得・生成時点, scope・共有関係, 用途・保存・共有・送信の制限, revision・generation 前提)`。本書 §4 の各 table の CORR / REV / GEN / 時点 field がこれに当たる。
- 遅延結果の帰属に要る `(attempt, Task revision 前提, source 範囲, presence/restore generation)` の対応を `action_attempt` / `task_revision` / `history_message` / `undelivered` の CORR として保持すること。source を消した後に依存関係も消失し遅延結果を識別できなくなる実装は不可とする。
- 各受入・保存先が飛行中の消去条件を適用できるよう、`erasure_condition(operation, sweep, valid_interval)` を durable かつ index 付きで保持すること。既知 source の追跡だけでは区間内再到着を扱えない。
- 派生物・cache・Client / 拡張一時 copy・処理中利用も消去へ参加できるよう、derived の invalidation key（§5）に消去条件を含めること。hit を理由に制約確認を省かない。
- 局所処理・検証との関係（どの参加先がどこまで除去・検証・未完了か）を `deletion_participant` として durable に保持すること。局所返却で hold を解除しない。

### 8.2 過剰設計にしないための制限

- 「すべての文字列の意味的依存を DB graph へ保存する」ことはしない。graph 全体を永続 object にせず、必要な対応だけを CORR として保持し辿れればよい。
- 全 Summary へ同じ retention / revision 方式を課さない。分離を確認できない混合出力は全入力に依存し得るものとして扱うが、LLM 自己申告だけで依存を外さないという contract で足り、完全な意味 graph を要求しない。
- 指定文字列の機械的検索・除去・残存検証を LLM へ依存させない。意味的同一情報の特定に LLM を利用できるが完全検出を保証しない。検出限界を既知依存の追跡省略・未確認範囲の完了扱いにしない。
- 対象記述の伝達・保持に private 本文の複製を増やさない。`deletion_operation.mechanical_search` は操作期間限定の `SealedSearchToken` とし、全域完了を durable に確定する前に除去または復元不能化する。最終消去と完了 marker が不可分でない場合は `finalizing` を durable に保ち、token が復元可能なまま完了記録・Audit・説明へ進めない。制限情報・識別用の値も復元できるなら保護・消去対象である。

### 8.3 Crash 中の deletion の persistence

- 操作途中の restart でも未完了の認識と必要な保留・再保存防止を `deletion_operation` + `erasure_condition` + `deletion_participant` として Host で保全する。完了と誤認したり保留を黙って解除したりしない。
- 参加者・残存検証が終了していても検索 token の最終消去が未完了なら `finalizing` として扱う。restart 後は token を再利用して通常処理を開始せず、最終消去と完了確定だけを再開する。token の復元不能化を確認するまで hold を解除しない。
- 消去中に Client が切断した場合、確認不能を成功に読み替えず、`deletion_participant` を pending/unreachable として保全する。古い一時 data を再接続時に Host へ戻して再形成しない。
- 未完了消去と backup 作成が重なる場合、制約を無視した正常・即実行可能な copy を作らない。作成を待たせるか未完了・制約も復旧可能に含めるかは自由度である（§9）。

## 9. Backup / Restore と persistence schema の関係

`backup-restore.md` の Owner decision を維持する。古い Backup Restore に関する既存 decision を変更しない。

Restore の実行確認・復元後の一括有効化、および Full Reset の強い確認は IPC §18 の Host PC 上の trusted first-party management surface で行う。remote 要求の受付を確認済みとして扱わない。復元成立と再有効化は別確認のまま保つ。

### 9.1 Backup に含めるもの / 含めないもの

| 扱い | 対象 |
|---|---|
| 含める（durable の対応を復旧可能な形で） | Character 静的構成・revision・適用関係対応（外部 Package 原本は除く）、Companion 同一性・活動状態・適用済み構成（削除済み個体の私的 state は backup 時点に存在しないものとして扱い、残存 historical record は含める）、History・非会話活動記録・evidence・未伝達（原 record と報告状況を分け、表示 copy 送信を報告済みにしない対応を保つ）、Summary・根拠関係・Memory 現在・過去 revision・Skill 有効・過去 revision・原本対応・実行結果対応・Relationship・State と保持根拠（共有根拠の利用関係を保ち Global 本文から私的根拠全文への access 拡大を作らない）、Task・Task context・委任対応・Action 試行・確定度・停止結果、Workspace 関連付け（外部実体は辿って収集しない）、Schedule 設定・作成時 tz・初期入力・発生対応・各回 Task 対応、Rule・Permission 判断記録・assignment 同意（Observer 専用含む）・fallback・device・保存禁止・非共有・cap 等の制御条件の記録、Provider 非秘密登録・能力情報・MCP・Plugin 非秘密受入設定、利用量・費用の記録（報告/不明の区別を含む。現在消費の正本として扱わない条件は §6 に従う）、Credential の用途・参照元（非秘密のみ）、Audit 追記順・保持、保持方針・操作状況・backup 設定と作成結果のうち復旧可能な対応に必要な範囲 |
| 含めない（rebuild / transient / external / secret） | 派生物（embedding・index・query 派生・Prompt cache・Provider session・有効経路・次回表示・集計表示等）、一時 data（Raw・詳細 payload・内部推論・Client 表示 copy・入力途中・audio buffer・観測候補・推論中 context・MCP Apps 表示等。ただし受理済み指示・作業記録・未伝達・作用不明まで失ってよいわけではなく Host 正本の範囲で復元する）、Credential 等の secret・外部 Workspace 実体・Provider/MCP 側固有状態・保有 copy・外部 Package 原本・export 済み copy、Client 接続材料の secret 部分 |
| 作成成功の条件 | 各部の copy 出力成功だけを成功にしない。対象時点・参照・必要な履歴と未完了状況の対応が揃って初めて成功とする。実行中の不明がある場合、最後の正常記録が外部最新とは限らないことを保ち、不明を未実行へ戻して正常 copy と偽らない |

上表の device は Group G の `device_ref` / `client_last_connection` と Group F の `device_permission` 等の非秘密参照・記録だけを指す。Host device-auth record（Group K、E）の検証材料・現在 trust 範囲・失効状態は、非秘密部分を含めて backup から除外する。現在環境で維持する認証・trust を、復元 data の一部として扱わない（§9.2）。

Backup 作成に担当 Companion や Task Agent の稼働を必要とせず、管理面と保存済み data の利用可能性を Body・Voice・Provider・拡張の成功へ従属させない。Backup 設定自体が復元対象に含まれる場合、保存先・schedule・保持数・保護は backup 時点へ置換されるが、Owner が別保存先へ作成した既存 copy そのものは削除しない。

### 9.2 現在 environment から維持するもの

- Restore 開始前から Host にある現在の Credential store の秘密値本体（E）とその登録・更新・失効の状態（Backup から復元・巻戻ししない）。DB 側の非秘密参照（`credential_ref`）は backup の復元対象に含めるが、復元後は現在の秘密値・用途・有効性と照合し、利用可能なら現在の Credential を利用し、不足・無効なら再認証を要求する。復元された assignment/consent だけで自動利用を開始しない。
- 現在の Host device-auth store（Group K、E）の pairing identity・検証材料・現在 trust 範囲・失効状態。検証材料が非秘密でも backup 除外・Restore 非置換とし、失効で削除した材料の不在も維持する。新 Host 等で対応材料がなければ未認証とし、Host-local 最終確認による新規 pairing を必要とする。復元した `device_ref` / `device_permission` から E 側 record を生成・再有効化しない。
- 外部 Workspace の現在内容・存在・access、外部 account・source 状態、Provider・MCP 側固有状態・保有 copy、外部作用の既成事実、OS・device・Network の現在状態、現在日時・tz。
- 現在の接続・到達性・device 利用可能性・排他性の事実。保存された接続・帰属を現在の到達性とみなさない。
- Restore 前に未完了の全域操作の保留・再保存防止に必要な条件（特に未完了 targeted deletion の消去条件・検証未完了・再保存防止は置換で黙って解除しない。§9.4 に従う）。
- Restore 操作自体の受理・説明・実行保留。復元前正常は置換成立まで破壊しない。
- Audit は backup 時点へ置換されるが、置換成立後の Restore 事実（目的・対象範囲・影響・除外・確認・成立結果）は新しい Audit 事実として追記する。追記順を発生順・許可正本・再生入力にしない。

### 9.3 Restore 時の全置換対象・正本切替前後の識別

- 置換対象は現在の Credential store（秘密値・Host device-auth record、E）を除く対象内部 data の対応 backup 時点への全置換であり、旧 live との merge ではない。DB 側の非秘密参照（`credential_ref`・`device_ref`）と `device_permission` は置換対象に含めるが、現在 store の維持・照合は §9.2 に従う。局所 copy 成功の集合だけを成立にしない。
- いかなる時点でも Host 正本は一つである。置換成立前は復元前正常が正本、成立後は復元内容が正本であり、backup copy・部分置換状態を正本にしない。旧 state と復元 state を意味的に混ぜた第三の状態を作らない。
- 実行 authority だけ先に復活させない。復元成立・一括有効化・現在条件の確認前に新規 Action・送信・外部作用を開始しない。
- 識別は `restore_generation_state` の現在 generation と各参照の `restore_generation` 前提タグで行う。復元を跨ぐ参照（Task・Rule・同意・assignment・作用・未伝達・全域未完了）には generation を添え、旧世代の参照だけで復元後に利用・実行・送信しない。Task の目的変更は `task_revision` の前進であり generation 変化で代替しない（revision と generation を混同しない）。
- 復元成立と実行再有効化は別々に確認する。復元成立後も Task・Schedule・外部接続の自動処理は保留し、Owner が内容確認後まとめて有効化できる。一件ずつの再承認は要求しないが、Deny・同意・cap・認証不足・外部作用不明を無視しない。
- 復元された assignment/consent・Rule・Schedule・Client device 参照・Provider/MCP 参照は、現在の Credential・device-auth の同一 pairing identity / 有効検証材料 / trust 範囲 / 非失効・制約・保留・到達性・排他性と照合して初めて利用できる。device 行だけでは auth・許可を復活させず、過去の機能許可も現在 E 側 trust 範囲を超えない。stale な Permission・Provider・Client・作用結果・外部参照を現在事実にしない。dangling 参照は未解決とし、不明は不明のまま保持し replay しない。
- 旧 live 要求・結果・Client copy を復元正本へ混ぜない。切替前に開始した推論・Tool 結果が切替後に届いても用途別受入で現在の対象・制限・意味へ照合し、旧作用説明と区別する。Client copy で Host を上書きせず、未送信操作を自動 queue にしない。

### 9.4 旧 backup 交差（確定済み Owner decision の落とし込み）

以下は確定済みの製品判断であり本書で変更しない。

- 古い Backup を Targeted Deletion 完了後に自動改変・再消去しない。保存済み copy を黙って書き換えない。
- Restore 前に削除済み情報や旧 Rule・同意・Schedule が戻り得ることを説明する。
- Owner が理解して明示 restore した場合、それを過去 state の意図的な再導入として扱う。戻った情報は新しい正本として扱い、新しい Experience の根拠になり得る。自動再形成禁止の迂回ではない。
- 過去の Targeted Deletion を Restore 後に自動再適用しない。改めて消去するには新しい targeted deletion の明示が必要である。
- cache・session・delayed result 等による意図しない再出現とは区別する。前者は再保存防止で防ぐ対象であり、後者は Owner の明示判断による別操作である。復元成立後の保留・一括有効化・現在条件の再評価を経ずに戻った情報を送信・作用・学習へ自動利用しない。
- 未完了の消去と backup/restore が重なる場合は、制約を無視した正常・即実行可能な copy を作らず、未完了・保留・再保存防止を Host で保全する。
- 消去区間に重なる Restore 由来の再到着は、各受入・保存先が進行中の消去条件を適用し、古い根拠だけからの再形成・遅延再保存を防ぐ。

### 9.5 Restore operation 自身の durable progress

- `restore_operation`（受理・説明・隔離・照合・置換・保留・一括有効化対応・状態）と `restore_generation_state` を D3/D1 として Host で保全する。保留中・確認途中・置換途中に再起動しても、復元済み assignment/consent だけで自動利用を開始しない。部分置換を新正本にしない。到達不能・確認不能を成功に読み替えない。
- 失敗時は復元前正常を維持し、不完全な復元や旧 live と競合する正本を成功と表示しない。

## 10. Concurrency のために persistence が持つべき property と atomic compare

concurrency mechanism そのものは [Concurrency Control](concurrency-control.md) が定める。本節は成立のために persistence 側が持つ必要のある property と「どの比較を atomic に行える必要があるか」を固定する。

### 10.1 必須 property

- **identity**: 各 domain の newtype（`CompanionId`, `TaskId`, `ActionAttemptId`, `DeletionOperationId`, `BackupPointId` 等）。opaque・一意・再利用しない・削除後に再発行しない。内部表現が同じでも相互変換しない。
- **revision**: 同一 identity に対する owner 判断の順序（`task_revision`, `learning_revision`, `rule_revision`, `character_revision`）。`(identity, revision)` の組で扱い、単独で持ち歩かない。異なる identity 間・異なる owner 間で比較しない。
- **generation**: 同一 lifecycle の区間順序（`presence_generation`, `restore_generation`, `deletion_sweep_generation`）。値の新旧ではなく区間の識別。異なる lifecycle 間で比較しない。global version counter を設けない。
- **expected current value（boundary token）**: 照合の入力として渡す期待する対応の写し（`expected_task_revision`, `expected_presence_generation + expected_state`, `expected_certainty`, `expected_rule_revision` 等）。authority ではなく比較材料であり、不一致なら hold・deny・不足・再評価へ戻す。暗黙の「最新を使う」を設けず、欠落は不受理の理由にする。
- **operation identity**: 全域操作の対応付け先（`deletion_operation_id + sweep`, `restore_id + generation`, `backup_point_id`）。局所完了と全域完了を区別するために使う。
- **attempt identity**: 論理的な試行の識別（`action_attempt_id + prior_unknown` 対応）。retry は別 Attempt とし、同じ試行の継続として除外しない。
- **durable boundary**: 消去条件・有効区間・完了境界・保留・hold（`erasure_condition.valid_interval`, `hold_conditions`, `post_restore_hold`）。各利用箇所が照合できる関係として durable に持つ。

### 10.2 Atomic compare 要件表（mechanism は後続）

| 利用・受入 | atomic に比較しなければならないもの | 不一致時の扱い |
|---|---|---|
| Task への委任・steering・Action 開始 | `(task_id, expected_task_revision)` × 現在 `(task_id, task_revision)` × 委任 scope・Workspace 有効性・steering 前提 | 開始しない。旧判定・解決済み経路だけで開始しない |
| Action の新規開始 | `attempt` の `(Task revision 前提, 委任 scope, 実対象・操作種別, 依拠 Permission evaluation)` × 現在の同型の組 × Client 依存なら `presence_generation` と現接続・可用性 × 消去・復元保留 | 開始しない。Owner 確認待ちは実行せず待機にする |
| 確定度の更新 | `(attempt_id, expected_certainty=Unknown)` × 新 evidence の事実 owner 確認 × 現在 Task revision | 新 evidence なしに Unknown を未実行・成功・失敗へ書き換えない |
| Client 依存活動の開始・継続 | `claimed_generation` × `presence_attribution.generation` × 現接続・可用性 × 現在許可・停止・保留 | 確認不能なら継続しない。旧一時 state・旧承認だけで成立させない |
| 消去区間の受入・生成・再保存 | 到着・生成情報の `(source 関係, 取得・生成時点)` × `(operation, sweep, valid_interval)` × 保持者の局所検証 | 区間内再到着・再生成は消去対象とする。実行中処理による再保存をしない |
| 復元後の利用 | 利用の `(restore generation 前提, assignment/consent revision, Credential 照合, 依拠 Rule revision)` × 現在の `(restore_generation, 現 store, 現制約, 復元後保留)` | 旧 live・旧同意・旧 assignment だけで自動利用・自動処理を開始しない |
| 権限・Rule 解釈の採用 | 過去 Allow・復元 Rule・context 内許可文・cache 判定 × 現在の `(rule revision, 同意, device, cap, 失効・停止・帰属・消去・復元保留)` | 制御を変更しない。将来 Rule は解釈・表示・保存・Undo を経る |
| 費用・資源の継続判断 | 消費の `(用途・送信先対応, 報告/不明/処理中の別)` × 現在 cap・資源・不明の扱い | 処理中・遅延・不明をゼロにしない。並列で同一残額を使い切れる扱いにしない |
| Character 適用 | `(character_id, expected_character_revision)` × 現在適用関係 × `OwnerSelectionRef` | 未確認部品を更新済みにしない。適用禁止種別を適用しない |
| 全域完了の確定 | 各 `participant` の局所完了・検証・未完了・失敗 × 機械的残存検証 × 区間内再到着の取込み × 本文非再保存 × 検索 token の除去・復元不能化 | 未確認・検証失敗・pending/unreachable・token 残存を成功に読み替えない。局所完了の集合だけを全域完了にしない |

「最新の値を読んだ」「cache に hit した」「到着順で最後」であることは、いずれも単独では受入根拠にならない。revision と generation は混ぜない。

## 11. Technology choice（logical requirements からの導出）

| state 種別 | 使う技術 | 導出理由 |
|---|---|---|
| Host durable primary / history / operation（D1/D2/D3。秘密値・blob 本体・derived を除く） | SQLite 単一 file（例: `app.db`） | backup 整合（対象時点・参照対応・未完了状況を復旧可能な対応で含める）と必要最小の cross-owner atomic read（Task+未伝達、usage+cap、presence 切替等）のため。共有は mechanism であり ownership 統合ではない。単一 file を理由に一つの state・lifecycle・revision へまとめない |
| embedding / index / score（R） | sqlite-vec（derived file / group。例: `derived.db`） | 検索用派生は元 state との対応・利用制限に従い、派生物なしでも正本が失われないこと、targeted deletion へ参加しつつ primary を破壊せず再構築できることが必要であるため。古い派生物から権限・状態を復活させない |
| cache / session / 一時 context（R/T） | SQLite derived group または process memory（永続化しない） | hit/miss・期限切れで論理 context・権限・永続化契約を変えないこと、Provider 側にしか継続状態が残らない構造を作らないことが必要であるため |
| 内部保持 copy・中間 file の blob 本体 | filesystem（例: `internal_copies/`）+ DB 側の参照・由来・用途・削除 marker | 大きい blob を DB inline に必須としないため。意味は作業等に残り、外部所有を理由に内部消去から除外しない。backup には内容を含め、外部 Workspace 実体は収集しない |
| Credential 秘密値・Client 接続材料の秘密部分・Host device-auth record | credential store（OS 分離保管の抽象。DPAPI / libsecret / Keychain 等。具体方式は固定しない） | 秘密の通常経路への非露出、device 検証材料・trust の backup 除外・Restore 維持・Reset 範囲の分離が必要であるため。DB 側は非秘密参照だけを持つ（Group K） |
| portable full backup copy | filesystem（例: `*.ene-backup`。暗号化選択可能） | 稼働中の正本とは別の外部 copy として境界を保ち、明示 restore を経ずに live 正本へ戻さないことが必要であるため。保存先・schedule・保持数・保護は Owner が選ぶ |
| Client 一時・Provider 保有・MCP 側状態・外部 Workspace 実体・外部 Package 原本 | 各所在のまま（ene の durable としない） | Client を正本・永続 cache にしないこと、Provider/MCP 側を内部正本・内部消去保証に含めないこと、外部所有を内部所有にしないことが必要であるため |

DB を分ける / 同一にする判断は上表の必要性に限る。Host durable を単一 file にすることは、全 state を一つの保存単位・一つの lifecycle・一つの revision へまとめる理由にならない。一般 App Data は Owner の OS account だけが扱える領域に置き、一律の application-level 暗号化を必須にしない（要件の Local data 契約を維持する）。

## 12. Validation walkthrough

固定チェックリストではなく、正常系と意味のある競合・障害を選んで walkthrough した。各行の owner・contract は既存のままである。

| scenario | 歩行と必要な結果 | 本書の成立箇所 |
|---|---|---|
| normal conversation → History → Learning | 会話→`history_message` append +（要時）`undelivered` の AU1a 原子 durable。形成は別 transaction で認識・学習が現在認識・根拠・scope・保存条件・消去状況へ照合し、必要な状態だけ形成・Summary を共通根拠に対応付ける。訂正は新 Experience とし過去発言を現在認識へ書き換えない | §3, §4 Group B/C, §7 AU1a |
| Task delegation → result | Task 作成の AU2 原子 durable（task+task_revision+初期 context＋関連付けを確定した場合の workspace_assoc の durable-before-visible）。委任は `expected_task_revision` の atomic compare。結果は attempt→Task revision→現在 Task の順に辿り、記録（元へ残す）と採用（現在の受入）を分ける。旧目的の結果を新目的に自動採用しない | §4 Group D/E, §7 AU2/AU3/AU5, §10 |
| unknown external Action → restart | `action_attempt(Unknown + 根拠・hold + generation タグ)` を D3 として保全し、restart 後に Unknown のまま再構成する。自動再実行・replay せず、重複 risk を示した Owner 判断による新 attempt とする。記録保存失敗を未実行の根拠にしない | §4 Group E, §6, §7 AU5 |
| Client move → restart → stale reconnect | `presence_attribution(generation)` + hint・復旧先（非現在）+ 現接続・許可・排他性の確認で stale を識別する。再接続の古い一時 state・旧承認・解決済み経路だけで presence・Permission・再開を成立させない。元 Client 利用不能なら active なしで待つ。旧 round・旧試行を replay しない | §4 Group G, §6, §7 AU7, §10 |
| Targeted Deletion 中の crash | `deletion_operation` + `erasure_condition` + `deletion_participant` を Host で保全し、未完了・保留・再保存防止を維持する。参加者・残存検証完了後でも検索 token が復元可能なら `finalizing` として再起動後に最終消去を続け、token の除去・復元不能化を確認する前に全域完了・hold 解除へ進まない。区間内再到着・遅延再保存・古い根拠からの再形成を防ぎ、完了記録・Audit へ本文を残さない | §4 Group J, §6, §7 AU9, §8 |
| Restore 途中の crash | 置換成立前は復元前正常を正本とし、成立後は復元内容を正本とする単一正本を守る。staging 検証→`restore_generation` switch の原子確定とし、部分置換を新正本にしない。成立後も自動処理は保留し一括有効化待ちとする。再起動で保留を解除しない | §4 Group J, §6, §7 AU11, §9 |
| delayed result arrival | 用途別受入（会話・Task・Learning・Permission・次 Action・必要事実）で現在の対象・制限・意味へ照合し、記録・更新・次実行・提示を分ける。到着が遅いことだけで出来事を新しくせず、最新到着だけで現在値を決めない。消去・復元・steering・Cancel 後の到着物は元 Action/Task へ事実を残し後続を自動開始しない | §7, §10（CC-03 の persistence 側の保持） |
| Companion / Character deletion | Companion 削除は個体固有 Summary・Companion scope・Skill 過去 revision・State・主体/相手 Relationship・担当 Schedule を削除し、History・活動記録・Task 記録・Global・共有 Summary・外部 file を残す。各 owner の局所 durable の集約とし単一 transaction にしない。新規禁止 hold を先行させる。Character 削除は静的定義の除去であり経験・記録へ cascade しない。dangling は未解決とする | §4 Group A/B/C/D, §6, §7 AU12/AU13 |
| undelivered Task completion | Task 完了と報告完了を分ける。`undelivered(Pending→Summarized→Presented)` は提示確認後の durable 更新とし、接続・表示 copy 送信・Task 完了だけで報告済みにしない。active なし期間は Host に残し次 Client で現在の結果・利用制限・削除状況へ照合して要約報告する | §4 Group B, §6, §7 AU1a/AU1b/AU8 |
| retention / reset 等の全域操作（追加） | 通常 History/log 整理は形成済みへ cascade させず、Learning revision・Summary の cleanup は既定 OFF・明示 opt-in のみとする。全データ Reset の途中 crash でも旧処理・一時 copy から復活させない。外部 Workspace・保存 backup を削除しない | §4 Group J, §6 |
| Schedule 到来と active なし（追加） | Schedule 設定・発生対応・各回 Task の区別を保ち、停止中の回は missed とし自動補完しない。将来回は新 Task として現在条件で開始する。active なしでも Host 完結作業は継続し、Client 依存確認は判断待ちにする | §4 Group D, §6 |
| Observer routing と scope 変更（追加） | routing 限定文脈は R/T の派生表現とし、新正本・新 scope・包括共有にしない。scope 変更・同意失効・消去を生成済み要約・処理中結果にも適用する。混合文しかなく分離を確認できなければ個体へ渡さない | §5 |

## 13. 意図的に残した Design Freedom

| 設計対象 | 固定済みの architecture property | 残す Design Freedom |
|---|---|---|
| 保存実装 | 対応の保持・照合可能性、本文非複製、secret 分離、derived 分離、backup 除外を守る | 具体 SQL 方言・index・migration・vacuum・暗号実装・filesystem layout・blob inline/file の選択・`SealedSearchToken` の実装・到達性確認方式 |
| 原子性・順序の実現 | §7 の atomicity / ordering / durable-before-visible、§10 の atomic compare を守る。確認不能を許可・現在・完了へ変換しない | transaction library・lock / MVCC / actor / queue / event bus・IPC・process 配置・retry/timeout 値・exact progress 表現 |
| 派生物の実現 | 第二正本化の禁止、invalidation key（revision/generation/消去条件）の保持、消去参加を守る | embedding / scoring・cache 実装・routing context 生成 model/Provider・形式・更新頻度・鮮度・選択 algorithm・再構築の時機 |
| 全域操作の実現 | 単一正本・非混合・権限先行復活の禁止・成功表示の条件・再起動時の保全を守る | 停止伝達・切断検知・帰属調停・round 区切り・対象探索・参照整合・照合の配置・backup 整合時点の作り方・部分失敗からの復旧・保存形式・対応 version |
| 記録・報告・監査 | 元事実の確定度・privacy・順序の意味と管理到達性を維持する。本文別保管庫を作らない | audit format・保持期間・提示確認・要約粒度・UI layout・診断 stack |
| 時間・費用 | 経過時間の解釈、missed 非補完、並列・処理中・不明の上限反映を守る | 減衰・時刻計算・Capture 時機・費用予約・集計期間・推定・資源配分の機構 |

archive / file format、encryption implementation、DB schema の製品固有 SQL、serialization、transaction mechanism、Rust type / trait、crate / module、IPC、locking、exact progress representation、retry / timeout、specific library も固定しない。上表の対応関係から統一 Context layer、Policy Engine、Manager、Service、Coordinator の追加を導かない。既存の12責務、semantic owner、Host / Client 配置と trust boundary の下で実現方法を選ぶ。
