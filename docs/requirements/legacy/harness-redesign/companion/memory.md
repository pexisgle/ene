# メモリ

> 実現する要件: **P-201..P-213**(記憶と人格)、P-910 のローカル保持。

## 1. 責務と責務外

- **責務**: 記憶のスキーマ・種別、ボールトと共有記憶プール、想起(検索)、
  抽出・裁定・忘却・内省、ユーザーによる閲覧/編集/削除。
- **責務外**: プロンプトへの載せ方([../core/context-assembly.md](../core/context-assembly.md) の
  `memory.*` Source)、会話ログそのもの
  ([../core/session-log.md](../core/session-log.md))。

記憶はセッションログとは**別の永続化**を持つ。ログは「何が起きたか」、
記憶は「何を覚えているか」。記憶の変更には独自の監査ジャーナルが付く。

記憶は**通常のテーブル構造**で持ち、イベントソーシングの対象ではない(D-9)。
追記ログにすると実装量が跳ね上がるのに、得るものが少ない——記憶に必要な
「変更を追える」は `memory_journal` で足りる。

## 2. 種別体系(P-202)

| kind | 内容 | 例 |
|---|---|---|
| `episodic` | 出来事の記憶 | 「先週一緒に旅行の計画を立てた」 |
| `semantic` | 事実・知識 | 「ユーザーの猫の名はタマ」 |
| `user_profile` | ユーザーの長期プロファイル(P-208) | 呼称・好み・生活リズム |
| `preference` | 応答の好み | 「返事は短めが好き」 |
| `commitment` | 約束・未完了のタスク | 「明日3時に会議の準備を促す」 |

種別とは**直交する軸**として、各記憶は `scope` を持つ(§4)。

## 3. スキーマ

データベースは `<data>/companions.db`(soul の状態と同じ DB、
[soul-and-affect.md §2](soul-and-affect.md))。

```sql
CREATE TABLE memories (
  id            TEXT PRIMARY KEY,      -- MemoryId (UUIDv7)
  soul_id       TEXT NOT NULL,         -- 書いた soul。scope=shared でも出所として保持
  scope         TEXT NOT NULL,         -- private | shared (§4)
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
CREATE INDEX idx_mem_scope ON memories (scope, soul_id, kind);
CREATE INDEX idx_mem_title ON memories (soul_id, title);
CREATE VIRTUAL TABLE mem_fts USING fts5(title, content, content='memories');
```

`scope` を独立した列にして、`soul_id` は常に「書いた soul」を指す。
共有記憶に所有者不明の行を作らないためで、これにより
「誰が最初に知ったか」を失わずに全員が参照できる。

- ベクトル検索は sqlite-vec の仮想テーブル(`mem_vec`)を
  別テーブルとして持ち、`memories.id` と対にする。
  埋め込みプロバイダがない構成ではベクトル列を作らず、
  検索は語彙+recency に退化する(機能は落ちるが動作する)。
- 記憶の**変更履歴**は `memory_journal` テーブル(append-only)に
  `created/updated/forgotten/superseded/restored` を記録(P-210 の監査)。

## 4. スコープと共有記憶プール(P-207 / D-7)

記憶は既定でコンパニオンごとに分かれる。ただし全部を分けると、
ユーザーの名前や生活リズムを人格ごとに覚え直させることになり、不自然である。
逆に全部を共有すると、別人格である意味が消える。境界は次のとおり。

| scope | 読める範囲 | 入るもの |
|---|---|---|
| `private` | 書いた soul のみ | 体験、関係性、感情、その soul とのやりとり固有の事柄 |
| `shared` | すべての soul | ユーザーに関する事実と、抽出器が重要と判断した事項 |

### 4.1 誰が scope を決めるか

**記憶抽出の補助LLM が、抽出時に付与する**(§6)。種別による機械的な
振り分けではない。

種別で決めない理由は、同じ `episodic` でも「ユーザーが転職した」は
全員が知っているべきで、「一緒に映画を見て泣いた」はその soul のものだから。
種別は「何の記憶か」を表すが、「誰が知っているべきか」は表さない。

判定を誤ったときは、ユーザーが閲覧画面から scope を変更できる(§12)。

### 4.2 読み取り側の扱い

共有記憶は**出所(書いた soul)を保持する**が、想起の際に出所を意識させない。
参照する soul は、それを自分の知識として扱う。

「B が A から聞いた話として話す」ようにはしない。ユーザーから見れば
自分が一度言ったことであり、誰に言ったかを覚えているのはユーザーの側だからである。
出所は診断と、削除時の追跡のために残す。

### 4.3 明示的な書き込み(P-212)

抽出器の判定を待たず、soul が明示的に共有プールへ書き込むツールを持つ
(`memory.write_shared`。[../tools/registry.md §0.1](../tools/registry.md#01-ハーネス機能ツールホスト内))。
ユーザーが「これはみんなに覚えておいて」と言った場合の経路。

## 5. 想起(P-203 / D-18)

想起の経路は**2つあり、両方を持つ**。

| 経路 | 起動 | 用途 |
|---|---|---|
| 自動想起 | ターン開始の `before_turn` | 会話に必要な背景を、モデルが求める前に載せる |
| 想起ツール | モデルの tool call | 「あのとき何て言ったっけ」を明示的に引く |

自動想起だけだと、モデルが必要としている記憶を先読みで当てなければならず、
外したときに取り返せない。ツールだけだと、モデルは「自分が何を知らないか」を
知らないので呼ぶきっかけがない。両方あって初めて成立する。

想起ツールは**ハーネス機能ツール** `memory.recall` であり、ホスト内で実行される
(D-10、[../tools/registry.md §0.1](../tools/registry.md#01-ハーネス機能ツールホスト内))。
記憶ストアと密結合するため、プロセス境界の向こうに置かない。

### 5.1 検索の手順

1. **計画**: 新規発話から意図を検出し、検索計画(対象種別・件数予算)を作る。
2. **範囲**: 自分の `private` 記憶と、全体の `shared` 記憶を対象にする。
   他 soul の `private` は検索に入らない。
3. **ハイブリッドスコア**: 次の重み付き和で並べる。重みは設定であり、
   実装しながら調整する(D-29)。

   | 要素 | 内容 |
   |---|---|
   | ベクトル類似 | 埋め込みがある場合のコサイン類似 |
   | 語彙一致 | FTS の BM25 を正規化したもの |
   | 新しさ | 経過時間による減衰 |
   | 重要度 | `salience` |
   | 関係性 | trust / affinity との一致 |

   埋め込みプロバイダがない構成ではベクトル項を 0 にし、語彙項へ再配分する
   (機能は落ちるが動作する)。
4. **多様化**: MMR で上位から選択し、同じ話題の重複を避ける。
5. **更新**: 採用された記憶は `last_access`/`access_count` を更新
   (忘却への耐性になる)。
6. **矛盾・失効**: `superseded_by` がある記憶は返さない。
   `expires_at` 超過の commitment は「期限切れ」として別 Source に回す。

## 6. ターン後の抽出(P-204)

`Terminal` の後、バックグラウンドで実行(対話を止めない)。
これは**ハーネス内部処理**であり、モデルからは呼べない(D-10)。

1. **決定論抽出**: 約束(commitment の言い回し)、ユーザー声明
   (「私の名前は」「好きなのは」)、ツールの事実(tool_grounding)。
2. **補助LLM による抽出**: 候補を列挙させる。1候補あたり
   kind / title / content / confidence / salience に加えて、
   **`scope: private | shared`** を出させる(D-7)。
   スコープ判定を抽出と同じ呼び出しに含めるのは、判断材料が同じだからである
   ——「この事実はユーザー自身についてのものか、この soul との関係の中の
   ものか」は、抽出時に一度考えれば済む。
3. **候補キュー**: 抽出結果は即採用ではなく**候補**になる。
   confidence が低い候補は承認キューへ(P-211)。
   `scope: shared` の候補は、`private` より慎重に扱う——
   誤って共有すると全人格に伝播するので、承認の閾値を別に持つ。

## 7. 裁定(P-205)

候補を確定記憶にする判断。

1. **重複検知**: タイトル正規化の一致、または埋め込み類似が閾値以上のものは
   既存記憶の**更新**(内容を併合、access を継承)。
2. **矛盾処理**: 既存と内容が矛盾する場合、新記憶を採用し
   既存に `superseded_by` を付けて置換。置換は journal に記録。
3. **スコアリング**: confidence/salience を最終化。salience が低すぎる
   候補は採用しない(ノイズの抑止)。
4. **スコープの確定**: `shared` 候補は、同じ内容の `private` 記憶が
   他 soul に既にあれば統合する。同じ事実を人格ごとに重複して持たない。
5. **承認キュー**(P-211): 機密らしい(第三者情報・連絡先・金銭)、
   または低信頼の候補は承認キューへ。承認 plane のポップアップで
   「覚えていい?」を確認。拒否は候補の破棄(journal に記録)。

## 8. 忘却と減衰(P-206)

- **減衰**: `salience` はアクセスがない期間に応じて減衰する。
  減衰の速さは種別ごとに違い、`preference` は減衰しない。
- **忘却候補**: salience が閾値を下回った記憶を忘却候補にする。
- **忘却の確定**: 既定では、忘却候補は**削除前にユーザー確認**を通す
  (記憶の喪失は必ず可視にする)。確認なしの即削除も設定できる。
- `commitment` は忘却の対象外(期限管理のみ)。
- `shared` 記憶の忘却は、いずれかの soul がアクセスしていれば減衰しない。
  全員が使わなくなって初めて候補になる。

## 9. 内省(P-213, stretch)

- 定期的に記憶を俯瞰し、`episodic` の要点を `semantic` に昇格、
  陳腐化した記憶の整理提案を作る。scope の見直しもここで提案する。
- 内省の結果も候補キューを通る(自動書き換えはしない)。

## 10. ユーザーによる閲覧/編集/削除(P-210)

- 設定 UI の「記憶」画面: 種別/scope/日付/キーワードで一覧、
  内容の編集、**scope の変更**、削除。削除は tombstone(journal 記録)であり、
  完全消去は猶予期間の後。
- 対話からの削除(「〇〇は忘れて」)も同じ API を通り、
  journal に `user_request` として残る。
- 削除された記憶のセッションログ上の痕跡は `redaction` で秘匿
  ([../core/session-log.md §3.5](../core/session-log.md#35-秘匿と-tombstone))。
- `shared` 記憶の削除は全 soul に効く。削除確認でその旨を示す。

## 11. 容量と整理

記憶件数と DB サイズにソフト上限を持ち、超過時は忘却候補の生成を加速する。
具体的な値は実装しながら決める(D-29)。

## 12. 設定キー

具体的な数値は実装しながら決める(D-29)。

| キー | 説明 |
|---|---|
| `mind.recall.budget` | 想起件数予算 |
| `mind.recall.weights.*` | ハイブリッドスコアの重み |
| `mind.recall.mmr_lambda` | 多様化係数 |
| `mind.memory_approval.require_approval` | 低信頼候補の承認要否 |
| `mind.memory_approval.confidence_threshold` | 自動採用の信頼閾値 |
| `mind.memory_approval.shared_confidence_threshold` | `scope: shared` の自動採用閾値(private より高い) |
| `mind.forgetting.mode` | 確認あり / 即削除 |
| `mind.reflection.interval` | 内省周期(stretch) |

## 13. 障害モード

| 障害 | 挙動 |
|---|---|
| 埋め込みプロバイダ不能 | ベクトル検索をスキップし語彙検索で継続+警告。抽出時の埋め込み付与も保留(後で付与) |
| 抽出の失敗 | 候補なしで終了。決定論抽出のみ確定。次ターンで再試行 |
| scope 判定の欠落 | `private` に倒す。誤って共有するより、共有し損ねるほうが回復しやすい |
| 承認キューの滞留 | 未処理の候補は salience を減衰させ、猶予期間の後に破棄(journal 記録) |
| DB 破損 | sessions.db と同じ復元フロー([../platform/process-model.md](../platform/process-model.md)) |

---

- 前: [inner-channel.md](inner-channel.md) / 次: [proactive.md](proactive.md)

