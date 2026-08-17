# セッションログ

> 実現する要件: **P-501**(イベントソーシング・ログ)、**P-502**(model-visible = logged)、
> **P-108**(セッション分割・一覧・再開)、**P-109**(fork)、**P-110**(エクスポート)、
> P-114 の拡張余地(room)。

セッションで起きた**すべての事実**は追記専用のイベントログに記録される。
UI の履歴表示・モデルへの提示・fork・リプレイ・エクスポートは、すべて
ログからの投影であり、ログ以外の場所に「正」の状態を置かない。

## 1. 責務と責務外

- **責務**: イベントの語彙定義、永続化、順序保証、投影(`derive_messages`)、
  fork/resume、エクスポート、不変条件の検証。
- **責務外**: コンテキスト組立(何を入れるかの政策)は
  [context-assembly.md](context-assembly.md)、ループの駆動は
  [agent-loop.md](agent-loop.md)、可変状態(レジスタ)・使用量台帳・
  クラッシュ回復の耐久機構は [storage-model.md](storage-model.md) と
  [durability.md](durability.md)(この文書は entries ストアの語彙のみ)、
  記憶の変更履歴は
  [../companion/memory.md](../companion/memory.md)(記憶は独自の監査ジャーナルを持つ)。

## 2. 不変条件

| # | 不変条件 | 検証手段 |
|---|---|---|
| L-1 | **model-visible = logged**: モデル要求に含まれるあらゆるコンテンツは、セッションログから再構成可能 | dev ビルドでモデル要求のコンテンツハッシュと投影ハッシュを照合する runtime assert。CI でキャプチャ型テスト |
| L-2 | ログは追記専用。seq はセッション内で単調増加・無欠番 | 書き込みは単一ライターアクター経由のみ。起動時に `PRAGMA integrity_check` + seq 欠番スキャン(サンプル) |
| L-3 | 訂正は新イベント(tombstone)であり、過去イベントの UPDATE/DELETE はしない | スキーマ層で DELETE を許可しない API 面 |
| L-4 | 対話レーンのターンは重複しない(同 soul 同 lane)。job レーンのターンは別命名空間 | ループが強制し、ログの `turn/start` に `lane` を記録して検査可能にする |
| L-5 | fork は境界 seq までの**接頭辞コピー**であり、元セッションは不変 | fork 後に元セッションへ追記できること、fork が境界以降を持たないことをテスト |

## 3. イベント語彙

全イベント共通フィールド:

| フィールド | 型 | 説明 |
|---|---|---|
| `session_id` | `SessionId`(UUIDv7) | 属するセッション |
| `seq` | `u64` | セッション内の単調増加連番 |
| `ts` | RFC3339 | 記録時刻(UTC) |
| `kind` | string | 下表のイベント種別 |
| `payload` | MessagePack | 種別ごとのフィールド |

`payload` のエンコーディングは MessagePack(IPC と統一)。各 kind の
スキーマバージョンは payload 内の `v: u32` で持ち、読み込み側は未知の
`v` を「将来のイベント」として保存したまま読み飛ばす(前方互換読み飛ばし)。

### 3.1 セッション生命周期

| kind | payload フィールド | 説明 |
|---|---|---|
| `session/start` | `soul_id`, `body_id?`, `created_by: client\|schedule\|import` | セッション開始。最初のイベント |
| `session/title` | `title` | タイトル更新(上書き式) |
| `session/summary` | `scope: session_end\|compaction_ref`, `summary` | 要約の記録。`session_end` はセッション境界の要約 |
| `session/end` | `reason: explicit\|idle_timeout\|topic_boundary`, `summary_ref?` | セッション終了。再開時は tombstone 化する(§7) |
| `session/reopen` | `previous_end_seq` | 終了済みセッションへの追記開始。対応する `session/end` を tombstone する |
| `session/archived` | `archived: bool` | アーカイブ状態の切替 |
| `fork/point` | `source_session_id`, `boundary_seq` | **fork 先**セッションの先頭に置く。接頭辞の来歴 |

### 3.2 ターンとステップ

| kind | payload フィールド | 説明 |
|---|---|---|
| `turn/start` | `turn_id`, `lane: dialogue\|delegation:<id>`, `origin: user\|proactive\|scheduled\|delegation\|subagent`, `delegation_id?`, `trigger: text\|voice\|timer\|system` | ターン開始。対話レーンでは先行 `turn/end` なしに出現しない(L-4)。`delegation` は job レーンの子ターン、`subagent` は対話レーンの**報告ターン**([delegation.md §6](delegation.md#6-報告ターンp-521)) |
| `turn/end` | `turn_id`, `outcome: completed\|interrupted\|cancelled\|failed`, `error_class?` | ターン終了。`interrupted` はユーザー割り込み、`failed` は必ず `error_class` を伴う |
| `step/start` | `turn_id`, `step_index: u32` | ステップ開始(モデル要求の前) |
| `step/end` | `turn_id`, `step_index`, `outcome: next\|stop\|error`, `finish_reason?` | ステップ終了 |

### 3.3 メッセージ

コンテンツは**ブロック列**で表現する(テキスト/画像参照/ツール参照/内面タグ)。

| kind | payload フィールド | 説明 |
|---|---|---|
| `user/message` | `turn_id?`, `blocks: [Block]`, `input_modality: text\|voice\|attachment`, `client_id` | ユーザー発話。音声入力は ASR 確定テキスト+元音声の artifact 参照 |
| `assistant/message` | `turn_id`, `step_index`, `blocks: [Block]`, `finish_reason`, `token_count?` | 確定した応答。**チャンクは永続化しない**(軽量優先、§6 参照) |
| `assistant/thinking` | `turn_id`, `step_index`, `blocks`(thinking 原文), `model_id` | プロバイダの reasoning 出力。**記録はする**(永続)が、ユーザー向け表示には絶対に出ない([visibility.md §3](visibility.md))。履歴への再送はプロバイダ規約に従う(§5 の `ThinkingVisibility`) |
| `inner/message` | `turn_id?`, `step_index?`, `aspects: [thought\|emotion\|action_intent]`, `blocks`, `model_visible: bool` | 内面イベント。詳細は [../companion/inner-channel.md](../companion/inner-channel.md) |
| `context/system_message` | `blocks`, `source_key` | 途中システムメッセージ(Context Source の変更通知)。詳細は [context-assembly.md](context-assembly.md) |
| `context/epoch` | `epoch_id`, `reason: init\|compaction\|session_move\|incompatible` | Context Epoch の境界マーカー |
| `compaction/applied` | `from_seq`, `to_seq`, `summary_event_seq` | seq 範囲が要約で置き換えられたことを示すマーカー |
| `compaction/start` | `compaction_id`, `turn_id?` | compaction 操作の**ロック取得**(ロック括弧の開始、[operations.md §5](operations.md#5-compactionstate構造変更))。孤立ロック検出の鍵 |
| `compaction/end` | `compaction_id` | ロック解除。`start` があって `end` がない = 孤立ロック(fault、[durability.md §8](durability.md#8-故障の分類faults)) |

### 3.4 ツール

| kind | payload フィールド | 説明 |
|---|---|---|
| `tool/call` | `turn_id`, `step_index`, `call_id`, `tool_name`, `source: plugin:<id>\|mcp:<server>\|delegated`, `args` | ツール呼び出し。ビルトインも `plugin:<id>`(バンドル済みアウトプロセス)。`delegated` は秘匿サブエージェントの親呼び出し(中身は秘匿、この枠だけ記録) |
| `tool/result` | `call_id`, `status: ok\|error\|cancelled\|denied`, `blocks` または `spill_ref`, `error_class?`, `duration_ms` | ツール結果。`denied` は承認 plane の拒否。`completed`/`failed` は使わない(`ok`/`error`) |
| `tool/spill` | `call_id`, `spill_ref: sha256`, `size_bytes`, `summary_blocks` | 巨大出力の spill 記録([context-assembly.md §5](context-assembly.md#5-spill)) |
| `tool/pruned` | `call_id`, `from_seq`, `original_size`, `kept_chars` | 直近ツール出力の prune 置換([context-assembly.md §7](context-assembly.md#7-compactionp-506))。投影は置換後を使う。元行は残る |
| `question/asked` | `turn_id`, `call_id?`, `question_id`, `blocks`, `channel: dialogue\|approval` | ask-user の質問。対話レーンは `dialogue`(キャラ発話)。ポップアップは承認 plane(`approval`)に限る([agent-loop.md §9](agent-loop.md#9-人間協調面plan--ask-userp-511-p-512)) |
| `approval/decision` | `call_id`, `decision: allow\|deny`, `mode: policy\|ai_auto\|popup`, `policy_ref?`, `reason?` | 承認判断。詳細は [../security/approval.md](../security/approval.md) |

### 3.5 秘匿と tombstone

| kind | payload フィールド | 説明 |
|---|---|---|
| `redaction` | `target_seq`, `reason: user_request\|secret_scan` | 対象イベントの秘匿。投影時に payload を秘匿済みプレースホルダに置換。元行は残る |

サブエージェント内部の会話・ツール呼び出しは、親セッションログに
**記録しない**(P-508)。委譲の実体は**子セッション**
(`kind='delegation'`)であり、そのターン・ツール・内面はすべて
子セッションに記録される。親セッションログに残るのは、
`delegation/*` の要約イベント(§3.6)と `tool/call{source: delegated}`
(親が `delegate.*` を呼んだ枠)のみ。詳細は [delegation.md](delegation.md)。

### 3.6 委譲

親セッションに記録される委譲の**要約**イベント。子セッションの
生内容はここに出ない([visibility.md](visibility.md)、P-520)。

再帰委譲(孫以降)の場合、`delegation/start`/`delegation/end` は
**ルート親のセッション**(対話レーンを持つセッション)に集約して
記録する。中間の子セッションは自分の子の分を記録しない
(秘匿の連鎖と、窓口の一元化のため、[delegation.md §7](delegation.md#7-子のコンテキストと実行))。

| kind | payload フィールド | 説明 |
|---|---|---|
| `delegation/start` | `delegation_id`, `mode: internal\|public`, `goal_excerpt`(要約形のみ), `budget` | 委譲の受理。`mode=internal` は秘匿(P-508)。UI 投影は public のみ |
| `delegation/progress` | `delegation_id`, `note`(子が送った要約), `fraction?` | 子の progress 報告の受理。レート制限済み([delegation.md §9](delegation.md#9-予算とガード)) |
| `delegation/question` | `delegation_id`, `question_id`, `question`(要約形) | 子の質問。親の回答は `delegation/answer` |
| `delegation/answer` | `delegation_id`, `question_id` | 質問の回答・取下げ(回答文は親の発話として対話ログに既出) |
| `delegation/end` | `delegation_id`, `outcome: completed\|failed\|cancelled`, `error_class?`, `artifact_ids`, `summary` | 終端。`summary` は子→親の終端メッセージ(要約形)であり、生の作業ログを含まない |

## 4. SQLite スキーマ

データベースは1ファイル(`<data>/sessions.db`、WAL モード)。記憶 DB・監査 DB とは
分離する([../companion/memory.md](../companion/memory.md)、[../security/approval.md](../security/approval.md))。

```sql
CREATE TABLE sessions (
  id                TEXT PRIMARY KEY,        -- SessionId (UUIDv7)
  soul_id           TEXT NOT NULL,
  kind              TEXT NOT NULL DEFAULT 'conversation',  -- conversation | delegation
  delegation_id     TEXT,                    -- kind='delegation' のとき委譲 id
  title             TEXT,
  created_at        TEXT NOT NULL,
  ended_at          TEXT,
  end_reason        TEXT,
  archived          INTEGER NOT NULL DEFAULT 0,
  parent_session_id TEXT,                    -- fork 元(存在すれば)
  fork_seq          INTEGER,                 -- fork 境界
  next_seq          INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE session_events (
  session_id TEXT    NOT NULL REFERENCES sessions(id),
  seq        INTEGER NOT NULL,
  ts         TEXT    NOT NULL,
  kind       TEXT    NOT NULL,
  payload    BLOB    NOT NULL,               -- MessagePack
  PRIMARY KEY (session_id, seq)
) WITHOUT ROWID;

-- 一覧・検索用(最小限の索引のみ。走査は PK 順で十分速い)
- セッション一覧・検索は既定で `kind='conversation'` のみ。
  子セッション(`kind='delegation'`)は診断経路からしか開かない
  ([visibility.md](visibility.md))。
CREATE INDEX idx_sessions_soul ON sessions (soul_id, kind, created_at DESC);
CREATE INDEX idx_events_kind ON session_events (session_id, kind);

-- spill 参照(content-addressed。本体はファイルストア)
CREATE TABLE spill_objects (
  sha256      TEXT PRIMARY KEY,
  size_bytes  INTEGER NOT NULL,
  mime        TEXT,
  created_at  TEXT NOT NULL
);
```

- `next_seq` は sessions 行上の単調カウンターとして更新し、イベント挿入と
  同一トランザクションでコミットする(欠番防止、L-2)。
- PRAGMA 既定: `journal_mode=WAL`、`synchronous=NORMAL`(軽量優先。
  クラッシュ時は最後にコミットされたイベントまでが有効)、
  `busy_timeout=5000`。
- 書き込みは**単一ライターアクター**に集約。読み取り(投影・一覧)は
  WAL のおかげで並行可能。
- 同一 DB にはさらに `session_registers`(可変状態)と
  `session_usage`(使用量台帳)の2テーブルがある。スキーマ・書き込み規律・
  排他性規則は [storage-model.md](storage-model.md) の §3・§4 が
  定義する(entries/registers/usage の3ストアで1つの永続モデル)。

## 5. 投影(`derive_messages`)

モデル履歴と UI 履歴の唯一の生成手段。入力と規則:

- 入力: `session_id`、任意の `since_seq`、`InnerVisibility`、
  `ThinkingVisibility`、`redaction: apply`(既定)。
- `InnerVisibility`: `self_reference`(モデル履歴向け、窓あり)か
  `off`(UI/エクスポート既定)。
- `ThinkingVisibility`: `provider`(モデル履歴向け。プロバイダ規約で返送)か
  `off`(UI/エクスポート既定)。
- 出力: `ProjectedHistory { messages: [ProjectedMessage], truncated_prefix: bool }`。

規則:

1. seq 昇順に走査し、`user/message`・`assistant/message`・
   `context/system_message` を時系列で採用する。
   `assistant/thinking` は **ThinkingVisibility = `provider`** のときのみ、
   対応する `assistant/message` の直後に隣接採用する
   (モデル履歴向け。プロバイダの多ターン規約が thinking の返送を
   要求するため)。UI 履歴とエクスポート既定では常に `off`。
2. `inner/message` は `model_visible = true` かつ InnerVisibility が
   `self_reference` のとき、**自己参照窓**(既定: 同一セッションの直近 24 件、
   設定 `mind.inner.self_reference_window`)分だけを末尾側から含める。
   窓の判定は [context-assembly.md](context-assembly.md) が最終決定するが、
   投影自体は窓パラメータに従うだけ。
3. `compaction/applied` が存在する場合、`[from_seq, to_seq)` のメッセージ群
   (thinking イベントを含む)は `summary_event_seq` の要約メッセージ
   1件で置き換える。
4. `redaction` の対象イベントは秘匿済みプレースホルダブロックになる。
5. `tool/call`/`tool/result` はメッセージには含めず、対応する
   `assistant/message`(呼び出し)と結果ブロックとして隣接配置する
   (プロトコル変換はプロバイダ適応の責務)。
6. ターン・ステップの境界情報は `ProjectedMessage` のメタデータとして添える
   (UI 表示と fork 点の特定のため)。

## 6. ライブストリームとの分担

- **永続ログ**: メッセージ単位の確定イベントのみ。
- **ライブバス**(WS でクライアントへ): テキストチャンク・音声チャンク・
  ツール進捗・内面ストリーミングなどの**非永続**イベント。
- リプレイは永続ログから再構成する(チャンクの再現はしない。
  タイプ演出はクライアント側がメッセージから合成する)。
  これは dsh がチャンクも保持する設計からの意図的な逸脱であり、
  軽量目標(設計原則1)に基づく。

## 7. fork / resume

- **fork**: `(source_session_id, boundary_seq)` を指定。新セッション行を作り、
  境界までのイベントをコピー(同一トランザクション、コピー件数上限なし。
  ただし1イベント=数 KB 程度なので実用上問題なし)、先頭に `fork/point` を
  追記する。元は不変(L-5)。
- **resume**: 終了済みセッションへの追記は `session/end` の打ち消しではなく、
  `session/reopen` を追記し、対応する `session/end` を tombstone する
  (`ended_at` を NULL に戻す)。その後の `turn/start` が追記を再開する。
  履歴の正しさは投影が保証する。

## 8. エクスポート(P-110)

- 形式: バージョン付き JSON(イベント全量、redaction 適用済み)+
  人間可読 Markdown(会話部分)。
- 秘匿処理の既定: 資格情報文字列のスキャン(既知のキー様式パターン)で
  該当ブロックを秘匿。内面イベントは `include_inner: false`、
  thinking イベントは `include_thinking: false` が既定
  (thinking を含むエクスポートは診断専用)。
  **子セッション(kind='delegation')はエクスポート対象外**
  (親の `delegation/*` 要約イベントは含まれる、§3.6)。
- エクスポートは読み取りのみで、ログに変更を加えない。

## 9. 設定キーと既定値

| キー | 既定 | 説明 |
|---|---|---|
| `store.sessions.db_path` | `<data>/sessions.db` | ログ DB パス |
| `store.sessions.synchronous` | `NORMAL` | `FULL` に変更可(安全優先環境向け) |
| `mind.inner.self_reference_window` | `24` | 内面自己参照窓(件数) |
| `store.sessions.export.include_inner` | `false` | エクスポートに内面を含めるか |
| `store.sessions.export.include_thinking` | `false` | エクスポートに thinking を含めるか(診断専用) |
| `store.sessions.projection.thinking` | `provider` | モデル履歴投影の thinking 採用(`provider\|off`) |

## 10. 障害モード

| 障害 | 挙動 |
|---|---|
| イベント書き込み失敗 | 単一トランザクションなので部分書き込みは起きない。3回再試行後、ターンを `failed`(error_class=`log_write`)で終了し、ライフサイクルイベントで報告 |
| DB 破損 | 起動時 `integrity_check` 失敗 → 起動を拒否し、最新バックアップからの復元を提案([../platform/process-model.md](../platform/process-model.md)) |
| ディスク満杯 | 書き込みエラーとして上記と同じ経路。spill 書き込みは先に容量チェック(閾値: 空き 256 MiB) |
| 未知の `v` のイベント | 保存したまま読み飛ばし、起動ログに警告 |

---

- 次: [context-assembly.md](context-assembly.md)
