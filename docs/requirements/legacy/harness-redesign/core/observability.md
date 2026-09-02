# ローカル可観測性

> 実現する要件: **P-517**(ローカル可観測性)、P-510 の診断面、P-910(プライバシー既定)の両立。
> 参照: pi §5.8(telemetry)のスパン語彙とコンテンツ禁止規則を、ローカル限定で適合させる。

「何が起きたか」の事実の記録は3層に分かれる。混同しない。

| 層 | 内容 | 権威 |
|---|---|---|
| **永続ログ** | セッションの事実(entries)・使用量(usage) | あり。投影の源 |
| **構造化ログ**(tracing) | プロセスの診断。人間が読む | なし。再起動で消えてよい |
| **スパン計測** | 時間・回数・待ちの計量。性能と詰まりの診断 | なし。リングバッファ |

## 1. 責務と責務外

- **責務**: スパンの語彙と親子規則、属性のプライバシー規則、
  ローカル保存、開発 CLI からの閲覧、構造化ログとの分担。
- **責務外**: 外部への送信(**しない**、P-910)。UI の「今何をしているか」
  表示はライブバスの役割([../platform/server-api.md §3](../platform/server-api.md#3-websocketイベント購読))。

## 2. スパン語彙

必須スパン(親子は実際の駆動のネストに従う):

```text
ene.harness.operation   (run | compaction の受理から終端まで)
ene.harness.step        (モデル要求+ツール実行の1単位。再試行は子スパンで相関)
ene.harness.tool        (実実行または安全リプレイされたツール効果ごと)
ene.harness.waterfall   (waterfall ハンドラの実行)
ene.harness.wait        (ask-user・承認待ち・retry バックオフ等の待機)
ene.session.write       (ストレージトランザクションごと)
ene.provider.request    (LLM/STT/TTS/embedding のプロバイダ要求ごと)
ene.plugin.ipc          (IPC 往復)
```

- `ene.session.write` の開始属性は**項目数と項目種別**
  (`entry` / `usage` / `register`)。終了属性は最初と最後の seq。
  呼び出し側は lane/operation id を渡せる。ストレージ側は
  **ペイロードから推測しない**。書き込みが起きなかった
  条件付きコミットにはスパンを出さない。
- 合成セットルメントと blocked/invalid ツールには provider/tool スパンを出さない。
- 再試行・回復の相関は `step_id`/`op_id` 属性で行う。
- コンパニオン固有の計量(感情裁定の遅延・能動発話のゲート判定)は
  ドメイン側が `ene.companion.*` 名前空間で追加できる。規則は同じ。

## 3. プライバシー規則(絶対)

スパン属性に載せてよいのは、**宣言された id・名前・個数・時間・
状態・usage の数値**のみ。載せてはならないもの:

- プロンプト・補完・内面テキスト
- ツールの引数・結果・ファイル内容
- プロバイダのペイロード・ヘッダー・ハンドル・資格情報

コンテンツを含む診断が必要な場面(委譲の子のデバッグ等)は、
**診断専用の経路**を別に持ち、開発者限定で閲覧する
([../platform/clients.md](../platform/clients.md) の `ene debug`)。
委譲の子セッションは永続だが**診断専用**であり、ユーザー可視
チャネルには絶対に出ない([delegation.md](delegation.md)、
[visibility.md](visibility.md))。
この分離が「テレメトリなし」(P-910)と「診断できる」(P-517)の両立点。

## 4. 保存と閲覧

- 保存: メモリ内リングバッファ(既定 10 万スパン)+
  `<data>/diag/spans.<date>.jsonl` への追記(既定 3 日保持、
  `core.diag.retention_days`)。**永続ログとは別ファイル**。
- 閲覧:
  - CLI: `ene debug spans`(期間/レーン/種別の絞り込み、
    ツリー表示)。
  - HTTP: `GET /diag/spans`(ローカル認証、開発用途。
    [../platform/server-api.md](../platform/server-api.md) の system 領域に属する)。
- コスト/トークンの集計はスパンでなく **usage 台帳**から出す
  ([storage-model.md §4](storage-model.md#4-usage-ledgerコスト台帳))。
  スパンは台帳の代替にならない。

## 5. 設定キーと既定値

| キー | 既定 | 説明 |
|---|---|---|
| `core.diag.enabled` | `true` | 計測の有効化。off でも構造化ログは出る |
| `core.diag.ring_size` | `100000` | メモリ内リング(スパン数) |
| `core.diag.retention_days` | `3` | jsonl の保持日数 |

## 6. 障害モード

| 障害 | 挙動 |
|---|---|
| リング満杯 | 最古から破棄。破棄カウンターを構造化ログに |
| diag ファイル書き込み失敗 | 計測を継続し、書き込みのみ抑止(診断が本体を止めない) |
| 属性にコンテンツが混入 | 型が防ぐ(属性は宣言されたフィールドのみ)。CI の走査で検知 |

---

- 前: [lane-api.md](lane-api.md) / 次: [../plugins/README.md](../plugins/README.md)
