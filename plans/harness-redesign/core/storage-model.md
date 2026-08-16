# 永続ストレージモデル

> 実現する要件: **P-501**(永続化の土台)、**P-502**、**P-516**(使用量台帳)、
> P-110(エクスポートの基礎)。
> 参照: pi の「three stores, one invariant」を Ene のイベントソーシング設計に適合させる。

[session-log.md](session-log.md) が「何が記録されるか」(語彙)を定義するのに対し、
この文書は「どう永続化され、クラッシュしても壊れないか」(機構)を定義する。
ハーネスの**耐久性(durability)**の土台であり、
[operations.md](operations.md)(操作状態機械)と
[durability.md](durability.md)(回復)はこの3ストアの上にのみ成り立つ。

## 1. 3ストア、1不変条件

永続されるものはすべて、次の3つのいずれかに入る。**第4の場所は存在しない。**

| ストア | 性質 | 実体 |
|---|---|---|
| **entries** | 書き込み一度(write-once)、追記専用 | `session_events` テーブル([session-log.md §4](session-log.md#4-sqlite-スキーマ)) |
| **registers** | 可変の名前空間付きセル。上書き・削除で現在値を保持 | `session_registers` テーブル(§3) |
| **usage ledger** | 追記専用のコスト台帳。変更・削除しない | `session_usage` テーブル(§4) |

設計の帰結:

- 「モデルに見せる履歴」は entries から投影する(session-log の責務)。
- 「進行中の操作はどこまで進んだか」は registers に置く(**entries に置かない**。
  操作状態は会話の一部ではなく、回復用の機構だから)。
- 「どれだけコストを使ったか」は usage ledger に置く(entries に埋め込まない。
  集計が O(n) 走査になり、改ざん余地が生まれるため)。
- entries に定着する前のコンテンツ(キューされた入力・遅延書き込み)は
  `pending.entry` レジスタで待ち、**配置トランザクションで entry になる**(§3.4)。
- ストアごとの投影(分岐索引・全文検索・統計)は3ストアから再構築可能であり、
  それ自体に権威を持たない。壊れたら作り直す。

## 2. アトミックトランザクション

**唯一の書き込み原語**。entry 挿入・usage 挿入・register 書き込み(set/delete)の
集合を、すべて成功かすべて無しかでコミットする。

```text
Transaction {
  entries:  [EventRow]          -- session_events への挿入
  usage:    [UsageRow]          -- session_usage への挿入
  registers:[{op: set|delete, namespace, key, value?}]
}
```

規則:

1. トランザクションは **all-or-none**。内部の状態が外部から観測されることはない。
2. SQLite の `BEGIN IMMEDIATE` で開始する(遅延ロックだと、読み→他接続コミット→
   書き込みで `database is locked` になる。即時ロックが必須)。
3. `sessions.next_seq` の採番は、entry 挿入と同一トランザクション内で行う
   ([session-log.md §4](session-log.md#4-sqlite-スキーマ) の L-2)。
4. seq と ts はコミット時にストレージが割り当てる(呼び出し側は指定しない)。
5. 書き込みは単一ライターアクターに集約
   ([platform/process-model.md §2](../platform/process-model.md))。
   並行書き込みの競合はアクター内で直列化する。

## 3. registers(可変状態)

```sql
CREATE TABLE session_registers (
  session_id TEXT    NOT NULL,
  namespace  TEXT    NOT NULL,     -- §3.1 のいずれか
  key        TEXT    NOT NULL,
  value      BLOB    NOT NULL,     -- MessagePack。namespace で型が決まる
  seq        INTEGER NOT NULL,     -- 最後に set したトランザクションの seq
  PRIMARY KEY (session_id, namespace, key)
);
```

- 上書きは旧値を捨て、削除はキーを消す。tombstone は置かない
  (JSON の `null` は合法な値であり、「キーが無い」とは別の状態)。
- `seq` は「このセルを最後に書いたトランザクション」であり、
  回復時の新旧判定に使う([operations.md](operations.md))。

### 3.1 名前空間(全集)

| namespace | key | 値 | 生存期間 |
|---|---|---|---|
| `lane.leaf` | レーン名 | entry の seq か null | セッション生存 |
| `lane.state` | レーン名 | `{ current_op_id?, pending_next_run? }` | セッション生存 |
| `lane.last_result` | レーン名 | 直近操作の終端結果 | セッション生存(上書き式) |
| `op.meta` | 操作 id | 受理データ(書き込み一度) | 操作生存 |
| `op.state` | 操作 id | **操作の全状態 = プログラムカウンタ**([operations.md §2](operations.md#2-opstate--プログラムカウンタ)) | 操作生存 |
| `op.tool_args` | `{op_id}:{step_id}:{source_index}` | 実行確定時の実効引数(書き込み一度) | 操作生存 |
| `op.preparation` | `{op_id}:{task_id}` | 構造変更(コンパクション等)の事前計算(書き込み一度) | 操作生存 |
| `pending.entry` | 予約された entry id | 未配置コンテンツ(§3.4) | 配置か取消まで |
| `delegation.inbox` | 委譲 id | 親→子メッセージ(instruct/answer)のキュー。子の claim で消費([delegation.md §8](delegation.md#8-耐久性)) | 委譲生存 |
| `delegation.pending_question` | `{delegation_id}:{question_id}` | 子の未回答質問(複数同時可、[delegation.md §5](delegation.md#5-子--親メールボックス))。回答か取下げで消える | 委譲生存 |
| `fact.*` | アプリ定義 | セッション名・ラベル等の可変事実 | セッション生存 |

生存期間の2種が鍵の形で読み取れる:

```text
lane.* / fact.*   セッション生存。明示的削除のみで消える
op.*              操作生存。終端トランザクション([operations.md §6](operations.md#6-終端トランザクション))が削除する
pending.entry     コンテンツが配置されるか取消されるまで生存
delegation.inbox  委譲生存。子の終端トランザクションが削除する
delegation.pending_question  同上(回答/取下げでも消える)
```

- `op.meta`・`op.preparation` は書き込み一度。`op.tool_args` はキーごとに一度
  (ステップ id を鍵にするのでバッチが衝突しない)。`op.state` のみ上書きされる。
- 操作終了時に未消化の `pending.entry`(inbox 残り・アボートで排出された分)は
  終端トランザクションが削除する。
- `lane.last_result` は終端トランザクションのみが書き、次回の終端で上書きされる。
  レーンあたり有界に1つだけ、永続に保たれる。**回復はこれを読まない**
  (回復は `op.state` を読む、[durability.md](durability.md))。これは
  「操作を受理してクラッシュしたアプリが、再起動後に結果を知れる」ためだけに存在する。
- キャンセルは痕跡を残さない: キューされた項目の取消は
  `pending`(まだレジスタあり)→ `cancelled`、entry 存在 → `already_consumed`、
  それ以外 → `not_found`。失われた cancel を再送するクライアントは
  `not_found` を成功として扱う。

### 3.2 書き込みの規律

- `op.state` は遷移のたびに**全状態**を上書きする(差分パッチではない)。
  回復はジャーナルの再生でも欠損からの推論でもなく、このレジスタを読んで
  switch するだけ。状態は **total**(前の状態に依存しない)。
- 小さい捕獲値(設定・ストリームオプション・再試行ポリシー)は inline、
  大きく安定したペイロードは兄弟の `op.*` レジスタか id 参照で置く。
- 操作が終わると終端トランザクションがレジスタ群を削除する。
  **終了したセッションには、会話と台帳と少数の lane/fact レジスタだけが残る。**
  回収すべき死んだ状態は存在しない。

### 3.3 レジスタと entry の排他性

キューされた id ごとに、**レジスタに居る(未配置)か entry に居る(配置済み)か**の
どちらか一方のみが成立する。配置トランザクションが
「entry 挿入 + `pending.entry` レジスタ削除」を原子的に行うことで保証する。
この排他性は回復の正しさの根拠になる([durability.md §3](durability.md#3-回復ポリシー))。

### 3.4 pending.entry(未配置コンテンツ)

対話の入力・遅延書き込みなど、**定着するがまだ置き場所が無い**コンテンツは、
予約された entry id をキーに `pending.entry` レジスタで待つ。

- 受理時に id を発行(UUIDv7)し、レジスタにペイロードを入れる。
- claim されて配置されるとき、**同一トランザクション**で entry になり
  レジスタは消える。
- キャンセル・ターン未消費で終わったものは、終端トランザクションが削除する。
- 意図的な二重書き込み(レジスタ→entry)はキューされた項目だけが払う。
  病的なペイロードでの増幅は計測してから最適化する(先行最適化はしない)。

## 4. usage ledger(コスト台帳)

```sql
CREATE TABLE session_usage (
  id         TEXT PRIMARY KEY,     -- UUIDv7
  session_id TEXT    NOT NULL,
  seq        INTEGER NOT NULL,     -- コミット時に割り当て
  soul_id    TEXT    NOT NULL,
  lane       TEXT    NOT NULL,     -- dialogue | delegation:<id>
  task       TEXT    NOT NULL,     -- chat | extract | classify | summarize | proactive | approve
  provider   TEXT    NOT NULL,
  model      TEXT    NOT NULL,
  entry_seq  INTEGER,              -- 紐づく entry があれば
  input_tokens      INTEGER NOT NULL DEFAULT 0,
  output_tokens     INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens INTEGER NOT NULL DEFAULT 0,
  cache_write_tokens INTEGER NOT NULL DEFAULT 0,
  cost_micro_usd    INTEGER,       -- 不明なら null
  adjustment INTEGER NOT NULL DEFAULT 0,  -- 1 = プロバイダ報告ではなく調整行
  details    BLOB,                 -- 任意の付加情報(MessagePack)
  created_at TEXT    NOT NULL
);
CREATE INDEX idx_usage_session ON session_usage (session_id, seq);
CREATE INDEX idx_usage_soul_task ON session_usage (soul_id, task, created_at);
```

規則:

- **追記専用**。行の変更・削除はしない。フォークはゼロから始まる。
- 確定した試行は、必ずその応答 entry と usage 行を**同一トランザクション**で
  コミットする(セットルメント、[durability.md §2](durability.md#2-effect-sandwich))。
- **失敗した構造変更(コンパクション等)もコストは残す**(失敗しても金は掛かる)。
- `adjustment` 行は「プロバイダ報告ではない調整」(legacy インポート合計・
  推定値の補正)。集計側は区別できる。
- 統計 API(`get_stats`)は、コミット後のどの時点でも台帳の合計と一致する。
  これが受入条件(台帳完全性、[invariants.md §3 横断要件](invariants.md#横断要件))。
- 使用量はセッション単位の台帳に加え、soul 単位・タスク単位の集計を
  インデックスで支える。「今月どれだけ使ったか」はユーザーに見える情報であり
  (プロダクト原則: ローカルファーストでもコストは可視に)、
  設定 UI と `GET /audit` に並ぶ `GET /usage` で公開する。

## 5. 恒等性(UUIDv7)

- entry・usage・予約 id はすべてセッションの id 生成器による **UUIDv7**。
  先頭 48bit は発行時刻なので、あらゆる参照が自己記述的で時刻順に並ぶ。
  コスト: id から作成時刻が漏れる(ローカルファーストなので許容)。
- 発行規則:
  1. id は**予約時**に発行する。直接追記は同一トランザクションで配置。
     アシスタント/ツール id は配置が要求時間だけ後ろにずれる。
  2. ツール結果 id は**アシスタント id のタイムスタンプを継承**する
     (新しい乱数テール)。呼び出しと結果のグループが、深夜0時を跨いでも
     id 順で時間的にまとまる。
  3. 合成セットルメントは予約済み id に書き込む
     ([durability.md §3](durability.md#3-回復ポリシー))。特殊ケースは作らない。
- **不透明ペイロード**(fact のカスタム値・メッセージテキスト・details)は
  entry id を埋め込み得る。ハーネスはその参照を追跡せず、古くなり得る。
  参照ではなく**内容をコピーする**のが規約。
- **絶対則**: セッション内で entry と usage 行は削除しない。
  唯一の例外は精密書き換え(§6)。親 entry の欠落は常に破損。

## 6. 精密書き換え(唯一の削除経路)

entries/usage を物理的に消す唯一の認められた経路。管理目的のみ。

- 用途: 秘匿(redaction)した行の物理消去、セッション全体の完全削除、
  サイズ肥大時の再構築。
- 手順: 現行ストアから**保持する行をコピー**した新ストアを作り、
  アトミックなリネームで差し替える(copy-retained-and-swap)。
  途中クラッシュでは旧ストアが無傷で残る。
- **通常の削除要求(記憶の忘却等)はここを通さない。**
  記憶は別 DB であり([companion/memory.md](../companion/memory.md))、
  セッションログの秘匿は `redaction` イベントによる投影置換
  ([session-log.md §3.5](session-log.md#35-秘匿と-tombstone))。
  物理消去は「ログ自体を消したい」明示的・管理的な操作に限る。

## 7. バックエンド

- v1.0 のバックエンドは **SQLite のみ**(単一ファイル、WAL、
  [session-log.md §4](session-log.md#4-sqlite-スキーマ) の PRAGMA)。
- ただしストレージ層は、`commit(Transaction) → CommitResult{first_seq, seqs, ts}`
  と §1 の3ストアの**インターフェース**で切り離す。
  将来のバックエンド(パーティション保持の Postgres 等)は
  この契約を満たす別実装として追加でき、Parts 2〜4 の上位は触らない。
  (上位 = operations/durability/agent-loop の層。契約が変わらない限り触らない)
- バックエンド適合性テスト([invariants.md §3 横断要件](invariants.md#横断要件)):
  同一シナリオで同一のクエリ結果・レジスタ状態・統計になることを、
  バックエンドごとに同じテストスイートで検証する。

## 8. スキーマ進化(P-518)

完全な耐久性は「実行中の状態のスナップショット」を意味し、実行中の状態は
**その日の状態機械の形**をしている。アプリを更新して状態機械が変わると、
古い形式の耐久状態がそのまま残る(操作の途中・バッチの途中・drain の途中)。
セッションは長期生存が前提なので、これを避けることはできない。

### なぜコストが小さいか

アップグレード時点で存在し得る可変面は、この設計では極めて小さい:

| 存在物 | マイグレーション負荷 |
|---|---|
| entries・usage 行(何年分) | 書き換え不能。**読み込み互換を永遠に保つ** |
| `lane.*`/`fact.*` レジスタ(レーンごとに少数) | 自明。オープン時の変換ループ |
| `op.*` レジスタ | **開いている操作の分のみ**。通常ゼロ |
| `pending.entry` レジスタ | 開いている操作の inbox 項目+予約済み next_run のみ |

可変面は数十個の現行レジスタに尽き、オープン時のマイグレーションが
成立する。単一ライター([session-log.md §4](session-log.md#4-sqlite-スキーマ))
なので、マイグレーションに並行性の問題がない。

### 機構: storage version + オープン時連鎖マイグレーション

データベースは1つの `storage_version`(整数、`sessions.db` のメタテーブル)
を持つ。バージョン付き名前空間接尾辞(`lane.state.v2`)は使わない:
検査する数は1つ、連鎖は `v1→v2→v3`、レジスタキーは点参照のまま安定する。

```text
オープン時:
  version == 現行 → 進む
  version  < 現行 → マイグレーションを順に実行。各段は1トランザクション:
                     レジスタ値の変換 → 開いている操作の処理(下記) →
                     version を上げる
  version  > 現行 → オープン拒否(古いバイナリが新しいデータを開こうとしている)
```

各段は変換と version 更新をアトミックにコミットするので、連鎖の途中で
クラッシュしても記録済み version から再開できる。変換は**変換済み値に
対して冪等**でなければならない(フィールド写像は構成により冪等)。

### マイグレーションは total

レジスタの変換はフィールド写像だが、状態機械の形の変更はそれ以上である。
次バージョンが `failure_drain` を廃止するなら、`failure_drain` の途中で
止まっている古い `op.state` にフィールド対応の行き先はない。規則:

- **マイグレーションは total**。vN→vN+1 のマイグレーションは、
  `lane.*`/`fact.*`・`pending.entry` のペイロード・開いている操作の
  `op.meta`/`op.state` を含む**すべてのレジスタ値**を変換する。
- 状態機械を変更する者は、到達可能なすべての旧状態を定義された新状態へ
  運ぶ写像を、**同じ変更**で書き、同じ変更でレビュー・テストする。
- 自然な後継がない状態は明示的な選択(通常は最も近い安全な
  intent 前の状態)に写像し、そこから通常の回復
  ([durability.md §3](durability.md#3-回復ポリシー))に進める。
  強制確定や部分脱出は作らない。
- マイグレーションはオープン時に**静止した**レジスタに対して走る
  (駆動なし・効果なし・すべての `op.state` が何らかのトランザクションが
  コミットした total 状態)。少数・全列挙可能・全型付きの値に対する
  純粋関数なので成立する。

### 3層の政策

```text
entries + usage      安定性の予算はここに置く。payload はプロバイダ形の
                     メッセージ+3種の構造型。何年分も書き換え不能なので、
                     変更は永遠に読み込み互換でなければならない。
                     精密書き換え(§6)は存在するが管理的操作であり、
                     オープン時の手順ではない。
lane / fact          オープン時に機械的にマイグレーション。少数であり、
                     永遠に安い。
op.* / pending.*     構成により一時的で少数。状態機械の変更ごとに
                     自状態の total 写像が同梱される。バージョン間で
                     機械が変化してよいのはここで、写像コストは
                     開いている操作の数(通常ゼロ)で上界が決まる。
```

設計の帰結: 揮発する部分(オーケストレーション)は一時的にし、
耐久部分(会話)は構造的に退屈にした。スキーマ進化の難しさは
退屈な部分の難しさに等しい。

## 9. 障害モード

| 障害 | 挙動 |
|---|---|
| トランザクション途中のクラッシュ | all-or-none なので中間状態は存在しない。回復は最後にコミットされた状態から([durability.md](durability.md)) |
| `BEGIN IMMEDIATE` 忘れ | 並行コミットで `database is locked`。回帰テストで検出([invariants.md §3 横断要件](invariants.md#横断要件)) |
| レジスタと entry の二重在庫 | §3.3 の排他性違反 = 破損。ロード時に拒否 |
| 親 entry が無い子 entry | 破損。ロード拒否+復元提案 |
| 台帳と entry の不一致(usage あるが entry 無い) | セットルメント違反。検証テストで検出([invariants.md §3 横断要件](invariants.md#横断要件)) |

---

- 前: [visibility.md](visibility.md) / 次: [operations.md](operations.md)
