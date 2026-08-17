# Broker(資源仲介)

> 実現する要件: **P-902**(Broker 仲介)、P-604(外部連携の資格情報注入。
> 連携自体は MCP に委ねるが、鍵の受け渡しはここを通る)、
> P-907(資格情報ボールト接点)、P-1006(サイドカー)。

プラグインは原則としてファイル・ネットワーク・資格情報に**直接**
アクセスできない。すべての資源操作はホストの Broker RPC を通る。
Broker は「何ができるか」(capability)と「何を承認したか」(plane の判断)を
照合し、FD/ハンドルを委譲する唯一の窓口である。

## 1. 責務と責務外

- **責務**: 資源操作の仲介、FD/ハンドルの委譲と回収、資格情報の
  必要最小限注入、同梱アセットの素通り、操作の監査記録。
- **責務外**: 承認の**判断**(ポリシー・AI・ポップアップ)は
  [../security/approval.md](../security/approval.md) の承認 plane。
  Broker は plane の決定を**適用**する。サンドボックス強制は
  [../security/sandbox.md](../security/sandbox.md)。

## 2. Broker RPC メソッド一覧

呼び出しは [ipc.md](ipc.md) の capability 副プロトコル
(`request{ method, params, capability_ref }`)で届く。
`capability_ref` は hello で付与された能力の参照であり、
宣言外の操作は即 `denied`。

### ファイル

| method | params | 返却 | 承認 |
|---|---|---|---|
| `fs.open_read` | `path` | `fd`(読み取り専用) | ポリシー依存 |
| `fs.open_write` | `path`, `create: bool`, `truncate: bool` | `fd`(書き込み専用) | ポリシー依存(書き込みは既定で承認要) |
| `fs.list` | `dir`, `depth` | エントリ列 | ポリシー依存 |
| `fs.stat` | `path` | メタデータ | 自動(読み取り同等) |
| `fs.watch` | `path` | ストリーム(イベント) | ポリシー依存 |

- パスは必ずホスト側で正規化し、`..`・シンボリックリンクによる
  スコープ逸脱を検査する。スコープ: `workspace`(内部 workspace 根)/
  `home`(ユーザー領域の許可済み接頭辞)/ `assets_bypass`(同梱アセット)。
- `assets_bypass` スコープの読み取りは plane を素通り(承認なし)。
  manifest 列挙パスとの厳密一致のみ。

### ネットワーク

| method | params | 返却 | 承認 |
|---|---|---|---|
| `net.connect` | `url`, `method`, `headers`, `body?` | ストリーム fd(要求/応答) | ポリシー依存(接続先ホスト単位) |
| `net.fetch` | `url`, `limits` | 本体バイト列(上限付き) | ポリシー依存 |

- 直接ソケットは与えない。HTTP(S)はホスト側のクライアントが代行し、
  ストリームを fd で返す。SSRF 防護(私的アドレス帯の既定拒否)は
  ホスト側で強制。

### 資格情報

| method | params | 返却 | 承認 |
|---|---|---|---|
| `cred.get` | `credential_id` | **値を返さない**。代わりに `inject_ref` | 初回は承認、以降はポリシー |
| `cred.inject` | `inject_ref`, `target: header\|env\|bearer` | 注入済み要求の代行 | — |

- 資格情報の平文はプラグインプロセスに渡さない。ネットワーク操作と
  組み合わせ、ホスト側で注入して代行する(P-907)。
- 例外: プロバイダプラグインと MCP ブリッジの API キーは、
  自分で HTTP を行う必要があるため `cred.export`
  (承認必須+監査+有効期限付き)で注入できる。
  外部サービス連携を MCP に委ねた(D-23)結果、この経路を通る鍵が
  増える。設定ファイルに平文で書かせないことがこの経路の存在理由なので、
  MCP サーバーの設定も例外にしない。

### プロセス(サイドカー)

| method | params | 返却 | 承認 |
|---|---|---|---|
| `proc.spawn_sidecar` | `binary_ref`, `args`, `port_policy` | `sidecar_handle` | 起動は承認、以降は自動 |
| `proc.kill_sidecar` | `sidecar_handle` | — | 自動 |
| `proc.health` | `sidecar_handle` | 状態 | 自動 |

- サイドカー(llama-server 等)はプラグイン自身ではなく**ホストが**
  spawn・監視・kill する(P-1006)。ループバックポートの割当はホスト管理。
- バイナリ解決順: 設定パス → CAS アーティファクト → バンドル内 → 拒否。
  任意 URL からのダウンロードはしない。

## 3. 委譲のライフサイクル

```text
broker.request → plane 判断 → broker.grant{ grant_id, fds }
  → プラグイン使用 → broker.release / 期限切れ / プラグイン死亡
```

- 全委譲は `grant_id` で追跡し、プラグイン死亡時にホストが全回収する。
- 委譲には**有効期限**(ファイル・資格情報は時間、ストリームは接続寿命)が
  あり、延長は再要求。
- 各委譲は監査ログに記録(method・対象・grant_id・プラグイン id・承認経路)。

## 4. 障害モード

| 障害 | 挙動 |
|---|---|
| capability 未宣言の操作 | 即 `denied`。監査ログに「宣言外要求」として記録(攻撃検知の材料) |
| plane が保留中(ポップアップ待ち) | 要求は保留キューに入り、決定までブロック。プラグイン側は `tool.progress` で待機を報告すべき |
| fd 回収漏れ | 期限切れで強制 close。連続漏れはプラグインの欠陥としてライフサイクル警告 |
| 委譲中の承認取り消し | 進行中操作は完了させるが、次の委譲から `denied`。取り消しは監査ログに記録 |

## 5. 設定キー

数値は実装しながら決める(D-29)。

| キー | 説明 |
|---|---|
| `broker.fs.default_scope` | 未指定時の fs スコープ(既定は `workspace`) |
| `broker.net.private_ranges` | 私的アドレス帯の既定(`deny\|allow\|ask`。既定 `deny`) |
| `broker.grant.file_ttl` | ファイル委譲の有効期間 |
| `broker.cred.ttl` | 資格情報 export の有効期間 |

---

- 前: [manifest-and-profile.md](manifest-and-profile.md) / 次: [../tools/README.md](../tools/README.md)

