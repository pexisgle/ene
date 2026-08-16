# 承認 plane

> 実現する要件: **P-903..P-906**(承認)、**P-907**(資格情報ボールト)、
> **P-908**(監査ログ)、P-211(記憶の承認キュー)。

## 1. 設計の立場(確定済み決定)

- 承認は「事前ポリシー + AI による自動承認 + ポップアップ」の3層。
- 粗粒度のモード(すべて確認 / AI 自動承認 / すべて承認)を
  事前設定でき、ポリシーは対話からも追加できる。
- 自動承認は「確認の手間」を省くだけで、検証(署名・ハッシュ・
  サンドボックス)を無効化しない(vision.md 設計原則5)。
- plane は Broker([../plugins/broker.md](../plugins/broker.md))の
  決定を**適用**する。Broker が窓口、plane が判断。

## 2. ポリシー(P-903)

### 粗粒度モード

soul ごと(またはグローバル)に1つのモードを持つ。

| モード | 挙動 |
|---|---|
| `ask_all` | read_only 以外の全操作を確認 |
| `policy`(既定) | ポリシーに合致すれば自動、しなければポップアップ |
| `ai_auto` | ポリシー合致→自動、不合致→AI 判断(§3) |
| `auto` | read_only は自動、それ以外も自動(上級者向け) |

### ポリシー規則(DSL)

ポリシーは「ツール名+対象」の規則列。YAML/TOML で記述。

```yaml
rules:
  # fs 系: workspace 内の読み書きは自動
  - tool: "fs.*"
    scope: "workspace"
    decision: allow
  # exec: 承認必須(対象を限定しても)
  - tool: "exec.*"
    decision: ask
  # web: 取得は自動、送信系は確認
  - tool: "web.fetch"
    decision: allow
  - tool: "mail.send"
    decision: ask
  # ワイルドカード+プレフィックス
  - tool: "mcp:*"
    scope_prefix: "calendar."
    decision: allow
```

- 評価は**先頭一致**。合致しなければモードに従う。
- `decision` は `allow`(自動実行)/ `ask`(ポップアップ)/
  `ai`(AI 判断)/ `deny`(禁止)。
- `scope` は対象のスコープ(workspace/home 等)、`scope_prefix` は
  対象文字列のプレフィックス(URL・パス・宛先)。
- 規則の変更は監査ログに残る。

## 3. AI 自動承認(P-904)

- モード `ai_auto`、または規則の `decision: ai` のとき、
  承認判断をモデルに委ねる。
- 判断は専用の軽量モデル(`ai.tasks.approve`)が、
  「操作の内容・対象・副作用・文脈(直近の対話)」を見て
  allow/deny+理由を返す。
- **判断の理由**は必ず監査ログに残す(P-904 の受入条件)。
- deny はポップアップに繰り上げ(ユーザーが上書き可能)。
- AI 判断は**高リスク操作には使わない**(exec の破壊的コマンド、
  資格情報の export、外部送信)。高リスクは常にポップアップ
  (§4 のリスク分類)。

## 4. リスク分類とポップアップ(P-905)

ツールは `side_effects`([../tools/registry.md §2](../tools/registry.md#2-tooldefinition))
からリスクを導く。

| リスク | 例 | 既定の扱い |
|---|---|---|
| なし(read_only) | fs.read, web.fetch | ポリシー次第で自動 |
| 低 | workspace 内の fs.write | ポリシー次第 |
| 中 | 外部送信(mail.send)、スケジュール作成 | ポリシー or ポップアップ |
| 高 | exec の破壊的コマンド、資格情報 export、共有スペース作成 | 常にポップアップ(AI 判断なし) |

- ポップアップは**全クライアントに配信**され、最も早い応答を採用
  ([../platform/clients.md §3](../platform/clients.md))。
- ポップアップには「この種は今後自動にする」の選択肢を付け、
  選択するとポリシー規則の追加提案になる(§5)。
- タイムアウト(既定 5 分、`approval.popup.timeout`)で deny。
- **internal 委譲由来のポップアップは匿名化する**(P-508/P-520、
  [../core/visibility.md](../core/visibility.md)): 操作の内容・対象・
  リスクは表示するが、「どのサブエージェントが」を示す文言は載せない。
  承認の判断自体は通常通り行える(委譲の存在秘匿と承認の機能は両立する)。

## 5. 対話からのポリシー追加(P-906)

- ユーザーが対話で「次から確認しないで」「これは勝手にやっていい」と
  言ったとき、soul は内部ツール `approval.policy_add` を呼ぶ。
- plane は提案された規則を**ポップアップで確認**してから追加する
  (対話だけで勝手にポリシーを変えない)。
- 追加された規則は監査ログに「対話由来」として記録。
- ポップアップの「今後自動」選択も同じ経路(提案→確認→追加)。
- ユーザーは設定 UI でポリシーを一覧・編集・削除できる。

## 6. 監査ログ(P-908)

`<data>/audit.db`(専用 DB、append-only)。

| 記録対象 | フィールド |
|---|---|
| ツール実行 | ts, soul_id, tool, args の要約, decision, mode, policy_ref?, reason?, duration |
| 承認判断 | 上記+応答クライアント、AI の理由 |
| ポリシー変更 | 変更内容、由来(対話/ポップアップ/手動)、確認の有無 |
| 設定変更 | キー、新旧値、由来 |
| キャラ/プラグインのインストール | digest、署名、確認の有無 |
| 記憶の削除/秘匿 | 対象、由来 |

- 監査ログは改ざん検知のため hash chain(前行ハッシュを保持)。
- 閲覧は設定 UI と `GET /audit` API([../platform/server-api.md](../platform/server-api.md))。

## 7. 資格情報ボールト(P-907)

- API キー等を `<data>/vault`(OS キーリング優先、フォールバックは
  パスフレーズ派生キーの暗号化ファイル)に保管。
- プラグインには平文を渡さない。Broker の `cred.inject` で
  ホスト側が注入して代行([../plugins/broker.md §2](../plugins/broker.md))。
- プロバイダの `cred.export` は例外で、承認必須+有効期限付き。
- ボールトのロック解除は起動時の1回(キーリング)または
  パスフレーズ入力。

## 8. 記憶の承認キュー(P-211)

- 記憶抽出の低信頼/機密候補は承認キューへ
  ([../companion/memory.md §7](../companion/memory.md#7-裁定p-205))。
- plane のポップアップと同じ UI 面で「覚えていい?」を確認。
- 承認/拒否は監査ログに記録。

## 9. プライバシー(P-910)

- テレメトリなし。ローカルファースト。
- エクスポートには秘匿処理([../core/session-log.md §8](../core/session-log.md#8-エクスポートp-110))。
- クラウド LLM を使う場合、送信内容に資格情報が混入しないよう
  送信前に既知パターンをスキャン(漏洩防止)。

## 10. 障害モード

| 障害 | 挙動 |
|---|---|
| ポップアップの応答なし | タイムアウトで deny。対話で「さっきの確認、どうする?」と再提示はしない(ユーザーの意思を尊重) |
| AI 判断モデルの不能 | ポップアップに繰り上げ+警告 |
| ポリシーの矛盾(複数規則合致) | 先頭一致。矛盾は設定の検証で警告 |
| 監査ログの書き込み失敗 | 操作自体を**拒否**(監査が残らない操作は実行しない、データ保全優先) |
| キーリング不可環境 | パスフレーズフォールバック。それもなければ資格情報を使う機能を無効化+通知 |

## 11. 設定キーと既定値

| キー | 既定 | 説明 |
|---|---|---|
| `approval.mode` | `policy` | 粗粒度モード |
| `approval.popup.timeout` | `300` | ポップアップタイムアウト(秒) |
| `approval.ai.model` | `auto` | AI 判断のプロバイダ(`ai.tasks.approve`) |
| `approval.policy_file` | `<data>/policy.yaml` | ポリシー規則 |
| `approval.audit.db` | `<data>/audit.db` | 監査 DB |
| `vault.backend` | `auto` | `auto`(キーリング優先)/ `file` |

---

- 前: [sandbox.md](sandbox.md) / 次: [../migration.md](../migration.md)
