# メモリ

> 実現する要件: **P-201..P-213**(記憶と人格)、P-910 のローカル保持。

## 1. 責務と責務外

- **責務**: 記憶のスキーマ・種別、ボールトと共有スペース、想起(検索)、
  抽出・裁定・忘却・内省、ユーザーによる閲覧/編集/削除。
- **責務外**: プロンプトへの載せ方([../core/context-assembly.md](../core/context-assembly.md) の
  `memory.*` Source)、会話ログそのもの
  ([../core/session-log.md](../core/session-log.md))。

記憶はセッションログとは**別の永続化**を持つ。ログは「何が起きたか」、
記憶は「何を覚えているか」。記憶の変更には独自の監査ジャーナルが付く。

## 2. 種別体系(P-202)

| kind | 内容 | 例 |
|---|---|---|
| `episodic` | 出来事の記憶 | 「先週一緒に旅行の計画を立てた」 |
| `semantic` | 事実・知識 | 「ユーザーの猫の名はタマ」 |
| `user_profile` | ユーザーの長期プロファイル(P-208) | 呼称・好み・生活リズム |
| `preference` | 応答の好み | 「返事は短めが好き」 |
| `commitment` | 約束・未完了のタスク | 「明日3時に会議の準備を促す」 |
| `shared_note` | 共有スペース内のメモ | 「家の買い物リスト」 |

## 3. スキーマ

データベースは `<data>/companions.db`(soul の状態と同じ DB、
[soul-and-affect.md §2](soul-and-affect.md))。

```sql
CREATE TABLE memories (
  id            TEXT PRIMARY KEY,      -- MemoryId (UUIDv7)
  soul_id       TEXT NOT NULL,         -- ボールトの所有者
  space_id      TEXT,                  -- NULL = 専用ボールト。非NULL = 共有スペース
  kind          TEXT NOT NULL,         -- §2 の種別
  title         TEXT NOT NULL,         -- 短い題(重複検知の主キー相当)
  content       TEXT NOT NULL,
  embedding     BLOB,                  -- f32 ベクトル(埋め込みがある場合)
  confidence    REAL NOT NULL DEFAULT 0.5,
  salience      REAL NOT NULL DEFAULT 0.5,  -- 重要度
  source        TEXT NOT NULL,         -- extraction | user_stated | tool | import | shared
  source_seq    INTEGER,               -- 由来セッションイベントへの参照(任意)
  created_at    TEXT NOT NULL,
  last_access   TEXT NOT NULL,
  access_count  INTEGER NOT NULL DEFAULT 0,
  superseded_by TEXT,                  -- 矛盾置換されたときの後継
  expires_at    TEXT                   -- commitment 等の期限
);
CREATE INDEX idx_mem_soul ON memories (soul_id, space_id, kind);
CREATE INDEX idx_mem_title ON memories (soul_id, title);
CREATE VIRTUAL TABLE mem_fts USING fts5(title, content, content='memories');

CREATE TABLE spaces (
  id        TEXT PRIMARY KEY,
  name      TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL
);
CREATE TABLE space_members (
  space_id  TEXT NOT NULL,
  soul_id   TEXT NOT NULL,
  role      TEXT NOT NULL,             -- reader | writer
  PRIMARY KEY (space_id, soul_id)
);
```

- ベクトル検索は sqlite-vec の仮想テーブル(`mem_vec`)を
  別テーブルとして持ち、`memories.id` と対にする。
  埋め込みプロバイダがない構成ではベクトル列を作らず、
  検索は語彙+recency に退化する(機能は落ちるが動作する)。
- 記憶の**変更履歴**は `memory_journal` テーブル(append-only)に
  `created/updated/forgotten/superseded/restored` を記録(P-210 の監査)。

## 4. 共有スペース(P-207)

- 既定では soul は**自分のボールトしか読めない**。検索も自分の
  `soul_id` に限定される。
- 共有スペースは命名領域(`spaces`)で、`space_members` に
  `reader`/`writer` として参加した soul のみが見える。
- 書き込みは `writer` のみ。`shared_note` 種別として保存し、
  `soul_id` は書き込み元(来歴の保持)。
- 「メモを渡す」ツール(P-212)は、共有スペースへの `shared_note`
  書き込みとして実装する。渡された側は想起時にそれが読める。
- スペースの作成・参加はユーザー操作(設定/対話から)で、
  soul が勝手に共有を作れない。

## 5. 想起(P-203)

ターン開始の `before_turn` で実行。

1. **計画**: 新規発話から意図を検出し、検索計画(対象種別・件数予算)
   を作る。予算は既定 8 件(`mind.recall.budget`)。
2. **ハイブリッドスコア**:
   ```text
   score = w_vec * cosine_sim          (埋め込みがある場合、w_vec = 0.45)
         + w_lex * lexical_overlap      (FTS の BM25 正規化、w_lex = 0.25)
         + w_rec * recency_decay        (半減期 30 日、w_rec = 0.15)
         + w_sal * salience             (w_sal = 0.10)
         + w_rel * relationship_match   (trust/affinity の寄与、w_rel = 0.05)
   ```
   埋め込みがない構成では w_vec=0、w_lex=0.60 に再配分。
3. **多様化**: MMR(λ=0.7)で上位から選択し、同じ話題の重複を避ける。
4. **更新**: 採用された記憶は `last_access`/`access_count` を更新
   (忘却への耐性になる)。
5. **矛盾・失効**: `superseded_by` がある記憶は返さない。
   `expires_at` 超過の commitment は「期限切れ」として別 Source に回す。

## 6. ターン後の抽出(P-204)

`Terminal` の後、バックグラウンドで実行(対話を止めない)。

1. **決定論抽出**: 約束(commitment の言い回し)、ユーザー声明
   (「私の名前は」「好きなのは」)、ツールの事実(tool_grounding)。
2. **LLM 抽出**: `ai.tasks.extract` のプロバイダで、候補
   (kind/title/content/confidence/salience)を列挙させる。
3. **候補キュー**: 抽出結果は即採用ではなく**候補**になる。
   `mind.memory_approval.require_approval`(既定 `true`)のとき、
   confidence が閾値(0.8)未満の候補は承認キューへ(P-211)。

## 7. 裁定(P-205)

候補を確定記憶にする判断。

1. **重複検知**: タイトル正規化の一致、または埋め込み類似 0.92 以上
   は既存記憶の**更新**(内容を併合、access を継承)。
2. **矛盾処理**: 既存と内容が矛盾する場合、新記憶を採用し
   既存に `superseded_by` を付けて置換。置換は journal に記録。
3. **スコアリング**: confidence/salience を最終化。salience 0.2 未満の
   候補は採用しない(ノイズの抑止)。
4. **承認キュー**(P-211): 機密らしい(第三者情報・連絡先・金銭)、
   または低信頼の候補は承認キューへ。承認 plane のポップアップで
   「覚えていい?」を確認。拒否は候補の破棄(journal に記録)。

## 8. 忘却と減衰(P-206)

- **減衰**: `salience` はアクセスがない期間に応じて減衰
  (半減期: `episodic` 90 日、`semantic`/`user_profile` 365 日、
  `preference` は減衰しない)。
- **忘却候補**: salience が 0.1 を下回った記憶を忘却候補にする。
- **忘却の確定**: `mind.forgetting.mode` が `auto` のとき、
  忘却候補は**削除前にユーザー確認**(ポップアップ、週次まとめ)。
  `silent` のとき即削除。既定は `auto`(記憶の喪失は必ず可視に)。
- `commitment` は忘却の対象外(期限管理のみ)。

## 9. 内省(P-213, stretch)

- 定期(既定: 週次)に記憶を俯瞰し、`episodic` の要点を `semantic` に
  昇格、陳腐化した `shared_note` の整理提案を作る。
- 内省の結果も候補キューを通る(自動書き換えはしない)。

## 10. ユーザーによる閲覧/編集/削除(P-210)

- 設定 UI の「記憶」画面: 種別/日付/キーワードで一覧、
  内容の編集、削除。削除は tombstone(journal 記録)であり、
  完全消去は 30 日後。
- 対話からの削除(「〇〇は忘れて」)も同じ API を通り、
  journal に `user_request` として残る。
- 削除された記憶のセッションログ上の痕跡は `redaction` で秘匿
  ([../core/session-log.md §3.5](../core/session-log.md#35-秘匿と-tombstone))。

## 11. 容量目標と整理

- 1 soul あたりの記憶件数のソフト上限: 10 万件。超過時は
  忘却候補の生成を加速(閾値を 0.2 に引き上げ)。
- DB サイズの目標: 埋め込み込みで 1 soul あたり 500 MiB 未満。

## 12. 設定キーと既定値

| キー | 既定 | 説明 |
|---|---|---|
| `mind.recall.budget` | `8` | 想起件数予算 |
| `mind.recall.mmr_lambda` | `0.7` | 多様化係数 |
| `mind.memory_approval.require_approval` | `true` | 低信頼候補の承認要否 |
| `mind.memory_approval.confidence_threshold` | `0.8` | 自動採用の信頼閾値 |
| `mind.forgetting.mode` | `auto` | `auto`(確認あり)/ `silent` |
| `mind.reflection.interval` | `weekly` | 内省周期(stretch) |

## 13. 障害モード

| 障害 | 挙動 |
|---|---|
| 埋め込みプロバイダ不能 | ベクトル検索をスキップし語彙検索で継続+警告。抽出時の埋め込み付与も保留(後で付与) |
| 抽出の失敗 | 候補なしで終了。決定論抽出のみ確定。次ターンで再試行 |
| 承認キューの滞留 | 48 時間未処理の候補は salience を減衰させ、7 日で破棄(journal 記録) |
| DB 破損 | sessions.db と同じ復元フロー([../platform/process-model.md](../platform/process-model.md)) |

---

- 前: [inner-channel.md](inner-channel.md) / 次: [proactive.md](proactive.md)

