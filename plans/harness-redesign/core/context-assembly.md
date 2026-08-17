# コンテキスト組立

> 実現する要件: **P-505**(コンテキスト組立)、**P-506**(compaction)、
> **P-507/P-614**(spill/巨大出力)。

モデル要求に載せるコンテキストは、**System Context**(指示・状況)と
**Session History**(会話履歴)に分離する(opencode に倣う)。
System Context は Context Source のレジストリから組み立てられ、
Session History は [session-log.md](session-log.md) の投影から得られる。
組立機構(レジストリ・epoch・途中システムメッセージ・compaction)は
両層で共有する。載せる Source と履歴は**層の専用実装が登録する**
([../product/vision.md](../product/vision.md#51-コアデーモン内の2層)、
[agent-loop.md §2.1](agent-loop.md#21-エージェント実装の登録表p-522))。
裏層は親の対話履歴を既定で含めず、委譲ブリーフ(goal+excerpt)を使う
([delegation.md](delegation.md) §7)。

## 1. 責務と責務外

- **責務**: Context Source レジストリ、epoch の管理、途中システムメッセージの
  生成と取り込み、履歴バジェットと compaction、spill、トークンメーター。
- **責務外**: 何が Source として登録されるかの政策(記憶・スキル・workspace 等は
  各ドメインが登録する)。ログへの永続化そのものは session-log の責務。

## 2. Context Source

1つの Context Source は次の組である。

| 要素 | 説明 |
|---|---|
| `key` | 安定キー(`memory.recall`、`workspace.instructions` 等)。命名規則: `<domain>.<name>` |
| `scope` | `global`(全 soul)・`soul:<id>`・`session:<id>`。狭いスコープが優先 |
| `codec` | 値の型と MessagePack codec |
| `loader` | 値を観測する非同期関数。失敗してもよい(Unavailable) |
| `render_baseline` | epoch 開始時の描画(純関数) |
| `render_update` | 変更時の描画(純関数)。省略時は baseline を再使用 |
| `render_removal` | 動的 Source が消えたときの描画(任意) |
| `max_tokens` | この Source の上限(ソフトキャップ) |

レジストリはスコープごとの挿入順リストであり、組み立ては
**固定の描画順**に従う。層が登録していない Source はスキップする。

```text
描画順(System Context、登録されているものだけ描画):
 1. platform_contract        (出力契約・安全規則)           両層
 2. identity_kernel          (キャラ定義。予算: 窓の 1/8、400..4000 トークンでクランプ)   両層
 3. character_state          (感情要約)                     表層
 4. memory.semantic          (想起した記憶)                 表層
 5. memory.user_profile                                     表層
 6. memory.commitments       (有効な約束)                   表層
 7. workspace.context        (当該 spawn の job 根)         裏層
 8. skills.active            (読み込み済みスキル)           裏層
 9. mcp.resources            (MCP resources からの変換分)   裏層
10. scene_state              (転がり要約)                   両層
11. inner_recent             (内面自己参照窓)               表層
12. style_examples           (発話スタイル例)               表層
13. interruption_note        (中断後のみ)                   表層
14. delegation.active        (進行中委譲の要約)             表層
15. delegation.brief         (goal+excerpt)                 裏層
```

キー名 `delegation.active` が正。`delegate.status` ツールは同じレジスタを読む。

### 2.1 層ごとの Source 集合

| Source | 表層 | 裏層 |
|---|---|---|
| `platform_contract` | 必須 | 必須(作業向け契約。発話チャネル禁止を含む) |
| `identity_kernel` | 必須 | 必須(同じ soul) |
| `character_state` | 必須 | なし |
| `memory.*` | 必須 | なし(親が excerpt で選んだ記憶だけが brief に入る) |
| `workspace.context` | なし | 必須(当該 spawn の job ディレクトリ) |
| `skills.active` | なし(カタログ名の参照のみ identity 経由) | 読み込み済み分 |
| `mcp.resources` | なし | 当該委譲が購読した分 |
| `inner_recent` | 必須 | なし |
| `style_examples` | 任意 | なし |
| `delegation.active` | 必須 | なし |
| `delegation.brief` | なし | 必須 |

### Unavailable の扱い

loader が失敗(Unavailable)した Source は、**直前の有効な状態を保持**し、
更新も削除通知も出さない。初回ロード前はスキップ(除外のまま)。
これは「一時的な観測不能でコンテキストを壊さない」ための規則であり、
opencode の Unavailable Context に倣う。

## 3. Context Epoch と baseline

- **Epoch** は、1回描画された System Context がプロバイダキャッシュの
  基準点として有効な期間。開始時に `context/epoch` イベントを記録する。
- Epoch が終わるのは: コンパクション完了・セッション移動・
  非互換遷移(キャラ差し替え等)。
- Epoch 内の変更は、**途中システムメッセージ**として Session History 側に
  時系列で追記される(opencode の Mid-Conversation System Message)。
  System Context の baseline 自体は epoch 中不変。

## 4. 途中システムメッセージの取り込み

状態の「押し付け」はしない。**安全なプロバイダターン境界**
(モデル要求の直前、入力昇格とツール確定の後)で初めて変更を取り込む。

手順:

1. ターン境界で、各 Source の現在値と**Context Snapshot**(モデルに最後に
   見せた値の JSON 状態)を比較する。
2. 変更のある Source は `render_update` で描画し、1つの
   `context/system_message` に結合して記録(複数 Source の変更は1メッセージに
   まとめる)。
3. Snapshot は対応するメッセージ記録と**原子的に**進める。
4. 境界では、ユーザー入力・ツール結果が先、システムメッセージは後。

## 5. spill

ツール出力が履歴に残すには大きすぎる場合の処置。

| パラメータ | 既定 | 説明 |
|---|---|---|
| `harness.tool_output.soft_limit_tokens` | `8000` | これを超える出力は spill 候補 |
| `harness.tool_output.hard_limit_tokens` | `32000` | これを超える出力は必ず spill |
| spill の保存先 | `<data>/spill/<sha256>` | content-addressed。`spill_objects` テーブルと対 |

手順: ツール結果が閾値超え → 全量を spill ファイルへ書き出し →
履歴には**要約ブロック+ spill 参照**(sha256・サイズ)を残す →
`tool/spill` イベント記録。モデルが続きを必要とする場合は
`spill.read` ツール(範囲読み)で取得する。

## 6. トークンメーターと窓計算

- **有効窓** = プロバイダ公称窓 − 応答予約(既定 4096) − 安全マージン(既定 窓の 10%)。
- 配分は固定予算制ではなく**優先度付きパッキング**: 描画順の上位から
  ソフトキャップ内で詰め、溢れたら下位の Source を詰める。
  ただし `platform_contract`・`identity_kernel`・直近の履歴は保護対象。
- トークン数はプロバイダの tokenizer が使える場合はそれ、
  使えない場合は推定(4 文字/トークンの保守推定、CJK は 1.5 文字/トークン)。
- 履歴のトークン計上には `assistant/thinking` を含む(記録され、
  プロバイダ規約で返送されるため)。thinking が窓の主因になる構成では
  compaction(+prune、§7)がこれを畳む
  ([visibility.md §3](visibility.md#3-thinking-の扱い))。

## 7. compaction(P-506)

- **起動**: 履歴トークンが有効窓の `compaction.trigger_ratio` を超えたとき。
  **先回り実行**でターン中のストールを避ける。
- **内容**: 古いメッセージ群を**転がり scene summary** に畳む。
  直近 `compaction.keep_recent` メッセージは原文保持。
  要約は約束・決定事項・未完の話題を必ず保持する(要約プロンプトの契約)。
- **実行主体**: 要約は**補助LLM**(D-15、P-524)が生成する。会話用モデルを
  使わないのは、コンパクションがモデルの意思とは無関係にハーネスが起こす
  内部処理だからである。ハーネス内部処理であってツールではないので、
  ツールレジストリには現れない(D-10)。
- **記録**: 成功時のみ、同一トランザクションで `session/summary` +
  `compaction/applied`(範囲指定)。元メッセージ行は削除しない
  (投影が置き換える、L-3)。`compaction/start` / `compaction/end` は書かない。
- **通知**: `context_compressed` をライフサイクルイベントで通知。
  表層UIには出さない(機構の詳細は表層の深さに含まれない、D-11)。
  詳細画面には出る。
- **失敗**: 要約生成の失敗は compaction 自体の失敗。履歴は原文のまま
  保たれ、次ターンで再試行する。履歴の破壊は起きない。
- **セッションは分割しない**: 長い会話への対処は compaction であって
  セッション分割ではない(D-8)。話題が変わっても履歴は続く。

### prune(compaction 前段の大出力刈り込み)

compaction は「古くなった履歴を要約する」が、**直近の大きなツール出力**は
要約されず原文保持される(keep_recent)。ここが窓を圧迫する主因である
(opencode の prune に倣う)。prune は compaction の**前段**として走る:

1. 履歴を新しい側から遡り、`tool/result` の原文トークンを積算する。
2. 累積が `prune.protect_tokens` 以内の出力は**保護**(原文保持)。
3. それより古い `tool/result` は、`prune.keep_chars` の先頭+
   「以降は spill 参照」の注記で**置き換える**。本体は既に
   content-addressed ファイルにある([§5](context-assembly.md#5-spill) の
   spill と同じストア)。
4. 刈り込み量が `prune.minimum_tokens` 未満なら何もせず終わる
   (小刻みな書き換えを避ける)。
5. 置換は `tool/pruned` イベント(call_id・from_seq)で記録し、
   投影は置換後のブロックを使う。元行は消さない(L-3)。

- prune は**compaction と同じ操作**として走る。単独コマンドにはしない
  (閾値管理が二重になる)。
- 「保護されたツール」(skill 展開の結果等、再取得が不能なもの)は
  累積に関係なく保護する(リスト: 設定 `prune.protected_tools`)。
- **v1.0 の耐久性**: 成功コミット前にプロセスが死んだら、未確定の要約は
  消える。`compaction/applied` が無いので投影は原文のまま。次回の閾値で
  やり直せば済む。起動時に畳みかけを検出する必要はない。
  ロック括弧と効果のサンドウィッチは後継である
  ([operations.md](operations.md)、[durability.md](durability.md)、P-525)。

## 8. 設定キー

具体的な数値は実装しながら決める(D-29)。トークン閾値はモデルの窓サイズと
実測の圧迫具合に依存するので、文書に固定すると嘘になる。

| キー | 説明 |
|---|---|
| `harness.context.response_reserve_tokens` | 応答予約 |
| `harness.context.safety_margin_ratio` | 安全マージン |
| `harness.context.token_estimation` | `auto\|chars4\|cjk15` |
| `compaction.trigger_ratio` | 有効窓に対する起動比 |
| `compaction.wait` | `on_idle\|immediate` |
| `compaction.keep_recent` | 原文保持するメッセージ数 |
| `compaction.prune.enabled` | prune 前段の有効化 |
| `compaction.prune.protect_tokens` | 直近ツール出力の保護枠 |
| `compaction.prune.minimum_tokens` | 刈り込み実行の最小効果 |
| `compaction.prune.keep_chars` | 刈り込み後の残存文字数 |
| `compaction.prune.protected_tools` | 無条件保護のツール |
| `harness.tool_output.soft_limit_tokens` | spill 候補閾値 |
| `harness.tool_output.hard_limit_tokens` | spill 必須閾値 |

## 9. 障害モード

| 障害 | 挙動 |
|---|---|
| Source の loader が継続失敗 | Unavailable 扱い(§2)。失敗はライフサイクルイベントで報告し、しきい値(連続10回)で Source を無効化して警告 |
| Snapshot とログの不整合 | 投影ハッシュ照合(dev assert)で検出。検出時は該当 Source の Snapshot を強制リセットし、次境界で全量再送 |
| compaction 中に新たな発話 | 進行中ターンがあれば compaction は待ってから走る。待機中の発話は inbox に入る |
| spill 書き込み失敗 | ツール結果は切り詰め版(先頭 2000 トークン+切り詰め注記)で履歴化し、`tool/result` に `degraded: true` を付す |

---

- 前: [session-log.md](session-log.md) / 次: [agent-loop.md](agent-loop.md)
