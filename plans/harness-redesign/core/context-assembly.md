# コンテキスト組立

> 実現する要件: **P-505**(コンテキスト組立)、**P-506**(compaction)、
> **P-507/P-614**(spill/巨大出力)。

モデル要求に載せるコンテキストは、**System Context**(指示・状況)と
**Session History**(会話履歴)に分離する(opencode に倣う)。
System Context は Context Source のレジストリから組み立てられ、
Session History は [session-log.md](session-log.md) の投影から得られる。
表層と裏層は同じ組立機構を使うが、載せる Source と履歴が違う
([../product/vision.md](../product/vision.md#51-コアデーモン内の2層))。
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
**固定の描画順**(下表)に従う。

```text
描画順(System Context):
 1. platform_contract        (出力契約・安全規則)
 2. identity_kernel          (キャラ定義。予算: 窓の 1/8、400..4000 トークンでクランプ)
 3. character_state          (感情要約)
 4. memory.semantic          (想起した記憶)
 5. memory.user_profile
 6. memory.commitments       (有効な約束)
 7. workspace.context        (job/成果物の関連チャンク)
 8. skills.active            (読み込み済みスキル)
 9. mcp.resources            (MCP resources からの変換分)
10. scene_state              (転がり要約)
11. inner_recent             (内面自己参照窓)
12. style_examples           (発話スタイル例)
13. interruption_note        (中断後のみ)
14. delegation.active        (進行中委譲の状態・進捗・未回答質問の要約、
                             [delegation.md](delegation.md)。親は status を
                             呼ばなくても境界で現状を知る)
```

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

- **起動**: 履歴トークンが有効窓の `compaction.trigger_ratio`(既定 0.75)を
  超えたとき。**先回り実行**(設定 `compaction.wait`: 既定 `on_idle`)で
  ターン中のストールを避ける。
- **内容**: 古いメッセージ群を**転がり scene summary** に畳む。
  直近 `compaction.keep_recent`(既定 12)メッセージは原文保持。
  要約は約束・決定事項・未完の話題を必ず保持する(要約プロンプトの契約)。
- **記録**: `session/summary` + `compaction/applied`(範囲指定)。
  元メッセージ行は削除しない(投影が置き換える、L-3)。
- **通知**: `context_compressed` をライフサイクルイベントで通知。
  UI では「記憶の整理」として最小限に表示する(機構の詳細は秘匿)。
- **失敗**: 要約生成の失敗は compaction 自体の失敗。履歴は原文のまま
  保たれ、次ターンで再試行する。履歴の破壊は起きない。

### prune(compaction 前段の大出力刈り込み)

compaction は「古くなった履歴を要約する」が、**直近の大きなツール出力**は
要約されず原文保持される(keep_recent)。ここが窓を圧迫する主因である
(opencode の prune に倣う)。prune は compaction の**前段**として走る:

1. 履歴を新しい側から遡り、`tool/result` の原文トークンを積算する。
2. 累積が `prune.protect_tokens`(既定 40000)以内の出力は**保護**(原文保持)。
3. それより古い `tool/result` は、`prune.keep_chars`(既定 2000 文字)の
   先頭+「以降は spill 参照」の注記で**置き換える**。
   本体は既に content-addressed ファイルにある
   ([§5](context-assembly.md#5-spill) の spill と同じストア)。
4. 刈り込み量が `prune.minimum_tokens`(既定 20000)未満なら何もせず終わる
   (小刻みな書き換えを避ける)。
5. 置換は `tool/pruned` イベント(call_id・from_seq)で記録し、
   投影は置換後のブロックを使う。元行は消さない(L-3)。

- prune は**compaction と同じ操作**(CompactionState の phase として走る)。
  単独コマンドにはしない(閾値管理が二重になる)。
- 「保護されたツール」(skill 展開の結果等、再取得が不能なもの)は
  累積に関係なく保護する(リスト: 設定 `prune.protected_tools`、既定 `[skill]`)。
- **耐久機構**: compaction は構造変更の操作であり、ロック括弧・準備の
  永続化・効果のサンドウィッチ・失敗分類は [operations.md §5](operations.md#5-compactionstate構造変更)
  と [durability.md](durability.md) が定義する。この文書は「何をどう畳むか」のみ。

## 8. 設定キーと既定値

| キー | 既定 | 説明 |
|---|---|---|
| `harness.context.response_reserve_tokens` | `4096` | 応答予約 |
| `harness.context.safety_margin_ratio` | `0.10` | 安全マージン |
| `harness.context.token_estimation` | `auto` | `auto\|chars4\|cjk15` |
| `compaction.trigger_ratio` | `0.75` | 起動比 |
| `compaction.wait` | `on_idle` | `on_idle\|immediate` |
| `compaction.keep_recent` | `12` | 保持メッセージ数 |
| `compaction.prune.enabled` | `true` | prune 前段の有効化 |
| `compaction.prune.protect_tokens` | `40000` | 直近ツール出力の保護枠 |
| `compaction.prune.minimum_tokens` | `20000` | 刈り込み実行の最小効果 |
| `compaction.prune.keep_chars` | `2000` | 刈り込み後の残存文字数 |
| `compaction.prune.protected_tools` | `[skill]` | 無条件保護のツール |
| `harness.tool_output.soft_limit_tokens` | `8000` | spill 候補閾値 |
| `harness.tool_output.hard_limit_tokens` | `32000` | spill 必須閾値 |

## 9. 障害モード

| 障害 | 挙動 |
|---|---|
| Source の loader が継続失敗 | Unavailable 扱い(§2)。失敗はライフサイクルイベントで報告し、しきい値(連続10回)で Source を無効化して警告 |
| Snapshot とログの不整合 | 投影ハッシュ照合(dev assert)で検出。検出時は該当 Source の Snapshot を強制リセットし、次境界で全量再送 |
| compaction 中に新たな発話 | compaction は境界を跨がない(境界でロック)。待機中の発話はキューイング |
| spill 書き込み失敗 | ツール結果は切り詰め版(先頭 2000 トークン+切り詰め注記)で履歴化し、`tool/result` に `degraded: true` を付す |

---

- 前: [session-log.md](session-log.md) / 次: [agent-loop.md](agent-loop.md)
