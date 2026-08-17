# ジョブ・スケジュール・成果物

> 実現する要件: **P-605**(job)、**P-606**(スケジュール)、**P-607**(リマインダー)、
> **P-608**(ワークフロー)、**P-609**(artifact)。

対話の外で進む「仕事」の全体。**ユーザー向けの呼称は「タスク」**(D-12)。
job は**非同期委譲の public モード**である——実体・ライフサイクル・
親↔子メッセージ・耐久性は [../core/delegation.md](../core/delegation.md) が
定義し、この文書はユーザー向けの job 面(定義・スケジュール・
ワークフロー・成果物)を定義する。`job_id` は `delegation_id` の
ユーザー向け別名である。

## 1. 内部 workspace 概念

- ユーザーに「プロジェクト」は見せない(UX は AITuber 型、確定済み決定)。
- workspace は **spawn(委譲)ごと**に独立する。常駐の第二人格用ディレクトリではない。
- 構成: `<data>/workspaces/<soul_id>/` 配下に
  `jobs/<job_id>/`(当該 spawn の作業ディレクトリ・中間成果物)、
  `artifacts/`(交付済み成果物)、`scratch/`(一時)。
- **fs/exec の既定スコープは `jobs/<job_id>/`**。soul 根でも
  `artifacts/` でもない([../tools/capabilities.md §1](../tools/capabilities.md#1-fs-seam))。
  成果物の交付だけが soul の `artifacts/` へコピーする。

## 2. job(P-605)

### ライフサイクル

委譲のライフサイクル([../core/delegation.md §3](../core/delegation.md#3-ライフサイクル))に
job 固有の UX 状態を足す:

```text
created → queued → running → completed
                      ├─→ failed(error_class 付き)
                      ├─→ cancelled
                      └─→ interrupted(D-5。起動時に running から確定)
```

- job は **job レーン**で走る([../core/agent-loop.md §2](../core/agent-loop.md#2-レーンp-504))。
  対話レーンを塞がない。
- job の実体は「目標+ツール集合+ターン列」。ワークフローは
  ステップを持つ job(§4)。
- 進捗: 子の `progress` メッセージのうち `fraction`/`note` を持つものが
  job の進捗になる。`job/progress` ライブイベントで配信する。
  **進捗をユーザーに伝えるのはコンパニオンの発話**であって、
  ステータスバーではない(D-13)。タスクの一覧・カードは詳細画面側に置き、
  表層UIでは会話の中で語られる
  ([../core/visibility.md](../core/visibility.md))。

### 定義フィールド

| フィールド | 型 | 説明 |
|---|---|---|
| `job_id` | `JobId` | 一意識別子 |
| `soul_id` | `SoulId` | 属する soul |
| `title` | string | ユーザー向けの題(「旅行のしおりを作る」) |
| `goal` | string | モデル向けの目標記述 |
| `status` | enum | §ライフサイクル |
| `progress` | `{ fraction?, note? }` | 進捗(任意更新) |
| `workspace_dir` | path | `<ws>/jobs/<job_id>/` |
| `artifact_ids` | [ArtifactId] | 生成した成果物 |
| `created_from` | `turn_id` | 依頼の由来 |
| `error_class` | string? | 失敗分類 |
| `created_at`/`ended_at` | RFC3339 | 時刻 |

### 永続スキーマ

`<data>/companions.db`(記憶・スケジュールと同じ DB)。
`job_id` は public 委譲の `delegation_id` と同じ。internal 委譲は行を持たない。
`GET /jobs` はこの表を読む([../platform/server-api.md](../platform/server-api.md))。
会話ログの `delegation/*` は要約であり、一覧の正ではない(D-9)。

```sql
CREATE TABLE jobs (
  id                 TEXT PRIMARY KEY,  -- JobId = public の delegation_id
  soul_id            TEXT NOT NULL,
  title              TEXT NOT NULL,
  goal               TEXT NOT NULL,
  status             TEXT NOT NULL,     -- created|queued|running|completed|failed|cancelled|interrupted
  progress_fraction  REAL,
  progress_note      TEXT,
  workspace_dir      TEXT NOT NULL,
  error_class        TEXT,
  created_from_turn  TEXT,              -- 依頼元 turn_id
  plan               TEXT,              -- ワークフローのステップ配列(JSON、§4)。線形 job は NULL
  created_at         TEXT NOT NULL,
  ended_at           TEXT
);
CREATE INDEX idx_jobs_soul ON jobs (soul_id, status, created_at DESC);
```

成果物は `artifacts.job_id` で辿る(§5)。状態の更新は委譲の受理・進捗・終端と
同一トランザクションで行う。

### 開始経路

1. **対話からの依頼**: 表層モデルが `delegate.start(goal, mode: public)` を
   呼ぶ([../core/delegation.md §4](../core/delegation.md#4-層間エンベロープと表層--裏層))。
   「引き受けた」発話の後、裏で走る。job 定義(title 等)は
   親が同じ呼び出しで決める。
2. **スケジュールの発火**(§4): `scheduled` origin のターンが job を生む。
3. **ユーザーの直接作成**(UI): タスク画面からの作成(stretch の範囲で v1.0 最小)。

### キャンセルと中断

- `delegate.cancel`(UI では「タスクの中止」): 進行中のターン列を
  停止(協調的キャンセル、[../core/delegation.md §4](../core/delegation.md#4-層間エンベロープと表層--裏層))。
  中間成果物は保持し、`cancelled` で確定。
- **異常終了時**(D-5): `running` だった job は次回起動で
  `interrupted` として検出される。**自動再開はしない。**
  再開すると、どこまで副作用が及んだか分からないまま同じ操作を
  繰り返す危険がある(ファイルを二度書く、メールを二度送る)。
  v1.0 の扱いは以下:

  1. 起動時に `running` のまま残った job を検出する。
  2. workspace の一時ファイルなど、安全に消せるものを片付ける。
  3. コンパニオンがユーザーに報告する
     (「さっきの調べもの、途中で止まっちゃった。やり直す?」)。
  4. 再開するかはユーザーが決める。再開は**新しい job として**始まる。

  `queued` のまま未着手だった job も自動では走らせない(D-5)。
  一覧に `queued` のまま残り、中断報告の材料になる。始めるかはユーザーが決める。

  副作用の記録による安全な再開(effect sandwich)は後継設計
  ([../core/durability.md](../core/durability.md))。

## 3. スケジュール(P-606)

### 永続スキーマ

`<data>/companions.db` に保存(メモリと同じ DB)。

```sql
CREATE TABLE schedules (
  id          TEXT PRIMARY KEY,
  soul_id     TEXT NOT NULL,
  name        TEXT NOT NULL,
  spec        TEXT NOT NULL,      -- cron 式(分 時 日 月 曜日)+ タイムゾーン
  timezone    TEXT NOT NULL,      -- IANA tz。既定はシステム tz
  action_kind TEXT NOT NULL,      -- remind | job | turn
  action_ref  TEXT,               -- job テンプレート/発話内容への参照
  enabled     INTEGER NOT NULL DEFAULT 1,
  important   INTEGER NOT NULL DEFAULT 0,  -- 1 なら quiet hours を貫通
  last_fired  TEXT,
  next_fire   TEXT                -- 計算済み次発火時刻(インデックス)
);
CREATE INDEX idx_sched_next ON schedules (enabled, next_fire);
```

- 発火は **`TurnOrigin::Scheduled` のターン**を起こす。能動発話
  (`TurnOrigin::Proactive`)のパイプラインには入れない
  ([../companion/proactive.md](../companion/proactive.md))。
  origin と発火ゲートの所有者はこの文書。
  `remind` は発話によるリマインド、`job` は job の生成、
  `turn` は通常の発話ターン。
- quiet hours の扱い: `quiet_policy: silent` の時間帯に発火予定の
  スケジュールは**次可能な時刻へ繰り下げ**(リマインドの喪失を防ぐ)。
  `important = 1` の行は貫通する。作成時に付ける列であり、能動発話の
  `mind.proactive.quiet_policy` とは別物。
  能動発話側で同じ発火を再ゲートしない。
- タイムゾーンはスケジュールごとに保持し、サマータイムも
  tz DB で正しく扱う。`next_fire` は発火後に再計算。
- **停止中の発火漏れ**(起動時に `next_fire` が過去):
  `remind` は**1回だけ**即時発火する(時刻の約束を落とさない。繰り越しはしない)。
  `job` / `turn` は走らせない(D-5)。過ぎた枠は捨てて次の `next_fire` を計算し、
  中断報告の材料にする。

### リマインダー(P-607)

- 「X時に〇〇を思い出させて」は、(a) commitment として記憶に記録
  ([../companion/memory.md §2](../companion/memory.md#2-種別体系p-202))、
  (b) 一回限りのスケジュール(`spec` = 指定時刻)を作成、の両方で実現。
- commitment は「約束が存在する」こと(想起・プロンプト載せ)、
  スケジュールは「時刻に発火する」ことを分担する。
- リマインド発火は `remind` action のターンとして、
  「〇〇の時間だよ」の発話+内面の `action_intent` を出す。

## 4. ワークフロー(P-608)

- ユーザーからは1つの job として見える、**ステップ列**を持つ job。
- 実体: job の `goal` に加え、`plan` フィールド
  (ステップの配列: `{ id, description, status }`)を持つ。
- ステップの進行はモデル自身が plan を更新しながら進める
  (`job.plan_write`。plan はハーネス側の job 状態なので、ホスト内で動く。
  [../tools/registry.md §0.1](../tools/registry.md#01-ハーネス機能ツールホスト内))。
  コアはステップ列を詳細画面に表示するだけ(進行の強制はしない)。
- ステップ間の成果物は workspace_dir に蓄積し、最終成果物を
  artifact として交付する。
- ワークフロー専用エンジン(外部 DSL 等)は導入しない。
  「モデルがツールで自己管理する」方式(dsh の workflow の
  シンプルな線形版)で、軽量原則を守る。

## 5. artifact 体系(P-609)

### スキーマ

```sql
CREATE TABLE artifacts (
  id          TEXT PRIMARY KEY,
  soul_id     TEXT NOT NULL,
  job_id      TEXT,
  kind        TEXT NOT NULL,     -- text | markdown | csv | docx | xlsx | pptx | image | pdf | binary
  title       TEXT NOT NULL,
  path        TEXT NOT NULL,     -- workspace 内相対
  mime        TEXT,
  size_bytes  INTEGER,
  created_at  TEXT NOT NULL,
  delivered   INTEGER NOT NULL DEFAULT 0  -- 交付済みか
);
CREATE INDEX idx_art_soul ON artifacts (soul_id, created_at DESC);
```

### 生成と交付

- **生成**: ツールが workspace_dir にファイルを書き、
  `artifact.register`(ハーネス機能ツール)で登録する。
- **段階化**(確定済み): v1.0 は `text/markdown/csv`(テキスト系)を
  実装。`docx/xlsx/pptx/image/pdf` は**形式と接点を v1.0 で固定し、
  実装は後継**(プロバイダ/ツールとして追加できるようにする)。
- **交付**: 完成時に `job/completed` と共に artifact をユーザーに提示。
  クライアントはダウンロード/表示/共有の操作を提供。
  交付は `delivered` フラグで管理し、再提示はしない。
- **巨大成果物**: spill とは別枠(成果物は交付物なので削らない)。
  ただし登録時に容量チェック(`tasks.artifact.max_bytes`)。

## 6. 自動化ルール(後継の接点)

- 「毎朝のブリーフィング」のような複合自動化は、
  スケジュール(`job` action)+ skill([skills.md](skills.md))の
  組合せで表現する。専用のルールエンジン DSL は導入しない
  (軽量原則)。
- 外部サービスの受信イベント(カレンダー変更等)をトリガーにする
  「イベント駆動の自動化」は後継。受信元は MCP サーバーになる(D-23)ので、
  形式上の接点として、スケジュールの `spec` に `on_event:<source>` を
  予約する。

## 7. 設定キー

数値は実装しながら決める(D-29)。

| キー | 説明 |
|---|---|
| (並行上限) | `harness.delegation.max_active` に統合([core/delegation.md](../core/delegation.md))。job は委譲なので独自キーを持たない |
| `tasks.jobs.workspace_root` | workspace 根(既定は `<data>/workspaces`) |
| `tasks.schedules.timezone` | 既定タイムゾーン(既定はシステム tz) |
| `tasks.artifact.max_bytes` | 成果物単一上限 |
| `tasks.artifact.kinds` | v1.0 の生成有効種別(テキスト系のみ) |

## 8. 障害モード

| 障害 | 挙動 |
|---|---|
| job のターン列が失敗 | ステップ予算内で再試行。不能なら `failed` で確定し、**報告ターン**が親の対話レーンに届く([../core/delegation.md §6](../core/delegation.md#6-報告ターンp-521)) |
| 子が親に質問を送った | 親への wake は報告ターンで届き、**親がユーザーに口頭で尋ねる**(ask-user 経路の転送)。詳細画面のタスク一覧では「確認待ち」表示 |
| 異常終了時の running job | `interrupted` として検出し、片付けて報告。自動再開はしない(§2 キャンセルと中断) |
| 異常終了時の queued job | `queued` のまま残し、中断報告の材料にする。自動では走らせない(D-5) |
| スケジュールの発火漏れ(停止中) | `remind` は1回だけ即時発火。`job` / `turn` は走らせず、過ぎた枠を捨てて次を計算し、中断報告の材料にする(D-5) |
| artifact 登録の容量超過 | 登録拒否+job に報告。モデルは分割/圧縮を判断 |
| quiet hours 中の remind | 繰り下げ(`silent`)。`important` は貫通 |

---

- 次: [skills.md](skills.md)
