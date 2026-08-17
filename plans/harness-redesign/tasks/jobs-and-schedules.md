# ジョブ・スケジュール・成果物

> 実現する要件: **P-605**(job)、**P-606**(スケジュール)、**P-607**(リマインダー)、
> **P-608**(ワークフロー)、**P-609**(artifact)。

対話の外で進む「仕事」の全体。ユーザーには「おつかい」として見える。
job は**非同期委譲の public モード**である——実体・ライフサイクル・
親↔子メッセージ・耐久性は [../core/delegation.md](../core/delegation.md) が
定義し、この文書はユーザー向けの job 面(定義・スケジュール・
ワークフロー・成果物)を定義する。`job_id` は `delegation_id` の
ユーザー向け別名である。

## 1. 内部 workspace 概念

- ユーザーに「プロジェクト」は見せない(UX は AITuber 型、確定済み決定)。
- 内部では、job・成果物・ファイルツールが根を置く
  **workspace** を soul ごとに持つ。
- 構成: `<data>/workspaces/<soul_id>/` 配下に
  `jobs/<job_id>/`(作業ディレクトリ・中間成果物)、
  `artifacts/`(交付済み成果物)、`scratch/`(一時)。
- fs/exec ツールの既定スコープはこの根
  ([../tools/capabilities.md §1](../tools/capabilities.md))。

## 2. job(P-605)

### ライフサイクル

委譲のライフサイクル([../core/delegation.md §3](../core/delegation.md#3-ライフサイクル))に
job 固有の UX 状態を足す:

```text
created → queued → running → completed
                      ├─→ failed(error_class 付き)
                      └─→ cancelled
```

- job は **job レーン**で走る([../core/agent-loop.md §2](../core/agent-loop.md#2-レーンp-504))。
  対話レーンを塞がない。
- job の実体は「目標+ツール集合+ターン列」。ワークフローは
  ステップを持つ job(§4)。
- 進捗: 子の `progress` メッセージのうち `fraction`/`note` を持つものが
  job の進捗になる。`job/progress` ライブイベントで配信し、
  クライアントは「おつかい」のカードとして表示
  (P-613/visibility の規則に従い、生内容は出さない)。

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

### 開始経路

1. **対話からの依頼**: 表層モデルが `delegate.start(goal, mode: public)` を
   呼ぶ([../core/delegation.md §4](../core/delegation.md#4-層間エンベロープと表層--裏層))。
   「引き受けた」発話の後、裏で走る。job 定義(title 等)は
   親が同じ呼び出しで決める。
2. **スケジュールの発火**(§4): `scheduled` origin のターンが job を生む。
3. **ユーザーの直接作成**(UI): 「おつかい」画面からの作成(stretch の範囲で v1.0 最小)。

### キャンセルと中断

- `delegate.cancel`(UI では「おつかいの中止」): 進行中のターン列を
  停止(協調的キャンセル、[../core/delegation.md §4](../core/delegation.md#4-層間エンベロープと表層--裏層))。
  中間成果物は保持し、`cancelled` で確定。
- アプリ終了時は `queued`/`running` の job を永続化し、
  次回起動で `queued` から再開する(running 中のターンは破棄して再実行)。

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
  last_fired  TEXT,
  next_fire   TEXT                -- 計算済み次発火時刻(インデックス)
);
CREATE INDEX idx_sched_next ON schedules (enabled, next_fire);
```

- 発火は `TurnOrigin::Scheduled` のターンを起こす
  ([../companion/proactive.md §3](../companion/proactive.md#3-ソースと優先度))。
  `remind` は発話によるリマインド、`job` は job の生成、
  `turn` は通常の発話ターン。
- quiet hours の扱い: `quiet_policy: silent` の時間帯に発火予定の
  スケジュールは**次可能な時刻へ繰り下げ**(リマインドの喪失を防ぐ)。
  `important`(P-606 相当)の指定があるものは貫通。
- タイムゾーンはスケジュールごとに保持し、サマータイムも
  tz DB で正しく扱う。`next_fire` は発火後に再計算。

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
  (`todo_write` 相当の内部ツールで管理)。
  コアはステップ列を UI に表示するだけ(進行の強制はしない)。
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
  `artifact.register` 内部ツールで登録する。
- **段階化**(確定済み): v1.0 は `text/markdown/csv`(テキスト系)を
  実装。`docx/xlsx/pptx/image/pdf` は**形式と接点を v1.0 で固定し、
  実装は後継**(プロバイダ/ツールとして追加できるようにする)。
- **交付**: 完成時に `job/completed` と共に artifact をユーザーに提示。
  クライアントはダウンロード/表示/共有の操作を提供。
  交付は `delivered` フラグで管理し、再提示はしない。
- **巨大成果物**: spill とは別枠(成果物は交付物なので削らない)。
  ただし登録時に容量チェック(単一 100 MiB 上限、`tasks.artifact.max_bytes`)。

## 6. 自動化ルール(後継の接点)

- 「毎朝のブリーフィング」のような複合自動化は、
  スケジュール(`job` action)+ skill([skills.md](skills.md))の
  組合せで表現する。専用のルールエンジン DSL は導入しない
  (軽量原則)。
- コネクタ受信イベント(カレンダー変更等)をトリガーにする
  「イベント駆動の自動化」は後継(M1/M2 参照)。形式上の接点として、
  スケジュールの `spec` に `on_event:<source>` を予約する。

## 7. 設定キーと既定値

| キー | 既定 | 説明 |
|---|---|---|
| (並行上限) | `harness.delegation.max_active`(既定 4) | [core/delegation.md §10](../core/delegation.md#10-設定キーと既定値) に統合。job は委譲なので独自キーを持たない |
| `tasks.jobs.workspace_root` | `<data>/workspaces` | workspace 根 |
| `tasks.schedules.timezone` | `system` | 既定タイムゾーン |
| `tasks.artifact.max_bytes` | `104857600` | 成果物単一上限(100 MiB) |
| `tasks.artifact.kinds` | `text,markdown,csv` | v1.0 の生成有効種別 |

## 8. 障害モード

| 障害 | 挙動 |
|---|---|
| job のターン列が失敗 | ステップ予算内で再試行。不能なら `failed` で確定し、**報告ターン**が親の対話レーンに届く([../core/delegation.md §6](../core/delegation.md#6-報告ターンp-521)) |
| 子が親に質問を送った | job カードに「確認待ち」表示。親への wake は報告ターンで届き、親がユーザーに尋ねる(ask-user 経路の転送) |
| アプリ終了時の running job | 永続化して次回再開。中断位置のターンは再実行 |
| スケジュールの発火漏れ(停止中) | 起動時に `next_fire` が過去になっているものは**1回だけ**即時発火(繰り越しはしない) |
| artifact 登録の容量超過 | 登録拒否+job に報告。モデルは分割/圧縮を判断 |
| quiet hours 中の remind | 繰り下げ(`silent`)。`important` は貫通 |

---

- 次: [skills.md](skills.md)
